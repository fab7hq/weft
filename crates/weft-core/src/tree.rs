//! The tree both clients draw: a project, then AGENTS and WORK.
//!
//! AGENTS is the session index weftd keeps (`sessions.md`); WORK is the record
//! by Eval round (`board.md` "Rounds"). Built from those and nothing else, so
//! the desktop's list and a device's tree are one set of rows. Folding Open or an
//! Eval is the client's own, asked here and never kept; Sealed and the
//! earlier sessions are counts (remote.md §6).

use serde::Serialize;

use crate::board::{Act, Board, PaneInfo};
use crate::ledger::Unit;
use crate::readiness::{Gap, Readiness};
use crate::rounds::Rounds;
use crate::routing::Routing;
use crate::sessions::State;
use crate::turns::Turn;
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
    /// The board by Eval round, as weftd built it.
    pub rounds: &'a Rounds,
    pub readiness: &'a dyn Fn(&str) -> Readiness,
    pub routing: &'a Routing,
    /// What the project still needs before RingFrame is set up in it, if
    /// it is not.
    pub setup: Option<Needs>,
}

/// What a client folds. Asked, never kept here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fold<'a> {
    Project,
    Open,
    Eval(&'a str),
    /// Sealed is folded unless the client has opened it.
    Sealed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Project {
    #[serde(rename = "project")]
    pub name: String,
    /// The Open round's Asks.
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Work {
    pub open: Open,
    /// An Eval being judged first, then each one not sealed, newest first.
    pub evals: Vec<Group>,
    pub sealed: Sealed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Open {
    /// The newest completed Eval's number: `Open · since Eval n`.
    pub since: Option<usize>,
    pub asks: Vec<Ask>,
    /// `None` when Eval can run on the Open round, else why not.
    pub eval: Option<Why>,
    #[serde(skip)]
    pub folded: bool,
    /// How many Asks it holds, folded or not.
    #[serde(skip)]
    pub count: usize,
}

/// One Eval's round.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub eval: String,
    pub n: usize,
    pub judging: bool,
    /// As recorded: `aligned`, `drifted` or `incomplete`.
    pub verdict: Option<&'static str>,
    /// As the record writes it: `0.67`, `1.0`.
    pub agreement: Option<String>,
    pub asks: Vec<Ask>,
    /// `None` when Seal can run on it, else why not.
    pub seal: Option<Why>,
    #[serde(skip)]
    pub folded: bool,
    #[serde(skip)]
    pub count: usize,
}

/// Sealed, folded: counts on the wire; the desktop lists its Seals once opened.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sealed {
    pub seals: usize,
    pub asks: usize,
    #[serde(skip)]
    pub folded: bool,
    #[serde(skip)]
    pub rows: Vec<SealRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SealRow {
    pub seal: String,
    pub n: usize,
    /// The decision as a person reads it: accepted, rejected, parked, dropped.
    pub decision: String,
    /// The Eval it seals, by its number.
    pub eval: Option<usize>,
    pub asks: usize,
}

/// An Ask in a round. Only Open's carry a state word.
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
/// Ask on an agent, Eval on Open, Seal on an Eval.
pub fn select(act: Act) -> Option<&'static str> {
    match act {
        Act::Ask => Some("select.agent"),
        Act::Eval => Some("select.open"),
        Act::Seal => Some("select.eval"),
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
    let r = f.rounds;
    let mut out = Project {
        name: f.name.to_string(),
        open: r.open.len(),
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
            open: Open {
                since: r.since,
                asks: Vec::new(),
                eval: crate::rounds::eval_why(r, &|u| board(u).not_ready(Act::Eval)),
                folded: folded(Fold::Open),
                count: r.open.len(),
            },
            evals: Vec::new(),
            sealed: Sealed {
                seals: r.seals.len(),
                asks: r.sealed_asks(),
                folded: folded(Fold::Sealed),
                rows: Vec::new(),
            },
        },
    };
    let asks = |ids: &[String], worded: bool| -> Vec<Ask> {
        ids.iter()
            .filter_map(|id| f.units.iter().find(|u| &u.ask_id == id))
            .map(|u| Ask {
                unit: u.ask_id.clone(),
                title: u.title.clone(),
                harness: u.harness.clone(),
                word: worded.then(|| Word::of(u)),
            })
            .collect()
    };
    if !out.work.open.folded {
        out.work.open.asks = asks(&r.open, true);
    }
    for round in r.judging.iter().chain(&r.evals) {
        let folded = folded(Fold::Eval(&round.eval));
        out.work.evals.push(Group {
            eval: round.eval.clone(),
            n: round.n,
            judging: round.judging,
            verdict: round.verdict.map(|v| v.recorded()),
            agreement: round.agreement.map(crate::ledger::score),
            asks: if folded { Vec::new() } else { asks(&round.asks, false) },
            seal: crate::rounds::seal_why(round, &|u| board(u).not_ready(Act::Seal)),
            folded,
            count: round.asks.len(),
        });
    }
    if !out.work.sealed.folded {
        out.work.sealed.rows = r
            .seals
            .iter()
            .map(|s| SealRow {
                seal: s.seal.clone(),
                n: s.n,
                decision: decision(&s.decision),
                eval: s.eval.as_ref().map(|e| e.n),
                asks: s.asks.len(),
            })
            .collect();
    }
    if out.folded {
        out.agents.rows.clear();
        out.work.open.asks.clear();
        out.work.evals.iter_mut().for_each(|g| g.asks.clear());
        out.work.sealed.rows.clear();
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

/// Whether an Ask row holds `q`, ignoring case: its title, its harness, or the
/// word for where it stands.
pub fn ask_matches(a: &Ask, q: &str) -> bool {
    let q = q.to_lowercase();
    let has = |text: &str| text.to_lowercase().contains(&q);
    has(&a.title) || has(&a.harness) || a.word.is_some_and(|w| has(w.say()))
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
    use crate::ledger::{Check, Sent, Verdict};
    use crate::rounds::{Named, Round, Sealed as SealedRound};
    use crate::sessions::Row;

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

    /// T11.1's sample: three running, eighteen ended; Open with two Asks,
    /// Eval 3 with two, four Seals over thirty-one Asks.
    /// A world: units, panes, their states, the index and the rounds.
    type World = (Vec<Unit>, Vec<PaneInfo>, Vec<Option<Turn>>, Vec<Row>, Rounds);

    fn weft() -> World {
        let mut telemetry = unit("ask_2", "telemetry", "claude");
        telemetry.requested_only = true;
        let check = Check {
            eval_id: "evl_3".into(),
            verdict: Verdict::Matches,
            agreement: 0.67,
            judged_by: None,
        };
        let mut health = unit("ask_3", "health endpoint", "codex");
        health.check = Some(check.clone());
        let mut build = unit("ask_4", "build number", "codex");
        build.check = Some(check);
        let units = vec![unit("ask_1", "fix login", "claude"), telemetry, health, build];
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
        let mut seals = Vec::new();
        for (n, asks) in [(4, 2), (3, 9), (2, 11), (1, 9)] {
            seals.push(SealedRound {
                seal: format!("sel_{n}"),
                n,
                decision: if n == 2 { "deferred".into() } else { "accepted".into() },
                at: String::new(),
                by: "you".into(),
                eval: (n > 2).then(|| Named { id: format!("evl_{}", n - 2), n: n - 2 }),
                asks: (0..asks).map(|i| format!("old_{n}_{i}")).collect(),
            });
        }
        let rounds = Rounds {
            judging: Vec::new(),
            open: vec!["ask_1".into(), "ask_2".into()],
            since: Some(3),
            evals: vec![Round {
                eval: "evl_3".into(),
                n: 3,
                judging: false,
                verdict: Some(Verdict::Matches),
                agreement: Some(0.67),
                asks: vec!["ask_3".into(), "ask_4".into()],
                sealed: None,
            }],
            seals,
            record: Default::default(),
        };
        (units, panes, agents, sessions, rounds)
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
            rounds: &w.4,
            readiness: ready,
            routing: &routing,
            setup: None,
        };
        rows(&[facts], folded).remove(0)
    }

    /// The project row counts Open Asks and agents waiting (D12.3); AGENTS
    /// counts running and ended, shows the running and 3 ended, and counts
    /// the rest.
    #[test]
    fn the_project_and_agents_rows_count_as_t11_1_does() {
        let t = build(&weft(), READY, AS_DRAWN);
        assert_eq!((t.name.as_str(), t.open, t.needs), ("weft", 2, 1));
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

    /// WORK: Open with its words, each Eval with title and harness only,
    /// Sealed as counts until opened.
    #[test]
    fn work_is_the_rounds_with_words_only_under_open() {
        let t = build(&weft(), READY, AS_DRAWN);
        let open: Vec<_> =
            t.work.open.asks.iter().map(|a| (a.title.as_str(), a.word.map(Word::say))).collect();
        assert_eq!(open, [("fix login", Some("ASKED")), ("telemetry", Some("ASKING"))]);
        assert_eq!((t.work.open.since, t.work.open.eval.as_ref()), (Some(3), None));
        let g = &t.work.evals[0];
        assert_eq!(
            (g.n, g.verdict, g.agreement.as_deref(), g.seal.as_ref()),
            (3, Some("aligned"), Some("0.67"), None)
        );
        assert!(g.asks.iter().all(|a| a.word.is_none()), "the round says their state");
        assert_eq!(
            g.asks.iter().map(|a| a.title.as_str()).collect::<Vec<_>>(),
            ["health endpoint", "build number"]
        );
        let s = &t.work.sealed;
        assert_eq!((s.seals, s.asks, s.rows.len()), (4, 31, 0));
        let t = build(&weft(), READY, &|_, _| false);
        let rows: Vec<_> =
            t.work.sealed.rows.iter().map(|r| (r.n, r.decision.as_str(), r.eval, r.asks)).collect();
        assert_eq!(
            rows[..3],
            [(4, "accepted", Some(2), 2), (3, "accepted", Some(1), 9), (2, "parked", None, 11)]
        );
    }

    #[test]
    fn a_folded_project_or_group_keeps_its_counts() {
        let w = weft();
        let t = build(&w, READY, &|_, f| f == Fold::Project);
        assert!(t.folded && t.agents.rows.is_empty() && t.work.open.asks.is_empty());
        assert_eq!((t.open, t.needs, t.agents.running), (2, 1, 3), "a folded project still counts");
        let t = build(&w, READY, &|_, f| matches!(f, Fold::Open | Fold::Eval("evl_3")));
        assert!(t.work.open.folded && t.work.open.asks.is_empty());
        assert_eq!((t.work.open.count, t.work.evals[0].count), (2, 2));
        assert!(t.work.evals[0].asks.is_empty());
    }

    /// Eval and Seal on a group say why not by the harness they go to, and a
    /// sealed or judging Eval says so (D12.6, D12.1).
    #[test]
    fn eval_and_seal_on_a_round_say_why_not() {
        let unset = |h: &str| {
            if h == "claude" { Readiness::Missing(Gap::Plugin) } else { Readiness::Ready }
        };
        let t = build(&weft(), &unset, AS_DRAWN);
        assert_eq!(
            t.work.open.eval.as_ref().map(Why::say).as_deref(),
            Some("claude is not set up for RingFrame.")
        );
        assert_eq!(t.work.evals[0].seal, None, "Eval 3's Asks are codex's");
        let mut w = weft();
        w.4.judging.push(Round {
            eval: "evl_4".into(),
            n: 4,
            judging: true,
            verdict: None,
            agreement: None,
            asks: vec!["ask_1".into()],
            sealed: None,
        });
        w.4.evals[0].sealed = Some(Named { id: "sel_5".into(), n: 5 });
        let t = build(&w, READY, AS_DRAWN);
        let says: Vec<_> =
            t.work.evals.iter().map(|g| (g.n, g.judging, g.seal.as_ref().map(Why::say))).collect();
        assert_eq!(
            says,
            [
                (4, true, Some("Eval 4 is still being judged.".into())),
                (3, false, Some("Eval 3 is sealed already.".into()))
            ]
        );
    }

    #[test]
    fn a_press_on_the_wrong_row_says_what_to_select() {
        let say = |a| select(a).map(crate::words::word);
        assert_eq!(say(Act::Ask), Some("Select an agent to Ask."));
        assert_eq!(say(Act::Eval), Some("Select Open to Eval its Asks."));
        assert_eq!(say(Act::Seal), Some("Select an Eval to Seal it."));
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
        assert_eq!((v["open"].clone(), v["needs"].clone()), (2.into(), 1.into()));
        assert_eq!(v["agents"]["running"], 3);
        assert_eq!(v["agents"]["earlier"], 12);
        assert_eq!(
            v["agents"]["rows"][1],
            serde_json::json!({"session": "k1", "harness": "claude", "state": "running",
                "turn": "waiting", "title": "k1 title", "at": 1129, "asks": 0,
                "ask": {"key": "why.no_ringframe"}})
        );
        assert_eq!(
            v["work"]["open"]["asks"][1],
            serde_json::json!({"unit": "ask_2", "title": "telemetry", "harness": "claude", "word": "asking"})
        );
        assert_eq!(
            v["work"]["evals"][0]["asks"][0],
            serde_json::json!({"unit": "ask_3", "title": "health endpoint", "harness": "codex"})
        );
        assert_eq!(v["work"]["sealed"], serde_json::json!({"seals": 4, "asks": 31}));
        assert!(v.get("harnesses").is_none());
    }
}
