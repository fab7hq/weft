//! `config.get` and `config.set`: Weft's settings read and changed from any
//! client. A change is written to `config.toml` and applied to every open
//! project at once; what Weft would not read is refused, and nothing written.
//! A RingFrame setting is also RingFrame's to read: it is asked first.

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

    /// `config.set`: a `ringframe` key is asked of RingFrame first, off the
    /// run loop; any other is changed at once.
    pub(super) fn set_config(
        &mut self,
        client: u64,
        call_id: u64,
        at: usize,
        scope: String,
        key: Vec<String>,
        value: Option<serde_json::Value>,
    ) -> Answer {
        if key.first().map(String::as_str) != Some("ringframe") {
            return self.change_config(at, &scope, &key, value.as_ref());
        }
        let path = match self.path_of(at, &scope, &key) {
            Ok(path) => path,
            Err(no) => return no,
        };
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        let known = &self.projects[at].harnesses;
        let candidate = match checked_edit(&text, &path, value.as_ref(), known) {
            Ok(candidate) => candidate,
            Err(why) => return Answer::No("config", why),
        };
        let root = self.projects[at].root.clone();
        let Some(layers) = weft_core::config::read(&candidate, known).ringframe_override(&root)
        else {
            return self.change_config(at, &scope, &key, value.as_ref());
        };
        let (tx, out) = (self.tx.clone(), self.outside.clone());
        std::thread::spawn(move || {
            let wake = match crate::ringframe::reads_overrides(out.as_ref(), &root, &layers) {
                Ok(()) => Wake::ConfigChecked { client, call_id, project: at, scope, key, value },
                Err(e) => {
                    Wake::Answer { client, call_id, answer: Answer::No("config", refusal(e)) }
                }
            };
            let _ = tx.send(wake);
        });
        Answer::Later
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
        let path = match self.path_of(at, scope, key) {
            Ok(path) => path,
            Err(no) => return no,
        };
        let known = &self.projects[at].harnesses;
        let changed =
            crate::routing::change(&self.config, |text| checked_edit(text, &path, value, known));
        if let Err(why) = changed {
            return Answer::No("config", why);
        }
        self.reread_config();
        Answer::Ok(self.config_json(at))
    }

    /// Where `key` lives in `config.toml` for `scope`.
    fn path_of(&self, at: usize, scope: &str, key: &[String]) -> Result<Vec<String>, Answer> {
        let Some(first) = key.first() else {
            return Err(Answer::No("config", "which setting: the key is empty".into()));
        };
        let mut path = match scope {
            "machine" => Vec::new(),
            "project" if MACHINE_ONLY.contains(&first.as_str()) => {
                return Err(Answer::No(
                    "config",
                    format!("{first} is this machine's: set it for every project"),
                ));
            }
            "project" => {
                vec!["projects".to_string(), self.projects[at].root.to_string_lossy().into_owned()]
            }
            other => {
                return Err(Answer::No(
                    "config",
                    format!("{other}: the scope is machine or project"),
                ));
            }
        };
        path.extend(key.iter().cloned());
        Ok(path)
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

/// `text` with the value at `path` set or taken out, refused when Weft would
/// not read something the change brought.
fn checked_edit(
    text: &str,
    path: &[String],
    value: Option<&serde_json::Value>,
    known: &weft_core::harness::Harnesses,
) -> Result<String, String> {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let was = weft_core::config::read(text, known);
    let out = weft_core::config::edit(text, &path, value)?;
    let now = weft_core::config::read(&out, known);
    let unread: Vec<&str> = now
        .unknown
        .iter()
        .chain(&now.ignored)
        .filter(|n| !was.unknown.contains(n) && !was.ignored.contains(n))
        .map(String::as_str)
        .collect();
    if unread.is_empty() {
        Ok(out)
    } else {
        Err(format!("Weft would not read {}: nothing was changed", unread.join(", ")))
    }
}

/// What RingFrame said when it would not read the settings, in its words.
fn refusal(e: crate::ringframe::Error) -> String {
    match e {
        crate::ringframe::Error::NotInstalled => {
            "RingFrame is not installed, so its settings cannot be checked: nothing was changed"
                .into()
        }
        crate::ringframe::Error::Refused { message, .. } => {
            let said = serde_json::from_str::<serde_json::Value>(&message)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or(message);
            format!("RingFrame would not read it ({said}): nothing was changed")
        }
    }
}
