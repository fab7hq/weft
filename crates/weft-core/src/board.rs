//! What the board knows, and what may be done with it.
//!
//! One borrowed view of the facts an action's availability depends on, and the
//! rule that reads them. It lives here and not in whatever is drawing at the
//! time, so a second client asks the same question and gets the same
//! sentence.

use crate::ledger::{Sent, Unit};
use crate::readiness::Readiness;
use crate::routing::Routing;

/// Something the interface offers a key for. One home for whether it can be
/// used right now, and for the sentence that says what would make it work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    // RingFrame's three acts, under RingFrame's names. Weft had two of its
    // own — `Check` and `Decide` — and two vocabularies for one lifecycle is
    // one too many.
    Ask,
    Eval,
    Seal,
    /// Carry the selected unit forward, whatever that means for it. See
    /// [`next_step`].
    Proceed,
    /// The selected unit's whole story, in one blocking view.
    Detail,
    /// A new Ask about work that already exists.
    FollowUp,
    /// Set the agent that owns the work up for RingFrame.
    ReadyUp,
    NewAgent,
    OpenProject,
    ToggleSidebar,
    /// Weft's own operations, gathered out of the bar.
    WeftMenu,
    Help,
    Quit,
}

/// What carrying a unit forward means right now. One verb on the screen,
/// this many things underneath, and the unit decides which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// The chooser was shown and never answered; show it again.
    Confirm,
    /// Confirmed and never submitted; put the prompt in front of the agent.
    Send,
    /// Submitted, and nothing has judged it.
    Eval,
    /// Judged, and nothing has decided it.
    Seal,
}

/// What a follow-up carries: the Eval that judged this work when there is one,
/// else the Ask that started it. RingFrame's Ask reads the rest from the
/// record by that id, so any harness can take the follow-up.
pub fn follow_up_of(unit: &Unit) -> &str {
    unit.check.as_ref().map_or(unit.ask_id.as_str(), |c| c.eval_id.as_str())
}

/// `None` when the unit is closed, cancelled, or waiting on someone else —
/// which is exactly when the row carries no dot.
pub fn next_step(unit: &Unit) -> Option<Next> {
    if unit.cancelled || unit.sealed.is_some() {
        return None;
    }
    if unit.check.is_some() {
        return Some(Next::Seal);
    }
    if unit.awaiting_yes() {
        return Some(Next::Confirm);
    }
    match unit.sent {
        Sent::ReadyToSend => Some(Next::Send),
        // Nothing has judged it, and it has reached the agent one way or
        // another. A prompt still sitting unconfirmed is not this.
        Sent::TakenByAgent | Sent::Arrived { .. } => Some(Next::Eval),
        _ => None,
    }
}

/// A pane, as everything outside the daemon sees one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneInfo {
    pub pane: u32,
    pub harness: String,
    /// The command line this pane was started with, so a client knows whether
    /// a session is already open somewhere rather than starting it twice.
    pub spec: String,
    pub running: bool,
}

/// Everything an availability decision reads. Borrowed: the shell owns this
/// state and builds one of these when it wants an answer.
pub struct Board<'a> {
    pub units: &'a [Unit],
    pub selected: usize,
    pub panes: &'a [PaneInfo],
    /// The pane in front of the person. A fresh Ask goes here when the project
    /// routes none.
    pub focused: usize,
    pub readiness: &'a dyn Fn(&str) -> Readiness,
    pub routing: &'a Routing,
    /// Why this workspace could not finish an Ask, if it could not.
    pub workspace_gap: Option<&'a str>,
}

