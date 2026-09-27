//! An agent's state, as its harness's hooks reported it.
//!
//! RingFrame's plugin records four plain events per session, the same for
//! every harness, in `<project>/.fab7/rf/sessions/<harness>/<id>/turns.jsonl`
//! ([ADR-0013], turn-state.md). Weft reads them and never writes them, and it
//! reads nothing off the screen: a state is a recorded report or it is none.
//!
//! [ADR-0013]: ../../../plans/weft/adr/0013-the-agents-turn-is-a-hook-fact.md

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ledger::Unit;

/// The four events. Which hook stands for each is the plugin's to say; Weft
/// never knows a hook's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    /// A session started and is at its input.
    Ready,
    /// A turn started.
    Working,
    /// The harness is asking its person something.
    Waiting,
    /// The turn ended. Never "done": that is an Eval and a Seal.
    TurnEnded,
}

impl Turn {
    pub fn recorded(text: &str) -> Option<Self> {
        match text {
            "ready" => Some(Turn::Ready),
            "working" => Some(Turn::Working),
            "waiting" => Some(Turn::Waiting),
            "turn_ended" => Some(Turn::TurnEnded),
            _ => None,
        }
    }

    /// What the list says.
    pub fn plain(self) -> &'static str {
        match self {
            Turn::Ready => "ready",
            Turn::Working => "working",
            Turn::Waiting => "waiting for you",
            Turn::TurnEnded => "turn ended",
        }
    }
}

/// One session's receipts: when it was first heard of, and its latest event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub harness: String,
    pub id: String,
    /// The first event's time, in milliseconds since the epoch.
    pub first: i64,
    pub latest: Turn,
    /// The latest event's time, in milliseconds since the epoch.
    pub at: i64,
}

/// One `turns.jsonl`, read tolerantly: a line that is torn, unreadable, or
/// names an event that is not one of the four is skipped. No event, no
/// session.
pub fn read(harness: &str, id: &str, text: &str) -> Option<Session> {
    let mut out: Option<Session> = None;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let Some(turn) = v.get("event").and_then(Value::as_str).and_then(Turn::recorded) else {
            continue;
        };
        let Some(at) = v.get("time").and_then(Value::as_str).and_then(millis) else { continue };
        match &mut out {
            None => {
                out = Some(Session {
                    harness: harness.into(),
                    id: id.into(),
                    first: at,
                    latest: turn,
                    at,
                })
            }
            Some(s) => {
                s.first = s.first.min(at);
                if at >= s.at {
                    s.latest = turn;
                    s.at = at;
                }
            }
        }
    }
    out
}

/// Why Weft will not type into an agent on its own. The person may still say
/// to type anyway.
pub const WORKING: &str = "the agent is working";
pub const ASKING: &str = "the agent is asking you something";
pub const UNKNOWN: &str = "Weft can't tell whether the agent is ready";

/// May Weft type into an agent in this state? Only after `ready` or
/// `turn_ended`: Weft recognises ready rather than every way of being
/// unready, so no event at all is a refusal too.
pub fn may_type(state: Option<Turn>) -> Result<(), &'static str> {
    match state {
        Some(Turn::Ready | Turn::TurnEnded) => Ok(()),
        Some(Turn::Working) => Err(WORKING),
        Some(Turn::Waiting) => Err(ASKING),
        None => Err(UNKNOWN),
    }
}

/// The session a unit of work is in: where its prompt was seen to arrive,
/// else where it was asked. Both are the ledger's; nothing is inferred.
pub fn session_of<'a>(unit: &Unit, sessions: &'a [Session]) -> Option<&'a Session> {
    let (harness, id) = match &unit.arrived {
        Some(a) => (a.host.as_str(), a.session.as_str()),
        None => (unit.harness.as_str(), unit.session_ref.as_deref()?),
    };
    sessions.iter().find(|s| s.harness == harness && s.id == id)
}

/// The agent state a unit of work's row shows, if its session reported one.
pub fn state_of(unit: &Unit, sessions: &[Session]) -> Option<Turn> {
    session_of(unit, sessions).map(|s| s.latest)
}

/// What was last done to a pane that a receipt would answer: Weft started
/// it, which its `ready` follows, or someone pressed Enter in it, which a
/// `working` follows, or the `ready` of a new session (as `/clear` starts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Poke {
    /// In milliseconds since the epoch.
    pub at: i64,
    pub enter: bool,
}

/// How a pane came to run its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Weft started the pane to resume it.
    Resumed,
    /// Its `ready` answered the pane's start.
    AfterStart,
    /// Its `ready` or `working` answered an Enter in the pane.
    AfterEnter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bound {
    pub session: String,
    pub how: How,
}

