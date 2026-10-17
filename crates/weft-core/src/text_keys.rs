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

/// The system whose keys apply: the Mac's ⌘ and ⌥, or Ctrl elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    Mac,
    Linux,
}

impl System {
    /// The system this was built for.
    pub fn this() -> System {
        if cfg!(target_os = "macos") { System::Mac } else { System::Linux }
    }

    /// Its name in `text_keys.json`.
    pub fn name(self) -> &'static str {
        match self {
            System::Mac => "macos",
            System::Linux => "linux",
        }
    }
}

/// Every key, as `modifiers-key` with the modifiers in the order
/// `ctrl-alt-shift-cmd`: what it does, the Mac's chord, and Linux's. An
/// empty chord: that system has no key of its own for it.
pub const TEXT_KEYS: &[(Edit, &str, &str)] = &[
    (Edit::Send, "enter", "enter"),
    (Edit::NewLine, "shift-enter", "shift-enter"),
    (Edit::Take, "tab", "tab"),
    (Edit::Drop, "escape", "escape"),
    (Edit::Left, "left", "left"),
    (Edit::Right, "right", "right"),
    (Edit::Up, "up", "up"),
    (Edit::Down, "down", "down"),
    (Edit::WordLeft, "alt-left", "ctrl-left"),
    (Edit::WordRight, "alt-right", "ctrl-right"),
    (Edit::LineStart, "cmd-left", ""),
    (Edit::LineEnd, "cmd-right", ""),
    (Edit::LineStart, "home", "home"),
    (Edit::LineEnd, "end", "end"),
    (Edit::Start, "cmd-up", "ctrl-home"),
    (Edit::End, "cmd-down", "ctrl-end"),
    (Edit::SelectLeft, "shift-left", "shift-left"),
    (Edit::SelectRight, "shift-right", "shift-right"),
    (Edit::SelectUp, "shift-up", "shift-up"),
    (Edit::SelectDown, "shift-down", "shift-down"),
    (Edit::SelectWordLeft, "alt-shift-left", "ctrl-shift-left"),
    (Edit::SelectWordRight, "alt-shift-right", "ctrl-shift-right"),
    (Edit::SelectLineStart, "shift-cmd-left", ""),
    (Edit::SelectLineEnd, "shift-cmd-right", ""),
    (Edit::SelectLineStart, "shift-home", "shift-home"),
    (Edit::SelectLineEnd, "shift-end", "shift-end"),
    (Edit::SelectStart, "shift-cmd-up", "ctrl-shift-home"),
    (Edit::SelectEnd, "shift-cmd-down", "ctrl-shift-end"),
    (Edit::SelectAll, "cmd-a", "ctrl-a"),
    (Edit::DeleteBack, "backspace", "backspace"),
    (Edit::DeleteBack, "shift-backspace", "shift-backspace"),
    (Edit::DeleteForward, "delete", "delete"),
    (Edit::DeleteWordBack, "alt-backspace", "ctrl-backspace"),
    (Edit::DeleteWordForward, "alt-delete", "ctrl-delete"),
    (Edit::DeleteLineBack, "cmd-backspace", "ctrl-shift-backspace"),
    (Edit::Copy, "cmd-c", "ctrl-c"),
    (Edit::Cut, "cmd-x", "ctrl-x"),
    (Edit::Paste, "cmd-v", "ctrl-v"),
    (Edit::Undo, "cmd-z", "ctrl-z"),
    (Edit::Redo, "shift-cmd-z", "ctrl-shift-z"),
];

/// Each of `system`'s keys and what it does.
pub fn keys(system: System) -> impl Iterator<Item = (&'static str, Edit)> {
    TEXT_KEYS
        .iter()
        .map(move |(e, mac, linux)| (if system == System::Mac { *mac } else { *linux }, *e))
        .filter(|(chord, _)| !chord.is_empty())
}

/// What `chord` does on `system`, if anything.
pub fn edit(system: System, chord: &str) -> Option<Edit> {
    keys(system).find(|(c, _)| *c == chord).map(|(_, e)| e)
}

