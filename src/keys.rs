//! The toggle key and where each keystroke goes.
//!
//! Spec: `plans/weft/spec/interface.md`. Weft reserves exactly one key.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Weft,
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// `Ctrl+Shift+W`, available only where the terminal reports the Shift
    /// modifier on a Ctrl+letter chord.
    CtrlShiftW,
    /// `Ctrl+]`. Legacy-safe: one control byte, claimed by neither the
    /// harnesses nor the emulators.
    CtrlRightBracket,
}

impl Toggle {
    pub fn label(self) -> &'static str {
        match self {
            Toggle::CtrlShiftW => "Ctrl+Shift+W",
            Toggle::CtrlRightBracket => "Ctrl+]",
        }
    }
}

/// In the legacy encoding `Ctrl`+letter collapses to one control byte and
/// `Shift` is not encoded, so `Ctrl+Shift+W` is indistinguishable from
/// `Ctrl+W` unless the terminal speaks the Kitty keyboard protocol.
pub fn negotiate(kitty_keyboard: bool) -> Toggle {
    if kitty_keyboard { Toggle::CtrlShiftW } else { Toggle::CtrlRightBracket }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub code: Key,
    pub ctrl: bool,
    pub shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Up,
    Down,
    Left,
    Enter,
    Tab,
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ToggleFocus,
    Pick(i8),
    Open,
    Back,
    Ask,
    Check,
    Decide,
    Help,
    Quit,
    NextPane,
    /// Hand the keystroke to the harness untouched.
    ToAgent,
}

pub fn is_toggle(chord: Chord, toggle: Toggle) -> bool {
    match toggle {
        // The Kitty protocol may report the base key in either case.
        Toggle::CtrlShiftW => {
            chord.ctrl && chord.shift && matches!(chord.code, Key::Char('W' | 'w'))
        }
        // Terminals send Ctrl+] as 0x1D, which the legacy DEC convention names
        // Ctrl+5 — Ctrl+4..7 are Ctrl+\ ] ^ _. Both spellings reach us.
        Toggle::CtrlRightBracket => chord.ctrl && matches!(chord.code, Key::Char(']' | '5')),
    }
}

