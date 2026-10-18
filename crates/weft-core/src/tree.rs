//! The tree both clients draw: a project, then AGENTS and WORK.
//!
//! AGENTS is the session index weftd keeps (`sessions.md`); WORK is the record
//! as units of work. Built from those and nothing else, so the desktop's list
//! and a device's tree are one set of rows. Folding is the client's own, asked
//! here and never kept; the earlier sessions and the earlier sealed units are
//! counts (remote.md §6).

use serde::Serialize;

use crate::board::{Act, Board, PaneInfo};
use crate::ledger::Unit;
use crate::readiness::{Gap, Readiness};
use crate::routing::Routing;
use crate::sessions::State;
use crate::turns::Turn;
use crate::work::{Status, Work as Units};
use crate::workspace::Needs;

/// What one project's rows are built from. Borrowed: the client owns it.
pub struct Facts<'a> {
    pub name: &'a str,
    pub units: &'a [Unit],
    pub panes: &'a [PaneInfo],
    /// Each pane's agent state, in the order of `panes`.
    pub agents: &'a [Option<Turn>],
    /// The project's session index, as weftd built it.
    pub sessions: &'a [crate::sessions::Row],
    /// The record as units of work, as weftd built it.
    pub work: &'a Units,
    pub readiness: &'a dyn Fn(&str) -> Readiness,
    pub routing: &'a Routing,
    /// What the project still needs before RingFrame is set up in it, if
    /// it is not.
    pub setup: Option<Needs>,
}

/// What a client folds. Asked, never kept here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold {
    Project,
    /// The earlier sealed units are folded unless the client has opened them.
    Sealed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    #[serde(rename = "project")]
    pub name: String,
    /// Its units of work not sealed.
    pub open: usize,
    /// Agents waiting for the person.
    pub needs: usize,
    /// `None` when an Ask can be made here, else why not.
    pub ask: Option<Why>,
    /// `None` when RingFrame is set up here, else the checklist's facts.
    pub setup: Option<Needs>,
    #[serde(skip)]
    pub folded: bool,
    pub agents: Agents,
    pub work: Work,
}

/// AGENTS: its header's counts, the rows the tree shows, and how many more.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agents {
    pub running: usize,
    pub ended: usize,
    pub rows: Vec<Agent>,
    /// The ended sessions past the ones shown: `▸ n earlier sessions`.
    pub earlier: usize,
}

/// One session, read the same whoever runs it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Agent {
    pub session: String,
    pub harness: String,
    pub state: State,
    pub turn: Option<Turn>,
    /// Its latest Ask's title; none reads "no Ask yet".
    pub title: Option<String>,
    /// When it was last used, in milliseconds since the epoch.
    pub at: i64,
    pub asks: usize,
    /// The Weft pane that holds it, while one does.
    #[serde(skip)]
    pub pane: Option<u32>,
    /// `None` when an Ask can go to it, else why not.
    pub ask: Option<Why>,
}

/// WORK: a row per unit of work, and how many sealed ones are not listed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Work {
    /// Every open unit, then the newest sealed; all of them once unfolded.
    pub units: Vec<WorkRow>,
    /// The sealed units past those listed: `n sealed earlier`.
    pub earlier: usize,
}

/// The sealed units WORK lists before folding the rest away.
pub const SEALED_IN_TREE: usize = 3;

/// One unit of work: its root Ask's title and harness, and one status.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkRow {
    /// Its root Ask.
    pub unit: String,
    pub title: String,
    pub harness: String,
    pub status: Status,
    /// How many Asks it holds: the root and each follow-up.
    pub asks: usize,
    /// `None` when Eval can run on it, else why not.
    pub eval: Option<Why>,
    /// `None` when Seal can run on it, else why not.
    pub seal: Option<Why>,
}

/// A unit's status as its row says it: asking, judging…, drifted · 0.75,
/// sealed · accepted.
pub fn status_word(s: &Status) -> String {
    use crate::words::{fill, word};
    match s {
        Status::Asking => word("work.asking").into(),
        Status::Asked => word("work.asked").into(),
        Status::Judging => word("tree.judging").into(),
        Status::Judged { verdict, agreement } => fill(
            "work.judged",
            &[
                ("verdict", word(&format!("verdict.{}", verdict.recorded()))),
                ("agreement", &agreement.map(crate::ledger::score).unwrap_or_default()),
            ],
        )
        .trim_end_matches(" · ")
        .to_string(),
        Status::Sealed { decision: d } => fill("work.sealed", &[("decision", &decision(d))]),
    }
}

