//! Delta catalogs: host deltas keyed by (host, capability); practice deltas
//! keyed by classification.
//!
//! The CLI selects TOML rules; the model adapts them to the task. Deltas guide
//! the work but do not run checks.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::config::{self, ConfigError};
use crate::workspace::Workspace;

pub const SCHEMA: &str = "ringframe.deltas/1";
pub const HOST_STATUS: [&str; 3] = ["candidate", "qualified", "retired"];
pub const PRACTICE_STATUS: [&str; 4] = ["attributed", "candidate", "qualified", "retired"];
pub const TIERS: [&str; 3] = ["core", "situational", "reference"];
pub const DEFAULT_DOMAIN: &str = "software-development";

/// Create a project's empty base-domain override file; preserve existing
/// configuration.
pub fn initialize(root: &Path) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(root)?;
    crate::workspace::set_private(root)?;
    let target = root.join("deltas").join("practices").join(format!("{DEFAULT_DOMAIN}.toml"));
    std::fs::create_dir_all(target.parent().expect("a practices directory"))?;
    // Create it only if it is not there; an existing file is the person's.
    let _ = std::fs::OpenOptions::new().write(true).create_new(true).open(&target);
    let ignore = root.join(".gitignore");
    if !ignore.exists() {
        std::fs::write(&ignore, "*\n")?;
    }
    Ok(vec![target.to_string_lossy().to_string()])
}

fn check(ok: bool, message: impl Into<String>) -> Result<(), ConfigError> {
    if ok { Ok(()) } else { Err(ConfigError(message.into())) }
}

fn dir() -> Result<PathBuf, ConfigError> {
    Ok(config::require_config()?.join("deltas"))
}

pub fn host_catalog_names() -> Result<Vec<String>, ConfigError> {
    Ok(config::stems(&dir()?))
}

/// Domains from the synced mirror and from personal overrides; a project file
/// only opts in.
pub fn domain_names() -> Result<Vec<String>, ConfigError> {
    let roots = [dir()?.join("practices"), config::overrides_dir().join("deltas/practices")];
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for root in roots {
        seen.extend(config::stems(&root));
    }
    Ok(seen.into_iter().collect())
}

pub struct Layer {
    pub root: &'static str,
    pub path: PathBuf,
    pub sha256: String,
    pub document: Value,
}

impl Layer {
    /// The layer as the record carries it: everything but the document itself.
    fn described(&self) -> Value {
        json!({"root": self.root, "path": self.path.to_string_lossy(), "sha256": self.sha256})
    }
}

/// Synced config, then personal overrides, then the project. Later layers win
/// by id.
fn layers(ws: Option<&Workspace>, relative: &str) -> Result<Vec<Layer>, ConfigError> {
    let mut locations: Vec<(&'static str, PathBuf)> = vec![
        ("config", dir()?.join(relative)),
        ("user", config::overrides_dir().join("deltas").join(relative)),
    ];
    if let Some(ws) = ws {
        locations.push(("workspace", ws.rf_dir().join("deltas").join(relative)));
    }
    let mut out = Vec::new();
    for (root, path) in locations {
        if !path.exists() && !path.with_extension("yaml").exists() {
            continue;
        }
        let doc = config::load_toml(&path)?;
        // An empty override inherits; only a document with content is a layer.
        if doc.as_object().is_some_and(|m| !m.is_empty()) {
            out.push(Layer { root, sha256: config::sha256_of(&doc), path, document: doc });
        }
    }
    Ok(out)
}

fn catalog(relative: &str, ws: Option<&Workspace>) -> Result<Value, ConfigError> {
    let mut merged = Value::Object(Map::new());
    for layer in layers(ws, relative)? {
        merged = config::merge(&merged, &layer.document);
    }
    Ok(merged)
}