pub fn route(chord: Chord, focus: Focus, toggle: Toggle) -> Action {
    if is_toggle(chord, toggle) {
        return Action::ToggleFocus;
    }
    if focus == Focus::Agent {
        return Action::ToAgent;
    }
    match chord.code {
        Key::Up => Action::Pick(-1),
        Key::Down => Action::Pick(1),
        Key::Enter => Action::Open,
        Key::Left => Action::Back,
        Key::Tab => Action::NextPane,
        Key::Char(c) => match c.to_ascii_lowercase() {
            'a' => Action::Ask,
            'c' => Action::Check,
            'd' => Action::Decide,
            'h' => Action::Help,
            'x' => Action::Quit,
            _ => Action::Help,
        },
        Key::Esc => Action::Help,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(code: Key) -> Chord {
        Chord { code, ctrl: false, shift: false }
    }

    #[test]
    fn kitty_capable_terminals_get_the_shift_chord() {
        assert_eq!(negotiate(true), Toggle::CtrlShiftW);
    }

    #[test]
    fn everything_else_falls_back_to_a_legacy_safe_key() {
        assert_eq!(negotiate(false), Toggle::CtrlRightBracket);
    }

    #[test]
    fn plain_ctrl_w_is_not_the_toggle_so_the_agent_keeps_delete_word() {
        let ctrl_w = Chord { code: Key::Char('W'), ctrl: true, shift: false };
        assert!(!is_toggle(ctrl_w, Toggle::CtrlShiftW));
        assert_eq!(route(ctrl_w, Focus::Agent, Toggle::CtrlShiftW), Action::ToAgent);
    }

    #[test]
    fn the_toggle_works_in_both_directions() {
        for toggle in [Toggle::CtrlShiftW, Toggle::CtrlRightBracket] {
            let chord = match toggle {
                Toggle::CtrlShiftW => Chord { code: Key::Char('W'), ctrl: true, shift: true },
                Toggle::CtrlRightBracket => Chord { code: Key::Char(']'), ctrl: true, shift: false },
            };
            assert_eq!(route(chord, Focus::Weft, toggle), Action::ToggleFocus);
            assert_eq!(route(chord, Focus::Agent, toggle), Action::ToggleFocus);
        }
    }

    #[test]
    fn in_the_agent_every_key_goes_through_including_esc_and_ctrl_c() {
        let t = Toggle::CtrlRightBracket;
        let ctrl_c = Chord { code: Key::Char('c'), ctrl: true, shift: false };
        for chord in [plain(Key::Esc), plain(Key::Up), plain(Key::Enter), plain(Key::Char('a')), ctrl_c] {
            assert_eq!(route(chord, Focus::Agent, t), Action::ToAgent, "{chord:?}");
        }
    }

    #[test]
    fn in_weft_the_visible_buttons_have_single_keys() {
        let t = Toggle::CtrlRightBracket;
        for (c, expected) in [
            ('a', Action::Ask),
            ('c', Action::Check),
            ('d', Action::Decide),
            ('h', Action::Help),
            ('x', Action::Quit),
        ] {
            assert_eq!(route(plain(Key::Char(c)), Focus::Weft, t), expected);
            assert_eq!(route(plain(Key::Char(c.to_ascii_uppercase())), Focus::Weft, t), expected);
        }
    }

    #[test]
    fn in_weft_arrows_pick_and_enter_opens() {
        let t = Toggle::CtrlRightBracket;
        assert_eq!(route(plain(Key::Up), Focus::Weft, t), Action::Pick(-1));
        assert_eq!(route(plain(Key::Down), Focus::Weft, t), Action::Pick(1));
        assert_eq!(route(plain(Key::Enter), Focus::Weft, t), Action::Open);
        assert_eq!(route(plain(Key::Left), Focus::Weft, t), Action::Back);
    }

    #[test]
    fn ctrl_right_bracket_is_recognised_as_the_legacy_ctrl_5_spelling() {
        let ctrl_5 = Chord { code: Key::Char('5'), ctrl: true, shift: false };
        assert!(is_toggle(ctrl_5, Toggle::CtrlRightBracket));
        assert_eq!(route(ctrl_5, Focus::Agent, Toggle::CtrlRightBracket), Action::ToggleFocus);
    }

    #[test]
    fn the_shift_chord_is_recognised_in_either_case() {
        for c in ['W', 'w'] {
            let chord = Chord { code: Key::Char(c), ctrl: true, shift: true };
            assert!(is_toggle(chord, Toggle::CtrlShiftW), "{c}");
        }
    }

    #[test]
    fn in_the_agent_nothing_but_the_toggle_is_ever_intercepted() {
        // Unless the person is using Weft's own ask, check or decide, every
        // keystroke is chat with the harness and is forwarded untouched.
        for toggle in [Toggle::CtrlShiftW, Toggle::CtrlRightBracket] {
            for code in [
                Key::Up, Key::Down, Key::Left, Key::Enter, Key::Tab, Key::Esc,
            ] {
                for ctrl in [false, true] {
                    for shift in [false, true] {
                        let chord = Chord { code, ctrl, shift };
                        assert_eq!(
                            route(chord, Focus::Agent, toggle),
                            Action::ToAgent,
                            "{chord:?} must reach the agent"
                        );
                    }
                }
            }
            for c in 'a'..='z' {
                for ctrl in [false, true] {
                    let chord = Chord { code: Key::Char(c), ctrl, shift: false };
                    if is_toggle(chord, toggle) {
                        continue;
                    }
                    assert_eq!(
                        route(chord, Focus::Agent, toggle),
                        Action::ToAgent,
                        "{chord:?} must reach the agent"
                    );
                }
            }
        }
    }

    #[test]
    fn the_live_toggle_is_the_one_shown_to_the_person() {
        assert_eq!(negotiate(true).label(), "Ctrl+Shift+W");
        assert_eq!(negotiate(false).label(), "Ctrl+]");
    }
}
