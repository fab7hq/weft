//! Composing input for a harness pane.
//!
//! Spec: `plans/weft/spec/injection.md`. The rule that shapes this module is
//! that a send is not a receipt: nothing here reports that a prompt arrived.

use std::time::Duration;

/// Harness TUIs process paste and submission asynchronously, so Enter sent in
/// the same write races the harness's own input handling.
pub const ENTER_DELAY: Duration = Duration::from_millis(80);

/// How long to wait for a pane to show what was written before giving up and
/// sending Enter anyway.
pub const ECHO_TIMEOUT: Duration = Duration::from_millis(2500);

/// How long the composer must hold still after the text appears, before Enter
/// is worth sending. Seeing the text is not the same as the harness having
/// finished taking it: a short payload renders almost at once, so Enter used
/// to arrive while the composer was still ingesting the paste, and the prompt
/// sat there unsent. A long payload took long enough to draw that it hid this.
pub const COMPOSER_SETTLE: Duration = Duration::from_millis(400);

/// How long to leave a composer alone before pressing Enter on it.
///
/// Codex suppresses Enter for 120ms after a burst of input, treating it as a
/// newline instead of a submission, so a command and its Enter written together
/// never submit at all. Longer than that window, and long enough for Claude
/// Code's composer too.
pub const SUBMIT_SETTLE: Duration = Duration::from_millis(300);

/// How long to wait for a host to show that a mode is on. Entering one is
/// local to the TUI and has no model call behind it, so this is generous
/// rather than long.
pub const MODE_TIMEOUT: Duration = Duration::from_millis(4000);

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
    /// The host never showed the mode as on, so the prompt was not sent. It
    /// would have run as an ordinary request, which is not what was confirmed.
    ModeNotEntered,
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

/// How a prompt that carries a command has to be put into a composer.
///
/// RingFrame records which one applies ([`crate::ledger::Delivery`]); this is
/// only the shape of the typing. All three deliver the same bytes, and none of
/// them reports that anything arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// Paste the whole thing. What every prompt used to get, and still right
    /// when the host will read the paste as text.
    Whole,
    /// Type the command, paste the body beside it, submit once. Typed, the
    /// command is read; pasted, it is not. Measured working on both hosts.
    TypeCommand { command: String, at: usize },
    /// Submit the command alone, wait for the host to show the mode is on,
    /// then send the body. Only for a command that is a mode and so leaves
    /// something to see — Weft checks rather than assumes.
    EnterMode { command: String, at: usize, active: String },
}

impl Handoff {
    /// The one place the choice is made, from what the record says.
    ///
    /// A prompt the host will not fold needs nothing special: the command is
    /// read straight out of the paste. Past the fold it does, and then a mode
    /// is entered first — because that can be confirmed — while an inline
    /// command is typed in front of the body, which cannot be.
    pub fn choose(d: &crate::ledger::Delivery) -> Self {
        let Some(command) = d.mode.clone() else { return Handoff::Whole };
        if !d.folds {
            return Handoff::Whole;
        }
        match (d.is_mode(), d.active.clone()) {
            (true, Some(active)) => Handoff::EnterMode { command, at: d.prefix_bytes, active },
            _ => Handoff::TypeCommand { command, at: d.prefix_bytes },
        }
    }

    /// The part of the prompt that is not the command.
    pub fn body<'a>(&self, prompt: &'a [u8]) -> &'a [u8] {
        match self {
            Handoff::Whole => prompt,
            Handoff::TypeCommand { at, .. } | Handoff::EnterMode { at, .. } => {
                prompt.get(*at..).unwrap_or(prompt)
            }
        }
    }
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

/// Collapse whitespace so a wrapped composer still matches.
pub fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A short distinctive tail of the payload to look for on screen. Short
/// enough to survive wrapping and truncation, long enough not to match by
/// accident.
pub fn echo_tail(payload: &[u8]) -> Option<String> {
    let text = squeeze(&String::from_utf8_lossy(payload));
    if text.chars().count() < 4 {
        return None;
    }
    let tail: String = text.chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
    Some(tail)
}

