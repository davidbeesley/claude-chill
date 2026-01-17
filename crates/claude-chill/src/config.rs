use crate::key_parser::{self, KeyCombination};
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

const DEFAULT_LOOKBACK_KEY: &str = "[ctrl][6]";

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub max_lines: usize,
    pub history_lines: usize,
    pub lookback_key: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_lines: 100,
            history_lines: 100_000,
            lookback_key: DEFAULT_LOOKBACK_KEY.to_string(),
        }
    }
}

impl Config {
    pub fn load() -> Self {
        let config_path = Self::config_path();
        match config_path {
            Some(path) if path.exists() => Self::load_from_file(&path),
            _ => Self::default(),
        }
    }

    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("claude-chill.toml"))
    }

    fn load_from_file(path: &PathBuf) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to parse config file {}: {}",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) => {
                eprintln!(
                    "Warning: Failed to read config file {}: {}",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    pub fn parse_lookback_key(&self) -> Result<KeyCombination, key_parser::ParseKeyError> {
        key_parser::parse(&self.lookback_key)
    }

    pub fn lookback_sequence(&self) -> Vec<u8> {
        self.parse_lookback_key()
            .map(|k| k.to_escape_sequence())
            .unwrap_or_else(|e| {
                eprintln!(
                    "Warning: Invalid lookback_key '{}': {}",
                    self.lookback_key, e
                );
                eprintln!("Using default: {}", DEFAULT_LOOKBACK_KEY);
                key_parser::parse(DEFAULT_LOOKBACK_KEY)
                    .map(|k| k.to_escape_sequence())
                    .unwrap_or_else(|_| b"\x1b[5;6~".to_vec())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.max_lines, 100);
        assert_eq!(config.history_lines, 100_000);
        assert_eq!(config.lookback_key, "[ctrl][6]");
    }

    #[test]
    fn test_default_lookback_sequence() {
        let config = Config::default();
        assert_eq!(config.lookback_sequence(), vec![0x1E]);
    }
}
