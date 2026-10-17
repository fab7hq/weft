//! The agents on this machine, as a device sees them (remote.md §5, §6): one
//! list across every open project, each agent named by an id that stays the
//! same for as long as its pane lives, and its turn as the daemon reported
//! it. Nothing here works out NEEDS YOU: `waiting` is the daemon's word.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use weft_core::board::{Act, Board, PaneInfo};
use weft_core::harness::Harnesses;
use weft_core::ledger::Unit;
use weft_core::readiness::Readiness;
use weft_core::routing::Routing;
use weft_core::turns::Turn;
use weft_core::workspace::Needs;
use weft_proto::{Call, Event};

/// Why a device's message was refused: a code, and a sentence.
pub type Refusal = (&'static str, String);

/// The first 12 hex digits of the SHA-256 of a project's path, as Weft's
/// own receipts key a project.
pub use weft_core::sessions::project_key;

/// An agent's id on the remote wire: `3f9a1c07b2de-4`.
pub fn agent_id(key: &str, pane: u32) -> String {
    format!("{key}-{pane}")
}

/// One project's agents, kept from its `project.open` answer and events.
pub struct Project {
    pub key: String,
    /// The folder's name, which is what the person calls it.
    pub name: String,
    panes: Vec<PaneInfo>,
    turns: Vec<Option<Turn>>,
    /// Each harness's title, by id.
    titles: HashMap<String, String>,
    /// Each pane's turn as last listed, and since when (milliseconds).
    since: HashMap<u32, (&'static str, i64)>,
    /// The board: the tree, and the works a device acts on.
    units: Vec<Unit>,
    /// The board grouped by Eval round, as the daemon built it.
    rounds: weft_core::rounds::Rounds,
    /// The Eval record behind each check, as the daemon read it.
    records: Value,
    /// The project's session index, and the Mac's distance from
    /// UTC, in milliseconds.
    sessions: Vec<weft_core::sessions::Row>,
    zone: i64,
    /// What each harness is short of, by the daemon's word.
    readiness: Value,
    routing: Routing,
    /// The harnesses the person can start here, fresh, and each one's
    /// command line, as `agents.available` last answered.
    starts: Vec<(String, String)>,
    /// The harness files, as the daemon read them: how to resume a session.
    files: Harnesses,
    /// What the project still needs before RingFrame is set up in it, if it
    /// is not.
    setup: Option<Needs>,
    /// Whether agents started here from now on run in turbo mode.
    turbo: bool,
    /// Reopened after weftd ended uncleanly, at this time,
    /// until someone acts in it.
    reopened: Option<i64>,
}

impl Project {
    /// From what `project.open` answered for `path`.
    pub fn opened(path: &Path, answer: &Value) -> Self {
        let titles = answer["harnesses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| {
                Some((h["name"].as_str()?.to_string(), h["title"].as_str()?.to_string()))
            })
            .collect();
        let mut p = Project {
            key: project_key(path),
            name: path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            ),
            panes: weft_proto::panes_of(&answer["panes"]).unwrap_or_default(),
            turns: serde_json::from_value(answer["agents"]["panes"].clone()).unwrap_or_default(),
            titles,
            since: HashMap::new(),
            units: serde_json::from_value(answer["units"].clone()).unwrap_or_default(),
            rounds: serde_json::from_value(answer["rounds"].clone()).unwrap_or_default(),
            records: answer["records"].clone(),
            sessions: serde_json::from_value(answer["agents"]["sessions"].clone())
                .unwrap_or_default(),
            zone: answer["agents"]["zone"].as_i64().unwrap_or(0),
            readiness: answer["readiness"].clone(),
            routing: Routing::opened(&answer["routing"]),
            starts: Vec::new(),
            files: serde_json::from_value(answer["harnesses"].clone()).unwrap_or_default(),
            setup: serde_json::from_value(answer["setup"].clone()).unwrap_or_default(),
            turbo: answer["turbo"].as_bool().unwrap_or(false),
            reopened: answer["reopened"].as_i64(),
        };
        p.stamp(now());
        p
    }

    /// Take an event; whether the list a device sees changed.
    pub fn heard(&mut self, event: &Event) -> bool {
        match event {
            Event::Panes { panes } => self.panes = panes.clone(),
            Event::Added { pane, harness, spec, turbo } => {
                if self.panes.iter().any(|p| p.pane == *pane) {
                    return false;
                }
                self.panes.push(PaneInfo {
                    pane: *pane,
                    harness: harness.clone(),
                    spec: spec.clone(),
                    running: true,
                    turbo: *turbo,
                });
            }
            Event::Exited { pane } => match self.panes.iter_mut().find(|p| p.pane == *pane) {
                Some(p) => p.running = false,
                None => return false,
            },
            Event::Agents { panes, sessions, zone } => {
                self.zone = *zone;
                self.turns = panes.clone();
                self.sessions = sessions.clone();
            }
            Event::Units { units, records, rounds } => {
                self.units = units.clone();
                self.records = records.clone();
                self.rounds = rounds.clone();
            }
            Event::Readiness { states } => self.readiness = states.clone(),
            Event::Workspace { setup, .. } => self.setup = *setup,
            Event::Turbo { on } => self.turbo = *on,
            Event::Reopened { since } => self.reopened = *since,
            _ => return false,
        }
        self.stamp(now());
        true
    }

    /// Whether this project has a pane with this number.
    pub fn has(&self, pane: u32) -> bool {
        self.panes.iter().any(|p| p.pane == pane)
    }

    /// The key the pane's harness keeps its own history behind (M3.6), from
    /// its harness file; none for one that prints into the terminal's.
    pub fn transcript(&self, pane: u32) -> Option<String> {
        let (harness, _) = self.harness(pane)?;
        self.files.find(harness)?.transcript.clone()
    }

    /// The pane's harness id and title.
    pub fn harness(&self, pane: u32) -> Option<(&str, &str)> {
        let p = self.panes.iter().find(|p| p.pane == pane)?;
        let title = self.titles.get(&p.harness).map_or(p.harness.as_str(), String::as_str);
        Some((p.harness.as_str(), title))
    }

    /// The turn a device is shown for the pane at `i`.
    fn turn(&self, i: usize) -> &'static str {
        if !self.panes[i].running {
            return "ended";
        }
        match self.turns.get(i).copied().flatten() {
            Some(Turn::Working) => "working",
            Some(Turn::Waiting) => "waiting",
            Some(Turn::TurnEnded | Turn::Ended) => "ended",
            // Started and heard nothing yet, or said it is at its input.
            Some(Turn::Ready) | None => "ready",
        }
    }

    /// The act put forward for the pane at `i` once its turn has ended, for
    /// its session's latest work: `eval`, `seal`, or none.
    fn next_act(&self, i: usize) -> Option<&'static str> {
        use weft_core::board::Act;
        let p = &self.panes[i];
        let session = self.sessions.iter().find(|r| r.pane == Some(p.pane))?;
        let turn = self.turns.get(i).copied().flatten();
        match weft_core::board::suggested_for(&self.units, &session.id, turn)? {
            Act::Eval => Some("eval"),
            Act::Seal => Some("seal"),
            _ => None,
        }
    }

    /// Note when each pane's turn changed.
    fn stamp(&mut self, at: i64) {
        let turns: Vec<_> =
            (0..self.panes.len()).map(|i| (self.panes[i].pane, self.turn(i))).collect();
        self.since.retain(|pane, _| turns.iter().any(|(p, _)| p == pane));
        for (pane, turn) in turns {
            match self.since.get(&pane) {
                Some((was, _)) if *was == turn => {}
                _ => {
                    self.since.insert(pane, (turn, at));
                }
            }
        }
    }

    /// This project's rows of the `agents` message.
    pub fn rows(&self) -> Vec<Value> {
        (0..self.panes.len())
            .map(|i| {
                let p = &self.panes[i];
                let (harness, title) = self.harness(p.pane).unwrap_or_default();
                json!({
                    "agent": agent_id(&self.key, p.pane),
                    "project": self.name,
                    "harness": harness,
                    "title": title,
                    "turn": self.turn(i),
                    "running": p.running,
                    "since": self.since.get(&p.pane).map_or(0, |(_, at)| *at),
                    "next": self.next_act(i),
                })
            })
            .collect()
    }

    /// This project's row of the `tree` message (remote.md §6):
    /// weft-core's AGENTS and WORK, Sealed and the earlier sessions as
    /// counts, each session a Weft pane holds naming its agent.
    pub fn tree(&self) -> Value {
        let ready = |h: &str| Readiness::of_word(self.readiness[h].as_str());
        let facts = weft_core::tree::Facts {
            name: &self.name,
            units: &self.units,
            panes: &self.panes,
            agents: &self.turns,
            sessions: &self.sessions,
            rounds: &self.rounds,
            readiness: &ready,
            routing: &self.routing,
            setup: self.setup,
        };
        let folded = |_: usize, f: weft_core::tree::Fold| f == weft_core::tree::Fold::Sealed;
        let project = weft_core::tree::rows(&[facts], &folded).remove(0);
        let mut row = serde_json::to_value(&project).unwrap_or_default();
        for (a, at) in project
            .agents
            .rows
            .iter()
            .zip(row["agents"]["rows"].as_array_mut().into_iter().flatten())
        {
            at["agent"] = a.pane.map(|p| agent_id(&self.key, p)).into();
        }
        row["turbo"] = self.turbo.into();
        row["reopened"] = self.reopened.into();
        row
    }

    /// Take what `agents.available` answered; whether it changed.
    pub fn available(&mut self, answer: &Value) -> bool {
        let starts: Vec<(String, String)> = answer["starts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["session"].is_null())
            .filter_map(|s| {
                Some((s["harness"].as_str()?.to_string(), s["spec"].as_str()?.to_string()))
            })
            .collect();
        let changed = starts != self.starts;
        self.starts = starts;
        changed
    }

    /// This project's rows of the `harnesses` message: what can be started.
    pub fn harnesses(&self) -> Vec<Value> {
        self.starts
            .iter()
            .map(|(h, _)| {
                let title = self.titles.get(h).map_or(h.as_str(), String::as_str);
                json!({"project": self.name, "harness": h, "title": title})
            })
            .collect()
    }

    /// The harness an act has to happen in when no agent of it runs, and
    /// the session that route names, as AGENTS reads it:
    /// the one its work was asked in, never a harness's latest.
    pub fn missing(&self, m: &Value) -> Option<(String, Value)> {
        let (act, unit) = match m["act"].as_str()? {
            "eval" => (Act::Eval, self.round_ask(None)),
            "seal" => (Act::Seal, self.round_ask(m["eval"].as_str())),
            "proceed" => (Act::Proceed, m["unit"].as_str().map(str::to_string)),
            _ => return None,
        };
        let at = self.units.iter().position(|u| Some(&u.ask_id) == unit.as_ref())?;
        let harness = self.board(Some(at), None, |b| b.missing_agent(act))?;
        let session = self.units[at].session_ref.as_deref().and_then(|id| self.session_row(id));
        Some((harness, session.map(|r| self.agent_json(r)).unwrap_or(Value::Null)))
    }

    /// The Ask a round's act is routed by: an Eval's newest, else Open's,
    /// else the newest of an Eval being judged (Eval goes on with it).
    fn round_ask(&self, eval: Option<&str>) -> Option<String> {
        let r = &self.rounds;
        match eval {
            Some(id) => r.eval(id).and_then(|g| g.asks.first()).cloned(),
            None => r.open.first().or(r.judging.first().and_then(|j| j.asks.first())).cloned(),
        }
    }

    /// One session of this project, as the index holds it.
    pub fn session_row(&self, id: &str) -> Option<&weft_core::sessions::Row> {
        self.sessions.iter().find(|r| r.id == id)
    }

    /// A session as AGENTS sends it (remote.md §6 `Agent`).
    fn agent_json(&self, r: &weft_core::sessions::Row) -> Value {
        json!({"session": r.id, "harness": r.harness, "state": r.state, "turn": r.turn,
               "title": r.title, "at": r.last, "asks": r.asks,
               "agent": r.pane.map(|p| agent_id(&self.key, p))})
    }

    /// An agent's turn, as its hooks reported it.
    pub fn turn_of(&self, pane: u32) -> Option<Turn> {
        let i = self.panes.iter().position(|p| p.pane == pane)?;
        self.turns.get(i).copied().flatten()
    }

    /// The call that resumes a session by its id: one the
    /// index holds for that harness, that held a conversation and that
    /// nothing runs. The daemon runs the harness file's own resume line.
    /// Refused `running` with the agent a Weft pane holds it in, if one does.
    pub fn resume(&self, harness: &str, id: &str) -> Result<Call, (Refusal, Option<String>)> {
        let row = self
            .session_row(id)
            .filter(|r| r.harness == harness && r.resumable && weft_core::sessions::is_id(id));
        let row =
            row.ok_or((("no_session", format!("there is no {harness} session {id} here")), None))?;
        if row.state == weft_core::sessions::State::Running {
            let agent = row.pane.map(|p| agent_id(&self.key, p));
            return Err((("running", format!("{id} is running")), agent));
        }
        Ok(Call::StartAgent {
            harness: harness.into(),
            spec: String::new(),
            session: Some(id.into()),
        })
    }

    /// The call that starts a fresh agent of `harness` here, with the command
    /// line the daemon offered for it and never one a device sent.
    pub fn start(&self, harness: &str) -> Result<Call, Refusal> {
        let (_, spec) = self
            .starts
            .iter()
            .find(|(h, _)| h == harness)
            .ok_or(("no_harness", format!("{harness} cannot be started in {}", self.name)))?;
        Ok(Call::StartAgent { harness: harness.into(), spec: spec.clone(), session: None })
    }

    /// The Ask with this id, if it is on this project's board.
    pub fn unit(&self, id: &str) -> Option<&Unit> {
        self.units.iter().find(|u| u.ask_id == id)
    }

    /// A rule's answer about this board, with `unit` selected and `pane` in front.
    fn board<T>(&self, unit: Option<usize>, pane: Option<u32>, f: impl FnOnce(Board) -> T) -> T {
        let ready = |h: &str| Readiness::of_word(self.readiness[h].as_str());
        f(Board {
            units: &self.units,
            selected: unit.unwrap_or(usize::MAX),
            panes: &self.panes,
            focused: pane.and_then(|p| self.panes.iter().position(|x| x.pane == p)).unwrap_or(0),
            readiness: &ready,
            routing: &self.routing,
            workspace_gap: None,
            agents: &self.turns,
        })
    }

    /// A work's Details (T11.2): its Ask only, from the parts the daemon read.
    pub fn detail(&self, unit: &Unit, source: Option<&str>, prompt: Option<&str>) -> Value {
        let session = unit.session_ref.as_deref().and_then(|id| self.session_row(id));
        let agent = session.map(|r| weft_core::tree::Agent {
            session: r.id.clone(),
            harness: r.harness.clone(),
            state: r.state,
            turn: r.turn,
            title: r.title.clone(),
            at: r.last,
            asks: r.asks,
            pane: r.pane,
            ask: None,
        });
        let d = weft_core::offers::work_detail(
            unit,
            &self.rounds.record,
            &self.rounds,
            agent,
            source,
            prompt,
            self.zone,
        );
        let mut v = serde_json::to_value(d).unwrap_or_default();
        if let Some(r) = session {
            v["session"] = self.agent_json(r);
        }
        v
    }

    /// An Eval's Details (T11.3), once.
    pub fn eval(&self, id: &str) -> Result<Value, Refusal> {
        let record = &self.rounds.record;
        let e = record
            .evals
            .iter()
            .find(|e| e.id == id)
            .ok_or(("no_eval", format!("{id} is no Eval here")))?;
        let judges: Option<weft_core::record::Record> =
            self.records.get(id).and_then(|v| serde_json::from_value(v.clone()).ok());
        let d = weft_core::offers::eval_detail(
            e,
            judges.as_ref(),
            &self.units,
            record,
            self.seal_why(id),
        );
        Ok(serde_json::to_value(d).unwrap_or_default())
    }

    /// Why Seal cannot run on this Eval, if it cannot: sealed already, still
    /// being judged, or as the harness it goes to.
    fn seal_why(&self, eval: &str) -> Option<weft_core::tree::Why> {
        match self.rounds.eval(eval) {
            Some(round) => {
                let board = |u: Option<&str>| {
                    let at = u.and_then(|id| self.units.iter().position(|x| x.ask_id == id));
                    self.board(at, None, |b| b.not_ready(Act::Seal))
                };
                weft_core::rounds::seal_why(round, &board)
            }
            None => weft_core::rounds::sealed_already(&self.rounds.record, eval),
        }
    }

    /// A Seal's Details (T11.4), its Asks a page at a time.
    pub fn seal(
        &self,
        id: &str,
        checked: Option<&Value>,
        after: Option<&str>,
    ) -> Result<Value, Refusal> {
        let s = self
            .rounds
            .record
            .seals
            .iter()
            .find(|s| s.id == id)
            .ok_or(("no_seal", format!("{id} is no Seal here")))?;
        let d = weft_core::offers::seal_detail(s, &self.units, &self.rounds.record, checked);
        let mut v = serde_json::to_value(&d).unwrap_or_default();
        let (asks, next) = page(&d.asks, after, |a| a.unit.as_str());
        v["asks"] = serde_json::to_value(asks).unwrap_or_default();
        v["next"] = next.into();
        Ok(v)
    }

    /// A session's Details (T11.5).
    pub fn session(&self, id: &str) -> Result<Value, Refusal> {
        let row = self.session_row(id).ok_or(("no_session", format!("{id} is no session here")))?;
        let joins = self.files.find(&row.harness).is_some_and(|h| h.resume_joins);
        let d = weft_core::offers::session_detail(row, &self.units, &self.rounds.record, joins);
        let mut v = serde_json::to_value(d).unwrap_or_default();
        v["agent"] = row.pane.map(|p| agent_id(&self.key, p)).into();
        Ok(v)
    }

    /// The Seals Sealed folds, a page at a time, newest first.
    pub fn sealed(&self, after: Option<&str>) -> Value {
        let rows: Vec<Value> = self
            .rounds
            .seals
            .iter()
            .map(|s| {
                json!({"seal": s.seal, "n": s.n, "decision": weft_core::tree::decision(&s.decision),
                       "at": weft_core::turns::millis(&s.at), "eval": s.eval.as_ref().map(|e| e.n),
                       "asks": s.asks.len()})
            })
            .collect();
        let (rows, next) = page(&rows, after, |r| r["seal"].as_str().unwrap_or_default());
        json!({"seals": rows, "next": next})
    }

    /// The earlier sessions, past the tree's, narrowed by `q` and grouped by
    /// the Mac's date, a page at a time.
    pub fn sessions(&self, q: Option<&str>, after: Option<&str>, now: i64) -> Value {
        use weft_core::sessions::{Group, matches, split};
        let found: Vec<Value> = split(&self.sessions)
            .1
            .iter()
            .filter(|r| q.is_none_or(|q| matches(r, q)))
            .map(|r| {
                let group = match Group::of(r.last, now, self.zone) {
                    Group::Today => "today",
                    Group::Yesterday => "yesterday",
                    Group::Week => "week",
                    Group::Earlier => "earlier",
                };
                json!({"session": r.id, "harness": r.harness, "title": r.title,
                       "asks": r.asks, "at": r.last, "group": group})
            })
            .collect();
        let total = found.len();
        let (rows, next) = page(&found, after, |r| r["session"].as_str().unwrap_or_default());
        json!({"total": total, "rows": rows, "next": next})
    }

    /// What a device's act asks the daemon (remote.md §6): an Ask to the
    /// agent it names (`pane`), Proceed or Follow up on a work (`unit`), Eval
    /// on the Open round, Seal on an Eval's (`eval`). When the act cannot
    /// run, the refusal's code is the why-not's `words` key.
    pub fn act(&self, m: &Value, pane: Option<u32>) -> Result<Call, Refusal> {
        let (act, daemon) = match m["act"].as_str() {
            Some("ask") => (Act::Ask, "ask"),
            Some("follow_up") => (Act::FollowUp, "follow_up"),
            Some("proceed") => (Act::Proceed, "send"),
            Some("eval") => (Act::Eval, "check"),
            Some("seal") => (Act::Seal, "decide"),
            _ => {
                return Err((
                    "not_allowed",
                    "the acts are ask, eval, seal, proceed and follow_up".into(),
                ));
            }
        };
        let said = |why: weft_core::tree::Why| (why.key, why.say());
        let mut eval = None;
        let unit = match act {
            Act::Ask => None,
            Act::Eval => {
                let board = |u: Option<&str>| {
                    let at = u.and_then(|id| self.units.iter().position(|x| x.ask_id == id));
                    self.board(at, None, |b| b.not_ready(Act::Eval))
                };
                if let Some(why) = weft_core::rounds::eval_why(&self.rounds, &board) {
                    return Err(said(why));
                }
                None
            }
            Act::Seal => {
                let id = m["eval"]
                    .as_str()
                    .ok_or(("bad_request", "a Seal names its Eval".to_string()))?;
                if self.rounds.eval(id).is_none()
                    && weft_core::rounds::sealed_already(&self.rounds.record, id).is_none()
                {
                    return Err(("no_eval", format!("{id} is no Eval round here")));
                }
                if let Some(why) = self.seal_why(id) {
                    return Err(said(why));
                }
                eval = Some(id.to_string());
                None
            }
            _ => match m["unit"].as_str() {
                Some(id) => Some(
                    self.units
                        .iter()
                        .position(|u| u.ask_id == id)
                        .ok_or(("no_unit", format!("{id} is not on the board")))?,
                ),
                None => return Err(("bad_request", "an act on a work names its unit".into())),
            },
        };
        let asking = matches!(act, Act::Ask | Act::FollowUp);
        let text = m["text"].as_str().filter(|t| !t.trim().is_empty()).map(str::to_string);
        if asking && text.is_none() {
            return Err(("bad_request", "an Ask needs its words".into()));
        }
        if matches!(act, Act::Ask | Act::FollowUp | Act::Proceed)
            && let Some(why) = self.board(unit, pane, |b| b.why_not(act))
        {
            return Err(said(why));
        }
        // An Ask can go into a new agent, of a harness this project starts.
        let start = match m["start"].as_str().filter(|_| asking) {
            Some(harness) => {
                self.start(harness)?;
                Some(harness.to_string())
            }
            None => None,
        };
        Ok(Call::Act {
            act: daemon.into(),
            unit: unit.map(|i| self.units[i].ask_id.clone()),
            pane: pane.filter(|_| asking && start.is_none()),
            text: text.filter(|_| asking),
            eval,
            start,
        })
    }

    /// Each pane's id on the wire and its turn, for push.
    pub fn turns(&self) -> Vec<(String, &'static str)> {
        (0..self.panes.len())
            .map(|i| (agent_id(&self.key, self.panes[i].pane), self.turn(i)))
            .collect()
    }
}

