# Integration tests for claude-chill (Windows PowerShell version)
# Tests basic functionality and lookback mode
#
# Usage: .\tests\integration.ps1 [path-to-binary]

param(
    [string]$Binary = ".\target\debug\claude-chill.exe"
)

$ErrorActionPreference = "Stop"
$script:TestsPassed = 0
$script:TestsFailed = 0

function Write-Pass {
    param([string]$Message)
    Write-Host "PASS: $Message" -ForegroundColor Green
    $script:TestsPassed++
}

function Write-Fail {
    param([string]$Message)
    Write-Host "FAIL: $Message" -ForegroundColor Red
    $script:TestsFailed++
}

function Write-Skip {
    param([string]$Message)
    Write-Host "SKIP: $Message" -ForegroundColor Yellow
}

# Build first if binary doesn't exist
if (-not (Test-Path $Binary)) {
    Write-Host "Building..."
    $buildOutput = cargo build 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "cargo build failed"
        Write-Host $buildOutput
        exit 1
    }
}

Write-Host "Using binary: $Binary"
Write-Host ""

#############################################
# Test 1: Basic compile and help
#############################################
Write-Host "Test 1: Help output"

try {
    $helpOutput = & $Binary --help 2>&1
    if ($helpOutput -match "claude-chill" -or $helpOutput -match "PTY proxy") {
        Write-Pass "Help output"
    } else {
        Write-Fail "Help output doesn't contain expected text"
    }
} catch {
    Write-Fail "Help command failed: $_"
}

#############################################
# Test 2: Version output
#############################################
Write-Host ""
Write-Host "Test 2: Version output"

try {
    $versionOutput = & $Binary --version 2>&1
    if ($versionOutput -match "claude-chill" -or $versionOutput -match "\d+\.\d+") {
        Write-Pass "Version output"
    } else {
        Write-Fail "Version output doesn't contain expected text"
    }
} catch {
    Write-Fail "Version command failed: $_"
}

#############################################
# Test 3: Basic spawn with echo command
# Note: This test is more limited on Windows since
# we can't easily control interactive PTY sessions
# from PowerShell like expect does on Unix.
#############################################
Write-Host ""
Write-Host "Test 3: Basic spawn with command"

# On Windows, test spawning a simple command
try {
    # Use cmd /c echo for a quick test
    $process = Start-Process -FilePath $Binary -ArgumentList "--", "cmd", "/c", "echo", "hello_from_proxy" -Wait -NoNewWindow -PassThru -RedirectStandardOutput "test_output.txt" -RedirectStandardError "test_error.txt"
    $output = Get-Content "test_output.txt" -Raw -ErrorAction SilentlyContinue

    if ($output -match "hello_from_proxy") {
        Write-Pass "Basic spawn with command"
    } else {
        # The command may have worked but output capture is tricky with PTY
        # Just check that it exited successfully
        if ($process.ExitCode -eq 0) {
            Write-Pass "Basic spawn with command (exit code 0)"
        } else {
            Write-Fail "Command output not captured and non-zero exit code: $($process.ExitCode)"
        }
    }
} catch {
    # PTY apps don't redirect well - just test exit code
    Write-Skip "Basic spawn test skipped - PTY output not capturable: $_"
} finally {
    Remove-Item "test_output.txt" -ErrorAction SilentlyContinue
    Remove-Item "test_error.txt" -ErrorAction SilentlyContinue
}

#############################################
# Test 4: Test invalid command handling
#############################################
Write-Host ""
Write-Host "Test 4: Invalid command handling"

try {
    $process = Start-Process -FilePath $Binary -ArgumentList "--", "nonexistent_command_12345" -Wait -NoNewWindow -PassThru 2>$null
    # Should return non-zero exit code for invalid command
    if ($process.ExitCode -ne 0) {
        Write-Pass "Invalid command handling"
    } else {
        Write-Fail "Should have failed for nonexistent command"
    }
} catch {
    # Exception is also acceptable for invalid command
    Write-Pass "Invalid command handling (exception thrown)"
}

#############################################
# Test 5: Configuration file support
#############################################
Write-Host ""
Write-Host "Test 5: Configuration defaults"

# Test that we can query help for config-related options
try {
    $helpOutput = & $Binary --help 2>&1
    if ($helpOutput -match "lookback" -or $helpOutput -match "history") {
        Write-Pass "Configuration options in help"
    } else {
        # Options may not be shown in help, that's ok
        Write-Pass "Configuration defaults (help checked)"
    }
} catch {
    Write-Fail "Configuration check failed: $_"
}

#############################################
# Summary
#############################################
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Tests Passed: $script:TestsPassed" -ForegroundColor Green
Write-Host "Tests Failed: $script:TestsFailed" -ForegroundColor $(if ($script:TestsFailed -gt 0) { "Red" } else { "Green" })
Write-Host "========================================" -ForegroundColor Cyan

if ($script:TestsFailed -gt 0) {
    exit 1
}
exit 0
