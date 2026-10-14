//! An agent's state, as its harness's hooks reported it.
//!
//! Each hook runs `weft signal`, and weftd records what its harness file's
//! `[signal]` says the hook means: four events about the turn and one about
//! the session, the same for every harness, one line per change in Weft's own
//! receipts, `~/.fab7/weft/sessions/<project key>/<harness>/<id>.jsonl`
//! (turn-state.md). Weft reads nothing off the screen:
//! a state is a recorded report or it is none.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ledger::Unit;

/// The five events. Which hook stands for each is the harness file's to
/// say; Weft's code never knows a hook's name.
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
    /// The session ended: its harness said so.
    Ended,
}

impl Turn {
    pub fn recorded(text: &str) -> Option<Self> {
        match text {
            "ready" => Some(Turn::Ready),
            "working" => Some(Turn::Working),
            "waiting" => Some(Turn::Waiting),
            "turn_ended" => Some(Turn::TurnEnded),
            "ended" => Some(Turn::Ended),
            _ => None,
        }
    }

    /// What the list says.
    pub fn plain(self) -> &'static str {
        match self {
            Turn::Ready => "ready",
            Turn::Working => "working",
            Turn::Waiting => "needs you",
            Turn::TurnEnded => "turn ended",
            Turn::Ended => "ended",
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
        // A question asked without stopping the turn, still unanswered: the
        // harness is asking its person, whatever its turn is doing.
        let asking = v.get("asking") == Some(&Value::Bool(true));
        let turn = if asking && turn != Turn::Ended { Turn::Waiting } else { turn };
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

/// Whether an agent in this state is asking its person for input: a
/// question, a choice, a permission. This is NEEDS YOU, and nothing else is:
/// work waiting on its next step is the person's to pick up when they
/// choose, and a finished turn says nothing about whether the work is done.
pub fn is_asking(state: Option<Turn>) -> bool {
    state == Some(Turn::Waiting)
}

/// NEEDS YOU: how many of these panes' agents are asking their person, of
/// one harness or of all. `states` is one per pane, in the same order.
pub fn asking(
    panes: &[crate::board::PaneInfo],
    states: &[Option<Turn>],
    harness: Option<&str>,
) -> usize {
    panes
        .iter()
        .zip(states)
        .filter(|(p, s)| is_asking(**s) && harness.is_none_or(|h| p.harness == h))
        .count()
}

/// OPEN: the agents running, which is the harnesses the person has open,
/// not the Asks on the record.
pub fn open(panes: &[crate::board::PaneInfo]) -> usize {
    panes.iter().filter(|p| p.running).count()
}

/// Why Weft will not type into an agent on its own, as `words` keys. The
/// person may still say to type anyway.
pub const WORKING: &str = "not_ready.working";
pub const ASKING: &str = "not_ready.asking";
pub const UNKNOWN: &str = "not_ready.unknown";

/// May Weft type into an agent in this state? Only after `ready` or
/// `turn_ended`: Weft recognises ready rather than every way of being
/// unready, so no event at all is a refusal too.
pub fn may_type(state: Option<Turn>) -> Result<(), &'static str> {
    match state {
        Some(Turn::Ready | Turn::TurnEnded) => Ok(()),
        Some(Turn::Working) => Err(crate::words::word(WORKING)),
        Some(Turn::Waiting) => Err(crate::words::word(ASKING)),
        Some(Turn::Ended) | None => Err(crate::words::word(UNKNOWN)),
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

/// The turn of a unit's session in a project's session index.
pub fn turn_in(unit: &Unit, index: &[crate::sessions::Row]) -> Option<Turn> {
    let (harness, id) = match &unit.arrived {
        Some(a) => (a.host.as_str(), a.session.as_str()),
        None => (unit.harness.as_str(), unit.session_ref.as_deref()?),
    };
    index.iter().find(|r| r.harness == harness && r.id == id)?.turn
}

/// How a pane came to run its session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// Weft started the pane to resume it.
    Resumed,
    /// A signal from inside the pane, carrying the pane's token, named it.
    Signalled,
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
            session: resumed.map(|session| Bound { session, how: How::Resumed }),
            running: true,
        }
    }
}

/// A signal from inside pane `pane` named `session`: that pane runs it now,
/// as a pane that starts a new session does. True when the map
/// changed.
pub fn signalled(panes: &mut [Started], pane: u32, session: &str) -> bool {
    let Some(p) = panes.iter_mut().find(|p| p.pane == pane) else { return false };
    if p.session.as_ref().is_some_and(|b| b.session == session) {
        return false;
    }
    p.session = Some(Bound { session: session.to_string(), how: How::Signalled });
    true
}

