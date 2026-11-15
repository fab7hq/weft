//! How often each practice rule is kept: per rule, the Asks it was offered
//! on and the ones it was sent with. Sense reads it to score a rule by what
//! the person did with it before.
//!
//! One table per person, at `~/.fab7/sense/memory/counts.json`, over every
//! workspace Weft knows. It is derived from their ledgers and holds nothing
//! they do not, so a missing or unreadable table is counted again from them.
//! Nothing in it leaves the machine.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub use weft_sense::Counts;

/// Every rule's counts, and the Asks counted, so each counts once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    rules: BTreeMap<String, Counts>,
    asks: BTreeSet<String>,
}

impl Table {
    pub fn get(&self, rule: &str) -> Option<Counts> {
        self.rules.get(rule).copied()
    }

    /// Count a ledger's compiled Asks not counted yet. An Ask counts for each
    /// rule it was offered, except the convention's, and as kept when it was
    /// sent with the rule, trimmed to fit the harness or not. An Ask
    /// compiled before RingFrame recorded what was offered, or one an older
    /// RingFrame cancelled, counts nothing.
    pub fn count(&mut self, events: &[Value], convention: &[String]) {
        let cancelled: BTreeSet<&str> = events
            .iter()
            .filter(|e| e["type"] == "ask.cancelled")
            .filter_map(|e| e["id"].as_str())
            .collect();
        for e in events.iter().filter(|e| e["type"] == "ask.compiled") {
            let Some(id) = e["id"].as_str() else { continue };
            let compiler = &e["data"]["compiler"];
            let Some(offered) = compiler["offered"].as_array() else { continue };
            if cancelled.contains(id) || !self.asks.insert(id.to_string()) {
                continue;
            }
            let ids = |key: &str| -> BTreeSet<&str> {
                compiler[key].as_array().into_iter().flatten().filter_map(Value::as_str).collect()
            };
            let kept: BTreeSet<&str> = ids("practices").union(&ids("trimmed")).copied().collect();
            let offered: BTreeSet<&str> = offered.iter().filter_map(Value::as_str).collect();
            for rule in offered.into_iter().filter(|r| !convention.iter().any(|c| c == r)) {
                let counts = self.rules.entry(rule.to_string()).or_default();
                counts.candidate += 1;
                counts.kept += u32::from(kept.contains(rule));
            }
        }
    }

    /// The table as its file holds it; none when it is not one.
    pub fn read(text: &str) -> Option<Table> {
        let doc: Value = serde_json::from_str(text).ok()?;
        let number = |v: &Value| v.as_u64().and_then(|n| u32::try_from(n).ok());
        let mut rules = BTreeMap::new();
        for (id, c) in doc["rules"].as_object()? {
            rules.insert(
                id.clone(),
                Counts { candidate: number(&c["candidate"])?, kept: number(&c["kept"])? },
            );
        }
        let asks = doc["asks"].as_array()?.iter().map(|a| a.as_str().map(str::to_string));
        Some(Table { rules, asks: asks.collect::<Option<_>>()? })
    }

    pub fn write(&self) -> String {
        let rules: serde_json::Map<String, Value> = self
            .rules
            .iter()
            .map(|(id, c)| (id.clone(), json!({"candidate": c.candidate, "kept": c.kept})))
            .collect();
        json!({"rules": rules, "asks": self.asks}).to_string()
    }
}

/// Where the table lives: `sense/memory` beside Weft's own folder.
pub fn dir(weft_dir: &Path) -> PathBuf {
    weft_dir.with_file_name("sense").join("memory")
}

/// Count every workspace's ledger from the start. A workspace whose ledger
/// is gone or unreadable counts nothing.
pub fn rebuild(roots: &[PathBuf], convention: &[String]) -> Table {
    let mut table = Table::default();
    for root in roots {
        table.count(&events(root), convention);
    }
    table
}

/// The table kept in `dir`; when it is missing or unreadable, every
/// workspace counted again, and kept.
pub fn table(dir: &Path, roots: &[PathBuf], convention: &[String]) -> Table {
    let path = dir.join("counts.json");
    if let Some(kept) = std::fs::read_to_string(&path).ok().and_then(|t| Table::read(&t)) {
        return kept;
    }
    let table = rebuild(roots, convention);
    let _ = write_atomic(&path, &table.write());
    table
}

