//! CLI conversation views. Stored artifacts and full fetch results stay
//! unchanged.

use serde_json::{Map, Value, json};

use crate::workspace::Workspace;

pub fn pick(data: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for key in keys {
        if let Some(v) = data.get(*key) {
            out.insert((*key).to_string(), v.clone());
        }
    }
    Value::Object(out)
}

const ASK_KEYS: [&str; 4] = ["ask_id", "title", "state", "capability"];
const DOMAIN_KEYS: [&str; 4] = ["domain", "base", "description", "project_opted_in"];

pub fn candidates(items: &[Value]) -> Value {
    json!(items.iter().map(|i| pick(i, &ASK_KEYS)).collect::<Vec<_>>())
}

fn eval_open(data: &Value) -> Value {
    let mut out = pick(data, &["eval_id", "brief_path", "changes_patch", "agents", "changes"]);
    out["brief"] = pick(&data["brief"], &["sha256"]);
    out["anchor"] = data["anchor"]["ref"].clone();
    out["subject"] = data["subject"]["kind"].clone();
    out
}

fn seal_view(data: &Value) -> Value {
    let mut out = pick(data, &["seal_id", "disposition", "asks", "limitations"]);
    out["eval"] = if data["eval"].is_null() {
        Value::Null
    } else {
        pick(&data["eval"], &["eval_id", "verdict", "confidence", "subject_matches"])
    };
    out
}

fn observed(data: &Value) -> Value {
    let mut result =
        pick(data, &["recorded", "reason", "kind", "ask_id", "state", "fact_id", "outcome"]);
    match data.get("submission") {
        Some(s) if !s.is_null() => {
            result["submission"] = pick(s, &["ask_id", "state"]);
        }
        _ => {}
    }
    result
}

/// Every fetch has an explicit conversation view; no full-result fallback.
fn fetch_view(cmd: &str, sub: Option<&str>, d: &Value) -> Option<Value> {
    Some(match (cmd, sub) {
        ("deltas", Some("domains")) => json!({
            "domains": d["domains"].as_array().into_iter().flatten()
                .map(|x| pick(x, &DOMAIN_KEYS)).collect::<Vec<_>>()
        }),
        ("deltas", Some("list")) => json!({
            "catalogs": d["catalogs"].as_array().into_iter().flatten()
                .map(|c| json!({
                    "catalog": c["catalog"],
                    "entries": c["entries"].as_array().into_iter().flatten()
                        .map(|e| pick(e, &["id", "label", "tier", "status", "enabled", "source"]))
                        .collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "judges": d["judges"],
        }),
        ("ask", Some("list")) => {
            json!({"asks": candidates(d["asks"].as_array().map_or(&[], Vec::as_slice))})
        }
        ("eval", Some("list")) => json!({
            "evals": d["evals"].as_array().into_iter().flatten()
                .map(|e| pick(e, &["eval_id", "verdict", "confidence", "state", "basis", "brief", "gathered", "voided"]))
                .collect::<Vec<_>>()
        }),
        ("seal", Some("check")) => pick(d, &["seal_id", "fresh", "subject_matches", "codes"]),
        ("eval", Some("grep")) => pick(d, &["lines"]),
        ("ledger", Some("verify")) => pick(d, &["clean", "findings"]),
        _ => return None,
    })
}

/// Actions have one sufficient response, with no output-mode flags.
fn action_view(cmd: &str, sub: Option<&str>, d: &Value) -> Option<Value> {
    Some(match (cmd, sub) {
        ("init", None) => pick(d, &["rf_dir", "config", "revision"]),
        ("sync", None) if d.get("latest").is_some() => {
            pick(d, &["revision", "latest", "plugin", "behind"])
        }
        ("sync", None) => pick(d, &["config", "revision"]),
        ("eval", Some("open")) => eval_open(d),
        ("eval", Some("show")) => d.clone(),
        ("eval", Some("window")) => d.clone(),
        ("eval", Some("commits")) => d.clone(),
        ("eval", Some("task")) => d.clone(),
        ("eval", Some("submit")) => pick(d, &["task_id", "state", "attempt", "errors", "left"]),
        ("eval", Some("void")) => pick(d, &["eval_id", "state", "reason"]),
        ("seal", Some("create")) => seal_view(d),
        ("observe", _) => observed(d),
        _ => return None,
    })
}

pub fn is_action(cmd: &str, sub: Option<&str>) -> bool {
    action_view(cmd, sub, &Value::Null).is_some()
}

pub fn project(
    cmd: &str,
    sub: Option<&str>,
    minimal: bool,
    result: &Value,
    ws: &Workspace,
) -> Value {
    if fetch_view(cmd, sub, &json!({})).is_some() {
        return if minimal {
            fetch_view(cmd, sub, result).expect("a fetch view")
        } else {
            result.clone()
        };
    }
    let mut out = action_view(cmd, sub, result).expect("every command has a view");
    if (cmd, sub) == ("seal", Some("create")) {
        let seal_id = out["seal_id"].as_str().unwrap_or_default();
        out["receipt_path"] =
            json!(ws.rf_dir().join(format!("seals/{seal_id}.json")).to_string_lossy());
    }
    out
}
