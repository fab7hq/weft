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

/// Select integration rules by host identity; versions are provenance only.
pub fn for_host(host: &Value) -> Result<Value, ConfigError> {
    for name in names()? {
        let p = load(&name)?;
        if !p["host"].is_null() && p["host"] == host["name"] {
            return Ok(p);
        }
    }
    load("unknown")
}

pub fn capability<'a>(profile: &'a Value, cap_id: &str) -> Option<&'a Value> {
    profile["capabilities"].as_array()?.iter().find(|c| c["id"] == cap_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::with_config_home;
    use serde_json::json;

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
