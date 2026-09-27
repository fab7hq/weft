//! Host capability profiles, read from the global config home.

use std::path::PathBuf;

use serde_json::Value;

use crate::config::{self, ConfigError};
use crate::workspace::PROFILE_SCHEMA;

fn dir() -> Result<PathBuf, ConfigError> {
    Ok(config::require_config()?.join("harnesses"))
}

pub fn load(name: &str) -> Result<Value, ConfigError> {
    let path = dir()?.join(format!("{name}.toml"));
    config::refuse_yaml(&path)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| ConfigError(format!("harnesses/{name}.toml: {e}")))?;
    let doc = config::load_toml_text(&text, &format!("harnesses/{name}.toml"))?;
    let found = doc.get("schema").and_then(Value::as_str);
    if found != Some(PROFILE_SCHEMA) {
        return Err(ConfigError(format!(
            "harnesses/{name}.toml declares {}; this release reads '{PROFILE_SCHEMA}'",
            found.map_or("None".to_string(), |s| format!("'{s}'"))
        )));
    }
    Ok(doc)
}

pub fn sha256(name: &str) -> Result<String, ConfigError> {
    Ok(config::sha256_of(&load(name)?))
}

pub fn names() -> Result<Vec<String>, ConfigError> {
    Ok(config::stems(&dir()?))
}

/// Every profile that loads. One that does not is skipped, with one warning
/// per profile, so it hides no other harness.
fn loaded() -> Result<Vec<Value>, ConfigError> {
    static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut out = Vec::new();
    for name in names()? {
        match load(&name) {
            Ok(p) => out.push(p),
            Err(e) => {
                let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
                if !warned.contains(&name) {
                    warned.push(name);
                    eprintln!("ringframe: skipping a profile that does not load: {e}");
                }
            }
        }
    }
    Ok(out)
}

/// Select integration rules by host identity; versions are provenance only.
pub fn for_host(host: &Value) -> Result<Value, ConfigError> {
    for p in loaded()? {
        if !p["host"].is_null() && p["host"] == host["name"] {
            return Ok(p);
        }
    }
    load("unknown")
}

/// The profile for a host by its id, or `unknown`'s.
pub fn of(host: &str) -> Result<Value, ConfigError> {
    for_host(&serde_json::json!({"name": host}))
}

/// Every profile that defines a harness: one with a `program` to start.
/// `unknown` defines none, so it is never offered (ADR-0018).
pub fn harnesses() -> Result<Vec<Value>, ConfigError> {
    Ok(loaded()?.into_iter().filter(|p| p["program"].is_string()).collect())
}

/// A host's facts, read from its profile. None when it names none, or
/// there is no profile to read.
fn fact(host: &str, key: &str) -> Option<String> {
    of(host).ok()?.get(key)?.as_str().map(str::to_string)
}

/// The name a person reads; the host's id when its profile gives none.
pub fn title(host: &str) -> String {
    fact(host, "title").unwrap_or_else(|| host.to_string())
}

/// The hook payload's session id field, from the host's profile.
pub fn session_field(host: &str) -> Option<String> {
    fact(host, "session_field")
}

/// The prefix a RingFrame skill is invoked with on this host.
pub fn invocation_prefix(host: &str) -> Option<String> {
    fact(host, "invocation_prefix")
}

/// The session a hook payload names, by the field the host's profile says.
pub fn session_of(host: &str, payload: &Value) -> String {
    session_field(host)
        .and_then(|f| payload.get(f)?.as_str().map(str::to_string))
        .unwrap_or_default()
}

