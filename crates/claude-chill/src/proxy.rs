use crate::escape_sequences::{
    ALT_SCREEN_ENTER, ALT_SCREEN_ENTER_LEGACY, ALT_SCREEN_EXIT, ALT_SCREEN_EXIT_LEGACY,
    CLEAR_SCREEN, CURSOR_HOME, INPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_CAPACITY, SYNC_BUFFER_CAPACITY,
    SYNC_END, SYNC_START,
};
use crate::line_buffer::LineBuffer;
use crate::platform::{self, PlatformSignal, PollResult, Pty, RawModeGuard};
use anyhow::{Context, Result};
use log::debug;
use memchr::memmem;
use std::time::{Duration, Instant};

#[cfg(unix)]
use crate::platform::OriginalTerminalState;
#[cfg(windows)]
use crate::platform::OriginalTerminalState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SequenceMatch {
    Complete,
    Partial,
    None,
}

pub struct ProxyConfig {
    pub max_history_lines: usize,
    pub lookback_key: String,
    pub lookback_sequence: Vec<u8>,
    pub auto_lookback_timeout_ms: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            max_history_lines: 100_000,
            lookback_key: "[ctrl][6]".to_string(),
            lookback_sequence: vec![0x1E],
            auto_lookback_timeout_ms: 5000,
        }
    }
}

const RENDER_DELAY_MS: u64 = 5;
const SYNC_BLOCK_DELAY_MS: u64 = 50;

pub struct Proxy {
    config: ProxyConfig,
    pty: Pty,
    original_terminal_state: Option<OriginalTerminalState>,
    history: LineBuffer,
    vt_parser: vt100::Parser,
    vt_prev_screen: Option<vt100::Screen>,
    last_output_time: Option<Instant>,
    last_render_time: Option<Instant>,
    auto_lookback_timeout: Duration,
    sync_buffer: Vec<u8>,
    in_sync_block: bool,
    in_lookback_mode: bool,
    in_alternate_screen: bool,
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
}

