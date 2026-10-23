//! An Ask compiled from the person's words as they wrote them, with the
//! practice rules they chose and every rule of the route, and confirmed in
//! the same write: pressing Send is the confirmation.

use serde_json::{Value, json};

use crate::ask::{AskError, check_links, delivery_block, event, head, ledger, list_of, str_of};
use crate::config::Overrides;
use crate::workspace::Workspace;
use crate::{digest, ids, profiles, rules, schema, store, workspace};

/// What the person sent.
pub struct Request<'a> {
    /// The words, verbatim.
    pub source: &'a [u8],
    /// The harness: `name`, and its `version` and `session_ref` when known.
    pub host: Value,
    pub capability: &'a str,
    /// `task`, `result` and `effects`.
    pub classification: Value,
    /// The plan the work follows, by its path.
    pub plan: Option<&'a str>,
    /// The practice rules chosen, by id.
    pub practices: Vec<String>,
    /// Who classified it and who chose the rules: `{"by": "person"}`, or
    /// `{"by": "sense@<version>", "probabilities": {…}}`.
    pub classified_by: Value,
    pub selected_by: Value,
    pub links: Vec<Value>,
    pub overrides: &'a Overrides,
}

const CLASSIFIED: [&str; 3] = ["task", "result", "effects"];

/// The ledger keeps what decides the route and the rules, and nothing else.
fn classification(given: &Value) -> Value {
    let kept = CLASSIFIED.iter().filter_map(|k| Some((k.to_string(), given.get(*k)?.clone())));
    Value::Object(kept.collect())
}

/// How strongly the chooser wanted a rule: Sense's probability, or above
/// every probability for a rule the person added.
fn score(selected_by: &Value, id: &str) -> f64 {
    selected_by["probabilities"][id].as_f64().unwrap_or(f64::INFINITY)
}

/// The plan, when one is named, must be one this workspace holds; a route
/// rule that works a plan needs one.
fn plan_line(ws: &Workspace, plan: Option<&str>, route: &[Value]) -> Result<Option<String>, AskError> {
    let held: Vec<String> =
        workspace::plans(ws).iter().filter_map(|p| p["path"].as_str().map(str::to_string)).collect();
    match plan {
        Some(path) if held.iter().any(|h| h == path) => Ok(Some(format!("Plan: {path}"))),
        Some(path) => Err(ledger(
            "ask.plan_path",
            format!("{path} is not a plan this workspace holds; held: {}", held.join(", ")),
        )),
        None if route.iter().any(|r| r["names_plan"] == json!(true)) => Err(ledger(
            "ask.plan_path",
            "this route's rules work a plan: name one this workspace holds",
        )),
        None => Ok(None),
    }
}

