//! The toggle key and where each keystroke goes.
//!
//! Spec: `plans/weft/spec/interface.md` §Keys. Weft reserves exactly one
//! chord; in the agent everything else goes through, `Esc` included.

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
    /// Expand or collapse the selected row.
    Open,
    /// Back: collapse the row, close the drawer, dismiss a dialog.
    Back,
    /// Jump to the next thing that needs you.
    NextNeedsYou,
    /// Show or hide the work list.
    ToggleWork,
    /// The exact wording that was sent, in the drawer.
    Wording,
    /// The judges, their votes and their reasons, in the drawer.
    Judges,
    /// Ask again about the selected row's work.
    Fix,
    /// Why Weft says a pane is waiting for an answer.
    Explain,
    Ask,
    Check,
    Decide,
    Help,
    Quit,
    NextPane,
    /// Hand the keystroke to the harness untouched.
    ToAgent,
    /// Weft has no meaning for this key, and does nothing with it.
    Ignore,
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
        // `Esc` is never Weft's; it belongs to the agent.
        Key::Esc => Action::Ignore,
        Key::Char(' ') => Action::NextNeedsYou,
        Key::Char(c) if c.is_ascii_digit() && c != '0' => {
            Action::PickPane(c.to_digit(10).unwrap_or(1) as u8)
        }
        Key::Char(c) => match c.to_ascii_lowercase() {
            'a' => Action::Ask,
            's' => Action::Send,
            'n' => Action::NewAgent,
            'c' => Action::Check,
            'd' => Action::Decide,
            'p' => Action::Wording,
            'j' => Action::Judges,
            'f' => Action::Fix,
            'e' => Action::Explain,
            'w' => Action::ToggleWork,
            'h' => Action::Help,
            'x' => Action::Quit,
            _ => Action::Ignore,
        },
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
    fn every_action_the_bar_shows_a_key_for_has_that_key() {
        // §Keys: every action on screen shows its key in brackets, and the bar
        // is the reference. A bar entry with nothing behind it is the v1 bug.
        let t = Toggle;
        for (c, expected) in [
            ('a', Action::Ask),
            ('s', Action::Send),
            ('c', Action::Check),
            ('d', Action::Decide),
            ('n', Action::NewAgent),
            ('w', Action::ToggleWork),
            ('h', Action::Help),
            ('x', Action::Quit),
        ] {
            assert_eq!(route(plain(Key::Char(c)), Focus::Weft, t), expected, "{c}");
            assert_eq!(
                route(plain(Key::Char(c.to_ascii_uppercase())), Focus::Weft, t),
                expected,
                "{c} upper"
            );
        }
    }

    #[test]
    fn a_rows_own_actions_have_their_own_keys() {
        let t = Toggle;
        for (c, expected) in [
            ('p', Action::Wording),
            ('j', Action::Judges),
            ('f', Action::Fix),
            ('d', Action::Decide),
        ] {
            assert_eq!(route(plain(Key::Char(c)), Focus::Weft, t), expected, "{c}");
        }
    }

    #[test]
    fn space_jumps_to_what_needs_you_and_e_explains_an_inference() {
        assert_eq!(route(plain(Key::Char(' ')), Focus::Weft, Toggle), Action::NextNeedsYou);
        assert_eq!(route(plain(Key::Char('e')), Focus::Weft, Toggle), Action::Explain);
    }

    #[test]
    fn digits_switch_between_agents() {
        assert_eq!(route(plain(Key::Char('1')), Focus::Weft, Toggle), Action::PickPane(1));
        assert_eq!(route(plain(Key::Char('3')), Focus::Weft, Toggle), Action::PickPane(3));
        // and never in the agent, where a digit is just a digit
        assert_eq!(route(plain(Key::Char('1')), Focus::Agent, Toggle), Action::ToAgent);
    }

    #[test]
    fn in_weft_arrows_pick_enter_opens_and_left_goes_back() {
        let t = Toggle;
        assert_eq!(route(plain(Key::Up), Focus::Weft, t), Action::Pick(-1));
        assert_eq!(route(plain(Key::Down), Focus::Weft, t), Action::Pick(1));
        assert_eq!(route(plain(Key::Enter), Focus::Weft, t), Action::Open);
        assert_eq!(route(plain(Key::Left), Focus::Weft, t), Action::Back);
    }

    #[test]
    fn tab_cycles_agents() {
        assert_eq!(route(plain(Key::Tab), Focus::Weft, Toggle), Action::NextPane);
    }

    #[test]
    fn esc_is_never_wefts_even_in_weft() {
        // v1 opened help on Esc. `Esc` belongs to the agent, so in Weft it
        // does nothing at all rather than something surprising.
        assert_eq!(route(plain(Key::Esc), Focus::Weft, Toggle), Action::Ignore);
    }

    #[test]
    fn a_key_weft_has_no_meaning_for_does_nothing() {
        // v1 fell through to Help, which opened a dialog on a stray keystroke.
        for c in ['q', 'z', 'r', '/'] {
            assert_eq!(route(plain(Key::Char(c)), Focus::Weft, Toggle), Action::Ignore, "{c}");
        }
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
