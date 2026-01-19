//! Unix (Linux, macOS) platform implementation.
//!
//! Uses POSIX PTY, termios, signals, and poll() for I/O multiplexing.

use super::{PlatformSignal, PollResult, TerminalSize};
use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::pty::{Winsize, openpty};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, kill, sigaction};
use nix::sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr};
use nix::unistd::{Pid, isatty, read, write};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};

// Signal flags - set by signal handlers, checked in main loop
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

/// Unix PTY wrapper
pub struct Pty {
    pub master: OwnedFd,
    child: Child,
}

impl Pty {
    /// Spawn a child process in a new PTY
    pub fn spawn(command: &str, args: &[&str], size: TerminalSize) -> Result<Self> {
        let winsize = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let pty = openpty(&winsize, None).context("openpty failed")?;
        let slave_fd = pty.slave.as_raw_fd();

        let child = unsafe {
            Command::new(command)
                .args(args)
                .pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
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

        // Close slave in parent - child has its own copy
        drop(pty.slave);

        // Set master to non-blocking
        set_nonblocking(&pty.master)?;

        Ok(Self {
            master: pty.master,
            child,
        })
    }

    /// Read from PTY master
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, Errno> {
        read(self.master.as_fd(), buf)
    }

    /// Write to PTY master
    pub fn write(&self, data: &[u8]) -> Result<()> {
        write_all(&self.master, data)
    }

    /// Set the PTY window size
    pub fn set_size(&self, size: TerminalSize) -> Result<()> {
        let winsize = Winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(
                self.master.as_raw_fd(),
                libc::TIOCSWINSZ as libc::c_ulong,
                &winsize,
            );
        }
        Ok(())
    }

    /// Get the child process ID
    pub fn child_pid(&self) -> u32 {
        self.child.id()
    }

    /// Send a signal to the child process
    pub fn signal(&self, sig: PlatformSignal) {
        let signal = match sig {
            PlatformSignal::Interrupt => Signal::SIGINT,
            PlatformSignal::Terminate => Signal::SIGTERM,
            PlatformSignal::Resize => return, // Resize is handled via ioctl, not signal
        };
        let pid = Pid::from_raw(self.child.id() as i32);
        let _ = kill(pid, signal);
    }

    /// Wait for child to exit
    pub fn wait(&mut self) -> Result<i32> {
        match self.child.wait() {
            Ok(status) => Ok(exit_code_from_status(status)),
            Err(e) => anyhow::bail!("wait failed: {}", e),
        }
    }

    /// Get raw file descriptor for polling
    pub fn as_raw_fd(&self) -> i32 {
        self.master.as_raw_fd()
    }
}

/// Terminal raw mode guard - restores original mode on drop
pub struct RawModeGuard {
    original_termios: Option<Termios>,
}

impl RawModeGuard {
    /// Enable raw mode and return guard that restores on drop
    pub fn new() -> Result<Self> {
        let original_termios = setup_raw_mode()?;
        Ok(Self { original_termios })
    }

    /// Take ownership of the original termios (for manual restoration)
    pub fn take(mut self) -> Option<Termios> {
        self.original_termios.take()
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(ref termios) = self.original_termios {
            let _ = tcsetattr(io::stdin(), SetArg::TCSANOW, termios);
        }
    }
}

/// Restore terminal from raw mode
pub fn restore_terminal(termios: &Termios) {
    let _ = tcsetattr(io::stdin(), SetArg::TCSANOW, termios);
}

/// Original termios for restoration (re-exported type)
pub type OriginalTerminalState = Termios;

/// Check if stdin is a TTY
pub fn is_stdin_tty() -> bool {
    isatty(io::stdin().as_fd()).unwrap_or(false)
}

/// Get the current terminal size
pub fn get_terminal_size() -> Result<TerminalSize> {
    let mut ws: Winsize = unsafe { std::mem::zeroed() };
    let ret = unsafe {
        libc::ioctl(
            io::stdout().as_raw_fd(),
            libc::TIOCGWINSZ as libc::c_ulong,
            &mut ws,
        )
    };
    if ret == -1 || ws.ws_row == 0 || ws.ws_col == 0 {
        Ok(TerminalSize::default())
    } else {
        Ok(TerminalSize {
            rows: ws.ws_row,
            cols: ws.ws_col,
        })
    }
}

