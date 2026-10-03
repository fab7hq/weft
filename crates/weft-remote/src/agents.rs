//! The agents on this machine, as a device sees them (remote.md §5, §6): one
//! list across every open project, each agent named by an id that stays the
//! same for as long as its pane lives, and its turn as the daemon reported
//! it. Nothing here works out NEEDS YOU: `waiting` is the daemon's word.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use weft_core::board::PaneInfo;
use weft_core::ledger::Unit;
use weft_core::turns::Turn;
use weft_proto::Event;

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
    /// The board, for the Ask an Eval or a Seal from a device is about.
    units: Vec<Unit>,
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
            Event::Agents { panes, .. } => self.turns = panes.clone(),
            Event::Units { units, .. } => self.units = units.clone(),
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
}
