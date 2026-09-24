//! Event validation: required keys, enums, and no unknown top-level keys.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::store::{LedgerError, SCHEMA};

const TOP: [&str; 8] = ["schema", "event_id", "type", "time", "id", "actor", "links", "data"];

/// The enumerations, by the path they are reached at. A `None` in the list
/// means the key may be present and null.
fn enum_values(key: &str) -> &'static [&'static str] {
    match key {
        "actor.kind" => &["agent", "human", "policy"],
        "links[].rel" => &["evaluates", "follows", "remediates", "revises", "seals", "supersedes"],
        "fact.outcome" => &["failed", "succeeded"],
        "classification.task[]" => &[
            "clarify",
            "diagnose",
            "document",
            "implement",
            "operate",
            "plan",
            "question",
            "research",
            "review",
        ],
        "classification.result" => {
            &["answer", "continuing_objective", "evidence", "plan", "workspace_change"]
        }
        "classification.interaction" => &["approval_gated", "interactive"],
        "classification.horizon" => &["one_turn", "persistent", "session"],
        "classification.effects[]" => &[
            "execute",
            "external_effect",
            "external_read",
            "read",
            "workspace_read",
            "workspace_write",
            "write",
        ],
        "data.delivery_mode" => &["human_handoff", "native_dispatch", "unsupported"],
        "data.source_verified" => &["exact", "unverified"],
        "data.mode" => &["human_handoff", "native_dispatch"],
        "data.mechanism" => &["capability_activate", "prompt_submit"],
        "data.state" => &["delivery_failed", "handoff_ready", "native_accepted", "unavailable"],
        "data.submission" => &["not_applicable", "unobserved"],
        "submission.state" => &["attributed", "observed"],
        "data.verdict" => &["aligned", "drifted", "incomplete"],
        "data.disposition" => &["abandoned", "accepted", "deferred", "rejected"],
        other => panic!("no enumeration for {other}"),
    }
}

/// `data.mechanism` is the one enumeration that admits null.
fn nullable(key: &str) -> bool {
    key == "data.mechanism"
}

fn required(event_type: &str) -> Option<Vec<&'static str>> {
    const ASK_COMMON: [&str; 9] = [
        "title",
        "classification",
        "selected_capability",
        "route_explanation",
        "host",
        "source",
        "prompt",
        "source_verified",
        "limitations",
    ];
    let mut out: Vec<&'static str> = match event_type {
        // `delivery` is written but not required: a record from before it
        // exists reads exactly as it did, and a reader treats its absence as
        // "no mode".
        "ask.compiled" => vec!["delivery_mode"],
        // An observation, not an outcome: the Ask stays open after it.
        "ask.unanswered" => return Some(vec!["unanswered"]),
        "ask.confirmed" => return Some(vec!["confirmation"]),
        "ask.cancelled" => return Some(vec!["cancellation"]),
        "ask.submission" => {
            return Some(vec![
                "state",
                "observed_by",
                "attributed_by",
                "as_modified",
                "host",
                "prompt_sha256",
            ]);
        }
        "ask.delivery" => {
            return Some(vec![
                "mode",
                "mechanism",
                "state",
                "qualification",
                "receipt",
                "submission",
                "limitations",
            ]);
        }
        "eval.opened" => return Some(vec!["brief", "basis", "anchor", "subject"]),
        "eval.gathered" => return Some(vec!["eval_id", "context_map", "host"]),
        "tool.fact" => return Some(vec!["command", "outcome", "subject", "host", "session_ref"]),
        "eval.completed" => {
            return Some(vec![
                "basis",
                "subject",
                "verdict",
                "confidence",
                "artifact",
                "limitations",
            ]);
        }
        "seal.created" => {
            return Some(vec!["basis", "eval", "subject", "disposition", "authority", "artifact"]);
        }
        "seal.refused" => {
            return Some(vec![
                "basis",
                "eval",
                "subject",
                "disposition",
                "authority",
                "refusal_codes",
            ]);
        }
        _ => return None,
    };
    let mut all = ASK_COMMON.to_vec();
    all.append(&mut out);
    Some(all)
}

