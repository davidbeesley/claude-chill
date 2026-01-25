/// Filters out terminal query escape sequences that would cause the terminal
/// to respond via stdin when replayed.
///
/// Query sequences include:
/// - CSI c, CSI 0c, CSI >c, CSI >0c, CSI =c (Device Attributes DA1/DA2/DA3)
/// - CSI 5n, CSI 6n, CSI ?6n (Device Status Reports / Cursor Position)
/// - CSI ?u, CSI ?Nu (Kitty keyboard protocol queries)
/// - CSI >q (XTVERSION query)
/// - CSI Ps $p, CSI ?Ps $p (DECRQM - Request Mode)
/// - CSI 14t, CSI 16t, CSI 18t, etc. (XTWINOPS window queries)
/// - DCS $ q ... ST (DECRQSS - Request Selection or Setting)
/// - DCS + q ... ST (XTGETTCAP - Request Termcap/Terminfo)
/// - OSC N;? ST (color/property queries)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum FilterState {
    #[default]
    Normal,
    Escape,                 // Saw ESC
    Csi,                    // Saw ESC [
    CsiParam,               // Saw ESC [ followed by params
    CsiParamDollar,         // Saw ESC [ N... $ (DECRQM prefix)
    CsiGt,                  // Saw ESC [ >
    CsiGtParam,             // Saw ESC [ > followed by params
    CsiEq,                  // Saw ESC [ =
    CsiQuestion,            // Saw ESC [ ?
    CsiQuestionParam,       // Saw ESC [ ? followed by params
    CsiQuestionParamDollar, // Saw ESC [ ? N... $ (DECRQM private prefix)
    Osc,                    // Saw ESC ]
    OscParam,               // Inside OSC, collecting param number
    OscSemicolon,           // Saw ; in OSC
    OscQuery,               // Saw ? after ; in OSC (query sequence)
    OscQuerySt,             // Saw ESC in OSC query, looking for \
    Dcs,                    // Saw ESC P (Device Control String)
    DcsCollect,             // Collecting DCS content
    DcsEscape,              // Saw ESC in DCS, looking for \
}

/// Stateful filter for terminal query sequences.
/// Maintains state across multiple filter() calls to handle sequences
/// that are split across chunk boundaries.
#[derive(Debug, Default)]
pub struct TerminalQueryFilter {
    state: FilterState,
    pending: Vec<u8>,
}

impl TerminalQueryFilter {
    pub fn new() -> Self {
        Self {
            state: FilterState::Normal,
            pending: Vec::with_capacity(32),
        }
    }