/// An Ask as a list names it, with its state word where the list says one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ask {
    pub unit: String,
    pub title: String,
    pub harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word: Option<Word>,
}

/// Where a work stands, in one word: the furthest act that has happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Word {
    Asking,
    Asked,
    Evaled,
    Sealed,
    Cancelled,
}

impl Word {
    pub fn of(unit: &Unit) -> Self {
        if unit.cancelled {
            Word::Cancelled
        } else if unit.requested_only {
            // Asked, and its prompt still being composed: under way, not done.
            Word::Asking
        } else if unit.sealed.is_some() {
            Word::Sealed
        } else if unit.check.is_some() {
            Word::Evaled
        } else {
            Word::Asked
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Word::Asking => "word.asking",
            Word::Asked => "word.asked",
            Word::Evaled => "word.evaled",
            Word::Sealed => "word.sealed",
            Word::Cancelled => "word.cancelled",
        }
    }

    pub fn say(self) -> &'static str {
        crate::words::word(self.key())
    }
}

/// Why an act cannot run: a `words` key, and the harness and the work its
/// sentence names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Why {
    pub key: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Why {
    pub fn new(key: &'static str) -> Self {
        Why { key, harness: None, title: None }
    }

    /// A why-not about a work: its sentence may name the work and its harness.
    pub fn about(key: &'static str, unit: &Unit) -> Self {
        Why { key, harness: Some(unit.harness.clone()), title: Some(unit.title.clone()) }
    }

    /// A why-not naming a harness.
    pub fn of(key: &'static str, harness: &str) -> Self {
        Why { key, harness: Some(harness.to_string()), title: None }
    }

    /// Why an act going to this harness cannot run, if it cannot.
    pub fn not_ready(state: Readiness, harness: &str) -> Option<Self> {
        let (key, named) = match state {
            Readiness::Ready => return None,
            Readiness::Missing(Gap::Cli) => ("why.no_ringframe", false),
            Readiness::Missing(Gap::Marketplace | Gap::Plugin) => ("why.not_set_up", true),
            Readiness::Unknown => ("why.cannot_ask", true),
        };
        Some(Why { key, harness: named.then(|| harness.to_string()), title: None })
    }

    pub fn say(&self) -> String {
        let named = |v: &Option<String>| v.clone().unwrap_or_default();
        crate::words::fill(
            self.key,
            &[("harness", &named(&self.harness)), ("title", &named(&self.title))],
        )
    }
}

/// The sentence an act says when the row selected is not one it acts on:
/// Ask on an agent, Eval and Seal on a unit of work.
pub fn select(act: Act) -> Option<&'static str> {
    match act {
        Act::Ask => Some("select.agent"),
        Act::Eval => Some("select.work_eval"),
        Act::Seal => Some("select.work_seal"),
        _ => None,
    }
}

/// The rows of each project.
pub fn rows(projects: &[Facts], folded: &dyn Fn(usize, Fold) -> bool) -> Vec<Project> {
    projects.iter().enumerate().map(|(at, f)| project(f, &|fold| folded(at, fold))).collect()
}

