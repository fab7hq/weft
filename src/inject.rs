//! Composing input for a harness pane.
//!
//! Spec: `plans/weft/spec/injection.md`. The rule that shapes this module is
//! that a send is not a receipt: nothing here reports that a prompt arrived.

use std::time::Duration;

/// Harness TUIs process paste and submission asynchronously, so Enter sent in
/// the same write races the harness's own input handling.
pub const ENTER_DELAY: Duration = Duration::from_millis(80);

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
const ENTER: &[u8] = b"\r";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Write(Vec<u8>),
    Wait(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The pane is waiting on a permission or approval dialog. Injecting would
    /// answer a dialog Weft must never answer.
    PaneBlocked,
    NoProcess,
    InjectionInFlight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneState {
    pub running: bool,
    pub blocked: bool,
    pub injecting: bool,
}

pub fn check(pane: &PaneState) -> Result<(), Refusal> {
    if !pane.running {
        return Err(Refusal::NoProcess);
    }
    if pane.blocked {
        return Err(Refusal::PaneBlocked);
    }
    if pane.injecting {
        return Err(Refusal::InjectionInFlight);
    }
    Ok(())
}

/// One ordered submission: the payload byte-exact, a wait, then Enter alone.
pub fn compose(payload: &[u8], bracketed_paste: bool) -> Vec<Step> {
    let body = if bracketed_paste {
        let mut b = Vec::with_capacity(PASTE_START.len() + payload.len() + PASTE_END.len());
        b.extend_from_slice(PASTE_START);
        b.extend_from_slice(payload);
        b.extend_from_slice(PASTE_END);
        b
    } else {
        payload.to_vec()
    };
    vec![Step::Write(body), Step::Wait(ENTER_DELAY), Step::Write(ENTER.to_vec())]
}

/// What Weft may do after an injection produced no observable result.
///
/// Herdr: "A timeout or `agent_prompt_stalled` does not prove that no input
/// was sent." So there is no retry to return.
pub fn after_stall() -> Option<Vec<Step>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> &'static [u8] {
        "/plan Return the real build number".as_bytes()
    }

    #[test]
    fn plain_write_is_payload_then_wait_then_enter() {
        assert_eq!(
            compose(payload(), false),
            vec![
                Step::Write(payload().to_vec()),
                Step::Wait(ENTER_DELAY),
                Step::Write(b"\r".to_vec()),
            ]
        );
    }

    #[test]
    fn bracketed_paste_wraps_the_payload_and_nothing_else() {
        let steps = compose(payload(), true);
        let Step::Write(body) = &steps[0] else { panic!("first step writes") };
        assert!(body.starts_with(PASTE_START));
        assert!(body.ends_with(PASTE_END));
        assert_eq!(&body[PASTE_START.len()..body.len() - PASTE_END.len()], payload());
    }

    #[test]
    fn enter_is_always_a_separate_write_after_a_wait() {
        for bracketed in [false, true] {
            let steps = compose(payload(), bracketed);
            assert_eq!(steps.len(), 3, "payload, wait, enter");
            assert_eq!(steps[1], Step::Wait(ENTER_DELAY));
            assert_eq!(steps[2], Step::Write(b"\r".to_vec()));
        }
    }

    #[test]
    fn payload_bytes_are_never_altered() {
        let awkward = "line one\nline two\ttabbed \u{1b}[31m and a CR\r".as_bytes();
        let Step::Write(body) = &compose(awkward, false)[0] else { panic!() };
        assert_eq!(body, awkward);
    }

    #[test]
    fn a_blocked_pane_is_refused() {
        let pane = PaneState { running: true, blocked: true, injecting: false };
        assert_eq!(check(&pane), Err(Refusal::PaneBlocked));
    }

    #[test]
    fn a_dead_pane_is_refused() {
        let pane = PaneState { running: false, blocked: false, injecting: false };
        assert_eq!(check(&pane), Err(Refusal::NoProcess));
    }

    #[test]
    fn a_second_injection_is_refused_while_one_is_in_flight() {
        let pane = PaneState { running: true, blocked: false, injecting: true };
        assert_eq!(check(&pane), Err(Refusal::InjectionInFlight));
    }

    #[test]
    fn a_live_idle_pane_is_allowed() {
        let pane = PaneState { running: true, blocked: false, injecting: false };
        assert_eq!(check(&pane), Ok(()));
    }

    #[test]
    fn a_stall_never_produces_a_retry() {
        assert_eq!(after_stall(), None);
    }
}
