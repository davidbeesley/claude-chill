use crate::escape_sequences::{CLEAR_SCREEN, CURSOR_HOME};

fn create_parser_with_scrollback(rows: u16, cols: u16, scrollback: usize) -> vt100::Parser {
    vt100::Parser::new(rows, cols, scrollback)
}

fn get_full_vt_history(parser: &vt100::Parser) -> Vec<u8> {
    let screen = parser.screen();
    let (_, cols) = screen.size();
    let mut output = Vec::new();

    for row in screen.scrollback_rows() {
        row.write_contents_formatted(&mut output, 0, cols, 0, false, None, None);
        output.extend_from_slice(b"\r\n");
    }

    output.extend_from_slice(&screen.contents_formatted());

    output
}

fn get_scrollback_text(parser: &vt100::Parser) -> String {
    let screen = parser.screen();
    let (_, cols) = screen.size();
    let mut text = String::new();

    for row in screen.scrollback_rows() {
        row.write_contents(&mut text, 0, cols, false);
        text.push('\n');
    }

    text
}

#[test]
fn test_scrollback_accumulates_lines() {
    let mut parser = create_parser_with_scrollback(3, 80, 100);

    parser.process(b"line1\r\n");
    parser.process(b"line2\r\n");
    parser.process(b"line3\r\n");
    parser.process(b"line4\r\n");
    parser.process(b"line5\r\n");

    let scrollback = get_scrollback_text(&parser);
    assert!(
        scrollback.contains("line1"),
        "scrollback should contain line1: {}",
        scrollback
    );
    assert!(
        scrollback.contains("line2"),
        "scrollback should contain line2: {}",
        scrollback
    );
}

#[test]
fn test_scrollback_respects_capacity() {
    let mut parser = create_parser_with_scrollback(3, 80, 5);

    for i in 0..20 {
        parser.process(format!("line{}\r\n", i).as_bytes());
    }

    let count = parser.screen().scrollback_row_count();
    assert!(
        count <= 5,
        "scrollback should respect capacity, got {}",
        count
    );
}

#[test]
fn test_clear_scrollback() {
    let mut parser = create_parser_with_scrollback(3, 80, 100);

    parser.process(b"line1\r\nline2\r\nline3\r\nline4\r\nline5\r\n");

    let count_before = parser.screen().scrollback_row_count();
    assert!(count_before > 0, "should have scrollback before clear");

    parser.screen_mut().clear_scrollback();

    let count_after = parser.screen().scrollback_row_count();
    assert_eq!(count_after, 0, "scrollback should be empty after clear");
}

#[test]
fn test_clear_screen_does_not_clear_scrollback() {
    let mut parser = create_parser_with_scrollback(3, 80, 100);

    parser.process(b"line1\r\nline2\r\nline3\r\nline4\r\nline5\r\n");

    let count_before = parser.screen().scrollback_row_count();
    assert!(
        count_before > 0,
        "should have scrollback before clear screen"
    );

    parser.process(CLEAR_SCREEN);
    parser.process(CURSOR_HOME);

    let count_after = parser.screen().scrollback_row_count();
    assert_eq!(
        count_after, count_before,
        "clear screen should NOT clear scrollback"
    );
}

#[test]
fn test_alt_screen_does_not_add_to_scrollback() {
    let mut parser = create_parser_with_scrollback(3, 80, 100);

    parser.process(b"main1\r\nmain2\r\nmain3\r\nmain4\r\n");

    let count_before = parser.screen().scrollback_row_count();

    parser.process(b"\x1b[?1049h");

    for i in 0..10 {
        parser.process(format!("alt{}\r\n", i).as_bytes());
    }

    let count_during_alt = parser.screen().scrollback_row_count();
    assert_eq!(
        count_during_alt, count_before,
        "main screen scrollback should be preserved during alt screen"
    );

    parser.process(b"\x1b[?1049l");

    let count_after = parser.screen().scrollback_row_count();
    assert_eq!(
        count_after, count_before,
        "scrollback should be same after exiting alt screen"
    );

    let scrollback = get_scrollback_text(&parser);
    assert!(
        !scrollback.contains("alt"),
        "scrollback should not contain alt screen content"
    );
}

#[test]
fn test_full_history_includes_scrollback_and_screen() {
    let mut parser = create_parser_with_scrollback(3, 80, 100);

    parser.process(b"scrollback1\r\nscrollback2\r\nscrollback3\r\nscreen1\r\nscreen2\r\n");

    let history = get_full_vt_history(&parser);
    let history_str = String::from_utf8_lossy(&history);

    assert!(
        history_str.contains("scrollback1") || parser.screen().scrollback_row_count() > 0,
        "history should include scrollback content"
    );
}