/// A pane, as Weft's pane map keeps it: what it was started as, and the
/// session it runs once Weft has bound one to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub pane: u32,
    pub harness: String,
    /// The command line it was started with.
    pub spec: String,
    /// When Weft started it, in milliseconds since the epoch.
    pub at: i64,
    pub session: Option<Bound>,
    /// The latest start or Enter that no receipt has answered yet.
    #[serde(skip)]
    pub poke: Option<Poke>,
    #[serde(skip)]
    pub running: bool,
}

impl Started {
    /// A pane Weft has just started, to resume `resumed` when it names a
    /// session: that one is known at start, so no command line is read.
    pub fn new(pane: u32, harness: &str, spec: &str, at: i64, resumed: Option<String>) -> Self {
        Started {
            pane,
            harness: harness.into(),
            spec: spec.into(),
            at,
            poke: resumed.is_none().then_some(Poke { at, enter: false }),
            session: resumed.map(|session| Bound { session, how: How::Resumed }),
            running: true,
        }
    }

    /// Someone pressed Enter in it: the person, or Weft sending on their yes.
    pub fn enter(&mut self, at: i64) {
        self.poke = Some(Poke { at, enter: true });
    }
}

/// How long a start or an Enter waits for the receipt that answers it. A
/// harness says `ready` seconds after it starts and `working` at once after
/// an Enter; one nothing answered by then, such as an Enter in an empty
/// composer, answers nothing later.
pub const ANSWERED_WITHIN: i64 = 30_000;

/// A session's new receipt, and what it binds (turn-state.md §4).
///
/// Its own pane's start or Enter is answered by it. A session no pane runs
/// goes to the one running pane of its harness whose unanswered start or
/// Enter came before this receipt; that pane takes it, as a pane that
/// starts a new session does. When two panes could have, only this session
/// stays unbound, until a later receipt of it answers one of them alone.
/// A binding is decided here once and kept. True when the map changed.
pub fn heard(panes: &mut [Started], s: &Session) -> bool {
    let theirs = |p: &Started| p.running && p.harness == s.harness;
    let runs = |p: &Started| p.session.as_ref().is_some_and(|b| b.session == s.id);
    if let Some(p) = panes.iter_mut().find(|p| theirs(p) && runs(p)) {
        if p.poke.is_some_and(|k| k.at <= s.at) {
            p.poke = None;
        }
        return false;
    }
    let answers = |k: Poke| {
        k.at <= s.at
            && s.at - k.at <= ANSWERED_WITHIN
            && match s.latest {
                Turn::Ready => true,
                Turn::Working => k.enter,
                _ => false,
            }
    };
    let could: Vec<usize> = (0..panes.len())
        .filter(|&i| theirs(&panes[i]) && panes[i].poke.is_some_and(answers))
        .collect();
    let [one] = could[..] else { return false };
    let p = &mut panes[one];
    let how =
        if p.poke.take().is_some_and(|k| k.enter) { How::AfterEnter } else { How::AfterStart };
    p.session = Some(Bound { session: s.id.clone(), how });
    true
}

/// Each pane's state: its session's latest event, or none.
pub fn pane_states(panes: &[Started], sessions: &[Session]) -> Vec<Option<Turn>> {
    panes
        .iter()
        .map(|p| {
            let b = p.session.as_ref().filter(|_| p.running)?;
            sessions.iter().find(|s| s.harness == p.harness && s.id == b.session).map(|s| s.latest)
        })
        .collect()
}

/// The shape RingFrame writes a time in, for a test that writes receipts.
#[cfg(any(test, feature = "fixtures"))]
pub fn stamp(millis: i64) -> String {
    // Howard Hinnant's civil-from-days.
    let (days, rem) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let (h, mi, sec, ms) = (rem / 3_600_000, rem / 60_000 % 60, rem / 1000 % 60, rem % 1000);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.{ms:03}Z")
}

