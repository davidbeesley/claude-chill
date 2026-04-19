use crate::escape_sequences::{
    ALT_SCREEN_ENTER, ALT_SCREEN_ENTER_LEGACY, ALT_SCREEN_EXIT, ALT_SCREEN_EXIT_LEGACY,
    BRACKETED_PASTE_DISABLE, BRACKETED_PASTE_ENABLE, BRACKETED_PASTE_END, BRACKETED_PASTE_START,
    CLEAR_SCREEN, CURSOR_HOME, INPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_CAPACITY, SYNC_BUFFER_CAPACITY,
    SYNC_END, SYNC_START,
};
use crate::history_filter::HistoryFilter;
use crate::line_buffer::LineBuffer;
use crate::platform::{self, PlatformSignal, PollResult, Pty, PtyReadError, RawModeGuard};
use anyhow::{Context, Result};
use log::debug;
use memchr::memmem;
use std::time::{Duration, Instant};
use termwiz::escape::Action;
use termwiz::escape::csi::{CSI, Keyboard};
use termwiz::escape::parser::Parser as TermwizParser;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceMatch {
    Complete,
    Partial,
    None,
}

pub struct ProxyConfig {
    pub max_history_lines: usize,
    pub lookback_key: String,
    pub lookback_sequence_legacy: Vec<u8>,
    pub lookback_sequence_kitty: Vec<u8>,
    pub auto_lookback_timeout_ms: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            max_history_lines: 100_000,
            lookback_key: "[ctrl][6]".to_string(),
            lookback_sequence_legacy: vec![0x1E],
            lookback_sequence_kitty: b"\x1b[54;5u".to_vec(),
            auto_lookback_timeout_ms: 15000,
        }
    }
}

const RENDER_DELAY_MS: u64 = 5;
const SYNC_BLOCK_DELAY_MS: u64 = 50;

pub struct Proxy {
    config: ProxyConfig,
    pty: Pty,
    original_terminal_state: Option<platform::OriginalTerminalState>,
    history: LineBuffer,
    history_filter: HistoryFilter,
    vt_parser: vt100::Parser,
    vt_prev_screen: Option<vt100::Screen>,
    last_output_time: Option<Instant>,
    last_render_time: Option<Instant>,
    last_stdin_time: Option<Instant>,
    last_auto_lookback_time: Option<Instant>,
    auto_lookback_timeout: Duration,
    sync_buffer: Vec<u8>,
    in_sync_block: bool,
    in_lookback_mode: bool,
    in_alternate_screen: bool,
    in_bracketed_paste: bool,
    kitty_mode_supported: bool,
    kitty_mode_stack: u32,
    kitty_output_parser: TermwizParser,
    vt_render_pending: bool,
    lookback_cache: Vec<u8>,
    lookback_input_buffer: Vec<u8>,
    output_buffer: Vec<u8>,
    sync_start_finder: memmem::Finder<'static>,
    sync_end_finder: memmem::Finder<'static>,
    clear_screen_finder: memmem::Finder<'static>,
    cursor_home_finder: memmem::Finder<'static>,
    alt_screen_enter_finder: memmem::Finder<'static>,
    alt_screen_exit_finder: memmem::Finder<'static>,
    alt_screen_enter_legacy_finder: memmem::Finder<'static>,
    alt_screen_exit_legacy_finder: memmem::Finder<'static>,
    paste_start_finder: memmem::Finder<'static>,
    paste_end_finder: memmem::Finder<'static>,
    pty_drain_buffer: Vec<u8>,
    paste_remainder: Vec<u8>,
}