pub fn capability<'a>(profile: &'a Value, cap_id: &str) -> Option<&'a Value> {
    profile["capabilities"].as_array()?.iter().find(|c| c["id"] == cap_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_config_home;
    use serde_json::json;

    /// One profile that will not load is skipped, and the others still
    /// define their harnesses and answer for their hosts.
    #[test]
    fn a_broken_profile_is_skipped_and_hides_no_other() {
        with_config_home(|_| {
            let dir = config::config_dir().join("harnesses");
            std::fs::write(dir.join("aaa-broken.toml"), "schema = [not toml").unwrap();
            std::fs::write(dir.join("zzz-old.toml"), "schema = \"ringframe.profile/0\"\n").unwrap();
            let hosts: Vec<Value> =
                harnesses().unwrap().into_iter().map(|p| p["host"].clone()).collect();
            assert!(
                hosts.contains(&json!("codex")) && hosts.contains(&json!("claude-code")),
                "{hosts:?}"
            );
            assert_eq!(of("codex").unwrap()["profile_id"], "codex");
            assert_eq!(of("nobody").unwrap()["profile_id"], "unknown");
        });
    }

    #[test]
    fn claude_profile_loads_and_digests() {
        with_config_home(|_| {
            let p = load("claude-code").unwrap();
            assert_eq!(p["profile_id"], "claude-code");
            let ids: std::collections::BTreeSet<&str> = p["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap())
                .collect();
            assert_eq!(ids, ["native_direct", "native_goal", "native_plan"].into());
            assert_eq!(sha256("claude-code").unwrap().len(), 64);
        });
    }

    #[test]
    fn a_known_host_keeps_its_capabilities_whatever_the_version_says() {
        with_config_home(|_| {
            for host in ["claude-code", "codex"] {
                for version in [
                    json!("0.1.0"),
                    json!("2.2.0"),
                    json!("10.0.0-beta.1"),
                    json!("codex-cli 0.153.4"),
                    json!("2.1.263 (Claude Code)"),
                    json!("development"),
                    json!(""),
                    json!(null),
                ] {
                    let p = for_host(&json!({"name": host, "version": version})).unwrap();
                    assert_eq!(p["profile_id"], host);
                    assert_eq!(p["version_range"], json!(null));
                    assert!(capability(&p, "native_plan").is_some());
                    assert_eq!(for_host(&json!({"name": host})).unwrap(), p);
                }
            }
        });
    }

    #[test]
    fn an_unrecognised_host_gets_manual_handoff() {
        with_config_home(|_| {
            for host in [json!("cursor"), json!("../codex"), json!(""), json!(null)] {
                let p = for_host(&json!({"name": host, "version": "2.2.0"})).unwrap();
                assert_eq!(p["profile_id"], "unknown");
                assert_eq!(p["fallback"], "human_handoff");
                assert!(capability(&p, "native_plan").is_none());
            }
        });
    }

    #[test]
    fn capability_lookup() {
        with_config_home(|_| {
            let p = load("claude-code").unwrap();
            let cap = capability(&p, "native_plan").unwrap();
            assert_eq!(cap["activation"]["tool"], "EnterPlanMode");
            assert_eq!(cap["delivery_mode"], "native_dispatch");
            assert!(capability(&p, "native_review").is_none());
            let direct = capability(&p, "native_direct").unwrap();
            assert!(
                direct["requires_explicit_request_for_effects"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("write"))
            );
        });
    }

    #[test]
    fn codex_is_handoff_only_with_request_user_input() {
        with_config_home(|_| {
            let p = load("codex").unwrap();
            assert_eq!(p["profile_id"], "codex");
            assert_eq!(p["confirmation"], json!({"tool": "request_user_input"}));
            let ids: std::collections::BTreeSet<&str> = p["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["id"].as_str().unwrap())
                .collect();
            assert_eq!(
                ids,
                ["native_direct", "native_goal", "native_plan", "native_review"].into()
            );
            for c in p["capabilities"].as_array().unwrap() {
                if c["id"] != "native_direct" {
                    assert_eq!(c["delivery_mode"], "human_handoff", "{}", c["id"]);
                    assert_eq!(c["activation"]["mechanism"], json!(null), "{}", c["id"]);
                }
            }
            let goal = capability(&p, "native_goal").unwrap();
            assert_eq!(goal["prompt_prefix"], "/goal ");
            assert_eq!(goal["max_prompt_chars"], 4000);
            assert_eq!(
                for_host(&json!({"name": "codex", "version": "codex-cli 0.153.1"})).unwrap()["profile_id"],
                "codex"
            );
        });
    }

    #[test]
    fn goal_is_a_capability_on_both_hosts_and_subagents_are_declared() {
        with_config_home(|_| {
            for name in ["claude-code", "codex"] {
                let p = load(name).unwrap();
                let goal = capability(&p, "native_goal").unwrap();
                assert_eq!(goal["delivery_mode"], "human_handoff", "{name}");
                assert_eq!(goal["prompt_prefix"], "/goal ", "{name}");
                assert_eq!(p["subagents"], true, "{name}");
            }
        });
    }
}
