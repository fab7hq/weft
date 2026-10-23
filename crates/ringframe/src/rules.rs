//! The rules an Ask carries, and the prompt they make with the person's
//! words: the practice rules chosen, then every rule of the route.

use serde_json::{Map, Value, json};

use crate::ask::{AskError, ledger};
use crate::config::{self, ConfigError, Overrides};
use crate::deltas;
use crate::workspace::Workspace;

pub const RULES: &str = "Rules:";

/// The team's rules, committed with the code: read after the shipped ones
/// and before the person's.
pub const REPO_FILE: &str = ".fab7/rules.toml";

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
    let layers = deltas::layers(over, host)?;
    // The layer that last set each rule.
    let layer_of = |id: &str| {
        let last = layers.iter().rev().find(|l| {
            l.document["entries"].as_array().is_some_and(|list| list.iter().any(|e| e["id"] == id))
        });
        last.map_or("shipped", |l| if l.root == "config" { "shipped" } else { l.root.as_str() })
            .to_string()
    };
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
        .map(|e| {
            let mut e = e.clone();
            e["layer"] = json!(layer_of(&text(&e, "id")));
            e
        })
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
                let (sa, sb) =
                    (score(&text(&practices[a], "id")), score(&text(&practices[b], "id")));
                sa.total_cmp(&sb)
            })
            .expect("not empty");
        dropped.push(text(&practices.remove(lowest), "id"));
    }
    dropped
}

// ---- the repo layer ----------------------------------------------------

/// The domain whose shipped catalog holds `id`; a rule no catalog ships is
/// the base's.
fn domain_of(id: &str, over: &Overrides) -> Result<String, ConfigError> {
    for name in deltas::domain_names(over)? {
        let shipped = config::config_dir().join(format!("deltas/practices/{name}.toml"));
        if shipped.exists()
            && config::load_toml(&shipped)?["entries"]
                .as_array()
                .is_some_and(|list| list.iter().any(|e| e["id"] == id))
        {
            return Ok(name);
        }
    }
    Ok(deltas::DEFAULT_DOMAIN.to_string())
}

/// `rules.toml` as a layer: each rule under the domain that ships its id,
/// and each pack it names switched on.
fn repo_layer(doc: &Value, over: &Overrides) -> Result<Map<String, Value>, ConfigError> {
    if let Some(key) = doc
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, _)| k)
        .find(|k| !["packs", "entries"].contains(&k.as_str()))
    {
        return Err(ConfigError(format!("{REPO_FILE}: unknown key \"{key}\"")));
    }
    let mut deltas = Map::new();
    for pack in strings(&doc["packs"]) {
        deltas.entry(format!("practices/{pack}")).or_insert_with(|| json!({}));
    }
    for e in doc["entries"].as_array().into_iter().flatten() {
        let path = format!("practices/{}", domain_of(&text(e, "id"), over)?);
        let slot = deltas.entry(path).or_insert_with(|| json!({}));
        let list =
            slot.as_object_mut().expect("a document").entry("entries").or_insert_with(|| json!([]));
        list.as_array_mut().expect("a list").push(e.clone());
    }
    Ok(deltas)
}

/// The person's layers with the team's file read before them.
pub fn with_repo(ws: &Workspace, over: &Overrides) -> Result<Overrides, ConfigError> {
    let path = ws.root.join(REPO_FILE);
    if !path.exists() {
        return Ok(over.with_first("repo", Map::new()));
    }
    Ok(over.with_first("repo", repo_layer(&config::load_toml(&path)?, over)?))
}

