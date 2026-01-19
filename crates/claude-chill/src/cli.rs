use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "claude-chill",
    about = "PTY proxy that reduces terminal flicker using VT-based rendering",
    long_about = "claude-chill sits between your terminal and a child process, intercepting \
                  synchronized output blocks and rendering them efficiently using a VT emulator.\n\n\
                  Full history is preserved. Press the lookback key (default: Ctrl+6) \
                  to dump history to terminal, then scroll up to view it.",
    version,
    after_help = "USAGE EXAMPLES:\n    \
                  claude-chill claude\n    \
                  claude-chill -- claude --verbose      # Use -- for command flags\n\n\
                  CONFIGURATION:\n    \
                  Create ~/.config/claude-chill.toml:\n\n    \
                  history_lines = 100000 # Lines stored for lookback\n    \
                  lookback_key = \"[ctrl][6]\"\n\n\
                  KEY FORMAT: [modifier][key]\n    \
                  Modifiers: [ctrl], [shift], [alt]\n    \
                  Keys: [a]-[z], [f1]-[f12], [pageup], [enter], [space], etc."
)]
pub struct Cli {
    #[arg(help = "Command to run", required = true, value_name = "COMMAND")]
    pub command: String,

    #[arg(
        help = "Arguments passed to command (use -- before command flags)",
        value_name = "ARGS",
        trailing_var_arg = true
    )]
    pub args: Vec<String>,

    #[arg(
        short = 'H',
        long = "history",
        help = "Maximum history lines for lookback",
        value_name = "N"
    )]
    pub history_lines: Option<usize>,

    #[arg(
        short = 'k',
        long = "lookback-key",
        help = "Key to trigger lookback (e.g., [ctrl][6])",
        value_name = "KEY"
    )]
    pub lookback_key: Option<String>,
}
