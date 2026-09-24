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
const DOMAIN_KEYS: [&str; 5] = ["domain", "base", "description", "concerns", "project_opted_in"];

pub fn candidates(items: &[Value]) -> Value {
    json!(items.iter().map(|i| pick(i, &ASK_KEYS)).collect::<Vec<_>>())
}

fn capability(data: &Value) -> Value {
    // The prefix and how it behaves travel together: a client that knows the
    // command but not whether it is a mode cannot decide how to submit it.
    let mut result = pick(
        data,
        &[
            "id",
            "selection",
            "effects",
            "delivery_mode",
            "continuation",
            "limitations",
            "requires_explicit_request_for_effects",
            "max_prompt_chars",
            "prompt_prefix",
            "prompt_prefix_kind",
            "prompt_prefix_active",
        ],
    );
    for (field, keys) in [
        ("confirmation", ["tool", "requires_feature"]),
        ("activation", ["mechanism", "tool"]),
        ("receipt", ["captured_by", "tool"]),
    ] {
        let source = data.get(field).cloned().unwrap_or_else(|| json!({}));
        let mut value = Map::new();
        for k in keys {
            match source.get(k) {
                Some(v) if !v.is_null() => {
                    value.insert(k.to_string(), v.clone());
                }
                _ => {}
            }
        }
        if !value.is_empty() {
            result[field] = Value::Object(value);
        }
    }
    result
}

fn profile(data: &Value) -> Value {
    let mut out = json!({
        "host": data["host"],
        "routing": pick(&data["routing"], &["guidance", "precedence"]),
        "capabilities": data["capabilities"].as_array().into_iter().flatten()
            .map(capability).collect::<Vec<_>>(),
    });
    if let Some(v) = data.get("paste_fold_chars") {
        out["paste_fold_chars"] = v.clone();
    }
    // Only when there are any: an empty list reads as a fact about the
    // project, and the commonest project has no plans at all.
    if data["plans"].as_array().is_some_and(|p| !p.is_empty()) {
        out["plans"] = data["plans"].clone();
    }
    out
}

fn eval_open(data: &Value) -> Value {
    let mut out = pick(data, &["eval_id", "brief_path", "changes"]);
    out["brief"] = pick(&data["brief"], &["sha256"]);
    out["anchor"] = data["anchor"]["ref"].clone();
    out["subject"] = data["subject"]["kind"].clone();
    out
}

fn eval_close(data: &Value) -> Value {
    let mut out =
        pick(data, &["eval_id", "verdict", "confidence", "items", "drift", "delta", "limitations"]);
    out["basis"] = pick(&data["basis"], &["unrecorded_prompts"]);
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

fn capture(data: &Value) -> Value {
    let mut result = pick(data, &["captured"]);
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
        ("profile", Some("show")) => profile(d),
        ("deltas", Some("domains")) => json!({
            "domains": d["domains"].as_array().into_iter().flatten()
                .map(|x| pick(x, &DOMAIN_KEYS)).collect::<Vec<_>>()
        }),
        ("deltas", Some("render")) => d["text"].clone(),
        ("ask", Some("list")) => {
            json!({"asks": candidates(d["asks"].as_array().map_or(&[], Vec::as_slice))})
        }
        ("eval", Some("list")) => json!({
            "evals": d["evals"].as_array().into_iter().flatten()
                .map(|e| pick(e, &["eval_id", "verdict", "confidence", "state", "basis"]))
                .collect::<Vec<_>>()
        }),
        ("seal", Some("check")) => pick(d, &["seal_id", "fresh", "subject_matches", "codes"]),
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
        ("ask", Some("compile")) => {
            pick(d, &["ask_id", "prompt_path", "delivery_mode", "source_verified"])
        }
        ("ask", Some("copy")) => d.clone(),
        ("ask", Some("preflight")) => pick(d, &["ready", "workspace"]),
        ("ask", Some("confirm")) => pick(d, &["ask_id", "confirmation"]),
        ("ask", Some("cancel")) => json!({"ask_id": d["ask_id"], "state": "cancelled"}),
        ("ask", Some("unanswered")) => pick(d, &["ask_id", "unanswered", "reason"]),
        ("ask", Some("submitted")) => pick(d, &["ask_id", "state", "as_modified"]),
        ("ask", Some("delivery")) => {
            if d.is_object() {
                pick(d, &["ask_id", "mode", "state", "recorded", "error"])
            } else {
                d.clone()
            }
        }
        ("eval", Some("open")) => eval_open(d),
        ("eval", Some("close")) => eval_close(d),
        ("seal", Some("create")) => seal_view(d),
        ("sessions", Some("capture")) => capture(d),
        ("fact", None) => pick(d, &["recorded", "fact_id", "outcome"]),
        ("sessions", Some("prune")) => pick(d, &["removed"]),
        ("export", None) => pick(d, &["ask_id", "out", "files"]),
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
