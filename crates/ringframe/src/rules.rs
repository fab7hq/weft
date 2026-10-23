//! The rules an Ask carries, and the prompt they make with the person's
//! words: the practice rules chosen, then every rule of the route.

use serde_json::{Value, json};

use crate::config::{ConfigError, Overrides};
use crate::deltas;

pub const RULES: &str = "Rules:";

/// What a rule is about. Of near-duplicates, Sense proposes the best of each
/// family; the person may add more.
pub const FAMILIES: [&str; 6] = ["scope", "simplicity", "reuse", "testing", "cleanup", "domain"];

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

/// Every practice rule for these tasks, the base's then each pack's that is
/// switched on, in shipped order, each with the layer that last set it.
pub fn practice_rules(over: &Overrides, tasks: &[String]) -> Result<Vec<Value>, ConfigError> {
    let mut all = Vec::new();
    for domain in deltas::domains_on(over)? {
        deltas::load_practice_catalog(&domain, over)?;
        all.extend(deltas::effective(over, &domain)?);
    }
    let mut out = Vec::new();
    for (_, mut e) in all {
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
pub(crate) fn for_tasks(entry: &Value, tasks: &[String]) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::testing::with_config_home;
    use std::path::Path;

    fn shipped_practices() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../fab7/products/ringframe/config/deltas/practices")
    }

    fn ids(list: &[Value]) -> Vec<String> {
        list.iter().map(|e| text(e, "id")).collect()
    }

    fn over(layers: Value) -> Overrides {
        Overrides::parse(&layers.to_string()).unwrap()
    }

    /// The installed base catalog with `edit` applied to its entry `id`.
    fn shipped_with(id: &str, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) {
        let path = config::config_dir().join("deltas/practices/software-development.toml");
        let mut text = std::fs::read_to_string(&path).unwrap();
        let mut doc = config::load_toml_text(&text, "fixture").unwrap();
        let entry = doc["entries"].as_array_mut().unwrap().iter_mut()
            .find(|e| e["id"] == id).unwrap().as_object_mut().unwrap();
        edit(entry);
        text = toml::to_string(&doc).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_shipped_rule_without_when_or_with_an_unknown_family_is_refused() {
        with_config_home(|_| {
            shipped_with("practice.kiss", |e| {
                e.remove("when");
            });
            let e = deltas::load_practice_catalog(deltas::DEFAULT_DOMAIN, &Overrides::default());
            let e = e.unwrap_err().0;
            assert!(e.contains("practice.kiss") && e.contains("when"), "{e}");
        });
        with_config_home(|_| {
            shipped_with("practice.kiss", |e| {
                e.insert("family".into(), json!("style"));
            });
            let e = deltas::load_practice_catalog(deltas::DEFAULT_DOMAIN, &Overrides::default());
            let e = e.unwrap_err().0;
            assert!(e.contains("practice.kiss") && e.contains("family"), "{e}");
        });
    }

    #[test]
    fn the_persons_own_rule_may_leave_out_when() {
        with_config_home(|_| {
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [
                    {"id": "own.1", "text": "One commit per item.",
                     "applies_to": {"task": ["implement"]}}]}}}}]));
            let rules = practice_rules(&layers, &["implement".into()]).unwrap();
            let own = rules.iter().find(|e| e["id"] == "own.1").expect("loaded");
            assert_eq!(own["layer"], "weft");
        });
    }

    #[test]
    fn selection_is_by_task_alone_with_no_tier_cap_or_concern() {
        with_config_home(|_| {
            let rules = practice_rules(&Overrides::default(), &["implement".into()]).unwrap();
            let got = ids(&rules);
            // Once situational, once past the cap: each is offered now.
            for id in ["practice.hyrum", "practice.dry", "practice.solid", "practice.knuth"] {
                assert!(got.contains(&id.to_string()), "{id} in {got:?}");
            }
            // A rule only for planning is not for implementing.
            assert!(!got.contains(&"practice.pareto".to_string()));
            let shipped = deltas::load_practice_catalog(deltas::DEFAULT_DOMAIN, &Overrides::default());
            let shipped = shipped.unwrap();
            assert!(shipped.get("concerns").is_none() && shipped["render"].get("core_cap").is_none());
        });
    }

    #[test]
    fn a_pack_is_off_until_a_layer_switches_it_on() {
        with_config_home(|_| {
            std::fs::copy(
                shipped_practices().join("autonomous-trading.toml"),
                config::config_dir().join("deltas/practices/autonomous-trading.toml"),
            )
            .unwrap();
            let tasks = ["implement".to_string()];
            let off = practice_rules(&Overrides::default(), &tasks).unwrap();
            assert!(!ids(&off).contains(&"practice.idempotent_orders".to_string()));
            let on = over(json!([{"layer": "weft-project", "ringframe": {"deltas": {
                "practices/autonomous-trading": {}}}}]));
            let on = practice_rules(&on, &tasks).unwrap();
            assert!(ids(&on).contains(&"practice.idempotent_orders".to_string()));
            // The base first, then the pack.
            let first_pack = ids(&on).iter().position(|i| i == "practice.idempotent_orders");
            assert!(ids(&on).iter().position(|i| i == "practice.kiss") < first_pack);
        });
    }

    #[test]
    fn fab7s_practice_files_load_and_name_every_rule_the_sense_gold_set_labels() {
        let dir = shipped_practices();
        if !dir.is_dir() {
            return;
        }
        let gold = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sense-gold-rule-ids.txt"),
        )
        .unwrap();
        with_config_home(|_| {
            let installed = config::config_dir().join("deltas/practices");
            for file in ["software-development.toml", "autonomous-trading.toml"] {
                std::fs::copy(dir.join(file), installed.join(file)).unwrap();
            }
            let mut named = Vec::new();
            for domain in ["software-development", "autonomous-trading"] {
                let cat = deltas::load_practice_catalog(domain, &Overrides::default())
                    .unwrap_or_else(|e| panic!("{domain}: {e}"));
                for e in cat["entries"].as_array().unwrap() {
                    assert!(FAMILIES.contains(&text(e, "family").as_str()), "{e}");
                    named.push(text(e, "id"));
                }
            }
            let missing: Vec<&str> = gold
                .lines()
                .filter(|l| !l.starts_with('#') && !l.is_empty())
                .filter(|id| !named.iter().any(|n| n == id))
                .collect();
            assert_eq!(missing, Vec::<&str>::new());
        });
    }
}