/// After an Ask is compiled in `root`: count what its ledger holds that the
/// table has not, and keep the table.
pub fn learn(dir: &Path, roots: &[PathBuf], root: &Path, convention: &[String]) -> Table {
    let mut table = table(dir, roots, convention);
    table.count(&events(root), convention);
    let _ = write_atomic(&dir.join("counts.json"), &table.write());
    table
}

/// Let the table go, so it is counted again from the ledgers when next read:
/// a new bundle may decide other rules by convention.
pub fn forget(dir: &Path) {
    let _ = std::fs::remove_file(dir.join("counts.json"));
}

/// Sense as weftd keeps it: the bundle loaded, if any.
pub type Loaded = std::sync::RwLock<Option<weft_sense::SenseModel>>;

/// The loaded bundle's convention; none without a bundle, and then nothing
/// is counted or scored.
fn convention(model: &Loaded) -> Option<Vec<String>> {
    Some(model.read().ok()?.as_ref()?.convention().to_vec())
}

/// After an Ask is compiled in `root`, off the person's send: count it over
/// the open workspaces, when a bundle is loaded.
pub fn after_compile(model: &Loaded, weft_dir: &Path, root: &Path) {
    let Some(convention) = convention(model) else { return };
    let roots = crate::state::load(weft_dir).open_projects;
    learn(&dir(weft_dir), &roots, root, &convention);
}

/// `rules`, each with the person's counts for it, when a bundle is loaded.
pub fn with_counts(
    model: &Loaded,
    weft_dir: &Path,
    rules: &[weft_sense::Rule],
) -> Vec<weft_sense::Rule> {
    let Some(convention) = convention(model) else { return rules.to_vec() };
    let roots = crate::state::load(weft_dir).open_projects;
    let table = table(&dir(weft_dir), &roots, &convention);
    rules.iter().map(|r| weft_sense::Rule { counts: table.get(&r.id), ..r.clone() }).collect()
}