/// Milliseconds since the epoch from the shape RingFrame writes:
/// `YYYY-MM-DDTHH:MM:SS[.mmm]Z`. Anything else is not a time Weft reads.
pub fn millis(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || !text.ends_with('Z') {
        return None;
    }
    let num = |a: usize, z: usize| text.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let frac = match text[19..text.len() - 1].strip_prefix('.') {
        Some(f) => format!("{f:0<3}").get(..3)?.parse::<i64>().ok()?,
        None if text.len() == 20 => 0,
        None => return None,
    };
    // Howard Hinnant's days-from-civil.
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if mo > 2 { mo - 3 } else { mo + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some((days * 86_400 + h * 3600 + mi * 60 + s) * 1000 + frac)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::project;
    use serde_json::json;

    const T0: &str = "2026-09-27T10:00:00.000Z";

    fn line(event: &str, session: &str, time: &str) -> String {
        format!("{}\n", json!({"event": event, "session_id": session, "time": time}))
    }

    fn session(harness: &str, id: &str, first: i64, latest: Turn) -> Session {
        Session { harness: harness.into(), id: id.into(), first, latest, at: first }
    }

    fn asked(id: &str, session: &str) -> Value {
        json!({"type": "ask.compiled", "id": id, "time": T0,
               "data": {"title": "t", "delivery_mode": "human_handoff",
                        "host": {"name": "claude-code", "session_ref": session,
                                 "session_ref_source": "capture"}}})
    }

    #[test]
    fn the_latest_event_is_the_state_and_a_line_that_is_not_one_is_skipped() {
        let text = [
            line("ready", "s1", "2026-09-27T10:00:00.000Z"),
            line("working", "s1", "2026-09-27T10:00:05.000Z"),
            "{torn".to_string(),
            line("done", "s1", "2026-09-27T10:00:09.000Z"),
            line("turn_ended", "s1", "2026-09-27T10:00:07.000Z"),
        ]
        .concat();
        let s = read("claude-code", "s1", &text).expect("a session");
        assert_eq!(s.latest, Turn::TurnEnded);
        assert_eq!(s.first, millis("2026-09-27T10:00:00.000Z").unwrap());
        assert_eq!(read("claude-code", "s1", "{torn\n"), None, "no event, no session");
    }

    /// turn-state.md §6 test 1: a ledger fixture plus a receipt fixture
    /// project the expected state on the right row.
    #[test]
    fn a_receipt_projects_its_state_on_the_row_of_the_ask_in_that_session() {
        let units = project(&[asked("ask_1", "s1"), asked("ask_2", "s2")]);
        let sessions = [session("claude-code", "s2", 1, Turn::Working)];
        assert_eq!(state_of(&units[0], &sessions), None);
        assert_eq!(state_of(&units[1], &sessions), Some(Turn::Working));
    }

    /// Where the prompt arrived is where the work goes on.
    #[test]
    fn once_its_prompt_arrives_an_ask_follows_the_session_it_arrived_in() {
        let arrived = json!({"type": "ask.submission", "id": "ask_1", "time": T0,
            "data": {"state": "observed", "as_modified": false,
                     "host": {"name": "codex", "session_ref": "c9"}}});
        let units = project(&[asked("ask_1", "s1"), arrived]);
        let sessions = [
            session("claude-code", "s1", 1, Turn::TurnEnded),
            session("codex", "c9", 2, Turn::Working),
        ];
        assert_eq!(state_of(&units[0], &sessions), Some(Turn::Working));
    }

    /// §6 test 2: a receipt for a session no Ask names changes no row.
    #[test]
    fn a_receipt_for_a_session_no_ask_names_changes_no_row() {
        let units = project(&[asked("ask_1", "s1")]);
        let sessions = [
            session("claude-code", "elsewhere", 1, Turn::Waiting),
            session("codex", "s1", 1, Turn::Waiting),
        ];
        assert_eq!(state_of(&units[0], &sessions), None, "another session, or another harness");
    }

    /// §6 test 3: with no receipts, no row shows a state and Weft does not
    /// type on its own.
    #[test]
    fn with_no_receipts_nothing_has_a_state_and_nothing_may_be_typed() {
        let units = project(&[asked("ask_1", "s1")]);
        assert_eq!(state_of(&units[0], &[]), None);
        assert_eq!(pane_states(&[started(0, 0)], &[]), vec![None]);
        assert_eq!(may_type(None), Err(UNKNOWN));
    }

    /// §6 test 4: Weft types on its own after `ready` and `turn_ended`, and
    /// after nothing else; each refusal names its reason.
    #[test]
    fn weft_types_on_its_own_only_after_ready_or_a_turn_that_ended() {
        assert_eq!(may_type(Some(Turn::Ready)), Ok(()));
        assert_eq!(may_type(Some(Turn::TurnEnded)), Ok(()));
        assert_eq!(may_type(Some(Turn::Working)), Err("the agent is working"));
        assert_eq!(may_type(Some(Turn::Waiting)), Err("the agent is asking you something"));
        assert_eq!(may_type(None), Err("Weft can't tell whether the agent is ready"));
    }

    fn started(pane: u32, at: i64) -> Started {
        Started::new(pane, "claude-code", "claude", at, None)
    }

    fn receipt(id: &str, latest: Turn, at: i64) -> Session {
        Session { harness: "claude-code".into(), id: id.into(), first: at, latest, at }
    }

    fn bound(p: &Started) -> Option<(&str, How)> {
        p.session.as_ref().map(|b| (b.session.as_str(), b.how))
    }

    #[test]
    fn a_ready_after_a_start_binds_that_pane_and_a_session_from_before_it_binds_none() {
        let mut panes = [started(0, 1000)];
        assert!(!heard(&mut panes, &receipt("old", Turn::TurnEnded, 500)), "before every pane");
        assert!(heard(&mut panes, &receipt("a", Turn::Ready, 1800)));
        assert_eq!(bound(&panes[0]), Some(("a", How::AfterStart)));
        assert_eq!(panes[0].poke, None, "answered");
    }

    #[test]
    fn two_panes_started_together_each_get_their_own_state_once_each_is_typed_into() {
        let mut panes = [started(0, 1000), started(1, 1010)];
        // Either `ready` could be either pane's.
        assert!(!heard(&mut panes, &receipt("a", Turn::Ready, 2000)));
        assert!(!heard(&mut panes, &receipt("b", Turn::Ready, 2100)));
        assert_eq!((bound(&panes[0]), bound(&panes[1])), (None, None));
        // A `working` answers an Enter, and only one pane had one.
        panes[1].enter(5000);
        let b = receipt("b", Turn::Working, 5100);
        assert!(heard(&mut panes, &b));
        panes[0].enter(6000);
        let a = receipt("a", Turn::Working, 6100);
        assert!(heard(&mut panes, &a));
        assert_eq!(bound(&panes[0]), Some(("a", How::AfterEnter)));
        assert_eq!(bound(&panes[1]), Some(("b", How::AfterEnter)));
        assert_eq!(pane_states(&panes, &[a, b]), vec![Some(Turn::Working), Some(Turn::Working)]);
    }

    #[test]
    fn a_pane_resumed_with_extra_flags_runs_its_session_from_the_start() {
        let spec = "claude --resume old --model opus --effort high";
        let mut panes = [Started::new(0, "claude-code", spec, 5000, Some("old".into()))];
        assert_eq!(bound(&panes[0]), Some(("old", How::Resumed)));
        let old = receipt("old", Turn::Ready, 5500);
        assert!(!heard(&mut panes, &old), "already its own");
        assert_eq!(pane_states(&panes, &[old]), vec![Some(Turn::Ready)]);
    }

    #[test]
    fn an_ambiguous_session_leaves_every_other_pane_bound() {
        let mut panes = [started(0, 1000), started(1, 3000), started(2, 3005)];
        let a = receipt("a", Turn::Ready, 1500);
        assert!(heard(&mut panes, &a));
        // The person types in the first while two more start.
        panes[0].enter(3002);
        let b = receipt("b", Turn::Ready, 4000);
        assert!(!heard(&mut panes, &b), "three panes could have started it");
        assert_eq!(bound(&panes[0]), Some(("a", How::AfterStart)), "nothing was taken away");
        let busy = receipt("a", Turn::Working, 4100);
        assert_eq!(
            pane_states(&panes, &[busy, b]),
            vec![Some(Turn::Working), None, None],
            "the bound pane keeps its state"
        );
    }

    #[test]
    fn a_pane_that_starts_a_new_session_takes_it() {
        // `/clear` starts a new session in the same pane.
        let mut panes = [started(0, 1000)];
        assert!(heard(&mut panes, &receipt("a", Turn::Ready, 1500)));
        panes[0].enter(9000);
        assert!(heard(&mut panes, &receipt("c", Turn::Ready, 9300)));
        assert_eq!(bound(&panes[0]), Some(("c", How::AfterEnter)));
    }

    #[test]
    fn an_enter_nothing_answered_is_forgotten_rather_than_blocking_the_next_pane() {
        let later = 2000 + ANSWERED_WITHIN;
        let mut panes = [started(0, 1000), started(1, later + 1000)];
        panes[0].enter(2000); // an empty composer: no receipt follows
        assert!(heard(&mut panes, &receipt("b", Turn::Ready, later + 1500)));
        assert_eq!(bound(&panes[1]).map(|b| b.0), Some("b"));
    }

    #[test]
    fn a_pane_that_is_not_running_has_no_state_and_takes_nothing() {
        let mut p = started(0, 10);
        p.running = false;
        let mut panes = [p];
        let a = receipt("a", Turn::Ready, 12);
        assert!(!heard(&mut panes, &a));
        assert_eq!(pane_states(&panes, &[a]), vec![None]);
    }

    #[test]
    fn a_stamp_reads_back_as_the_instant_it_names() {
        for ms in [0, 1_767_225_600_007, 1_709_164_800_000, 4_102_444_800_999] {
            assert_eq!(millis(&stamp(ms)), Some(ms), "{}", stamp(ms));
        }
    }

    #[test]
    fn a_recorded_time_reads_as_the_instant_it_named() {
        assert_eq!(millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(millis("2026-01-01T00:00:00.007Z"), Some(1_767_225_600_007));
        assert_eq!(millis("2024-02-29T00:00:00Z"), Some(1_709_164_800_000));
        assert_eq!(millis("2026-01-01T00:00:00+01:00"), None, "not RingFrame's shape");
        assert_eq!(millis("nonsense"), None);
    }
}
