//! Delta catalogs: host deltas keyed by (host, capability); practice deltas
//! keyed by classification.
//!
//! The CLI selects TOML rules; the model adapts them to the task. Deltas guide
//! the work but do not run checks.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::config::{self, ConfigError, Overrides};

pub const SCHEMA: &str = "ringframe.deltas/1";
pub const HOST_STATUS: [&str; 3] = ["candidate", "qualified", "retired"];
pub const PRACTICE_STATUS: [&str; 4] = ["attributed", "candidate", "qualified", "retired"];
pub const DEFAULT_DOMAIN: &str = "software-development";

fn check(ok: bool, message: impl Into<String>) -> Result<(), ConfigError> {
    if ok { Ok(()) } else { Err(ConfigError(message.into())) }
}

fn dir() -> Result<PathBuf, ConfigError> {
    Ok(config::require_config()?.join("deltas"))
}

pub fn host_catalog_names() -> Result<Vec<String>, ConfigError> {
    Ok(config::stems(&dir()?))
}

/// Domains from the synced mirror and from the override layers.
pub fn domain_names(over: &Overrides) -> Result<Vec<String>, ConfigError> {
    let mut seen: BTreeSet<String> = config::stems(&dir()?.join("practices")).into_iter().collect();
    seen.extend(over.domains().map(str::to_string));
    Ok(seen.into_iter().collect())
}

pub struct Layer {
    pub root: String,
    pub path: String,
    pub sha256: String,
    pub document: Value,
}

/// The synced bundle, then each `--override` layer in order. Later layers win
/// by id. `relative` is the document's path under `deltas/`, without the
/// extension.
pub(crate) fn layers(over: &Overrides, relative: &str) -> Result<Vec<Layer>, ConfigError> {
    let mut docs: Vec<(String, String, Value)> = Vec::new();
    let synced = dir()?.join(format!("{relative}.toml"));
    if synced.exists() || synced.with_extension("yaml").exists() {
        docs.push(("config".into(), synced.to_string_lossy().into(), config::load_toml(&synced)?));
    }
    for (name, doc) in over.documents(relative) {
        docs.push((name.into(), format!("deltas/{relative}"), doc.clone()));
    }
    // An empty document inherits; only one with content is a layer.
    Ok(docs
        .into_iter()
        .filter(|(_, _, doc)| doc.as_object().is_some_and(|m| !m.is_empty()))
        .map(|(root, path, document)| Layer {
            root,
            path,
            sha256: config::sha256_of(&document),
            document,
        })
        .collect())
}

fn catalog(relative: &str, over: &Overrides) -> Result<Value, ConfigError> {
    let mut merged = Value::Object(Map::new());
    for layer in layers(over, relative)? {
        merged = config::merge(&merged, &layer.document);
    }
    Ok(merged)
}

/// Every catalog, the hosts' then the practices', its entries as the layers
/// leave them: each with `source`, the layer that last set it (`shipped`, or
/// an override layer's name), and the catalog's `render`. A catalog the layers
/// leave unreadable is refused, as any command reading it would.
pub fn list(over: &Overrides) -> Result<Vec<Value>, ConfigError> {
    let mut catalogs = Vec::new();
    for host in host_catalog_names()? {
        load_host_catalog(&host, over)?;
        catalogs.push(host);
    }
    for domain in domain_names(over)? {
        load_practice_catalog(&domain, over)?;
        catalogs.push(format!("practices/{domain}"));
    }
    let mut out = Vec::new();
    for relative in catalogs {
        let mut merged = Value::Object(Map::new());
        let mut source = std::collections::HashMap::new();
        for layer in layers(over, &relative)? {
            let name = if layer.root == "config" { "shipped" } else { layer.root.as_str() };
            for e in entries(&layer.document) {
                if let Some(id) = e.get("id").and_then(Value::as_str) {
                    source.insert(id.to_string(), name.to_string());
                }
            }
            merged = config::merge(&merged, &layer.document);
        }
        let listed: Vec<Value> = entries(&merged)
            .iter()
            .map(|e| {
                let mut e = e.clone();
                let id = text_of(&e, "id");
                e["source"] = json!(source.get(&id));
                e
            })
            .collect();
        out.push(json!({"catalog": relative, "render": merged.get("render"), "entries": listed}));
    }
    Ok(out)
}

