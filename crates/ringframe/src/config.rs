//! Authored configuration is TOML; evidence is canonical JSON.
//!
//! Identities of configuration documents are digests of the parsed document's
//! canonical JSON, so comments and formatting never change a profile or
//! catalog identity.

use std::path::{Path, PathBuf};

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

/// A TOML document as the JSON value the rest of the crate reads.
///
/// TOML has no null: a key the document leaves out is absent, which every
/// reader here treats as null. A date or time has no JSON form and is refused
/// rather than guessed at.
pub fn load_toml_text(text: &str, where_: &str) -> Result<Value, ConfigError> {
    let doc: toml::Table = text.parse().map_err(|e| ConfigError(format!("{where_}: {e}")))?;
    to_json(toml::Value::Table(doc)).map_err(|e| ConfigError(format!("{where_}: {e}")))
}

/// Read `path`, refusing when the YAML file it replaced is still beside it.
pub fn load_toml(path: &Path) -> Result<Value, ConfigError> {
    refuse_yaml(path)?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError(format!("{}: {e}", path.display())))?;
    load_toml_text(&text, &path.display().to_string())
}

/// Configuration is TOML; a `.yaml` where a `.toml` is read is named, never
/// read or converted.
pub fn refuse_yaml(toml_path: &Path) -> Result<(), ConfigError> {
    match yaml_left(toml_path) {
        Some(detail) => Err(ConfigError(format!("config.yaml_found: {detail}"))),
        None => Ok(()),
    }
}

/// What is wrong when the YAML file `toml_path` replaced is still there.
pub fn yaml_left(toml_path: &Path) -> Option<String> {
    let yaml = toml_path.with_extension("yaml");
    yaml.is_file()
        .then(|| format!("{} is YAML; this release reads {}", yaml.display(), toml_path.display()))
}

/// The stems of the configuration files in `dir`, sorted. A `.yaml` one is
/// listed too, so that reading it refuses by name rather than skipping it.
pub fn stems(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".toml").or_else(|| name.strip_suffix(".yaml")).map(str::to_string)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn to_json(node: toml::Value) -> Result<Value, String> {
    Ok(match node {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i.into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| format!("{f} is not a finite number"))?,
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => return Err(format!("unsupported date-time {d}")),
        toml::Value::Array(items) => {
            Value::Array(items.into_iter().map(to_json).collect::<Result<_, _>>()?)
        }
        toml::Value::Table(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| Ok((k, to_json(v)?)))
                .collect::<Result<Map<_, _>, String>>()?,
        ),
    })
}

/// The layers `--override` supplies, applied in order over the synced
/// bundle and merged by id. Each is a name, which is its provenance, and the
/// delta documents it overrides, keyed by their path under `deltas/` without
/// the extension.
#[derive(Debug, Default)]
pub struct Overrides(Vec<(String, Map<String, Value>)>);

impl Overrides {
    /// `[{"layer": "<name>", "ringframe": {"deltas": {"<path>": {…}}}}, …]`.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let bad = |what: String| ConfigError(format!("--override {what}"));
        let value: Value =
            serde_json::from_str(text).map_err(|e| bad(format!("is not JSON: {e}")))?;
        let Value::Array(items) = value else {
            return Err(bad("must be a list of layers".into()));
        };
        let mut layers = Vec::new();
        for (i, item) in items.into_iter().enumerate() {
            let Value::Object(mut layer) = item else {
                return Err(bad(format!("layer {i} must be an object")));
            };
            let name = match layer.remove("layer") {
                Some(Value::String(s)) if !s.is_empty() => s,
                _ => return Err(bad(format!("layer {i} needs a \"layer\" name"))),
            };
            let Some(Value::Object(mut ringframe)) = layer.remove("ringframe") else {
                return Err(bad(format!("layer {name} needs a \"ringframe\" object")));
            };
            let deltas = match ringframe.remove("deltas") {
                None => Map::new(),
                Some(Value::Object(d)) => d,
                Some(_) => return Err(bad(format!("layer {name}: deltas must be an object"))),
            };
            if let Some(key) = layer.keys().chain(ringframe.keys()).next() {
                return Err(bad(format!("layer {name}: unknown key \"{key}\"")));
            }
            if let Some((path, _)) = deltas.iter().find(|(_, d)| !d.is_object()) {
                return Err(bad(format!("layer {name}: deltas.\"{path}\" must be an object")));
            }
            layers.push((name, deltas));
        }
        Ok(Overrides(layers))
    }

    /// Each layer's document for `path`, in order.
    pub fn documents<'a>(&'a self, path: &'a str) -> impl Iterator<Item = (&'a str, &'a Value)> {
        self.0.iter().filter_map(move |(name, deltas)| Some((name.as_str(), deltas.get(path)?)))
    }

    /// The practice domains any layer names.
    pub fn domains(&self) -> impl Iterator<Item = &str> {
        self.0.iter().flat_map(|(_, d)| d.keys()).filter_map(|k| k.strip_prefix("practices/"))
    }
}

