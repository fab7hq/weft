//! `config.get` and `config.set`: Weft's settings read and changed from any
//! client. A change is written to `config.toml` and applied to every open
//! project at once; what Weft would not read is refused, and nothing written.

use super::*;

/// Keys only the machine has: a project's would be ignored.
const MACHINE_ONLY: [&str; 5] = ["notify", "suggest", "service", "remote", "harnesses"];

impl Session {
    /// This project's settings: the file's machine tables and this project's
    /// as written, what applies here, and what Weft did not read.
    pub(super) fn config_json(&self, at: usize) -> serde_json::Value {
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        let p = &self.projects[at];
        let mut machine = text
            .parse::<toml::Table>()
            .ok()
            .and_then(|t| serde_json::to_value(t).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let project = machine["projects"].get(p.root.to_string_lossy().as_ref()).cloned();
        if let Some(tables) = machine.as_object_mut() {
            tables.remove("projects");
        }
        let read = crate::routing::read_at(&self.config, &p.harnesses);
        let ringframe =
            p.ringframe.as_deref().and_then(|o| serde_json::from_str::<serde_json::Value>(o).ok());
        serde_json::json!({
            "machine": machine,
            "project": project,
            "effective": {
                "routing": routing_json(&p.routing),
                "turbo": p.turbo,
                "notify": p.notify,
                "suggest": p.suggest_on,
                "ringframe": ringframe,
            },
            "unknown": read.unknown,
            "ignored": read.ignored,
            "leftover": read.leftover,
        })
    }

    /// One key set, or taken out, in `scope` (`machine` or `project`), then
    /// applied everywhere. Refused when Weft would not read the result.
    pub(super) fn change_config(
        &mut self,
        at: usize,
        scope: &str,
        key: &[String],
        value: Option<&serde_json::Value>,
    ) -> Answer {
        let Some(first) = key.first() else {
            return Answer::No("config", "which setting: the key is empty".into());
        };
        let root = self.projects[at].root.to_string_lossy().into_owned();
        let mut path: Vec<&str> = match scope {
            "machine" => Vec::new(),
            "project" if MACHINE_ONLY.contains(&first.as_str()) => {
                return Answer::No(
                    "config",
                    format!("{first} is this machine's: set it for every project"),
                );
            }
            "project" => vec!["projects", &root],
            other => {
                return Answer::No("config", format!("{other}: the scope is machine or project"));
            }
        };
        path.extend(key.iter().map(String::as_str));
        let known = &self.projects[at].harnesses;
        let before = std::fs::read_to_string(&self.config).unwrap_or_default();
        let was = weft_core::config::read(&before, known);
        let changed = crate::routing::change(&self.config, |text| {
            let out = weft_core::config::edit(text, &path, value)?;
            let now = weft_core::config::read(&out, known);
            let unread: Vec<&String> = now
                .unknown
                .iter()
                .chain(&now.ignored)
                .filter(|n| !was.unknown.contains(n) && !was.ignored.contains(n))
                .collect();
            if unread.is_empty() {
                Ok(out)
            } else {
                let names: Vec<&str> = unread.iter().map(|n| n.as_str()).collect();
                Err(format!("Weft would not read {}: nothing was changed", names.join(", ")))
            }
        });
        if let Err(why) = changed {
            return Answer::No("config", why);
        }
        self.reread_config();
        Answer::Ok(self.config_json(at))
    }

    /// `config.toml` read again for every project, and each project's clients
    /// told: its settings, and its turbo when that moved.
    fn reread_config(&mut self) {
        for at in 0..self.projects.len() {
            let p = &mut self.projects[at];
            let config = crate::routing::read_at(&self.config, &p.harnesses);
            p.routing = config.routing(&p.root);
            p.ringframe = config.ringframe_override(&p.root);
            p.notify = config.notify();
            p.suggest_on = config.suggest();
            let turbo = config.turbo(&p.root);
            let moved = turbo != p.turbo;
            p.turbo = turbo;
            if moved {
                self.clients.broadcast(at, &Out::Turbo { on: turbo });
            }
            let body = self.config_json(at);
            self.clients.broadcast(at, &Out::Config { config: body });
        }
    }
}