fn entries(cat: &Value) -> &[Value] {
    cat.get("entries").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub fn load_host_catalog(host: &str, over: &Overrides) -> Result<Value, ConfigError> {
    let cat = catalog(host, over)?;
    check(
        cat.get("schema").and_then(Value::as_str) == Some(SCHEMA)
            && cat.get("scope").and_then(Value::as_str) == Some("host")
            && cat.get("host").and_then(Value::as_str) == Some(host),
        format!("deltas/{host}.toml: not a host catalog"),
    )?;
    for e in entries(&cat) {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("None");
        check(
            ["id", "label", "capability", "text", "matrix_ref", "status"]
                .iter()
                .all(|k| e.get(*k).is_some()),
            format!("deltas/{host}.toml: entry {id} incomplete"),
        )?;
        let status = text_of(e, "status");
        check(
            HOST_STATUS.contains(&status.as_str()),
            format!("deltas/{host}.toml: {id} status '{status}'"),
        )?;
    }
    Ok(cat)
}

pub fn load_practice_catalog(domain: &str, over: &Overrides) -> Result<Value, ConfigError> {
    let relative = format!("practices/{domain}");
    let cat = catalog(&relative, over)?;
    check(
        cat.get("schema").and_then(Value::as_str) == Some(SCHEMA)
            && cat.get("scope").and_then(Value::as_str) == Some("practice"),
        format!("deltas/practices/{domain}.toml: not a practice catalog"),
    )?;
    // A rule that ships, or that a team shares, says when it matters and
    // which family it is of. The person's own rules come only from their
    // layers, and may say less.
    let shared: BTreeSet<String> = layers(over, &relative)?
        .iter()
        .filter(|l| l.root == "config" || l.root == "repo")
        .flat_map(|l| entries(&l.document).iter().map(|e| text_of(e, "id")).collect::<Vec<_>>())
        .collect();
    // Nothing here judges the content. Catalogs live on the user's machine and
    // are theirs to edit; a rule that never matches is the author's choice to
    // make. Only the structure RingFrame must read is required.
    for e in entries(&cat) {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("None");
        check(
            ["id", "text", "applies_to"].iter().all(|k| e.get(*k).is_some()),
            format!("practice entry {id} incomplete"),
        )?;
        // Refused rather than ignored: ignoring it would quietly widen the
        // person's rule to every route.
        check(
            e["applies_to"].get("capability").is_none(),
            format!(
                "practice entry {id}: applies_to.capability names a route; route rules are \
                 host entries (deltas/<host>.toml)"
            ),
        )?;
        if shared.contains(id) {
            check(
                ["label", "when", "family"].iter().all(|k| !text_of(e, k).trim().is_empty()),
                format!(
                    "practice entry {id}: a shipped or shared rule needs label, when and family"
                ),
            )?;
            let family = text_of(e, "family");
            check(
                crate::rules::FAMILIES.contains(&family.as_str()),
                format!(
                    "practice entry {id}: family '{family}' is not one of {}",
                    crate::rules::FAMILIES.join(", ")
                ),
            )?;
        }
    }
    Ok(cat)
}

/// The base domain, then each pack a layer switches on by naming it.
pub fn domains_on(over: &Overrides) -> Result<Vec<String>, ConfigError> {
    let mut out = vec![DEFAULT_DOMAIN.to_string()];
    for name in domain_names(over)? {
        if name != DEFAULT_DOMAIN && over.documents(&format!("practices/{name}")).next().is_some() {
            out.push(name);
        }
    }
    Ok(out)
}

/// Merged entries in catalog order, annotated with the last scope defining
/// each. An ordered list rather than a map, because the render order is the
/// catalog's.
pub fn effective(over: &Overrides, domain: &str) -> Result<Vec<(String, Value)>, ConfigError> {
    let cat = load_practice_catalog(domain, over)?;
    let mut merged: Vec<(String, Value)> = entries(&cat)
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e["layer"] = json!("user");
            (text_of(&e, "id"), e)
        })
        .collect();
    for layer in layers(over, &format!("practices/{domain}"))? {
        for entry in entries(&layer.document) {
            let id = text_of(entry, "id");
            if let Some((_, e)) = merged.iter_mut().find(|(k, _)| *k == id) {
                e["layer"] = json!(layer.root);
            }
        }
    }
    Ok(merged)
}

