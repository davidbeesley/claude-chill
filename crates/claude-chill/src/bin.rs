mod cli;

use clap::Parser;
use claude_chill::config::Config;
use claude_chill::key_parser;
use claude_chill::proxy::{Proxy, ProxyConfig};
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let config = Config::load();

    let max_lines = cli.max_lines.unwrap_or(config.max_lines);
    let history_lines = cli.history_lines.unwrap_or(config.history_lines);

    let lookback_key = cli
        .lookback_key
        .clone()
        .unwrap_or_else(|| config.lookback_key.clone());

    let lookback_sequence = match key_parser::parse(&lookback_key) {
        Ok(key) => key.to_escape_sequence(),
        Err(e) => {
            eprintln!("Invalid lookback key '{}': {}", lookback_key, e);
            eprintln!("Using default: [ctrl][shift][j]");
            config.lookback_sequence()
        }
    };

    let proxy_config = ProxyConfig {
        max_output_lines: max_lines,
        max_history_lines: history_lines,
        lookback_key,
        lookback_sequence,
    };

    let cmd_args: Vec<&str> = cli.args.iter().map(|s| s.as_str()).collect();

    match Proxy::spawn(&cli.command, &cmd_args, proxy_config) {
        Ok(mut proxy) => match proxy.run() {
            Ok(exit_code) => ExitCode::from(exit_code as u8),
            Err(e) => {
                eprintln!("Proxy error: {}", e);
                ExitCode::from(1)
            }
        },
        Err(e) => {
            eprintln!("Failed to start proxy: {:#}", e);
            ExitCode::from(1)
        }
    }
}
