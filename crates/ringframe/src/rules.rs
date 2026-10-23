//! The rules an Ask carries, and the prompt they make with the person's
//! words: the practice rules chosen, then every rule of the route.

use serde_json::{Value, json};

use crate::config::{ConfigError, Overrides};
use crate::deltas;

pub const RULES: &str = "Rules:";

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// Every rule of this host's route, in file order: qualified, switched on,
/// and for this kind of result.
pub fn route_rules(
    over: &Overrides,
    host: &str,
    capability: &str,
    classification: &Value,
) -> Result<Vec<Value>, ConfigError> {
    if !deltas::host_catalog_names()?.iter().any(|h| h == host) {
        return Ok(Vec::new());
    }
    let cat = deltas::load_host_catalog(host, over)?;
    Ok(cat["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| {
            text(e, "capability") == capability
                && text(e, "status") == "qualified"
                && e.get("enabled") != Some(&json!(false))
                && deltas::matches(e, classification)
        })
        .cloned()
        .collect())
}

/// Every practice rule for these tasks, in shipped order, each with the
/// layer that last set it.
pub fn practice_rules(over: &Overrides, tasks: &[String]) -> Result<Vec<Value>, ConfigError> {
    let mut out = Vec::new();
    for (_, mut e) in deltas::effective(over, deltas::DEFAULT_DOMAIN)? {
        let on = e.get("enabled") != Some(&json!(false))
            && ["attributed", "qualified"].contains(&deltas::status_of(&e).as_str());
        if !on || !for_tasks(&e, tasks) {
            continue;
        }
        if e["layer"] == "config" {
            e["layer"] = json!("shipped");
        }
        out.push(e);
    }
    Ok(out)
}

/// `applies_to.task` is a hard filter: a rule naming tasks is only for those.
fn for_tasks(entry: &Value, tasks: &[String]) -> bool {
    let named = strings(&entry["applies_to"]["task"]);
    named.is_empty() || named.iter().any(|t| tasks.contains(t))
}

fn line(e: &Value) -> String {
    format!("- {}: {}", deltas::label(e), deltas::squeeze(&text(e, "text")))
}

/// The person's words, then what they rest on (one line each: the plan, a
/// remedy's findings), then `Rules:`. Under it the practices chosen, in
/// shipped order and under their phase headings when the Ask names several
/// tasks, then every route rule under `For this route:`.
pub fn compose(
    words: &str,
    context: &[String],
    practices: &[Value],
    route: &[Value],
    tasks: &[String],
) -> String {
    let mut parts = vec![words.trim_end_matches('\n').to_string()];
    if !context.is_empty() {
        parts.push(context.join("\n"));
    }
    let mut rules: Vec<String> = Vec::new();
    for (heading, group) in by_phase(practices, tasks) {
        rules.extend(heading);
        rules.extend(group.iter().map(|e| line(e)));
    }
    if !route.is_empty() {
        rules.push(deltas::THIS_ROUTE.to_string());
        rules.extend(route.iter().map(line));
    }
    if !rules.is_empty() {
        parts.push(format!("{RULES}\n{}", rules.join("\n")));
    }
    format!("{}\n", parts.join("\n\n"))
}

/// One group with no heading for one task; otherwise a group per phase,
/// `Throughout:` first, each heading only when a rule is under it.
fn by_phase<'a>(practices: &'a [Value], tasks: &[String]) -> Vec<(Option<String>, Vec<&'a Value>)> {
    if tasks.len() < 2 {
        return vec![(None, practices.iter().collect())];
    }
    let heading = |e: &Value| {
        deltas::phase_heading(&deltas::phase_of(e, tasks)).unwrap_or(deltas::EVERY_PHASE)
    };
    let order = std::iter::once(deltas::EVERY_PHASE)
        .chain(tasks.iter().filter_map(|t| deltas::phase_heading(t)));
    order
        .map(|h| (Some(h.to_string()), practices.iter().filter(|e| heading(e) == h).collect()))
        .filter(|(_, group): &(_, Vec<&Value>)| !group.is_empty())
        .collect()
}

/// Drop the lowest-scored practice rule until `fits` holds; of two with the
/// same score, the later one goes. Returns the ids dropped, in order. Route
/// rules are never passed here, so none is ever dropped.
pub fn trim(
    practices: &mut Vec<Value>,
    score: impl Fn(&str) -> f64,
    fits: impl Fn(&[Value]) -> bool,
) -> Vec<String> {
    let mut dropped = Vec::new();
    while !fits(practices) && !practices.is_empty() {
        let lowest = (0..practices.len())
            .rev()
            .min_by(|&a, &b| {
                let (sa, sb) = (score(&text(&practices[a], "id")), score(&text(&practices[b], "id")));
                sa.total_cmp(&sb)
            })
            .expect("not empty");
        dropped.push(text(&practices.remove(lowest), "id"));
    }
    dropped
}
