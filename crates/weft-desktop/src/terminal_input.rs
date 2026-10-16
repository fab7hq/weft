//! What a key, a paste or the mouse sends to a pane's program, by the modes
//! that program asked its terminal for.

use weft_term::{Modes, Mouse, MouseEncoding};

/// The modifier keys held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub platform: bool,
}

/// The bytes a key sends: `key` as the window names it, `typed` the text it
/// makes, if any. Keys with ⌘ belong to the window and send nothing.
pub fn key(key: &str, typed: Option<&str>, held: Held, modes: Modes) -> Option<Vec<u8>> {
    if held.platform {
        return None;
    }
    let modifier = 1 + u8::from(held.shift) + 2 * u8::from(held.alt) + 4 * u8::from(held.control);
    // A cursor key: application mode sends SS3, a modifier always CSI.
    let cursor = |end: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{end}").into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{end}").into_bytes()
        } else {
            format!("\x1b[{end}").into_bytes()
        }
    };
    let bytes = match key {
        "enter" => b"\r".to_vec(),
        "backspace" => vec![0x7f],
        "tab" if held.shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => b"\x1b".to_vec(),
        "space" => b" ".to_vec(),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "delete" => b"\x1b[3~".to_vec(),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        _ => return text_key(key, typed, held),
    };
    Some(bytes)
}

fn text_key(key: &str, typed: Option<&str>, held: Held) -> Option<Vec<u8>> {
    if held.control && key.len() == 1 {
        let ch = key.as_bytes()[0].to_ascii_lowercase();
        if ch.is_ascii_lowercase() {
            return Some(vec![ch - b'a' + 1]);
        }
        if ch == b']' {
            return Some(vec![0x1d]);
        }
    }
    let text = typed
        .filter(|t| !t.is_empty())
        .or((key.chars().count() == 1 && !held.control).then_some(key))?;
    let mut bytes = Vec::new();
    if held.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

/// A paste: whole, and inside the paste markers when the program asked
/// for them, so several lines never arrive as several Enters.
pub fn paste(text: &str, bracketed_paste: bool) -> Vec<u8> {
    weft_core::inject::paste(text.as_bytes(), bracketed_paste)
}

/// A mouse button, or the wheel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// What a pointer did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Press,
    Release,
    /// Moved with the button held.
    Drag,
}

/// The report a mouse act makes at a cell `(row, col)` from 0, when the
/// program asked for that kind of act; `None` when it did not.
pub fn mouse(button: Button, act: Act, (row, col): (u16, u16), modes: Modes) -> Option<Vec<u8>> {
    let wanted = match (modes.mouse, act) {
        (Mouse::None, _) => false,
        (_, Act::Press) => true,
        (_, Act::Release) => !matches!(button, Button::WheelUp | Button::WheelDown),
        (Mouse::Drag | Mouse::Motion, Act::Drag) => true,
        (Mouse::Click, Act::Drag) => false,
    };
    if !wanted {
        return None;
    }
    let base = match button {
        Button::Left => 0,
        Button::Middle => 1,
        Button::Right => 2,
        Button::WheelUp => 64,
        Button::WheelDown => 65,
    } + if act == Act::Drag { 32 } else { 0 };
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    Some(match modes.mouse_encoding {
        MouseEncoding::Sgr => {
            let end = if act == Act::Release { 'm' } else { 'M' };
            format!("\x1b[<{base};{x};{y}{end}").into_bytes()
        }
        MouseEncoding::Default | MouseEncoding::Utf8 => {
            // The old form has no button for a release, and stops at 223.
            let code = if act == Act::Release { 3 } else { base };
            let at = |v: u32| (32 + v.min(223)) as u8;
            vec![0x1b, b'[', b'M', 32 + code as u8, at(x), at(y)]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> Modes {
        Modes::default()
    }

    #[test]
    fn arrows_follow_application_cursor_mode() {
        assert_eq!(key("up", None, Held::default(), modes()), Some(b"\x1b[A".to_vec()));
        let app = Modes { app_cursor: true, ..modes() };
        assert_eq!(key("up", None, Held::default(), app), Some(b"\x1bOA".to_vec()));
        assert_eq!(key("home", None, Held::default(), app), Some(b"\x1bOH".to_vec()));
        let shift = Held { shift: true, ..Held::default() };
        assert_eq!(key("left", None, shift, app), Some(b"\x1b[1;2D".to_vec()), "a modifier is CSI");
    }

    #[test]
    fn letters_controls_and_the_window_s_own_keys() {
        assert_eq!(key("a", Some("a"), Held::default(), modes()), Some(b"a".to_vec()));
        let ctrl = Held { control: true, ..Held::default() };
        assert_eq!(key("c", None, ctrl, modes()), Some(vec![3]));
        let alt = Held { alt: true, ..Held::default() };
        assert_eq!(key("b", Some("b"), alt, modes()), Some(b"\x1bb".to_vec()));
        let cmd = Held { platform: true, ..Held::default() };
        assert_eq!(key("v", Some("v"), cmd, modes()), None, "⌘V is the window's");
        assert_eq!(key("enter", None, Held::default(), modes()), Some(b"\r".to_vec()));
    }

    #[test]
    fn a_paste_is_bracketed_only_when_asked() {
        assert_eq!(paste("one\ntwo", true), b"\x1b[200~one\ntwo\x1b[201~".to_vec());
        assert_eq!(paste("one\ntwo", false), b"one\ntwo".to_vec());
    }

    #[test]
    fn the_mouse_reports_only_what_the_program_asked_for() {
        let at = (4, 9);
        assert_eq!(mouse(Button::Left, Act::Press, at, modes()), None, "no reporting asked");
        let sgr = Modes { mouse: Mouse::Click, mouse_encoding: MouseEncoding::Sgr, ..modes() };
        assert_eq!(mouse(Button::Left, Act::Press, at, sgr), Some(b"\x1b[<0;10;5M".to_vec()));
        assert_eq!(mouse(Button::Left, Act::Release, at, sgr), Some(b"\x1b[<0;10;5m".to_vec()));
        assert_eq!(mouse(Button::Left, Act::Drag, at, sgr), None, "clicks only");
        assert_eq!(mouse(Button::WheelUp, Act::Press, at, sgr), Some(b"\x1b[<64;10;5M".to_vec()));
        let drag = Modes { mouse: Mouse::Drag, ..sgr };
        assert_eq!(mouse(Button::Left, Act::Drag, at, drag), Some(b"\x1b[<32;10;5M".to_vec()));
        let old = Modes { mouse: Mouse::Click, ..modes() };
        assert_eq!(
            mouse(Button::Left, Act::Press, at, old),
            Some(vec![0x1b, b'[', b'M', 32, 42, 37])
        );
        assert_eq!(
            mouse(Button::Left, Act::Release, at, old),
            Some(vec![0x1b, b'[', b'M', 35, 42, 37])
        );
    }
}
