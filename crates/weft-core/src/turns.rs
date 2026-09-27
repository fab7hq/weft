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

/// A pane, as binding it to a session needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub harness: String,
    /// When Weft started it, in milliseconds since the epoch.
    pub at: i64,
    /// The session it was started to resume, when its command line names one.
    pub resumed: Option<String>,
    pub running: bool,
}

/// Which session each pane is running.
///
/// A resumed pane runs the session its command line names. Otherwise a
/// session belongs to the pane that was started before it was first heard
/// of: the one such pane with no session yet, or the only such pane at all
/// (an agent that started a new session of its own). When more than one pane
/// could have started it, none of them is given a state again, because a
/// wrong pane's state would be a guess.
pub fn bind(panes: &[Started], sessions: &[Session]) -> Vec<Option<String>> {
    let mut bound: Vec<Option<String>> = panes.iter().map(|p| p.resumed.clone()).collect();
    let mut lost = vec![false; panes.len()];
    let mut order: Vec<&Session> = sessions.iter().collect();
    order.sort_by_key(|s| (s.first, s.id.clone()));
    for s in order {
        if bound.iter().any(|b| b.as_deref() == Some(s.id.as_str())) {
            continue;
        }
        let could: Vec<usize> = (0..panes.len())
            .filter(|&i| {
                let p = &panes[i];
                p.running && p.harness == s.harness && p.at <= s.first
            })
            .collect();
        let free: Vec<usize> =
            could.iter().copied().filter(|&i| bound[i].is_none() && !lost[i]).collect();
        match (free.as_slice(), could.as_slice()) {
            (_, []) => {}
            ([one], _) => bound[*one] = Some(s.id.clone()),
            ([], [one]) if !lost[*one] => bound[*one] = Some(s.id.clone()),
            _ => {
                for i in could {
                    lost[i] = true;
                    bound[i] = None;
                }
            }
        }
    }
    bound
}

/// Each pane's state: its session's latest event, or none.
pub fn pane_states(panes: &[Started], sessions: &[Session]) -> Vec<Option<Turn>> {
    bind(panes, sessions)
        .into_iter()
        .zip(panes)
        .map(|(id, p)| {
            let id = id.filter(|_| p.running)?;
            sessions.iter().find(|s| s.harness == p.harness && s.id == id).map(|s| s.latest)
        })
        .collect()
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
        let panes =
            [Started { harness: "claude-code".into(), at: 0, resumed: None, running: true }];
        assert_eq!(pane_states(&panes, &[]), vec![None]);
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

    fn pane(harness: &str, at: i64) -> Started {
        Started { harness: harness.into(), at, resumed: None, running: true }
    }

    #[test]
    fn a_session_belongs_to_the_pane_started_before_it_that_has_none() {
        let panes = [pane("claude-code", 10), pane("claude-code", 20), pane("codex", 15)];
        let sessions = [
            session("claude-code", "old", 5, Turn::TurnEnded),
            session("claude-code", "a", 12, Turn::Working),
            session("codex", "c", 16, Turn::Ready),
            session("claude-code", "b", 22, Turn::Waiting),
        ];
        assert_eq!(
            bind(&panes, &sessions),
            vec![Some("a".into()), Some("b".into()), Some("c".into())],
            "a session from before every pane is nobody's"
        );
        assert_eq!(
            pane_states(&panes, &sessions),
            vec![Some(Turn::Working), Some(Turn::Waiting), Some(Turn::Ready)]
        );
    }

    #[test]
    fn a_resumed_pane_runs_the_session_its_command_line_names() {
        let mut p = pane("claude-code", 50);
        p.resumed = Some("old".into());
        let sessions = [session("claude-code", "old", 5, Turn::Ready)];
        assert_eq!(pane_states(&[p], &sessions), vec![Some(Turn::Ready)]);
    }

    #[test]
    fn the_only_agent_that_could_have_started_a_new_session_moves_to_it() {
        // `/clear` starts a new session in the same pane.
        let panes = [pane("claude-code", 10)];
        let sessions = [
            session("claude-code", "a", 12, Turn::TurnEnded),
            session("claude-code", "b", 30, Turn::Ready),
        ];
        assert_eq!(bind(&panes, &sessions), vec![Some("b".into())]);
    }

    #[test]
    fn when_two_panes_could_have_started_a_session_neither_gets_a_state() {
        let panes = [pane("claude-code", 10), pane("claude-code", 11)];
        let sessions = [
            session("claude-code", "a", 12, Turn::Ready),
            session("claude-code", "b", 13, Turn::Ready),
        ];
        assert_eq!(bind(&panes, &sessions), vec![None, None]);
        // Nor after one of them starts a new session: a lost pane stays lost.
        let one_bound = [
            session("claude-code", "a", 10, Turn::Ready),
            session("claude-code", "b", 12, Turn::Ready),
        ];
        let panes = [pane("claude-code", 9), pane("claude-code", 11)];
        assert_eq!(bind(&panes, &one_bound), vec![Some("a".into()), Some("b".into())]);
        let cleared =
            [one_bound.to_vec(), vec![session("claude-code", "c", 40, Turn::Ready)]].concat();
        assert_eq!(bind(&panes, &cleared), vec![None, None]);
    }

    #[test]
    fn a_pane_that_is_not_running_has_no_state() {
        let mut p = pane("claude-code", 10);
        let sessions = [session("claude-code", "a", 12, Turn::Ready)];
        p.running = false;
        assert_eq!(pane_states(&[p], &sessions), vec![None]);
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