/// A harness may collapse a multi-line paste rather than show it: Claude Code
/// draws `[Pasted text #1 +13 lines]` and the prompt itself never appears. The
/// composer is saying it took the paste, and that is as much as a screen can
/// say about a prompt it has folded away.
/// How each host writes a paste it folded away, measured against the installed
/// versions: Codex 0.155.1 says `[Pasted Content 1206 chars]`, Claude Code
/// 2.1.278 says `[Pasted text #1]`. The earlier reader wanted `[Pasted text`
/// *and* `lines]`, which by then matched neither, so every long prompt failed
/// echo verification and sat in the composer unsent.
const FOLDED: &[&str] = &["[Pasted Content", "[Pasted text"];

pub fn collapsed_paste(screen: &str) -> bool {
    FOLDED.iter().any(|marker| screen.contains(marker))
}

/// The same, but checked against how much was written, where the host says.
///
/// Codex names the size — `[Pasted Content 1400 chars]` — so a placeholder
/// left over from something else does not pass for this paste. Claude Code
/// numbers its pastes instead, and there the marker is all there is to go on.
pub fn collapsed_paste_of(screen: &str, bytes: usize) -> bool {
    if let Some(at) = screen.find("[Pasted Content ") {
        let rest = &screen[at + "[Pasted Content ".len()..];
        let count: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = count.parse::<usize>() {
            return n == bytes;
        }
    }
    collapsed_paste(screen)
}

/// The slash command a payload opens with, if it opens with one.
///
/// It matters because a folded paste is not scanned for commands: the host
/// takes the whole thing as ordinary text, so `/plan …` runs as an ordinary
/// request at the ordinary model. Weft must not submit that — it would be
/// doing something other than what was confirmed.
pub fn leading_command(payload: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(payload);
    let first = text.lines().next()?.trim_start();
    let word = first.strip_prefix('/')?.split_whitespace().next()?;
    (!word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ':'))
        .then(|| format!("/{word}"))
}

