# VHS Terminal Tests

This directory contains [VHS](https://github.com/charmbracelet/vhs) tape files for testing claude-chill's terminal functionality.

## Overview

VHS is a terminal recording tool that allows us to write declarative test scripts (`.tape` files) that:
- Automate terminal interactions
- Output ASCII captures for verification (GIF generation disabled for faster CI)

## Running Tests Locally

### Prerequisites

1. Install VHS:
   ```bash
   # Linux (Debian/Ubuntu)
   ./tests/vhs/run-tests.sh --install-vhs

   # macOS
   brew install charmbracelet/tap/vhs
   ```

2. Build claude-chill:
   ```bash
   cargo build
   ```

### Run All Tests

```bash
./tests/vhs/run-tests.sh
```

### Run Specific Test

```bash
./tests/vhs/run-tests.sh tests/vhs/ctrl-b.tape
```

## Test Files

| File | Description | Status |
|------|-------------|--------|
| `basic.tape` | Basic spawn, echo, and exit | Passing |
| `lookback.tape` | Lookback mode toggle (Ctrl+^) | Passing |
| `ctrl-b.tape` | Ctrl-B cursor movement (Issue #27) | **Failing** |

## Writing New Tests

VHS tape files use a simple declarative syntax:

```tape
# Output configuration
Output tests/vhs/output/my-test.ascii
Output tests/vhs/output/my-test.gif

# Terminal settings
Set Shell "bash"
Set FontSize 14
Set Width 800
Set Height 400

# Commands
Type "echo hello"
Enter
Sleep 300ms
Ctrl+B    # Send Ctrl+B
```

### Key Commands

- `Type "text"` - Type text
- `Enter` - Press Enter
- `Ctrl+X` - Press Ctrl+X (where X is a letter)
- `Sleep 100ms` - Wait
- `Hide/Show` - Hide/show setup commands

## CI Integration

VHS tests run automatically in CI via `.github/workflows/vhs-tests.yml`. ASCII outputs are uploaded as artifacts for review.

## Output Directory

Generated files are saved to `tests/vhs/output/`:
- `*.ascii` - Text captures of final terminal state
- `*.log` - Test run logs

The output directory is gitignored.