/// Returns (supported, initial_flags) - if flags > 0, terminal is already in Kitty mode
fn detect_kitty_support() -> (bool, u32) {
    use std::io::Write;
    use termwiz::escape::csi::Device;

    // Query sequences:
    // CSI ? u       - Kitty keyboard protocol query
    // CSI c         - Primary Device Attributes (all terminals respond)
    const KITTY_QUERY: &[u8] = b"\x1b[?u";
    const DA_QUERY: &[u8] = b"\x1b[c";

    // Send both queries
    let stdout = std::io::stdout();
    let mut stdout_lock = stdout.lock();
    if stdout_lock.write_all(KITTY_QUERY).is_err() {
        return (false, 0);
    }
    if stdout_lock.write_all(DA_QUERY).is_err() {
        return (false, 0);
    }
    if stdout_lock.flush().is_err() {
        return (false, 0);
    }
    drop(stdout_lock);

    // Read responses with timeout using termwiz parser
    let mut parser = TermwizParser::new();
    let mut buf = [0u8; 256];
    let mut kitty_supported = false;
    let mut kitty_flags: u32 = 0;
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_millis(500);

    while start.elapsed() < timeout {
        match platform::poll_stdin(50) {
            Ok(false) => continue,
            Ok(true) => {
                match platform::read_stdin(&mut buf) {
                    Ok(0) => continue,
                    Ok(n) => {
                        let actions = parser.parse_as_vec(&buf[..n]);
                        for action in actions {
                            if let Action::CSI(csi) = action {
                                match csi {
                                    CSI::Keyboard(Keyboard::ReportKittyState(flags)) => {
                                        kitty_supported = true;
                                        kitty_flags = u32::from(flags.bits());
                                    }
                                    CSI::Device(dev)
                                        if matches!(*dev, Device::DeviceAttributes(_)) =>
                                    {
                                        // DA response means all responses received
                                        debug!(
                                            "Kitty detection complete: supported={} flags={}",
                                            kitty_supported, kitty_flags
                                        );
                                        return (kitty_supported, kitty_flags);
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(_) => break,
                }
            }
            Err(_) => continue,
        }
    }

    debug!(
        "Kitty detection timed out, supported={} flags={}",
        kitty_supported, kitty_flags
    );
    (kitty_supported, kitty_flags)
}

impl Proxy {
    pub fn spawn(command: &str, args: &[&str], config: ProxyConfig) -> Result<Self> {
        let term_size = platform::get_terminal_size()?;

        let raw_mode_guard = RawModeGuard::new()?;
        platform::setup_signal_handlers()?;

        // Detect Kitty support before spawning child
        // If flags > 0, terminal is already in Kitty mode (inherited from parent)
        let (kitty_supported, kitty_initial_flags) = detect_kitty_support();
        let kitty_initial_stack = if kitty_initial_flags > 0 { 1 } else { 0 };

        let pty = Pty::spawn(command, args, term_size).context("PTY spawn failed")?;

        // Enable bracketed paste on the real terminal so tmux/Ghostty wraps
        // paste content in \x1b[200~ ... \x1b[201~ markers. Without this,
        // the child app's \x1b[?2004h gets eaten by the VT renderer before
        // it reaches the terminal, and pastes arrive as plain keystrokes.
        platform::write_stdout(BRACKETED_PASTE_ENABLE)?;

        let vt_parser = vt100::Parser::new(term_size.rows, term_size.cols, 0);

        // Seed history with clear screen so replay starts fresh
        let mut history = LineBuffer::new(config.max_history_lines);
        history.push_bytes(CLEAR_SCREEN);
        history.push_bytes(CURSOR_HOME);

        let auto_lookback_timeout = Duration::from_millis(config.auto_lookback_timeout_ms);

        debug!("Proxy::spawn: command={} args={:?}", command, args);

        Ok(Self {
            history,
            history_filter: HistoryFilter::new(),
            config,
            pty,
            original_terminal_state: raw_mode_guard.take(),
            vt_parser,
            vt_prev_screen: None,
            last_output_time: None,
            last_render_time: None,
            last_stdin_time: None,
            last_auto_lookback_time: None,
            auto_lookback_timeout,
            sync_buffer: Vec::with_capacity(SYNC_BUFFER_CAPACITY),
            in_sync_block: false,
            in_lookback_mode: false,
            in_alternate_screen: false,
            in_bracketed_paste: false,
            kitty_mode_supported: kitty_supported,
            kitty_mode_stack: kitty_initial_stack,
            kitty_output_parser: TermwizParser::new(),
            vt_render_pending: false,
            lookback_cache: Vec::new(),
            lookback_input_buffer: Vec::with_capacity(INPUT_BUFFER_CAPACITY),
            output_buffer: Vec::with_capacity(OUTPUT_BUFFER_CAPACITY),
            sync_start_finder: memmem::Finder::new(SYNC_START),
            sync_end_finder: memmem::Finder::new(SYNC_END),
            clear_screen_finder: memmem::Finder::new(CLEAR_SCREEN),
            cursor_home_finder: memmem::Finder::new(CURSOR_HOME),
            alt_screen_enter_finder: memmem::Finder::new(ALT_SCREEN_ENTER),
            alt_screen_exit_finder: memmem::Finder::new(ALT_SCREEN_EXIT),
            alt_screen_enter_legacy_finder: memmem::Finder::new(ALT_SCREEN_ENTER_LEGACY),
            alt_screen_exit_legacy_finder: memmem::Finder::new(ALT_SCREEN_EXIT_LEGACY),
            paste_start_finder: memmem::Finder::new(BRACKETED_PASTE_START),
            paste_end_finder: memmem::Finder::new(BRACKETED_PASTE_END),
            pty_drain_buffer: Vec::new(),
            paste_remainder: Vec::new(),
        })
    }

    pub fn run(&mut self) -> Result<i32> {
        let mut buf = [0u8; 65536];

        loop {
            // Check for platform signals
            for signal in platform::check_signals() {
                match signal {
                    PlatformSignal::Resize => {
                        self.forward_winsize()?;
                    }
                    PlatformSignal::Interrupt => {
                        self.pty.signal(PlatformSignal::Interrupt);
                    }
                    PlatformSignal::Terminate => {
                        self.pty.signal(PlatformSignal::Terminate);
                    }
                }
            }

            let poll_timeout_ms = self
                .time_until_render()
                .map(|d| d.as_millis().min(100) as u16)
                .unwrap_or(100);

            let poll_result = platform::poll_io(&self.pty, poll_timeout_ms)?;

            match poll_result {
                PollResult::Timeout => {
                    self.flush_pending_vt_render()?;
                    self.check_auto_lookback()?;
                    continue;
                }
                PollResult::Interrupted => continue,
                PollResult::PtyHangup => break,
                PollResult::PtyReadable => {
                    self.flush_pending_vt_render()?;
                    self.handle_pty_read(&mut buf)?;
                }
                PollResult::StdinReadable => {
                    self.flush_pending_vt_render()?;
                    self.handle_stdin_read(&mut buf)?;
                }
                PollResult::BothReadable => {
                    self.flush_pending_vt_render()?;
                    // Handle PTY first, then stdin
                    if self.handle_pty_read(&mut buf)? {
                        break;
                    }
                    self.handle_stdin_read(&mut buf)?;
                }
            }
        }

        // Final render before exit
        if self.vt_render_pending {
            self.render_vt_screen()?;
        }

        self.pty.wait()
    }

    /// Handle reading from PTY. Returns true if PTY closed (should break loop).
    fn handle_pty_read(&mut self, buf: &mut [u8]) -> Result<bool> {
        match self.pty.read(buf) {
            Ok(0) => Ok(true),
            Ok(n) => {
                self.process_output(&buf[..n])?;
                Ok(false)
            }
            Err(PtyReadError::WouldBlock) => Ok(false),
            Err(PtyReadError::Eof) => Ok(true),
            Err(PtyReadError::Other(msg)) => anyhow::bail!("{}", msg),
        }
    }

    /// Handle reading from stdin. Returns true if stdin closed (should break loop).
    fn handle_stdin_read(&mut self, buf: &mut [u8]) -> Result<bool> {
        match platform::read_stdin(buf) {
            Ok(0) => Ok(true),
            Ok(n) => {
                self.process_input(&buf[..n])?;
                if !self.pty_drain_buffer.is_empty() {
                    let drained = std::mem::take(&mut self.pty_drain_buffer);
                    self.process_output(&drained)?;
                }
                Ok(false)
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => anyhow::bail!("read from stdin failed: {}", e),
        }
    }

    fn process_output(&mut self, data: &[u8]) -> Result<()> {
        self.process_output_inner(data, true)
    }

    fn process_output_inner(&mut self, data: &[u8], feed_vt: bool) -> Result<()> {
        debug!(
            "process_output: len={} in_alt={} in_lookback={} feed_vt={}",
            data.len(),
            self.in_alternate_screen,
            self.in_lookback_mode,
            feed_vt
        );

        if self.in_alternate_screen {
            // Feed VT but NOT history while in alt screen
            // Alt screen content (TUI editors, etc.) shouldn't be in lookback history
            if feed_vt {
                self.vt_parser.process(data);
            }
            return self.process_output_alt_screen(data);
        }

        if self.in_lookback_mode {
            debug!("process_output: caching {} bytes for lookback", data.len());
            self.lookback_cache.extend_from_slice(data);
            return Ok(());
        }

        // Feed data to VT emulator (unless already fed by caller)
        if feed_vt {
            self.vt_parser.process(data);
        }
        self.vt_render_pending = true;
        self.last_output_time = Some(Instant::now());

        // Process sync blocks for history management
        let mut pos = 0;
        while pos < data.len() {
            // Check for alt screen enter
            if let Some(alt_pos) = self.find_alt_screen_enter(&data[pos..]) {
                debug!(
                    "process_output: ALT_SCREEN_ENTER detected at pos={}",
                    pos + alt_pos
                );
                // Add ALL remaining data to history (including alt screen enter and content)
                // This ensures history matches VT exactly
                let remaining = &data[pos..];
                if self.in_sync_block {
                    self.sync_buffer.extend_from_slice(remaining);
                    self.flush_sync_block_to_history();
                    self.in_sync_block = false;
                } else {
                    self.push_to_history(remaining);
                }
                self.in_alternate_screen = true;
                let seq_len = self.alt_screen_enter_len(&data[pos + alt_pos..]);
                // Write alt screen enter directly
                self.write_to_terminal(&data[pos + alt_pos..pos + alt_pos + seq_len])?;
                return self.process_output_alt_screen(&data[pos + alt_pos + seq_len..]);
            }

            if self.in_sync_block {
                if let Some(idx) = self.sync_end_finder.find(&data[pos..]) {
                    debug!("process_output: SYNC_END at pos={}", pos + idx);
                    self.sync_buffer.extend_from_slice(&data[pos..pos + idx]);
                    self.sync_buffer.extend_from_slice(SYNC_END);
                    self.flush_sync_block_to_history();
                    self.in_sync_block = false;
                    pos += idx + SYNC_END.len();
                } else {
                    self.sync_buffer.extend_from_slice(&data[pos..]);
                    break;
                }
            } else if let Some(idx) = self.sync_start_finder.find(&data[pos..]) {
                debug!("process_output: SYNC_START at pos={}", pos + idx);
                // Add any data before SYNC_START to history
                if idx > 0 {
                    self.push_to_history(&data[pos..pos + idx]);
                }
                self.in_sync_block = true;
                self.sync_buffer.clear();
                self.sync_buffer.extend_from_slice(SYNC_START);
                pos += idx + SYNC_START.len();
            } else {
                // No sync block, just add to history
                self.push_to_history(&data[pos..]);
                break;
            }
        }

        Ok(())
    }

    fn process_output_alt_screen(&mut self, data: &[u8]) -> Result<()> {
        if let Some(exit_pos) = self.find_alt_screen_exit(data) {
            debug!(
                "process_output_alt_screen: ALT_SCREEN_EXIT detected at pos={}",
                exit_pos
            );
            self.write_to_terminal(&data[..exit_pos])?;
            let seq_len = self.alt_screen_exit_len(&data[exit_pos..]);
            self.write_to_terminal(&data[exit_pos..exit_pos + seq_len])?;
            self.in_alternate_screen = false;

            // Force full VT render to restore main screen content
            debug!("process_output_alt_screen: rendering VT screen after alt exit");
            self.vt_prev_screen = None;
            self.render_vt_screen()?;

            // Data after ALT_EXIT was already fed to VT and history when we processed
            // the alt screen chunk, so we just need to check for more alt screen transitions
            let remaining = &data[exit_pos + seq_len..];
            if !remaining.is_empty() {
                // Check if there's another alt screen enter in the remaining data
                if self.find_alt_screen_enter(remaining).is_some() {
                    // Need to process for alt screen detection, but skip VT/history feed
                    return self.process_output_check_alt_only(remaining);
                }
            }
            return Ok(());
        }
        self.write_to_terminal(data)
    }

    /// Check for alt screen transitions without re-feeding VT/history
    fn process_output_check_alt_only(&mut self, data: &[u8]) -> Result<()> {
        if let Some(alt_pos) = self.find_alt_screen_enter(data) {
            debug!(
                "process_output_check_alt_only: ALT_SCREEN_ENTER at pos={}",
                alt_pos
            );
            self.in_alternate_screen = true;
            let seq_len = self.alt_screen_enter_len(&data[alt_pos..]);
            self.write_to_terminal(&data[alt_pos..alt_pos + seq_len])?;
            return self.process_output_alt_screen(&data[alt_pos + seq_len..]);
        }
        Ok(())
    }

    fn find_alt_screen_enter(&self, data: &[u8]) -> Option<usize> {
        let pos1 = self.alt_screen_enter_finder.find(data);
        let pos2 = self.alt_screen_enter_legacy_finder.find(data);
        match (pos1, pos2) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn find_alt_screen_exit(&self, data: &[u8]) -> Option<usize> {
        let pos1 = self.alt_screen_exit_finder.find(data);
        let pos2 = self.alt_screen_exit_legacy_finder.find(data);
        match (pos1, pos2) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn alt_screen_enter_len(&self, data: &[u8]) -> usize {
        if data.starts_with(ALT_SCREEN_ENTER) {
            ALT_SCREEN_ENTER.len()
        } else {
            ALT_SCREEN_ENTER_LEGACY.len()
        }
    }

    fn alt_screen_exit_len(&self, data: &[u8]) -> usize {
        if data.starts_with(ALT_SCREEN_EXIT) {
            ALT_SCREEN_EXIT.len()
        } else {
            ALT_SCREEN_EXIT_LEGACY.len()
        }
    }

    fn kitty_mode_enabled(&self) -> bool {
        self.kitty_mode_stack > 0
    }

    /// Write data to the terminal and track Kitty keyboard protocol state
    fn write_to_terminal(&mut self, data: &[u8]) -> Result<()> {
        Self::update_kitty_mode_helper(
            &mut self.kitty_output_parser,
            &mut self.kitty_mode_stack,
            self.kitty_mode_supported,
            data,
        );
        platform::write_stdout(data)
    }

    fn update_kitty_mode_helper(
        parser: &mut TermwizParser,
        stack: &mut u32,
        supported: bool,
        data: &[u8],
    ) {
        let actions = parser.parse_as_vec(data);
        for action in actions {
            if let Action::CSI(csi) = action {
                match csi {
                    CSI::Keyboard(Keyboard::PushKittyState { flags, .. }) => {
                        if supported {
                            *stack = stack.saturating_add(1);
                            debug!(
                                "Kitty keyboard protocol push (flags={:?}, stack={})",
                                flags, stack
                            );
                        }
                    }
                    CSI::Keyboard(Keyboard::SetKittyState { flags, .. }) => {
                        if supported && !flags.is_empty() && *stack == 0 {
                            *stack = 1;
                            debug!(
                                "Kitty keyboard protocol set (flags={:?}, stack={})",
                                flags, stack
                            );
                        } else if flags.is_empty() && *stack > 0 {
                            debug!("Kitty keyboard protocol set empty flags (stack={})", stack);
                        }
                    }
                    CSI::Keyboard(Keyboard::PopKittyState(n)) => {
                        let prev = *stack;
                        *stack = stack.saturating_sub(n);
                        debug!(
                            "Kitty keyboard protocol pop {} (stack {} -> {})",
                            n, prev, stack
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    fn flush_sync_block_to_history(&mut self) {
        let has_clear_screen = self.clear_screen_finder.find(&self.sync_buffer).is_some();
        let has_cursor_home = self.cursor_home_finder.find(&self.sync_buffer).is_some();
        let is_full_redraw = has_clear_screen && has_cursor_home;

        debug!(
            "flush_sync_block: len={} full_redraw={}",
            self.sync_buffer.len(),
            is_full_redraw
        );

        if is_full_redraw {
            debug!("CLEARING HISTORY");
            self.history.clear();
            // Re-seed with clear screen after clearing
            self.history.push_bytes(CLEAR_SCREEN);
            self.history.push_bytes(CURSOR_HOME);
        }
        self.push_to_history(&self.sync_buffer.clone());
        self.sync_buffer.clear();
    }

    /// Push data to history, filtering out terminal query sequences that would
    /// cause the terminal to respond when replayed.
    fn push_to_history(&mut self, data: &[u8]) {
        let filtered = self.history_filter.filter(data);
        self.history.push_bytes(&filtered);
    }

    fn flush_pending_vt_render(&mut self) -> Result<()> {
        if !self.vt_render_pending || self.in_lookback_mode || self.in_alternate_screen {
            return Ok(());
        }

        let elapsed = self
            .last_output_time
            .map(|t| t.elapsed())
            .unwrap_or(Duration::MAX);

        // Wait longer if in sync block (more data likely coming)
        let delay = if self.in_sync_block {
            Duration::from_millis(SYNC_BLOCK_DELAY_MS)
        } else {
            Duration::from_millis(RENDER_DELAY_MS)
        };

        if elapsed >= delay {
            self.render_vt_screen()?;
        }

        Ok(())
    }

    fn time_until_render(&self) -> Option<Duration> {
        if !self.vt_render_pending || self.in_lookback_mode || self.in_alternate_screen {
            return None;
        }

        let elapsed = self
            .last_output_time
            .map(|t| t.elapsed())
            .unwrap_or(Duration::MAX);

        let delay = if self.in_sync_block {
            Duration::from_millis(SYNC_BLOCK_DELAY_MS)
        } else {
            Duration::from_millis(RENDER_DELAY_MS)
        };

        if elapsed >= delay {
            Some(Duration::ZERO)
        } else {
            Some(delay - elapsed)
        }
    }

    fn render_vt_screen(&mut self) -> Result<()> {
        let is_diff = self.vt_prev_screen.is_some();
        self.output_buffer.clear();
        self.output_buffer.extend_from_slice(SYNC_START);

        match &self.vt_prev_screen {
            Some(prev) => {
                // Diff-based render: only send changes
                self.output_buffer
                    .extend_from_slice(&self.vt_parser.screen().contents_diff(prev));
            }
            None => {
                // First render: full screen
                self.output_buffer
                    .extend_from_slice(&self.vt_parser.screen().contents_formatted());
            }
        }

        self.output_buffer
            .extend_from_slice(&self.vt_parser.screen().cursor_state_formatted());
        self.output_buffer.extend_from_slice(SYNC_END);

        debug!(
            "render_vt_screen: diff={} output_len={}\n",
            is_diff,
            self.output_buffer.len()
        );
        // Can't use write_to_terminal here due to borrow checker - can't pass
        // &self.output_buffer while also taking &mut self
        Self::update_kitty_mode_helper(
            &mut self.kitty_output_parser,
            &mut self.kitty_mode_stack,
            self.kitty_mode_supported,
            &self.output_buffer,
        );
        platform::write_stdout(&self.output_buffer)?;

        // Store current screen for next diff
        self.vt_prev_screen = Some(self.vt_parser.screen().clone());
        self.vt_render_pending = false;
        self.last_render_time = Some(Instant::now());
        Ok(())
    }

    fn check_auto_lookback(&mut self) -> Result<()> {
        if self.auto_lookback_timeout.is_zero() {
            return Ok(());
        }
        if self.in_lookback_mode || self.in_alternate_screen {
            return Ok(());
        }

        // Check if enough time has passed since last stdin activity
        let Some(stdin_time) = self.last_stdin_time else {
            return Ok(());
        };
        if stdin_time.elapsed() < self.auto_lookback_timeout {
            return Ok(());
        }

        // Check if there's been new output since last auto-lookback
        // AND enough time has passed since we last dumped
        let Some(render_time) = self.last_render_time else {
            return Ok(());
        };
        if let Some(last_auto) = self.last_auto_lookback_time {
            let no_new_output = render_time <= last_auto;
            let too_soon = last_auto.elapsed() < self.auto_lookback_timeout;
            if no_new_output || too_soon {
                return Ok(());
            }
        }

        debug!(
            "auto_lookback triggered: stdin_idle={}ms render_age={}ms last_auto_age={}ms",
            stdin_time.elapsed().as_millis(),
            render_time.elapsed().as_millis(),
            self.last_auto_lookback_time
                .map(|t| t.elapsed().as_millis())
                .unwrap_or(0)
        );
        self.dump_history()?;
        self.last_auto_lookback_time = Some(Instant::now());
        Ok(())
    }

    fn dump_history(&mut self) -> Result<()> {
        debug!(
            "dump_history: history_bytes={} lines={}",
            self.history.total_bytes(),
            self.history.line_count()
        );
        self.output_buffer.clear();
        self.history.append_all(&mut self.output_buffer);

        // Debug: write history to file if CLAUDE_CHILL_HISTORY_FILE is set
        if let Ok(path) = std::env::var("CLAUDE_CHILL_HISTORY_FILE")
            && let Err(e) = std::fs::write(&path, &self.output_buffer)
        {
            debug!("Failed to write history file: {}", e);
        }

        self.write_to_terminal(CLEAR_SCREEN)?;
        self.write_to_terminal(CURSOR_HOME)?;
        // Can't use write_to_terminal here due to borrow checker - can't pass
        // &self.output_buffer while also taking &mut self
        Self::update_kitty_mode_helper(
            &mut self.kitty_output_parser,
            &mut self.kitty_mode_stack,
            self.kitty_mode_supported,
            &self.output_buffer,
        );
        platform::write_stdout(&self.output_buffer)?;

        // Force full VT render on next output since terminal now shows history
        self.vt_prev_screen = None;
        Ok(())
    }

    fn process_input(&mut self, data: &[u8]) -> Result<()> {
        self.last_stdin_time = Some(Instant::now());

        debug!(
            "process_input: stdin={:?} paste={}",
            data, self.in_bracketed_paste
        );

        // In alternate screen, forward directly with deadlock prevention
        if self.in_alternate_screen {
            return self.pty.write_draining(data, &mut self.pty_drain_buffer);
        }

        // In bracketed paste, forward directly until paste end marker
        if self.in_bracketed_paste {
            let combined;
            let search_data = if self.paste_remainder.is_empty() {
                data
            } else {
                combined = [self.paste_remainder.as_slice(), data].concat();
                self.paste_remainder.clear();
                &combined
            };

            if let Some(pos) = self.paste_end_finder.find(search_data) {
                let end = pos + BRACKETED_PASTE_END.len();
                self.pty
                    .write_draining(&search_data[..end], &mut self.pty_drain_buffer)?;
                self.in_bracketed_paste = false;
                debug!("process_input: bracketed paste ended");
                if end < search_data.len() {
                    return self.process_input(&search_data[end..]);
                }
                return Ok(());
            }

            let (forward, remainder) =
                split_trailing_marker_prefix(search_data, BRACKETED_PASTE_END);
            if !forward.is_empty() {
                self.pty
                    .write_draining(forward, &mut self.pty_drain_buffer)?;
            }
            self.paste_remainder.extend_from_slice(remainder);
            return Ok(());
        }

        // Check for bracketed paste start
        if let Some(pos) = self.paste_start_finder.find(data) {
            debug!("process_input: bracketed paste started");
            // Process any data before paste start through normal lookback matching
            if pos > 0 {
                self.process_input_lookback(&data[..pos])?;
            }
            self.in_bracketed_paste = true;
            let paste_data = &data[pos..];
            // Check if paste end is in same chunk
            let search_start = BRACKETED_PASTE_START.len();
            if paste_data.len() > search_start
                && let Some(end_pos) = self.paste_end_finder.find(&paste_data[search_start..])
            {
                let end = search_start + end_pos + BRACKETED_PASTE_END.len();
                self.pty
                    .write_draining(&paste_data[..end], &mut self.pty_drain_buffer)?;
                self.in_bracketed_paste = false;
                debug!("process_input: bracketed paste ended (same chunk)");
                if end < paste_data.len() {
                    return self.process_input(&paste_data[end..]);
                }
                return Ok(());
            }
            let (forward, remainder) =
                split_trailing_marker_prefix(paste_data, BRACKETED_PASTE_END);
            if !forward.is_empty() {
                self.pty
                    .write_draining(forward, &mut self.pty_drain_buffer)?;
            }
            self.paste_remainder.extend_from_slice(remainder);
            return Ok(());
        }

        self.process_input_lookback(data)
    }

    /// Byte-by-byte input processing with lookback sequence matching.
    /// Only used for normal (non-paste, non-alt-screen) input.
    fn process_input_lookback(&mut self, data: &[u8]) -> Result<()> {
        let lookback_sequence = if self.kitty_mode_enabled() {
            self.config.lookback_sequence_kitty.clone()
        } else {
            self.config.lookback_sequence_legacy.clone()
        };

        for &byte in data {
            if self.in_lookback_mode && byte == 0x03 {
                self.lookback_input_buffer.clear();
                self.exit_lookback_mode()?;
                continue;
            }

            let lookback_action = self.check_sequence_match(
                byte,
                &mut self.lookback_input_buffer.clone(),
                &lookback_sequence,
            );

            self.lookback_input_buffer.push(byte);

            if self.lookback_input_buffer.len() > lookback_sequence.len() {
                let excess = self.lookback_input_buffer.len() - lookback_sequence.len();
                self.lookback_input_buffer.drain(..excess);
            }

            match lookback_action {
                SequenceMatch::Complete => {
                    self.lookback_input_buffer.clear();
                    if self.in_lookback_mode {
                        self.exit_lookback_mode()?;
                    } else {
                        self.enter_lookback_mode()?;
                    }
                    continue;
                }
                SequenceMatch::Partial => {
                    // Still might be lookback sequence, don't forward yet
                    continue;
                }
                SequenceMatch::None => {
                    // Not a lookback sequence - forward all buffered bytes
                    if !self.in_lookback_mode {
                        self.pty.write(&self.lookback_input_buffer.clone())?;
                    }
                    self.lookback_input_buffer.clear();
                }
            }
        }
        Ok(())
    }

    fn check_sequence_match(
        &self,
        byte: u8,
        buffer: &mut Vec<u8>,
        sequence: &[u8],
    ) -> SequenceMatch {
        buffer.push(byte);
        if buffer.len() > sequence.len() {
            let excess = buffer.len() - sequence.len();
            buffer.drain(..excess);
        }
        if buffer.as_slice() == sequence {
            SequenceMatch::Complete
        } else if sequence.starts_with(buffer) {
            SequenceMatch::Partial
        } else {
            SequenceMatch::None
        }
    }

    fn enter_lookback_mode(&mut self) -> Result<()> {
        debug!(
            "enter_lookback_mode: history_bytes={} lines={}",
            self.history.total_bytes(),
            self.history.line_count()
        );
        self.in_lookback_mode = true;
        self.lookback_cache.clear();
        self.vt_render_pending = false;

        self.output_buffer.clear();
        self.history.append_all(&mut self.output_buffer);
        debug!(
            "enter_lookback_mode: output_buffer_len={}",
            self.output_buffer.len()
        );

        platform::write_stdout(CLEAR_SCREEN)?;
        platform::write_stdout(CURSOR_HOME)?;
        platform::write_stdout(&self.output_buffer)?;

        let exit_msg = format!(
            "\r\n\x1b[7m--- LOOKBACK MODE: press {} or Ctrl+C to exit ---\x1b[0m\r\n",
            self.config.lookback_key
        );
        platform::write_stdout(exit_msg.as_bytes())?;

        Ok(())
    }

    fn exit_lookback_mode(&mut self) -> Result<()> {
        debug!(
            "exit_lookback_mode: cached_len={}",
            self.lookback_cache.len()
        );
        self.in_lookback_mode = false;

        // Process cached output through VT to update screen state
        let cached = std::mem::take(&mut self.lookback_cache);
        if !cached.is_empty() {
            debug!(
                "exit_lookback_mode: processing {} cached bytes",
                cached.len()
            );
            self.process_output(&cached)?;
        }

        // Reset sync block state
        self.in_sync_block = false;
        self.sync_buffer.clear();

        self.forward_winsize()?;

        // Force full render since terminal was showing history
        debug!("exit_lookback_mode: rendering VT screen");
        self.vt_prev_screen = None;
        self.render_vt_screen()?;

        Ok(())
    }

    fn forward_winsize(&mut self) -> Result<()> {
        if let Ok(term_size) = platform::get_terminal_size() {
            debug!(
                "forward_winsize: rows={} cols={}",
                term_size.rows, term_size.cols
            );
            // Resize VT emulator
            self.vt_parser
                .screen_mut()
                .set_size(term_size.rows, term_size.cols);
            // Force full render on next frame since size changed
            self.vt_prev_screen = None;
            // Forward to child process
            self.pty.set_size(term_size)?;
        }
        Ok(())
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        // Disable bracketed paste before restoring terminal
        let _ = platform::write_stdout(BRACKETED_PASTE_DISABLE);

        if let Some(ref state) = self.original_terminal_state {
            platform::restore_terminal(state);
        }
    }
}

fn split_trailing_marker_prefix<'a>(data: &'a [u8], marker: &[u8]) -> (&'a [u8], &'a [u8]) {
    let max_prefix = marker.len().saturating_sub(1).min(data.len());
    for prefix_len in (1..=max_prefix).rev() {
        if data.ends_with(&marker[..prefix_len]) {
            let split = data.len() - prefix_len;
            return (&data[..split], &data[split..]);
        }
    }
    (data, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to test Kitty tracking using the real update_kitty_mode_helper
    struct KittyTracker {
        parser: TermwizParser,
        mode_supported: bool,
        mode_stack: u32,
    }

    impl KittyTracker {
        fn new() -> Self {
            Self {
                parser: TermwizParser::new(),
                mode_supported: false,
                mode_stack: 0,
            }
        }

        fn mode_enabled(&self) -> bool {
            self.mode_stack > 0
        }

        fn process_output(&mut self, data: &[u8]) {
            // Uses the real production function
            Proxy::update_kitty_mode_helper(
                &mut self.parser,
                &mut self.mode_stack,
                self.mode_supported,
                data,
            );
        }

        fn process_input(&mut self, data: &[u8]) {
            // Kitty support detection from query response (CSI ? flags u)
            // This is done separately in detect_kitty_support() at startup
            if self.mode_supported {
                return;
            }
            let actions = self.parser.parse_as_vec(data);
            for action in actions {
                if let Action::CSI(CSI::Keyboard(Keyboard::ReportKittyState(_))) = action {
                    self.mode_supported = true;
                    return;
                }
            }
        }
    }

    // Tests for Kitty keyboard protocol tracking

    #[test]
    fn test_kitty_initially_disabled() {
        let tracker = KittyTracker::new();
        assert!(!tracker.mode_enabled());
        assert!(!tracker.mode_supported);
    }

    #[test]
    fn test_kitty_support_detected_from_query_response() {
        let mut tracker = KittyTracker::new();
        // Terminal responds to query with CSI ? flags u
        tracker.process_input(b"\x1b[?1u");
        assert!(tracker.mode_supported);
    }

    #[test]
    fn test_kitty_push_increments_stack() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        // CSI > 1 u = push with flags
        tracker.process_output(b"\x1b[>1u");
        assert_eq!(tracker.mode_stack, 1);
        assert!(tracker.mode_enabled());
    }

    #[test]
    fn test_kitty_push_requires_support() {
        let mut tracker = KittyTracker::new();
        // Push without support detection - should be ignored
        tracker.process_output(b"\x1b[>1u");
        assert_eq!(tracker.mode_stack, 0);
        assert!(!tracker.mode_enabled());
    }

    #[test]
    fn test_kitty_pop_decrements_stack() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        tracker.process_output(b"\x1b[>1u"); // push
        tracker.process_output(b"\x1b[<u"); // pop 1
        assert_eq!(tracker.mode_stack, 0);
        assert!(!tracker.mode_enabled());
    }

    #[test]
    fn test_kitty_pop_with_count() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        tracker.process_output(b"\x1b[>1u"); // push
        tracker.process_output(b"\x1b[>1u"); // push
        tracker.process_output(b"\x1b[>1u"); // push
        assert_eq!(tracker.mode_stack, 3);
        tracker.process_output(b"\x1b[<2u"); // pop 2
        assert_eq!(tracker.mode_stack, 1);
        assert!(tracker.mode_enabled());
    }

    #[test]
    fn test_kitty_pop_saturates_at_zero() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        tracker.process_output(b"\x1b[>1u"); // push
        tracker.process_output(b"\x1b[<5u"); // pop 5 (more than we have)
        assert_eq!(tracker.mode_stack, 0);
        assert!(!tracker.mode_enabled());
    }

    #[test]
    fn test_kitty_split_sequence_across_buffers() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        // Feed the sequence in parts
        tracker.process_output(b"\x1b[>");
        tracker.process_output(b"1u");
        assert_eq!(tracker.mode_stack, 1);
    }

    #[test]
    fn test_kitty_multiple_sequences_in_one_buffer() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        // Push twice, pop once, all in one buffer
        tracker.process_output(b"\x1b[>1u\x1b[>1u\x1b[<u");
        assert_eq!(tracker.mode_stack, 1);
    }

    #[test]
    fn test_kitty_mixed_with_other_sequences() {
        let mut tracker = KittyTracker::new();
        tracker.mode_supported = true;
        // Kitty push mixed with cursor moves and SGR
        tracker.process_output(b"\x1b[H\x1b[>1u\x1b[31m\x1b[2J");
        assert_eq!(tracker.mode_stack, 1);
    }

    #[test]
    fn test_kitty_typical_session_flow() {
        let mut tracker = KittyTracker::new();
        // 1. Terminal responds to query
        tracker.process_input(b"\x1b[?1u");
        assert!(tracker.mode_supported);
        assert!(!tracker.mode_enabled());
        // 2. App pushes keyboard mode
        tracker.process_output(b"\x1b[>1u");
        assert!(tracker.mode_enabled());
        // 3. App pops keyboard mode on exit
        tracker.process_output(b"\x1b[<u");
        assert!(!tracker.mode_enabled());
    }

    // Tests for sequence matching (used for lookback key detection)

    fn check_sequence(buffer: &[u8], byte: u8, sequence: &[u8]) -> SequenceMatch {
        let mut buf = buffer.to_vec();
        buf.push(byte);
        if buf.len() > sequence.len() {
            let excess = buf.len() - sequence.len();
            buf.drain(..excess);
        }
        if buf.as_slice() == sequence {
            SequenceMatch::Complete
        } else if sequence.starts_with(&buf) {
            SequenceMatch::Partial
        } else {
            SequenceMatch::None
        }
    }

    #[test]
    fn test_sequence_match_complete_single_byte() {
        // Single byte sequence (legacy Ctrl+6 = 0x1E)
        let sequence = &[0x1E];
        assert_eq!(check_sequence(&[], 0x1E, sequence), SequenceMatch::Complete);
    }

    #[test]
    fn test_sequence_match_complete_multi_byte() {
        // Multi-byte Kitty sequence: ESC [ 5 4 ; 5 u
        let sequence = b"\x1b[54;5u";
        let mut buffer = Vec::new();
        for &byte in &sequence[..sequence.len() - 1] {
            let result = check_sequence(&buffer, byte, sequence);
            assert_eq!(result, SequenceMatch::Partial);
            buffer.push(byte);
            if buffer.len() > sequence.len() {
                buffer.drain(..buffer.len() - sequence.len());
            }
        }
        // Final byte completes the sequence
        assert_eq!(
            check_sequence(&buffer, sequence[sequence.len() - 1], sequence),
            SequenceMatch::Complete
        );
    }

    #[test]
    fn test_sequence_match_partial() {
        let sequence = b"\x1b[54;5u";
        assert_eq!(check_sequence(&[], 0x1b, sequence), SequenceMatch::Partial);
        assert_eq!(
            check_sequence(&[0x1b], b'[', sequence),
            SequenceMatch::Partial
        );
        assert_eq!(
            check_sequence(&[0x1b, b'['], b'5', sequence),
            SequenceMatch::Partial
        );
    }

    #[test]
    fn test_sequence_match_none_wrong_byte() {
        let sequence = b"\x1b[54;5u";
        // Start with wrong byte
        assert_eq!(check_sequence(&[], b'a', sequence), SequenceMatch::None);
        // Wrong byte after partial match
        assert_eq!(check_sequence(&[0x1b], b'O', sequence), SequenceMatch::None);
    }

    #[test]
    fn test_sequence_match_buffer_rolling() {
        // Test that the rolling buffer properly handles the case where
        // random bytes precede the actual sequence. The buffer keeps
        // only the last N bytes where N = sequence.len()
        let sequence = b"\x1b[54;5u"; // 7 bytes
        // User types random chars - no match
        assert_eq!(check_sequence(&[], b'a', sequence), SequenceMatch::None);
        assert_eq!(check_sequence(b"a", b'b', sequence), SequenceMatch::None);
        // Buffer [a, b, ESC] doesn't start sequence (sequence starts with ESC)
        assert_eq!(check_sequence(b"ab", 0x1b, sequence), SequenceMatch::None);
        // After more typing, old bytes get trimmed from buffer
        // When buffer finally contains just ESC at the right position, it matches
        // But with rolling buffer, we need the EXACT prefix
        // Fresh start: ESC alone is a partial match
        assert_eq!(check_sequence(&[], 0x1b, sequence), SequenceMatch::Partial);
    }

    #[test]
    fn test_sequence_match_interleaved_typing() {
        // User types "ab" then the lookback sequence
        let sequence = &[0x1E];
        assert_eq!(check_sequence(&[], b'a', sequence), SequenceMatch::None);
        assert_eq!(check_sequence(b"a", b'b', sequence), SequenceMatch::None);
        assert_eq!(
            check_sequence(b"ab", 0x1E, sequence),
            SequenceMatch::Complete
        );
    }

    #[test]
    fn test_split_trailing_marker_prefix_no_match() {
        let marker = b"\x1b[201~";
        let data = b"hello world";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"hello world");
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_split_trailing_marker_prefix_one_byte() {
        let marker = b"\x1b[201~";
        let data = b"paste data\x1b";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"paste data");
        assert_eq!(remainder, b"\x1b");
    }

    #[test]
    fn test_split_trailing_marker_prefix_partial() {
        let marker = b"\x1b[201~";
        let data = b"paste data\x1b[20";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"paste data");
        assert_eq!(remainder, b"\x1b[20");
    }

    #[test]
    fn test_split_trailing_marker_prefix_max_prefix() {
        let marker = b"\x1b[201~";
        let data = b"paste\x1b[201";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"paste");
        assert_eq!(remainder, b"\x1b[201");
    }

    #[test]
    fn test_split_trailing_marker_prefix_full_marker_not_held() {
        let marker = b"\x1b[201~";
        let data = b"paste\x1b[201~";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"paste\x1b[201~");
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_split_trailing_marker_prefix_false_positive() {
        let marker = b"\x1b[201~";
        let data = b"paste data\x1b[999";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert_eq!(forward, b"paste data\x1b[999");
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_split_trailing_marker_prefix_empty_data() {
        let marker = b"\x1b[201~";
        let data = b"";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert!(forward.is_empty());
        assert!(remainder.is_empty());
    }

    #[test]
    fn test_split_trailing_marker_prefix_data_shorter_than_marker() {
        let marker = b"\x1b[201~";
        let data = b"\x1b[2";
        let (forward, remainder) = split_trailing_marker_prefix(data, marker);
        assert!(forward.is_empty());
        assert_eq!(remainder, b"\x1b[2");
    }
}
