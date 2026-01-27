# VHS test for lookback mode functionality
#
# Tests that Ctrl+^ (0x1E) enters lookback mode and can exit cleanly

Output tests/vhs/output/lookback.ascii

Set Shell "bash"
Set FontSize 14
Set Width 800
Set Height 400
Set Theme "Catppuccin Mocha"
Set TypingSpeed 30ms

# Start claude-chill with bash
Hide
Type "./target/debug/claude-chill -- /bin/bash --norc --noprofile"
Enter
Sleep 500ms
Show

# Set a simple prompt
Hide
Type "export PS1='TEST> '"
Enter
Sleep 200ms
Show

# Generate some output to look back at
Type "echo 'Line 1: This is test output'"
Enter
Sleep 200ms

Type "echo 'Line 2: More test output'"
Enter
Sleep 200ms

Type "echo 'Line 3: Even more output'"
Enter
Sleep 200ms

# Enter lookback mode with Ctrl+^ (0x1E)
# Note: In VHS, we can send raw bytes
Ctrl+6
Sleep 500ms

# Should see LOOKBACK MODE indicator
# Exit lookback mode
Ctrl+6
Sleep 500ms

# Verify shell still works after lookback
Type "echo 'Shell still works!'"
Enter
Sleep 300ms

Type "exit"
Enter
Sleep 300ms