impl Board<'_> {
    fn readiness(&self, harness: &str) -> Readiness {
        (self.readiness)(harness)
    }

    pub fn selected_unit(&self) -> Option<&Unit> {
        self.units.get(self.selected)
    }

    fn pane_for(&self, harness: &str) -> Option<usize> {
        self.panes.iter().position(|p| p.harness == harness)
    }

    /// Open units, as the title bar counts them.
    pub fn open_count(&self) -> usize {
        self.units.iter().filter(|u| u.is_open()).count()
    }

    /// Units waiting on a decision only the person can make. Panes waiting for
    /// an answer are an inference and carry their own dot on the tab, so they
    /// are not folded into a count the board claims to have read.
    pub fn needs_you(&self) -> usize {
        self.units.iter().filter(|u| u.needs_you()).count()
    }

    /// The harness whose readiness decides what the person can do now: the one
    /// that owns the selected work, or the agent on screen when there is none.
    /// The harness an act goes to, and so the harness its readiness is about.
    ///
    /// A project may route each of RingFrame's three acts to a harness of its
    /// own — *Codex asks, Claude implements, Codex evaluates*. Absent,
    /// everything falls back to what it did before: the harness that owns the
    /// work, or the pane in front of you.
    ///
    /// `Send` is not routed. It is the delivery of an Ask already compiled, so
    /// it follows that Ask's own record rather than a preference.
    pub fn deciding_harness(&self, act: Act) -> Option<String> {
        // An Eval routed per stage goes where its next stage runs.
        if act == Act::Eval
            && let Some(stages) = &self.routing.eval_stages
        {
            let fallback = self
                .selected_unit()
                .map(|u| u.harness.clone())
                .or_else(|| self.panes.get(self.focused).map(|p| p.harness.clone()))?;
            let (gather, debate) =
                crate::eval_stages::resolve(Some(stages), self.routing.get("eval"), &fallback);
            let gathered = self.selected_unit().is_some_and(|u| u.gathered.is_some());
            return Some(if gathered { debate.harness } else { gather.harness });
        }
        if let Some(routed) = Self::act_key(act).and_then(|k| self.routing.get(k)) {
            return Some(routed.to_string());
        }
        self.selected_unit()
            .map(|u| u.harness.clone())
            .or_else(|| self.panes.get(self.focused).map(|p| p.harness.clone()))
    }

    /// Which of RingFrame's three acts this is, if it is one of them.
    pub fn act_key(act: Act) -> Option<&'static str> {
        match act {
            Act::Ask | Act::FollowUp => Some("ask"),
            Act::Eval => Some("eval"),
            Act::Seal => Some("seal"),
            _ => None,
        }
    }

    /// The harness this act has to happen in, when no pane of it is open.
    /// That is not a refusal: the work can be picked back up there.
    pub fn missing_agent(&self, act: Act) -> Option<String> {
        let u = self.selected_unit()?;
        let name = match (act, next_step(u)) {
            (Act::Proceed, Some(Next::Send | Next::Confirm)) => u.harness.clone(),
            (Act::Proceed, Some(_)) | (Act::Eval | Act::Seal, _) => self.deciding_harness(act)?,
            _ => return None,
        };
        self.pane_for(&name).is_none().then_some(name)
    }

    /// Why an action is not available now, as one sentence. `None` means it is.
    pub fn unavailable(&self, act: Act) -> Option<String> {
        let no_pane =
            |harness: &str| format!("No {harness} pane is open, so there is nowhere to send this.");
        // An Ask this workspace could never finish is refused here rather
        // than after a turn spent composing one.
        if matches!(act, Act::Ask | Act::FollowUp)
            && let Some(gap) = self.workspace_gap
        {
            return Some(gap.to_string());
        }
        // Everything RingFrame owns waits on the harness that owns the work.
        // Readiness is per harness: Claude Code may be ready while Codex is not.
        if matches!(act, Act::Ask | Act::Proceed | Act::Eval | Act::Seal | Act::FollowUp)
            && let Some(name) = self.deciding_harness(act)
        {
            let state = self.readiness(&name);
            if !state.is_ready() {
                let mut say = state.say(&name).unwrap_or_default();
                if state.can_be_set_up() {
                    say.push_str(" [U]PDATE sets it up.");
                }
                return Some(say);
            }
        }
        match act {
            Act::NewAgent
            | Act::ToggleSidebar
            | Act::OpenProject
            | Act::WeftMenu
            | Act::Help
            | Act::Quit => None,
            Act::ReadyUp => match self.deciding_harness(Act::ReadyUp) {
                None => Some("Start an agent first — [N]EW AGENT.".into()),
                Some(name) if self.readiness(&name).is_ready() => {
                    Some(format!("{name} is already set up for RingFrame."))
                }
                // A missing CLI opens the panel too: it has nothing to run,
                // but it carries the one command the person needs.
                Some(name) if self.readiness(&name) == Readiness::Unknown => self
                    .readiness(&name)
                    .say(&name)
                    .map(|s| format!("{s} There is nothing to offer until it answers.")),
                Some(_) => None,
            },
            Act::Ask => {
                (self.panes.is_empty()).then(|| "Start an agent first — [N]EW AGENT.".to_string())
            }
            Act::FollowUp => match self.selected_unit() {
                None => Some("Nothing has been asked for yet.".into()),
                Some(_) if self.panes.is_empty() => {
                    Some("Start an agent first — [N]EW AGENT.".into())
                }
                Some(_) => None,
            },
            // One verb, so its refusal is about the one thing it would do.
            // `next_step` decides what that is; this decides whether it can.
            Act::Proceed => match self.selected_unit() {
                None => Some("Nothing has been asked for yet.".into()),
                Some(u) => match next_step(u) {
                    None => Some(format!("{} is closed. [F] FOLLOW UP asks again.", u.title)),
                    Some(Next::Send | Next::Confirm) => {
                        self.pane_for(&u.harness).is_none().then(|| no_pane(&u.harness))
                    }
                    Some(_) => {
                        let name = self.deciding_harness(act)?;
                        self.pane_for(&name).is_none().then(|| no_pane(&name))
                    }
                },
            },
            Act::Detail => self
                .selected_unit()
                .is_none()
                .then(|| "Nothing has been asked for yet.".to_string()),
            // Where it goes is the project's to say, and it needs no pane there:
            // the daemon types into an idle one of that harness, or starts one.
            Act::Eval | Act::Seal => {
                self.selected_unit().is_none().then(|| "Nothing to work on yet.".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Sent, Unit};
    use crate::readiness::Gap;

    fn pane(harness: &str) -> PaneInfo {
        PaneInfo { pane: 0, harness: harness.into(), spec: harness.into(), running: true }
    }

    fn unit(harness: &str, sent: Sent) -> Unit {
        Unit {
            ask_id: "ask_1".into(),
            title: "health endpoint".into(),
            harness: harness.into(),
            session_ref: None,
            delivery: Default::default(),
            route: "native_plan".into(),
            asked_at: "2026-09-19T14:02:00Z".into(),
            delivery_mode: "human_handoff".into(),
            cancelled: false,
            unanswered: false,
            confirmed: true,
            sent,
            check: None,
            gathered: None,
            sealed: None,
            seal_id: None,
            sealed_at: None,
        }
    }

    #[test]
    fn a_follow_up_carries_the_eval_when_there_is_one_and_the_ask_when_not() {
        let mut u = unit("codex", Sent::TakenByAgent);
        assert_eq!(follow_up_of(&u), "ask_1");
        u.check = Some(crate::ledger::Check {
            eval_id: "evl_1".into(),
            verdict: crate::ledger::Verdict::DoesntMatch,
            agreement: 0.67,
            judged_by: None,
        });
        assert_eq!(follow_up_of(&u), "evl_1");
    }

    /// Everything a second client would have to assemble. That it is this
    /// small, and needs no App and no terminal, is the point of the crate.
    fn board<'a>(
        units: &'a [Unit],
        panes: &'a [PaneInfo],
        ready: &'a dyn Fn(&str) -> Readiness,
        routing: &'a Routing,
    ) -> Board<'a> {
        Board {
            units,
            selected: 0,
            panes,
            focused: 0,
            readiness: ready,
            routing,
            workspace_gap: None,
        }
    }

    const READY: &dyn Fn(&str) -> Readiness = &|_| Readiness::Ready;

    #[test]
    fn an_act_is_available_when_its_harness_is_ready_and_open() {
        let units = [unit("codex", Sent::ReadyToSend)];
        let panes = [pane("codex")];
        let none = Routing::default();
        let b = board(&units, &panes, READY, &none);
        assert_eq!(b.unavailable(Act::Proceed), None);
        assert_eq!(b.deciding_harness(Act::Eval).as_deref(), Some("codex"));
    }

    #[test]
    fn a_routed_act_is_about_the_harness_it_goes_to() {
        let units = [unit("codex", Sent::ReadyToSend)];
        let panes = [pane("codex")];
        let routing = crate::config::read("[routing]\neval = \"claude-code\"\n")
            .routing(std::path::Path::new("/p"));
        let b = board(&units, &panes, READY, &routing);
        assert_eq!(b.deciding_harness(Act::Eval).as_deref(), Some("claude-code"));
        assert_eq!(b.unavailable(Act::Eval), None, "no claude-code pane: the daemon starts one");
        let unset: &dyn Fn(&str) -> Readiness = &|h| {
            if h == "claude-code" { Readiness::Missing(Gap::Plugin) } else { Readiness::Ready }
        };
        let said =
            board(&units, &panes, unset, &routing).unavailable(Act::Eval).expect("not set up");
        assert!(said.contains("claude-code"), "readiness is the routed harness's: {said}");
        // Seal was not routed, so it still follows the work.
        assert_eq!(b.deciding_harness(Act::Seal).as_deref(), Some("codex"));
    }

    #[test]
    fn an_eval_routed_per_stage_is_about_the_harness_its_next_stage_runs_in() {
        let mut units = [unit("claude-code", Sent::TakenByAgent)];
        let panes = [pane("claude-code")];
        let routing = crate::config::read(
            "[eval.gather]\nharness = \"codex\"\n\n[eval.debate]\nharness = \"claude-code\"\n",
        )
        .routing(std::path::Path::new("/p"));
        let b = board(&units, &panes, READY, &routing);
        assert_eq!(b.deciding_harness(Act::Eval).as_deref(), Some("codex"));
        units[0].gathered =
            Some(crate::ledger::Gathered { eval_id: "evl_1".into(), by: "codex".into() });
        let b = board(&units, &panes, READY, &routing);
        assert_eq!(b.deciding_harness(Act::Eval).as_deref(), Some("claude-code"));
    }

    #[test]
    fn readiness_is_the_first_answer_and_names_the_harness() {
        let units = [unit("codex", Sent::ReadyToSend)];
        let panes = [pane("codex")];
        let missing: &dyn Fn(&str) -> Readiness = &|_| Readiness::Missing(Gap::Plugin);
        let none = Routing::default();
        let b = board(&units, &panes, missing, &none);
        let said = b.unavailable(Act::Proceed).expect("not set up");
        assert!(said.contains("codex is not set up"), "{said}");
        assert!(said.contains("[U]PDATE"), "and says what fixes it: {said}");
    }

    #[test]
    fn a_workspace_that_could_not_finish_an_ask_refuses_it_first() {
        let units: [Unit; 0] = [];
        let panes = [pane("codex")];
        let none = Routing::default();
        let mut b = board(&units, &panes, READY, &none);
        b.workspace_gap = Some("This workspace is not a Git repository.");
        assert_eq!(
            b.unavailable(Act::Ask).as_deref(),
            Some("This workspace is not a Git repository.")
        );
        // And only for the acts that would compile one.
        assert_eq!(b.unavailable(Act::Help), None);
    }

    #[test]
    fn one_verb_carries_a_unit_the_whole_way() {
        // The ladder the detail view walks, and the only thing `[P]ROCEED`
        // has to know. Each step is where the previous one leaves the unit.
        let mut u = unit("codex", Sent::NotSent);
        u.confirmed = false;
        u.unanswered = true;
        assert_eq!(next_step(&u), Some(Next::Confirm));

        let u = unit("codex", Sent::ReadyToSend);
        assert_eq!(next_step(&u), Some(Next::Send));

        let u = unit("codex", Sent::Arrived { exact: true });
        assert_eq!(next_step(&u), Some(Next::Eval));

        let mut u = unit("codex", Sent::Arrived { exact: true });
        u.check = Some(crate::ledger::Check {
            eval_id: "evl_1".into(),
            verdict: crate::ledger::Verdict::Matches,
            agreement: 1.0,
            judged_by: None,
        });
        assert_eq!(next_step(&u), Some(Next::Seal));

        u.sealed = Some("accepted".into());
        assert_eq!(next_step(&u), None, "a closed unit has nothing to carry");

        let mut u = unit("codex", Sent::ReadyToSend);
        u.cancelled = true;
        assert_eq!(next_step(&u), None);
    }

    #[test]
    fn the_verb_is_live_exactly_when_the_row_carries_a_dot() {
        // The sidebar and the detail view may never disagree about whether
        // something is waiting.
        for sent in
            [Sent::NotSent, Sent::ReadyToSend, Sent::TakenByAgent, Sent::Arrived { exact: true }]
        {
            let u = unit("codex", sent);
            assert_eq!(next_step(&u).is_some(), u.needs_you() || next_step(&u).is_some());
            if u.needs_you() {
                assert!(next_step(&u).is_some(), "{sent:?} needs you but cannot proceed");
            }
        }
    }

    #[test]
    fn the_counts_read_the_record_and_nothing_else() {
        let units = [unit("codex", Sent::ReadyToSend), unit("codex", Sent::TakenByAgent)];
        let panes = [pane("codex")];
        let none = Routing::default();
        let b = board(&units, &panes, READY, &none);
        assert_eq!(b.open_count(), 2);
        assert_eq!(b.needs_you(), 1, "only the one waiting on a person");
    }
}
