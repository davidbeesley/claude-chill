use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseKeyError {
    pub raw: String,
    pub reason: String,
}

impl ParseKeyError {
    pub fn new(raw: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot parse {:?} as key: {}", self.raw, self.reason)
    }
}

impl std::error::Error for ParseKeyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    F(u8),
    Enter,
    Esc,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Space,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyCombination {
    pub code: KeyCode,
    pub modifiers: Modifiers,
}

impl KeyCombination {
    pub fn to_escape_sequence(&self) -> Vec<u8> {
        key_to_escape_sequence(&self.code, &self.modifiers)
    }
}

impl fmt::Display for KeyCombination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.ctrl {
            write!(f, "[ctrl]")?;
        }
        if self.modifiers.shift {
            write!(f, "[shift]")?;
        }
        if self.modifiers.alt {
            write!(f, "[alt]")?;
        }
        let key_name = match &self.code {
            KeyCode::Char(c) => format!("[{}]", c),
            KeyCode::F(n) => format!("[f{}]", n),
            KeyCode::Enter => "[enter]".to_string(),
            KeyCode::Esc => "[esc]".to_string(),
            KeyCode::Tab => "[tab]".to_string(),
            KeyCode::Backspace => "[backspace]".to_string(),
            KeyCode::Delete => "[delete]".to_string(),
            KeyCode::Insert => "[insert]".to_string(),
            KeyCode::Home => "[home]".to_string(),
            KeyCode::End => "[end]".to_string(),
            KeyCode::PageUp => "[pageup]".to_string(),
            KeyCode::PageDown => "[pagedown]".to_string(),
            KeyCode::Up => "[up]".to_string(),
            KeyCode::Down => "[down]".to_string(),
            KeyCode::Left => "[left]".to_string(),
            KeyCode::Right => "[right]".to_string(),
            KeyCode::Space => "[space]".to_string(),
        };
        write!(f, "{}", key_name)
    }
}

pub fn parse(raw: &str) -> Result<KeyCombination, ParseKeyError> {
    let raw_lower = raw.to_ascii_lowercase();
    let mut modifiers = Modifiers::default();
    let mut key_code: Option<KeyCode> = None;

    let mut i = 0;
    let chars: Vec<char> = raw_lower.chars().collect();

    while i < chars.len() {
        if chars[i] != '[' {
            return Err(ParseKeyError::new(
                raw,
                format!("expected '[' at position {}", i),
            ));
        }

        let start = i + 1;
        let mut end = start;
        while end < chars.len() && chars[end] != ']' {
            end += 1;
        }

        if end >= chars.len() {
            return Err(ParseKeyError::new(raw, "unclosed bracket"));
        }

        let token: String = chars[start..end].iter().collect();
        i = end + 1;

        match token.as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" => modifiers.alt = true,
            _ => {
                if key_code.is_some() {
                    return Err(ParseKeyError::new(raw, "multiple key codes specified"));
                }
                key_code = Some(parse_key_code(&token, raw)?);
            }
        }
    }

    match key_code {
        Some(code) => Ok(KeyCombination { code, modifiers }),
        None => Err(ParseKeyError::new(raw, "no key code specified")),
    }
}

fn parse_key_code(token: &str, raw: &str) -> Result<KeyCode, ParseKeyError> {
    let code = match token {
        "[" => KeyCode::Char('['),
        "]" => KeyCode::Char(']'),
        "esc" | "escape" => KeyCode::Esc,
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "space" => KeyCode::Space,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        s if s.len() == 1 => KeyCode::Char(s.chars().next().unwrap_or(' ')),
        _ => return Err(ParseKeyError::new(raw, format!("unknown key: {}", token))),
    };
    Ok(code)
}

