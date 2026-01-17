use crate::escape_sequences::{
    ALT_SCREEN_ENTER, ALT_SCREEN_ENTER_LEGACY, ALT_SCREEN_EXIT, ALT_SCREEN_EXIT_LEGACY,
    CLEAR_SCREEN, CLEAR_SCROLLBACK, CURSOR_HOME, INPUT_BUFFER_CAPACITY, OUTPUT_BUFFER_CAPACITY,
    SYNC_BUFFER_CAPACITY, SYNC_END, SYNC_START,
};
use crate::line_buffer::LineBuffer;
use anyhow::{Context, Result};
use memchr::memmem;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::pty::{Winsize, openpty};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use nix::sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr};
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

static SIGWINCH_RECEIVED: AtomicBool = AtomicBool::new(false);
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);
static SIGTERM_RECEIVED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigwinch(_: libc::c_int) {
    SIGWINCH_RECEIVED.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sigint(_: libc::c_int) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
}

extern "C" fn handle_sigterm(_: libc::c_int) {
    SIGTERM_RECEIVED.store(true, Ordering::SeqCst);
}

pub struct ProxyConfig {
    pub max_output_lines: usize,
    pub max_history_lines: usize,
    pub lookback_key: String,
    pub lookback_sequence: Vec<u8>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            max_output_lines: 100,
            max_history_lines: 100_000,
            lookback_key: "[ctrl][6]".to_string(),
            lookback_sequence: vec![0x1E],
        }
    }
}

pub struct Proxy {
    config: ProxyConfig,
    pty_master: OwnedFd,
    child: Child,
    original_termios: Option<Termios>,
    history: LineBuffer,
    sync_buffer: Vec<u8>,
    in_sync_block: bool,
    in_lookback_mode: bool,
    in_alternate_screen: bool,
    lookback_cache: Vec<u8>,
    input_buffer: Vec<u8>,
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
        let winsize = get_terminal_size()?;
        let pty = openpty(&winsize, None).context("openpty failed")?;

        let original_termios = setup_raw_mode()?;
        setup_signal_handlers()?;

        let slave_fd = pty.slave.as_raw_fd();