/// `text_keys.json`: for each system, each key and the name of what it does, sorted.
pub fn json() -> String {
    let map: std::collections::BTreeMap<&str, std::collections::BTreeMap<&str, &str>> =
        [System::Mac, System::Linux]
            .into_iter()
            .map(|s| (s.name(), keys(s).map(|(c, e)| (c, e.name())).collect()))
            .collect();
    serde_json::to_string_pretty(&map).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_sends_shift_enter_adds_a_line_tab_takes_and_escape_drops_on_both() {
        for system in [System::Mac, System::Linux] {
            assert_eq!(edit(system, "enter"), Some(Edit::Send));
            assert_eq!(edit(system, "shift-enter"), Some(Edit::NewLine));
            assert_eq!(edit(system, "tab"), Some(Edit::Take));
            assert_eq!(edit(system, "escape"), Some(Edit::Drop));
            assert_eq!(edit(system, "a"), None, "a character types itself");
        }
    }

    #[test]
    fn the_mac_moves_selects_copies_and_undoes_with_its_own_keys() {
        for (chord, e) in [
            ("alt-left", Edit::WordLeft),
            ("cmd-left", Edit::LineStart),
            ("home", Edit::LineStart),
            ("cmd-up", Edit::Start),
            ("alt-shift-left", Edit::SelectWordLeft),
            ("shift-cmd-down", Edit::SelectEnd),
            ("cmd-a", Edit::SelectAll),
            ("alt-backspace", Edit::DeleteWordBack),
            ("cmd-backspace", Edit::DeleteLineBack),
            ("cmd-c", Edit::Copy),
            ("cmd-x", Edit::Cut),
            ("cmd-v", Edit::Paste),
            ("cmd-z", Edit::Undo),
            ("shift-cmd-z", Edit::Redo),
        ] {
            assert_eq!(edit(System::Mac, chord), Some(e), "{chord}");
        }
        assert_eq!(edit(System::Mac, "ctrl-c"), None, "Ctrl is not the Mac's");
    }

    #[test]
    fn linux_does_the_same_with_ctrl() {
        for (chord, e) in [
            ("ctrl-left", Edit::WordLeft),
            ("ctrl-right", Edit::WordRight),
            ("home", Edit::LineStart),
            ("end", Edit::LineEnd),
            ("ctrl-home", Edit::Start),
            ("ctrl-end", Edit::End),
            ("ctrl-shift-left", Edit::SelectWordLeft),
            ("ctrl-shift-end", Edit::SelectEnd),
            ("ctrl-a", Edit::SelectAll),
            ("ctrl-backspace", Edit::DeleteWordBack),
            ("ctrl-delete", Edit::DeleteWordForward),
            ("ctrl-c", Edit::Copy),
            ("ctrl-x", Edit::Cut),
            ("ctrl-v", Edit::Paste),
            ("ctrl-z", Edit::Undo),
            ("ctrl-shift-z", Edit::Redo),
        ] {
            assert_eq!(edit(System::Linux, chord), Some(e), "{chord}");
        }
        for mac_only in ["cmd-c", "alt-left", "cmd-left"] {
            assert_eq!(edit(System::Linux, mac_only), None, "{mac_only}");
        }
    }

    #[test]
    fn every_edit_has_a_key_on_each_system() {
        for system in [System::Mac, System::Linux] {
            for (e, ..) in TEXT_KEYS {
                assert!(keys(system).any(|(_, k)| k == *e), "{system:?} {e:?}");
            }
        }
    }

    #[test]
    fn each_key_is_held_once_with_its_modifiers_in_order() {
        let order = ["ctrl", "alt", "shift", "cmd"];
        for system in [System::Mac, System::Linux] {
            let mut seen = std::collections::BTreeSet::new();
            for (chord, _) in keys(system) {
                assert!(seen.insert(chord), "{system:?}: {chord} twice");
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
    }

    #[test]
    fn the_json_names_every_key_of_each_system() {
        let v: serde_json::Value = serde_json::from_str(&json()).unwrap();
        for system in [System::Mac, System::Linux] {
            assert_eq!(v[system.name()].as_object().unwrap().len(), keys(system).count());
        }
        assert_eq!(v["linux"]["ctrl-shift-z"], "redo");
        assert_eq!(v["macos"]["shift-enter"], "new_line");
    }
}
