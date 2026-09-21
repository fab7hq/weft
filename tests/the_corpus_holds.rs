//! The canonical-JSON gate (ADR-0013).
//!
//! RingFrame's evidence rests on one function. Briefs, intents, judgements,
//! ledger events, `response_sha256`, config digests — all of them are a
//! sha256 over:
//!
//!     json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
//!
//! `corpus/canonical/samples.jsonl` is what the Python core produced on real
//! artifacts and on the edge cases real data misses. It was generated once,
//! while the Python core still existed, and it is the reference: the Rust port
//! does not get to decide what canonical means.
//!
//! This is the first thing the port has to pass, before a line of it is
//! written. It is green today only because `serde_json::Value` orders its keys
//! and leaves non-ASCII alone; if that ever stops being true, this says so
//! here rather than in a digest mismatch on a customer's ledger.

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
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let rows: Vec<Sample> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("a corpus line is not JSON");
            let s = |k: &str| v[k].as_str().unwrap_or_else(|| panic!("no {k} in {l}")).to_string();
            Sample { name: s("name"), kind: s("kind"), input: s("input"), canonical: s("canonical"), sha256: s("sha256") }
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

/// What the port must produce. One place, so there is one thing to change when
/// `serde_json` is no longer enough.
fn canonical(value: &Value) -> String {
    serde_json::to_string(value).expect("a parsed value would not serialise")
}

#[test]
fn every_sample_canonicalises_the_way_python_did() {
    let mut wrong = Vec::new();
    // Config arrives as YAML, and what gets digested is its *parsed* form. That
    // half of the gate belongs to the YAML reader the port still has to choose,
    // so it is not this test's to answer; the two tests below still hold those
    // rows to their canonical bytes.
    for s in corpus().into_iter().filter(|s| s.kind == "json") {
        let parsed: Value = match serde_json::from_str(&s.input) {
            Ok(v) => v,
            Err(e) => {
                wrong.push(format!("{}: input does not parse: {e}", s.name));
                continue;
            }
        };
        let got = canonical(&parsed);
        if got != s.canonical {
            wrong.push(format!("{}\n     python: {}\n     rust:   {}", s.name, cut(&s.canonical), cut(&got)));
        }
    }
    assert!(wrong.is_empty(), "{} samples differ from the reference:\n  {}", wrong.len(), wrong.join("\n  "));
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
    if s.chars().count() <= 120 { s.to_string() } else { s.chars().take(120).collect::<String>() + "…" }
}