fn events(root: &Path) -> Vec<Value> {
    let ledger = ringframe::api::workspace(root).and_then(|ws| ringframe::api::events(&ws));
    ledger.unwrap_or_default()
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ringframe::tests::{fixture_home, folder};
    use crate::ringframe::{AskDraft, compile, set_up};

    fn compiled(id: &str, offered: Option<&[&str]>, practices: &[&str], trimmed: &[&str]) -> Value {
        let mut compiler = json!({"practices": practices, "trimmed": trimmed});
        if let Some(o) = offered {
            compiler["offered"] = json!(o);
        }
        json!({"type": "ask.compiled", "id": id, "data": {"compiler": compiler}})
    }

    fn convention() -> Vec<String> {
        vec!["practice.commit_style".into()]
    }

    #[test]
    fn a_kept_rule_counts_twice_an_unticked_one_its_offer_and_a_convention_nothing() {
        let offered = ["practice.kiss", "practice.yagni", "practice.gall", "practice.commit_style"];
        let mut table = Table::default();
        table.count(
            &[compiled(
                "ask_1",
                Some(&offered),
                &["practice.kiss", "practice.commit_style"],
                &["practice.gall"],
            )],
            &convention(),
        );
        let got = |id| table.get(id);
        assert_eq!(got("practice.kiss"), Some(Counts { candidate: 1, kept: 1 }));
        assert_eq!(got("practice.yagni"), Some(Counts { candidate: 1, kept: 0 }), "unticked");
        assert_eq!(got("practice.gall"), Some(Counts { candidate: 1, kept: 1 }), "trimmed");
        assert_eq!(got("practice.commit_style"), None, "the convention's");
    }

    #[test]
    fn an_ask_counts_once_and_one_cancelled_or_from_before_the_field_counts_nothing() {
        let mut table = Table::default();
        let ask = compiled("ask_1", Some(&["practice.kiss"]), &["practice.kiss"], &[]);
        table.count(std::slice::from_ref(&ask), &convention());
        table.count(&[ask], &convention());
        let older = compiled("ask_2", None, &["practice.kiss"], &[]);
        let cancelled = compiled("ask_3", Some(&["practice.kiss"]), &["practice.kiss"], &[]);
        table.count(
            &[older, cancelled, json!({"type": "ask.cancelled", "id": "ask_3"})],
            &convention(),
        );
        assert_eq!(table.get("practice.kiss"), Some(Counts { candidate: 1, kept: 1 }));
        assert_eq!(Table::read(&table.write()), Some(table), "the file reads back");
    }

    /// With a bundle loaded, each compiled Ask is counted over the open
    /// workspaces and the next Ask's rules carry their counts; without one,
    /// nothing is counted and the rules carry none.
    #[test]
    fn a_loaded_bundle_counts_each_ask_and_gives_sense_the_rules_counts() {
        fixture_home();
        let root = folder("memory-wired", true);
        let tmp = tempfile::tempdir().unwrap();
        let weft_dir = tmp.path().join(".fab7/weft");
        let state =
            weft_core::state::WeftState { open_projects: vec![root.clone()], ..Default::default() };
        crate::state::save(&weft_dir, &state).unwrap();
        let rule = |id: &str| weft_sense::Rule {
            id: id.into(),
            label: id.into(),
            when: "w".into(),
            counts: None,
        };
        let rules = [rule("practice.dry"), rule("practice.kiss"), rule("practice.occam")];
        let none = Loaded::new(None);
        assert_eq!(set_up(&root), None);
        compile(&root, &draft("Fix bug 0.", &["practice.dry"]), None).unwrap();
        after_compile(&none, &weft_dir, &root);
        assert!(!dir(&weft_dir).exists(), "without a bundle nothing is counted");
        assert_eq!(with_counts(&none, &weft_dir, &rules), rules);

        let bundle = tmp.path().join("bundle");
        crate::versions::tests::stage_sense_bundle(&bundle, "0.1.1");
        let loaded = Loaded::new(Some(weft_sense::SenseModel::load(&bundle).unwrap()));
        for i in 1..3 {
            compile(&root, &draft(&format!("Fix bug {i}."), &["practice.dry"]), None).unwrap();
            after_compile(&loaded, &weft_dir, &root);
        }
        let counts = |id| with_counts(&loaded, &weft_dir, &rules).into_iter().find(|r| r.id == id);
        let kept = Counts { candidate: 3, kept: 3 };
        assert_eq!(counts("practice.dry").unwrap().counts, Some(kept), "the one before too");
        assert_eq!(
            counts("practice.occam").unwrap().counts,
            Some(Counts { candidate: 3, kept: 0 })
        );
        assert_eq!(counts("practice.kiss").unwrap().counts, None, "the convention's");
        let _ = std::fs::remove_dir_all(root);
    }

    fn draft(words: &str, rules: &[&str]) -> AskDraft {
        AskDraft {
            words: words.into(),
            harness: "claude-code".into(),
            route: "native_direct".into(),
            task: "implement".into(),
            effects: vec!["read".into(), "write".into()],
            plan: None,
            rules: rules.iter().map(|r| r.to_string()).collect(),
            link: None,
            sense: None,
            session: None,
        }
    }

    /// Counting Ask by Ask as each is sent gives the table a rebuild from
    /// the ledgers gives, and a missing or unreadable table is rebuilt.
    #[test]
    fn a_rebuild_from_the_ledgers_gives_the_table_counting_ask_by_ask() {
        fixture_home();
        let roots = vec![folder("memory-a", true), folder("memory-b", true)];
        let home = tempfile::tempdir().unwrap();
        let dir = dir(&home.path().join(".fab7/weft"));
        assert_eq!(dir, home.path().join(".fab7/sense/memory"));
        let sends =
            [(0, &["practice.kiss"][..]), (1, &["practice.kiss", "practice.yagni"]), (0, &[])];
        let mut learnt = Table::default();
        for (i, (at, rules)) in sends.iter().enumerate() {
            let root = &roots[*at];
            assert_eq!(set_up(root), None);
            compile(root, &draft(&format!("Fix bug {i}."), rules), None).unwrap();
            learnt = learn(&dir, &roots, root, &convention());
        }
        assert_eq!(learnt.get("practice.kiss").map(|c| (c.candidate, c.kept)), Some((3, 2)));
        assert_eq!(learnt.get("practice.yagni").map(|c| c.kept), Some(1));
        assert_eq!(rebuild(&roots, &convention()), learnt);
        // A table that is not one is counted again, the same.
        std::fs::write(dir.join("counts.json"), "{not json").unwrap();
        assert_eq!(learn(&dir, &roots, &roots[0], &convention()), learnt);
        forget(&dir);
        assert!(!dir.join("counts.json").exists());
        assert_eq!(table(&dir, &roots, &convention()), learnt, "read again, counted again");
        assert!(dir.join("counts.json").exists(), "and kept");
        assert_eq!(learn(&dir, &roots, &roots[1], &convention()), learnt);
        for root in roots {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
