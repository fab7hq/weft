//! What a key does in a text field, on the desktop and on the web.
//!
//! One table. The desktop's text field reads it, and `text_keys.json`, written
//! from it, is what the web app's message and search fields are checked
//! against. A key that is not here types its character, or does nothing.

/// What a key does to a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// Send the message; in a one-line field, do what the field is for.
    Send,
    /// A new line in a message; nothing in a one-line field.
    NewLine,
    /// Take the suggestion shown after the caret.
    Take,
    /// Drop the suggestion.
    Drop,
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    Start,
    End,
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectWordLeft,
    SelectWordRight,
    SelectLineStart,
    SelectLineEnd,
    SelectStart,
    SelectEnd,
    SelectAll,
    DeleteBack,
    DeleteForward,
    DeleteWordBack,
    DeleteWordForward,
    DeleteLineBack,
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
}

impl Edit {
    /// Its name in `text_keys.json`.
    pub fn name(self) -> &'static str {
        match self {
            Edit::Send => "send",
            Edit::NewLine => "new_line",
            Edit::Take => "take",
            Edit::Drop => "drop",
            Edit::Left => "left",
            Edit::Right => "right",
            Edit::Up => "up",
            Edit::Down => "down",
            Edit::WordLeft => "word_left",
            Edit::WordRight => "word_right",
            Edit::LineStart => "line_start",
            Edit::LineEnd => "line_end",
            Edit::Start => "start",
            Edit::End => "end",
            Edit::SelectLeft => "select_left",
            Edit::SelectRight => "select_right",
            Edit::SelectUp => "select_up",
            Edit::SelectDown => "select_down",
            Edit::SelectWordLeft => "select_word_left",
            Edit::SelectWordRight => "select_word_right",
            Edit::SelectLineStart => "select_line_start",
            Edit::SelectLineEnd => "select_line_end",
            Edit::SelectStart => "select_start",
            Edit::SelectEnd => "select_end",
            Edit::SelectAll => "select_all",
            Edit::DeleteBack => "delete_back",
            Edit::DeleteForward => "delete_forward",
            Edit::DeleteWordBack => "delete_word_back",
            Edit::DeleteWordForward => "delete_word_forward",
            Edit::DeleteLineBack => "delete_line_back",
            Edit::Copy => "copy",
            Edit::Cut => "cut",
            Edit::Paste => "paste",
            Edit::Undo => "undo",
            Edit::Redo => "redo",
        }
    }
}

/// Every key, as `modifiers-key` with the modifiers in the order
/// `ctrl-alt-shift-cmd`, and what it does.
pub const TEXT_KEYS: &[(&str, Edit)] = &[
    ("enter", Edit::Send),
    ("shift-enter", Edit::NewLine),
    ("tab", Edit::Take),
    ("escape", Edit::Drop),
    ("left", Edit::Left),
    ("right", Edit::Right),
    ("up", Edit::Up),
    ("down", Edit::Down),
    ("alt-left", Edit::WordLeft),
    ("alt-right", Edit::WordRight),
    ("cmd-left", Edit::LineStart),
    ("cmd-right", Edit::LineEnd),
    ("home", Edit::LineStart),
    ("end", Edit::LineEnd),
    ("cmd-up", Edit::Start),
    ("cmd-down", Edit::End),
    ("shift-left", Edit::SelectLeft),
    ("shift-right", Edit::SelectRight),
    ("shift-up", Edit::SelectUp),
    ("shift-down", Edit::SelectDown),
    ("alt-shift-left", Edit::SelectWordLeft),
    ("alt-shift-right", Edit::SelectWordRight),
    ("shift-cmd-left", Edit::SelectLineStart),
    ("shift-cmd-right", Edit::SelectLineEnd),
    ("shift-home", Edit::SelectLineStart),
    ("shift-end", Edit::SelectLineEnd),
    ("shift-cmd-up", Edit::SelectStart),
    ("shift-cmd-down", Edit::SelectEnd),
    ("cmd-a", Edit::SelectAll),
    ("backspace", Edit::DeleteBack),
    ("shift-backspace", Edit::DeleteBack),
    ("delete", Edit::DeleteForward),
    ("alt-backspace", Edit::DeleteWordBack),
    ("alt-delete", Edit::DeleteWordForward),
    ("cmd-backspace", Edit::DeleteLineBack),
    ("cmd-c", Edit::Copy),
    ("cmd-x", Edit::Cut),
    ("cmd-v", Edit::Paste),
    ("cmd-z", Edit::Undo),
    ("shift-cmd-z", Edit::Redo),
];