/// Compile and confirm. Returns the Ask's id, the prompt as stored and the
/// practice rules dropped to fit the host's limit.
pub fn compile(ws: &Workspace, req: Request<'_>) -> Result<Value, AskError> {
    workspace::require_git(ws)?;
    let source = req.source;
    if source.iter().all(u8::is_ascii_whitespace) || std::str::from_utf8(source).is_err() {
        return Err(ledger("ask.source", "the words must be non-empty UTF-8"));
    }
    check_links(ws, &req.links)?;
    let classification = classification(&req.classification);
    let tasks = list_of(&classification, "task");
    let host_name = str_of(&req.host, "name");
    let profile = profiles::for_host(&req.host)?;
    let Some(cap) = profiles::capability(&profile, req.capability).cloned() else {
        return Err(ledger(
            "ask.capability",
            format!("'{}' is not in profile {}", req.capability, str_of(&profile, "profile_id")),
        ));
    };
    let config = |e: crate::config::ConfigError| ledger("ask.rules", e.0);
    let route = rules::route_rules(req.overrides, &host_name, req.capability, &classification)
        .map_err(config)?;
    let offered = rules::practice_rules(req.overrides, &tasks).map_err(config)?;
    if let Some(unknown) = req.practices.iter().find(|id| !offered.iter().any(|e| e["id"] == **id)) {
        return Err(ledger(
            "ask.practice_unknown",
            format!("'{unknown}' is not a practice rule for {tasks:?} in any layer"),
        ));
    }
    // Shipped order, whatever order they were chosen in.
    let mut practices: Vec<Value> =
        offered.into_iter().filter(|e| req.practices.iter().any(|id| e["id"] == *id)).collect();
    let mut context = Vec::new();
    if let Some(l) = req.links.iter().find(|l| str_of(l, "rel") == "remediates") {
        context.push(format!(
            "Read .fab7/rf/evals/{}/eval.md first: it lists what the last Eval found.",
            str_of(l, "id")
        ));
    }
    context.extend(plan_line(ws, req.plan, &route)?);
    let words = String::from_utf8_lossy(source);
    let prefix = str_of(&cap, "prompt_prefix");
    let compose = |p: &[Value]| rules::compose(&words, &context, p, &route, &tasks);
    let typed_chars = |p: &[Value]| prefix.chars().count() + compose(p).trim_end().chars().count();
    let max = cap.get("max_prompt_chars").and_then(Value::as_u64).map(|m| m as usize);
    let trimmed = rules::trim(
        &mut practices,
        |id| score(&req.selected_by, id),
        |p| max.is_none_or(|m| typed_chars(p) <= m),
    );
    if let Some(m) = max
        && typed_chars(&practices) > m
    {
        return Err(ledger(
            "ask.too_long",
            format!(
                "{} on {host_name} allows {m} characters; with no practice rule this is {} over",
                req.capability,
                typed_chars(&practices) - m
            ),
        ));
    }
    let prompt = compose(&practices).into_bytes();
    let typed = [prefix.as_bytes(), &prompt].concat();

    let ask_id = ids::new_id("ask");
    let reference = |role: &str, file: &str, bytes: &[u8]| {
        json!({"role": role, "path": format!("asks/{ask_id}/{file}"),
               "bytes": bytes.len(), "sha256": digest::sha256_bytes(bytes)})
    };
    let source_ref = reference("source_intent", "source.txt", source);
    let prompt_ref = reference("generated_prompt", "prompt.txt", &prompt);
    let mut limitations = list_of(&cap, "limitations");
    if str_of(&profile, "profile_id") == "unknown" {
        limitations.push("qualification gap: no profile for this host".into());
    }
    let (baselines, unreadable) = crate::evidence::baselines(ws, &[&words, &compose(&practices)]);
    limitations.extend(unreadable);
    let ids_of = |list: &[Value]| list.iter().map(|e| str_of(e, "id")).collect::<Vec<_>>();
    let data = json!({
        "title": crate::ask::requested_title(source),
        "classification": classification,
        "classified_by": req.classified_by,
        "selected_by": req.selected_by,
        "selected_capability": req.capability,
        "route_explanation": null,
        "host": {
            "name": host_name,
            "version": req.host.get("version").cloned().unwrap_or(Value::Null),
            "surface": "weft",
            "session_ref": req.host.get("session_ref").cloned().unwrap_or(Value::Null),
            "workspace": ws.describe(),
            "profile_id": str_of(&profile, "profile_id"),
            "profile_sha256": profiles::sha256(
                profile.get("host").and_then(Value::as_str).unwrap_or("unknown"))?,
        },
        "source": source_ref, "prompt": prompt_ref,
        "source_verified": "exact", "limitations": limitations,
        "delivery_mode": cap["delivery_mode"],
        "delivery": delivery_block(&profile, &cap, &typed),
        "compiler": {
            "source": "verbatim", "plan": req.plan, "practices": ids_of(&practices),
            "trimmed": trimmed, "route": ids_of(&route),
        },
        "base_commit": head(ws), "baselines": baselines,
    });
    let person = json!({"kind": "human", "id": "local-user", "authority": "interactive"});
    let compiled = event("ask.compiled", &ask_id, &person, data, &req.links);
    let confirmed = event(
        "ask.confirmed",
        &ask_id,
        &person,
        json!({"confirmation": {"by": "person", "via": "weft", "observed_by": "weft"}}),
        &[],
    );
    schema::validate_event(&compiled)?;
    schema::validate_event(&confirmed)?;
    for (r, bytes) in [(&source_ref, source), (&prompt_ref, &prompt[..])] {
        let wrote = store::publish(ws, &str_of(r, "path"), bytes, &str_of(r, "role"))?;
        assert_eq!(&wrote, r, "the published file is not what was recorded");
    }
    store::append_all(ws, &[compiled, confirmed])?;
    Ok(json!({
        "ask_id": ask_id,
        "prompt": String::from_utf8_lossy(&prompt),
        "prompt_path": ws.rf_dir().join(str_of(&prompt_ref, "path")).to_string_lossy(),
        "trimmed": trimmed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{commit, eval_bench};

    pub(crate) const PLAN: &str = "# Uptime\n\n## Phase 1\n\n| # | Step | Done when |\n\
                                   | --- | --- | --- |\n| 1 | Add `uptime`. | A test calls it. |\n";

    fn implement() -> Value {
        json!({"task": ["implement"], "result": "workspace_change", "effects": ["write"]})
    }

    pub(crate) fn request<'a>(source: &'a [u8], over: &'a Overrides) -> Request<'a> {
        Request {
            source,
            host: json!({"name": "claude-code", "version": "2.1.260"}),
            capability: "native_plan",
            classification: implement(),
            plan: None,
            practices: vec!["practice.kiss".into(), "practice.yagni".into()],
            classified_by: json!({"by": "person"}),
            selected_by: json!({"by": "sense@0.3.0",
                                "probabilities": {"practice.kiss": 0.9, "practice.yagni": 0.7}}),
            links: Vec::new(),
            overrides: over,
        }
    }

    fn ledger_lines(ws: &Workspace) -> Vec<Value> {
        let raw = std::fs::read_to_string(ws.rf_dir().join("ledger.jsonl")).unwrap();
        raw.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn one_append_holds_the_compile_and_its_confirmation() {
        eval_bench(|ws| {
            let over = Overrides::default();
            // Bytes a rewording would change: a blank line, a dash, trailing spaces.
            let source = "Add an uptime check.  \n\nIt answers 200 — nothing else.\n".as_bytes();
            let out = compile(ws, request(source, &over)).unwrap();
            let ask_id = str_of(&out, "ask_id");
            let stored = std::fs::read(ws.rf_dir().join(format!("asks/{ask_id}/source.txt")));
            assert_eq!(stored.unwrap(), source);

            let lines = ledger_lines(ws);
            assert_eq!(lines.len(), 2, "one compile, one confirmation, nothing else");
            let (compiled, confirmed) = (&lines[0], &lines[1]);
            assert_eq!((compiled["type"].as_str(), confirmed["type"].as_str()),
                       (Some("ask.compiled"), Some("ask.confirmed")));
            assert_eq!((&compiled["id"], &confirmed["id"]), (&json!(ask_id), &json!(ask_id)));
            assert_eq!(confirmed["data"]["confirmation"]["by"], "person");
            assert_eq!(confirmed["data"]["confirmation"]["via"], "weft");
            let d = &compiled["data"];
            assert_eq!(d["classified_by"], json!({"by": "person"}));
            assert_eq!(d["selected_by"]["by"], "sense@0.3.0");
            assert_eq!(d["selected_by"]["probabilities"]["practice.kiss"], 0.9);
            assert_eq!(d["classification"], implement());
            assert_eq!(d["source_verified"], "exact");
            assert_eq!(d["compiler"]["source"], "verbatim");
            assert_eq!(d["compiler"]["practices"], json!(["practice.kiss", "practice.yagni"]));
            // The Ask reads as confirmed.
            let listed = crate::ask::list_asks(ws).unwrap();
            assert_eq!(listed[0]["outcome"], "confirmed", "{listed:?}");
        });
    }

    #[test]
    fn the_prompt_is_typed_with_its_command_in_front() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let mut req = request(b"Build the uptime plan.", &over);
            req.capability = "native_goal";
            req.plan = Some("plans/uptime/plan.md");
            let out = compile(ws, req).unwrap();
            let ask_id = str_of(&out, "ask_id");
            let stored = std::fs::read_to_string(ws.rf_dir().join(format!("asks/{ask_id}/prompt.txt")));
            let stored = stored.unwrap();
            assert!(stored.starts_with("Build the uptime plan.\n"), "{stored}");
            assert_eq!(str_of(&out, "prompt"), stored);
            let typed = crate::ask::prompt_for_host(ws, &ask_id, "claude-code").unwrap();
            assert_eq!(typed["prompt"], json!(format!("/goal {stored}")));
            assert_eq!(crate::ask::prompt_body(ws, &ask_id).unwrap(), stored);
            assert_eq!(typed["delivery"]["prefix_bytes"], 6);
            // The hook sees what was typed, and knows it for this Ask.
            let seen = crate::digest::sha256_bytes(format!("/goal {stored}").as_bytes());
            let rec = crate::ask::submission_as(ws, "claude-code", "s1", &seen, "hook", None);
            let rec = rec.unwrap().expect("the typed prompt is this Ask's");
            assert_eq!(rec["ask_id"], json!(ask_id), "{rec}");
        });
    }
}