fn project(f: &Facts, folded: &dyn Fn(Fold) -> bool) -> Project {
    let board = |unit: Option<&str>| Board {
        units: f.units,
        selected: unit
            .and_then(|id| f.units.iter().position(|u| u.ask_id == id))
            .unwrap_or(usize::MAX),
        panes: f.panes,
        focused: f.panes.iter().position(|p| p.running).unwrap_or(0),
        readiness: f.readiness,
        routing: f.routing,
        workspace_gap: None,
        agents: f.agents,
    };
    let running = |r: &&crate::sessions::Row| r.state == State::Running;
    let (shown, earlier) = crate::sessions::split(f.sessions);
    let w = f.work;
    let open = w.units.iter().filter(|u| !u.sealed()).count();
    let sealed = w.units.len() - open;
    let listed =
        if folded(Fold::Sealed) { open + sealed.min(SEALED_IN_TREE) } else { w.units.len() };
    let mut out = Project {
        name: f.name.to_string(),
        open,
        needs: f
            .sessions
            .iter()
            .filter(running)
            .filter(|s| turn_now(f, s) == Some(Turn::Waiting))
            .count(),
        ask: board(None).why_not(Act::Ask),
        setup: f.setup,
        folded: folded(Fold::Project),
        agents: Agents {
            running: f.sessions.iter().filter(running).count(),
            ended: f.sessions.iter().filter(|s| s.state != State::Running).count(),
            rows: shown.iter().map(|s| agent(f, s)).collect(),
            earlier: earlier.len(),
        },
        work: Work {
            units: w
                .units
                .iter()
                .take(listed)
                .filter_map(|u| {
                    let root = f.units.iter().find(|a| a.ask_id == u.root)?;
                    let ready = |a: Option<&str>| board(a).not_ready(Act::Eval);
                    let seal_ready = |a: Option<&str>| board(a).not_ready(Act::Seal);
                    Some(WorkRow {
                        unit: u.root.clone(),
                        title: root.title.clone(),
                        harness: root.harness.clone(),
                        status: u.status.clone(),
                        asks: u.asks.len(),
                        eval: crate::work::eval_why(u, root, &ready),
                        seal: crate::work::seal_why(w, u, root, &seal_ready),
                    })
                })
                .collect(),
            earlier: w.units.len() - listed,
        },
    };
    if out.folded {
        out.agents.rows.clear();
        out.work.units.clear();
    }
    out
}

/// A session's turn as the daemon reads it now: in a pane, the pane's (its
/// hook's word, or what Weft read from its screen when the hook said
/// nothing); else its hook's last word.
fn turn_now(f: &Facts, s: &crate::sessions::Row) -> Option<Turn> {
    let at = s.pane.and_then(|p| f.panes.iter().position(|i| i.pane == p));
    at.and_then(|i| f.agents.get(i).copied().flatten()).or(s.turn)
}

/// A session's row, and why an Ask cannot go to it when it cannot.
pub fn agent(f: &Facts, s: &crate::sessions::Row) -> Agent {
    let ask = match (s.state, s.pane) {
        (State::Running, Some(_)) => Why::not_ready((f.readiness)(&s.harness), &s.harness),
        (State::Running, None) => Some(Why::of("why.elsewhere", &s.harness)),
        (State::Ended, _) => Some(Why::of("why.ended", &s.harness)),
        (State::Quiet, _) => Some(Why::of("why.quiet", &s.harness)),
    };
    Agent {
        session: s.id.clone(),
        harness: s.harness.clone(),
        state: s.state,
        turn: turn_now(f, s),
        title: s.title.clone(),
        at: s.last,
        asks: s.asks,
        pane: s.pane,
        ask,
    }
}

/// Whether an agent row holds `q`, ignoring case: its harness, its latest
/// Ask's title, or its session id.
pub fn agent_matches(a: &Agent, q: &str) -> bool {
    let q = q.to_lowercase();
    let has = |text: &str| text.to_lowercase().contains(&q);
    has(&a.harness) || has(&a.session) || a.title.as_deref().is_some_and(has)
}

/// Whether a unit of work's row holds `q`, ignoring case: its title, its
/// harness, or its status.
pub fn work_matches(w: &WorkRow, q: &str) -> bool {
    let q = q.to_lowercase();
    let has = |text: &str| text.to_lowercase().contains(&q);
    has(&w.title) || has(&w.harness) || has(&status_word(&w.status))
}

/// A Seal's decision in the words a person reads (board.md).
pub fn decision(recorded: &str) -> String {
    match recorded {
        "deferred" => "parked".into(),
        "abandoned" => "dropped".into(),
        other => other.into(),
    }
}