/// And the head, because a composer that was still waking up can swallow the
/// first characters of a paste. Watching only the tail let Weft press Enter on
/// a prompt whose beginning had been eaten, and the harness then acted on
/// something the person never asked for.
pub fn echo_head(payload: &[u8]) -> Option<String> {
    let text = squeeze(&String::from_utf8_lossy(payload));
    if text.chars().count() < 4 {
        return None;
    }
    Some(text.chars().take(24).collect())
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
    fn a_collapsed_paste_is_what_a_folded_prompt_looks_like() {
        // Read off Claude Code 2.1.278 taking a multi-line `/goal …` prompt.
        assert!(collapsed_paste("❯ [Pasted text #1 +13 lines]"));
        assert!(!collapsed_paste("❯ /goal Keep working on this codebase"));
        assert!(!collapsed_paste("❯ "));
    }

    #[test]
    fn a_composer_that_swallowed_the_first_characters_is_not_an_echo() {
        // Found by a W4 run: Codex dropped "ma" from the front of the paste,
        // the tail still matched, and Weft submitted "ke health()…".
        let payload = b"/rf:ask make health() report the real package version";
        let head = echo_head(payload).expect("head");
        let tail = echo_tail(payload).expect("tail");
        let swallowed = "› ask make health() report the real package version";
        assert!(squeeze(swallowed).contains(&tail), "the tail alone was never the problem");
        assert!(!squeeze(swallowed).contains(&head), "the head is what catches it: {head:?}");
    }

    #[test]
    fn the_echo_tail_survives_a_wrapped_composer() {
        let tail =
            echo_tail(b"/rf:ask make health() report the real package version").expect("tail");
        let wrapped = "› /rf:ask make health() report the\n  real package  version";
        assert!(squeeze(wrapped).contains(&tail), "tail {tail:?} not in {wrapped:?}");
    }

    #[test]
    fn a_very_short_payload_has_no_distinctive_tail() {
        assert_eq!(echo_tail(b"ok"), None);
    }

    #[test]
    fn the_echo_tail_is_the_end_not_the_start() {
        let tail = echo_tail(b"$rf:eval").expect("tail");
        assert_eq!(tail, "$rf:eval");
    }

    #[test]
    fn a_stall_never_produces_a_retry() {
        assert_eq!(after_stall(), None);
    }

    #[test]
    fn a_folded_paste_is_recognised_in_both_hosts_wording() {
        // Measured against the installed versions. The reader before this one
        // wanted "[Pasted text" and "lines]" together, which matched neither,
        // so every long prompt failed echo verification and sat unsent.
        assert!(collapsed_paste("› [Pasted Content 1206 chars]"), "codex 0.155.1");
        assert!(collapsed_paste("❯ [Pasted text #1]"), "claude code 2.1.278");
        assert!(!collapsed_paste("› /plan ship the thing"));
        assert!(!collapsed_paste(""));
    }

    #[test]
    fn a_placeholder_from_something_else_does_not_pass_for_this_paste() {
        // Codex names the size, so it can be checked. A stale placeholder
        // reporting a different one is not this prompt's echo.
        assert!(collapsed_paste_of("› [Pasted Content 1400 chars]", 1400));
        assert!(!collapsed_paste_of("› [Pasted Content 1400 chars]", 42));
        // Claude Code numbers its pastes instead, so the marker is all there is.
        assert!(collapsed_paste_of("❯ [Pasted text #1]", 42));
        assert!(!collapsed_paste_of("❯ /plan ship it", 42));
    }

    #[test]
    fn a_prompt_that_opens_with_a_command_says_which() {
        assert_eq!(leading_command(b"/plan ship the thing").as_deref(), Some("/plan"));
        assert_eq!(leading_command(b"/goal do it\nand then\nmore").as_deref(), Some("/goal"));
        assert_eq!(leading_command(b"/review").as_deref(), Some("/review"));
        assert_eq!(leading_command(b"/rf:ask something").as_deref(), Some("/rf:ask"));
    }

    #[test]
    fn ordinary_text_opens_with_no_command() {
        for payload in [
            &b"ship the thing"[..],
            &b"$rf:ask something"[..],
            // A path is not a command, and neither is a bare slash.
            &b"/usr/local/bin/thing is missing"[..],
            &b"/ "[..],
            &b""[..],
        ] {
            assert_eq!(leading_command(payload), None, "{:?}", String::from_utf8_lossy(payload));
        }
    }

    use crate::ledger::Delivery;

    fn plan(folds: bool) -> Delivery {
        Delivery {
            mode: Some("/plan".into()),
            kind: Some("mode".into()),
            active: Some("Plan mode".into()),
            prefix_bytes: 6,
            folds,
        }
    }

    fn goal(folds: bool) -> Delivery {
        Delivery {
            mode: Some("/goal".into()),
            kind: Some("inline".into()),
            active: None,
            prefix_bytes: 6,
            folds,
        }
    }

    #[test]
    fn a_prompt_the_host_will_not_fold_goes_in_whole() {
        // The command is read straight out of the paste; nothing to work around.
        assert_eq!(Handoff::choose(&plan(false)), Handoff::Whole);
        assert_eq!(Handoff::choose(&goal(false)), Handoff::Whole);
        assert_eq!(Handoff::choose(&Delivery::default()), Handoff::Whole);
    }

    #[test]
    fn a_mode_past_the_fold_is_entered_first_and_confirmed() {
        // `/plan` switches a mode that persists, so it can be sent alone and
        // the host says when it is on. Weft waits for that rather than assuming.
        assert_eq!(
            Handoff::choose(&plan(true)),
            Handoff::EnterMode { command: "/plan".into(), at: 6, active: "Plan mode".into() }
        );
    }

    #[test]
    fn an_inline_command_past_the_fold_is_typed_in_front_of_the_body() {
        // `/goal` takes its objective as the argument: sent alone it only views
        // the goal, so there is no mode to enter and nothing to confirm.
        assert_eq!(
            Handoff::choose(&goal(true)),
            Handoff::TypeCommand { command: "/goal".into(), at: 6 }
        );
    }

    #[test]
    fn a_mode_with_nothing_to_look_for_is_not_entered_blind() {
        // Without something the host shows, entering the mode could not be
        // confirmed — so it is typed instead, which needs no confirmation.
        let unverifiable = Delivery { active: None, ..plan(true) };
        assert_eq!(
            Handoff::choose(&unverifiable),
            Handoff::TypeCommand { command: "/plan".into(), at: 6 }
        );
    }

    #[test]
    fn the_body_is_the_prompt_without_its_command() {
        let prompt = b"/plan Ship the endpoint.\n";
        assert_eq!(Handoff::choose(&plan(true)).body(prompt), b"Ship the endpoint.\n");
        assert_eq!(Handoff::choose(&goal(true)).body(prompt), b"Ship the endpoint.\n");
        // Whole means whole: the command goes in with everything else.
        assert_eq!(Handoff::Whole.body(prompt), prompt);
        // And a recorded offset past the end never panics or truncates oddly.
        let odd = Handoff::TypeCommand { command: "/plan".into(), at: 9_000 };
        assert_eq!(odd.body(prompt), prompt);
    }
}
