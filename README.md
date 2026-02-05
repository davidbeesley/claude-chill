# claude-chill

[![CI](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml/badge.svg)](https://github.com/davidbeesley/claude-chill/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
![Version](https://img.shields.io/badge/version-0.1.4-blue)
![Linux](https://img.shields.io/badge/Linux-supported-green)
![macOS](https://img.shields.io/badge/macOS-supported-green)
![Windows](https://img.shields.io/badge/Windows-unsupported-red)
![Rust](https://img.shields.io/badge/rust-2024-orange)

A PTY proxy that tames Claude Code's massive terminal updates using VT-based rendering.

## The Problem

Claude Code uses synchronized output to update the terminal atomically. It wraps output in sync markers (`\x1b[?2026h` ... `\x1b[?2026l`) so the terminal renders everything at once without flicker.

The problem: Claude Code sends *entire* screen redraws in these sync blocks - often thousands of lines. Your terminal receives a 5000-line atomic update when only 20 lines are visible. This causes lag, flicker, or jitters in the terminal, making for a poor user experience.

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

### Command Line Help

```
$ claude-chill --help
A PTY proxy that tames Claude Code's massive terminal updates

Usage: claude-chill [OPTIONS] <COMMAND> [ARGS]...

Arguments:
  <COMMAND>  Command to run (e.g., "claude")
  [ARGS]...  Arguments to pass to the command

Options:
  -H, --history <HISTORY_LINES>
          Max lines stored for lookback (default: 100000)
  -k, --lookback-key <LOOKBACK_KEY>
          Key to toggle lookback mode, quote to prevent glob expansion (default: "[ctrl][6]")
  -a, --auto-lookback-timeout <AUTO_LOOKBACK_TIMEOUT>
          Auto-lookback timeout in ms, 0 to disable (default: 15000)
  -h, --help
          Print help
  -V, --version
          Print version
```

### Examples

```bash
# Basic usage
claude-chill claude

# Pass arguments to claude
claude-chill -- claude --verbose

# Custom history size
claude-chill -H 50000 claude

# Custom lookback key
claude-chill -k "[f12]" claude

# Disable auto-lookback (see below)
claude-chill -a 0 claude

# Combine options with claude arguments
claude-chill -H 50000 -a 0 -- claude --verbose
```

## Lookback Mode

Press `Ctrl+6` (or your configured key) to enter lookback mode:

> **macOS Note:** Some Mac terminals don't send control characters for `Ctrl+number` keys.`Ctrl+Shift+6` (i.e., `Ctrl+^`) will work by default, or the lookback key can be customized.

1. **Claude pauses** - Output from Claude is cached, input is blocked
2. **History dumps** - The full history buffer is written to your terminal
3. **Scroll freely** - Use your terminal's scrollback to review everything
4. **Exit** - Press the lookback key again or `Ctrl+C` to resume

When you exit lookback mode, any cached output is processed and the current state is displayed.

## Auto-Lookback

After `auto_lookback_timeout_ms` (default 15 seconds) of idle (no user input), the full history is automatically dumped to your terminal so you can scroll back without pressing any keys. This continues to re-dump every `auto_lookback_timeout_ms` while idle. This is useful for reviewing Claude's output after it finishes working.

**Note:** The auto-lookback causes a brief screen flicker during the transition as it clears the screen and writes the history buffer. Disable with `-a 0` or adjust the timeout with `-a 30000` (30 seconds).

## Configuration

Config file location:
- **Linux**: `~/.config/claude-chill.toml`
- **macOS**: `~/Library/Application Support/claude-chill.toml`

```toml
history_lines = 100000           # Max lines stored for lookback
lookback_key = "[ctrl][6]"       # Key to toggle lookback mode
refresh_rate = 20                # Rendering FPS
auto_lookback_timeout_ms = 15000 # Auto-lookback after 15s idle (0 to disable)
```

Note: History is cleared on full screen redraws, so lookback shows output since Claude's last full render.

### Kitty Keyboard Protocol

Modern terminals like Kitty, Ghostty, and WezTerm support the [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/) which encodes keys differently than legacy terminals.

claude-chill automatically tracks Kitty protocol state by monitoring the escape sequences passing through the proxy. When Claude Code enables Kitty mode, claude-chill switches to expecting Kitty-encoded key sequences for the lookback key. When Claude Code disables it, claude-chill switches back to legacy mode. This happens transparently with no configuration needed.

### Key Format

`[modifier][key]` - Examples: `[f12]`, `[ctrl][g]`, `[ctrl][shift][j]`

Modifiers: `[ctrl]`, `[shift]`, `[alt]`

Keys: `[a]`-`[z]`, `[f1]`-`[f12]`, `[pageup]`, `[pagedown]`, `[home]`, `[end]`, `[enter]`, `[tab]`, `[space]`, `[esc]`

**Note:** Quote the key value on the command line to prevent shell glob expansion: `-k "[ctrl][7]"`

### Why Ctrl+6?

`Ctrl+6` sends 0x1E (ASCII RS), a control character not frequently used by terminals, signals, or shells. Avoid `Ctrl+letter` hotkeys - terminals can't distinguish `Ctrl+J` from `Ctrl+Shift+J`.

**macOS caveat:** Mac terminals don't send control characters for `Ctrl+number` combinations. On macOS, press `Ctrl+Shift+6` (equivalent to `Ctrl+^`) which produces the same 0x1E byte. A more portable default key may be chosen in a future release.

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

## Installation with Nix

### Any System (Linux / MacOS)

```bash
# Install directly from GitHub
nix profile install github:davidbeesley/claude-chill

# Or run without installing
nix run github:davidbeesley/claude-chill -- --help
```

### NixOS

Add the following to your `flake.nix`:
```nix
inputs.claude-chill.url = "github:davidbeesley/claude-chill";
```

And then the following package to your environment.systemPackages or home.packages:
```nix
inputs.claude-chill.packages.${system}.default
```

## Disclaimer

This tool was developed for personal convenience. It works for me on Linux and macOS, but it hasn't been extensively tested across different terminals or edge cases. Don't use it to send anyone to space, perform surgery, or run critical infrastructure. If it breaks, you get to keep both pieces.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
