//! The toggle key and where each keystroke goes.
//!
//! Spec: `plans/weft/spec/interface.md`. Weft reserves exactly one key.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Weft,
    Agent,
}

/// The one key Weft reserves.
///
/// `Ctrl+Shift+W` was tried and removed: outside terminals that speak the
/// Kitty keyboard protocol it is indistinguishable from `Ctrl+W`, and several
/// emulators bind it to close-tab and swallow it before any application sees
/// it. `Ctrl+]` is one control byte, arrives everywhere, and is claimed by
/// neither harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Toggle;

impl Toggle {
    pub fn label(self) -> &'static str {
        "Ctrl+]"
    }
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
    /// Type a confirmed prompt into the agent on the person's behalf.
    Send,
    /// Start another agent.
    NewAgent,
    /// Switch to the nth agent, counting from one.
    PickPane(u8),
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

/// Terminals send `Ctrl+]` as `0x1D`, which the legacy DEC convention names
/// `Ctrl+5` — Ctrl+4..7 are Ctrl+`\ ] ^ _`. Both spellings reach us.
pub fn is_toggle(chord: Chord, _toggle: Toggle) -> bool {
    chord.ctrl && matches!(chord.code, Key::Char(']' | '5'))
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
        Key::Char(c) if c.is_ascii_digit() && c != '0' => {
            Action::PickPane(c.to_digit(10).unwrap_or(1) as u8)
        }
        Key::Char(c) => match c.to_ascii_lowercase() {
            'a' => Action::Ask,
            's' => Action::Send,
            'n' => Action::NewAgent,
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
    fn in_the_agent_every_key_goes_through_including_esc_and_ctrl_c() {
        let t = Toggle;
        let ctrl_c = Chord { code: Key::Char('c'), ctrl: true, shift: false };
        for chord in [plain(Key::Esc), plain(Key::Up), plain(Key::Enter), plain(Key::Char('a')), ctrl_c] {
            assert_eq!(route(chord, Focus::Agent, t), Action::ToAgent, "{chord:?}");
        }
    }

    #[test]
    fn in_weft_the_visible_buttons_have_single_keys() {
        let t = Toggle;
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
    fn every_key_the_action_bar_offers_has_an_action() {
        // The bar advertised [S]end it long before anything was wired to it.
        let t = Toggle;
        for (c, expected) in [
            ('a', Action::Ask),
            ('s', Action::Send),
            ('n', Action::NewAgent),
            ('c', Action::Check),
            ('d', Action::Decide),
            ('h', Action::Help),
            ('x', Action::Quit),
        ] {
            assert_eq!(route(plain(Key::Char(c)), Focus::Weft, t), expected, "{c}");
        }
    }

    #[test]
    fn digits_switch_between_agents() {
        assert_eq!(route(plain(Key::Char('1')), Focus::Weft, Toggle), Action::PickPane(1));
        assert_eq!(route(plain(Key::Char('3')), Focus::Weft, Toggle), Action::PickPane(3));
        // and never in the agent, where a digit is just a digit
        assert_eq!(route(plain(Key::Char('1')), Focus::Agent, Toggle), Action::ToAgent);
    }

    #[test]
    fn in_weft_arrows_pick_and_enter_opens() {
        let t = Toggle;
        assert_eq!(route(plain(Key::Up), Focus::Weft, t), Action::Pick(-1));
        assert_eq!(route(plain(Key::Down), Focus::Weft, t), Action::Pick(1));
        assert_eq!(route(plain(Key::Enter), Focus::Weft, t), Action::Open);
        assert_eq!(route(plain(Key::Left), Focus::Weft, t), Action::Back);
    }

    #[test]
    fn ctrl_right_bracket_is_recognised_in_both_spellings() {
        for c in [']', '5'] {
            let chord = Chord { code: Key::Char(c), ctrl: true, shift: false };
            assert!(is_toggle(chord, Toggle), "{c}");
            assert_eq!(route(chord, Focus::Agent, Toggle), Action::ToggleFocus);
            assert_eq!(route(chord, Focus::Weft, Toggle), Action::ToggleFocus);
        }
    }

    #[test]
    fn ctrl_w_is_left_to_the_agent_to_delete_a_word() {
        let ctrl_w = Chord { code: Key::Char('w'), ctrl: true, shift: false };
        assert!(!is_toggle(ctrl_w, Toggle));
        assert_eq!(route(ctrl_w, Focus::Agent, Toggle), Action::ToAgent);
    }

    #[test]
    fn the_toggle_is_named_the_same_everywhere() {
        assert_eq!(Toggle.label(), "Ctrl+]");
    }


    #[test]
    fn in_the_agent_nothing_but_the_toggle_is_ever_intercepted() {
        // Unless the person is using Weft's own ask, check or decide, every
        // keystroke is chat with the harness and is forwarded untouched.
        for toggle in [Toggle] {
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

}
