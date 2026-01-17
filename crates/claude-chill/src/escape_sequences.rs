pub const SYNC_START: &[u8] = b"\x1b[?2026h";
pub const SYNC_END: &[u8] = b"\x1b[?2026l";
pub const CLEAR_SCREEN: &[u8] = b"\x1b[2J";
pub const CLEAR_SCROLLBACK: &[u8] = b"\x1b[3J";
pub const CURSOR_HOME: &[u8] = b"\x1b[H";

pub const SYNC_BUFFER_CAPACITY: usize = 1024 * 1024;
pub const OUTPUT_BUFFER_CAPACITY: usize = 32768;
pub const INPUT_BUFFER_CAPACITY: usize = 64;