/// The override folders this release no longer reads, where they still exist.
pub fn leftover_folders(ws: &crate::workspace::Workspace) -> Vec<PathBuf> {
    [home().join("overrides"), ws.rf_dir().join("deltas")]
        .into_iter()
        .filter(|p| p.exists())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(text: &str) -> Value {
        load_toml_text(text, "t.toml").unwrap()
    }

    #[test]
    fn toml_identity_ignores_comments_and_formatting() {
        let a =
            parse("schema = \"x/1\"\nname = \"demo\"   # a comment\nitems = [\"one\", \"two\"]\n");
        let b = parse(
            "# different layout, same document\nitems = [\n  \"one\",\n  \"two\",\n]\nname = 'demo'\nschema = \"x/1\"\n",
        );
        let want = json!({"schema": "x/1", "name": "demo", "items": ["one", "two"]});
        assert_eq!(a, want);
        assert_eq!(b, want);
        assert_eq!(sha256_of(&a), sha256_of(&b));
    }

    #[test]
    fn the_loader_refuses_what_json_cannot_hold_and_an_empty_file_is_empty() {
        let e = load_toml_text("at = 1979-05-27T07:32:00Z\n", "bad.toml").unwrap_err();
        assert!(e.0.starts_with("bad.toml: unsupported date-time"), "{e}");
        let e = load_toml_text("schema: x/1\n", "yaml.toml").unwrap_err();
        assert!(e.0.starts_with("yaml.toml:"), "{e}");
        assert_eq!(load_toml_text("# only a comment\n", "t.toml").unwrap(), json!({}));
    }

    #[test]
    fn a_yaml_file_where_toml_is_read_is_refused_naming_both() {
        let dir = crate::testing::tmp_dir();
        let toml_path = dir.path().join("codex.toml");
        std::fs::write(&toml_path, "schema = \"x/1\"\n").unwrap();
        assert_eq!(load_toml(&toml_path).unwrap(), json!({"schema": "x/1"}));
        std::fs::write(dir.path().join("codex.yaml"), "schema: x/1\n").unwrap();
        std::fs::write(dir.path().join("other.yaml"), "schema: x/1\n").unwrap();
        assert_eq!(stems(dir.path()), ["codex", "other"]);
        for path in [toml_path.clone(), dir.path().join("other.toml")] {
            let e = load_toml(&path).unwrap_err();
            let yaml = path.with_extension("yaml");
            assert!(e.0.starts_with("config.yaml_found: "), "{e}");
            assert!(e.0.contains(&yaml.display().to_string()), "{e}");
            assert!(e.0.contains(&path.display().to_string()), "{e}");
        }
    }

    #[test]
    fn a_yaml_file_left_in_the_synced_mirror_is_refused() {
        use crate::testing::with_config_home;
        with_config_home(|_| {
            let toml_path = config_dir().join("harnesses/codex.toml");
            std::fs::rename(&toml_path, toml_path.with_extension("yaml")).unwrap();
            let e = crate::profiles::load("codex").unwrap_err();
            assert!(e.0.starts_with("config.yaml_found: "), "{e}");
            assert!(e.0.contains("harnesses/codex.yaml"), "{e}");
            assert!(e.0.contains("harnesses/codex.toml"), "{e}");
            let host = config_dir().join("deltas/claude-code.toml");
            std::fs::rename(&host, host.with_extension("yaml")).unwrap();
            let e =
                crate::deltas::load_host_catalog("claude-code", &Overrides::default()).unwrap_err();
            assert!(e.0.contains("deltas/claude-code.yaml"), "{e}");
        });
    }

    /// The conversion's check, made once against the values: each TOML file
    /// digests to what its YAML parsed to, less the nulls TOML cannot hold. A
    /// deliberate change to a file's content changes its line here.
    #[test]
    fn every_shipped_toml_file_holds_what_its_yaml_held() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let shipped = here.join("../../../fab7/products/ringframe");
        let fixture = here.join("tests/fixtures/config");
        let mut roots = vec![(
            fixture,
            vec![
                (
                    "deltas/claude-code.toml",
                    "8e8df4d73344e3797e984a9a484e1da9fb5dee0e9337cfee701b49cb8027b630",
                ),
                (
                    "deltas/codex.toml",
                    "ad74ec85c229d37b4336518796f070320b62ddf2be302b36406ccf5272a1ed00",
                ),
                (
                    "deltas/practices/software-development.toml",
                    "8c7d2406cc02eb257f1d6c90d3d7ec1baaa7468d20bbd9f599c1353bd1ee1496",
                ),
                (
                    "harnesses/claude-code.toml",
                    "0a2545c1a1c24a3b8c03c976251adc8e12e2676a06d917a087f6856f06e4b069",
                ),
                (
                    "harnesses/codex.toml",
                    "e050482ebfd4aba59b688f57f2ca5d0a1d6cbbee03e70bcaca1eb249290ced6b",
                ),
                (
                    "harnesses/unknown.toml",
                    "dfedc7b4b74791f7e91d747c54fecf12e1b3bc8bf5d1186b2d72d6cb20673bfc",
                ),
            ],
        )];
        // The marketplace is checked too when it happens to be beside us.
        if shipped.is_dir() {
            roots.push((
                shipped,
                vec![
                    (
                        "bundle.toml",
                        "5d0f73340532af8dbe0205ce6c0465b223219dd0cdc1fe05bc59139d78df128c",
                    ),
                    (
                        "config/deltas/claude-code.toml",
                        "ef50e33661581006d879be728b3c089653d9b85d2ae07780327628e5d64231fb",
                    ),
                    (
                        "config/deltas/codex.toml",
                        "449f3a3c7045aee262bf130965c513b698e3fb49c4c44d30588cd53c5dd75480",
                    ),
                    (
                        "config/deltas/practices/autonomous-trading.toml",
                        "39466af5142c804275669dd55d036e6464d863a3b581bbacfa1ed71b57c2c2a3",
                    ),
                    (
                        "config/deltas/practices/software-development.toml",
                        "8c7d2406cc02eb257f1d6c90d3d7ec1baaa7468d20bbd9f599c1353bd1ee1496",
                    ),
                    (
                        "config/harnesses/claude-code.toml",
                        "0a2545c1a1c24a3b8c03c976251adc8e12e2676a06d917a087f6856f06e4b069",
                    ),
                    (
                        "config/harnesses/codex.toml",
                        "e050482ebfd4aba59b688f57f2ca5d0a1d6cbbee03e70bcaca1eb249290ced6b",
                    ),
                    (
                        "config/harnesses/unknown.toml",
                        "dfedc7b4b74791f7e91d747c54fecf12e1b3bc8bf5d1186b2d72d6cb20673bfc",
                    ),
                ],
            ));
        }
        for (root, files) in roots {
            for (rel, want) in files {
                let doc = load_toml(&root.join(rel)).unwrap_or_else(|e| panic!("{e}"));
                assert_eq!(sha256_of(&doc), want, "{}", root.join(rel).display());
            }
        }
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
    fn global_and_project_configuration_share_rf_without_creating_rt() {
        use crate::{
            deltas, profiles,
            testing::{repo, with_config_home, ws_for},
        };
        with_config_home(|home| {
            let repo = repo();
            let ws = ws_for(repo.path());
            assert_eq!(super::home(), home.join(".fab7/rf"));
            assert_eq!(
                profiles::load("codex").unwrap()["confirmation"]["tool"],
                "request_user_input"
            );
            // The project holds no configuration, and the home only the mirror.
            assert!(!ws.rf_dir().join("deltas").exists());
            let mut held: Vec<String> = std::fs::read_dir(home.join(".fab7/rf"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            held.sort();
            assert_eq!(held, ["config"]);
            assert_eq!(
                std::fs::read_to_string(config_dir().join(".revision")).unwrap().trim(),
                "local"
            );
            assert!(!home.join(".fab7/rt").exists());
            assert!(!ws.root.join(".fab7/rt").exists());
            let _ = deltas::revision();
        });
    }

    fn over(layers: Value) -> Overrides {
        Overrides::parse(&layers.to_string()).unwrap()
    }

    #[test]
    fn a_later_override_layer_wins_by_id_and_each_is_named() {
        use crate::{deltas, profiles, testing::with_config_home};
        with_config_home(|_| {
            let mut doc =
                load_toml(&config_dir().join("deltas/practices/software-development.toml"))
                    .unwrap();
            doc["render"]["core_cap"] = json!(1);
            doc["entries"][0]["text"] = json!("Machine rule.");
            let layers = over(json!([
                {"layer": "weft", "ringframe": {"deltas": {"practices/software-development": doc}}},
                {"layer": "weft-project", "ringframe": {"deltas": {
                    "practices/software-development": {"render": {"core_cap": 2},
                        "entries": [{"id": "practice.kiss", "text": "Project rule."}]},
                    "codex": {"entries": [{"id": "codex.native_plan.hand_back",
                        "status": "qualified", "text": "Project host rule."}]}}}},
            ]));
            let result = deltas::render(
                &layers,
                &profiles::load("codex").unwrap(),
                "native_plan",
                &json!({"task": ["implement"]}),
                &["qualified".to_string()],
                deltas::DEFAULT_DOMAIN,
            )
            .unwrap();
            assert_eq!(result["practice"]["selected"], json!(["practice.kiss", "practice.yagni"]));
            let text = result["text"].as_str().unwrap();
            assert!(text.contains("Project rule."), "{text}");
            assert!(!text.contains("Machine rule."));
            assert!(text.contains("Project host rule."));
            let roots: Vec<&Value> = result["practice"]["layers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| &l["root"])
                .collect();
            assert_eq!(roots, ["config", "weft", "weft-project"]);
            let listing = deltas::effective(&layers, deltas::DEFAULT_DOMAIN).unwrap();
            let kiss = &listing.iter().find(|(k, _)| k == "practice.kiss").unwrap().1;
            assert_eq!(kiss["layer"], "weft-project");
        });
    }

    #[test]
    fn an_empty_override_inherits_and_an_override_can_clear_entries() {
        use crate::{deltas, testing::with_config_home};
        with_config_home(|_| {
            let layer = |doc: Value| {
                over(json!([{"layer": "weft", "ringframe": {"deltas": {
                    "practices/software-development": doc}}}]))
            };
            let original =
                deltas::effective(&Overrides::default(), deltas::DEFAULT_DOMAIN).unwrap();
            assert!(original.iter().any(|(k, _)| k == "practice.kiss"));
            let cleared = layer(json!({"entries": []}));
            assert!(deltas::effective(&cleared, deltas::DEFAULT_DOMAIN).unwrap().is_empty());
            let empty = layer(json!({}));
            assert_eq!(deltas::effective(&empty, deltas::DEFAULT_DOMAIN).unwrap(), original);
        });
    }

    #[test]
    fn delta_merge_preserves_earlier_nested_fields_and_later_list_values() {
        use crate::{deltas, testing::with_config_home};
        with_config_home(|_| {
            let layers = over(json!([
                {"layer": "weft", "ringframe": {"deltas": {"practices/software-development":
                    {"entries": [{"id": "practice.kiss",
                        "applies_to": {"task": ["implement"], "result": ["workspace_change"]}}]}}}},
                {"layer": "weft-project", "ringframe": {"deltas": {"practices/software-development":
                    {"entries": [{"id": "practice.kiss", "applies_to": {"task": ["plan"]},
                        "why": null}]}}}},
            ]));
            let listing = deltas::effective(&layers, deltas::DEFAULT_DOMAIN).unwrap();
            let merged = &listing.iter().find(|(k, _)| k == "practice.kiss").unwrap().1;
            assert_eq!(
                merged["applies_to"],
                json!({"task": ["plan"], "result": ["workspace_change"]})
            );
            assert_eq!(merged["why"], json!(null));
        });
    }

    #[test]
    fn a_malformed_override_is_refused_naming_what_is_wrong() {
        for (text, says) in [
            ("{", "--override is not JSON"),
            (r#"{"layer": "weft"}"#, "--override must be a list of layers"),
            ("[1]", "--override layer 0 must be an object"),
            (r#"[{"ringframe": {}}]"#, r#"--override layer 0 needs a "layer" name"#),
            (r#"[{"layer": "weft"}]"#, r#"--override layer weft needs a "ringframe" object"#),
            (r#"[{"layer": "weft", "ringframe": {"deltas": []}}]"#, "deltas must be an object"),
            (r#"[{"layer": "weft", "ringframe": {"routing": {}}}]"#, r#"unknown key "routing""#),
            (
                r#"[{"layer": "w", "ringframe": {}, "x": 1}]"#,
                r#"--override layer w: unknown key "x""#,
            ),
            (
                r#"[{"layer": "weft", "ringframe": {"deltas": {"codex": []}}}]"#,
                r#"deltas."codex" must be an object"#,
            ),
        ] {
            let e = Overrides::parse(text).unwrap_err();
            assert!(e.0.contains(says), "{text}: {e}");
        }
        assert!(Overrides::parse("[]").is_ok());
    }
}
