# claude-chill

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
2. **Truncates output** - Sends only the last N lines (default: 100) to your terminal
3. **Preserves history** - Stores the full content in a buffer
4. **Enables lookback** - Press a key to dump the buffer, then scroll up

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

## Lookback

Press `Ctrl+Shift+J` to dump the history buffer to terminal. Scroll up to see it. The next update from Claude will resume normal display.

## Configuration

Create `~/.config/claude-chill.toml`:

```toml
max_lines = 100        # Lines shown per sync block
history_lines = 100000 # Lines stored for lookback
lookback_key = "[ctrl][shift][j]"
```

### Key Format

`[modifier][key]` - Examples: `[f12]`, `[ctrl][g]`, `[ctrl][shift][j]`

Modifiers: `[ctrl]`, `[shift]`, `[alt]`

Keys: `[a]`-`[z]`, `[f1]`-`[f12]`, `[pageup]`, `[pagedown]`, `[home]`, `[end]`, `[enter]`, `[tab]`, `[space]`, `[esc]`

## Disclaimer

This tool was developed for personal convenience on Debian Linux. It works for me, but it hasn't been extensively tested across different terminals, operating systems, or edge cases. Don't use it to send anyone to space, perform surgery, or run critical infrastructure. If it breaks, you get to keep both pieces.

## License

MIT