/// What `chord` does, if anything.
pub fn edit(chord: &str) -> Option<Edit> {
    TEXT_KEYS.iter().find(|(c, _)| *c == chord).map(|(_, e)| *e)
}

/// `text_keys.json`: each key and the name of what it does, sorted.
pub fn json() -> String {
    let map: std::collections::BTreeMap<&str, &str> =
        TEXT_KEYS.iter().map(|(c, e)| (*c, e.name())).collect();
    serde_json::to_string_pretty(&map).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sends_shift_enter_adds_a_line_tab_takes_and_escape_drops() {
        assert_eq!(edit("enter"), Some(Edit::Send));
        assert_eq!(edit("shift-enter"), Some(Edit::NewLine));
        assert_eq!(edit("tab"), Some(Edit::Take));
        assert_eq!(edit("escape"), Some(Edit::Drop));
    }

    #[test]
    fn the_mac_moves_selections_clipboard_and_undo_are_there() {
        for (chord, e) in [
            ("alt-left", Edit::WordLeft),
            ("alt-right", Edit::WordRight),
            ("cmd-left", Edit::LineStart),
            ("cmd-right", Edit::LineEnd),
            ("home", Edit::LineStart),
            ("end", Edit::LineEnd),
            ("cmd-up", Edit::Start),
            ("cmd-down", Edit::End),
            ("shift-left", Edit::SelectLeft),
            ("alt-shift-left", Edit::SelectWordLeft),
            ("shift-cmd-left", Edit::SelectLineStart),
            ("shift-cmd-down", Edit::SelectEnd),
            ("cmd-a", Edit::SelectAll),
            ("backspace", Edit::DeleteBack),
            ("alt-backspace", Edit::DeleteWordBack),
            ("cmd-backspace", Edit::DeleteLineBack),
            ("delete", Edit::DeleteForward),
            ("cmd-c", Edit::Copy),
            ("cmd-x", Edit::Cut),
            ("cmd-v", Edit::Paste),
            ("cmd-z", Edit::Undo),
            ("shift-cmd-z", Edit::Redo),
        ] {
            assert_eq!(edit(chord), Some(e), "{chord}");
        }
        assert_eq!(edit("a"), None, "a character types itself");
    }

    #[test]
    fn each_key_is_held_once_with_its_modifiers_in_order() {
        let order = ["ctrl", "alt", "shift", "cmd"];
        let mut seen = std::collections::BTreeSet::new();
        for (chord, _) in TEXT_KEYS {
            assert!(seen.insert(*chord), "{chord} twice");
            let parts: Vec<&str> = chord.split('-').collect();
            let mods: Vec<usize> = parts[..parts.len() - 1]
                .iter()
                .map(|m| {
                    order.iter().position(|o| o == m).unwrap_or_else(|| panic!("{chord}: {m}"))
                })
                .collect();
            assert!(mods.windows(2).all(|w| w[0] < w[1]), "{chord}");
        }
    }

    #[test]
    fn the_json_names_every_key() {
        let v: serde_json::Value = serde_json::from_str(&json()).unwrap();
        assert_eq!(v.as_object().unwrap().len(), TEXT_KEYS.len());
        assert_eq!(v["shift-enter"], "new_line");
    }
}
