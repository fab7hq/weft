//! The tree both clients draw: project, then harness, then work.
//!
//! Built from the board and the agents, and nothing else, so the TUI's list
//! and a device's tree are one set of rows. Folding is the client's own; it
//! is asked, never kept here.

use serde::Serialize;

use crate::board::{Act, Board, PaneInfo};
use crate::ledger::Unit;
use crate::readiness::{Gap, Readiness};
use crate::routing::Routing;
use crate::turns::{Session, Turn};

/// What one project's rows are built from. Borrowed: the client owns it.
pub struct Facts<'a> {
    pub name: &'a str,
    pub units: &'a [Unit],
    pub panes: &'a [PaneInfo],
    /// Each pane's agent state, in the order of `panes`.
    pub agents: &'a [Option<Turn>],
    /// The sessions the hooks reported, for a work's own agent state.
    pub sessions: &'a [Session],
    /// A harness's title, by its harness file, when it has one.
    pub title: &'a dyn Fn(&str) -> Option<String>,
    pub readiness: &'a dyn Fn(&str) -> Readiness,
    pub routing: &'a Routing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    #[serde(rename = "project")]
    pub name: String,
    /// Agents running.
    pub open: usize,
    /// Agents asking their person for input.
    pub needs: usize,
    #[serde(skip)]
    pub folded: bool,
    pub harnesses: Vec<Harness>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Harness {
    pub harness: String,
    /// The pane a tap on the harness opens: one asking for input first.
    #[serde(skip)]
    pub pane: Option<u32>,
    pub title: Option<String>,
    /// That agent's state, while it runs.
    pub turn: Option<TurnWord>,
    pub turbo: bool,
    pub running: bool,
    /// Agents of it asking for input.
    #[serde(skip)]
    pub needs: usize,
    #[serde(skip)]
    pub folded: bool,
    pub works: Vec<Work>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Work {
    /// The Ask's id.
    pub unit: String,
    /// Where it is in the project's board.
    #[serde(skip)]
    pub at: usize,
    pub title: String,
    pub word: Word,
    pub turn: Option<TurnWord>,
    /// `None` when Eval can run on it, else why not.
    pub eval: Option<Why>,
    pub seal: Option<Why>,
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

/// An agent's turn under the names a device reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnWord {
    Ready,
    Working,
    Waiting,
    Ended,
}

impl From<Turn> for TurnWord {
    fn from(t: Turn) -> Self {
        match t {
            Turn::Ready => TurnWord::Ready,
            Turn::Working => TurnWord::Working,
            Turn::Waiting => TurnWord::Waiting,
            Turn::TurnEnded => TurnWord::Ended,
        }
    }
}

impl TurnWord {
    pub fn key(self) -> &'static str {
        match self {
            TurnWord::Ready => "turn.ready",
            TurnWord::Working => "turn.working",
            TurnWord::Waiting => "turn.waiting",
            TurnWord::Ended => "turn.ended",
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

/// The rows of each project. `folded` is asked with a project's index and,
/// for a harness, its name.
pub fn rows(projects: &[Facts], folded: &dyn Fn(usize, Option<&str>) -> bool) -> Vec<Project> {
    projects.iter().enumerate().map(|(at, f)| project(at, f, folded)).collect()
}

fn project(at: usize, f: &Facts, folded: &dyn Fn(usize, Option<&str>) -> bool) -> Project {
    let mut out = Project {
        name: f.name.to_string(),
        open: crate::turns::open(f.panes),
        needs: crate::turns::asking(f.panes, f.agents, None),
        folded: folded(at, None),
        harnesses: Vec::new(),
    };
    if out.folded {
        return out;
    }
    // Harnesses by the name a person reads, A to Z: the tree does not
    // reshuffle itself as work arrives. An agent with nothing asked of it yet
    // still has a row, so there is somewhere to say it needs your input.
    let mut seen: Vec<&str> = Vec::new();
    let running = f.panes.iter().map(|p| p.harness.as_str());
    for name in f.units.iter().map(|u| u.harness.as_str()).chain(running) {
        if !seen.contains(&name) {
            seen.push(name);
        }
    }
    let read_as = |name: &str| (f.title)(name).unwrap_or_else(|| name.to_string()).to_lowercase();
    seen.sort_by_key(|name| read_as(name));
    for name in seen {
        let mine = || f.panes.iter().zip(f.agents).filter(|(p, _)| p.running && p.harness == name);
        let running = mine().next().is_some();
        // Nothing to go back to: no agent, and nothing left open.
        if !running && !f.units.iter().any(|u| u.harness == name && u.is_open()) {
            continue;
        }
        let agent = mine().find(|(_, t)| **t == Some(Turn::Waiting)).or_else(|| mine().next());
        let folded = folded(at, Some(name));
        let works = if folded {
            Vec::new()
        } else {
            f.units
                .iter()
                .enumerate()
                .filter(|(_, u)| u.harness == name)
                .map(|(i, u)| work(f, i, u))
                .collect()
        };
        out.harnesses.push(Harness {
            harness: name.to_string(),
            pane: agent.map(|(p, _)| p.pane),
            title: (f.title)(name),
            turn: agent.and_then(|(_, t)| *t).map(TurnWord::from),
            turbo: mine().any(|(p, _)| p.turbo),
            running,
            needs: crate::turns::asking(f.panes, f.agents, Some(name)),
            folded,
            works,
        });
    }
    out
}

fn work(f: &Facts, at: usize, unit: &Unit) -> Work {
    let board = Board {
        units: f.units,
        selected: at,
        panes: f.panes,
        focused: 0,
        readiness: f.readiness,
        routing: f.routing,
        workspace_gap: None,
        agents: f.agents,
    };
    Work {
        unit: unit.ask_id.clone(),
        at,
        title: unit.title.clone(),
        word: Word::of(unit),
        turn: crate::turns::state_of(unit, f.sessions).map(TurnWord::from),
        eval: board.not_ready(Act::Eval),
        seal: board.not_ready(Act::Seal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{Check, Sent, Verdict};

    fn pane(id: u32, harness: &str, running: bool, turbo: bool) -> PaneInfo {
        PaneInfo { pane: id, harness: harness.into(), spec: harness.into(), running, turbo }
    }

    fn unit(id: &str, title: &str, harness: &str) -> Unit {
        Unit {
            ask_id: id.into(),
            title: title.into(),
            harness: harness.into(),
            session_ref: Some(format!("s_{id}")),
            confirmed: true,
            sent: Sent::TakenByAgent,
            ..Default::default()
        }
    }

    fn checked(mut u: Unit) -> Unit {
        u.check = Some(Check {
            eval_id: "evl_1".into(),
            verdict: Verdict::Matches,
            agreement: 1.0,
            judged_by: None,
        });
        u
    }

    fn session(id: &str, harness: &str, latest: Turn) -> Session {
        Session { harness: harness.into(), id: format!("s_{id}"), first: 0, latest, at: 0 }
    }

    /// The Atlas's sample: weft has fix login and telemetry of claude,
    /// health endpoint and readme fix of codex.
    fn weft() -> (Vec<Unit>, Vec<PaneInfo>, Vec<Option<Turn>>, Vec<Session>) {
        let mut telemetry = unit("ask_2", "telemetry", "claude");
        telemetry.requested_only = true;
        let mut readme = checked(unit("ask_4", "readme fix", "codex"));
        readme.sealed = Some("accepted".into());
        let mut old = unit("ask_5", "old idea", "codex");
        old.cancelled = true;
        let units = vec![
            unit("ask_1", "fix login", "claude"),
            telemetry,
            checked(unit("ask_3", "health endpoint", "codex")),
            readme,
            old,
        ];
        let panes = vec![pane(0, "claude", true, true), pane(1, "codex", true, false)];
        let agents = vec![Some(Turn::Waiting), Some(Turn::TurnEnded)];
        let sessions = vec![
            session("ask_1", "claude", Turn::Waiting),
            session("ask_3", "codex", Turn::TurnEnded),
        ];
        (units, panes, agents, sessions)
    }

    fn build(
        units: &[Unit],
        panes: &[PaneInfo],
        agents: &[Option<Turn>],
        sessions: &[Session],
        ready: &dyn Fn(&str) -> Readiness,
        folded: &dyn Fn(usize, Option<&str>) -> bool,
    ) -> Project {
        let title = |h: &str| (h == "claude").then(|| "Claude Code".to_string());
        let routing = Routing::default();
        let facts = Facts {
            name: "weft",
            units,
            panes,
            agents,
            sessions,
            title: &title,
            readiness: ready,
            routing: &routing,
        };
        rows(&[facts], folded).remove(0)
    }

    const READY: &dyn Fn(&str) -> Readiness = &|_| Readiness::Ready;
    const OPEN: &dyn Fn(usize, Option<&str>) -> bool = &|_, _| false;

    #[test]
    fn a_project_counts_its_agents_and_those_that_need_you() {
        let (u, p, a, s) = weft();
        let t = build(&u, &p, &a, &s, READY, OPEN);
        assert_eq!((t.name.as_str(), t.open, t.needs), ("weft", 2, 1));
        let names: Vec<_> = t.harnesses.iter().map(|h| h.harness.as_str()).collect();
        assert_eq!(names, ["claude", "codex"]);
    }

    #[test]
    fn every_work_has_its_word_and_its_agents_turn() {
        let (u, p, a, s) = weft();
        let t = build(&u, &p, &a, &s, READY, OPEN);
        let works: Vec<_> = t
            .harnesses
            .iter()
            .flat_map(|h| &h.works)
            .map(|w| (w.title.as_str(), w.word.say(), w.turn.map(TurnWord::say)))
            .collect();
        assert_eq!(
            works,
            [
                ("fix login", "ASKED", Some("waiting for you")),
                ("telemetry", "ASKING", None),
                ("health endpoint", "EVALED", Some("turn ended")),
                ("readme fix", "SEALED", None),
                ("old idea", "CANCELLED", None),
            ]
        );
    }

    #[test]
    fn a_harness_opens_the_agent_that_needs_you_and_says_turbo() {
        let (u, _, _, s) = weft();
        let p = [
            pane(2, "claude", true, false),
            pane(1, "codex", true, false),
            pane(0, "claude", true, true),
        ];
        let a = [Some(Turn::Working), Some(Turn::TurnEnded), Some(Turn::Waiting)];
        let t = build(&u, &p, &a, &s, READY, OPEN);
        let claude = &t.harnesses[0];
        assert_eq!(claude.pane, Some(0), "the claude agent asking for input");
        assert_eq!(claude.turn, Some(TurnWord::Waiting));
        assert!(claude.turbo, "an agent of it runs in turbo mode");
        assert!(!t.harnesses[1].turbo);
        assert_eq!(claude.needs, 1);
    }

    #[test]
    fn a_harness_not_running_keeps_its_open_work_and_drops_once_nothing_is_open() {
        let (u, mut p, a, s) = weft();
        p[1].running = false;
        let t = build(&u, &p, &a, &s, READY, OPEN);
        let codex = &t.harnesses[1];
        assert!(!codex.running);
        assert_eq!((codex.pane, codex.turn), (None, None));
        assert_eq!(t.open, 1);
        let closed: Vec<Unit> = u.into_iter().filter(|u| u.harness == "claude").collect();
        let t = build(&closed, &p, &a, &s, READY, OPEN);
        assert_eq!(t.harnesses.len(), 1, "codex has no agent and nothing open");
    }

    #[test]
    fn a_folded_project_or_harness_keeps_its_row_and_drops_what_is_under_it() {
        let (u, p, a, s) = weft();
        let t = build(&u, &p, &a, &s, READY, &|_, h| h.is_none());
        assert!(t.folded && t.harnesses.is_empty());
        assert_eq!(t.needs, 1, "a folded project still counts");
        let t = build(&u, &p, &a, &s, READY, &|_, h| h == Some("codex"));
        assert!(t.harnesses[1].folded && t.harnesses[1].works.is_empty());
        assert_eq!(t.harnesses[0].works.len(), 2);
    }

    #[test]
    fn eval_and_seal_say_why_not_by_the_harness_they_go_to() {
        let (u, p, a, s) = weft();
        let t = build(&u, &p, &a, &s, READY, OPEN);
        assert!(t.harnesses.iter().flat_map(|h| &h.works).all(|w| w.eval.is_none()));
        for (state, key, named) in [
            (Readiness::Missing(Gap::Plugin), "why.not_set_up", true),
            (Readiness::Missing(Gap::Marketplace), "why.not_set_up", true),
            (Readiness::Missing(Gap::Cli), "why.no_ringframe", false),
            (Readiness::Unknown, "why.cannot_ask", true),
        ] {
            let codex_only = move |h: &str| if h == "codex" { state } else { Readiness::Ready };
            let t = build(&u, &p, &a, &s, &codex_only, OPEN);
            assert_eq!(t.harnesses[0].works[0].eval, None, "claude's work is ready");
            let w = &t.harnesses[1].works[0];
            let want = Why { key, harness: named.then(|| "codex".to_string()), title: None };
            assert_eq!((w.eval.as_ref(), w.seal.as_ref()), (Some(&want), Some(&want)));
        }
        let w = &build(&u, &p, &a, &s, &|_| Readiness::Missing(Gap::Plugin), OPEN).harnesses[1]
            .works[0];
        assert_eq!(
            w.eval.as_ref().map(Why::say).as_deref(),
            Some("codex is not set up for RingFrame.")
        );
    }

    #[test]
    fn the_wire_names_the_fields_as_the_protocol_does() {
        let (u, p, a, s) = weft();
        let t = build(&u, &p, &a, &s, &|_| Readiness::Missing(Gap::Cli), OPEN);
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["project"], "weft");
        let claude = &v["harnesses"][0];
        assert_eq!(claude["title"], "Claude Code");
        assert_eq!(claude["turn"], "waiting");
        assert_eq!(
            claude["works"][0],
            serde_json::json!({"unit": "ask_1", "title": "fix login", "word": "asked",
                "turn": "waiting", "eval": {"key": "why.no_ringframe"},
                "seal": {"key": "why.no_ringframe"}})
        );
        assert!(claude.get("pane").is_none() && claude.get("folded").is_none());
    }
}
