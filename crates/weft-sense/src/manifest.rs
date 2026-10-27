//! A bundle's `manifest.json`: its files and the heads it describes. Bundle 3
//! lists its heads; a bundle 2 manifest is read as its two softmax heads.

use anyhow::{Context, bail};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const BUNDLE_2: &str = "fab7.sense.bundle/2";
pub const BUNDLE_3: &str = "fab7.sense.bundle/3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadKind {
    Softmax,
    Sigmoid,
    Noul,
    Choice,
    Derived,
    Pair,
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
    /// Per class, for `noul` and `sigmoid` heads.
    #[serde(default)]
    pub thresholds: Vec<f32>,
    /// For a `pair` head that proposes at or above it.
    pub threshold: Option<f32>,
    pub unsure_below: Option<f32>,
    pub min_confidence_bp: Option<u16>,
    pub weights: Option<String>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub schema: String,
    pub version: String,
    pub max_seq_len: usize,
    pub max_bottom_lines: usize,
    pub heads: Vec<Head>,
    pub files: BTreeMap<String, FileEntry>,
}

/// Both schemas' fields; bundle 2's head fields sit at the top level.
#[derive(Deserialize)]
struct RawManifest {
    schema: String,
    version: String,
    #[serde(default = "seq_len")]
    max_seq_len: usize,
    #[serde(default = "bottom_lines")]
    max_bottom_lines: usize,
    #[serde(default)]
    heads: Vec<Head>,
    files: BTreeMap<String, FileEntry>,
    #[serde(default)]
    classes: Vec<String>,
    #[serde(default)]
    conversation_classes: Vec<String>,
    #[serde(default = "one")]
    temperature: f32,
    #[serde(default = "one")]
    conversation_temperature: f32,
    min_confidence_bp: Option<u16>,
}

fn seq_len() -> usize {
    256
}

fn bottom_lines() -> usize {
    15
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        let raw: RawManifest = serde_json::from_slice(bytes).context("parse manifest.json")?;
        let heads = match raw.schema.as_str() {
            BUNDLE_3 => raw.heads,
            BUNDLE_2 => bundle_2_heads(&raw),
            other => bail!("unsupported Sense bundle schema: {other}"),
        };
        Ok(Self {
            schema: raw.schema,
            version: raw.version,
            max_seq_len: raw.max_seq_len,
            max_bottom_lines: raw.max_bottom_lines,
            heads,
            files: raw.files,
        })
    }

    /// The head named `name`, or an error that names it.
    pub fn head(&self, name: &str) -> anyhow::Result<&Head> {
        self.heads
            .iter()
            .find(|h| h.name == name)
            .with_context(|| format!("Sense bundle {} has no {name} head", self.version))
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

fn bundle_2_heads(raw: &RawManifest) -> Vec<Head> {
    let softmax = |name: &str, classes: &[String], temperature| Head {
        name: name.into(),
        kind: HeadKind::Softmax,
        classes: classes.to_vec(),
        questions: Vec::new(),
        temperature,
        thresholds: Vec::new(),
        threshold: None,
        unsure_below: None,
        min_confidence_bp: raw.min_confidence_bp,
        weights: None,
    };
    vec![
        softmax("surface", &raw.classes, raw.temperature),
        softmax("conversation", &raw.conversation_classes, raw.conversation_temperature),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUNDLE_2_JSON: &str = r#"{"schema": "fab7.sense.bundle/2", "version": "0.2.0",
        "temperature": 1.5, "conversation_temperature": 1.17, "min_confidence_bp": 7500,
        "classes": ["idle_ready", "error_or_limit"], "conversation_classes": ["agent_reply"],
        "files": {"model": {"path": "sense.int8.onnx", "sha256": "ab"}}}"#;

    const BUNDLE_3_JSON: &str = r#"{"schema": "fab7.sense.bundle/3", "version": "0.3.0",
        "heads": [
          {"name": "surface", "kind": "softmax", "classes": ["idle_ready"], "temperature": 1.5},
          {"name": "ask_route", "kind": "choice", "classes": ["direct", "plan"],
           "questions": ["now", "later"], "temperature": 0.9, "unsure_below": 0.5},
          {"name": "ask_effects", "kind": "noul", "classes": ["read"], "questions": ["reads"],
           "thresholds": [0.7]},
          {"name": "rule_relevance", "kind": "pair", "weights": "pair_rule.safetensors",
           "threshold": 0.39},
          {"name": "someday", "kind": "hologram"}],
        "files": {}}"#;

    #[test]
    fn a_bundle_2_manifest_reads_as_its_two_softmax_heads() {
        let m = Manifest::parse(BUNDLE_2_JSON.as_bytes()).unwrap();
        let surface = m.head("surface").unwrap();
        assert_eq!(surface.kind, HeadKind::Softmax);
        assert_eq!(surface.classes, ["idle_ready", "error_or_limit"]);
        assert_eq!((surface.temperature, surface.min_confidence_bp), (1.5, Some(7500)));
        let conversation = m.head("conversation").unwrap();
        assert_eq!((conversation.classes.len(), conversation.temperature), (1, 1.17));
        assert_eq!(m.file("model").unwrap().path, "sense.int8.onnx");
        assert_eq!(m.heads.len(), 2, "no new heads");
    }

    #[test]
    fn a_bundle_3_manifest_reads_every_head_it_lists() {
        let m = Manifest::parse(BUNDLE_3_JSON.as_bytes()).unwrap();
        let route = m.head("ask_route").unwrap();
        assert_eq!(
            (route.kind, route.temperature, route.unsure_below),
            (HeadKind::Choice, 0.9, Some(0.5))
        );
        assert_eq!(route.questions, ["now", "later"]);
        assert_eq!(m.head("ask_effects").unwrap().thresholds, [0.7]);
        let rule = m.head("rule_relevance").unwrap();
        assert_eq!((rule.kind, rule.threshold), (HeadKind::Pair, Some(0.39)));
        assert_eq!(m.head("someday").unwrap().kind, HeadKind::Unknown, "ignored, not refused");
    }

    #[test]
    fn a_missing_head_or_file_is_an_error_that_names_it() {
        let m = Manifest::parse(BUNDLE_2_JSON.as_bytes()).unwrap();
        let err = m.head("ask_task").unwrap_err().to_string();
        assert!(err.contains("ask_task"), "{err}");
        let err = m.file("ask_model").unwrap_err().to_string();
        assert!(err.contains("ask_model"), "{err}");
    }

    #[test]
    fn a_bundle_whose_manifest_is_not_the_one_expected_fails_naming_folder_and_digest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("manifest.json"), BUNDLE_2_JSON).unwrap();
        let found = crate::sha256_hex(BUNDLE_2_JSON.as_bytes());
        assert_eq!(bundle_with_digest(dir.path(), &found).unwrap(), dir.path());
        let err = bundle_with_digest(dir.path(), &"0".repeat(64)).unwrap_err().to_string();
        assert!(err.contains(&dir.path().display().to_string()), "{err}");
        assert!(err.contains(&found), "{err}");
    }

    #[test]
    fn an_unknown_schema_is_refused() {
        let json = BUNDLE_2_JSON.replace("bundle/2", "bundle/9");
        let err = Manifest::parse(json.as_bytes()).unwrap_err().to_string();
        assert!(err.contains("fab7.sense.bundle/9"), "{err}");
    }
}
