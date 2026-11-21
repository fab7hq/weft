//! A Sense bundle's `manifest.json` (`fab7.sense.xs/1`): its files, its
//! normalizing table, its adapters and, under each, the heads it reads.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "fab7.sense.xs/1";
pub const NORMALIZER: &str = "fab7.sense.decision.normalize/2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadKind {
    Noul,
    Choice,
    Derived,
    /// One row per candidate, its question the head's `question` with the
    /// candidate's text in it (sense-xs).
    Document,
    /// A kind this runtime does not know: the head is ignored.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Head {
    pub name: String,
    /// The adapter that reads it.
    #[serde(skip)]
    pub adapter: String,
    pub kind: HeadKind,
    #[serde(default)]
    pub classes: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default = "one")]
    pub temperature: f32,
    /// Per class, for a `noul` head.
    #[serde(default)]
    pub thresholds: Vec<f32>,
    /// For a `choice` head: "unsure" below it.
    pub unsure_below: Option<f32>,
    /// For `turn_end` and a `document` head: the probability an answer must reach.
    pub sure: Option<f32>,
    /// For the `plan` head: how far the top candidate must lead the runner-up.
    pub margin: Option<f32>,
    /// For a `document` head: its question, `{document}` the candidate's text.
    pub question: Option<String>,
    /// For `rule_relevance`: how much a rule's counts weigh.
    pub memory: Option<Memory>,
    pub cut: Option<Cut>,
    /// For `rule_relevance`: the rules decided by the Ask's task and effects.
    #[serde(default)]
    pub convention: Vec<Convention>,
}

fn one() -> f32 {
    1.0
}

/// A rule's counts add `w · ln((kept + a) / (candidate − kept + a))` to its logit.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Memory {
    pub w: f32,
    pub a: f32,
}

/// A head's text keeps its `first` tokens.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Cut {
    pub first: usize,
}

