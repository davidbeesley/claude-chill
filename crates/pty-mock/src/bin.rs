use clap::{Parser, Subcommand};
use std::io::{self, BufRead, Read, Write};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Echo,
    AltScreen,
    PasteEcho,
    BufferFill,
    SyncBlocks,
}

fn cmd_echo() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if writeln!(stdout, "{}", line).is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }
}

fn cmd_alt_screen() {
    let mut stdout = io::stdout();
    let mut stdin = io::stdin();

    // Enter alt screen
    let _ = stdout.write_all(b"\x1b[?1049h");
    let _ = stdout.flush();

    // Write some content while in alt screen
    let _ = stdout.write_all(b"alt screen content\r\n");
    let _ = stdout.flush();

    // Wait for any byte on stdin as trigger to exit
    let mut trigger = [0u8; 1];
    let _ = stdin.read_exact(&mut trigger);

    // Exit alt screen
    let _ = stdout.write_all(b"\x1b[?1049l");
    let _ = stdout.flush();

    // Write content after alt screen exit
    let _ = stdout.write_all(b"back to normal\r\n");
    let _ = stdout.flush();
}

fn cmd_paste_echo() {
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    // Enable bracketed paste mode
    let _ = stdout.write_all(b"\x1b[?2004h");
    let _ = stdout.flush();

    // Read lines, prefix paste content
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if writeln!(stdout, "pasted: {}", line).is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }
}

fn cmd_buffer_fill() {
    let mut stdout = io::stdout();
    let stdin = io::stdin();
    let mut stdin_lock = stdin.lock();

    // Write large output to fill the PTY buffer
    let chunk = "X".repeat(1024) + "\r\n";
    for _ in 0..256 {
        if stdout.write_all(chunk.as_bytes()).is_err() {
            break;
        }
    }
    let _ = stdout.flush();

    // Now read stdin and echo it back
    let mut buf = [0u8; 4096];
    loop {
        match stdin_lock.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() {
                    break;
                }
                if stdout.flush().is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn cmd_sync_blocks() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        // Wrap response in sync update block
        let _ = stdout.write_all(b"\x1b[?2026h");
        if writeln!(stdout, "sync: {}", line).is_err() {
            break;
        }
        let _ = stdout.write_all(b"\x1b[?2026l");
        if stdout.flush().is_err() {
            break;
        }
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Echo => cmd_echo(),
        Commands::AltScreen => cmd_alt_screen(),
        Commands::PasteEcho => cmd_paste_echo(),
        Commands::BufferFill => cmd_buffer_fill(),
        Commands::SyncBlocks => cmd_sync_blocks(),
    }
}
