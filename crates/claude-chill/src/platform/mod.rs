//! Platform abstraction layer for cross-platform terminal/PTY support.
//!
//! This module provides a common interface for platform-specific functionality:
//! - Unix (Linux, macOS): Uses POSIX PTY, termios, signals
//! - Windows: Uses Windows Pseudoconsole API, Console API, console events

use anyhow::Result;
use std::io;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

/// Terminal size in rows and columns
#[derive(Debug, Clone, Copy)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
}

impl Default for TerminalSize {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

/// Platform-specific signal types that we handle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSignal {
    /// Window resize (SIGWINCH on Unix, WINDOW_BUFFER_SIZE_EVENT on Windows)
    Resize,
    /// Interrupt (SIGINT on Unix, CTRL_C_EVENT on Windows)
    Interrupt,
    /// Terminate (SIGTERM on Unix, CTRL_BREAK_EVENT on Windows)
    Terminate,
}

/// Result of polling for I/O events
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PollResult {
    /// PTY has data ready to read
    PtyReadable,
    /// Stdin has data ready to read
    StdinReadable,
    /// Both PTY and stdin have data ready
    BothReadable,
    /// Timeout with no events
    Timeout,
    /// Poll was interrupted (e.g., by signal)
    Interrupted,
    /// PTY closed/hung up
    PtyHangup,
}

/// Check if stdin is a TTY
pub fn is_tty() -> bool {
    #[cfg(unix)]
    {
        unix::is_stdin_tty()
    }
    #[cfg(windows)]
    {
        windows::is_stdin_tty()
    }
}

/// Get the current terminal size
pub fn get_terminal_size() -> Result<TerminalSize> {
    #[cfg(unix)]
    {
        unix::get_terminal_size()
    }
    #[cfg(windows)]
    {
        windows::get_terminal_size()
    }
}

/// Write data to stdout
pub fn write_stdout(data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        unix::write_stdout(data)
    }
    #[cfg(windows)]
    {
        windows::write_stdout(data)
    }
}

/// Read from stdin (non-blocking where possible)
pub fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
    #[cfg(unix)]
    {
        unix::read_stdin(buf)
    }
    #[cfg(windows)]
    {
        windows::read_stdin(buf)
    }
}