    /// Filter terminal query sequences from input bytes.
    /// Returns filtered output. Maintains state for sequences split across calls.
    pub fn filter(&mut self, input: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(input.len());

        for &byte in input {
            match self.state {
                FilterState::Normal => {
                    if byte == 0x1B {
                        self.state = FilterState::Escape;
                        self.pending.clear();
                        self.pending.push(byte);
                    } else {
                        output.push(byte);
                    }
                }

                FilterState::Escape => {
                    self.pending.push(byte);
                    match byte {
                        b'[' => self.state = FilterState::Csi,
                        b']' => self.state = FilterState::Osc,
                        b'P' => self.state = FilterState::Dcs,
                        _ => {
                            // Not a sequence we care about, emit pending
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::Csi => {
                    self.pending.push(byte);
                    match byte {
                        b'>' => self.state = FilterState::CsiGt,
                        b'=' => self.state = FilterState::CsiEq,
                        b'?' => self.state = FilterState::CsiQuestion,
                        b'c' => {
                            // ESC [ c - Primary DA query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'0'..=b'9' => self.state = FilterState::CsiParam,
                        _ => {
                            // Unknown CSI sequence, emit
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiParam => {
                    self.pending.push(byte);
                    match byte {
                        b'0'..=b'9' | b';' => {} // Continue collecting params
                        b'c' => {
                            // ESC [ 0 c or similar - Primary DA query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'n' => {
                            // Check if this is 5n or 6n (Device Status Reports)
                            if is_device_status_query(&self.pending) {
                                // Discard query
                                self.pending.clear();
                            } else {
                                output.extend_from_slice(&self.pending);
                                self.pending.clear();
                            }
                            self.state = FilterState::Normal;
                        }
                        b't' => {
                            // Check if this is an XTWINOPS query (14t, 16t, 18t, etc.)
                            if is_window_query(&self.pending) {
                                self.pending.clear();
                            } else {
                                output.extend_from_slice(&self.pending);
                                self.pending.clear();
                            }
                            self.state = FilterState::Normal;
                        }
                        b'$' => {
                            // Possible DECRQM prefix
                            self.state = FilterState::CsiParamDollar;
                        }
                        _ => {
                            // End of CSI sequence, emit
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiGt => {
                    self.pending.push(byte);
                    match byte {
                        b'c' => {
                            // ESC [ > c - Secondary DA query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'q' => {
                            // ESC [ > q - XTVERSION query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'0'..=b'9' => self.state = FilterState::CsiGtParam,
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiGtParam => {
                    self.pending.push(byte);
                    match byte {
                        b'0'..=b'9' => {} // Continue
                        b'c' => {
                            // ESC [ > 0 c - Secondary DA query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiEq => {
                    self.pending.push(byte);
                    match byte {
                        b'c' => {
                            // ESC [ = c - Tertiary DA query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiQuestion => {
                    self.pending.push(byte);
                    match byte {
                        b'u' => {
                            // ESC [ ? u - Kitty keyboard protocol query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'0'..=b'9' => self.state = FilterState::CsiQuestionParam,
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiQuestionParam => {
                    self.pending.push(byte);
                    match byte {
                        b'0'..=b'9' | b';' => {} // Continue
                        b'n' => {
                            // ESC [ ? N n - Extended cursor position query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'u' => {
                            // ESC [ ? N u - Kitty keyboard protocol query, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        b'$' => {
                            // Possible DECRQM private mode prefix
                            self.state = FilterState::CsiQuestionParamDollar;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::Osc => {
                    self.pending.push(byte);
                    match byte {
                        b'0'..=b'9' => self.state = FilterState::OscParam,
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::OscParam => {
                    self.pending.push(byte);
                    match byte {
                        b'0'..=b'9' => {} // Continue
                        b';' => self.state = FilterState::OscSemicolon,
                        0x07 | 0x1B => {
                            // End of OSC (BEL or ST), not a query
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::OscSemicolon => {
                    self.pending.push(byte);
                    match byte {
                        b'?' => self.state = FilterState::OscQuery,
                        0x07 => {
                            // Not a query, emit
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        0x1B => {
                            // Start of ST, not a query
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            // Some other content, not a query
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::OscQuery => {
                    self.pending.push(byte);
                    match byte {
                        0x07 => {
                            // ESC ] N ; ? BEL - Query sequence, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        0x1B => self.state = FilterState::OscQuerySt,
                        _ => {
                            // Not a simple query, might be content starting with ?
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::OscQuerySt => {
                    self.pending.push(byte);
                    match byte {
                        b'\\' => {
                            // ESC ] N ; ? ESC \ - Query sequence, discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            // Not ST, emit what we have
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiParamDollar => {
                    self.pending.push(byte);
                    match byte {
                        b'p' => {
                            // ESC [ Ps $ p - DECRQM (Request Mode), discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::CsiQuestionParamDollar => {
                    self.pending.push(byte);
                    match byte {
                        b'p' => {
                            // ESC [ ? Ps $ p - DECRQM private (Request Mode), discard
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            output.extend_from_slice(&self.pending);
                            self.pending.clear();
                            self.state = FilterState::Normal;
                        }
                    }
                }

                FilterState::Dcs => {
                    self.pending.push(byte);
                    match byte {
                        b'$' | b'+' => {
                            // ESC P $ or ESC P + - start of query (DECRQSS or XTGETTCAP)
                            self.state = FilterState::DcsCollect;
                        }
                        0x1B => {
                            // ESC in DCS - might be ST
                            self.state = FilterState::DcsEscape;
                        }
                        _ => {
                            // Other DCS content, collect but we'll discard queries
                            self.state = FilterState::DcsCollect;
                        }
                    }
                }

                FilterState::DcsCollect => {
                    self.pending.push(byte);
                    if byte == 0x1B {
                        // ESC in DCS - looking for ST
                        self.state = FilterState::DcsEscape;
                    }
                }

                FilterState::DcsEscape => {
                    self.pending.push(byte);
                    match byte {
                        b'\\' => {
                            // ESC \ - ST, end of DCS
                            // Check if this was a query (starts with $ or +)
                            if is_dcs_query(&self.pending) {
                                // Discard query
                                self.pending.clear();
                            } else {
                                output.extend_from_slice(&self.pending);
                                self.pending.clear();
                            }
                            self.state = FilterState::Normal;
                        }
                        _ => {
                            // Not ST, continue collecting
                            self.state = FilterState::DcsCollect;
                        }
                    }
                }
            }
        }

        // Don't emit pending bytes - keep them for next call
        // They'll be emitted when the sequence completes

        output
    }

    /// Flush any pending bytes. Call this when done filtering to get
    /// any incomplete sequences that should be emitted.
    pub fn flush(&mut self) -> Vec<u8> {
        let result = std::mem::take(&mut self.pending);
        self.state = FilterState::Normal;
        result
    }
}

/// Check if pending buffer is a device status query (5n or 6n)
fn is_device_status_query(pending: &[u8]) -> bool {
    // pending should be like [ESC, '[', '5', 'n'] or [ESC, '[', '6', 'n']
    if pending.len() < 4 {
        return false;
    }

    // Check for patterns like ESC [ 5 n or ESC [ 6 n
    // The number part could be just '5' or '6'
    let param_start = 2;
    let param_end = pending.len() - 1; // exclude the 'n'
    let param_slice = &pending[param_start..param_end];

    // Parse the parameter
    if let Ok(param_str) = std::str::from_utf8(param_slice)
        && let Ok(param) = param_str.parse::<u32>()
    {
        return param == 5 || param == 6;
    }

    false
}

fn is_window_query(pending: &[u8]) -> bool {
    // XTWINOPS queries: ESC [ Ps t where Ps is a query operation
    // Query operations that cause terminal to respond:
    // 11 - Report window state (iconified/normal)
    // 13 - Report window position
    // 14 - Report window size in pixels
    // 15 - Report screen size in pixels
    // 16 - Report character cell size in pixels
    // 18 - Report window size in characters
    // 19 - Report screen size in characters
    // 20 - Report icon label
    // 21 - Report window title
    if pending.len() < 4 {
        return false;
    }

    let param_start = 2;
    let param_end = pending.len() - 1; // exclude the 't'
    let param_slice = &pending[param_start..param_end];

    if let Ok(param_str) = std::str::from_utf8(param_slice) {
        // Handle single parameter or first parameter before semicolon
        let first_param = param_str.split(';').next().unwrap_or("");
        if let Ok(param) = first_param.parse::<u32>() {
            return matches!(param, 11 | 13 | 14 | 15 | 16 | 18 | 19 | 20 | 21);
        }
    }

    false
}

fn is_dcs_query(pending: &[u8]) -> bool {
    // DCS queries start with ESC P followed by $ or +
    // ESC P $ q ... ESC \ - DECRQSS (Request Selection or Setting)
    // ESC P + q ... ESC \ - XTGETTCAP (Request Termcap/Terminfo)
    if pending.len() < 4 {
        return false;
    }

    // Check for ESC P $ or ESC P +
    pending.len() >= 3
        && pending[0] == 0x1B
        && pending[1] == b'P'
        && (pending[2] == b'$' || pending[2] == b'+')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_filter_normal_text() {
        let mut filter = TerminalQueryFilter::new();
        let input = b"Hello, World!";
        let output = filter.filter(input);
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_no_filter_normal_escape_sequences() {
        let mut filter = TerminalQueryFilter::new();
        // Color codes should pass through
        let input = b"\x1b[31mRed\x1b[0m";
        let output = filter.filter(input);
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_filter_primary_da_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ c
        let input = b"before\x1b[cafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_primary_da_query_with_param() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 0 c
        let input = b"before\x1b[0cafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_secondary_da_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ > c
        let input = b"before\x1b[>cafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_secondary_da_query_with_param() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ > 0 c
        let input = b"before\x1b[>0cafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_tertiary_da_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ = c
        let input = b"before\x1b[=cafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_device_status_5n() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 5 n
        let input = b"before\x1b[5nafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_device_status_6n() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 6 n
        let input = b"before\x1b[6nafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_extended_cursor_position() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? 6 n
        let input = b"before\x1b[?6nafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_osc_query_with_bel() {
        let mut filter = TerminalQueryFilter::new();
        // ESC ] 11 ; ? BEL
        let input = b"before\x1b]11;?\x07after";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_osc_query_with_st() {
        let mut filter = TerminalQueryFilter::new();
        // ESC ] 11 ; ? ESC \
        let input = b"before\x1b]11;?\x1b\\after";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_no_filter_osc_set() {
        let mut filter = TerminalQueryFilter::new();
        // ESC ] 11 ; color BEL - setting color, not query
        let input = b"before\x1b]11;rgb:00/00/00\x07after";
        let output = filter.filter(input);
        // This should pass through since it's not a query (doesn't start with ?)
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_filter_multiple_queries() {
        let mut filter = TerminalQueryFilter::new();
        let input = b"start\x1b[c\x1b]11;?\x07\x1b[6nend";
        let output = filter.filter(input);
        assert_eq!(output, b"startend".to_vec());
    }

    #[test]
    fn test_preserve_cursor_movement() {
        let mut filter = TerminalQueryFilter::new();
        // Cursor movement should not be filtered
        let input = b"\x1b[H\x1b[2J";
        let output = filter.filter(input);
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_split_sequence_query_filtered() {
        let mut filter = TerminalQueryFilter::new();
        // Split ESC [ c across two calls - should still be filtered
        let output1 = filter.filter(b"before\x1b[");
        let output2 = filter.filter(b"cafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_sequence_non_query_preserved() {
        let mut filter = TerminalQueryFilter::new();
        // Split ESC [ 3 1 m across two calls - should be preserved
        let output1 = filter.filter(b"before\x1b[31");
        let output2 = filter.filter(b"mafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"\x1b[31mafter".to_vec());
    }

    #[test]
    fn test_flush_incomplete_sequence() {
        let mut filter = TerminalQueryFilter::new();
        // Incomplete sequence at end
        let output = filter.filter(b"text\x1b[");
        assert_eq!(output, b"text".to_vec());
        // Flush should emit the incomplete sequence
        let flushed = filter.flush();
        assert_eq!(flushed, b"\x1b[".to_vec());
    }

    #[test]
    fn test_filter_kitty_keyboard_query_simple() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? u - Kitty keyboard protocol query
        let input = b"before\x1b[?uafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_kitty_keyboard_query_with_param() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? 1 u - Kitty keyboard query with flags
        let input = b"before\x1b[?1uafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_xtversion_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ > q - XTVERSION query
        let input = b"before\x1b[>qafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_decrqm_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 4 $ p - DECRQM (Request Mode)
        let input = b"before\x1b[4$pafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_decrqm_private_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? 1 $ p - DECRQM private (Request Mode)
        let input = b"before\x1b[?1$pafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_xtwinops_window_size_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 14 t - Report window size in pixels
        let input = b"before\x1b[14tafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_xtwinops_title_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 21 t - Report window title
        let input = b"before\x1b[21tafter";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_no_filter_xtwinops_non_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 1 t - De-iconify window (not a query)
        let input = b"before\x1b[1tafter";
        let output = filter.filter(input);
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_filter_dcs_decrqss() {
        let mut filter = TerminalQueryFilter::new();
        // ESC P $ q m ESC \ - DECRQSS query for SGR
        let input = b"before\x1bP$qm\x1b\\after";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_filter_dcs_xtgettcap() {
        let mut filter = TerminalQueryFilter::new();
        // ESC P + q 544e ESC \ - XTGETTCAP query
        let input = b"before\x1bP+q544e\x1b\\after";
        let output = filter.filter(input);
        assert_eq!(output, b"beforeafter".to_vec());
    }

    #[test]
    fn test_split_kitty_keyboard_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? u split across reads
        let output1 = filter.filter(b"before\x1b[");
        let output2 = filter.filter(b"?uafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_kitty_keyboard_query_with_param() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? 1 u split after the digit
        let output1 = filter.filter(b"before\x1b[?1");
        let output2 = filter.filter(b"uafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_xtversion_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ > q split after >
        let output1 = filter.filter(b"before\x1b[>");
        let output2 = filter.filter(b"qafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_decrqm_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 4 $ p split after $
        let output1 = filter.filter(b"before\x1b[4$");
        let output2 = filter.filter(b"pafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_decrqm_private_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ ? 1 $ p split after $
        let output1 = filter.filter(b"before\x1b[?1$");
        let output2 = filter.filter(b"pafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_xtwinops_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 14 t split after 1
        let output1 = filter.filter(b"before\x1b[1");
        let output2 = filter.filter(b"4tafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_dcs_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC P $ q m ESC \ split after ESC P
        let output1 = filter.filter(b"before\x1bP");
        let output2 = filter.filter(b"$qm\x1b\\after");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_dcs_query_at_st() {
        let mut filter = TerminalQueryFilter::new();
        // ESC P $ q m ESC \ split before ST
        let output1 = filter.filter(b"before\x1bP$qm\x1b");
        let output2 = filter.filter(b"\\after");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_osc_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC ] 11 ; ? BEL split after ;
        let output1 = filter.filter(b"before\x1b]11;");
        let output2 = filter.filter(b"?\x07after");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }

    #[test]
    fn test_split_device_status_query() {
        let mut filter = TerminalQueryFilter::new();
        // ESC [ 6 n split after 6
        let output1 = filter.filter(b"before\x1b[6");
        let output2 = filter.filter(b"nafter");
        assert_eq!(output1, b"before".to_vec());
        assert_eq!(output2, b"after".to_vec());
    }
}