/// Move one of the person's rules into the team's file, as their layers
/// have it, and return it: their tool then takes it out of their own layer.
/// The file keeps what it held; the rule is added at its end.
pub fn share(ws: &Workspace, id: &str, over: &Overrides) -> Result<Value, AskError> {
    let mut mine: Option<Value> = None;
    for domain in deltas::domain_names(over)? {
        for (_, doc) in over.documents(&format!("practices/{domain}")) {
            for e in doc["entries"].as_array().into_iter().flatten().filter(|e| e["id"] == id) {
                mine = Some(config::merge(mine.as_ref().unwrap_or(&json!({})), e));
            }
        }
    }
    let Some(entry) = mine else {
        return Err(ledger("rules.not_personal", format!("'{id}' is in none of your layers")));
    };
    // A cloned repository is not trusted: a link here would write elsewhere.
    let path = ws.root.join(REPO_FILE);
    for p in [ws.root.join(".fab7"), path.clone()] {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ledger(
                "rules.link",
                format!("{} is a link; RingFrame writes no link", p.display()),
            ));
        }
    }
    let mut team = if path.exists() { config::load_toml(&path)? } else { json!({}) };
    if team["entries"].as_array().is_some_and(|l| l.iter().any(|e| e["id"] == id)) {
        return Err(ledger("rules.already_shared", format!("'{id}' is in {REPO_FILE} already")));
    }
    let list =
        team.as_object_mut().expect("a document").entry("entries").or_insert_with(|| json!([]));
    list.as_array_mut().expect("a list").push(entry.clone());
    // The team's file as it would be, on its own, must load.
    let layer = repo_layer(&team, over)?;
    let alone = Overrides::default().with_first("repo", layer.clone());
    for domain in layer.keys().filter_map(|k| k.strip_prefix("practices/")) {
        deltas::load_practice_catalog(domain, &alone)
            .map_err(|e| ledger("rules.incomplete", e.0))?;
    }
    let block = toml::to_string(&json!({"entries": [entry]}))
        .map_err(|e| ledger("rules.share", e.to_string()))?;
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&block);
    std::fs::create_dir_all(ws.root.join(".fab7"))?;
    // Written whole beside it, then renamed over it: never a half-written
    // file. Created new, so nothing already there, a link least of all, is
    // written through.
    let fresh = ws.root.join(format!("{REPO_FILE}.{}", crate::ids::new_id("tmp")));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&fresh)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    std::fs::rename(&fresh, &path)?;
    Ok(entry)
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
        let entry = doc["entries"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == id)
            .unwrap()
            .as_object_mut()
            .unwrap();
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
            let shipped =
                deltas::load_practice_catalog(deltas::DEFAULT_DOMAIN, &Overrides::default());
            let shipped = shipped.unwrap();
            assert!(
                shipped.get("concerns").is_none() && shipped["render"].get("core_cap").is_none()
            );
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

    // ---- the repo layer -------------------------------------------------

    fn team_file(ws: &crate::workspace::Workspace, text: &str) {
        std::fs::create_dir_all(ws.root.join(".fab7")).unwrap();
        std::fs::write(ws.root.join(REPO_FILE), text).unwrap();
    }

    fn kiss(rules: &[Value]) -> Value {
        rules.iter().find(|e| e["id"] == "practice.kiss").cloned().expect("kiss")
    }

    fn person(layer: &str, text: &str) -> Value {
        json!({"layer": layer, "ringframe": {"deltas": {"practices/software-development": {
            "entries": [{"id": "practice.kiss", "text": text}]}}}})
    }

    #[test]
    fn layers_apply_shipped_then_repo_then_weft_then_weft_project() {
        crate::testing::eval_bench(|ws| {
            let tasks = ["implement".to_string()];
            let offered = |layers: Value| {
                practice_rules(&with_repo(ws, &over(layers)).unwrap(), &tasks).unwrap()
            };
            assert_eq!(kiss(&offered(json!([])))["layer"], "shipped");
            team_file(ws, "[[entries]]\nid = \"practice.kiss\"\ntext = \"The team's KISS.\"\n");
            let k = kiss(&offered(json!([])));
            assert_eq!(
                (k["layer"].as_str(), k["text"].as_str()),
                (Some("repo"), Some("The team's KISS."))
            );
            // Still the shipped rule's `when` and family: the team changed the wording.
            assert!(!text(&k, "when").is_empty());
            let k = kiss(&offered(json!([person("weft", "Mine.")])));
            assert_eq!((k["layer"].as_str(), k["text"].as_str()), (Some("weft"), Some("Mine.")));
            let k =
                kiss(&offered(json!([person("weft", "Mine."), person("weft-project", "Here.")])));
            assert_eq!(
                (k["layer"].as_str(), k["text"].as_str()),
                (Some("weft-project"), Some("Here."))
            );
        });
    }

    #[test]
    fn a_team_rule_says_when_it_matters_and_a_team_can_switch_a_pack_on() {
        crate::testing::eval_bench(|ws| {
            team_file(
                ws,
                "[[entries]]\nid = \"team.commits\"\nlabel = \"Commits\"\n\
                           text = \"One commit per item.\"\napplies_to = { task = [\"implement\"] }\n",
            );
            let e = with_repo(ws, &Overrides::default())
                .and_then(|o| practice_rules(&o, &["implement".into()]))
                .unwrap_err();
            assert!(e.0.contains("team.commits") && e.0.contains("when"), "{e}");

            std::fs::copy(
                shipped_practices().join("autonomous-trading.toml"),
                config::config_dir().join("deltas/practices/autonomous-trading.toml"),
            )
            .unwrap();
            team_file(
                ws,
                "packs = [\"autonomous-trading\"]\n\n[[entries]]\nid = \"team.commits\"\n\
                           label = \"Commits\"\nwhen = \"Work is committed.\"\nfamily = \"cleanup\"\n\
                           text = \"One commit per item.\"\napplies_to = { task = [\"implement\"] }\n",
            );
            let rules = practice_rules(
                &with_repo(ws, &Overrides::default()).unwrap(),
                &["implement".into()],
            )
            .unwrap();
            let got = ids(&rules);
            assert!(got.contains(&"team.commits".to_string()), "{got:?}");
            assert!(got.contains(&"practice.idempotent_orders".to_string()), "{got:?}");
        });
    }

    #[test]
    fn sharing_refuses_a_rules_file_that_is_a_link() {
        crate::testing::eval_bench(|ws| {
            let elsewhere = crate::testing::tmp_dir();
            let target = elsewhere.path().join("profile");
            std::fs::write(&target, "kept\n").unwrap();
            std::fs::create_dir_all(ws.root.join(".fab7")).unwrap();
            std::os::unix::fs::symlink(&target, ws.root.join(REPO_FILE)).unwrap();
            let mine = json!({"id": "own.1", "label": "Commits", "text": "One commit per item.",
                              "when": "Work is committed.", "family": "cleanup",
                              "applies_to": {"task": ["implement"]}});
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [mine]}}}}]));
            let e = share(ws, "own.1", &layers).unwrap_err().to_string();
            assert!(e.contains("rules.link"), "{e}");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept\n");
        });
    }

    #[test]
    fn sharing_moves_a_persons_rule_into_the_repo_layer() {
        crate::testing::eval_bench(|ws| {
            team_file(ws, "# Our rules.\n");
            let mine = json!({"id": "own.1", "label": "Commits", "text": "One commit per item.",
                              "when": "Work is committed.", "family": "cleanup",
                              "applies_to": {"task": ["implement"]}});
            let layers = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [mine]}}}}]));
            let shared = share(ws, "own.1", &layers).unwrap();
            assert_eq!(shared["text"], "One commit per item.");
            let file = std::fs::read_to_string(ws.root.join(REPO_FILE)).unwrap();
            assert!(file.starts_with("# Our rules.\n"), "the team's file keeps what it held");
            let ignored = std::process::Command::new("git")
                .arg("-C")
                .arg(&ws.root)
                .args(["check-ignore", "-q", REPO_FILE])
                .status()
                .unwrap();
            assert_eq!(ignored.code(), Some(1), "Git keeps the team's file");
            // Without the person's layer, the team's file offers it.
            let rules = practice_rules(
                &with_repo(ws, &Overrides::default()).unwrap(),
                &["implement".into()],
            )
            .unwrap();
            let own = rules.iter().find(|e| e["id"] == "own.1").expect("shared");
            assert_eq!(own["layer"], "repo");

            let again = share(ws, "own.1", &layers).unwrap_err().to_string();
            assert!(again.contains("rules.already_shared"), "{again}");
            let shipped = share(ws, "practice.yagni", &layers).unwrap_err().to_string();
            assert!(shipped.contains("rules.not_personal"), "{shipped}");
            let bare = over(json!([{"layer": "weft", "ringframe": {"deltas": {
                "practices/software-development": {"entries": [
                    {"id": "own.2", "text": "Plain.", "applies_to": {"task": ["implement"]}}]}}}}]));
            let e = share(ws, "own.2", &bare).unwrap_err().to_string();
            assert!(e.contains("rules.incomplete") && e.contains("when"), "{e}");
        });
    }
}
