use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "claude-chill", version)]
pub struct Cli {
    pub command: String,

    #[arg(trailing_var_arg = true)]
    pub args: Vec<String>,

    #[arg(short = 'H', long = "history")]
    pub history_lines: Option<usize>,

    #[arg(short = 'k', long = "lookback-key")]
    pub lookback_key: Option<String>,

    #[arg(long = "auto-lookback-timeout")]
    pub auto_lookback_timeout: Option<u64>,
}