fn any_shared(a: &Value, b: &Value) -> bool {
    let set: BTreeSet<&str> =
        b.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    a.as_array().into_iter().flatten().filter_map(Value::as_str).any(|x| set.contains(x))
}

fn non_empty_list(v: Option<&Value>) -> bool {
    v.and_then(Value::as_array).is_some_and(|a| !a.is_empty())
}

/// Whether an entry's `applies_to` matches the classification. A host entry
/// is keyed by its capability as well; a route that serves two kinds of work
/// (`native_plan` for a plan, and for bounded work) tells them apart here.
pub(crate) fn matches(entry: &Value, classification: &Value) -> bool {
    let a = entry.get("applies_to").cloned().unwrap_or_else(|| json!({}));
    if non_empty_list(a.get("task"))
        && !any_shared(&a["task"], classification.get("task").unwrap_or(&Value::Null))
    {
        return false;
    }
    if non_empty_list(a.get("result")) {
        let want = classification.get("result").unwrap_or(&Value::Null);
        if !a["result"].as_array().into_iter().flatten().any(|v| v == want) {
            return false;
        }
    }
    if non_empty_list(a.get("effects"))
        && !any_shared(&a["effects"], classification.get("effects").unwrap_or(&Value::Null))
    {
        return false;
    }
    true
}

/// Installed practice domains, and whether a layer switches each on.
pub fn domains(over: &Overrides) -> Result<Vec<Value>, ConfigError> {
    let mut out = Vec::new();
    for name in domain_names(over)? {
        let cat = load_practice_catalog(&name, over)?;
        // Opting in is an override layer naming the domain.
        let opted_in = over.documents(&format!("practices/{name}")).next().is_some();
        out.push(json!({
            "domain": name,
            "base": name == DEFAULT_DOMAIN,
            "description": text_of(&cat, "description").split_whitespace().collect::<Vec<_>>().join(" "),
            "project_opted_in": opted_in,
            "sha256": config::sha256_of(&cat),
        }));
    }
    Ok(out)
}

/// How a phase reads in the rendered block. An Ask naming several tasks gets
/// one group per task, so a rule meant for research is not weighed against one
/// meant for implementation.
pub const PHASES: [(&str, &str); 9] = [
    ("question", "When answering:"),
    ("research", "While researching:"),
    ("clarify", "When clarifying:"),
    ("plan", "While planning:"),
    ("implement", "While implementing:"),
    ("diagnose", "While diagnosing:"),
    ("review", "While reviewing:"),
    ("operate", "While operating:"),
    ("document", "When documenting:"),
];
pub const EVERY_PHASE: &str = "Throughout:";
/// The host's route rules: what this turn delivers, the order of the work,
/// when it stops. They lead and are not mixed in with the principles — a
/// deliverable listed third of ten reads like an aside.
pub const THIS_ROUTE: &str = "For this route:";

pub(crate) fn phase_heading(task: &str) -> Option<&'static str> {
    PHASES.iter().find(|(t, _)| *t == task).map(|(_, h)| *h)
}