/// Setup signal handlers for SIGWINCH, SIGINT, SIGTERM
pub fn setup_signal_handlers() -> Result<()> {
    setup_signal_handler(Signal::SIGWINCH, handle_sigwinch)?;
    setup_signal_handler(Signal::SIGINT, handle_sigint)?;
    setup_signal_handler(Signal::SIGTERM, handle_sigterm)?;
    Ok(())
}

/// Check and clear pending signals, returning which ones fired
pub fn check_signals() -> Vec<PlatformSignal> {
    let mut signals = Vec::new();
    if SIGWINCH_RECEIVED.swap(false, Ordering::SeqCst) {
        signals.push(PlatformSignal::Resize);
    }
    if SIGINT_RECEIVED.swap(false, Ordering::SeqCst) {
        signals.push(PlatformSignal::Interrupt);
    }
    if SIGTERM_RECEIVED.swap(false, Ordering::SeqCst) {
        signals.push(PlatformSignal::Terminate);
    }
    signals
}

/// Poll for I/O events on PTY and stdin
pub fn poll_io(pty_fd: i32, timeout_ms: u16) -> Result<PollResult> {
    let master_fd = unsafe { BorrowedFd::borrow_raw(pty_fd) };
    let stdin_fd = io::stdin();
    let stdin_borrowed = unsafe { BorrowedFd::borrow_raw(stdin_fd.as_raw_fd()) };

    let mut poll_fds = [
        PollFd::new(master_fd, PollFlags::POLLIN),
        PollFd::new(stdin_borrowed, PollFlags::POLLIN),
    ];

    match poll(&mut poll_fds, PollTimeout::from(timeout_ms)) {
        Ok(0) => Ok(PollResult::Timeout),
        Ok(_) => {
            let pty_readable = poll_fds[0]
                .revents()
                .map(|r| r.contains(PollFlags::POLLIN))
                .unwrap_or(false);
            let pty_hangup = poll_fds[0]
                .revents()
                .map(|r| r.contains(PollFlags::POLLHUP))
                .unwrap_or(false);
            let stdin_readable = poll_fds[1]
                .revents()
                .map(|r| r.contains(PollFlags::POLLIN))
                .unwrap_or(false);

            if pty_hangup {
                Ok(PollResult::PtyHangup)
            } else if pty_readable && stdin_readable {
                Ok(PollResult::BothReadable)
            } else if pty_readable {
                Ok(PollResult::PtyReadable)
            } else if stdin_readable {
                Ok(PollResult::StdinReadable)
            } else {
                Ok(PollResult::Timeout)
            }
        }
        Err(Errno::EINTR) => Ok(PollResult::Interrupted),
        Err(e) => anyhow::bail!("poll failed: {}", e),
    }
}

/// Write data to stdout
pub fn write_stdout(data: &[u8]) -> Result<()> {
    write_all(&io::stdout(), data)
}

/// Read from stdin
pub fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
    match read(io::stdin().as_fd(), buf) {
        Ok(n) => Ok(n),
        Err(Errno::EAGAIN) => Ok(0),
        Err(e) => Err(io::Error::from_raw_os_error(e as i32)),
    }
}

// ============ Internal helpers ============

fn setup_raw_mode() -> Result<Option<Termios>> {
    let stdin = io::stdin();
    if !isatty(&stdin).unwrap_or(false) {
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

fn set_nonblocking<Fd: AsFd>(fd: &Fd) -> Result<()> {
    let flags = fcntl(fd.as_fd(), FcntlArg::F_GETFL).context("fcntl F_GETFL failed")?;
    let flags = OFlag::from_bits_truncate(flags);
    fcntl(fd.as_fd(), FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))
        .context("fcntl F_SETFL failed")?;
    Ok(())
}

fn write_all<F: AsFd>(fd: &F, data: &[u8]) -> Result<()> {
    let mut written = 0;
    while written < data.len() {
        match write(fd, &data[written..]) {
            Ok(n) => written += n,
            Err(Errno::EAGAIN) | Err(Errno::EINTR) => continue,
            Err(e) => anyhow::bail!("write failed: {}", e),
        }
    }
    Ok(())
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
