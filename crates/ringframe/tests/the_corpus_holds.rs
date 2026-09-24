//! The canonical-JSON gate.
//!
//! RingFrame's evidence rests on one function. Briefs, intents, judgements,
//! ledger events, `response_sha256`, config digests — all of them are a
//! sha256 over:
//!
//!     json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
//!
//! `corpus/canonical/samples.jsonl` is a frozen reference set: real artifacts
//! and the edge cases real data misses, each paired with its expected bytes.
//! It was generated once and is the reference — nothing here gets to decide
//! what canonical means.
//!
//! It holds `store::canonical` for JSON. If it drifts, this says so here
//! rather than in a digest mismatch on someone's ledger. The config samples
//! are YAML, which this release no longer reads: they sit out the first test,
//! and the other two still hold their canonical forms.

use std::path::PathBuf;

use serde_json::Value;

struct Sample {
    name: String,
    kind: String,
    input: String,
    canonical: String,
    sha256: String,
}

fn corpus() -> Vec<Sample> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/canonical/samples.jsonl");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rows: Vec<Sample> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("a corpus line is not JSON");
            let s = |k: &str| v[k].as_str().unwrap_or_else(|| panic!("no {k} in {l}")).to_string();
            Sample {
                name: s("name"),
                kind: s("kind"),
                input: s("input"),
                canonical: s("canonical"),
                sha256: s("sha256"),
            }
        })
        .collect();
    assert!(rows.len() > 50, "the corpus is too small to prove anything: {}", rows.len());
    assert!(rows.iter().any(|s| s.kind == "yaml"), "the corpus lost its config samples");
    rows
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// What the port must produce: the core's own function, not a copy of it.
fn canonical(value: &Value) -> String {
    String::from_utf8(ringframe::store::canonical(value)).expect("canonical JSON is UTF-8")
}

#[test]
fn every_sample_canonicalises_the_way_python_did() {
    let mut wrong = Vec::new();
    for s in corpus() {
        if s.kind == "yaml" {
            continue;
        }
        let parsed: Value = match serde_json::from_str(&s.input) {
            Ok(v) => v,
            Err(e) => {
                wrong.push(format!("{}: input does not parse: {e}", s.name));
                continue;
            }
        };
        let got = canonical(&parsed);
        if got != s.canonical {
            wrong.push(format!(
                "{}\n     python: {}\n     rust:   {}",
                s.name,
                cut(&s.canonical),
                cut(&got)
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} samples differ from the reference:\n  {}",
        wrong.len(),
        wrong.join("\n  ")
    );
}

#[test]
fn every_digest_is_of_the_canonical_bytes() {
    // The corpus carries its own digests. If they ever stop agreeing with the
    // bytes beside them, the file was edited by hand and is no longer evidence.
    for s in corpus() {
        assert_eq!(digest(s.canonical.as_bytes()), s.sha256, "{}", s.name);
    }
}

#[test]
fn canonicalising_the_canonical_form_changes_nothing() {
    // A canonical form that is not a fixed point cannot be re-derived from a
    // stored artifact, which is what verification does.
    for s in corpus() {
        let parsed: Value = serde_json::from_str(&s.canonical).expect(&s.name);
        assert_eq!(canonical(&parsed), s.canonical, "{}", s.name);
    }
}

fn cut(s: &str) -> String {
    if s.chars().count() <= 120 {
        s.to_string()
    } else {
        s.chars().take(120).collect::<String>() + "…"
    }
}
