//! A sense-d bundle's `manifest.json` (sense-d spec/bundle.md §2): its
//! files, its normalizing table and the heads it describes. Only
//! `fab7.sense.decision/1` is read.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = "fab7.sense.decision/1";
pub const NORMALIZER: &str = "fab7.sense.decision.normalize/2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadKind {
    Noul,
    Choice,
    NoulPerItem,
    Derived,
    /// A kind this runtime does not know: the head is ignored.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Head {
    pub name: String,
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
    /// For a `noul_per_item` head that proposes at or above it.
    pub threshold: Option<f32>,
    /// For a `noul_per_item` head that flags at or below it.
    pub flag_below: Option<f32>,
    /// For a `choice` head: "unsure" below it.
    pub unsure_below: Option<f32>,
    pub cut: Option<Cut>,
    /// For `rule_relevance`: the rules decided by the Ask's task and effects.
    #[serde(default)]
    pub convention: Vec<Convention>,
}

fn one() -> f32 {
    1.0
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

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub version: String,
    pub normalizer: String,
    pub normalize: Normalize,
    pub heads: Vec<Head>,
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
        let manifest: Self = serde_json::from_value(schema).context("read manifest.json")?;
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

/// The sense-d bundle tests read: the sense repository's export beside this
/// checkout, or where CI puts the pinned one (`~/.fab7/sense/sense-d`), and
/// only if its manifest is the one whose SHA-256 is `sha256`.
pub fn bundle_for_tests(sha256: &str) -> anyhow::Result<PathBuf> {
    let beside = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../sense/decision/dist/sense-d");
    let dir = match beside.exists() {
        true => beside,
        false => {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".fab7/sense/sense-d")
        }
    };
    bundle_with_digest(&dir, sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"{"schema": "fab7.sense.decision/1", "version": "0.1.0",
        "base_model": "microsoft/deberta-v3-xsmall",
        "normalizer": "fab7.sense.decision.normalize/2",
        "normalize": {"replacements": {"\n": " ↵ "}, "screen_last_lines": 15},
        "heads": [
          {"name": "surface", "kind": "choice", "text": "screen", "classes": ["idle_ready"],
           "questions": ["idle"], "temperature": 0.5, "unsure_below": 0.51},
          {"name": "conversation", "kind": "choice", "text": "entry", "cut": {"first": 96},
           "classes": ["agent_reply"], "questions": ["reply"]},
          {"name": "ask_effects", "kind": "noul", "classes": ["read"], "questions": ["reads"],
           "thresholds": [0.7]},
          {"name": "rule_relevance", "kind": "noul_per_item", "question_from": "rule.when",
           "convention": [{"rules": ["practice.kiss"], "task": ["implement"], "effects": ["write"]}],
           "threshold": 0.52},
          {"name": "someday", "kind": "hologram"}],
        "files": {"model": {"path": "sense.int8.onnx", "sha256": "ab"}}}"#;

    #[test]
    fn a_sense_d_manifest_reads_every_head_it_lists() {
        let m = Manifest::parse(MANIFEST.as_bytes()).unwrap();
        let surface = m.head_of("surface", HeadKind::Choice).unwrap();
        assert_eq!((surface.temperature, surface.unsure_below), (0.5, Some(0.51)));
        assert_eq!(m.head("conversation").unwrap().cut.unwrap().first, 96);
        assert_eq!(m.head_of("ask_effects", HeadKind::Noul).unwrap().thresholds, [0.7]);
        let rule = m.head_of("rule_relevance", HeadKind::NoulPerItem).unwrap();
        assert_eq!(rule.threshold, Some(0.52));
        assert_eq!(rule.convention[0].rules, ["practice.kiss"]);
        assert_eq!(m.head("someday").unwrap().kind, HeadKind::Unknown, "ignored, not refused");
        assert_eq!(m.normalize.replacements["\n"], " ↵ ");
        assert_eq!(m.file("model").unwrap().path, "sense.int8.onnx");
    }

    #[test]
    fn a_missing_head_or_file_is_an_error_that_names_it() {
        let m = Manifest::parse(MANIFEST.as_bytes()).unwrap();
        let err = m.head("ask_task").unwrap_err().to_string();
        assert!(err.contains("ask_task"), "{err}");
        let err = m.file("tokenizer").unwrap_err().to_string();
        assert!(err.contains("tokenizer"), "{err}");
        let err = m.head_of("surface", HeadKind::Noul).unwrap_err().to_string();
        assert!(err.contains("surface"), "{err}");
    }

    #[test]
    fn an_earlier_bundle_or_an_unknown_normalizer_is_refused() {
        for schema in ["fab7.sense.bundle/2", "fab7.sense.bundle/3", "fab7.sense.decision/2"] {
            let json = MANIFEST.replace("fab7.sense.decision/1", schema);
            let err = Manifest::parse(json.as_bytes()).unwrap_err().to_string();
            assert!(err.contains(schema), "{err}");
        }
        let json = MANIFEST.replace("normalize/2", "normalize/1");
        let err = Manifest::parse(json.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("normalize/1"), "{err}");
    }

    #[test]
    fn a_bundle_whose_manifest_is_not_the_one_expected_fails_naming_folder_and_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), MANIFEST).unwrap();
        let found = crate::sha256_hex(MANIFEST.as_bytes());
        assert_eq!(bundle_with_digest(dir.path(), &found).unwrap(), dir.path());
        let err = bundle_with_digest(dir.path(), &"0".repeat(64)).unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains(&found), "{err}");
    }
}