fn key_to_escape_sequence(code: &KeyCode, modifiers: &Modifiers) -> Vec<u8> {
    let modifier_code = match (modifiers.ctrl, modifiers.shift, modifiers.alt) {
        (false, false, false) => 0,
        _ => 1 + modifiers.shift as u8 + (modifiers.alt as u8 * 2) + (modifiers.ctrl as u8 * 4),
    };

    match code {
        KeyCode::PageUp => modified_key(b"5", modifier_code),
        KeyCode::PageDown => modified_key(b"6", modifier_code),
        KeyCode::Home => {
            if modifier_code == 0 {
                b"\x1b[H".to_vec()
            } else {
                format!("\x1b[1;{}H", modifier_code).into_bytes()
            }
        }
        KeyCode::End => {
            if modifier_code == 0 {
                b"\x1b[F".to_vec()
            } else {
                format!("\x1b[1;{}F", modifier_code).into_bytes()
            }
        }
        KeyCode::Up => arrow_key(b'A', modifier_code),
        KeyCode::Down => arrow_key(b'B', modifier_code),
        KeyCode::Right => arrow_key(b'C', modifier_code),
        KeyCode::Left => arrow_key(b'D', modifier_code),
        KeyCode::Insert => modified_key(b"2", modifier_code),
        KeyCode::Delete => modified_key(b"3", modifier_code),
        KeyCode::F(n) => function_key(*n, modifier_code),
        KeyCode::Enter => {
            if modifiers.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        KeyCode::Tab => {
            if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Backspace => {
            if modifiers.ctrl {
                vec![0x08]
            } else {
                vec![0x7f]
            }
        }
        KeyCode::Space => {
            if modifiers.ctrl {
                vec![0x00]
            } else {
                b" ".to_vec()
            }
        }
        KeyCode::Char(c) => char_to_escape_sequence(*c, modifiers),
    }
}

fn char_to_escape_sequence(c: char, modifiers: &Modifiers) -> Vec<u8> {
    if modifiers.ctrl {
        let ctrl_byte = match c {
            'a'..='z' => Some((c.to_ascii_uppercase() as u8) - b'A' + 1),
            'A'..='Z' => Some((c as u8) - b'A' + 1),
            '@' => Some(0x00),
            '[' => Some(0x1B),
            '\\' => Some(0x1C),
            ']' => Some(0x1D),
            '^' | '6' => Some(0x1E),
            '_' | '7' => Some(0x1F),
            '2' => Some(0x00),
            '3' => Some(0x1B),
            '4' => Some(0x1C),
            '5' => Some(0x1D),
            '8' => Some(0x7F),
            _ => None,
        };
        if let Some(byte) = ctrl_byte {
            return if modifiers.alt {
                vec![0x1b, byte]
            } else {
                vec![byte]
            };
        }
    }
    if modifiers.alt {
        vec![0x1b, c as u8]
    } else if modifiers.shift {
        vec![c.to_ascii_uppercase() as u8]
    } else {
        vec![c as u8]
    }
}

fn modified_key(base: &[u8], modifier: u8) -> Vec<u8> {
    if modifier == 0 {
        format!("\x1b[{}~", std::str::from_utf8(base).unwrap_or("")).into_bytes()
    } else {
        format!(
            "\x1b[{};{}~",
            std::str::from_utf8(base).unwrap_or(""),
            modifier
        )
        .into_bytes()
    }
}

fn arrow_key(direction: u8, modifier: u8) -> Vec<u8> {
    if modifier == 0 {
        vec![0x1b, b'[', direction]
    } else {
        format!("\x1b[1;{}{}", modifier, direction as char).into_bytes()
    }
}

fn function_key(n: u8, modifier: u8) -> Vec<u8> {
    let code = match n {
        1 => 11,
        2 => 12,
        3 => 13,
        4 => 14,
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return b"\x1b[24~".to_vec(),
    };

    if modifier == 0 {
        format!("\x1b[{}~", code).into_bytes()
    } else {
        format!("\x1b[{};{}~", code, modifier).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_key() {
        let key = parse("[f12]").unwrap();
        assert_eq!(key.code, KeyCode::F(12));
        assert!(!key.modifiers.ctrl);
        assert!(!key.modifiers.shift);
    }

    #[test]
    fn test_parse_ctrl_shift_pageup() {
        let key = parse("[ctrl][shift][pageup]").unwrap();
        assert_eq!(key.code, KeyCode::PageUp);
        assert!(key.modifiers.ctrl);
        assert!(key.modifiers.shift);
        assert!(!key.modifiers.alt);
    }

    #[test]
    fn test_parse_alt_enter() {
        let key = parse("[alt][enter]").unwrap();
        assert_eq!(key.code, KeyCode::Enter);
        assert!(key.modifiers.alt);
    }

    #[test]
    fn test_parse_single_char() {
        let key = parse("[ctrl][c]").unwrap();
        assert_eq!(key.code, KeyCode::Char('c'));
        assert!(key.modifiers.ctrl);
    }

    #[test]
    fn test_escape_sequence_ctrl_shift_pageup() {
        let key = parse("[ctrl][shift][pageup]").unwrap();
        assert_eq!(key.to_escape_sequence(), b"\x1b[5;6~".to_vec());
    }

    #[test]
    fn test_escape_sequence_f12() {
        let key = parse("[f12]").unwrap();
        assert_eq!(key.to_escape_sequence(), b"\x1b[24~".to_vec());
    }

    #[test]
    fn test_escape_sequence_ctrl_c() {
        let key = parse("[ctrl][c]").unwrap();
        assert_eq!(key.to_escape_sequence(), vec![0x03]);
    }

    #[test]
    fn test_display() {
        let key = parse("[ctrl][shift][pageup]").unwrap();
        assert_eq!(key.to_string(), "[ctrl][shift][pageup]");
    }

    #[test]
    fn test_case_insensitive() {
        let key = parse("[CTRL][SHIFT][PAGEUP]").unwrap();
        assert_eq!(key.code, KeyCode::PageUp);
        assert!(key.modifiers.ctrl);
        assert!(key.modifiers.shift);
    }

    #[test]
    fn test_error_unclosed_bracket() {
        let result = parse("[ctrl");
        assert!(result.is_err());
    }

    #[test]
    fn test_error_no_key() {
        let result = parse("[ctrl][shift]");
        assert!(result.is_err());
    }

    #[test]
    fn test_ctrl_caret() {
        let key = parse("[ctrl][^]").unwrap();
        assert_eq!(key.to_escape_sequence(), vec![0x1E]);
    }

    #[test]
    fn test_ctrl_6_same_as_ctrl_caret() {
        let key = parse("[ctrl][6]").unwrap();
        assert_eq!(key.to_escape_sequence(), vec![0x1E]);
    }

    #[test]
    fn test_ctrl_bracket() {
        let key = parse("[ctrl][[]").unwrap();
        assert_eq!(key.to_escape_sequence(), vec![0x1B]);
    }

    #[test]
    fn test_ctrl_backslash() {
        let key = parse("[ctrl][\\]").unwrap();
        assert_eq!(key.to_escape_sequence(), vec![0x1C]);
    }
}
