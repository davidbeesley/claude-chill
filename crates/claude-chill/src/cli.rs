use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "claude-chill",
    version,
    about = "A PTY proxy that tames Claude Code's massive terminal updates"
)]
pub struct Cli {
    /// Command to run (e.g., "claude")
    pub command: String,

    /// Arguments to pass to the command
    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,

    /// Max lines stored for lookback (default: 100000)
    #[arg(short = 'H', long = "history")]
    pub history_lines: Option<usize>,

    /// Key to toggle lookback mode, quote to prevent glob expansion (default: "[ctrl][6]")
    #[arg(short = 'k', long = "lookback-key")]
    pub lookback_key: Option<String>,

    /// Auto-lookback timeout in ms, 0 to disable (default: 5000)
    #[arg(short = 'a', long = "auto-lookback-timeout")]
    pub auto_lookback_timeout: Option<u64>,
}