pub fn revision() -> String {
    std::fs::read_to_string(config::config_dir().join(".revision"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "local".into())
}

pub fn label(entry: &Value) -> String {
    for key in ["label", "principle"] {
        let v = text_of(entry, key);
        if !v.is_empty() {
            return v;
        }
    }
    let id = text_of(entry, "id");
    id.rsplit('.').next().unwrap_or(&id).to_string()
}

pub(crate) fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn status_of(entry: &Value) -> String {
    let s = text_of(entry, "status");
    if s.is_empty() { "attributed".into() } else { s }
}

/// Which named task this rule belongs to, or every one of them.
pub(crate) fn phase_of(entry: &Value, tasks: &[String]) -> String {
    let declared: Vec<String> = entry
        .get("applies_to")
        .and_then(|a| a.get("task"))
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_else(|| tasks.to_vec());
    let matched: Vec<&String> = tasks.iter().filter(|t| declared.contains(t)).collect();
    if matched.len() == tasks.len() {
        EVERY_PHASE.to_string()
    } else {
        matched.first().map_or_else(|| EVERY_PHASE.to_string(), |t| (*t).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles;
    use crate::testing::{TempDir, repo, with_config_home, ws_for};
    use crate::workspace::Workspace;
    use std::path::Path;

    fn impl_task() -> Value {
        json!({"task": ["implement"], "result": "workspace_change",
               "interaction": "approval_gated", "horizon": "session", "effects": ["write"]})
    }

    /// The whole fixture: an isolated config home and a workspace in a repo.
    fn bench<T>(body: impl FnOnce(&Workspace, &Path) -> T) -> T {
        with_config_home(|home| {
            let repo = repo();
            let ws = ws_for(repo.path());
            body(&ws, home)
        })
    }

    fn entry<'a>(list: &'a [(String, Value)], id: &str) -> &'a Value {
        &list.iter().find(|(k, _)| k == id).unwrap_or_else(|| panic!("no {id}")).1
    }

    #[test]
    fn shipped_catalogs_validate_and_have_provenance() {
        with_config_home(|_| {
            for name in host_catalog_names().unwrap() {
                let cat = load_host_catalog(&name, &Overrides::default()).unwrap();
                assert_eq!(cat["schema"], SCHEMA);
                assert_eq!(cat["scope"], "host");
                assert_eq!(cat["host"], name);
                for e in entries(&cat) {
                    let id = text_of(e, "id");
                    assert!(
                        id.starts_with(&format!("{name}.{}.", text_of(e, "capability"))),
                        "{id}"
                    );
                    assert!(!text_of(e, "matrix_ref").is_empty(), "{id}");
                    assert!(!text_of(e, "label").is_empty(), "{id}");
                    assert!(HOST_STATUS.contains(&text_of(e, "status").as_str()), "{id}");
                }
            }
            // Every host with a profile has its route rules.
            assert_eq!(host_catalog_names().unwrap(), ["antigravity", "claude-code", "codex"]);
            let prac = load_practice_catalog(DEFAULT_DOMAIN, &Overrides::default()).unwrap();
            assert_eq!(prac["scope"], "practice");
            assert_eq!(prac["render"], json!({"heading": "Rules:"}));
            let all: Vec<String> = entries(&prac).iter().map(|e| text_of(e, "id")).collect();
            let unique: BTreeSet<&String> = all.iter().collect();
            assert_eq!(all.len(), unique.len(), "duplicate entry ids");
            for e in entries(&prac) {
                let id = text_of(e, "id");
                assert!(!text_of(e, "principle").is_empty(), "{id}");
                assert!(!text_of(e, "text").trim().is_empty(), "{id}");
                assert!(!text_of(e, "when").trim().is_empty(), "{id}");
                assert!(crate::rules::FAMILIES.contains(&text_of(e, "family").as_str()), "{id}");
                assert!(PRACTICE_STATUS.contains(&status_of(e).as_str()), "{id}");
                // Directives, never principle names.
                let text = text_of(e, "text");
                assert!(!text.contains("SOLID") && !text.contains("YAGNI"), "{id}");
            }
        });
    }

    fn over(layers: Value) -> Overrides {
        Overrides::parse(&layers.to_string()).unwrap()
    }

    #[test]
    fn override_layers_apply_in_order_by_id() {
        bench(|_, _| {
            let mut doc = config::load_toml(
                &config::config_dir().join("deltas/practices/software-development.toml"),
            )
            .unwrap();
            let list = doc["entries"].as_array_mut().unwrap();
            list.iter_mut()
                .find(|e| e["id"] == "practice.kiss")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("text".into(), json!("Keep it plain."));
            list.push(json!({
                "id": "practice.team.commit_style", "tier": "core",
                "applies_to": {"task": ["implement"]},
                "text": "One commit per item, message names the item."
            }));
            let layers = over(json!([
                {"layer": "weft", "ringframe": {"deltas": {"practices/software-development": doc}}},
                {"layer": "weft-project", "ringframe": {"deltas": {"practices/software-development":
                    {"entries": [{"id": "practice.yagni", "enabled": false}]}}}},
            ]));

            let chosen = crate::rules::practice_rules(&layers, &["implement".into()]).unwrap();
            let chosen: Vec<String> = chosen.iter().map(|e| text_of(e, "id")).collect();
            assert!(!chosen.contains(&"practice.yagni".to_string()));
            assert!(chosen.contains(&"practice.team.commit_style".to_string()));
            let named: Vec<(String, String)> =
                super::layers(&layers, "practices/software-development")
                    .unwrap()
                    .iter()
                    .map(|l| (l.root.clone(), l.path.clone()))
                    .collect();
            let synced = config::config_dir().join("deltas/practices/software-development.toml");
            assert_eq!(
                named,
                [
                    ("config".into(), synced.to_string_lossy().to_string()),
                    ("weft".into(), "deltas/practices/software-development".into()),
                    ("weft-project".into(), "deltas/practices/software-development".into()),
                ]
            );
            let listing = effective(&layers, DEFAULT_DOMAIN).unwrap();
            assert_eq!(entry(&listing, "practice.kiss")["layer"], "weft");
            assert_eq!(entry(&listing, "practice.kiss")["text"], "Keep it plain.");
            assert_eq!(entry(&listing, "practice.yagni")["layer"], "weft-project");
            assert_eq!(entry(&listing, "practice.yagni")["enabled"], false);
        });
    }

    // ---- catalog reachability and task coverage ----------------------------

    const TASKS: [&str; 9] = [
        "question",
        "research",
        "clarify",
        "plan",
        "implement",
        "diagnose",
        "review",
        "operate",
        "document",
    ];

    fn probe(task: &str) -> Value {
        json!({"task": [task], "result": "plan", "effects": ["read"]})
    }

    #[test]
    fn a_host_entry_without_a_label_is_refused() {
        bench(|_, _| {
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {"codex": {
                "entries": [{"id": "codex.native_direct.bare", "capability": "native_direct",
                             "text": "Do it.", "matrix_ref": "x", "status": "candidate"}]}}}}]));
            let e = load_host_catalog("codex", &layers).unwrap_err();
            assert!(e.0.contains("codex.native_direct.bare incomplete"), "{e}");
        });
    }

    #[test]
    fn a_practice_entry_that_names_a_route_is_refused() {
        bench(|_, _| {
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [{
                    "id": "practice.team.routed", "tier": "core", "text": "Stop.",
                    "applies_to": {"capability": ["native_plan"]}}]}}}}]));
            let e = load_practice_catalog(DEFAULT_DOMAIN, &layers).unwrap_err();
            assert!(e.0.contains("route rules are host entries"), "{e}");
        });
    }

    /// The classifications that reach each route in practice.
    fn route_probes() -> Vec<(&'static str, Value)> {
        let mut plan = impl_task();
        plan["result"] = json!("plan");
        let mut answer = probe("question");
        answer["result"] = json!("answer");
        let mut objective = impl_task();
        objective["result"] = json!("continuing_objective");
        vec![
            ("native_plan", plan),
            ("native_plan", impl_task()),
            ("native_goal", impl_task()),
            ("native_goal", objective),
            ("native_direct", impl_task()),
            ("native_direct", answer),
        ]
    }

    /// The rules of a host's route a classification reaches, whatever their
    /// status.
    fn reached(host: &str, capability: &str, cls: &Value) -> Vec<String> {
        entries(&load_host_catalog(host, &Overrides::default()).unwrap())
            .iter()
            .filter(|e| text_of(e, "capability") == capability && matches(e, cls))
            .map(|e| text_of(e, "id"))
            .collect()
    }

    #[test]
    fn a_host_rule_selects_by_result() {
        // `native_plan` is also the route for ordinary bounded work, which
        // must not be told to write a plan and stop.
        bench(|_, _| {
            for host in ["antigravity", "claude-code", "codex"] {
                let mut plan = impl_task();
                plan["result"] = json!("plan");
                assert_eq!(
                    reached(host, "native_plan", &plan),
                    [format!("{host}.native_plan.plan_as_files")]
                );
                assert_eq!(
                    reached(host, "native_plan", &impl_task()),
                    [format!("{host}.native_plan.after_approval")]
                );
            }
        });
    }

    #[test]
    fn every_host_rule_can_be_selected() {
        bench(|_, _| {
            for host in host_catalog_names().unwrap() {
                let got: BTreeSet<String> =
                    route_probes().iter().flat_map(|(cap, cls)| reached(&host, cap, cls)).collect();
                let all: BTreeSet<String> =
                    entries(&load_host_catalog(&host, &Overrides::default()).unwrap())
                        .iter()
                        .map(|e| text_of(e, "id"))
                        .collect();
                assert_eq!(got, all, "{host}");
            }
        });
    }

    #[test]
    fn a_goal_prompt_with_its_route_rules_fits() {
        // A goal shares its character budget with the words and the rules;
        // the route rules alone leave room for both.
        bench(|_, _| {
            for host in ["antigravity", "claude-code", "codex"] {
                let profile = profiles::load(host).unwrap();
                let cap = profiles::capability(&profile, "native_goal").unwrap().clone();
                let Some(max) = cap.get("max_prompt_chars").and_then(Value::as_u64) else {
                    continue;
                };
                let catalog = load_host_catalog(host, &Overrides::default()).unwrap();
                let route: Vec<Value> = entries(&catalog)
                    .iter()
                    .filter(|e| {
                        text_of(e, "capability") == "native_goal" && matches(e, &impl_task())
                    })
                    .cloned()
                    .collect();
                assert_eq!(route.len(), 2, "{host}");
                let words = "b".repeat(1500);
                let prompt = crate::rules::compose(&words, &[], &[], &route, &["implement".into()]);
                let n = (text_of(&cap, "prompt_prefix") + &prompt).chars().count() as u64;
                assert!(n <= max, "{host}: {n} of {max}");
            }
        });
    }

    #[test]
    fn every_task_has_a_practice_rule_and_named_ones_are_for_their_task() {
        bench(|_, _| {
            let for_task = |task: &str| -> Vec<String> {
                crate::rules::practice_rules(&Overrides::default(), &[task.to_string()])
                    .unwrap()
                    .iter()
                    .map(|e| text_of(e, "id"))
                    .collect()
            };
            for task in TASKS {
                assert!(!for_task(task).is_empty(), "{task} selects no rule");
            }
            for (task, rule) in [
                ("research", "practice.occam"),
                ("review", "practice.linus"),
                ("question", "practice.confirmation_bias"),
                ("implement", "practice.testing_pyramid"),
            ] {
                assert!(for_task(task).contains(&rule.to_string()), "{rule} missing for {task}");
            }
        });
    }

    // ---- packs ------------------------------------------------------------

    fn second_domain() -> &'static str {
        r#"schema = "ringframe.deltas/1"
