//! Authored configuration is YAML; evidence is canonical JSON.
//!
//! Identities of configuration documents are digests of the parsed document's
//! canonical JSON, so comments and formatting never change a profile or
//! catalog identity.

use std::path::{Path, PathBuf};

use saphyr::{LoadableYamlNode, Scalar, ScalarStyle, Yaml, YamlLoader};
use serde_json::{Map, Value};

use crate::digest;
use crate::store::canonical;

#[derive(Debug)]
pub struct ConfigError(pub String);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConfigError {}

/// The global config home. Written by `ringframe init --global`, read on every
/// invocation.
pub fn home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".fab7").join("rf")
}

/// Synced configuration: a mirror of the bundle. `sync` overwrites it; never
/// edit it.
pub fn config_dir() -> PathBuf {
    home().join("config")
}

/// Personal deltas, merged over `config_dir()` by id. `sync` never touches
/// them.
pub fn overrides_dir() -> PathBuf {
    home().join("overrides")
}

pub fn require_config() -> Result<PathBuf, ConfigError> {
    let d = config_dir();
    if !d.join("harnesses").is_dir() {
        return Err(ConfigError(format!(
            "config.absent: no configuration in {}; run `ringframe init --global`",
            d.display()
        )));
    }
    Ok(d)
}

pub fn load_yaml_text(text: &str, where_: &str, allow_empty: bool) -> Result<Value, ConfigError> {
    let docs = Yaml::load_from_str(text).map_err(|e| ConfigError(format!("{where_}: {e}")))?;
    let doc = match docs.first() {
        None => {
            return if allow_empty {
                Ok(Value::Object(Map::new()))
            } else {
                Err(ConfigError(format!("{where_}: top level must be a mapping")))
            };
        }
        Some(d) => to_json(d).map_err(|e| ConfigError(format!("{where_}: {e}")))?,
    };
    match doc {
        Value::Null if allow_empty => Ok(Value::Object(Map::new())),
        Value::Object(_) => Ok(doc),
        _ => Err(ConfigError(format!("{where_}: top level must be a mapping"))),
    }
}

pub fn load_yaml(path: &Path, allow_empty: bool) -> Result<Value, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
    load_yaml_text(&text, &path.display().to_string(), allow_empty)
}

/// YAML's value model onto JSON's.
///
/// Anything the safe subset does not cover is refused rather than guessed at:
/// PyYAML's `safe_load` would refuse the same documents, and a configuration
/// file is not a place to be generous.
fn to_json(node: &Yaml) -> Result<Value, String> {
    Ok(match node {
        Yaml::Value(scalar) => match scalar {
            Scalar::Null => Value::Null,
            Scalar::Boolean(b) => Value::Bool(*b),
            Scalar::Integer(i) => Value::Number((*i).into()),
            Scalar::FloatingPoint(f) => serde_json::Number::from_f64(f.into_inner())
                .map(Value::Number)
                .ok_or_else(|| format!("{} is not a finite number", f.into_inner()))?,
            Scalar::String(s) => Value::String(s.to_string()),
        },
        Yaml::Sequence(items) => {
            Value::Array(items.iter().map(to_json).collect::<Result<_, _>>()?)
        }
        Yaml::Mapping(map) => {
            let mut out = Map::new();
            for (key, value) in map {
                out.insert(mapping_key(key)?, to_json(value)?);
            }
            Value::Object(out)
        }
        Yaml::Tagged(tag, _) => {
            return Err(format!("unsupported tag !{}{}", tag.handle, tag.suffix));
        }
        Yaml::Alias(_) => return Err("unsupported alias".into()),
        Yaml::Representation(..) => return Err("unresolved scalar".into()),
        Yaml::BadValue => return Err("could not be parsed".into()),
    })
}

