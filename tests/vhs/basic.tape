# Basic VHS test for claude-chill
# Tests basic functionality: spawn, echo, exit

Output tests/vhs/output/basic.ascii

Set Shell "bash"
Set FontSize 14
Set Width 800
Set Height 400
Set Theme "Catppuccin Mocha"
Set TypingSpeed 30ms

# Start claude-chill with a clean bash shell
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

# Test basic echo
Type "echo 'Hello from VHS test!'"
Enter
Sleep 300ms

# Test that the shell is responsive
Type "echo $((1 + 1))"
Enter
Sleep 300ms

# Exit cleanly
Type "exit"
Enter
Sleep 500ms
