//! Turning a keystroke back into the bytes a terminal would have sent.
//!
//! Used only while the person is focused on a pane, where Weft passes
//! everything through untouched.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn encode(key: KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let b = ctrl_byte(c)?;
            vec![b]
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => function_key(n)?,
        KeyCode::Null => vec![0x00],
        _ => return None,
    };

    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

fn function_key(n: u8) -> Option<Vec<u8>> {
    let seq: &[u8] = match n {
        1 => b"\x1bOP",
        2 => b"\x1bOQ",
        3 => b"\x1bOR",
        4 => b"\x1bOS",
        5 => b"\x1b[15~",
        6 => b"\x1b[17~",
        7 => b"\x1b[18~",
        8 => b"\x1b[19~",
        9 => b"\x1b[20~",
        10 => b"\x1b[21~",
        11 => b"\x1b[23~",
        12 => b"\x1b[24~",
        _ => return None,
    };
    Some(seq.to_vec())
}

/// Ctrl+letter is the letter with the top three bits cleared: Ctrl+A is 0x01.
fn ctrl_byte(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        'a'..='z' => Some((c.to_ascii_lowercase() as u8) - b'a' + 1),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '?' => Some(0x1f),
        // Legacy DEC spelling: Ctrl+4..7 are Ctrl+\ ] ^ _, which is how most
        // terminals report those bytes back to us.
        '4' => Some(0x1c),
        '5' => Some(0x1d),
        '6' => Some(0x1e),
        '7' => Some(0x1f),
        ' ' | '@' => Some(0x00),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn plain_characters_pass_through_as_themselves() {
        assert_eq!(encode(k(KeyCode::Char('a'))), Some(b"a".to_vec()));
        assert_eq!(encode(k(KeyCode::Char('/'))), Some(b"/".to_vec()));
    }

    #[test]
    fn unicode_survives_intact() {
        assert_eq!(encode(k(KeyCode::Char('é'))), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn ctrl_c_reaches_the_agent_as_an_interrupt() {
        assert_eq!(encode(ctrl('c')), Some(vec![0x03]));
    }

    #[test]
    fn ctrl_w_still_deletes_a_word_in_the_agent() {
        assert_eq!(encode(ctrl('w')), Some(vec![0x17]));
    }

    #[test]
    fn esc_reaches_the_agent_so_claude_code_can_interrupt() {
        assert_eq!(encode(k(KeyCode::Esc)), Some(vec![0x1b]));
    }

    #[test]
    fn arrows_are_the_usual_escape_sequences() {
        assert_eq!(encode(k(KeyCode::Up)), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(k(KeyCode::Left)), Some(b"\x1b[D".to_vec()));
    }

    #[test]
    fn alt_prefixes_with_escape() {
        let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(encode(alt_b), Some(b"\x1bb".to_vec()));
    }

    #[test]
    fn the_legacy_ctrl_number_spellings_round_trip() {
        assert_eq!(encode(ctrl('5')), Some(vec![0x1d]));
        assert_eq!(encode(ctrl(']')), Some(vec![0x1d]));
    }

    #[test]
    fn function_keys_reach_the_agent() {
        assert_eq!(encode(k(KeyCode::F(1))), Some(b"\x1bOP".to_vec()));
        assert_eq!(encode(k(KeyCode::F(12))), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn shift_tab_reaches_the_agent_so_modes_can_be_cycled() {
        assert_eq!(encode(k(KeyCode::BackTab)), Some(b"\x1b[Z".to_vec()));
    }

    #[test]
    fn enter_is_a_carriage_return() {
        assert_eq!(encode(k(KeyCode::Enter)), Some(b"\r".to_vec()));
    }
}
