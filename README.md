# claude-chill

[![CI](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml/badge.svg)](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![GitHub stars](https://img.shields.io/github/stars/davidbeesley/claude-chill)](https://github.com/davidbeesley/claude-chill/stargazers)
[![GitHub forks](https://img.shields.io/github/forks/davidbeesley/claude-chill)](https://github.com/davidbeesley/claude-chill/network)
![GitHub repo size](https://img.shields.io/github/repo-size/davidbeesley/claude-chill)
![Rust](https://img.shields.io/badge/rust-2024-orange)

A PTY proxy that tames Claude Code's massive terminal updates.

## The Problem

Claude Code uses synchronized output to update the terminal atomically. It wraps output in sync markers (`\x1b[?2026h` ... `\x1b[?2026l`) so the terminal renders everything at once without flicker.

The problem: Claude Code sends *entire* screen redraws in these sync blocks - often thousands of lines. Your terminal receives a 5000-line atomic update when only 20 lines are visible. This causes lag, flicker, and makes scrollback useless since each update clears history.

Analysis of terminal recordings confirms Claude Code wraps 100% of its output in sync blocks - every byte of visible output goes through synchronized updates.

Sync blocks start with one of three patterns (from a 3.5GB recording sample):

| Pattern | Count | Frequency | Avg Size |
|---------|-------|-----------|----------|
| Line clearing (`2K` + `1A` repeated) | 3,544 | 55% | 2.7 KB |
| Full screen clear (`2J` + `3J` + `H`) | 2,891 | 45% | 94.5 KB |
| CRLF + color codes | 1 | <1% | 2.0 KB |

The full screen clears are 35x larger than incremental line clears - these are the real problem.

## The Solution

claude-chill sits between your terminal and Claude Code:

1. **Intercepts sync blocks** - Catches those massive atomic updates
2. **Truncates full screen clears** - Only the 45% that are full redraws (94.5 KB avg) get truncated to the last N lines (default: 100). The 55% incremental updates (2.7 KB avg) pass through unchanged.
3. **Preserves history** - Accumulates content in a buffer. Clears on full screen clear, accumulates otherwise.
4. **Enables lookback** - Press a key to pause Claude and view the full history buffer

## Installation

```bash
cargo install --path crates/claude-chill
```

## Usage

```bash
claude-chill claude
claude-chill -- claude --verbose   # Use -- for command flags
claude-chill -l 50 -- claude       # Set max lines to 50
```

## Lookback Mode

Press `Ctrl+6` (or your configured key) to enter lookback mode:

1. **Claude pauses** - Output from Claude is cached, input is blocked
2. **History dumps** - The full history buffer is written to your terminal
3. **Scroll freely** - Use your terminal's scrollback to review everything
4. **Exit** - Press the lookback key again or `Ctrl+C` to resume

When you exit lookback mode, any cached output is processed and the current state is displayed.

## Configuration

Create `~/.config/claude-chill.toml`:

```toml
max_lines = 100        # Lines shown per sync block
history_lines = 100000 # Lines stored for lookback
lookback_key = "[ctrl][6]"
```

### Key Format

`[modifier][key]` - Examples: `[f12]`, `[ctrl][g]`, `[ctrl][shift][j]`

Modifiers: `[ctrl]`, `[shift]`, `[alt]`

Keys: `[a]`-`[z]`, `[f1]`-`[f12]`, `[pageup]`, `[pagedown]`, `[home]`, `[end]`, `[enter]`, `[tab]`, `[space]`, `[esc]`

### Why Ctrl+6?

`Ctrl+6` sends 0x1E, a control character not frequently used by terminals, signals, or shells. Avoid `Ctrl+letter` hotkeys - terminals can't distinguish `Ctrl+J` from `Ctrl+Shift+J`.

## Disclaimer

This tool was developed for personal convenience on Debian Linux. It works for me, but it hasn't been extensively tested across different terminals, operating systems, or edge cases. Don't use it to send anyone to space, perform surgery, or run critical infrastructure. If it breaks, you get to keep both pieces.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
