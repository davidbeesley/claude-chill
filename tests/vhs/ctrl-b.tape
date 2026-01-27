# VHS test for Ctrl-B cursor movement (Issue #27: Emacs vterm compatibility)
#
# This test verifies that Ctrl-B correctly moves the cursor backward.
# In Emacs and many terminals, Ctrl-B is the standard keybinding for
# moving the cursor one character to the left.
#
# EXPECTED: This test should FAIL until issue #27 is resolved.

Output tests/vhs/output/ctrl-b.ascii

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

# Set a simple prompt and enable emacs mode explicitly
Hide
Type "export PS1='TEST> '"
Enter
Type "set -o emacs"
Enter
Sleep 200ms
Show

# Type some text at the prompt
Type "echo ABCDEF"

# Now send Ctrl-B twice to move cursor back 2 positions
# Ctrl-B = 0x02
Ctrl+B
Sleep 100ms
Ctrl+B
Sleep 100ms

# Type 'X' - if Ctrl-B worked, cursor should be before 'E'
# So the output should be "ABCDXEF" instead of "ABCDEFX"
Type "X"
Sleep 100ms

Enter
Sleep 500ms

# The test expects "ABCDXEF" to appear in the output
# If we see "ABCDEFX" instead, the Ctrl-B didn't work

Type "exit"
Enter
Sleep 300ms
