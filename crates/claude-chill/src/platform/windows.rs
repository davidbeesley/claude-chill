//! Windows platform implementation.
//!
//! Uses Windows Pseudoconsole API, Console API, and console events.
//!
//! The Windows Pseudoconsole (ConPTY) was introduced in Windows 10 version 1809
//! and provides a way to create pseudo-terminals similar to Unix PTYs.

use super::{PlatformSignal, PollResult, TerminalSize};
use anyhow::{Context, Result};
use std::io::{self, Read, Write};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{
    CloseHandle, BOOL, FALSE, HANDLE, INVALID_HANDLE_VALUE, TRUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, GetConsoleMode, GetConsoleScreenBufferInfo,
    GetStdHandle, ResizePseudoConsole, SetConsoleCtrlHandler, SetConsoleMode, CONSOLE_MODE,
    CONSOLE_SCREEN_BUFFER_INFO, COORD, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT,
    ENABLE_PROCESSED_INPUT, ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
    HPCON, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, WaitForMultipleObjects, WaitForSingleObject,
    EXTENDED_STARTUPINFO_PRESENT, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTUPINFOEXW,
};

// Signal flags - set by console control handler, checked in main loop
static CTRL_C_RECEIVED: AtomicBool = AtomicBool::new(false);
static CTRL_BREAK_RECEIVED: AtomicBool = AtomicBool::new(false);

/// Original console mode for restoration
#[derive(Clone)]
pub struct OriginalTerminalState {
    stdin_mode: CONSOLE_MODE,
    stdout_mode: CONSOLE_MODE,
}

/// Windows Pseudoconsole (ConPTY) wrapper
pub struct Pty {
    hpc: HPCON,
    pipe_in: HANDLE,  // Write to this to send input to child
    pipe_out: HANDLE, // Read from this to get output from child
    process: HANDLE,
    thread: HANDLE,
    child_pid: u32,
}

impl Pty {
    /// Spawn a child process in a new Pseudoconsole
    pub fn spawn(command: &str, args: &[&str], size: TerminalSize) -> Result<Self> {
        unsafe {
            // Create pipes for PTY I/O
            let mut pipe_pty_in = INVALID_HANDLE_VALUE;
            let mut pipe_to_pty = INVALID_HANDLE_VALUE;
            let mut pipe_from_pty = INVALID_HANDLE_VALUE;
            let mut pipe_pty_out = INVALID_HANDLE_VALUE;

            // Input pipe: we write to pipe_to_pty, PTY reads from pipe_pty_in
            CreatePipe(&mut pipe_pty_in, &mut pipe_to_pty, None, 0)
                .context("CreatePipe for input failed")?;

            // Output pipe: PTY writes to pipe_pty_out, we read from pipe_from_pty
            CreatePipe(&mut pipe_from_pty, &mut pipe_pty_out, None, 0)
                .context("CreatePipe for output failed")?;

            // Create the Pseudoconsole
            let coord = COORD {
                X: size.cols as i16,
                Y: size.rows as i16,
            };
            let hpc = CreatePseudoConsole(coord, pipe_pty_in, pipe_pty_out, 0)
                .context("CreatePseudoConsole failed")?;

            // Close handles that the pseudoconsole now owns
            let _ = CloseHandle(pipe_pty_in);
            let _ = CloseHandle(pipe_pty_out);

            // Prepare startup info with pseudoconsole
            let mut attr_list_size: usize = 0;
            let _ = InitializeProcThreadAttributeList(
                LPPROC_THREAD_ATTRIBUTE_LIST(ptr::null_mut()),
                1,
                0,
                &mut attr_list_size,
            );

            let attr_list_buf = vec![0u8; attr_list_size];
            let attr_list = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list_buf.as_ptr() as *mut _);
            InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_list_size)
                .context("InitializeProcThreadAttributeList failed")?;

            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(hpc.0 as *const _),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
            .context("UpdateProcThreadAttribute failed")?;

            let mut startup_info = STARTUPINFOEXW {
                StartupInfo: std::mem::zeroed(),
                lpAttributeList: attr_list,
            };
            startup_info.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;

