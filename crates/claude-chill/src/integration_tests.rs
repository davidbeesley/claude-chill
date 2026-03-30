#[cfg(test)]
mod tests {
    use crate::proxy::{Proxy, ProxyConfig};
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::pty::openpty;
    use nix::sys::termios::{self, SetArg};
    use nix::unistd::read;
    use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::time::Duration;

    fn pty_mock_binary() -> Option<String> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let bin = format!("{}/../../target/debug/pty-mock", manifest_dir);
        if std::path::Path::new(&bin).exists() {
            return Some(bin);
        }
        None
    }

    fn spawn_command(cmd: &str, args: &[&str]) -> (OwnedFd, Child) {
        let pty = openpty(None, None).expect("openpty failed");

        let slave_fd = pty.slave.as_raw_fd();
        let mut termios_settings = termios::tcgetattr(&pty.slave).expect("tcgetattr failed");
        termios::cfmakeraw(&mut termios_settings);
        termios::tcsetattr(&pty.slave, SetArg::TCSANOW, &termios_settings)
            .expect("tcsetattr failed");

        let child = unsafe {
            Command::new(cmd)
                .args(args)
                .stdin(pty.slave.try_clone().expect("clone slave"))
                .stdout(pty.slave.try_clone().expect("clone slave"))
                .stderr(pty.slave.try_clone().expect("clone slave"))
                .pre_exec(move || {
                    if libc::setsid() == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) == -1 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                })
                .spawn()
                .expect("failed to spawn command")
        };

        drop(pty.slave);

        // Set master to non-blocking
        let flags = fcntl(&pty.master, FcntlArg::F_GETFL).expect("F_GETFL");
        let flags = OFlag::from_bits_truncate(flags);
        fcntl(&pty.master, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).expect("F_SETFL");

        (pty.master, child)
    }

    fn spawn_mock(subcommand: &str) -> Option<(OwnedFd, Child)> {
        let bin = pty_mock_binary()?;
        Some(spawn_command(&bin, &[subcommand]))
    }

    fn read_pty_output(master: &OwnedFd, timeout_ms: u16) -> Vec<u8> {
        let mut result = Vec::new();
        let mut buf = [0u8; 65536];
        let mut poll_fds = [PollFd::new(master.as_fd(), PollFlags::POLLIN)];

        loop {
            match poll(&mut poll_fds, PollTimeout::from(timeout_ms)) {
                Ok(0) => break,
                Ok(_) => match read(master.as_fd(), &mut buf) {
                    Ok(0) => break,
                    Ok(n) => result.extend_from_slice(&buf[..n]),
                    Err(nix::errno::Errno::EAGAIN) => break,
                    Err(_) => break,
                },
                Err(_) => break,
            }
        }
        result
    }

    fn make_proxy(master: OwnedFd, child: Child) -> Proxy {
        let config = ProxyConfig::default();
        Proxy::new_for_test(master, child, config, 24, 80)
    }

    fn dev_null() -> OwnedFd {
        use std::fs::OpenOptions;
        let f = OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .expect("open /dev/null");
        OwnedFd::from(f)
    }

    fn dup_master(proxy: &Proxy) -> OwnedFd {
        let raw = proxy.pty_master_fd_raw();
        unsafe { OwnedFd::from_raw_fd(libc::dup(raw)) }
    }

    #[test]
    fn test_echo_baseline() {
        let Some((master, child)) = spawn_mock("echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        proxy
            .process_input(b"hello world\r\n", &sink)
            .expect("process_input");
        proxy.flush_drain_buffer(&sink).expect("flush drain");

        std::thread::sleep(Duration::from_millis(50));

        let master_dup = dup_master(&proxy);
        let output = read_pty_output(&master_dup, 100);
        assert!(!output.is_empty(), "expected output from echo child");
        proxy
            .process_output(&output, &sink)
            .expect("process_output");

        let mut history = Vec::new();
        proxy.history().append_all(&mut history);
        let history_str = String::from_utf8_lossy(&history);
        assert!(
            history_str.contains("hello world"),
            "history should contain echoed text, got: {:?}",
            history_str
        );
    }

    #[test]
    fn test_alt_screen_tracking() {
        let Some((master, child)) = spawn_mock("alt-screen") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        std::thread::sleep(Duration::from_millis(50));

        let master_dup = dup_master(&proxy);
        let output = read_pty_output(&master_dup, 100);
        if !output.is_empty() {
            proxy
                .process_output(&output, &sink)
                .expect("process_output");
        }

        assert!(
            proxy.is_in_alternate_screen(),
            "proxy should be in alt screen"
        );

        proxy.process_input(b"x", &sink).expect("process_input");
        proxy.flush_drain_buffer(&sink).expect("flush drain");

        std::thread::sleep(Duration::from_millis(50));

        let master_dup = dup_master(&proxy);
        let output = read_pty_output(&master_dup, 100);
        if !output.is_empty() {
            proxy
                .process_output(&output, &sink)
                .expect("process_output");
        }

        assert!(
            !proxy.is_in_alternate_screen(),
            "proxy should have exited alt screen"
        );
    }

    #[test]
    fn test_bracketed_paste_flow() {
        let Some((master, child)) = spawn_mock("paste-echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        std::thread::sleep(Duration::from_millis(50));
        let master_dup = dup_master(&proxy);
        let output = read_pty_output(&master_dup, 100);
        if !output.is_empty() {
            proxy
                .process_output(&output, &sink)
                .expect("process_output");
        }

        let mut paste = Vec::new();
        paste.extend_from_slice(b"\x1b[200~");
        paste.extend_from_slice(b"pasted content\r\n");
        paste.extend_from_slice(b"\x1b[201~");

        proxy.process_input(&paste, &sink).expect("process_input");
        proxy.flush_drain_buffer(&sink).expect("flush drain");

        assert!(
            !proxy.is_in_bracketed_paste(),
            "paste mode should be off after complete paste"
        );
    }

    #[test]
    fn test_split_paste_end_marker() {
        let Some((master, child)) = spawn_mock("paste-echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        std::thread::sleep(Duration::from_millis(50));
        let master_dup = dup_master(&proxy);
        let output = read_pty_output(&master_dup, 100);
        if !output.is_empty() {
            proxy
                .process_output(&output, &sink)
                .expect("process_output");
        }

        let mut chunk1 = Vec::new();
        chunk1.extend_from_slice(b"\x1b[200~");
        chunk1.extend_from_slice(b"split test\r\n");
        chunk1.extend_from_slice(b"\x1b[20"); // partial end marker

        proxy
            .process_input(&chunk1, &sink)
            .expect("process_input chunk1");
        proxy.flush_drain_buffer(&sink).expect("flush drain");

        assert!(
            proxy.is_in_bracketed_paste(),
            "should still be in paste mode after partial end marker"
        );

        proxy
            .process_input(b"1~", &sink)
            .expect("process_input chunk2");
        proxy.flush_drain_buffer(&sink).expect("flush drain");

        assert!(
            !proxy.is_in_bracketed_paste(),
            "paste mode should be off after completing split end marker"
        );
    }

    #[test]
    fn test_alt_screen_exit_restores_main_content() {
        let Some((master, child)) = spawn_mock("echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        // Simulate child writing main screen content
        let main_content = b"Claude Code v2.1.84\r\nOpus 4.6\r\n~/public/claude-chill\r\n";
        proxy
            .process_output(main_content, &sink)
            .expect("process_output main content");

        let screen_before = proxy.vt_screen_text();
        assert!(
            screen_before.contains("Claude Code v2.1.84"),
            "main content should be on screen before alt screen, got: {:?}",
            screen_before
        );

        // Child enters alt screen
        proxy
            .process_output(b"\x1b[?1049h", &sink)
            .expect("process_output alt enter");
        assert!(proxy.is_in_alternate_screen());

        // Child writes alt screen content (e.g. vim, less)
        proxy
            .process_output(b"alt screen editor content\r\nline 2\r\n", &sink)
            .expect("process_output alt content");

        // Child exits alt screen
        proxy
            .process_output(b"\x1b[?1049l", &sink)
            .expect("process_output alt exit");
        assert!(!proxy.is_in_alternate_screen());

        // The VT parser should have restored the main screen content
        let screen_after = proxy.vt_screen_text();
        assert!(
            screen_after.contains("Claude Code v2.1.84"),
            "main content should be restored after alt screen exit, got: {:?}",
            screen_after
        );
    }

    #[test]
    fn test_alt_screen_exit_restores_after_clear_screen_sync_block() {
        // Reproduces bug: sync block containing clear screen clears VT parser,
        // but render_vt_screen never fires before alt-screen-enter arrives
        // in the next chunk. VT parser saves blank screen. After alt exit,
        // contents_formatted() paints blank over the real terminal which
        // still had the original content (it never got the clear render).
        let Some((master, child)) = spawn_mock("echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        // Initial content via sync block — this gets rendered
        let mut initial_sync = Vec::new();
        initial_sync.extend_from_slice(b"\x1b[?2026h");
        initial_sync.extend_from_slice(b"\x1b[H");
        initial_sync.extend_from_slice(b"Claude Code v2.1.84\r\n");
        initial_sync.extend_from_slice(b"Opus 4.6\r\n");
        initial_sync.extend_from_slice(b"~/public/claude-chill\r\n");
        initial_sync.extend_from_slice(b"\x1b[?2026l");

        proxy
            .process_output(&initial_sync, &sink)
            .expect("initial sync block");

        let screen_before = proxy.vt_screen_text();
        assert!(
            screen_before.contains("Claude Code v2.1.84"),
            "header should be on screen initially"
        );

        // Sync block with clear screen (simulates ctrl-g response) —
        // VT parser processes the clear but render never fires
        let mut clear_sync = Vec::new();
        clear_sync.extend_from_slice(b"\x1b[?2026h");
        clear_sync.extend_from_slice(b"\x1b[2J"); // clear screen
        clear_sync.extend_from_slice(b"\x1b[H");
        clear_sync.extend_from_slice(b"\x1b[?2026l");

        proxy
            .process_output(&clear_sync, &sink)
            .expect("clear sync block");

        // VT parser screen is now blank (clear screen was processed)
        let screen_after_clear = proxy.vt_screen_text();
        assert!(
            !screen_after_clear.contains("Claude Code"),
            "VT screen should be blank after clear sync block"
        );

        // Alt screen enter in the NEXT chunk — no render_vt_screen fired
        // between the clear sync and this. VT parser saves blank screen.
        proxy
            .process_output(b"\x1b[?1049h", &sink)
            .expect("alt screen enter");
        assert!(proxy.is_in_alternate_screen());

        // Editor content during alt screen
        proxy
            .process_output(b"editor content\r\n", &sink)
            .expect("alt content");

        // Alt screen exit — VT restores saved (blank) screen
        proxy
            .process_output(b"\x1b[?1049l", &sink)
            .expect("alt screen exit");
        assert!(!proxy.is_in_alternate_screen());

        // Clear screen wiped the main buffer before alt-screen saved it.
        // Correct VT100 behavior: restored screen should be blank.
        let screen_after_exit = proxy.vt_screen_text();
        assert!(
            !screen_after_exit.contains("Claude Code v2.1.84"),
            "header should NOT survive clear + alt screen round-trip, got: {:?}",
            screen_after_exit
        );
    }

    #[test]
    fn test_sync_block_tracking() {
        let Some((master, child)) = spawn_mock("echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        // Start a sync block without ending it
        proxy
            .process_output(b"\x1b[?2026h some content", &sink)
            .expect("partial sync block");
        assert!(
            proxy.is_in_sync_block(),
            "should be in sync block after SYNC_START without SYNC_END"
        );

        // End the sync block
        proxy
            .process_output(b" more content \x1b[?2026l", &sink)
            .expect("sync block end");
        assert!(
            !proxy.is_in_sync_block(),
            "should not be in sync block after SYNC_END"
        );
    }

    #[test]
    fn test_lookback_mode_toggle() {
        let Some((master, child)) = spawn_mock("echo") else {
            return;
        };
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        // Generate some output so lookback has content
        proxy
            .process_output(b"line 1\r\nline 2\r\nline 3\r\n", &sink)
            .expect("output");

        assert!(!proxy.is_in_lookback_mode());

        // Send legacy lookback key (0x1E = Ctrl+^)
        send_input(&mut proxy, &sink, &[0x1E]);
        assert!(
            proxy.is_in_lookback_mode(),
            "should enter lookback mode after Ctrl+^"
        );

        // Send it again to exit
        send_input(&mut proxy, &sink, &[0x1E]);
        assert!(
            !proxy.is_in_lookback_mode(),
            "should exit lookback mode after second Ctrl+^"
        );
    }

    fn pump_proxy(proxy: &mut Proxy, sink: &OwnedFd, timeout_ms: u16) {
        let master_dup = dup_master(proxy);
        let output = read_pty_output(&master_dup, timeout_ms);
        if !output.is_empty() {
            proxy.process_output(&output, sink).expect("process_output");
        }
    }

    fn send_input(proxy: &mut Proxy, sink: &OwnedFd, data: &[u8]) {
        proxy.process_input(data, sink).expect("process_input");
        proxy.flush_drain_buffer(sink).expect("flush drain");
    }

    fn wait_for_screen(proxy: &mut Proxy, sink: &OwnedFd, text: &str, timeout: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            pump_proxy(proxy, sink, 100);
            if proxy.vt_screen_text().contains(text) {
                return true;
            }
        }
        false
    }

    #[test]
    #[ignore] // requires claude CLI to be installed and authenticated
    fn test_claude_alt_screen_restore() {
        let (master, child) = spawn_command("claude", &[]);
        let mut proxy = make_proxy(master, child);
        let sink = dev_null();

        // Wait for Claude Code to start up and show its UI
        assert!(
            wait_for_screen(&mut proxy, &sink, "Claude Code", Duration::from_secs(10)),
            "Claude Code did not start, screen: {:?}",
            proxy.vt_screen_text()
        );

        let screen_before = proxy.vt_screen_text();
        eprintln!("Screen before alt screen:\n{}", screen_before);
        assert!(
            screen_before.contains("▐▛███▜▌"),
            "logo should be present before alt screen"
        );

        // Send Ctrl-G to open the editor (enters alt screen)
        send_input(&mut proxy, &sink, &[0x07]); // Ctrl-G

        // Wait for alt screen to be entered
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            pump_proxy(&mut proxy, &sink, 100);
            if proxy.is_in_alternate_screen() {
                break;
            }
        }
        assert!(
            proxy.is_in_alternate_screen(),
            "should have entered alt screen after Ctrl-G"
        );

        // Pump until editor has rendered
        pump_proxy(&mut proxy, &sink, 500);

        // Send :wq<Enter> to close the editor (exits alt screen)
        send_input(&mut proxy, &sink, b"\x1b");
        pump_proxy(&mut proxy, &sink, 100);
        send_input(&mut proxy, &sink, b":wq\r");

        // Wait for alt screen to be exited
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            pump_proxy(&mut proxy, &sink, 100);
            if !proxy.is_in_alternate_screen() {
                break;
            }
        }

        // Drain remaining output after alt screen exit
        pump_proxy(&mut proxy, &sink, 500);

        let vt_screen_after = proxy.vt_screen_text();

        eprintln!("VT parser screen after alt exit:\n{}", vt_screen_after);

        // Claude Code bug: after editor exit, the logo (▐▛███▜▌) is not
        // redrawn. This happens with or without claude-chill.
        // Update this assertion if Claude Code fixes the redraw.
        assert!(
            !vt_screen_after.contains("▐▛███▜▌"),
            "logo should NOT appear after alt screen exit (known Claude Code bug), got: {:?}",
            vt_screen_after
        );

        // Send /exit to quit
        send_input(&mut proxy, &sink, b"/exit\r");
        pump_proxy(&mut proxy, &sink, 1000);
    }
}