/// JSON has only string keys. YAML's other scalar keys become the text JSON
/// would give them, and a collection key is refused.
fn mapping_key(key: &Yaml) -> Result<String, String> {
    match to_json(key)? {
        Value::String(s) => Ok(s),
        Value::Null => Ok("null".into()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err("a mapping key must be a scalar".into()),
    }
}

pub fn sha256_of(doc: &Value) -> String {
    digest::sha256_bytes(&canonical(doc))
}

/// Recursive mappings and id-keyed lists; all other values are replaced.
pub fn merge(base: &Value, override_: &Value) -> Value {
    if let (Value::Object(b), Value::Object(o)) = (base, override_) {
        let mut result = b.clone();
        for (key, value) in o {
            let merged = match result.get(key) {
                Some(existing) => merge(existing, value),
                None => value.clone(),
            };
            result.insert(key.clone(), merged);
        }
        return Value::Object(result);
    }
    if let (Value::Array(b), Value::Array(o)) = (base, override_)
        && !o.is_empty()
        && b.iter().chain(o).all(|i| i.get("id").is_some())
    {
        // Keyed by id, and the base's order is kept: a person's override edits
        // an entry in place rather than moving it to the end.
        let mut order: Vec<String> = Vec::new();
        let mut by_id: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        for item in b.iter().chain(o) {
            let id = item["id"].as_str().unwrap_or_default().to_string();
            match by_id.get(&id) {
                Some(existing) => {
                    let merged = merge(existing, item);
                    by_id.insert(id, merged);
                }
                None => {
                    order.push(id.clone());
                    by_id.insert(id, item.clone());
                }
            }
        }
        return Value::Array(order.into_iter().map(|id| by_id[&id].clone()).collect());
    }
    override_.clone()
}

/// Refuse a document that leans on a scalar YAML 1.1 and 1.2 disagree about.
///
/// The Python era read `yes` as a boolean and `0777` as octal; this one reads
/// the first as a string and the second as seven hundred and seventy-seven.
/// The shipped configuration has none of these, and this is what keeps it that
/// way (ADR-0013).
///
/// Only *plain* scalars are at issue, so this asks the parser for the style
/// rather than guessing from the text: `no` is refused and `"no"` is fine,
/// which is exactly what the message tells the person to do.
pub fn lint_yaml_11(text: &str, where_: &str) -> Result<(), ConfigError> {
    let mut loader: YamlLoader<Yaml> = YamlLoader::default();
    loader.early_parse(false);
    let mut parser = saphyr_parser::Parser::new_from_str(text);
    parser.load(&mut loader, true).map_err(|e| ConfigError(format!("{where_}: {e}")))?;
    let mut found = Vec::new();
    for doc in loader.into_documents() {
        walk_plain(&doc, &mut found);
    }
    match found.first() {
        None => Ok(()),
        Some((raw, why)) => Err(ConfigError(format!(
            "config.yaml_11: {where_}: `{raw}` is {why}, which this release reads differently \
             from the Python one. Quote it."
        ))),
    }
}

fn walk_plain(node: &Yaml, found: &mut Vec<(String, &'static str)>) {
    match node {
        Yaml::Representation(raw, ScalarStyle::Plain, _) => {
            if let Some(why) = nineteen_eleven_scalar(raw) {
                found.push((raw.to_string(), why));
            }
        }
        Yaml::Sequence(items) => items.iter().for_each(|i| walk_plain(i, found)),
        Yaml::Mapping(map) => {
            for (k, v) in map {
                walk_plain(k, found);
                walk_plain(v, found);
            }
        }
        Yaml::Tagged(_, inner) => walk_plain(inner, found),
        _ => {}
    }
}

/// The text of a plain scalar the two YAML versions do not agree on.
fn nineteen_eleven_scalar(text: &str) -> Option<&'static str> {
    const BOOLEANS: [&str; 8] = ["yes", "no", "on", "off", "y", "n", "true", "false"];
    let lower = text.to_ascii_lowercase();
    if BOOLEANS.contains(&lower.as_str()) && !matches!(text, "true" | "false") {
        return Some("a YAML 1.1 boolean");
    }
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    if digits.len() > 1 && digits.starts_with('0') && digits[1..].bytes().all(|b| b.is_ascii_digit())
    {
        return Some("a YAML 1.1 octal integer");
    }
    if digits.contains('_') && digits.replace('_', "").bytes().all(|b| b.is_ascii_digit()) {
        return Some("a YAML 1.1 underscored integer");
    }
    if digits.contains(':')
        && digits.split(':').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Some("a YAML 1.1 sexagesimal number");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Value {
        load_yaml_text(text, "t.yaml", false).unwrap()
    }

    #[test]
    fn yaml_identity_ignores_comments_and_formatting() {
        let a = parse("schema: x/1\nname: demo   # a comment\nitems:\n  - one\n  - two\n");
        let b = parse("# different layout, same document\nitems: [one, two]\nschema: x/1\nname: demo\n");
        let want = json!({"schema": "x/1", "name": "demo", "items": ["one", "two"]});
        assert_eq!(a, want);
        assert_eq!(b, want);
        assert_eq!(sha256_of(&a), sha256_of(&b));
    }

    #[test]
    fn the_loader_is_safe_and_requires_a_mapping() {
        let e = load_yaml_text(
            "!!python/object/apply:os.system ['echo pwned']\n",
            "bad.yaml",
            false,
        )
        .unwrap_err();
        assert!(e.0.contains("bad.yaml"), "{e}");
        let e = load_yaml_text("- just\n- a list\n", "list.yaml", false).unwrap_err();
        assert!(e.0.contains("mapping"), "{e}");
    }

    #[test]
    fn an_empty_document_is_a_document_only_when_allowed() {
        assert_eq!(load_yaml_text("# only a comment\n", "t.yaml", true).unwrap(), json!({}));
        assert!(load_yaml_text("# only a comment\n", "t.yaml", false).is_err());
    }

    #[test]
    fn merge_is_recursive_for_mappings_and_keyed_for_id_lists() {
        let base = json!({"render": {"core_cap": 3, "tier": "core"}, "entries": [
            {"id": "a", "text": "A", "why": "because"}, {"id": "b", "text": "B"}]});
        let over = json!({"render": {"core_cap": 1}, "entries": [
            {"id": "a", "text": "A!"}, {"id": "c", "text": "C"}]});
        assert_eq!(
            merge(&base, &over),
            json!({"render": {"core_cap": 1, "tier": "core"}, "entries": [
                {"id": "a", "text": "A!", "why": "because"},
                {"id": "b", "text": "B"},
                {"id": "c", "text": "C"}]})
        );
    }

    #[test]
    fn a_list_without_ids_is_replaced_whole() {
        let base = json!({"items": ["one", "two"]});
        assert_eq!(merge(&base, &json!({"items": ["three"]})), json!({"items": ["three"]}));
        // An empty override list clears, rather than keeping the base by id.
        assert_eq!(merge(&json!({"e": [{"id": "a"}]}), &json!({"e": []})), json!({"e": []}));
    }

    #[test]
    fn the_lint_refuses_plain_one_one_scalars_and_allows_quoted_ones() {
        for bad in [
            "project_opted_in: yes\n",
            "enabled: No\n",
            "flag: on\n",
            "mode: OFF\n",
            "mask: 0777\n",
            "count: 1_000\n",
            "at: 1:30\n",
            "items:\n  - yes\n",
        ] {
            let e = lint_yaml_11(bad, "p.yaml").unwrap_err();
            assert!(e.0.starts_with("config.yaml_11: p.yaml:"), "{bad:?} gave {e}");
            assert!(e.0.contains("Quote it."), "{e}");
        }
        for good in [
            "project_opted_in: false\n",
            "project_opted_in: true\n",
            "answer: \"no\"\n",
            "answer: 'yes'\n",
            "mask: \"0777\"\n",
            "count: 1000\n",
            "text: |\n  yes\n",
            "note: says yes to it\n",
        ] {
            assert!(lint_yaml_11(good, "p.yaml").is_ok(), "{good:?} was refused");
        }
    }

    #[test]
    fn the_shipped_configuration_passes_the_lint() {
        // The claim ADR-0013 makes about the config as it stands. The fixture
        // travels with this repository so the check means something in CI; the
        // marketplace itself is checked too when it happens to be beside us.
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut roots = vec![here.join("tests/fixtures/config")];
        if let Ok(market) = here.join("../../../fab7/products/ringframe").canonicalize() {
            roots.push(market);
        }
        for root in roots {
            lint_every_yaml_under(&root);
        }
    }

    fn lint_every_yaml_under(root: &std::path::Path) {
        let mut checked = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "yaml") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    let name = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
                    lint_yaml_11(&text, &name).unwrap_or_else(|e| panic!("{e}"));
                    checked += 1;
                }
            }
        }
        assert!(checked > 5, "{}: only {checked} config files were checked", root.display());
    }
}