impl Proxy {
    pub fn spawn(command: &str, args: &[&str], config: ProxyConfig) -> Result<Self> {
        let term_size = platform::get_terminal_size()?;

        let raw_mode_guard = RawModeGuard::new()?;
        platform::setup_signal_handlers()?;

        let pty = Pty::spawn(command, args, term_size).context("PTY spawn failed")?;

        let vt_parser = vt100::Parser::new(term_size.rows, term_size.cols, 0);

        // Seed history with clear screen so replay starts fresh
        let mut history = LineBuffer::new(config.max_history_lines);
        history.push_bytes(CLEAR_SCREEN);
        history.push_bytes(CURSOR_HOME);

        let auto_lookback_timeout = Duration::from_millis(config.auto_lookback_timeout_ms);

        debug!("Proxy::spawn: command={} args={:?}", command, args);

        Ok(Self {
            history,
            config,
            pty,
            original_terminal_state: raw_mode_guard.take(),
            vt_parser,
            vt_prev_screen: None,
            last_output_time: None,
            last_render_time: None,
            auto_lookback_timeout,
            sync_buffer: Vec::with_capacity(SYNC_BUFFER_CAPACITY),
            in_sync_block: false,
            in_lookback_mode: false,
            in_alternate_screen: false,
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

            #[cfg(unix)]
            let poll_result = platform::poll_io(self.pty.as_raw_fd(), poll_timeout_ms)?;
            #[cfg(windows)]
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
                    match self.pty.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => self.process_output(&buf[..n])?,
                        #[cfg(unix)]
                        Err(nix::errno::Errno::EAGAIN) => {}
                        #[cfg(unix)]
                        Err(nix::errno::Errno::EIO) => break,
                        #[cfg(windows)]
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => anyhow::bail!("read from pty failed: {}", e),
                    }
                }
                PollResult::StdinReadable => {
                    self.flush_pending_vt_render()?;
                    match platform::read_stdin(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => self.process_input(&buf[..n])?,
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => anyhow::bail!("read from stdin failed: {}", e),
                    }
                }
                PollResult::BothReadable => {
                    self.flush_pending_vt_render()?;
                    // Handle PTY first
                    match self.pty.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => self.process_output(&buf[..n])?,
                        #[cfg(unix)]
                        Err(nix::errno::Errno::EAGAIN) => {}
                        #[cfg(unix)]
                        Err(nix::errno::Errno::EIO) => break,
                        #[cfg(windows)]
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => anyhow::bail!("read from pty failed: {}", e),
                    }
                    // Then handle stdin
                    match platform::read_stdin(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => self.process_input(&buf[..n])?,
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(e) => anyhow::bail!("read from stdin failed: {}", e),
                    }
                }
            }
        }

        // Final render before exit
        if self.vt_render_pending {
            self.render_vt_screen()?;
        }

        self.pty.wait()
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
            // Still feed VT and history while in alt screen so they stay in sync
            if feed_vt {
                self.vt_parser.process(data);
                self.history.push_bytes(data);
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
                    self.history.push_bytes(remaining);
                }
                self.in_alternate_screen = true;
                let seq_len = self.alt_screen_enter_len(&data[pos + alt_pos..]);
                // Write alt screen enter directly
                platform::write_stdout(&data[pos + alt_pos..pos + alt_pos + seq_len])?;
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
                    self.history.push_bytes(&data[pos..pos + idx]);
                }
                self.in_sync_block = true;
                self.sync_buffer.clear();
                self.sync_buffer.extend_from_slice(SYNC_START);
                pos += idx + SYNC_START.len();
            } else {
                // No sync block, just add to history
                self.history.push_bytes(&data[pos..]);
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
            platform::write_stdout(&data[..exit_pos])?;
            let seq_len = self.alt_screen_exit_len(&data[exit_pos..]);
            platform::write_stdout(&data[exit_pos..exit_pos + seq_len])?;
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
        platform::write_stdout(data)
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
            platform::write_stdout(&data[alt_pos..alt_pos + seq_len])?;
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
        self.history.push_bytes(&self.sync_buffer);
        self.sync_buffer.clear();
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
        let Some(render_time) = self.last_render_time else {
            return Ok(());
        };
        if render_time.elapsed() < self.auto_lookback_timeout {
            return Ok(());
        }
        self.dump_history()?;
        self.last_render_time = None;
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

        platform::write_stdout(CLEAR_SCREEN)?;
        platform::write_stdout(CURSOR_HOME)?;
        platform::write_stdout(&self.output_buffer)?;

        // Force full VT render on next output since terminal now shows history
        self.vt_prev_screen = None;
        Ok(())
    }

    fn process_input(&mut self, data: &[u8]) -> Result<()> {
        if self.in_alternate_screen {
            return self.pty.write(data);
        }

        for &byte in data {
            if self.in_lookback_mode && byte == 0x03 {
                self.lookback_input_buffer.clear();
                self.exit_lookback_mode()?;
                continue;
            }

            let lookback_action = self.check_sequence_match(
                byte,
                &mut self.lookback_input_buffer.clone(),
                &self.config.lookback_sequence.clone(),
            );

            self.lookback_input_buffer.push(byte);

            if self.lookback_input_buffer.len() > self.config.lookback_sequence.len() {
                let excess = self.lookback_input_buffer.len() - self.config.lookback_sequence.len();
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
                SequenceMatch::Partial => {}
                SequenceMatch::None => {
                    if !self
                        .config
                        .lookback_sequence
                        .starts_with(&self.lookback_input_buffer)
                    {
                        self.lookback_input_buffer.clear();
                    }
                }
            }

            if lookback_action == SequenceMatch::None && !self.in_lookback_mode {
                self.pty.write(&[byte])?;
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
        if let Some(ref state) = self.original_terminal_state {
            platform::restore_terminal(state);
        }
    }
}
