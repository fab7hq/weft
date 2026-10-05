//! The agents on this machine, as a device sees them (remote.md §5, §6): one
//! list across every open project, each agent named by an id that stays the
//! same for as long as its pane lives, and its turn as the daemon reported
//! it. Nothing here works out NEEDS YOU: `waiting` is the daemon's word.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use weft_core::board::{Act, Board, PaneInfo};
use weft_core::ledger::Unit;
use weft_core::readiness::Readiness;
use weft_core::routing::Routing;
use weft_core::turns::{Session, Turn};
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
    /// The board, for the tree and the Ask an act from a device is about.
    units: Vec<Unit>,
    /// The sessions the hooks reported, for a work's own turn.
    sessions: Vec<Session>,
    /// What each harness is short of, by the daemon's word.
    readiness: Value,
    routing: Routing,
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

    /// The Ask a device's Eval or Seal for this pane is about: the latest of
    /// its harness's here that is neither cancelled nor sealed. The board has
    /// no pane for an Ask, so this is the nearest the record says.
    pub fn unit_for(&self, pane: u32) -> Option<&Unit> {
        let (harness, _) = self.harness(pane)?;
        self.units.iter().rev().find(|u| {
            u.harness == harness && !u.cancelled && u.sealed.is_none() && !u.requested_only
        })
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
                let mut row = json!({
                    "agent": agent_id(&self.key, p.pane),
                    "project": self.name,
                    "harness": harness,
                    "title": title,
                    "turn": self.turn(i),
                    "since": self.since.get(&p.pane).map_or(0, |(_, at)| *at),
                });
                if let Some(u) = self.unit_for(p.pane) {
                    let eval = u.check.as_ref().map(|c| c.eval_id.clone());
                    row["unit"] = json!({"id": u.ask_id, "title": u.title, "eval": eval});
                }
                row
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
        };
        let project = weft_core::tree::rows(&[facts], &|_, _| false).remove(0);
        let mut row = serde_json::to_value(&project).unwrap_or_default();
        for (h, at) in
            project.harnesses.iter().zip(row["harnesses"].as_array_mut().into_iter().flatten())
        {
            at["agent"] = h.pane.map(|p| agent_id(&self.key, p)).into();
        }
        row
    }

    /// Whether an Ask with this id is on this project's board.
    pub fn has_unit(&self, id: &str) -> bool {
        self.units.iter().any(|u| u.ask_id == id)
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
        let ready = |h: &str| Readiness::of_word(self.readiness[h].as_str());
        let board = Board {
            units: &self.units,
            selected: unit.unwrap_or(usize::MAX),
            panes: &self.panes,
            focused: pane.and_then(|p| self.panes.iter().position(|x| x.pane == p)).unwrap_or(0),
            readiness: &ready,
            routing: &self.routing,
            workspace_gap: None,
            agents: &self.turns,
        };
        if let Some(why) = board.why_not(act) {
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
}
