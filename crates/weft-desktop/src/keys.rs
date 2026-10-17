//! The window's own shortcuts, and how each is shown beside what it does,
//! per system: ⌘ on the Mac; Ctrl with Shift on Linux, where Ctrl alone is
//! the agent's in its terminal. A text field's keys are `weft_core::text_keys`.

use weft_core::text_keys::System;

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Quit,
    OpenProject,
    ToggleList,
    /// The terminal's selection to the clipboard.
    Copy,
    /// The clipboard into the agent in front.
    Paste,
}

/// Each shortcut, as `modifiers-key` in the text key table's order: the
/// Mac's, and Linux's.
pub const SHORTCUTS: &[(Shortcut, &str, &str)] = &[
    (Shortcut::Quit, "cmd-q", "ctrl-shift-q"),
    (Shortcut::OpenProject, "cmd-o", "ctrl-shift-o"),
    (Shortcut::ToggleList, "cmd-b", "ctrl-shift-b"),
    (Shortcut::Copy, "cmd-c", "ctrl-shift-c"),
    (Shortcut::Paste, "cmd-v", "ctrl-shift-v"),
];

fn chord_of(
    system: System,
    (_, mac, linux): &(Shortcut, &'static str, &'static str),
) -> &'static str {
    if system == System::Mac { mac } else { linux }
}

/// The shortcut `chord` is on `system`, if it is one.
pub fn shortcut(system: System, chord: &str) -> Option<Shortcut> {
    SHORTCUTS.iter().find(|s| chord_of(system, s) == chord).map(|s| s.0)
}

/// How `shortcut` is shown beside what it does on `system`: `⌘O`, `Ctrl+Shift+O`.
pub fn hint(system: System, shortcut: Shortcut) -> String {
    let Some(s) = SHORTCUTS.iter().find(|s| s.0 == shortcut) else { return String::new() };
    let chord = chord_of(system, s);
    let (mods, key) = chord.rsplit_once('-').unwrap_or(("", chord));
    let names = mods.split('-').filter(|m| !m.is_empty()).map(|m| match (system, m) {
        (System::Mac, "cmd") => "⌘",
        (System::Mac, "shift") => "⇧",
        (System::Mac, "alt") => "⌥",
        (System::Mac, "ctrl") => "⌃",
        (_, "ctrl") => "Ctrl",
        (_, "shift") => "Shift",
        (_, "alt") => "Alt",
        _ => "Super",
    });
    let key = key.to_uppercase();
    match system {
        System::Mac => names.chain([key.as_str()]).collect(),
        System::Linux => names.chain([key.as_str()]).collect::<Vec<_>>().join("+"),
    }
}

/// How `shortcut` is shown on this system.
pub fn hint_here(shortcut: Shortcut) -> String {
    hint(System::this(), shortcut)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_takes_command_and_linux_ctrl_with_shift() {
        for (chord, s) in [
            ("cmd-q", Shortcut::Quit),
            ("cmd-o", Shortcut::OpenProject),
            ("cmd-b", Shortcut::ToggleList),
            ("cmd-c", Shortcut::Copy),
            ("cmd-v", Shortcut::Paste),
        ] {
            assert_eq!(shortcut(System::Mac, chord), Some(s), "{chord}");
        }
        for (chord, s) in [
            ("ctrl-shift-q", Shortcut::Quit),
            ("ctrl-shift-o", Shortcut::OpenProject),
            ("ctrl-shift-b", Shortcut::ToggleList),
            ("ctrl-shift-c", Shortcut::Copy),
            ("ctrl-shift-v", Shortcut::Paste),
        ] {
            assert_eq!(shortcut(System::Linux, chord), Some(s), "{chord}");
        }
        // Ctrl alone is the agent's: Ctrl+C interrupts it, Ctrl+B is tmux's.
        for agents in ["ctrl-c", "ctrl-b", "ctrl-o", "cmd-q"] {
            assert_eq!(shortcut(System::Linux, agents), None, "{agents}");
        }
        assert_eq!(shortcut(System::Mac, "ctrl-c"), None);
    }

    #[test]
    fn hints_read_as_each_system_writes_them() {
        assert_eq!(hint(System::Mac, Shortcut::OpenProject), "⌘O");
        assert_eq!(hint(System::Mac, Shortcut::Quit), "⌘Q");
        assert_eq!(hint(System::Linux, Shortcut::OpenProject), "Ctrl+Shift+O");
        assert_eq!(hint(System::Linux, Shortcut::Quit), "Ctrl+Shift+Q");
    }

    #[test]
    fn no_shortcut_takes_a_text_fields_key() {
        for system in [System::Mac, System::Linux] {
            for (s, ..) in
                SHORTCUTS.iter().filter(|s| !matches!(s.0, Shortcut::Copy | Shortcut::Paste))
            {
                let chord = chord_of(system, SHORTCUTS.iter().find(|x| x.0 == *s).unwrap());
                assert_eq!(weft_core::text_keys::edit(system, chord), None, "{system:?} {chord}");
            }
        }
    }
}