/// How many rows a page of a folded group holds (remote.md §6).
pub const PAGE: usize = 50;

/// The page of `rows` after the one `after` names (the first page with
/// none), and the `after` that reads the next page, or none at the end.
pub fn page<'a, T: Clone>(
    rows: &'a [T],
    after: Option<&str>,
    id: impl Fn(&'a T) -> &'a str,
) -> (Vec<T>, Option<String>) {
    let start = match after {
        Some(a) => rows.iter().position(|r| id(r) == a).map_or(rows.len(), |i| i + 1),
        None => 0,
    };
    let end = (start + PAGE).min(rows.len());
    let next = (end < rows.len()).then(|| id(&rows[end - 1]).to_string());
    (rows[start..end].to_vec(), next)
}

/// The `agents` message: every project's rows, in one list.
pub fn message(projects: &[Project]) -> Value {
    let rows: Vec<Value> = projects.iter().flat_map(Project::rows).collect();
    json!({"type": "agents", "agents": rows})
}

/// Which project and pane an agent id names.
pub fn find(projects: &[Project], agent: &str) -> Option<(usize, u32)> {
    let (key, pane) = agent.rsplit_once('-')?;
    let pane: u32 = pane.parse().ok()?;
    let at = projects.iter().position(|p| p.key == key)?;
    projects[at].has(pane).then_some((at, pane))
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agents_id_is_its_projects_key_and_its_pane() {
        let key = project_key(Path::new("/Users/me/work/thing"));
        assert_eq!(key.len(), 12);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(key, project_key(Path::new("/Users/me/work/thing")), "the same every time");
        assert_ne!(key, project_key(Path::new("/Users/me/work/other")));
        assert_eq!(agent_id("3f9a1c07b2de", 4), "3f9a1c07b2de-4");
    }

    fn unit(id: &str, title: &str) -> Unit {
        Unit {
            ask_id: id.into(),
            title: title.into(),
            harness: "claude".into(),
            session_ref: Some("s_1".into()),
            confirmed: true,
            sent: weft_core::ledger::Sent::TakenByAgent,
            ..Default::default()
        }
    }

    fn opened() -> Project {
        let answer = json!({
            "panes": [{"pane": 4, "harness": "claude", "spec": "claude", "running": true}],
            "agents": {"panes": ["waiting"], "sessions": [
                {"harness": "claude", "id": "s_1", "state": "running", "turn": "waiting",
                 "started": 0, "last": 0, "title": "fix login", "asks": 1, "pane": 4,
                 "resumable": true}]},
            "harnesses": [{"name": "claude", "title": "Claude Code"}],
            "units": [unit("ask_1", "fix login")],
            "rounds": {"open": ["ask_1"]},
            "readiness": {"claude": "ready"},
        });
        Project::opened(Path::new("/work/weft"), &answer)
    }

    fn open_asks(tree: &Value) -> Vec<(String, String)> {
        tree["work"]["open"]["asks"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|w| {
                (w["title"].as_str().unwrap().to_string(), w["word"].as_str().unwrap().to_string())
            })
            .collect()
    }

    /// Once an agent's turn has ended, its row puts forward the next act for
    /// its session's latest work, as the desktop's bar does.
    #[test]
    fn an_agents_row_puts_forward_the_next_act_once_its_turn_has_ended() {
        let p = opened();
        assert_eq!(p.rows()[0]["next"], Value::Null, "waiting on its person");
        let answer = json!({
            "panes": [{"pane": 4, "harness": "claude", "spec": "claude", "running": true}],
            "agents": {"panes": ["turn_ended"], "sessions": [
                {"harness": "claude", "id": "s_1", "state": "running", "turn": "turn_ended",
                 "started": 0, "last": 0, "title": "fix login", "asks": 1, "pane": 4,
                 "resumable": true}]},
            "harnesses": [{"name": "claude", "title": "Claude Code"}],
            "units": [unit("ask_1", "fix login")],
            "rounds": {"open": ["ask_1"]},
            "readiness": {"claude": "ready"},
        });
        let ended = Project::opened(Path::new("/work/weft"), &answer);
        assert_eq!(ended.rows()[0]["next"], "eval");
    }

    /// remote.md §6: AGENTS with each session's agent, and the Open round.
    #[test]
    fn the_tree_names_each_sessions_agent_and_the_open_rounds_asks() {
        let p = opened();
        let tree = p.tree();
        assert_eq!(
            (tree["project"].clone(), tree["open"].clone(), tree["needs"].clone()),
            (json!("weft"), json!(1), json!(1))
        );
        let a = &tree["agents"];
        assert_eq!(
            (a["running"].clone(), a["ended"].clone(), a["earlier"].clone()),
            (json!(1), json!(0), json!(0))
        );
        let s = &a["rows"][0];
        assert_eq!(s["agent"], agent_id(&p.key, 4));
        assert_eq!(
            (s["session"].clone(), s["turn"].clone(), s["title"].clone()),
            (json!("s_1"), json!("waiting"), json!("fix login"))
        );
        assert_eq!(open_asks(&tree), [("fix login".into(), "asked".into())]);
        assert_eq!(tree["work"]["open"]["eval"], Value::Null);
        assert!(tree.get("harnesses").is_none(), "no harness rows");
    }

    #[test]
    fn a_unit_added_an_eval_recorded_and_an_agent_ending_change_the_tree() {
        let mut p = opened();
        let units = vec![unit("ask_1", "fix login"), unit("ask_2", "telemetry")];
        let rounds = |open: &[&str], judged: &[&str]| weft_core::rounds::Rounds {
            open: open.iter().map(|s| s.to_string()).collect(),
            evals: if judged.is_empty() {
                Vec::new()
            } else {
                vec![weft_core::rounds::Round {
                    eval: "evl_1".into(),
                    n: 1,
                    judging: false,
                    verdict: Some(weft_core::ledger::Verdict::Matches),
                    agreement: Some(1.0),
                    asks: judged.iter().map(|s| s.to_string()).collect(),
                    sealed: None,
                }]
            },
            ..Default::default()
        };
        assert!(p.heard(&Event::Units {
            units: units.clone(),
            records: json!({}),
            rounds: rounds(&["ask_2", "ask_1"], &[])
        }));
        assert_eq!(
            open_asks(&p.tree()),
            [("telemetry".into(), "asked".into()), ("fix login".into(), "asked".into())]
        );
        assert!(p.heard(&Event::Units {
            units,
            records: json!({}),
            rounds: rounds(&["ask_2"], &["ask_1"])
        }));
        let tree = p.tree();
        let g = &tree["work"]["evals"][0];
        assert_eq!(
            (g["n"].clone(), g["verdict"].clone(), g["agreement"].clone()),
            (json!(1), json!("aligned"), json!("1.0"))
        );
        assert_eq!(
            g["asks"],
            json!([{"unit": "ask_1", "title": "fix login", "harness": "claude"}]),
            "no word under an Eval"
        );
        assert!(p.heard(&Event::Exited { pane: 4 }));
        assert_eq!(
            p.rows()[0]["running"],
            false,
            "a device tells an agent that ended from a turn that did"
        );
    }

    #[test]
    fn a_harness_not_set_up_is_why_eval_and_ask_cannot_run() {
        let mut p = opened();
        assert!(p.heard(&Event::Readiness { states: json!({"claude": "no_plugin"}) }));
        let tree = p.tree();
        let why = json!({"key": "why.not_set_up", "harness": "claude"});
        assert_eq!(tree["work"]["open"]["eval"], why);
        assert_eq!(tree["agents"]["rows"][0]["ask"], why);
    }

    #[test]
    fn the_tree_says_what_a_project_needs_until_ringframe_is_set_up_in_it() {
        use weft_core::workspace::Needs;
        assert_eq!(opened().tree()["setup"], Value::Null, "set up");
        let answer = json!({"panes": [], "units": [], "readiness": {},
            "setup": {"ringframe": true, "git": false, "commit": false}});
        let mut p = Project::opened(Path::new("/work/test"), &answer);
        assert_eq!(p.tree()["setup"], answer["setup"]);
        let no_commit = Needs::refused(Some("workspace.no_commit"));
        assert!(p.heard(&Event::Workspace { setup: Some(no_commit), gap: None }));
        assert_eq!(p.tree()["setup"], json!({"ringframe": true, "git": true, "commit": false}));
        assert!(p.heard(&Event::Workspace { setup: None, gap: None }));
        assert_eq!(p.tree()["setup"], Value::Null, "set up as soon as it all held");
    }

    /// Folded on the wire: Sealed is counts; Open and an Eval are the
    /// device's own to fold, so they arrive whole.
    #[test]
    fn sealed_travels_as_counts_and_a_device_folds_the_rest() {
        let tree = opened().tree();
        assert!(tree.get("folded").is_none() && tree["work"]["open"].get("folded").is_none());
        assert_eq!(tree["work"]["sealed"], json!({"seals": 0, "asks": 0}));
        assert_eq!(open_asks(&tree).len(), 1, "Open is not folded on the wire");
    }

    fn act(p: &Project, m: Value) -> Result<Call, Refusal> {
        p.act(&m, Some(4))
    }

    fn call(act: &str, unit: Option<&str>, pane: Option<u32>, text: Option<&str>) -> Call {
        Call::Act {
            act: act.into(),
            unit: unit.map(str::to_string),
            pane,
            text: text.map(str::to_string),
            eval: None,
            start: None,
        }
    }

    /// `opened`, with Eval 1 judged over ask_1 and ask_2 Open.
    fn judged() -> Project {
        let mut p = opened();
        let mut telemetry = unit("ask_2", "telemetry");
        telemetry.sent = weft_core::ledger::Sent::ReadyToSend;
        p.units.push(telemetry);
        let events = [
            json!({"type": "eval.opened", "id": "evl_1", "time": "2026-10-08T10:00:00Z",
                   "data": {"basis": {"asks": ["ask_1"]}}}),
            json!({"type": "eval.completed", "id": "evl_1", "time": "2026-10-08T10:05:00Z",
                   "data": {"basis": {"asks": ["ask_1"]}, "verdict": "aligned", "confidence": 1.0}}),
        ];
        let record = weft_core::rounds::record(&events);
        p.units[0].check = Some(weft_core::ledger::Check {
            eval_id: "evl_1".into(),
            verdict: weft_core::ledger::Verdict::Matches,
            agreement: 1.0,
            judged_by: None,
        });
        p.rounds = weft_core::rounds::rounds(&p.units, &record);
        p
    }

    #[test]
    fn each_act_makes_its_daemon_call() {
        let mut p = opened();
        let mut waiting = unit("ask_2", "telemetry");
        waiting.sent = weft_core::ledger::Sent::ReadyToSend;
        p.units.push(waiting);
        for (m, want) in [
            (
                json!({"act": "ask", "text": "add /health"}),
                call("ask", None, Some(4), Some("add /health")),
            ),
            (
                json!({"act": "follow_up", "unit": "ask_1", "text": "and a test"}),
                call("follow_up", Some("ask_1"), Some(4), Some("and a test")),
            ),
            (json!({"act": "proceed", "unit": "ask_2"}), call("send", Some("ask_2"), None, None)),
        ] {
            assert_eq!(act(&p, m.clone()), Ok(want), "{m}");
        }
        // Eval on the Open round, Seal on an Eval's (remote.md §6).
        let p = judged();
        assert_eq!(
            act(&p, json!({"act": "eval", "project": "weft"})),
            Ok(call("check", None, None, None))
        );
        let sealed = Call::Act {
            act: "decide".into(),
            unit: None,
            pane: None,
            text: None,
            eval: Some("evl_1".into()),
            start: None,
        };
        assert_eq!(act(&p, json!({"act": "seal", "project": "weft", "eval": "evl_1"})), Ok(sealed));
    }

    #[test]
    fn an_act_that_cannot_run_is_refused_with_its_why_not() {
        let p = opened();
        let refused = |p: &Project, m: Value| act(p, m).unwrap_err();
        assert_eq!(
            refused(&p, json!({"act": "proceed", "unit": "ask_1"})),
            (
                "why.nothing_to_send",
                "fix login has nothing to send. Follow up asks for more.".into()
            )
        );
        assert_eq!(refused(&p, json!({"act": "proceed", "unit": "ask_9"})).0, "no_unit");
        assert_eq!(refused(&p, json!({"act": "seal"})).0, "bad_request");
        assert_eq!(refused(&p, json!({"act": "seal", "eval": "evl_9"})).0, "no_eval");
        assert_eq!(refused(&p, json!({"act": "ask", "text": " "})).0, "bad_request");
        assert_eq!(refused(&p, json!({"act": "start"})).0, "not_allowed");
        // A sealed Eval keeps its act, which says so (D12.6).
        let mut s = judged();
        s.rounds.record.seals.push(weft_core::rounds::Seal {
            id: "sel_1".into(),
            n: 1,
            at: String::new(),
            disposition: "accepted".into(),
            by: "you".into(),
            eval: Some("evl_1".into()),
        });
        s.rounds.evals[0].sealed = Some(weft_core::rounds::Named { id: "sel_1".into(), n: 1 });
        assert_eq!(
            refused(&s, json!({"act": "seal", "eval": "evl_1"})),
            ("why.sealed_already", "Eval 1 is sealed already.".into())
        );
        let mut p = judged();
        assert!(p.heard(&Event::Readiness { states: json!({"claude": "no_plugin"}) }));
        for m in [
            json!({"act": "eval"}),
            json!({"act": "seal", "eval": "evl_1"}),
            json!({"act": "ask", "text": "x"}),
        ] {
            assert_eq!(
                refused(&p, m),
                ("why.not_set_up", "claude is not set up for RingFrame.".into())
            );
        }
    }

    /// M3.6: whose history is behind which key, from the harness's own file.
    #[test]
    fn an_agent_names_the_key_its_own_history_is_behind() {
        let files = weft_core::harness::fixture::harnesses();
        let answer = json!({
            "panes": [{"pane": 1, "harness": "codex", "spec": "codex", "running": true},
                      {"pane": 2, "harness": "claude-code", "spec": "claude", "running": true}],
            "harnesses": [serde_json::to_value(files.find("codex").unwrap()).unwrap(),
                          serde_json::to_value(files.find("claude-code").unwrap()).unwrap()],
        });
        let p = Project::opened(Path::new("/work/weft"), &answer);
        assert_eq!(p.transcript(1).as_deref(), Some("Ctrl+T"));
        assert_eq!(p.transcript(2), None, "it prints inline: the terminal keeps its history");
    }

    /// The header's ⚡ turbo (M2.9): the project's switch, as opened and as
    /// switched since, from the TUI or another window.
    #[test]
    fn the_tree_says_whether_the_project_starts_agents_in_turbo() {
        let p = Project::opened(Path::new("/work/weft"), &json!({"turbo": true}));
        assert_eq!(p.tree()["turbo"], true);
        let mut p = p;
        assert!(p.heard(&Event::Turbo { on: false }), "a change a device sees");
        assert_eq!(p.tree()["turbo"], false);
    }

    #[test]
    fn a_device_starts_only_what_the_daemon_offers_fresh() {
        let mut p = opened();
        let answer = json!({"starts": [
            {"harness": "claude", "label": "claude · pick up", "spec": "claude --resume s_1", "session": {"id": "s_1"}},
            {"harness": "claude", "label": "claude · start fresh", "spec": "claude", "session": null},
            {"harness": "codex", "label": "codex · start fresh", "spec": "codex", "session": null}]});
        assert!(p.available(&answer));
        assert!(!p.available(&answer), "the same list is no change");
        assert_eq!(
            p.harnesses(),
            [
                json!({"project": "weft", "harness": "claude", "title": "Claude Code"}),
                json!({"project": "weft", "harness": "codex", "title": "codex"})
            ]
        );
        assert_eq!(
            p.start("codex"),
            Ok(Call::StartAgent { harness: "codex".into(), spec: "codex".into(), session: None })
        );
        assert_eq!(p.start("sh").unwrap_err().0, "no_harness");
    }

    /// An Ask into a new agent names a harness this project starts, and no
    /// pane: the daemon starts it on the person's Enter.
    #[test]
    fn an_ask_into_a_new_agent_names_a_harness_offered_fresh() {
        let mut p = opened();
        p.available(&json!({"starts": [
            {"harness": "codex", "label": "codex · start fresh", "spec": "codex", "session": null}]}));
        let ask = json!({"act": "ask", "agent": "x", "text": "do it", "start": "codex"});
        let mut want = call("ask", None, None, Some("do it"));
        if let Call::Act { start, .. } = &mut want {
            *start = Some("codex".into());
        }
        assert_eq!(act(&p, ask), Ok(want));
        let nowhere = json!({"act": "ask", "agent": "x", "text": "do it", "start": "sh"});
        assert_eq!(act(&p, nowhere).unwrap_err().0, "no_harness");
    }

    /// T11.2's payload: the Ask, its breadcrumb, and its session as AGENTS
    /// reads it, with its agent.
    #[test]
    fn a_works_details_hold_its_ask_its_crumb_and_its_session() {
        let p = judged();
        let u = p.unit("ask_1").unwrap().clone();
        let d = p.detail(&u, Some("Refresh it"), Some("Refresh the token"));
        assert_eq!(d["crumb"]["eval"], json!({"eval": "evl_1", "n": 1, "verdict": "aligned"}));
        assert_eq!(d["ask"]["lines"][0], "You asked:");
        assert_eq!(d["session"]["agent"], agent_id(&p.key, 4));
        assert!(d.get("sections").is_none(), "no Eval or Seal section");
    }

    /// T11.3, T11.5: an Eval once, a session with its Asks.
    #[test]
    fn an_eval_and_a_session_are_read_once_each() {
        let p = judged();
        let e = p.eval("evl_1").unwrap();
        assert_eq!(
            (e["n"].clone(), e["verdict"].clone(), e["seal"].clone()),
            (json!(1), json!("aligned"), Value::Null)
        );
        assert_eq!(p.eval("evl_9").unwrap_err().0, "no_eval");
        let s = p.session("s_1").unwrap();
        assert_eq!(s["agent"], agent_id(&p.key, 4));
        assert_eq!(s["asks"].as_array().map(Vec::len), Some(2));
        assert_eq!(s["resume"], false, "running: go to it");
    }

    /// A session is resumed by its id, one the index holds and
    /// nothing runs; never a harness's latest (the `gl` case).
    #[test]
    fn a_session_is_resumed_by_its_id_and_nothing_else() {
        let mut p = opened();
        p.sessions.push(weft_core::sessions::Row::of(
            "claude",
            "s_old",
            weft_core::sessions::State::Ended,
            None,
            5,
        ));
        assert_eq!(
            p.resume("claude", "s_old"),
            Ok(Call::StartAgent {
                harness: "claude".into(),
                spec: String::new(),
                session: Some("s_old".into())
            })
        );
        let refused = |id: &str| p.resume("claude", id).unwrap_err();
        assert_eq!(refused("s_9").0.0, "no_session");
        assert_eq!(refused("--last").0.0, "no_session");
        assert_eq!(
            refused("s_1"),
            (("running", "s_1 is running".into()), Some(agent_id(&p.key, 4)))
        );
        assert_eq!(p.resume("codex", "s_old").unwrap_err().0.0, "no_session", "of that harness");
        // An act that has no agent names the session its work was asked in.
        let mut gone = judged();
        gone.panes[0].running = false;
        let (harness, session) = gone.missing(&json!({"act": "seal", "eval": "evl_1"})).unwrap();
        assert_eq!((harness.as_str(), session["session"].clone()), ("claude", json!("s_1")));
    }

    /// A large project: the tree carries Sealed and the earlier sessions as
    /// counts, and stays under one WebSocket message; pages cover every row
    /// once, in order; `q` narrows by title, harness and id.
    #[test]
    fn three_hundred_sessions_and_four_hundred_asks_travel_as_counts_and_pages() {
        use weft_core::sessions::{Row, State};
        let mut p = opened();
        p.sessions = (0..300)
            .map(|i| {
                let state = if i < 5 { State::Running } else { State::Ended };
                let mut r = Row::of(
                    if i % 2 == 0 { "claude" } else { "codex" },
                    &format!("s{i:03}"),
                    state,
                    None,
                    10_000 - i,
                );
                r.title = Some(format!("work number {i}"));
                r
            })
            .collect();
        let mut events = Vec::new();
        for i in 0..400 {
            events.push(json!({"type": "ask.compiled", "id": format!("ask_{i:03}"), "time": "2026-10-08T10:00:00Z",
                "data": {"title": format!("an Ask with a title of some length, number {i}"), "delivery_mode": "native_dispatch",
                         "host": {"name": "claude"}}}));
        }
        for k in 0..39 {
            let asks: Vec<String> = (k * 10..k * 10 + 10).map(|i| format!("ask_{i:03}")).collect();
            let links: Vec<Value> = asks.iter().map(|a| json!({"rel": "seals", "id": a})).collect();
            events.push(json!({"type": "seal.created", "id": format!("sel_{k:02}"), "time": "2026-10-08T11:00:00Z",
                "actor": {"kind": "human"}, "links": links, "data": {"basis": {"asks": asks}, "disposition": "accepted"}}));
        }
        p.units = weft_core::ledger::project(&events);
        p.rounds = weft_core::rounds::rounds(&p.units, &weft_core::rounds::record(&events));
        let tree = p.tree();
        let size = tree.to_string().len();
        assert!(size < crate::socket::MAX_MESSAGE, "{size} bytes");
        assert_eq!(tree["work"]["sealed"], json!({"seals": 39, "asks": 390}));
        assert_eq!(
            (tree["agents"]["rows"].as_array().map(Vec::len), tree["agents"]["earlier"].clone()),
            (Some(8), json!(292))
        );
        assert_eq!(tree["work"]["open"]["asks"].as_array().map(Vec::len), Some(10));

        let mut seen = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = p.sessions(None, after.as_deref(), 10_000);
            assert_eq!(page["total"], 292);
            seen.extend(
                page["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["session"].as_str().unwrap().to_string()),
            );
            match page["next"].as_str() {
                Some(n) => after = Some(n.to_string()),
                None => break,
            }
        }
        let want: Vec<String> = (8..300).map(|i| format!("s{i:03}")).collect();
        assert_eq!(seen, want, "every earlier session once, in order");
        let mut seals = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let page = p.sealed(after.as_deref());
            seals
                .extend(page["seals"].as_array().unwrap().iter().map(|s| s["n"].as_u64().unwrap()));
            match page["next"].as_str() {
                Some(n) => after = Some(n.to_string()),
                None => break,
            }
        }
        assert_eq!(seals, (1..=39).rev().collect::<Vec<u64>>(), "every Seal once, newest first");
        let found = |q: &str| p.sessions(Some(q), None, 10_000)["total"].clone();
        assert_eq!(found("number 29"), json!(11), "29 and 290–299");
        assert_eq!(found("CODEX"), json!(146));
        assert_eq!(found("s123"), json!(1));
    }
}
