use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "claude-chill",
    about = "PTY proxy that reduces terminal flicker by truncating synchronized output",
    long_about = "claude-chill sits between your terminal and a child process, intercepting \
                  synchronized output blocks and truncating them to reduce flicker.\n\n\
                  Full history is preserved. Press the lookback key (default: Ctrl+Shift+J) \
                  to dump history to terminal, then scroll up to view it.",
    version,
    after_help = "USAGE EXAMPLES:\n    \
                  claude-chill claude\n    \
                  claude-chill -- claude --verbose      # Use -- for command flags\n    \
                  claude-chill -l 50 -- claude          # Set max lines to 50\n\n\
                  CONFIGURATION:\n    \
                  Create ~/.config/claude-chill.toml:\n\n    \
                  max_lines = 100        # Lines shown per sync block\n    \
                  history_lines = 100000 # Lines stored for lookback\n    \
                  lookback_key = \"[ctrl][shift][j]\"\n\n\
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
        short = 'l',
        long = "max-lines",
        help = "Maximum lines per sync block",
        value_name = "N"
    )]
    pub max_lines: Option<usize>,

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
        help = "Key to trigger lookback (e.g., [ctrl][shift][j])",
        value_name = "KEY"
    )]
    pub lookback_key: Option<String>,
}