/// The pane a prompt for this unit goes to (ask-flow.md step 4): the running
/// pane that runs the session its prompt arrived in, else the one it was asked
/// in; with none, the first pane of its harness.
pub fn owning_pane(panes: &[Started], unit: &Unit) -> Option<u32> {
    let (harness, id) = match &unit.arrived {
        Some(a) => (a.host.as_str(), Some(a.session.as_str())),
        None => (unit.harness.as_str(), unit.session_ref.as_deref()),
    };
    let runs = |p: &&Started| {
        p.running
            && p.harness == harness
            && p.session.as_ref().is_some_and(|b| Some(b.session.as_str()) == id)
    };
    panes
        .iter()
        .find(runs)
        .or_else(|| panes.iter().find(|p| p.harness == unit.harness))
        .map(|p| p.pane)
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

/// A time in the shape every receipt carries: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
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
        assert_eq!(may_type(None), Err(crate::words::word(UNKNOWN)));
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

    /// A pane's session is the one its own signal names.
    #[test]
    fn a_signal_naming_a_pane_binds_its_session_to_that_pane() {
        let mut panes = [started(0, 1000), started(1, 1010)];
        assert!(signalled(&mut panes, 1, "b"));
        assert!(signalled(&mut panes, 0, "a"));
        assert_eq!(bound(&panes[0]), Some(("a", How::Signalled)));
        assert_eq!(bound(&panes[1]), Some(("b", How::Signalled)));
        assert!(!signalled(&mut panes, 0, "a"), "already its own");
        let (a, b) = (receipt("a", Turn::Working, 6100), receipt("b", Turn::Waiting, 6200));
        assert_eq!(pane_states(&panes, &[a, b]), vec![Some(Turn::Working), Some(Turn::Waiting)]);
        assert!(!signalled(&mut panes, 7, "c"), "no such pane");
    }

    #[test]
    fn a_pane_resumed_with_extra_flags_runs_its_session_from_the_start() {
        let spec = "claude --resume old --model opus --effort high";
        let mut panes = [Started::new(0, "claude-code", spec, 5000, Some("old".into()))];
        assert_eq!(bound(&panes[0]), Some(("old", How::Resumed)));
        assert!(!signalled(&mut panes, 0, "old"), "already its own");
        assert_eq!(
            pane_states(&panes, &[receipt("old", Turn::Ready, 5500)]),
            vec![Some(Turn::Ready)]
        );
    }

    #[test]
    fn a_pane_that_starts_a_new_session_takes_it() {
        // `/clear` starts a new session in the same pane.
        let mut panes = [started(0, 1000)];
        assert!(signalled(&mut panes, 0, "a"));
        assert!(signalled(&mut panes, 0, "c"));
        assert_eq!(bound(&panes[0]), Some(("c", How::Signalled)));
    }

    #[test]
    fn a_pane_that_is_not_running_has_no_state() {
        let mut p = started(0, 10);
        p.running = false;
        let mut panes = [p];
        signalled(&mut panes, 0, "a");
        assert_eq!(pane_states(&panes, &[receipt("a", Turn::Ready, 12)]), vec![None]);
    }

    /// ask-flow.md step 4: a prompt goes to the pane that owns its Ask, the
    /// one running the session it was asked or arrived in; only when none
    /// does, to the first pane of its harness (ringframe-ask-ledger-q09).
    #[test]
    fn a_send_goes_to_the_pane_that_owns_the_ask() {
        let units = project(&[asked("ask_1", "s2")]);
        let mut panes = [started(0, 1000), started(1, 1010), started(2, 1020)];
        panes[2].harness = "codex".into();
        assert_eq!(owning_pane(&panes, &units[0]), Some(0), "no pane runs it yet");
        signalled(&mut panes, 1, "s2");
        assert_eq!(owning_pane(&panes, &units[0]), Some(1));
        panes[1].running = false;
        assert_eq!(owning_pane(&panes, &units[0]), Some(0), "its pane has gone");
        let other = project(&[asked("ask_9", "s9")]);
        let mut codex = [panes[2].clone()];
        codex[0].harness = "codex".into();
        assert_eq!(owning_pane(&codex, &other[0]), None, "no pane of its harness");
    }

    #[test]
    fn a_session_that_ended_is_its_fifth_event() {
        assert_eq!(Turn::recorded("ended"), Some(Turn::Ended));
        let text =
            [line("working", "s1", T0), line("ended", "s1", "2026-09-27T10:00:09.000Z")].concat();
        assert_eq!(read("claude-code", "s1", &text).unwrap().latest, Turn::Ended);
        assert!(may_type(Some(Turn::Ended)).is_err(), "nothing to type into");
        assert!(!is_asking(Some(Turn::Ended)));
    }

    #[test]
    fn needs_you_is_an_agent_asking_and_open_is_an_agent_running() {
        let pane = |harness: &str, running| crate::board::PaneInfo {
            pane: 0,
            harness: harness.into(),
            spec: String::new(),
            running,
            turbo: false,
        };
        let panes = [pane("codex", true), pane("claude-code", true), pane("codex", false)];
        let states = [Some(Turn::Waiting), Some(Turn::TurnEnded), Some(Turn::Waiting)];
        assert_eq!(asking(&panes, &states, None), 2);
        assert_eq!(asking(&panes, &states, Some("codex")), 2);
        assert_eq!(
            asking(&panes, &states, Some("claude-code")),
            0,
            "a turn that ended asks nothing"
        );
        assert_eq!(open(&panes), 2);
        assert!(
            is_asking(Some(Turn::Waiting)) && !is_asking(None) && !is_asking(Some(Turn::Ready))
        );
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
