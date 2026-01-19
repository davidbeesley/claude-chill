# claude-chill

[![CI](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml/badge.svg)](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Linux](https://img.shields.io/badge/Linux-supported-green)
![macOS](https://img.shields.io/badge/macOS-supported-green)
![Windows](https://img.shields.io/badge/Windows-unsupported-red)
![Rust](https://img.shields.io/badge/rust-2024-orange)

A PTY proxy that tames Claude Code's massive terminal updates using VT-based rendering.

## The Problem

Claude Code uses synchronized output to update the terminal atomically. It wraps output in sync markers (`\x1b[?2026h` ... `\x1b[?2026l`) so the terminal renders everything at once without flicker.

The problem: Claude Code sends *entire* screen redraws in these sync blocks - often thousands of lines. Your terminal receives a 5000-line atomic update when only 20 lines are visible. This causes lag, flicker, and makes scrollback useless since each update clears history.

## The Solution

claude-chill sits between your terminal and Claude Code:

1. **Intercepts sync blocks** - Catches those massive atomic updates
2. **VT-based rendering** - Uses a VT100 emulator to track screen state and renders only the differences
3. **Preserves history** - Accumulates content in a buffer for lookback
4. **Enables lookback** - Press a key to pause Claude and view the full history buffer

## Installation

```bash
cargo install --path crates/claude-chill
```

## Usage

```bash
claude-chill claude
claude-chill -- claude --verbose   # Use -- for command flags
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
history_lines = 100000 # Max lines stored for lookback
lookback_key = "[ctrl][6]"
```

Note: History is cleared on full screen redraws, so lookback shows output since Claude's last full render.

### Key Format

`[modifier][key]` - Examples: `[f12]`, `[ctrl][g]`, `[ctrl][shift][j]`

Modifiers: `[ctrl]`, `[shift]`, `[alt]`

Keys: `[a]`-`[z]`, `[f1]`-`[f12]`, `[pageup]`, `[pagedown]`, `[home]`, `[end]`, `[enter]`, `[tab]`, `[space]`, `[esc]`

### Why Ctrl+6?

`Ctrl+6` sends 0x1E, a control character not frequently used by terminals, signals, or shells. Avoid `Ctrl+letter` hotkeys - terminals can't distinguish `Ctrl+J` from `Ctrl+Shift+J`.

## How It Works

claude-chill creates a pseudo-terminal (PTY) and spawns Claude Code as a child process. It then acts as a transparent proxy between your terminal and Claude:

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Terminal   │◄───►│ claude-chill │◄───►│  Claude Code │
│   (stdin/    │     │   (proxy)    │     │   (child)    │
│    stdout)   │     │              │     │              │
└──────────────┘     └──────────────┘     └──────────────┘
```

1. **Input handling**: Keystrokes pass through to Claude, except for the lookback key which toggles lookback mode
2. **Output processing**: Scans output for sync block markers. Non-sync output passes through directly
3. **VT emulation**: Feeds output through a VT100 emulator to track the virtual screen state
4. **Differential rendering**: Compares current screen to previous and emits only the changes
5. **History tracking**: Maintains a buffer of output for lookback mode since the last full redraw
6. **Signal forwarding**: Window resize (SIGWINCH), interrupt (SIGINT), and terminate (SIGTERM) signals are forwarded to Claude

## Disclaimer

This tool was developed for personal convenience. It works for me on Linux and macOS, but it hasn't been extensively tested across different terminals or edge cases. Don't use it to send anyone to space, perform surgery, or run critical infrastructure. If it breaks, you get to keep both pieces.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