/// These rules are on for an Ask with every one of `task` and, unless
/// `effects` is empty, any of `effects`.
#[derive(Debug, Clone, Deserialize)]
pub struct Convention {
    pub rules: Vec<String>,
    pub task: Vec<String>,
    pub effects: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Normalize {
    pub replacements: BTreeMap<String, String>,
    pub screen_last_lines: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

/// A sense-xs adapter: its file and the heads it reads.
#[derive(Debug, Clone, Deserialize)]
pub struct AdapterFile {
    pub name: String,
    pub file: String,
    pub sha256: String,
    pub heads: Vec<Head>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub version: String,
    pub normalizer: String,
    pub normalize: Normalize,
    /// Every head, each with its adapter's name.
    #[serde(skip)]
    pub heads: Vec<Head>,
    pub adapters: Vec<AdapterFile>,
    pub files: BTreeMap<String, FileEntry>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let schema: serde_json::Value =
            serde_json::from_slice(bytes).context("parse manifest.json")?;
        match schema["schema"].as_str() {
            Some(SCHEMA) => {}
            other => bail!("unsupported Sense bundle schema: {}", other.unwrap_or("none")),
        }
        let mut manifest: Self = serde_json::from_value(schema).context("read manifest.json")?;
        for adapter in &manifest.adapters {
            let heads = adapter.heads.iter().cloned();
            manifest.heads.extend(heads.map(|h| Head { adapter: adapter.name.clone(), ..h }));
        }
        if manifest.normalizer != NORMALIZER {
            bail!("unsupported Sense normalizer: {}", manifest.normalizer);
        }
        Ok(manifest)
    }

    /// The head named `name`, or an error that names it.
    pub fn head(&self, name: &str) -> anyhow::Result<&Head> {
        self.heads
            .iter()
            .find(|h| h.name == name)
            .with_context(|| format!("Sense bundle {} has no {name} head", self.version))
    }

    /// The head named `name`, which must be of `kind` and calibrated as it
    /// needs: one question per class, a threshold per `noul` class.
    pub fn head_of(&self, name: &str, kind: HeadKind) -> anyhow::Result<&Head> {
        let head = self.head(name)?;
        if head.kind != kind {
            bail!("the {name} head is {:?}, not {kind:?}", head.kind);
        }
        let (classes, questions) = (head.classes.len(), head.questions.len());
        let questioned = matches!(kind, HeadKind::Noul | HeadKind::Choice);
        if questioned && (classes == 0 || classes != questions) {
            bail!("the {name} head has {classes} classes and {questions} questions");
        }
        if kind == HeadKind::Noul && head.thresholds.len() != classes {
            bail!("the {name} head has {} thresholds for {classes} classes", head.thresholds.len());
        }
        Ok(head)
    }

    /// The adapter named `name`, or an error that names it.
    pub fn adapter(&self, name: &str) -> anyhow::Result<&AdapterFile> {
        self.adapters
            .iter()
            .find(|a| a.name == name)
            .with_context(|| format!("Sense bundle {} has no {name} adapter", self.version))
    }

    /// Every rule the `rule_relevance` head's convention decides: never
    /// scored by the model, never counted. None without the head.
    pub fn convention_rules(&self) -> Vec<String> {
        let head = self.head("rule_relevance").ok();
        let rules = head.into_iter().flat_map(|h| &h.convention).flat_map(|c| &c.rules);
        let mut rules: Vec<String> = rules.cloned().collect();
        rules.sort();
        rules.dedup();
        rules
    }

    /// The file the manifest lists under `key`, or an error that names it.
    pub fn file(&self, key: &str) -> anyhow::Result<&FileEntry> {
        self.files
            .get(key)
            .with_context(|| format!("Sense bundle {} lists no {key} file", self.version))
    }
}

/// `dir`, if its `manifest.json` is the one whose SHA-256 is `sha256`; the
/// manifest names every other file's digest, so this names the whole bundle.
pub fn bundle_with_digest(dir: &Path, sha256: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join("manifest.json");
    let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let found = crate::sha256_hex(&bytes);
    if found != sha256 {
        bail!("the Sense bundle in {} has manifest digest {found}, not {sha256}", dir.display());
    }
    Ok(dir.to_path_buf())
}

/// The Sense bundle tests read: `SENSE_XS_BUNDLE`'s folder (where CI puts the
/// pinned one, read even by a test that moves `HOME`), else the published bundle
/// in the sense repository beside this checkout, else `~/.fab7/sense/sense-xs`;
/// and only if its manifest is the one whose SHA-256 is `sha256`.
pub fn bundle_for_tests(sha256: &str) -> anyhow::Result<PathBuf> {
    let beside = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../sense/.trained-data/bundles/sense-xs-0.0.2");
    let dir = match std::env::var_os("SENSE_XS_BUNDLE") {
        Some(dir) => PathBuf::from(dir),
        None if beside.exists() => beside,
        None => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".fab7/sense/sense-xs")
        }
    };
    bundle_with_digest(&dir, sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XS: &str = r#"{"schema": "fab7.sense.xs/1", "version": "0.0.1",
        "base_model": "microsoft/deberta-v3-xsmall",
        "normalizer": "fab7.sense.decision.normalize/2",
        "normalize": {"replacements": {"\n": " ↵ "}, "screen_last_lines": 15},
        "adapters": [
          {"name": "state", "file": "adapters/state.onnx_adapter", "sha256": "aa",
           "heads": [{"name": "surface", "kind": "choice", "classes": ["idle_ready"],
                      "questions": ["idle"], "temperature": 0.5, "unsure_below": 0.6},
                     {"name": "conversation", "kind": "choice", "text": "entry",
                      "cut": {"first": 96}, "classes": ["agent_reply"], "questions": ["reply"]},
                     {"name": "someday", "kind": "hologram"}]},
          {"name": "ask", "file": "adapters/ask.onnx_adapter", "sha256": "bb",
           "heads": [{"name": "ask_effects", "kind": "noul", "classes": ["read"],
                      "questions": ["reads"], "thresholds": [0.7]},
                     {"name": "rule_relevance", "kind": "document", "document": "rule",
                      "question": "{document}", "sure": 0.6, "memory": {"w": 0.05, "a": 1.0},
                      "convention": [{"rules": ["practice.kiss"], "task": ["implement"],
                                      "effects": ["write"]}]},
                     {"name": "plan", "kind": "document", "question": "belongs: {document}",
                      "temperature": 0.3, "sure": 0.7, "margin": 0.15}]}],
        "files": {"model": {"path": "sense-xs.int8.onnx", "sha256": "ab"}}}"#;

    #[test]
    fn a_manifest_reads_each_head_with_its_adapter() {
        let m = Manifest::parse(XS.as_bytes()).unwrap();
        let surface = m.head_of("surface", HeadKind::Choice).unwrap();
        assert_eq!(surface.adapter, "state");
        assert_eq!((surface.temperature, surface.unsure_below), (0.5, Some(0.6)));
        assert_eq!(m.head("conversation").unwrap().cut.unwrap().first, 96);
        assert_eq!(m.head_of("ask_effects", HeadKind::Noul).unwrap().thresholds, [0.7]);
        let rule = m.head_of("rule_relevance", HeadKind::Document).unwrap();
        assert_eq!(rule.adapter, "ask");
        assert_eq!((rule.question.as_deref(), rule.sure), (Some("{document}"), Some(0.6)));
        assert_eq!(rule.memory, Some(Memory { w: 0.05, a: 1.0 }));
        assert_eq!(m.convention_rules(), ["practice.kiss"]);
        let plan = m.head_of("plan", HeadKind::Document).unwrap();
        assert_eq!((plan.sure, plan.margin), (Some(0.7), Some(0.15)));
        assert_eq!(m.head("someday").unwrap().kind, HeadKind::Unknown, "ignored, not refused");
        assert_eq!(m.normalize.replacements["\n"], " ↵ ");
        assert_eq!(m.file("model").unwrap().path, "sense-xs.int8.onnx");
        let ask = m.adapter("ask").unwrap();
        assert_eq!((ask.file.as_str(), ask.sha256.as_str()), ("adapters/ask.onnx_adapter", "bb"));
    }

    #[test]
    fn a_missing_head_adapter_or_file_is_an_error_that_names_it() {
        let m = Manifest::parse(XS.as_bytes()).unwrap();
        let err = m.head("ask_task").unwrap_err().to_string();
        assert!(err.contains("ask_task"), "{err}");
        let err = m.file("tokenizer").unwrap_err().to_string();
        assert!(err.contains("tokenizer"), "{err}");
        let err = m.head_of("surface", HeadKind::Noul).unwrap_err().to_string();
        assert!(err.contains("surface"), "{err}");
        let err = m.adapter("eval").unwrap_err().to_string();
        assert!(err.contains("eval"), "{err}");
    }

    #[test]
    fn sense_d_or_an_unknown_schema_or_normalizer_is_refused() {
        for schema in ["fab7.sense.decision/2", "fab7.sense.decision/3", "fab7.sense.xs/2"] {
            let json = XS.replace("fab7.sense.xs/1", schema);
            let err = Manifest::parse(json.as_bytes()).unwrap_err().to_string();
            assert!(err.contains(schema), "{err}");
        }
        let json = XS.replace("normalize/2", "normalize/1");
        let err = Manifest::parse(json.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("normalize/1"), "{err}");
    }

    #[test]
    fn a_bundle_whose_manifest_is_not_the_one_expected_fails_naming_folder_and_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), XS).unwrap();
        let found = crate::sha256_hex(XS.as_bytes());
        assert_eq!(bundle_with_digest(dir.path(), &found).unwrap(), dir.path());
        let err = bundle_with_digest(dir.path(), &"0".repeat(64)).unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains(&found), "{err}");
    }
}