/// Every event type the record can hold. A type with no required-key list is
/// not a type this release writes or reads.
pub const EVENT_TYPES: [&str; 12] = [
    "ask.compiled",
    "ask.confirmed",
    "ask.cancelled",
    "ask.unanswered",
    "ask.submission",
    "ask.delivery",
    "eval.opened",
    "eval.gathered",
    "eval.completed",
    "seal.created",
    "seal.refused",
    "tool.fact",
];

fn fail(path: &str, why: &str) -> LedgerError {
    LedgerError::schema(format!("{path} {why}"))
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn check_enum(path: &str, key: &str, value: &Value) -> Result<(), LedgerError> {
    let allowed = enum_values(key);
    let shown = format!("{allowed:?}").replace('"', "'");
    match value {
        Value::Null if nullable(key) => Ok(()),
        Value::Null => Err(fail(path, &format!("must be one of {shown}, got None"))),
        Value::String(s) if allowed.contains(&s.as_str()) => Ok(()),
        Value::String(s) => Err(fail(path, &format!("must be one of {shown}, got '{s}'"))),
        other => Err(fail(path, &format!("must be one of {shown}, got {}", type_name(other)))),
    }
}

fn check_enum_list(path: &str, key: &str, values: &Value) -> Result<(), LedgerError> {
    let allowed = format!("{:?}", enum_values(key)).replace('"', "'");
    let Some(items) = values.as_array() else {
        return Err(fail(
            path,
            &format!("must be a list from {allowed}, got {}", type_name(values)),
        ));
    };
    for v in items {
        check_enum(&format!("{path}[]"), key, v)?;
    }
    Ok(())
}

fn check_ref(path: &str, value: &Value) -> Result<(), LedgerError> {
    let ok = value
        .as_object()
        .is_some_and(|m| ["role", "path", "bytes", "sha256"].iter().all(|k| m.contains_key(*k)));
    if ok { Ok(()) } else { Err(fail(path, "is not an artifact reference")) }
}

fn get<'a>(obj: &'a Value, key: &str, path: &str) -> Result<&'a Value, LedgerError> {
    obj.get(key).ok_or_else(|| fail(path, "missing"))
}

pub fn validate_event(ev: &Value) -> Result<(), LedgerError> {
    let Some(map) = ev.as_object() else {
        return Err(fail("event", "must be a mapping"));
    };
    let extra: BTreeSet<&str> =
        map.keys().map(String::as_str).filter(|k| !TOP.contains(k)).collect();
    if !extra.is_empty() {
        return Err(fail(&extra.into_iter().collect::<Vec<_>>().join(","), "extra top-level key"));
    }
    for k in TOP {
        if !map.contains_key(k) {
            return Err(fail(k, "missing"));
        }
    }
    if map["schema"].as_str() != Some(SCHEMA) {
        return Err(fail("schema", &format!("must be {SCHEMA}")));
    }
    let event_type = map["type"].as_str().unwrap_or_default();
    let Some(required) = required(event_type) else {
        return Err(fail("type", "unknown event type"));
    };
    for k in ["kind", "id"] {
        if map["actor"].get(k).is_none() {
            return Err(fail(&format!("actor.{k}"), "missing"));
        }
    }
    check_enum("actor.kind", "actor.kind", &map["actor"]["kind"])?;
    for (i, link) in map["links"].as_array().into_iter().flatten().enumerate() {
        if link.get("rel").is_none() || link.get("id").is_none() {
            return Err(fail(&format!("links[{i}]"), "missing"));
        }
        check_enum(&format!("links[{i}].rel"), "links[].rel", &link["rel"])?;
    }
    let data = &map["data"];
    for k in &required {
        if data.get(k).is_none() {
            return Err(fail(&format!("data.{k}"), "missing"));
        }
    }
    match event_type {
        "ask.compiled" => validate_compiled(data)?,
        "ask.confirmed" | "ask.cancelled" | "ask.unanswered" => {
            let field = match event_type {
                "ask.confirmed" => "confirmation",
                "ask.cancelled" => "cancellation",
                _ => "unanswered",
            };
            let grade = &data[field];
            let graded = grade
                .as_object()
                .is_some_and(|g| g.contains_key("observed_by") || g.contains_key("attributed_by"));
            if !graded {
                return Err(fail(&format!("data.{field}"), "needs observed_by or attributed_by"));
            }
        }
        "ask.submission" => {
            check_enum("data.state", "submission.state", &data["state"])?;
            if truthy(&data["observed_by"]) || truthy(&data["attributed_by"]) {
            } else {
                return Err(fail("data.observed_by", "or attributed_by is required"));
            }
        }
        "ask.delivery" => {
            for k in ["mode", "mechanism", "state", "submission"] {
                check_enum(&format!("data.{k}"), &format!("data.{k}"), &data[k])?;
            }
            if data["qualification"].get("id").is_none() {
                return Err(fail("data.qualification.id", "missing"));
            }
        }
        "eval.opened" => check_ref("data.brief", &data["brief"])?,
        "eval.gathered" => check_ref("data.context_map", &data["context_map"])?,
        "tool.fact" => check_enum("data.outcome", "fact.outcome", &data["outcome"])?,
        "eval.completed" => {
            check_enum("data.verdict", "data.verdict", &data["verdict"])?;
            let ok = data["confidence"].as_f64().is_some_and(|c| (0.0..=1.0).contains(&c));
            if !ok {
                return Err(fail("data.confidence", "must be a number in [0, 1]"));
            }
            check_ref("data.artifact", &data["artifact"])?;
        }
        _ => {
            check_enum("data.disposition", "data.disposition", &data["disposition"])?;
            if event_type == "seal.created" {
                check_ref("data.artifact", &data["artifact"])?;
            }
        }
    }
    Ok(())
}

