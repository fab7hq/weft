//! The agents on this machine, as a device sees them (remote.md §5, §6): one
//! list across every open project, each agent named by an id that stays the
//! same for as long as its pane lives, and its turn as the daemon reported
//! it. Nothing here works out NEEDS YOU: `waiting` is the daemon's word.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use weft_core::board::{Act, Board, PaneInfo};
use weft_core::harness::Harnesses;
use weft_core::ledger::Unit;
use weft_core::readiness::Readiness;
use weft_core::routing::Routing;
use weft_core::sessions::Recorded;
use weft_core::turns::{Session, Turn};
use weft_core::workspace::Needs;
use weft_proto::{Call, Event};

/// Why a device's message was refused: a code, and a sentence.
pub type Refusal = (&'static str, String);

/// The first 12 hex digits of the SHA-256 of a project's path.
pub fn project_key(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().as_bytes());
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

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
    /// The sessions the hooks reported, for a work's own turn.
    sessions: Vec<Session>,
    /// What each harness is short of, by the daemon's word.
    readiness: Value,
    routing: Routing,
    /// The harnesses the person can start here, fresh, and each one's
    /// command line, as `agents.available` last answered.
    starts: Vec<(String, String)>,
    /// The latest session of each harness the daemon offers to pick up.
    resumes: Vec<(String, Recorded)>,
    /// The harness files, as the daemon read them: how to resume a session.
    files: Harnesses,
    /// What the project still needs before RingFrame is set up in it, if it
    /// is not.
    setup: Option<Needs>,
    /// Whether agents started here from now on run in turbo mode.
    turbo: bool,
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
            sessions: serde_json::from_value(answer["agents"]["sessions"].clone())
                .unwrap_or_default(),
            readiness: answer["readiness"].clone(),
            routing: Routing::opened(&answer["routing"]),
            starts: Vec::new(),
            resumes: Vec::new(),
            files: serde_json::from_value(answer["harnesses"].clone()).unwrap_or_default(),
            setup: serde_json::from_value(answer["setup"].clone()).unwrap_or_default(),
            turbo: answer["turbo"].as_bool().unwrap_or(false),
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
            Event::Agents { panes, sessions } => {
                self.turns = panes.clone();
                self.sessions = sessions.clone();
            }
            Event::Units { units, .. } => self.units = units.clone(),
            Event::Readiness { states } => self.readiness = states.clone(),
            Event::Workspace { setup, .. } => self.setup = *setup,
            Event::Turbo { on } => self.turbo = *on,
            _ => return false,
        }
        self.stamp(now());
        true
    }

    /// Whether this project has a pane with this number.
    pub fn has(&self, pane: u32) -> bool {
        self.panes.iter().any(|p| p.pane == pane)
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
            Some(Turn::TurnEnded) => "ended",
            // Started and heard nothing yet, or said it is at its input.
            Some(Turn::Ready) | None => "ready",
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
                })
            })
            .collect()
    }

    /// This project's row of the `tree` message: weft-core's rows, nothing
    /// folded (folding is the device's own), each harness naming the agent
    /// a tap on it opens.
    pub fn tree(&self) -> Value {
        let title = |h: &str| self.titles.get(h).cloned();
        let ready = |h: &str| Readiness::of_word(self.readiness[h].as_str());
        let facts = weft_core::tree::Facts {
            name: &self.name,
            units: &self.units,
            panes: &self.panes,
            agents: &self.turns,
            sessions: &self.sessions,
            title: &title,
            readiness: &ready,
            routing: &self.routing,
            setup: self.setup,
        };
        let project = weft_core::tree::rows(&[facts], &|_, _| false).remove(0);
        let mut row = serde_json::to_value(&project).unwrap_or_default();
        for (h, at) in
            project.harnesses.iter().zip(row["harnesses"].as_array_mut().into_iter().flatten())
        {
            at["agent"] = h.pane.map(|p| agent_id(&self.key, p)).into();
            // Not running: what a tap on it would resume (M2.9), else nothing.
            if h.pane.is_none() {
                let s = weft_core::offers::resume_session(None, self.latest(&h.harness));
                at["session"] = s.map(|s| json!({"at": s.at, "last": s.last})).into();
            }
        }
        row["turbo"] = self.turbo.into();
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
        self.resumes = answer["starts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| {
                let (h, r) = (s["harness"].as_str()?, &s["session"]);
                let last = r["last"].as_str()?.to_string();
                Some((
                    h.to_string(),
                    Recorded { id: r["id"].as_str()?.to_string(), at: r["at"].as_i64()?, last },
                ))
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

    /// The session a work's agent would resume, as the TUI picks one up.
    pub fn session_of(&self, unit: &Unit) -> Option<Recorded> {
        weft_core::offers::resume_session(Some(unit), self.latest(&unit.harness))
    }

    /// The harness's latest session on record, what a tap on a harness that
    /// runs no agent resumes (M2.9), as the TUI's Enter on it does.
    fn latest(&self, harness: &str) -> Option<&Recorded> {
        self.resumes.iter().find(|(h, _)| h == harness).map(|(_, s)| s)
    }

    /// The harness an act on this work has to happen in, when no agent of it runs.
    pub fn missing(&self, m: &Value) -> Option<String> {
        let act = match m["act"].as_str()? {
            "eval" => Act::Eval,
            "seal" => Act::Seal,
            "proceed" => Act::Proceed,
            _ => return None,
        };
        let at = self.units.iter().position(|u| Some(u.ask_id.as_str()) == m["unit"].as_str())?;
        self.board(Some(at), None, |b| b.missing_agent(act))
    }

    /// An agent's turn, as its hooks reported it.
    pub fn turn_of(&self, pane: u32) -> Option<Turn> {
        let i = self.panes.iter().position(|p| p.pane == pane)?;
        self.turns.get(i).copied().flatten()
    }

    /// The call that resumes a work's session with its harness's own resume
    /// command (`claude --resume <id>`, `codex resume <id>`), as the TUI's
    /// pick-up runs it.
    /// A work's session, or with no work the harness's latest.
    pub fn resume(&self, harness: &str, unit: Option<&str>) -> Result<Call, Refusal> {
        let session = match unit {
            Some(id) => self
                .unit(id)
                .ok_or(("no_unit", format!("{id} is not on the board")))
                .map(|u| self.session_of(u))?,
            None => weft_core::offers::resume_session(None, self.latest(harness)),
        };
        let s =
            session.ok_or(("no_session", weft_core::words::word("resume.nothing").to_string()))?;
        let h = self
            .files
            .find(harness)
            .ok_or(("no_harness", format!("Weft does not know how to start {harness}.")))?;
        Ok(Call::StartAgent {
            harness: harness.into(),
            spec: h.resume_spec(&s.id),
            session: Some(s.id),
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

    /// A work's Details, as the TUI draws them, from the parts the daemon read.
    pub fn detail(
        &self,
        unit: &Unit,
        source: Option<&str>,
        prompt: Option<&str>,
        record: Option<&weft_core::record::Record>,
        seal: Option<&Value>,
    ) -> Value {
        let at = self.units.iter().position(|u| u.ask_id == unit.ask_id);
        let debate = self.board(at, None, |b| b.deciding_harness(Act::Eval));
        let d = weft_core::offers::detail(unit, source, prompt, record, seal, debate.as_deref());
        serde_json::to_value(d).unwrap_or_default()
    }

    /// What a device's act asks the daemon (remote.md §6b): an Ask to the
    /// agent it names (`pane`), or an act on a work (`unit`). When the act
    /// cannot run, the refusal's code is the why-not's `words` key.
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
        let asking = matches!(act, Act::Ask | Act::FollowUp);
        let text = m["text"].as_str().filter(|t| !t.trim().is_empty()).map(str::to_string);
        if asking && text.is_none() {
            return Err(("bad_request", "an Ask needs its words".into()));
        }
        let unit = match (act, m["unit"].as_str()) {
            (Act::Ask, _) => None,
            (_, Some(id)) => Some(
                self.units
                    .iter()
                    .position(|u| u.ask_id == id)
                    .ok_or(("no_unit", format!("{id} is not on the board")))?,
            ),
            (_, None) => return Err(("bad_request", "an act on a work names its unit".into())),
        };
        if let Some(why) = self.board(unit, pane, |b| b.why_not(act)) {
            return Err((why.key, why.say()));
        }
        Ok(Call::Act {
            act: daemon.into(),
            unit: unit.map(|i| self.units[i].ask_id.clone()),
            pane: pane.filter(|_| asking),
            text: text.filter(|_| asking),
        })
    }

    /// Each pane's id on the wire and its turn, for push.
    pub fn turns(&self) -> Vec<(String, &'static str)> {
        (0..self.panes.len())
            .map(|i| (agent_id(&self.key, self.panes[i].pane), self.turn(i)))
            .collect()
    }
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
                {"harness": "claude", "id": "s_1", "first": 0, "latest": "working", "at": 0}]},
            "harnesses": [{"name": "claude", "title": "Claude Code"}],
            "units": [unit("ask_1", "fix login")],
            "readiness": {"claude": "ready"},
        });
        Project::opened(Path::new("/work/weft"), &answer)
    }

    fn works(tree: &Value) -> Vec<(String, String)> {
        tree["harnesses"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|h| h["works"].as_array().cloned().unwrap_or_default())
            .map(|w| {
                (w["title"].as_str().unwrap().to_string(), w["word"].as_str().unwrap().to_string())
            })
            .collect()
    }

    #[test]
    fn the_tree_names_each_harnesss_agent_and_each_works_word() {
        let p = opened();
        let tree = p.tree();
        assert_eq!(
            (tree["project"].clone(), tree["open"].clone(), tree["needs"].clone()),
            (json!("weft"), json!(1), json!(1))
        );
        let claude = &tree["harnesses"][0];
        assert_eq!(claude["agent"], agent_id(&p.key, 4));
        assert_eq!(
            (claude["title"].clone(), claude["turn"].clone()),
            (json!("Claude Code"), json!("waiting"))
        );
        assert_eq!(claude["works"][0]["turn"], "working", "the work's own session");
        assert_eq!(
            (claude["works"][0]["eval"].clone(), claude["works"][0]["seal"].clone()),
            (Value::Null, Value::Null)
        );
    }

    #[test]
    fn a_unit_added_an_eval_recorded_and_an_agent_ending_change_the_tree() {
        let mut p = opened();
        let mut units = vec![unit("ask_1", "fix login"), unit("ask_2", "telemetry")];
        assert!(p.heard(&Event::Units { units: units.clone(), records: json!({}) }));
        assert_eq!(
            works(&p.tree()),
            [("fix login".into(), "asked".into()), ("telemetry".into(), "asked".into())]
        );
        units[0].check = Some(weft_core::ledger::Check {
            eval_id: "evl_1".into(),
            verdict: weft_core::ledger::Verdict::Matches,
            agreement: 1.0,
            judged_by: None,
        });
        assert!(p.heard(&Event::Units { units, records: json!({}) }));
        assert_eq!(works(&p.tree())[0], ("fix login".into(), "evaled".into()));
        assert!(p.heard(&Event::Exited { pane: 4 }));
        assert_eq!(
            p.rows()[0]["running"],
            false,
            "a device tells an agent that ended from a turn that did"
        );
        let claude = &p.tree()["harnesses"][0];
        assert_eq!(
            (claude["running"].clone(), claude["agent"].clone(), claude["turn"].clone()),
            (json!(false), Value::Null, Value::Null)
        );
        assert_eq!(works(&p.tree()).len(), 2, "its open work stays");
    }

    #[test]
    fn a_harness_not_set_up_is_why_eval_and_seal_cannot_run() {
        let mut p = opened();
        assert!(p.heard(&Event::Readiness { states: json!({"claude": "no_plugin"}) }));
        let work = &p.tree()["harnesses"][0]["works"][0];
        assert_eq!(work["eval"], json!({"key": "why.not_set_up", "harness": "claude"}));
        assert_eq!(work["seal"], work["eval"]);
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

    #[test]
    fn a_device_folds_its_own_tree() {
        let tree = opened().tree();
        assert!(tree.get("folded").is_none() && tree["harnesses"][0].get("folded").is_none());
        assert_eq!(works(&tree).len(), 1, "nothing is folded on the wire");
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
        }
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
            (json!({"act": "eval", "unit": "ask_1"}), call("check", Some("ask_1"), None, None)),
            (json!({"act": "seal", "unit": "ask_1"}), call("decide", Some("ask_1"), None, None)),
            (json!({"act": "proceed", "unit": "ask_2"}), call("send", Some("ask_2"), None, None)),
        ] {
            assert_eq!(act(&p, m.clone()), Ok(want), "{m}");
        }
    }

    #[test]
    fn an_act_that_cannot_run_is_refused_with_its_why_not() {
        let mut p = opened();
        let refused = |p: &Project, m: Value| act(p, m).unwrap_err();
        assert_eq!(
            refused(&p, json!({"act": "proceed", "unit": "ask_1"})),
            (
                "why.nothing_to_send",
                "fix login has nothing to send. [F] FOLLOW UP asks for more.".into()
            )
        );
        assert_eq!(refused(&p, json!({"act": "eval", "unit": "ask_9"})).0, "no_unit");
        assert_eq!(refused(&p, json!({"act": "seal"})).0, "bad_request");
        assert_eq!(refused(&p, json!({"act": "ask", "text": " "})).0, "bad_request");
        assert_eq!(refused(&p, json!({"act": "start"})).0, "not_allowed");
        assert!(p.heard(&Event::Readiness { states: json!({"claude": "no_plugin"}) }));
        for m in [
            json!({"act": "eval", "unit": "ask_1"}),
            json!({"act": "seal", "unit": "ask_1"}),
            json!({"act": "ask", "text": "x"}),
        ] {
            assert_eq!(
                refused(&p, m),
                ("why.not_set_up", "claude is not set up for RingFrame.".into())
            );
        }
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

    #[test]
    fn a_works_details_say_where_its_debate_runs() {
        let mut p = opened();
        let mut u = unit("ask_1", "fix login");
        u.gathered =
            Some(weft_core::ledger::Gathered { eval_id: "evl_2".into(), by: "weft".into() });
        u.sent = weft_core::ledger::Sent::Arrived { exact: true };
        p.units = vec![u.clone()];
        let d = p.detail(&u, Some("Refresh it"), Some("Refresh the token"), None, None);
        assert_eq!(d["sections"][1]["state"], "GATHERED");
        assert_eq!(d["debate_in"], "claude");
        assert!(d["sections"][1]["lines"].to_string().contains("debate in claude"), "{d}");
        assert_eq!((d["proceed"].clone(), d["eval_view"].clone()), (json!(false), json!(true)));
    }

    #[test]
    fn work_whose_agent_is_gone_is_resumed_in_its_own_session() {
        let mut answer = json!({
            "panes": [{"pane": 4, "harness": "claude", "spec": "claude", "running": false}],
            "agents": {"panes": [null], "sessions": []},
            "harnesses": [{"name": "claude", "title": "Claude Code", "program": "claude", "resume": ["--resume"]}],
            "units": [unit("ask_1", "fix login")],
        });
        answer["harnesses"][0] = serde_json::to_value(
            weft_core::harness::fixture::harnesses().find("claude-code").unwrap(),
        )
        .unwrap();
        answer["harnesses"][0]["name"] = json!("claude");
        let mut p = Project::opened(Path::new("/work/weft"), &answer);
        assert_eq!(p.missing(&json!({"act": "eval", "unit": "ask_1"})).as_deref(), Some("claude"));
        assert_eq!(p.missing(&json!({"act": "ask", "text": "x"})), None, "an Ask names its agent");
        let s = p.session_of(p.unit("ask_1").unwrap()).expect("the session it was asked in");
        assert_eq!((s.id.as_str(), s.last.as_str()), ("s_1", "fix login"));
        let Ok(Call::StartAgent { harness, spec, session }) = p.resume("claude", Some("ask_1"))
        else {
            panic!()
        };
        assert_eq!((harness.as_str(), session.as_deref()), ("claude", Some("s_1")));
        assert!(spec.ends_with(" s_1"), "the harness file's resume command line: {spec}");
        let mut none = unit("ask_2", "telemetry");
        none.session_ref = None;
        assert_eq!(p.session_of(&none), None, "nothing to resume");

        // A tap on the harness itself (M2.9): its latest session, as the TUI's
        // Enter on a harness that runs no agent picks it up.
        assert_eq!(p.resume("claude", None).unwrap_err().0, "no_session", "none on record yet");
        assert_eq!(p.tree()["harnesses"][0]["session"], Value::Null);
        p.available(&json!({"starts": [
            {"harness": "claude", "spec": "claude --resume s_9", "session": {"id": "s_9", "at": 5, "last": "telemetry"}},
            {"harness": "claude", "spec": "claude", "session": null}]}));
        let Ok(Call::StartAgent { spec, session, .. }) = p.resume("claude", None) else { panic!() };
        assert_eq!(session.as_deref(), Some("s_9"));
        assert!(spec.ends_with(" s_9"), "{spec}");
        assert_eq!(p.tree()["harnesses"][0]["session"], json!({"at": 5, "last": "telemetry"}));
    }
}