/// What an agent row says after its title: its turn, `ended`, or when it
/// was last active.
pub fn state_word(a: &Agent, clock: &str) -> String {
    match (a.state, a.turn) {
        (State::Running, Some(t)) => t.plain().to_string(),
        (State::Running, None) => crate::words::word("state.running").to_string(),
        (State::Ended, _) => crate::words::word("state.ended").to_string(),
        (State::Quiet, _) => crate::words::fill("state.last_active", &[("time", clock)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Sent, Verdict};
    use crate::sessions::Row;
    use crate::work::{Eval, Record, Seal};

    fn pane(id: u32, harness: &str) -> PaneInfo {
        PaneInfo {
            pane: id,
            harness: harness.into(),
            spec: harness.into(),
            running: true,
            turbo: false,
        }
    }

    fn unit(id: &str, title: &str, harness: &str) -> Unit {
        Unit {
            ask_id: id.into(),
            title: title.into(),
            harness: harness.into(),
            confirmed: true,
            sent: Sent::TakenByAgent,
            ..Default::default()
        }
    }

    fn row(id: &str, harness: &str, state: State, turn: Option<Turn>, last: i64) -> Row {
        let mut r = Row::of(harness, id, state, turn, last);
        r.title = Some(format!("{id} title"));
        r
    }

    /// The cards' sample: three running, fifteen ended; fix login asked,
    /// health endpoint drifted, telemetry being judged, and fourteen sealed.
    /// A world: units, panes, their states, the index and the units of work.
    type World = (Vec<Unit>, Vec<PaneInfo>, Vec<Option<Turn>>, Vec<Row>, Units);

    fn at(h: u32, m: u32) -> String {
        format!("2026-10-08T{h:02}:{m:02}:00Z")
    }

    fn eval(id: &str, n: usize, ask: &str, verdict: Option<Verdict>, done: String) -> Eval {
        Eval {
            id: id.into(),
            n,
            opened_at: done.clone(),
            asks: vec![ask.into()],
            verdict,
            agreement: verdict.map(|_| 0.75),
            judged_by: None,
            completed_at: verdict.map(|_| done),
            voided: false,
            superseded: false,
        }
    }

    fn weft() -> World {
        let asked = |id: &str, title: &str, harness: &str, t: String| Unit {
            asked_at: t,
            ..unit(id, title, harness)
        };
        let mut units = vec![
            asked("ask_1", "fix login", "claude", at(11, 29)),
            asked("ask_2", "telemetry", "claude", at(11, 0)),
            asked("ask_3", "health endpoint", "codex", at(10, 52)),
        ];
        let mut record = Record {
            evals: vec![
                eval("evl_1", 1, "ask_3", Some(Verdict::DoesntMatch), at(11, 20)),
                eval("evl_2", 2, "ask_2", None, at(11, 10)),
            ],
            seals: Vec::new(),
        };
        let sealed = |id: &str,
                      title: &str,
                      t: String,
                      decision: &str,
                      units: &mut Vec<Unit>,
                      record: &mut Record| {
            let n = record.seals.len() + 1;
            let seal = format!("sel_{n}");
            units.push(Unit {
                seal_id: Some(seal.clone()),
                sealed: Some(decision.into()),
                ..asked(id, title, "codex", t.clone())
            });
            record.seals.push(Seal {
                id: seal,
                n,
                at: t,
                disposition: decision.into(),
                by: "you".into(),
                eval: None,
            });
        };
        for i in 0..12 {
            sealed(&format!("old_{i}"), "old", at(9, i), "accepted", &mut units, &mut record);
        }
        sealed("ask_5", "readme fix", at(10, 40), "deferred", &mut units, &mut record);
        sealed("ask_4", "build number", at(10, 50), "accepted", &mut units, &mut record);
        let work = crate::work::work(&units, &record);
        let panes = vec![pane(0, "codex"), pane(1, "claude"), pane(2, "codex")];
        let agents = vec![Some(Turn::Working), Some(Turn::Waiting), Some(Turn::TurnEnded)];
        let mut sessions = vec![
            row("c1", "codex", State::Running, Some(Turn::Working), 1131),
            row("k1", "claude", State::Running, Some(Turn::Waiting), 1129),
            row("c2", "codex", State::Running, Some(Turn::TurnEnded), 1052),
        ];
        sessions[0].pane = Some(0);
        sessions[1].pane = Some(1);
        for i in 0..15 {
            sessions.push(row(&format!("e{i:02}"), "claude", State::Ended, None, 900 - i));
        }
        (units, panes, agents, sessions, work)
    }

    const READY: &dyn Fn(&str) -> Readiness = &|_| Readiness::Ready;
    const AS_DRAWN: &dyn Fn(usize, Fold) -> bool = &|_, f| f == Fold::Sealed;

    fn build(
        w: &World,
        ready: &dyn Fn(&str) -> Readiness,
        folded: &dyn Fn(usize, Fold) -> bool,
    ) -> Project {
        let routing = Routing::default();
        let facts = Facts {
            name: "weft",
            units: &w.0,
            panes: &w.1,
            agents: &w.2,
            sessions: &w.3,
            work: &w.4,
            readiness: ready,
            routing: &routing,
            setup: None,
        };
        rows(&[facts], folded).remove(0)
    }

    /// The project row counts open units and agents waiting (D12.3); AGENTS
    /// counts running and ended, shows the running and 3 ended, and counts
    /// the rest.
    #[test]
    fn the_project_and_agents_rows_count_as_t11_1_does() {
        let t = build(&weft(), READY, AS_DRAWN);
        assert_eq!((t.name.as_str(), t.open, t.needs), ("weft", 3, 1));
        let a = &t.agents;
        assert_eq!((a.running, a.ended, a.earlier), (3, 15, 12));
        let ids: Vec<_> = a.rows.iter().map(|r| r.session.as_str()).collect();
        assert_eq!(ids, ["c1", "k1", "c2", "e00", "e01", "e02"]);
        assert_eq!(a.rows[1].turn, Some(Turn::Waiting));
    }

    /// An agent in a pane is shown, and counted, as the daemon reads it now:
    /// its hook's word, or what Weft read from its screen when the hook said
    /// nothing.
    #[test]
    fn an_agent_in_a_pane_is_what_the_daemon_reads_of_it() {
        let mut w = weft();
        // codex's hook said it is working; the daemon read it waiting.
        w.2[0] = Some(Turn::Waiting);
        let t = build(&w, READY, AS_DRAWN);
        assert_eq!(t.agents.rows[0].turn, Some(Turn::Waiting));
        assert_eq!(t.needs, 2, "k1 and c1");
    }

    /// Each agent row says why an Ask cannot go to it: an ended one (D12.4),
    /// one run by another program, a quiet one, one not set up.
    #[test]
    fn an_agent_says_why_an_ask_cannot_go_to_it() {
        let mut w = weft();
        w.3.insert(3, row("q1", "agy", State::Quiet, None, 2000));
        let t = build(&w, READY, AS_DRAWN);
        let why = |id: &str| {
            t.agents
                .rows
                .iter()
                .find(|r| r.session == id)
                .and_then(|r| r.ask.as_ref())
                .map(Why::say)
        };
        assert_eq!(why("c1"), None, "in a pane, set up");
        assert_eq!(
            why("c2").as_deref(),
            Some("codex runs in another program: Weft can't show it or type into it.")
        );
        assert_eq!(why("e00").as_deref(), Some("claude has ended: resume this session to Ask it."));
        assert_eq!(
            why("q1").as_deref(),
            Some("Weft can't tell whether this agy session runs: resume it to Ask it.")
        );
        let unset =
            |h: &str| if h == "codex" { Readiness::Missing(Gap::Plugin) } else { Readiness::Ready };
        let t = build(&weft(), &unset, AS_DRAWN);
        assert_eq!(
            t.agents.rows[0].ask.as_ref().map(Why::say).as_deref(),
            Some("codex is not set up for RingFrame.")
        );
    }

    /// WORK: a row per unit, open ones first, its title, harness and one
    /// status; the newest sealed, then a count of the rest until opened.
    #[test]
    fn work_is_a_row_per_unit_with_one_status() {
        let t = build(&weft(), READY, AS_DRAWN);
        let rows: Vec<_> = t
            .work
            .units
            .iter()
            .map(|u| (u.title.as_str(), u.harness.as_str(), status_word(&u.status)))
            .collect();
        assert_eq!(
            rows,
            [
                ("fix login", "claude", "asked".to_string()),
                ("health endpoint", "codex", "drifted · 0.75".to_string()),
                ("telemetry", "claude", "judging…".to_string()),
                ("build number", "codex", "sealed · accepted".to_string()),
                ("readme fix", "codex", "sealed · parked".to_string()),
                ("old", "codex", "sealed · accepted".to_string()),
            ]
        );
        assert_eq!((t.open, t.work.earlier), (3, 11));
        let t = build(&weft(), READY, &|_, _| false);
        assert_eq!((t.work.units.len(), t.work.earlier), (17, 0));
    }

    #[test]
    fn a_folded_project_keeps_its_counts() {
        let t = build(&weft(), READY, &|_, f| f == Fold::Project);
        assert!(t.folded && t.agents.rows.is_empty() && t.work.units.is_empty());
        assert_eq!((t.open, t.needs, t.agents.running), (3, 1, 3), "a folded project still counts");
    }

    /// Eval and Seal on a unit say why not: by the harness they go to, or
    /// because it is being judged, not judged yet, or sealed.
    #[test]
    fn eval_and_seal_on_a_unit_say_why_not() {
        let unset = |h: &str| {
            if h == "claude" { Readiness::Missing(Gap::Plugin) } else { Readiness::Ready }
        };
        let t = build(&weft(), &unset, AS_DRAWN);
        let says: Vec<_> = t
            .work
            .units
            .iter()
            .take(4)
            .map(|u| (u.eval.as_ref().map(Why::say), u.seal.as_ref().map(Why::say)))
            .collect();
        let no = "claude is not set up for RingFrame.".to_string();
        assert_eq!(
            says,
            [
                (Some(no.clone()), Some("No Eval of this work yet: Eval starts one.".into())),
                (None, None),
                (Some(no), Some("telemetry is still being judged.".into())),
                (
                    Some("build number is closed. Follow up asks again.".into()),
                    Some("build number is closed. Follow up asks again.".into())
                ),
            ]
        );
    }

    #[test]
    fn a_press_on_the_wrong_row_says_what_to_select() {
        let say = |a| select(a).map(crate::words::word);
        assert_eq!(say(Act::Ask), Some("Select an agent to Ask."));
        assert_eq!(say(Act::Eval), Some("Select a work to Eval it."));
        assert_eq!(say(Act::Seal), Some("Select a work to Seal it."));
        assert_eq!(select(Act::Proceed), None);
    }

    #[test]
    fn an_agents_state_reads_as_its_turn_ended_or_last_active() {
        let t = build(&weft(), READY, AS_DRAWN);
        let words: Vec<_> = t.agents.rows.iter().take(4).map(|a| state_word(a, "11:25")).collect();
        assert_eq!(words, ["working", "needs you", "turn ended", "ended"]);
        let quiet = Agent { state: State::Quiet, ..t.agents.rows[3].clone() };
        assert_eq!(state_word(&quiet, "11:25"), "last active 11:25");
    }

    /// remote.md §6: the names on the wire, folded content left out.
    #[test]
    fn the_wire_names_the_fields_as_the_protocol_does() {
        let t = build(&weft(), &|_| Readiness::Missing(Gap::Cli), AS_DRAWN);
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["project"], "weft");
        assert_eq!((v["open"].clone(), v["needs"].clone()), (3.into(), 1.into()));
        assert_eq!(v["agents"]["running"], 3);
        assert_eq!(v["agents"]["earlier"], 12);
        assert_eq!(
            v["agents"]["rows"][1],
            serde_json::json!({"session": "k1", "harness": "claude", "state": "running",
                "turn": "waiting", "title": "k1 title", "at": 1129, "asks": 0,
                "ask": {"key": "why.no_ringframe"}})
        );
        assert_eq!(
            v["work"]["units"][1],
            serde_json::json!({"unit": "ask_3", "title": "health endpoint", "harness": "codex",
                "status": {"state": "judged", "verdict": "drifted", "agreement": 0.75},
                "asks": 1, "eval": {"key": "why.no_ringframe"}, "seal": {"key": "why.no_ringframe"}})
        );
        assert_eq!(v["work"]["earlier"], 11);
        assert!(v.get("harnesses").is_none());
    }
}