/// Truthiness for the two fields that are checked for it: `null`, `false`,
/// and an empty string or collection are false; everything else is true.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}

fn validate_compiled(data: &Value) -> Result<(), LedgerError> {
    let c = &data["classification"];
    for k in ["task", "result", "interaction", "horizon", "effects"] {
        get(c, k, &format!("data.classification.{k}"))?;
    }
    check_enum_list("data.classification.task", "classification.task[]", &c["task"])?;
    for k in ["result", "interaction", "horizon"] {
        check_enum(&format!("data.classification.{k}"), &format!("classification.{k}"), &c[k])?;
    }
    check_enum_list("data.classification.effects", "classification.effects[]", &c["effects"])?;
    if let Some(v) = c.get("concerns")
        && !v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(fail(
            "data.classification.concerns",
            "must be a list of strings from the domain vocabulary",
        ));
    }
    if let Some(v) = c.get("domains")
        && !v.as_array().is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(fail(
            "data.classification.domains",
            "must be a list of installed practice domain names",
        ));
    }
    if let Some(comp) = data.get("compiler").filter(|v| !v.is_null()) {
        let ok = comp
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| ["prompt", "body", "composed"].contains(&s));
        if !ok {
            return Err(fail("data.compiler", "must be {source: prompt|body|composed, ...}"));
        }
    }
    for k in
        ["name", "version", "surface", "session_ref", "workspace", "profile_id", "profile_sha256"]
    {
        if data["host"].get(k).is_none() {
            return Err(fail(&format!("data.host.{k}"), "missing"));
        }
    }
    check_ref("data.source", &data["source"])?;
    check_ref("data.prompt", &data["prompt"])?;
    check_enum("data.source_verified", "data.source_verified", &data["source_verified"])?;
    check_enum("data.delivery_mode", "data.delivery_mode", &data["delivery_mode"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-01-01T00:00:00.000Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "u", "authority": "interactive"}, "links": []
        })
    }

    fn reference() -> Value {
        json!({"role": "source_intent", "path": "asks/ask_1/source.txt", "bytes": 1,
               "sha256": "a".repeat(64)})
    }

    /// A complete `ask.compiled`, which every other case starts from.
    fn compiled() -> Value {
        let mut ev = base();
        ev["data"] = json!({
            "delivery_mode": "native_dispatch", "title": "t",
            "classification": {"task": ["plan"], "result": "plan", "interaction": "interactive",
                               "horizon": "session", "effects": ["read"]},
            "selected_capability": "native_plan",
            "route_explanation": {"fits": "x", "alternatives": [], "continuation": "c",
                                  "effects": "e", "gaps": []},
            "host": {"name": "claude-code", "version": "2.1.260", "surface": "native-tui",
                     "session_ref": null, "workspace": {"root": "/w", "rule": "cwd"},
                     "profile_id": "claude-code", "profile_sha256": "b".repeat(64)},
            "source": reference(),
            "prompt": {"role": "generated_prompt", "path": "asks/ask_1/prompt.txt", "bytes": 1,
                       "sha256": "a".repeat(64)},
            "source_verified": "unverified", "limitations": []
        });
        ev
    }

    fn refuses(ev: &Value, expect: &str) {
        let e = validate_event(ev).expect_err("should have been refused");
        assert_eq!(e.code, "ledger.schema");
        assert!(e.detail.contains(expect), "wanted {expect:?}, got {:?}", e.detail);
    }

    #[test]
    fn valid_event_passes() {
        validate_event(&compiled()).unwrap();
    }

    #[test]
    fn missing_required_key_reports_path() {
        let mut ev = compiled();
        ev["data"].as_object_mut().unwrap().remove("prompt");
        refuses(&ev, "data.prompt");
    }

    #[test]
    fn unknown_top_level_key_rejected() {
        let mut ev = compiled();
        ev["extra"] = json!(1);
        refuses(&ev, "extra");
    }

    #[test]
    fn bad_enum_rejected() {
        let mut ev = compiled();
        ev["data"]["classification"]["result"] = json!("poem");
        refuses(&ev, "classification.result");
    }

    #[test]
    fn delivery_requires_mode_state_and_qualification() {
        let mut ev = base();
        ev["type"] = json!("ask.delivery");
        ev["data"] = json!({
            "mode": "human_handoff", "mechanism": null, "state": "handoff_ready",
            "qualification": {"id": null},
            "receipt": {"path": "asks/ask_1/prompt.txt", "emitted_by": "cli"},
            "submission": "unobserved", "limitations": []
        });
        validate_event(&ev).unwrap();
        ev["data"]["state"] = json!("sent");
        refuses(&ev, "data.state");
    }

    #[test]
    fn every_event_type_has_a_required_key_list() {
        for t in EVENT_TYPES {
            assert!(required(t).is_some(), "{t} has no required-key list");
        }
        assert!(required("ask.imagined").is_none());
    }

    #[test]
    fn confirmed_cancelled_and_submission_are_graded_observations() {
        let mut ev = base();
        ev["type"] = json!("ask.confirmed");
        ev["data"] =
            json!({"confirmation": {"observed_by": "skill", "surface": "AskUserQuestion"}});
        validate_event(&ev).unwrap();

        ev["type"] = json!("ask.cancelled");
        ev["data"] =
            json!({"cancellation": {"attributed_by": "human:local-user"}, "reason": "later"});
        validate_event(&ev).unwrap();

        ev["type"] = json!("ask.submission");
        ev["data"] = json!({"state": "observed", "observed_by": "hook:UserPromptSubmit",
                            "attributed_by": null, "as_modified": false,
                            "host": {"name": "claude-code", "session_ref": "s"},
                            "prompt_sha256": "a".repeat(64)});
        validate_event(&ev).unwrap();

        ev["data"] = json!({"state": "probably", "observed_by": null, "attributed_by": "human:x",
                            "as_modified": false, "host": {}, "prompt_sha256": "a".repeat(64)});
        refuses(&ev, "data.state");

        ev["type"] = json!("ask.confirmed");
        ev["data"] = json!({});
        refuses(&ev, "data.confirmation");
    }

    #[test]
    fn wrong_shapes_report_the_expected_shape_not_an_internal_error() {
        // A string where a list belongs must never be iterated character by
        // character, and a list where a string belongs must never reach an
        // internal type error.
        for (field, value, expect) in [
            ("task", json!("implement"), "task must be a list"),
            ("result", json!(["workspace_change"]), "result must be one of"),
            ("effects", json!("write"), "effects must be a list"),
            ("horizon", json!(null), "horizon must be one of"),
        ] {
            let mut ev = compiled();
            ev["data"]["classification"][field] = value;
            refuses(&ev, expect);
        }
    }
}