fn entries(cat: &Value) -> &[Value] {
    cat.get("entries").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn text_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

pub fn load_host_catalog(host: &str, ws: Option<&Workspace>) -> Result<Value, ConfigError> {
    let cat = catalog(&format!("{host}.toml"), ws)?;
    check(
        cat.get("schema").and_then(Value::as_str) == Some(SCHEMA)
            && cat.get("scope").and_then(Value::as_str) == Some("host")
            && cat.get("host").and_then(Value::as_str) == Some(host),
        format!("deltas/{host}.toml: not a host catalog"),
    )?;
    for e in entries(&cat) {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("None");
        check(
            ["id", "capability", "text", "matrix_ref", "status"]
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

pub fn load_practice_catalog(domain: &str, ws: Option<&Workspace>) -> Result<Value, ConfigError> {
    let mut cat = catalog(&format!("practices/{domain}.toml"), ws)?;
    check(
        cat.get("schema").and_then(Value::as_str) == Some(SCHEMA)
            && cat.get("scope").and_then(Value::as_str) == Some("practice"),
        format!("deltas/practices/{domain}.toml: not a practice catalog"),
    )?;
    let map = cat.as_object_mut().expect("a catalog is a mapping");
    let render = map.entry("render").or_insert_with(|| json!({}));
    if let Some(r) = render.as_object_mut() {
        r.entry("core_cap").or_insert_with(|| json!(5));
    }
    map.entry("concerns").or_insert_with(|| json!([]));
    // Nothing here judges the content. Catalogs live on the user's machine and
    // are theirs to edit; a rule that never matches, or a label the
    // composed-prompt audit reads as two, is the author's choice to make. Only
    // the structure the CLI must read is required.
    for e in entries(&cat) {
        let id = e.get("id").and_then(Value::as_str).unwrap_or("None");
        check(
            ["id", "text", "applies_to"].iter().all(|k| e.get(*k).is_some()),
            format!("practice entry {id} incomplete"),
        )?;
    }
    Ok(cat)
}

/// Merged entries in catalog order, annotated with the last scope defining
/// each. An ordered list rather than a map, because the render order is the
/// catalog's.
pub fn effective(
    ws: Option<&Workspace>,
    domain: &str,
) -> Result<Vec<(String, Value)>, ConfigError> {
    let cat = load_practice_catalog(domain, ws)?;
    let mut merged: Vec<(String, Value)> = entries(&cat)
        .iter()
        .map(|e| {
            let mut e = e.clone();
            e["layer"] = json!("user");
            (text_of(&e, "id"), e)
        })
        .collect();
    for layer in layers(ws, &format!("practices/{domain}.toml"))? {
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

fn matches(entry: &Value, classification: &Value, capability: &str) -> bool {
    let a = entry.get("applies_to").cloned().unwrap_or_else(|| json!({}));
    // The route, which is how a directive earns its own heading. It narrows
    // rather than decides: a route that serves two kinds of work needs the
    // result beside it, or the rule fires on the kind it is not about.
    if non_empty_list(a.get("capability"))
        && !a["capability"].as_array().into_iter().flatten().any(|v| v == capability)
    {
        return false;
    }
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

/// Validate against the union of the given domains' vocabularies.
pub fn validate_concerns(
    concerns: &[String],
    domains: &[String],
    ws: Option<&Workspace>,
) -> Result<(), ConfigError> {
    let mut vocab: BTreeSet<String> = BTreeSet::new();
    for n in domains {
        let cat = load_practice_catalog(n, ws)?;
        vocab.extend(
            cat["concerns"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    let unknown: Vec<&String> = concerns.iter().filter(|c| !vocab.contains(*c)).collect();
    let shown = |v: &[&String]| {
        format!("[{}]", v.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))
    };
    check(
        unknown.is_empty(),
        format!(
            "unknown concern(s) {}; domain {} knows [{}]",
            shown(&unknown),
            if domains.len() == 1 {
                format!("'{}'", domains[0])
            } else {
                format!(
                    "[{}]",
                    domains.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ")
                )
            },
            vocab.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ")
        ),
    )
}

/// Installed practice domains and the signals the Ask skill selects from.
pub fn domains(ws: Option<&Workspace>) -> Result<Vec<Value>, ConfigError> {
    let mut out = Vec::new();
    for name in domain_names()? {
        let cat = load_practice_catalog(&name, ws)?;
        let project = ws.map(|w| w.rf_dir().join("deltas/practices").join(format!("{name}.toml")));
        let opted_in = project.is_some_and(|p| {
            std::fs::read(&p).is_ok_and(|b| !b.iter().all(u8::is_ascii_whitespace))
        });
        out.push(json!({
            "domain": name,
            "base": name == DEFAULT_DOMAIN,
            "description": text_of(&cat, "description").split_whitespace().collect::<Vec<_>>().join(" "),
            "concerns": cat["concerns"].clone(),
            "project_opted_in": opted_in,
            "sha256": config::sha256_of(&cat),
        }));
    }
    Ok(out)
}

/// The base domain, then any specialist named by the classification. Unknown
/// names are refused.
pub fn selected_domains(
    classification: &Value,
    _ws: Option<&Workspace>,
) -> Result<Vec<String>, ConfigError> {
    let installed = domain_names()?;
    let mut extra: Vec<String> = Vec::new();
    for name in classification.get("domains").and_then(Value::as_array).into_iter().flatten() {
        let name = name.as_str().unwrap_or_default().to_string();
        if !installed.contains(&name) {
            return Err(ConfigError(format!(
                "'{name}' is not an installed practice domain; installed: {}",
                installed.join(", ")
            )));
        }
        if name != DEFAULT_DOMAIN && !extra.contains(&name) {
            extra.push(name);
        }
    }
    let mut out = vec![DEFAULT_DOMAIN.to_string()];
    out.append(&mut extra);
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
/// Rules that belong to the route rather than the work. They say what this
/// turn is for, so they lead and they are not mixed in with the principles —
/// a deliverable listed third of ten reads like an aside.
pub const THIS_ROUTE: &str = "For this route:";

fn phase_heading(task: &str) -> Option<&'static str> {
    PHASES.iter().find(|(t, _)| *t == task).map(|(_, h)| *h)
}

pub fn is_heading(line: &str) -> bool {
    line == EVERY_PHASE || line == THIS_ROUTE || PHASES.iter().any(|(_, h)| *h == line)
}

/// Whether a rule was chosen for the route rather than for the work.
fn route_scoped(entry: &Value) -> bool {
    entry.get("applies_to").is_some_and(|a| non_empty_list(a.get("capability")))
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

fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A candidate rule, with the two keys it is ordered by.
struct Ranked {
    priority: i64,
    order: usize,
    entry: Value,
}

fn status_of(entry: &Value) -> String {
    let s = text_of(entry, "status");
    if s.is_empty() { "attributed".into() } else { s }
}

/// Which named task this rule belongs to, or every one of them.
fn phase_of(entry: &Value, tasks: &[String]) -> String {
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

/// One catalog's rules under its own core cap, per phase when the Ask names
/// several tasks.
struct Practice {
    domain: String,
    shipped_sha256: Value,
    effective_sha256: String,
    layers: Vec<Value>,
    selected: Vec<String>,
    matched_concerns: Vec<String>,
    dropped_by_budget: Vec<String>,
    heading: String,
    rendered: Vec<(String, Vec<Value>)>,
    entries: Vec<Value>,
}

fn practice(
    ws: Option<&Workspace>,
    domain: &str,
    classification: &Value,
    capability: &str,
    statuses: &[String],
    subagents: bool,
) -> Result<Practice, ConfigError> {
    let cat = load_practice_catalog(domain, ws)?;
    let merged = effective(ws, domain)?;
    let concerns: Vec<String> = classification
        .get("concerns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    // `attributed` and `qualified` render; candidates only in evaluation runs.
    let mut practice_statuses: BTreeSet<&str> = ["attributed", "qualified"].into();
    if statuses.iter().any(|s| s == "candidate") {
        practice_statuses.insert("candidate");
    }

    let (mut core, mut situational): (Vec<Ranked>, Vec<Ranked>) = (Vec::new(), Vec::new());
    for (order, (_, e)) in merged.iter().enumerate() {
        if e.get("enabled") == Some(&json!(false))
            || !practice_statuses.contains(status_of(e).as_str())
            || !matches(e, classification, capability)
        {
            continue;
        }
        if e.get("requires").and_then(|r| r.get("host_capability")) == Some(&json!("subagents"))
            && !subagents
        {
            continue;
        }
        let tier = {
            let t = text_of(e, "tier");
            if t.is_empty() { "situational".to_string() } else { t }
        };
        let ranked = Ranked {
            priority: e.get("priority").and_then(Value::as_i64).unwrap_or(100),
            order,
            entry: e.clone(),
        };
        if tier == "core" {
            core.push(ranked);
        } else if tier == "situational"
            && e.get("concerns")
                .into_iter()
                .flat_map(|c| c.as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .any(|c| concerns.iter().any(|x| x == c))
        {
            situational.push(ranked);
        }
    }
    core.sort_by_key(|r| (r.priority, r.order));
    situational.sort_by_key(|r| (r.priority, r.order));

    let cap = cat["render"]["core_cap"].as_i64().unwrap_or(5).max(0) as usize;
    let tasks: Vec<String> = classification
        .get("task")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|t| phase_heading(t).is_some())
        .map(str::to_string)
        .collect();
    // One group per named task, so a research rule is never weighed against an
    // implementation one. A single task keeps one unnamed group, which renders
    // as today's flat list.
    let mut groups: Vec<String> = vec![EVERY_PHASE.to_string()];
    if tasks.len() > 1 {
        groups.extend(tasks.iter().filter_map(|t| phase_heading(t)).map(str::to_string));
    }
    let group_of = |entry: &Value| -> String {
        if tasks.len() > 1 {
            phase_heading(&phase_of(entry, &tasks)).unwrap_or(EVERY_PHASE).to_string()
        } else {
            EVERY_PHASE.to_string()
        }
    };
    /// One phase's rules: the core tier, then the situational one.
    type Phase = (String, (Vec<Ranked>, Vec<Ranked>));
    let mut by_phase: Vec<Phase> =
        groups.iter().map(|g| (g.clone(), (Vec::new(), Vec::new()))).collect();
    for (is_core, items) in [(true, core), (false, situational)] {
        for r in items {
            let g = group_of(&r.entry);
            if !by_phase.iter().any(|(k, _)| *k == g) {
                by_phase.push((g.clone(), (Vec::new(), Vec::new())));
            }
            let slot = by_phase.iter_mut().find(|(k, _)| *k == g).expect("just inserted");
            if is_core { slot.1.0.push(r) } else { slot.1.1.push(r) }
        }
    }
    // The cap governs the shipped default per phase; candidates under
    // evaluation are appended so that an evaluation arm equals the default arm
    // plus the candidates — nothing displaced, nothing hidden.
    let (mut selected, mut dropped, mut rendered) = (Vec::new(), Vec::new(), Vec::new());
    for g in &groups {
        let Some((_, (core, situational))) = by_phase.iter().find(|(k, _)| k == g) else {
            continue;
        };
        let stable: Vec<&Ranked> =
            core.iter().filter(|r| status_of(&r.entry) != "candidate").collect();
        let candidates: Vec<&Ranked> =
            core.iter().filter(|r| status_of(&r.entry) == "candidate").collect();
        let mut kept: Vec<Value> = stable.iter().take(cap).map(|r| r.entry.clone()).collect();
        kept.extend(candidates.iter().map(|r| r.entry.clone()));
        kept.extend(situational.iter().map(|r| r.entry.clone()));
        dropped.extend(stable.iter().skip(cap).map(|r| text_of(&r.entry, "id")));
        if !kept.is_empty() {
            selected.extend(kept.iter().cloned());
            rendered.push((g.clone(), kept));
        }
    }

    let catalog_layers = layers(ws, &format!("practices/{domain}.toml"))?;
    let shipped = dir()?.join("practices").join(format!("{domain}.toml"));
    // None when no synced file shipped this domain: it is the user's own.
    let shipped_sha256 = match std::fs::read_to_string(&shipped) {
        Ok(text) => Value::String(config::sha256_of(&config::load_toml_text(
            &text,
            &shipped.display().to_string(),
        )?)),
        Err(_) => Value::Null,
    };
    Ok(Practice {
        domain: domain.to_string(),
        shipped_sha256,
        effective_sha256: config::sha256_of(&cat),
        layers: catalog_layers.iter().map(Layer::described).collect(),
        selected: selected.iter().map(|e| text_of(e, "id")).collect(),
        matched_concerns: concerns
            .iter()
            .filter(|c| {
                selected.iter().any(|e| {
                    e.get("concerns")
                        .into_iter()
                        .flat_map(|x| x.as_array().into_iter().flatten())
                        .filter_map(Value::as_str)
                        .any(|x| x == c.as_str())
                })
            })
            .cloned()
            .collect(),
        dropped_by_budget: dropped,
        heading: {
            let h = text_of(&cat["render"], "heading");
            if h.is_empty() { "Rules:".into() } else { h }
        },
        entries: selected
            .iter()
            .map(|e| {
                json!({"id": text_of(e, "id"), "label": label(e), "text": squeeze(&text_of(e, "text"))})
            })
            .collect(),
        rendered,
    })
}

/// Deterministic delta block for one Ask: host lines, then the base practice
/// rules, then any specialist domain's.
pub fn render(
    ws: Option<&Workspace>,
    profile: &Value,
    capability: &str,
    classification: &Value,
    statuses: &[String],
    domain: &str,
) -> Result<Value, ConfigError> {
    // ---- host layer
    let mut host_block = json!({
        "catalog_sha256": null, "deltas": [], "status_filter": statuses, "text": "", "entries": []
    });
    let host = text_of(profile, "host");
    if !host.is_empty() && host_catalog_names()?.contains(&host) {
        let cat = load_host_catalog(&host, ws)?;
        let chosen: Vec<&Value> = entries(&cat)
            .iter()
            .filter(|e| {
                text_of(e, "capability") == capability
                    && statuses.contains(&text_of(e, "status"))
                    && e.get("enabled") != Some(&json!(false))
            })
            .collect();
        host_block = json!({
            "catalog_sha256": config::sha256_of(&cat),
            "deltas": chosen.iter().map(|e| text_of(e, "id")).collect::<Vec<_>>(),
            "status_filter": statuses,
            "text": chosen.iter().map(|e| text_of(e, "text").trim().to_string())
                .collect::<Vec<_>>().join("\n"),
            "entries": chosen.iter().map(|e| json!({
                "id": text_of(e, "id"), "label": label(e), "text": text_of(e, "text").trim()
            })).collect::<Vec<_>>(),
        });
    }

    // ---- practice layers: base first, then specialists, each under its own cap
    let names = if domain == DEFAULT_DOMAIN {
        selected_domains(classification, ws)?
    } else {
        vec![domain.to_string()]
    };
    let concerns: Vec<String> = classification
        .get("concerns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    validate_concerns(&concerns, &names, ws)?;
    let subagents = profile.get("subagents") == Some(&json!(true));
    let blocks: Vec<Practice> = names
        .iter()
        .map(|n| practice(ws, n, classification, capability, statuses, subagents))
        .collect::<Result<_, _>>()?;

    let all_entries: Vec<Value> = blocks.iter().flat_map(|b| b.entries.clone()).collect();
    let heading = blocks[0].heading.clone();
    // One phase at a time, base rules before each specialist's, so a reader
    // sees every rule for researching together and every rule for implementing
    // together.
    let tasks: Vec<String> = classification
        .get("task")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|t| phase_heading(t).is_some())
        .map(str::to_string)
        .collect();
    let mut order: Vec<String> = vec![EVERY_PHASE.to_string()];
    if tasks.len() > 1 {
        order.extend(tasks.iter().filter_map(|t| phase_heading(t)).map(str::to_string));
    }
    let rules_in = |phase: &str| -> Vec<Value> {
        blocks
            .iter()
            .flat_map(|b| b.rendered.iter())
            .filter(|(g, _)| g == phase)
            .flat_map(|(_, kept)| kept.iter().cloned())
            .collect()
    };
    let render_rule = |e: &Value| format!("- {}: {}", label(e), squeeze(&text_of(e, "text")));
    let mut lines: Vec<String> = Vec::new();

    // What this turn is for, before how to do it.
    let route_rules: Vec<Value> =
        order.iter().flat_map(|p| rules_in(p)).filter(route_scoped).collect();
    if !route_rules.is_empty() {
        lines.push(THIS_ROUTE.to_string());
        lines.extend(route_rules.iter().map(&render_rule));
        lines.push(String::new());
    }

    for phase in &order {
        let rules: Vec<Value> = rules_in(phase).into_iter().filter(|e| !route_scoped(e)).collect();
        if rules.is_empty() {
            continue;
        }
        if order.len() > 1 {
            lines.push(String::new());
            lines.push(phase.clone());
        }
        lines.extend(rules.iter().map(&render_rule));
    }
    let practice_text = if all_entries.is_empty() {
        String::new()
    } else {
        format!("{heading}\n{}", lines.join("\n").trim_start_matches('\n'))
    };

    let first = &blocks[0];
    let practice_block = json!({
        "domain": first.domain,
        "shipped_sha256": first.shipped_sha256,
        "effective_sha256": first.effective_sha256,
        "layers": first.layers,
        "revision": revision(),
        "text": practice_text,
        "entries": all_entries,
        "selected": blocks.iter().flat_map(|b| b.selected.clone()).collect::<Vec<_>>(),
        "matched_concerns": blocks.iter().flat_map(|b| b.matched_concerns.clone())
            .collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>(),
        "dropped_by_budget": blocks.iter().flat_map(|b| b.dropped_by_budget.clone()).collect::<Vec<_>>(),
        "phases": order.iter().filter(|g| !rules_in(g).is_empty()).map(|g| json!({
            "phase": g,
            "entries": rules_in(g).iter().map(|e| text_of(e, "id")).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "domains": blocks.iter().map(|b| json!({
            "domain": b.domain, "selected": b.selected, "dropped_by_budget": b.dropped_by_budget,
            "shipped_sha256": b.shipped_sha256, "effective_sha256": b.effective_sha256,
        })).collect::<Vec<_>>(),
    });

    let text = [text_of(&host_block, "text"), practice_text]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(json!({"text": text, "host": host_block, "practice": practice_block}))
}

/// A composed prompt must end with a `Rules:` list whose every label names a
/// supplied directive. Returns the applied ids and the omitted ones.
pub fn audit_composed(
    text: &str,
    supplied: &[Value],
) -> Result<(Vec<String>, Vec<String>), ConfigError> {
    let lines: Vec<&str> = text.lines().collect();
    let last_rules = lines.iter().rposition(|l| l.trim().eq_ignore_ascii_case("rules:"));
    let start = last_rules
        .ok_or_else(|| ConfigError("composed prompt has no `Rules:` section".to_string()))?;
    let mut by_label: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in supplied {
        let id = text_of(e, "id");
        for key in
            [label(e).to_lowercase(), text_of(e, "principle").to_lowercase(), id.to_lowercase()]
        {
            if !key.is_empty() {
                by_label.insert(key, id.clone());
            }
        }
    }
    let known = || {
        let mut names: Vec<String> = supplied.iter().map(label).collect();
        names.sort();
        names.dedup();
        format!("[{}]", names.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", "))
    };
    let mut applied: Vec<String> = Vec::new();
    let mut inside_a_rule = false;
    for line in &lines[start + 1..] {
        let line = line.trim();
        if line.is_empty() || is_heading(line) {
            // A phase heading the CLI printed; match the exact text, never its shape.
            continue;
        }
        // A directive is a sentence, and a sentence wraps. A line that does
        // not open a rule continues the one above it; only text arriving
        // before any rule is prose where a list belongs.
        if inside_a_rule && !line.starts_with("- ") {
            continue;
        }
        check(
            line.starts_with("- ") && line.contains(": "),
            format!(
                "rule line is not `- <labels>: <applied directive>`: '{}'",
                line.chars().take(60).collect::<String>()
            ),
        )?;
        inside_a_rule = true;
        let head = line[2..].split(": ").next().unwrap_or_default().replace(" and ", ",");
        for raw in head.split(',') {
            let lab = raw.trim();
            if lab.is_empty() {
                continue;
            }
            let id = by_label.get(&lab.to_lowercase()).ok_or_else(|| {
                ConfigError(format!(
                    "rule label '{lab}' names no supplied directive; supplied: {}",
                    known()
                ))
            })?;
            if !applied.contains(id) {
                applied.push(id.clone());
            }
        }
    }
    let omitted: Vec<String> =
        supplied.iter().map(|e| text_of(e, "id")).filter(|id| !applied.contains(id)).collect();
    Ok((applied, omitted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles;
    use crate::testing::{TempDir, repo, to_toml, with_config_home, ws_for};

    const QUALIFIED: [&str; 1] = ["qualified"];

    fn statuses(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

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

    fn rendered(ws: &Workspace, profile: &str, cls: &Value) -> Value {
        render(
            Some(ws),
            &profiles::load(profile).unwrap(),
            "native_plan",
            cls,
            &statuses(&QUALIFIED),
            DEFAULT_DOMAIN,
        )
        .unwrap()
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
    }

    fn selected(out: &Value) -> Vec<String> {
        ids(&out["practice"]["selected"])
    }

    fn entry<'a>(list: &'a [(String, Value)], id: &str) -> &'a Value {
        &list.iter().find(|(k, _)| k == id).unwrap_or_else(|| panic!("no {id}")).1
    }

    #[test]
    fn shipped_catalogs_validate_and_have_provenance() {
        with_config_home(|_| {
            for name in host_catalog_names().unwrap() {
                let cat = load_host_catalog(&name, None).unwrap();
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
                    assert!(HOST_STATUS.contains(&text_of(e, "status").as_str()), "{id}");
                }
            }
            let prac = load_practice_catalog(DEFAULT_DOMAIN, None).unwrap();
            assert_eq!(prac["scope"], "practice");
            // Six since practice.plan_as_files: planning carries one
            // structural rule on top of the five principles.
            assert_eq!(prac["render"], json!({"heading": "Rules:", "core_cap": 6}));
            let all: Vec<String> = entries(&prac).iter().map(|e| text_of(e, "id")).collect();
            let unique: BTreeSet<&String> = all.iter().collect();
            assert_eq!(all.len(), unique.len(), "duplicate entry ids");
            let vocab: BTreeSet<&str> =
                prac["concerns"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
            for e in entries(&prac) {
                let id = text_of(e, "id");
                assert!(!text_of(e, "principle").is_empty(), "{id}");
                assert!(!text_of(e, "text").trim().is_empty(), "{id}");
                let tier = text_of(e, "tier");
                assert!(tier.is_empty() || TIERS.contains(&tier.as_str()), "{id}");
                assert!(PRACTICE_STATUS.contains(&status_of(e).as_str()), "{id}");
                for c in
                    e.get("concerns").into_iter().flat_map(|c| c.as_array().into_iter().flatten())
                {
                    assert!(vocab.contains(c.as_str().unwrap_or_default()), "{id}: {c}");
                }
                // Directives, never principle names.
                let text = text_of(e, "text");
                assert!(!text.contains("SOLID") && !text.contains("YAGNI"), "{id}");
            }
        });
    }

    #[test]
    fn host_deltas_render_only_when_qualified_by_default() {
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &impl_task());
            assert_eq!(
                out["host"]["catalog_sha256"],
                json!(config::sha256_of(&load_host_catalog("claude-code", None).unwrap()))
            );
            // D1..D6 are candidates until measured.
            assert_eq!(out["host"]["deltas"], json!([]));
            assert_eq!(out["host"]["status_filter"], json!(["qualified"]));
            let out2 = render(
                Some(ws),
                &profiles::load("claude-code").unwrap(),
                "native_plan",
                &impl_task(),
                &statuses(&["qualified", "candidate"]),
                DEFAULT_DOMAIN,
            )
            .unwrap();
            assert!(
                ids(&out2["host"]["deltas"])
                    .contains(&"claude-code.native_plan.verify_paths".to_string())
            );
            assert!(!text_of(&out2["host"], "text").is_empty());
        });
    }

    #[test]
    fn practice_selection_is_faceted_tiered_and_budgeted() {
        bench(|ws, _| {
            let plain = rendered(ws, "codex", &impl_task());
            let core: Vec<String> = selected(&plain)
                .into_iter()
                .filter(|i| {
                    [
                        "practice.kiss",
                        "practice.yagni",
                        "practice.testing_pyramid",
                        "practice.boy_scout",
                    ]
                    .contains(&i.as_str())
                })
                .collect();
            assert_eq!(core.len(), 4);
            let cap = load_practice_catalog(DEFAULT_DOMAIN, None).unwrap()["render"]["core_cap"]
                .as_u64()
                .unwrap() as usize;
            assert!(selected(&plain).len() <= cap);
            // Situational: needs a concern.
            assert!(!selected(&plain).contains(&"practice.hyrum".to_string()));

            let mut cls = impl_task();
            cls["concerns"] = json!(["api_surface", "auth"]);
            let with_api = rendered(ws, "codex", &cls);
            for want in ["practice.hyrum", "practice.postel"] {
                assert!(selected(&with_api).contains(&want.to_string()), "{want}");
            }
            assert_eq!(with_api["practice"]["matched_concerns"], json!(["api_surface", "auth"]));
            // Labels as traceability tags, directives applied.
            let text = text_of(&with_api, "text");
            assert!(text.contains("- KISS: "), "{text}");
            assert!(text.contains("- Hyrum: "));

            let mut planning = impl_task();
            planning["task"] = json!(["plan"]);
            planning["result"] = json!("plan");
            planning["effects"] = json!(["read"]);
            let out = rendered(ws, "codex", &planning);
            assert!(!selected(&out).contains(&"practice.testing_pyramid".to_string()));
            assert!(selected(&out).contains(&"practice.gall".to_string()));

            let mut bad = impl_task();
            bad["concerns"] = json!(["telepathy"]);
            let e = render(
                Some(ws),
                &profiles::load("codex").unwrap(),
                "native_plan",
                &bad,
                &statuses(&QUALIFIED),
                DEFAULT_DOMAIN,
            )
            .unwrap_err();
            assert!(e.0.contains("concern"), "{e}");
        });
    }

    #[test]
    fn user_and_workspace_layers_override_by_id() {
        bench(|ws, _| {
            let catalog =
                config::overrides_dir().join("deltas/practices/software-development.toml");
            std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
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
            std::fs::write(&catalog, to_toml(&doc)).unwrap();
            std::fs::write(
                ws.rf_dir().join("deltas/practices/software-development.toml"),
                "schema = \"ringframe.deltas/1\"\nscope = \"practice\"\n\n\
                 [[entries]]\nid = \"practice.yagni\"\nenabled = false\n",
            )
            .unwrap();

            let r = rendered(ws, "codex", &impl_task());
            assert!(text_of(&r, "text").contains("Keep it plain."));
            assert!(!selected(&r).contains(&"practice.yagni".to_string()));
            assert!(selected(&r).contains(&"practice.team.commit_style".to_string()));
            let layers: Vec<(String, bool)> = r["practice"]["layers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| {
                    (text_of(l, "root"), text_of(l, "path").ends_with("software-development.toml"))
                })
                .collect();
            assert_eq!(
                layers,
                [("config".into(), true), ("user".into(), true), ("workspace".into(), true)]
            );
            let listing = effective(Some(ws), DEFAULT_DOMAIN).unwrap();
            assert_eq!(entry(&listing, "practice.kiss")["layer"], "user");
            assert_eq!(entry(&listing, "practice.yagni")["enabled"], false);
            assert_eq!(entry(&listing, "practice.gall")["layer"], "user");
        });
    }

    #[test]
    fn render_is_a_labelled_rules_list_and_entries_carry_labels() {
        bench(|ws, _| {
            let mut cls = impl_task();
            cls["concerns"] = json!(["api_surface"]);
            let r = rendered(ws, "codex", &cls);
            let text = text_of(&r["practice"], "text");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines[0], "Rules:");
            assert!(
                lines[1..]
                    .iter()
                    .filter(|l| !l.trim().is_empty() && !is_heading(l))
                    .all(|l| l.starts_with("- ") && l.contains(": ")),
                "{text}"
            );
            let labels: BTreeSet<String> = r["practice"]["entries"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| text_of(e, "label"))
                .collect();
            assert!(labels.contains("KISS") && labels.contains("Hyrum"));
            assert!(labels.iter().all(|l| !l.is_empty()));
            assert!(lines.iter().any(|l| l.starts_with("- KISS: ")));
            assert!(lines.iter().any(|l| l.starts_with("- Hyrum: ")));
            for name in host_catalog_names().unwrap() {
                for e in entries(&load_host_catalog(&name, None).unwrap()) {
                    assert!(!text_of(e, "label").is_empty(), "{}", text_of(e, "id"));
                }
            }
        });
    }

    #[test]
    fn candidate_practice_entries_render_only_when_candidates_are_requested() {
        bench(|ws, _| {
            let default = rendered(ws, "claude-code", &impl_task());
            // A candidate is never in the default prompt.
            assert!(!selected(&default).contains(&"practice.assumptions".to_string()));
            let evaluation = render(
                Some(ws),
                &profiles::load("claude-code").unwrap(),
                "native_plan",
                &impl_task(),
                &statuses(&["qualified", "candidate"]),
                DEFAULT_DOMAIN,
            )
            .unwrap();
            assert!(selected(&evaluation).contains(&"practice.assumptions".to_string()));
            let before: BTreeSet<String> = selected(&default).into_iter().collect();
            let after: BTreeSet<String> = selected(&evaluation).into_iter().collect();
            assert!(after.is_superset(&before), "an evaluation arm hides nothing");
        });
    }

    // ---- catalog reachability and task coverage ----------------------------

    const TASK_PROBE: [(&str, &[&str]); 9] = [
        ("question", &[]),
        ("research", &[]),
        ("clarify", &[]),
        ("plan", &[]),
        ("implement", &[]),
        ("diagnose", &["tests_only"]),
        ("review", &[]),
        ("operate", &["operate"]),
        ("document", &["cli"]),
    ];

    fn probe(task: &str, concerns: &[&str]) -> Value {
        json!({"task": [task], "result": "plan", "interaction": "approval_gated",
               "horizon": "session", "effects": ["read"], "concerns": concerns})
    }

    #[test]
    fn a_planning_directive_follows_what_the_turn_delivers_not_the_route() {
        // It was keyed on the route, and `native_plan` is also the route for
        // ordinary bounded work — so that work was told to write a plan and
        // stop, which is the opposite of what it was asked for.
        bench(|ws, _| {
            let mut plan = impl_task();
            plan["result"] = json!("plan");
            assert!(
                selected(&rendered(ws, "claude-code", &plan))
                    .contains(&"practice.plan_as_files".to_string()),
                "a turn that delivers a plan is told where to put it"
            );

            let build = impl_task();
            assert_eq!(build["result"], json!("workspace_change"));
            assert!(
                !selected(&rendered(ws, "claude-code", &build))
                    .contains(&"practice.plan_as_files".to_string()),
                "bounded work on the planning route is not told to stop at a plan"
            );
        });
    }

    #[test]
    fn every_entry_can_be_selected() {
        // A situational entry with no concerns never matches, so it is dead
        // configuration.
        bench(|ws, _| {
            let cat = load_practice_catalog(DEFAULT_DOMAIN, Some(ws)).unwrap();
            let dead: Vec<String> = entries(&cat)
                .iter()
                .filter(|e| {
                    let tier = text_of(e, "tier");
                    (tier.is_empty() || tier == "situational") && !non_empty_list(e.get("concerns"))
                })
                .map(|e| text_of(e, "id"))
                .collect();
            assert_eq!(dead, Vec::<String>::new());
        });
    }

    #[test]
    fn every_task_selects_at_least_one_rule() {
        bench(|ws, _| {
            for (task, concerns) in TASK_PROBE {
                let out = rendered(ws, "claude-code", &probe(task, concerns));
                assert!(!selected(&out).is_empty(), "{task} selects no rule");
            }
        });
    }

    #[test]
    fn named_rules_select_for_their_task() {
        bench(|ws, _| {
            for (task, rule) in [
                ("research", "practice.occam"),
                ("review", "practice.linus"),
                ("question", "practice.confirmation_bias"),
                ("implement", "practice.testing_pyramid"),
            ] {
                let concerns = TASK_PROBE.iter().find(|(t, _)| *t == task).unwrap().1;
                let out = rendered(ws, "claude-code", &probe(task, concerns));
                assert!(selected(&out).contains(&rule.to_string()), "{rule} missing for {task}");
            }
        });
    }

    #[test]
    fn the_core_cap_is_not_exceeded_for_any_task() {
        bench(|ws, _| {
            for (task, concerns) in TASK_PROBE {
                let out = rendered(ws, "claude-code", &probe(task, concerns));
                assert_eq!(
                    out["practice"]["dropped_by_budget"],
                    json!([]),
                    "{task} drops a core rule"
                );
                assert!(!selected(&out).is_empty());
            }
        });
    }

    // ---- additive specialist domains ---------------------------------------

    fn second_domain() -> &'static str {
        r#"schema = "ringframe.deltas/1"
scope = "practice"
domain = "fixture-domain"
description = "An invented domain used only by tests."
render = { heading = "Rules:", core_cap = 2 }
concerns = ["widgets", "api_surface"]

[[entries]]
id = "practice.widget_first"
label = "Widget First"
tier = "core"
applies_to = { task = ["implement"] }
text = "Build the widget before the housing."

[[entries]]
id = "practice.widget_check"
label = "Widget Check"
applies_to = { task = ["implement"] }
concerns = ["widgets"]
text = "Measure the widget after fitting it."
"#
    }

    /// A tiny specialist catalog installed beside the base one.
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

    #[test]
    fn absent_domains_render_only_the_base() {
        two_domains(|ws| {
            let out = rendered(ws, "claude-code", &impl_task());
            let listed: Vec<String> = out["practice"]["domains"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| text_of(d, "domain"))
                .collect();
            assert_eq!(listed, ["software-development"]);
            assert!(!selected(&out).iter().any(|i| i.starts_with("practice.widget")));
        });
    }

    #[test]
    fn a_specialist_domain_renders_after_the_base_under_its_own_cap() {
        two_domains(|ws| {
            let mut cls = impl_task();
            cls["domains"] = json!(["fixture-domain"]);
            cls["concerns"] = json!(["widgets"]);
            let out = rendered(ws, "claude-code", &cls);
            let p = &out["practice"];
            let listed: Vec<String> =
                p["domains"].as_array().unwrap().iter().map(|d| text_of(d, "domain")).collect();
            assert_eq!(listed, ["software-development", "fixture-domain"]);
            let base_ids = ids(&p["domains"][0]["selected"]);
            // Base first, specialist after.
            assert_eq!(selected(&out)[..base_ids.len()], base_ids[..]);
            assert!(selected(&out).contains(&"practice.widget_first".to_string()));
            assert!(selected(&out).contains(&"practice.widget_check".to_string()));
            // Route rules lead and are set apart, so compare the principles
            // among themselves: the base domain's, then the specialist's.
            let text = text_of(p, "text");
            let labels: Vec<&str> = text
                .lines()
                .filter(|l| l.starts_with("- ") && !l.starts_with("- Plan As Files"))
                .map(|l| l.split(':').next().unwrap_or(""))
                .collect();
            let base_principles =
                base_ids.iter().filter(|i| *i != "practice.plan_as_files").count();
            assert_eq!(labels[base_principles], "- Widget First", "{text}");
        });
    }

    #[test]
    fn an_unknown_domain_is_refused_with_the_installed_set() {
        two_domains(|ws| {
            let mut cls = impl_task();
            cls["domains"] = json!(["teleportation"]);
            let e = render(
                Some(ws),
                &profiles::load("claude-code").unwrap(),
                "native_plan",
                &cls,
                &statuses(&QUALIFIED),
                DEFAULT_DOMAIN,
            )
            .unwrap_err();
            assert!(e.0.contains("installed"), "{e}");
        });
    }

    #[test]
    fn concerns_validate_against_the_union_of_selected_domains() {
        two_domains(|ws| {
            let mut only_concern = impl_task();
            only_concern["concerns"] = json!(["widgets"]);
            let e = render(
                Some(ws),
                &profiles::load("claude-code").unwrap(),
                "native_plan",
                &only_concern,
                &statuses(&QUALIFIED),
                DEFAULT_DOMAIN,
            )
            .unwrap_err();
            assert!(e.0.contains("concern"), "{e}");

            let mut with_domain = only_concern.clone();
            with_domain["domains"] = json!(["fixture-domain"]);
            let out = rendered(ws, "claude-code", &with_domain);
            assert!(selected(&out).contains(&"practice.widget_check".to_string()));
        });
    }

    #[test]
    fn domains_lists_what_is_installed_and_whether_the_project_opted_in() {
        two_domains(|ws| {
            let listed = domains(Some(ws)).unwrap();
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
            assert!(ids(&fixture["concerns"]).contains(&"widgets".to_string()));
            assert_eq!(fixture["project_opted_in"], false);

            let opt_in = ws.rf_dir().join("deltas/practices/fixture-domain.toml");
            std::fs::create_dir_all(opt_in.parent().unwrap()).unwrap();
            std::fs::write(
                &opt_in,
                "schema = \"ringframe.deltas/1\"\nscope = \"practice\"\ndomain = \"fixture-domain\"\n",
            )
            .unwrap();
            let again = domains(Some(ws)).unwrap();
            let fixture = again.iter().find(|d| d["domain"] == "fixture-domain").unwrap();
            assert_eq!(fixture["project_opted_in"], true);
        });
    }

    #[test]
    fn project_init_seeds_only_the_base_domain() {
        two_domains(|ws| {
            let mut seeded: Vec<String> = std::fs::read_dir(ws.rf_dir().join("deltas/practices"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            seeded.sort();
            assert_eq!(seeded, ["software-development.toml"]);
        });
    }

    #[test]
    fn a_domain_can_be_added_through_personal_overrides() {
        // delta.md offers a domain file "in the marketplace or in your overrides".
        bench(|ws, _| {
            let mine = config::overrides_dir().join("deltas/practices/fixture-domain.toml");
            std::fs::create_dir_all(mine.parent().unwrap()).unwrap();
            std::fs::write(&mine, second_domain()).unwrap();
            let listed = domains(Some(ws)).unwrap();
            let fixture = listed.iter().find(|d| d["domain"] == "fixture-domain").expect("listed");
            assert_eq!(fixture["base"], false);

            let mut cls = impl_task();
            cls["domains"] = json!(["fixture-domain"]);
            cls["concerns"] = json!(["widgets"]);
            let out = rendered(ws, "claude-code", &cls);
            assert!(selected(&out).contains(&"practice.widget_first".to_string()));
            let block = out["practice"]["domains"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["domain"] == "fixture-domain")
                .unwrap()
                .clone();
            // Nothing shipped it; it is the user's own.
            assert_eq!(block["shipped_sha256"], json!(null));
        });
    }

    // ---- phases: one Ask that spans several tasks --------------------------

    fn research_and_implement() -> Value {
        json!({"task": ["research", "implement"], "result": "workspace_change",
               "interaction": "approval_gated", "horizon": "session",
               "effects": ["read", "write"]})
    }

    #[test]
    fn one_task_renders_a_flat_list() {
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &impl_task());
            let text = text_of(&out["practice"], "text");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines[0], "Rules:");
            // One task means no phase headings. The route heading is not a
            // phase; it says what the turn is for.
            assert!(!lines.iter().any(|l| PHASES.iter().any(|(_, h)| h == l)), "{text}");
            assert!(
                lines[1..]
                    .iter()
                    .filter(|l| !l.trim().is_empty() && **l != THIS_ROUTE)
                    .all(|l| l.starts_with("- ")),
                "{text}"
            );
        });
    }

    #[test]
    fn several_tasks_group_the_rules_by_phase() {
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &research_and_implement());
            let text = text_of(&out["practice"], "text");
            assert!(text.contains("While researching:"), "{text}");
            assert!(text.contains("While implementing:"));
            // A rule that applies to every named task is stated once, not
            // repeated per phase.
            assert_eq!(text.matches("- Occam").count(), 1);
            let labels: Vec<&str> = text.lines().filter(|l| l.starts_with("- ")).collect();
            let unique: BTreeSet<&&str> = labels.iter().collect();
            assert_eq!(labels.len(), unique.len());
        });
    }

    #[test]
    fn the_core_cap_applies_per_phase_so_research_rules_survive() {
        // Before grouping, implement's core rules outranked research's and
        // pushed them out.
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &research_and_implement());
            assert!(
                selected(&out).contains(&"practice.occam".to_string()),
                "a research rule must survive a research+implement Ask"
            );
            assert!(
                selected(&out).contains(&"practice.testing_pyramid".to_string()),
                "and so must an implement rule"
            );
            assert_eq!(out["practice"]["dropped_by_budget"], json!([]));
        });
    }

    #[test]
    fn phases_appear_in_the_order_the_classification_names_them() {
        bench(|ws, _| {
            let mut cls = research_and_implement();
            cls["task"] = json!(["implement", "research"]);
            let out = rendered(ws, "claude-code", &cls);
            let text = text_of(&out["practice"], "text");
            assert!(text.find("While implementing:") < text.find("While researching:"), "{text}");
        });
    }

    #[test]
    fn the_prompt_audit_accepts_phase_headings() {
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &research_and_implement());
            let supplied: Vec<Value> = out["practice"]["entries"].as_array().unwrap().clone();
            let composed = format!(
                "Do the thing.\n\nRules:\n\nWhile researching:\n- {}: applied to this task.\n",
                text_of(&supplied[0], "label")
            );
            let (applied, omitted) = audit_composed(&composed, &supplied).unwrap();
            assert_eq!(applied, [text_of(&supplied[0], "id")]);
            assert_eq!(omitted.len(), supplied.len() - 1);
        });
    }

    #[test]
    fn the_audit_still_rejects_a_line_that_is_neither_rule_nor_heading() {
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &impl_task());
            let supplied: Vec<Value> = out["practice"]["entries"].as_array().unwrap().clone();
            let e =
                audit_composed("Do it.\n\nRules:\nthis is just prose\n", &supplied).unwrap_err();
            assert!(e.0.contains("not `- <labels>"), "{e}");
        });
    }

    #[test]
    fn the_audit_accepts_every_heading_the_cli_can_print() {
        // Including the one-word `Throughout:`, which a shape heuristic misses.
        bench(|ws, _| {
            let out = rendered(ws, "claude-code", &research_and_implement());
            let supplied: Vec<Value> = out["practice"]["entries"].as_array().unwrap().clone();
            let headings: Vec<&str> =
                std::iter::once(EVERY_PHASE).chain(PHASES.iter().map(|(_, h)| *h)).collect();
            for heading in headings {
                let composed = format!(
                    "Do the thing.\n\nRules:\n{heading}\n- {}: applied here.\n",
                    text_of(&supplied[0], "label")
                );
                let (applied, _) = audit_composed(&composed, &supplied).unwrap();
                assert_eq!(applied, [text_of(&supplied[0], "id")], "{heading}");
            }
        });
    }

    #[test]
    fn a_rule_may_wrap_onto_more_than_one_line() {
        // A directive is a sentence and a sentence wraps. Demanding one line
        // per rule refuses prompts that are correct in every way that counts.
        bench(|ws, _| {
            let supplied = rendered(ws, "claude-code", &impl_task())["practice"]["entries"]
                .as_array()
                .unwrap()
                .clone();
            let label = text_of(&supplied[0], "label");
            let wrapped = format!(
                "Do the thing.\n\nRules:\n- {label}: applied to this task, at such\n  length that it wraps onto a second line,\n  and a third.\n"
            );
            let (applied, _) = audit_composed(&wrapped, &supplied).unwrap();
            assert_eq!(applied, [text_of(&supplied[0], "id")]);
        });
    }

    #[test]
    fn prose_before_any_rule_is_still_refused() {
        // Wrapping is a continuation of a rule. Text arriving before one is
        // prose where a list belongs, and that is still wrong.
        bench(|ws, _| {
            let supplied = rendered(ws, "claude-code", &impl_task())["practice"]["entries"]
                .as_array()
                .unwrap()
                .clone();
            let e =
                audit_composed("Do it.\n\nRules:\nthis is just prose\n", &supplied).unwrap_err();
            assert!(e.0.contains("not `- <labels>"), "{e}");
        });
    }

    #[test]
    fn a_route_rule_leads_and_is_set_apart() {
        // A deliverable listed third of ten reads like an aside.
        bench(|ws, _| {
            let mut cls = impl_task();
            cls["result"] = json!("plan");
            let out = rendered(ws, "claude-code", &cls);
            let text = text_of(&out["practice"], "text");
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines[0], "Rules:");
            assert_eq!(lines[1], THIS_ROUTE);
            assert!(lines[2].starts_with("- Plan As Files:"), "{text}");
            // And the principles follow, after a blank line.
            assert!(text.contains("\n\n- KISS:"), "{text}");
            // The heading is one the audit knows, so a composed prompt may
            // carry it back.
            assert!(is_heading(THIS_ROUTE));
        });
    }

    #[test]
    fn with_no_route_rule_there_is_no_route_heading() {
        bench(|ws, _| {
            let out = render(
                Some(ws),
                &profiles::load("claude-code").unwrap(),
                "native_goal",
                &impl_task(),
                &statuses(&QUALIFIED),
                DEFAULT_DOMAIN,
            )
            .unwrap();
            let text = text_of(&out["practice"], "text");
            assert!(!text.contains(THIS_ROUTE), "{text}");
            assert!(text.starts_with("Rules:\n- "), "{text}");
        });
    }

    #[test]
    fn the_audit_refuses_a_composed_prompt_with_no_rules_list() {
        let e = audit_composed("Just do it.\n", &[]).unwrap_err();
        assert!(e.0.contains("no `Rules:` section"), "{e}");
    }

    #[allow(dead_code)]
    fn unused(_: &TempDir) {}
}