scope = "practice"
domain = "fixture-domain"
description = "An invented pack used only by tests."
render = { heading = "Rules:" }

[[entries]]
id = "practice.widget_first"
label = "Widget First"
applies_to = { task = ["implement"] }
when = "Widgets are built."
family = "domain"
text = "Build the widget before the housing."

[[entries]]
id = "practice.widget_check"
label = "Widget Check"
applies_to = { task = ["implement"] }
when = "A widget is fitted."
family = "domain"
text = "Measure the widget after fitting it."
"#
    }

    /// A tiny pack installed beside the base catalog.
    fn two_domains<T>(body: impl FnOnce(&Workspace) -> T) -> T {
        bench(|ws, _| {
            std::fs::write(
                config::config_dir().join("deltas/practices/fixture-domain.toml"),
                second_domain(),
            )
            .unwrap();
            body(ws)
        })
    }

    fn switched_on() -> Overrides {
        over(json!([{"layer": "weft-project", "ringframe": {"deltas": {
            "practices/fixture-domain": {}}}}]))
    }

    #[test]
    fn a_pack_switched_on_comes_after_the_base() {
        two_domains(|_| {
            let ids_of = |over: &Overrides| -> Vec<String> {
                crate::rules::practice_rules(over, &["implement".into()])
                    .unwrap()
                    .iter()
                    .map(|e| text_of(e, "id"))
                    .collect()
            };
            let base = ids_of(&Overrides::default());
            assert!(!base.iter().any(|i| i.starts_with("practice.widget")));
            let both = ids_of(&switched_on());
            assert_eq!(both[..base.len()], base[..]);
            assert_eq!(both[base.len()..], ["practice.widget_first", "practice.widget_check"]);
        });
    }

    #[test]
    fn a_domain_can_be_added_through_an_override() {
        bench(|_, _| {
            let domain = config::load_toml_text(second_domain(), "fixture-domain").unwrap();
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/fixture-domain": domain}}}]));
            let listed = domains(&layers).unwrap();
            let fixture = listed.iter().find(|d| d["domain"] == "fixture-domain").expect("listed");
            assert_eq!(fixture["base"], false);
            let chosen = crate::rules::practice_rules(&layers, &["implement".into()]).unwrap();
            assert!(chosen.iter().any(|e| e["id"] == "practice.widget_first"));
        });
    }

    #[test]
    fn domains_lists_what_is_installed_and_whether_a_layer_switched_it_on() {
        two_domains(|_| {
            let listed = domains(&Overrides::default()).unwrap();
            let find = |name: &str| {
                listed
                    .iter()
                    .find(|d| d["domain"] == name)
                    .unwrap_or_else(|| panic!("{name}"))
                    .clone()
            };
            assert_eq!(find("software-development")["base"], true);
            let fixture = find("fixture-domain");
            assert_eq!(fixture["base"], false);
            assert!(!text_of(&fixture, "description").is_empty());
            assert_eq!(fixture["project_opted_in"], false);
            let again = domains(&switched_on()).unwrap();
            let fixture = again.iter().find(|d| d["domain"] == "fixture-domain").unwrap();
            assert_eq!(fixture["project_opted_in"], true);
        });
    }

    #[test]
    fn a_project_holds_no_delta_files() {
        two_domains(|ws| assert!(!ws.rf_dir().join("deltas").exists()));
    }

    // ---- phases: one Ask that spans several tasks --------------------------

    #[allow(dead_code)]
    fn unused(_: &TempDir) {}
}