        let child = unsafe {
            Command::new(command)
                .args(args)
                .pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::dup2(slave_fd, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::dup2(slave_fd, 1) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::dup2(slave_fd, 2) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if slave_fd > 2 {
                        libc::close(slave_fd);
                    }
                    Ok(())
                })
                .spawn()
                .context("spawn failed")?
        };

        drop(pty.slave);
        set_nonblocking(&pty.master)?;

        Ok(Self {
            history: LineBuffer::new(config.max_history_lines),
            config,
            pty_master: pty.master,
            child,
            original_termios,
            sync_buffer: Vec::with_capacity(SYNC_BUFFER_CAPACITY),
            in_sync_block: false,
            in_lookback_mode: false,
            in_alternate_screen: false,
            lookback_cache: Vec::new(),
            input_buffer: Vec::with_capacity(INPUT_BUFFER_CAPACITY),
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
        let stdin_raw = io::stdin().as_raw_fd();
        let stdout_raw = io::stdout().as_raw_fd();
        let master_raw = self.pty_master.as_raw_fd();

        let mut buf = [0u8; 65536];

        loop {
            if SIGWINCH_RECEIVED.swap(false, Ordering::SeqCst) {
                self.forward_winsize()?;
            }
            if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
                self.forward_signal(Signal::SIGINT);
            }
            if SIGTERM_RECEIVED.swap(false, Ordering::SeqCst) {
                self.forward_signal(Signal::SIGTERM);
            }

            let mut poll_fds = [
                libc::pollfd {
                    fd: master_raw,
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stdin_raw,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            let poll_ret = unsafe { libc::poll(poll_fds.as_mut_ptr(), 2, 100) };
            if poll_ret == 0 {
                continue;
            }
            if poll_ret < 0 {
                let err = unsafe { *libc::__errno_location() };
                if err == libc::EINTR {
                    continue;
                }
                anyhow::bail!("poll failed: errno {}", err);
            }

            if poll_fds[0].revents & libc::POLLIN != 0 {
                match libc_read(master_raw, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => self.process_output(&buf[..n], stdout_raw)?,
                    Err(e) if e == libc::EAGAIN => {}
                    Err(e) if e == libc::EIO => break,
                    Err(e) => anyhow::bail!("read from pty failed: errno {}", e),
                }
            }
            if poll_fds[0].revents & libc::POLLHUP != 0 {
                break;
            }

            if poll_fds[1].revents & libc::POLLIN != 0 {
                match libc_read(stdin_raw, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => self.process_input(&buf[..n], stdout_raw)?,
                    Err(e) if e == libc::EAGAIN => {}
                    Err(e) => anyhow::bail!("read from stdin failed: errno {}", e),
                }
            }
        }

        if !self.sync_buffer.is_empty() {
            write_all_raw(stdout_raw, &self.sync_buffer)?;
        }

        self.wait_child()
    }

    fn process_output(&mut self, data: &[u8], stdout_fd: i32) -> Result<()> {
        if self.in_alternate_screen {
            return self.process_output_alt_screen(data, stdout_fd);
        }

        if self.in_lookback_mode {
            self.lookback_cache.extend_from_slice(data);
            return Ok(());
        }

        let mut pos = 0;

        while pos < data.len() {
            if let Some(alt_pos) = self.find_alt_screen_enter(&data[pos..]) {
                if self.in_sync_block {
                    self.sync_buffer
                        .extend_from_slice(&data[pos..pos + alt_pos]);
                    write_all_raw(stdout_fd, &self.sync_buffer)?;
                    self.sync_buffer.clear();
                    self.in_sync_block = false;
                } else if alt_pos > 0 {
                    write_all_raw(stdout_fd, &data[pos..pos + alt_pos])?;
                }
                self.in_alternate_screen = true;
                let seq_len = self.alt_screen_enter_len(&data[pos + alt_pos..]);
                write_all_raw(stdout_fd, &data[pos + alt_pos..pos + alt_pos + seq_len])?;
                return self.process_output_alt_screen(&data[pos + alt_pos + seq_len..], stdout_fd);
            }

            if self.in_sync_block {
                if let Some(idx) = self.sync_end_finder.find(&data[pos..]) {
                    self.sync_buffer.extend_from_slice(&data[pos..pos + idx]);
                    self.sync_buffer.extend_from_slice(SYNC_END);
                    self.flush_sync_block(stdout_fd)?;
                    self.in_sync_block = false;
                    pos += idx + SYNC_END.len();
                } else {
                    self.sync_buffer.extend_from_slice(&data[pos..]);
                    break;
                }
            } else if let Some(idx) = self.sync_start_finder.find(&data[pos..]) {
                if idx > 0 {
                    write_all_raw(stdout_fd, &data[pos..pos + idx])?;
                }
                self.in_sync_block = true;
                self.sync_buffer.clear();
                self.sync_buffer.extend_from_slice(SYNC_START);
                pos += idx + SYNC_START.len();
            } else {
                write_all_raw(stdout_fd, &data[pos..])?;
                break;
            }
        }

        Ok(())
    }

    fn process_output_alt_screen(&mut self, data: &[u8], stdout_fd: i32) -> Result<()> {
        if let Some(exit_pos) = self.find_alt_screen_exit(data) {
            write_all_raw(stdout_fd, &data[..exit_pos])?;
            let seq_len = self.alt_screen_exit_len(&data[exit_pos..]);
            write_all_raw(stdout_fd, &data[exit_pos..exit_pos + seq_len])?;
            self.in_alternate_screen = false;
            return self.process_output(&data[exit_pos + seq_len..], stdout_fd);
        }
        write_all_raw(stdout_fd, data)
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

    fn flush_sync_block(&mut self, stdout_fd: i32) -> Result<()> {
        let has_clear_screen = self.clear_screen_finder.find(&self.sync_buffer).is_some();
        let has_cursor_home = self.cursor_home_finder.find(&self.sync_buffer).is_some();
        let is_full_redraw = has_clear_screen && has_cursor_home;

        if is_full_redraw {
            self.history.clear();
        }
        self.history.push_bytes(&self.sync_buffer);

        if is_full_redraw {
            self.create_truncated_output();
            write_all_raw(stdout_fd, &self.output_buffer)?;
        } else {
            write_all_raw(stdout_fd, &self.sync_buffer)?;
        }

        Ok(())
    }

    fn create_truncated_output(&mut self) {
        self.output_buffer.clear();
        self.output_buffer.extend_from_slice(SYNC_START);
        self.output_buffer.extend_from_slice(CLEAR_SCREEN);
        self.output_buffer.extend_from_slice(CLEAR_SCROLLBACK);
        self.output_buffer.extend_from_slice(CURSOR_HOME);
        self.history
            .append_last_n_lines(self.config.max_output_lines, &mut self.output_buffer);
        self.output_buffer.extend_from_slice(SYNC_END);
    }

    fn process_input(&mut self, data: &[u8], stdout_fd: i32) -> Result<()> {
        let master_fd = self.pty_master.as_raw_fd();

        if self.in_alternate_screen {
            return write_all_raw(master_fd, data);
        }

        for &byte in data {
            if self.in_lookback_mode && byte == 0x03 {
                self.input_buffer.clear();
                self.exit_lookback_mode(stdout_fd)?;
                continue;
            }

            self.input_buffer.push(byte);

            if self.input_buffer.len() > self.config.lookback_sequence.len() {
                let excess = self.input_buffer.len() - self.config.lookback_sequence.len();
                if !self.in_lookback_mode {
                    write_all_raw(master_fd, &self.input_buffer[..excess])?;
                }
                self.input_buffer.drain(..excess);
            }

            if self.input_buffer == self.config.lookback_sequence {
                self.input_buffer.clear();
                if self.in_lookback_mode {
                    self.exit_lookback_mode(stdout_fd)?;
                } else {
                    self.enter_lookback_mode()?;
                }
            } else if !self
                .config
                .lookback_sequence
                .starts_with(&self.input_buffer)
            {
                if !self.in_lookback_mode {
                    write_all_raw(master_fd, &self.input_buffer)?;
                }
                self.input_buffer.clear();
            }
        }
        Ok(())
    }

    fn enter_lookback_mode(&mut self) -> Result<()> {
        self.in_lookback_mode = true;
        self.lookback_cache.clear();

        self.output_buffer.clear();
        self.history.append_all(&mut self.output_buffer);

        let stdout_fd = io::stdout().as_raw_fd();
        write_all_raw(stdout_fd, CLEAR_SCREEN)?;
        write_all_raw(stdout_fd, CURSOR_HOME)?;
        write_all_raw(stdout_fd, &self.output_buffer)?;

        let exit_msg = format!(
            "\r\n\x1b[7m--- LOOKBACK MODE: press {} or Ctrl+C to exit ---\x1b[0m\r\n",
            self.config.lookback_key
        );
        write_all_raw(stdout_fd, exit_msg.as_bytes())?;

        Ok(())
    }

    fn exit_lookback_mode(&mut self, stdout_fd: i32) -> Result<()> {
        self.in_lookback_mode = false;

        let cached = std::mem::take(&mut self.lookback_cache);
        if !cached.is_empty() {
            self.process_output(&cached, stdout_fd)?;
        }

        self.output_buffer.clear();
        self.history.append_all(&mut self.output_buffer);

        write_all_raw(stdout_fd, CLEAR_SCREEN)?;
        write_all_raw(stdout_fd, CURSOR_HOME)?;
        write_all_raw(stdout_fd, &self.output_buffer)?;

        Ok(())
    }

    fn forward_winsize(&self) -> Result<()> {
        if let Ok(winsize) = get_terminal_size() {
            unsafe {
                libc::ioctl(self.pty_master.as_raw_fd(), libc::TIOCSWINSZ, &winsize);
            }
        }
        Ok(())
    }

    fn forward_signal(&self, signal: Signal) {
        let pid = self.child.id();
        unsafe {
            libc::kill(pid as i32, signal as i32);
        }
    }

    fn wait_child(&mut self) -> Result<i32> {
        match self.child.wait() {
            Ok(status) => Ok(exit_code_from_status(status)),
            Err(e) => anyhow::bail!("wait failed: {}", e),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        if let Some(ref termios) = self.original_termios {
            let _ = tcsetattr(io::stdin(), SetArg::TCSANOW, termios);
        }
    }
}

fn get_terminal_size() -> Result<Winsize> {
    let mut ws: Winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    if ret == -1 {
        ws.ws_row = 24;
        ws.ws_col = 80;
    }
    Ok(ws)
}

fn exit_code_from_status(status: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    if let Some(code) = status.code() {
        code
    } else if let Some(signal) = status.signal() {
        128 + signal
    } else {
        1
    }
}

fn setup_raw_mode() -> Result<Option<Termios>> {
    let stdin = io::stdin();
    let is_tty = unsafe { libc::isatty(stdin.as_raw_fd()) == 1 };
    if !is_tty {
        return Ok(None);
    }

    let original = tcgetattr(&stdin).context("tcgetattr failed")?;
    let mut raw = original.clone();
    cfmakeraw(&mut raw);
    tcsetattr(&stdin, SetArg::TCSANOW, &raw).context("tcsetattr failed")?;
    Ok(Some(original))
}

fn setup_signal_handler(signal: Signal, handler: extern "C" fn(libc::c_int)) -> Result<()> {
    let action = SigAction::new(
        SigHandler::Handler(handler),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    unsafe { sigaction(signal, &action) }.context(format!("sigaction {:?} failed", signal))?;
    Ok(())
}

fn setup_signal_handlers() -> Result<()> {
    setup_signal_handler(Signal::SIGWINCH, handle_sigwinch)?;
    setup_signal_handler(Signal::SIGINT, handle_sigint)?;
    setup_signal_handler(Signal::SIGTERM, handle_sigterm)?;
    Ok(())
}

fn set_nonblocking<Fd: AsFd>(fd: &Fd) -> Result<()> {
    let flags = fcntl(fd.as_fd(), FcntlArg::F_GETFL).context("fcntl F_GETFL failed")?;
    let flags = OFlag::from_bits_truncate(flags);
    fcntl(fd.as_fd(), FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .context("fcntl F_SETFL failed")?;
    Ok(())
}

fn write_all_raw(fd: i32, data: &[u8]) -> Result<()> {
    let mut written = 0;
    while written < data.len() {
        match libc_write(fd, &data[written..]) {
            Ok(n) => written += n,
            Err(e) if e == libc::EAGAIN || e == libc::EINTR => continue,
            Err(e) => anyhow::bail!("write failed: errno {}", e),
        }
    }
    Ok(())
}

fn libc_read(fd: i32, buf: &mut [u8]) -> Result<usize, i32> {
    let ret = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if ret < 0 {
        Err(unsafe { *libc::__errno_location() })
    } else {
        Ok(ret as usize)
    }
}

fn libc_write(fd: i32, buf: &[u8]) -> Result<usize, i32> {
    let ret = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if ret < 0 {
        Err(unsafe { *libc::__errno_location() })
    } else {
        Ok(ret as usize)
    }
}
