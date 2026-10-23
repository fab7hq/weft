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
    let layers = rules::with_repo(ws, req.overrides).map_err(config)?;
    let route = rules::route_rules(&layers, &host_name, req.capability, &classification)
        .map_err(config)?;
    let offered = rules::practice_rules(&layers, &tasks).map_err(config)?;
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
                "{} characters over the {m} that {} on {host_name} allows, with no practice \
                 rule left",
                typed_chars(&practices) - m,
                req.capability,
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

    fn read_prompt(ws: &Workspace, out: &Value) -> String {
        let id = str_of(out, "ask_id");
        std::fs::read_to_string(ws.rf_dir().join(format!("asks/{id}/prompt.txt"))).unwrap()
    }

    #[test]
    fn every_route_rule_appears_for_every_route_on_every_host() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let results = ["plan", "workspace_change", "answer", "continuing_objective"];
            let mut seen = 0;
            for host in crate::deltas::host_catalog_names().unwrap() {
                let profile = profiles::load(&host).unwrap();
                for cap in profile["capabilities"].as_array().unwrap() {
                    let capability = str_of(cap, "id");
                    for result in results {
                        let cls = json!({"task": ["implement"], "result": result, "effects": ["write"]});
                        let mut req = request(b"Build Phase 1 of plans/uptime/plan.md.", &over);
                        req.host = json!({"name": host});
                        req.capability = &capability;
                        req.classification = cls.clone();
                        req.plan = Some("plans/uptime/plan.md");
                        req.practices = Vec::new();
                        let prompt = read_prompt(ws, &compile(ws, req).unwrap());
                        let route = rules::route_rules(&over, &host, &capability, &cls).unwrap();
                        if route.is_empty() {
                            assert!(!prompt.contains(crate::deltas::THIS_ROUTE), "{prompt}");
                            continue;
                        }
                        let (_, block) = prompt.split_once("For this route:\n").expect("the block");
                        let want: Vec<String> = route
                            .iter()
                            .map(|r| format!("- {}: {}", crate::deltas::label(r),
                                             crate::deltas::squeeze(&str_of(r, "text"))))
                            .collect();
                        assert_eq!(block.lines().collect::<Vec<_>>(), want, "{host} {capability} {result}");
                        seen += want.len();
                    }
                }
            }
            assert!(seen > 0);
        });
    }

    #[test]
    fn the_words_then_the_practices_in_shipped_order_then_the_route() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let mut req = request(b"Build Phase 1 of plans/uptime/plan.md.\n", &over);
            req.capability = "native_goal";
            req.plan = Some("plans/uptime/plan.md");
            // Chosen in the reverse of shipped order.
            req.practices = vec!["practice.yagni".into(), "practice.kiss".into()];
            let prompt = read_prompt(ws, &compile(ws, req).unwrap());
            let shipped = rules::practice_rules(&over, &["implement".into()]).unwrap();
            let kiss = shipped.iter().find(|e| e["id"] == "practice.kiss").unwrap();
            let kiss_line =
                format!("- KISS: {}", crate::deltas::squeeze(&str_of(kiss, "text")));
            let at = |needle: &str| prompt.find(needle).unwrap_or_else(|| panic!("{needle}: {prompt}"));
            assert!(prompt.starts_with("Build Phase 1 of plans/uptime/plan.md.\n\nPlan: plans/uptime/plan.md\n\nRules:\n"));
            assert!(at(&kiss_line) < at("- YAGNI: "));
            assert!(at("- YAGNI: ") < at("For this route:\n- Step Loop: "));
            assert!(at("- Step Loop: ") < at("- Done Check: "));
        });
    }

    #[test]
    fn several_tasks_put_each_practice_under_its_phase() {
        eval_bench(|ws| {
            let over = Overrides::default();
            let mut req = request(b"Look into the cache, then fix it.", &over);
            req.classification = json!({"task": ["research", "implement"],
                                        "result": "workspace_change", "effects": ["write"]});
            req.practices = vec!["practice.occam".into(), "practice.testing_pyramid".into()];
            let prompt = read_prompt(ws, &compile(ws, req).unwrap());
            let at = |needle: &str| prompt.find(needle).unwrap_or_else(|| panic!("{needle}: {prompt}"));
            assert!(at("While researching:\n- Occam") < at("While implementing:\n- Testing Pyramid"));
            assert!(at("- Testing Pyramid") < at("For this route:"));
        });
    }

    #[test]
    fn a_practice_id_in_no_layer_is_refused() {
        eval_bench(|ws| {
            let over = Overrides::default();
            let mut req = request(b"Add an uptime check.", &over);
            req.practices = vec!["practice.kiss".into(), "practice.telepathy".into()];
            let e = compile(ws, req).unwrap_err().to_string();
            assert!(e.contains("ask.practice_unknown") && e.contains("practice.telepathy"), "{e}");
            assert!(crate::store::events(ws).unwrap().is_empty(), "nothing recorded");
        });
    }

    /// A goal of `n` characters of words, naming the plan.
    fn long_goal(n: usize) -> Vec<u8> {
        let head = "Build Phase 1 of plans/uptime/plan.md. ";
        format!("{head}{}", "x".repeat(n - head.len())).into_bytes()
    }

    #[test]
    fn a_goal_over_its_limit_drops_practices_lowest_score_first_and_keeps_every_route_rule() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let source = long_goal(2600);
            let mut req = request(&source, &over);
            req.capability = "native_goal";
            req.plan = Some("plans/uptime/plan.md");
            req.practices = ["practice.kiss", "practice.yagni", "practice.gall",
                             "practice.boy_scout", "practice.testing_pyramid"]
                .map(String::from).to_vec();
            // Boy Scout is the person's own addition: no score, so it stays.
            req.selected_by = json!({"by": "sense@0.3.0", "probabilities": {
                "practice.kiss": 0.4, "practice.yagni": 0.9, "practice.gall": 0.2,
                "practice.testing_pyramid": 0.6}});
            let out = compile(ws, req).unwrap();
            let trimmed: Vec<String> = list_of(&out, "trimmed");
            assert!(!trimmed.is_empty(), "the fixture must need trimming");
            let order = ["practice.gall", "practice.kiss", "practice.testing_pyramid", "practice.yagni"];
            assert_eq!(trimmed, order[..trimmed.len()], "lowest score first");
            let ask_id = str_of(&out, "ask_id");
            let typed = crate::ask::prompt_text(ws, &ask_id).unwrap();
            assert!(typed.trim_end().chars().count() <= 4000, "{}", typed.chars().count());
            assert!(typed.contains("- Boy Scout: "), "the person's rule stays");
            for kept in order[trimmed.len()..].iter() {
                let label = rules::practice_rules(&over, &["implement".into()]).unwrap()
                    .into_iter().find(|e| e["id"] == *kept).map(|e| crate::deltas::label(&e)).unwrap();
                assert!(typed.contains(&format!("- {label}: ")), "{kept} kept");
            }
            assert!(typed.contains("- Step Loop: ") && typed.contains("- Done Check: "));
            let d = &crate::store::events(ws).unwrap()[0]["data"];
            assert_eq!(list_of(&d["compiler"], "trimmed"), trimmed);
        });
    }

    #[test]
    fn a_goal_that_cannot_fit_without_practices_is_refused_with_the_overage() {
        eval_bench(|ws| {
            commit(&ws.root, &[("plans/uptime/plan.md", Some(PLAN))], "plan");
            let over = Overrides::default();
            let source = long_goal(4000);
            let mut req = request(&source, &over);
            req.capability = "native_goal";
            req.plan = Some("plans/uptime/plan.md");
            let e = compile(ws, req).unwrap_err().to_string();
            assert!(e.starts_with("ask.too_long"), "{e}");
            let over_by: usize = e.split_whitespace()
                .find_map(|w| w.parse().ok()).expect("the overage in characters");
            let route = rules::route_rules(&over, "claude-code", "native_goal", &implement()).unwrap();
            let bare = rules::compose(&String::from_utf8_lossy(&source),
                                      &["Plan: plans/uptime/plan.md".into()], &[], &route,
                                      &["implement".into()]);
            assert_eq!(over_by, "/goal ".len() + bare.trim_end().chars().count() - 4000, "{e}");
            assert!(crate::store::events(ws).unwrap().is_empty(), "nothing recorded");
        });
    }

    #[test]
    fn no_limit_drops_nothing() {
        eval_bench(|ws| {
            let over = Overrides::default();
            let source = long_goal(6000);
            let out = compile(ws, request(&source, &over)).unwrap();
            assert_eq!(out["trimmed"], json!([]));
        });
    }
}