            // Build command line
            let cmdline = if args.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", command, args.join(" "))
            };
            let cmdline_wide: Vec<u16> = cmdline.encode_utf16().chain(std::iter::once(0)).collect();

            let mut process_info = PROCESS_INFORMATION::default();

            CreateProcessW(
                None,
                windows::core::PWSTR(cmdline_wide.as_ptr() as *mut _),
                None,
                None,
                FALSE,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                None,
                &startup_info.StartupInfo,
                &mut process_info,
            )
            .context("CreateProcessW failed")?;

            let child_pid = process_info.dwProcessId;

            Ok(Self {
                hpc,
                pipe_in: pipe_to_pty,
                pipe_out: pipe_from_pty,
                process: process_info.hProcess,
                thread: process_info.hThread,
                child_pid,
            })
        }
    }

    /// Read from PTY output pipe
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        unsafe {
            let mut bytes_read: u32 = 0;
            let result = ReadFile(self.pipe_out, Some(buf), Some(&mut bytes_read), None);
            if result.is_ok() {
                Ok(bytes_read as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        }
    }

    /// Write to PTY input pipe
    pub fn write(&self, data: &[u8]) -> Result<()> {
        unsafe {
            let mut written: u32 = 0;
            let mut offset = 0;
            while offset < data.len() {
                let result = WriteFile(
                    self.pipe_in,
                    Some(&data[offset..]),
                    Some(&mut written),
                    None,
                );
                if result.is_err() {
                    anyhow::bail!("WriteFile failed: {}", std::io::Error::last_os_error());
                }
                offset += written as usize;
            }
            Ok(())
        }
    }

    /// Set the PTY window size
    pub fn set_size(&self, size: TerminalSize) -> Result<()> {
        let coord = COORD {
            X: size.cols as i16,
            Y: size.rows as i16,
        };
        unsafe {
            ResizePseudoConsole(self.hpc, coord).context("ResizePseudoConsole failed")?;
        }
        Ok(())
    }

    /// Get the child process ID
    pub fn child_pid(&self) -> u32 {
        self.child_pid
    }

    /// Send a signal to the child process
    pub fn signal(&self, _sig: PlatformSignal) {
        // Windows doesn't have Unix-style signals
        // For Ctrl+C, we could use GenerateConsoleCtrlEvent but it affects all processes
        // attached to the console. The pseudoconsole should forward Ctrl+C automatically.
    }

    /// Wait for child to exit
    pub fn wait(&mut self) -> Result<i32> {
        unsafe {
            WaitForSingleObject(self.process, u32::MAX);
            let mut exit_code: u32 = 0;
            GetExitCodeProcess(self.process, &mut exit_code)
                .context("GetExitCodeProcess failed")?;
            Ok(exit_code as i32)
        }
    }

    /// Get pipe handle for polling
    pub fn output_handle(&self) -> HANDLE {
        self.pipe_out
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.hpc);
            let _ = CloseHandle(self.pipe_in);
            let _ = CloseHandle(self.pipe_out);
            let _ = CloseHandle(self.process);
            let _ = CloseHandle(self.thread);
        }
    }
}

/// Console control handler for Ctrl+C, etc.
unsafe extern "system" fn console_ctrl_handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        0 => {
            // CTRL_C_EVENT
            CTRL_C_RECEIVED.store(true, Ordering::SeqCst);
            TRUE
        }
        1 => {
            // CTRL_BREAK_EVENT
            CTRL_BREAK_RECEIVED.store(true, Ordering::SeqCst);
            TRUE
        }
        _ => FALSE,
    }
}

/// Terminal raw mode guard - restores original mode on drop
pub struct RawModeGuard {
    original: Option<OriginalTerminalState>,
}

impl RawModeGuard {
    /// Enable raw mode and return guard that restores on drop
    pub fn new() -> Result<Self> {
        let original = setup_raw_mode()?;
        Ok(Self { original })
    }

    /// Take ownership of the original state (for manual restoration)
    pub fn take(mut self) -> Option<OriginalTerminalState> {
        self.original.take()
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Some(ref state) = self.original {
            restore_terminal(state);
        }
    }
}

/// Restore terminal from raw mode
pub fn restore_terminal(state: &OriginalTerminalState) {
    unsafe {
        if let Ok(stdin) = GetStdHandle(STD_INPUT_HANDLE) {
            let _ = SetConsoleMode(stdin, state.stdin_mode);
        }
        if let Ok(stdout) = GetStdHandle(STD_OUTPUT_HANDLE) {
            let _ = SetConsoleMode(stdout, state.stdout_mode);
        }
    }
}

/// Check if stdin is a TTY (console)
pub fn is_stdin_tty() -> bool {
    unsafe {
        if let Ok(stdin) = GetStdHandle(STD_INPUT_HANDLE) {
            let mut mode = CONSOLE_MODE::default();
            GetConsoleMode(stdin, &mut mode).is_ok()
        } else {
            false
        }
    }
}

/// Get the current terminal size
pub fn get_terminal_size() -> Result<TerminalSize> {
    unsafe {
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE).context("GetStdHandle failed")?;
        let mut csbi = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(stdout, &mut csbi).is_ok() {
            let cols = (csbi.srWindow.Right - csbi.srWindow.Left + 1) as u16;
            let rows = (csbi.srWindow.Bottom - csbi.srWindow.Top + 1) as u16;
            Ok(TerminalSize { rows, cols })
        } else {
            Ok(TerminalSize::default())
        }
    }
}

/// Setup console control handlers
pub fn setup_signal_handlers() -> Result<()> {
    unsafe {
        SetConsoleCtrlHandler(Some(console_ctrl_handler), TRUE)
            .context("SetConsoleCtrlHandler failed")?;
    }
    Ok(())
}

/// Check and clear pending signals, returning which ones fired
pub fn check_signals() -> Vec<PlatformSignal> {
    let mut signals = Vec::new();
    if CTRL_C_RECEIVED.swap(false, Ordering::SeqCst) {
        signals.push(PlatformSignal::Interrupt);
    }
    if CTRL_BREAK_RECEIVED.swap(false, Ordering::SeqCst) {
        signals.push(PlatformSignal::Terminate);
    }
    signals
}

/// Poll for I/O events
///
/// Note: Windows doesn't have poll() like Unix, so we use WaitForMultipleObjects
/// and check console input separately.
pub fn poll_io(pty: &Pty, timeout_ms: u16) -> Result<PollResult> {
    unsafe {
        let handles = [pty.output_handle()];

        let result = WaitForMultipleObjects(&handles, FALSE, timeout_ms as u32);

        match result {
            WAIT_TIMEOUT => {
                // Check if stdin has input (console input doesn't work well with WaitForMultipleObjects)
                if has_console_input()? {
                    return Ok(PollResult::StdinReadable);
                }
                Ok(PollResult::Timeout)
            }
            WAIT_OBJECT_0 => {
                // PTY output ready
                if has_console_input()? {
                    Ok(PollResult::BothReadable)
                } else {
                    Ok(PollResult::PtyReadable)
                }
            }
            WAIT_FAILED => {
                anyhow::bail!(
                    "WaitForMultipleObjects failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            _ => Ok(PollResult::Timeout),
        }
    }
}

/// Check if console has pending input
fn has_console_input() -> Result<bool> {
    use windows::Win32::System::Console::GetNumberOfConsoleInputEvents;
    unsafe {
        let stdin = GetStdHandle(STD_INPUT_HANDLE).context("GetStdHandle failed")?;
        let mut count: u32 = 0;
        if GetNumberOfConsoleInputEvents(stdin, &mut count).is_ok() && count > 0 {
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

/// Write data to stdout
pub fn write_stdout(data: &[u8]) -> Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(data)?;
    stdout.flush()?;
    Ok(())
}

/// Read from stdin
pub fn read_stdin(buf: &mut [u8]) -> io::Result<usize> {
    let mut stdin = io::stdin().lock();
    stdin.read(buf)
}

// ============ Internal helpers ============

fn setup_raw_mode() -> Result<Option<OriginalTerminalState>> {
    unsafe {
        let stdin = GetStdHandle(STD_INPUT_HANDLE).context("GetStdHandle stdin failed")?;
        let stdout = GetStdHandle(STD_OUTPUT_HANDLE).context("GetStdHandle stdout failed")?;

        let mut stdin_mode = CONSOLE_MODE::default();
        let mut stdout_mode = CONSOLE_MODE::default();

        // If not a console, return None
        if GetConsoleMode(stdin, &mut stdin_mode).is_err() {
            return Ok(None);
        }
        let _ = GetConsoleMode(stdout, &mut stdout_mode);

        let original = OriginalTerminalState {
            stdin_mode,
            stdout_mode,
        };

        // Set raw mode for input: disable line input, echo, and processed input
        // Enable virtual terminal input for escape sequences
        let new_stdin_mode = CONSOLE_MODE(
            (stdin_mode.0
                & !(ENABLE_LINE_INPUT.0 | ENABLE_ECHO_INPUT.0 | ENABLE_PROCESSED_INPUT.0))
                | ENABLE_VIRTUAL_TERMINAL_INPUT.0,
        );
        SetConsoleMode(stdin, new_stdin_mode).context("SetConsoleMode stdin failed")?;

        // Enable virtual terminal processing for output (ANSI escape sequences)
        let new_stdout_mode = CONSOLE_MODE(stdout_mode.0 | ENABLE_VIRTUAL_TERMINAL_PROCESSING.0);
        let _ = SetConsoleMode(stdout, new_stdout_mode);

        Ok(Some(original))
    }
}
