//! Atomic artifact publish, locked append-only ledger, and verification.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::digest;
use crate::workspace::Workspace;

pub const SCHEMA: &str = "ringframe.ledger/1";
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub struct LedgerError {
    pub code: String,
    pub detail: String,
}

impl LedgerError {
    pub fn schema(detail: impl Into<String>) -> Self {
        LedgerError::new("ledger.schema", detail)
    }

    fn new(code: &str, detail: impl Into<String>) -> Self {
        LedgerError { code: code.to_string(), detail: detail.into() }
    }
}

impl std::fmt::Display for LedgerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "{}", self.code)
        } else {
            write!(f, "{}: {}", self.code, self.detail)
        }
    }
}

impl std::error::Error for LedgerError {}

impl From<std::io::Error> for LedgerError {
    fn from(e: std::io::Error) -> Self {
        LedgerError::new("ledger.io", e.to_string())
    }
}

/// The one serialisation every digest in the product is taken over.
///
/// `weft/corpus/` holds what the Python core produced for this function, and
/// `cargo test --test the_corpus_holds` compares this against it byte for
/// byte. Do not make this clever.
pub fn canonical(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("a JSON value always serialises")
}

fn fsync_dir(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

/// Write bytes to tmp, fsync, rename into place; final paths are never
/// rewritten.
pub fn publish(
    ws: &Workspace,
    rel_path: &str,
    data: &[u8],
    role: &str,
) -> Result<Value, LedgerError> {
    ws.ensure()?;
    let final_path = ws.rf_dir().join(rel_path);
    if final_path.exists() {
        return Err(LedgerError::new("ledger.immutable", rel_path));
    }
    if let Some(parent) = final_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = ws.rf_dir().join("tmp").join(format!(
        "{}.{}.{}",
        rel_path.replace('/', "."),
        std::process::id(),
        crate::ids::random_hex(4)
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &final_path)?;
    fsync_dir(final_path.parent().expect("a published path has a parent"))?;
    Ok(digest::artifact_ref(role, rel_path, &final_path)?)
}

/// An advisory exclusive lock, released when the guard drops.
struct Lock(std::fs::File);

impl Lock {
    fn acquire(path: &Path) -> Result<Self, LedgerError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .open(path)?;
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            // SAFETY: a valid fd owned by `file` for the duration of the call.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(Lock(file));
            }
            if Instant::now() > deadline {
                return Err(LedgerError::new("ledger.lock_timeout", path.to_string_lossy()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: the fd is still open; the file is dropped right after.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

use std::os::unix::fs::OpenOptionsExt;

pub fn append(ws: &Workspace, event: &Value) -> Result<(), LedgerError> {
    ws.ensure()?;
    let mut line = canonical(event);
    line.push(b'\n');
    let ledger = ws.rf_dir().join("ledger.jsonl");
    let _lock = Lock::acquire(&ws.rf_dir().join("lock"))?;
    if let Ok(meta) = std::fs::metadata(&ledger)
        && meta.len() > 0
    {
        let mut f = std::fs::File::open(&ledger)?;
        f.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            return Err(LedgerError::new("ledger.torn_tail", "last line has no newline"));
        }
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&ledger)?;
    out.write_all(&line)?;
    out.sync_all()?;
    Ok(())
}

pub fn events(ws: &Workspace) -> Result<Vec<Value>, LedgerError> {
    let ledger = ws.rf_dir().join("ledger.jsonl");
    if !ledger.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&ledger)?;
    let mut out = Vec::new();
    for line in raw.lines() {
        if !line.trim().is_empty() {
            out.push(
                serde_json::from_str(line)
                    .map_err(|e| LedgerError::new("ledger.invalid_json", e.to_string()))?,
            );
        }
    }
    Ok(out)
}

/// Every artifact reference reachable from a value, in document order.
fn refs(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            if ["role", "path", "bytes", "sha256"].iter().all(|k| map.contains_key(*k)) {
                out.push(value.clone());
            }
            for v in map.values() {
                refs(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                refs(v, out);
            }
        }
        _ => {}
    }
}

fn finding(code: &str) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("code".into(), Value::String(code.into()));
    m
}

/// Stream the ledger; report findings without repairing anything.
pub fn verify(ws: &Workspace) -> Result<Vec<Value>, LedgerError> {
    let mut findings: Vec<Value> = Vec::new();
    let ledger = ws.rf_dir().join("ledger.jsonl");
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut links: Vec<(usize, String)> = Vec::new();
    let mut deliveries: BTreeMap<String, usize> = BTreeMap::new();
    let mut confirmed: BTreeSet<String> = BTreeSet::new();

    if ledger.exists() {
        let raw = std::fs::read(&ledger)?;
        if !raw.is_empty() && !raw.ends_with(b"\n") {
            findings.push(Value::Object(finding("ledger.torn_tail")));
        }
        for (n, line) in raw.split(|b| *b == b'\n').enumerate() {
            let n = n + 1;
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(ev) = serde_json::from_slice::<Value>(line) else {
                let mut f = finding("ledger.invalid_json");
                f.insert("line".into(), n.into());
                findings.push(Value::Object(f));
                continue;
            };
            if ev.get("schema").and_then(Value::as_str) != Some(SCHEMA) {
                let mut f = finding("ledger.schema");
                f.insert("line".into(), n.into());
                f.insert("path".into(), "schema".into());
                findings.push(Value::Object(f));
            }
            let id = ev.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
            ids.insert(id.clone());
            if let Some(list) = ev.get("links").and_then(Value::as_array) {
                for link in list {
                    links.push((
                        n,
                        link.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    ));
                }
            }
            match ev.get("type").and_then(Value::as_str) {
                Some("ask.confirmed") => {
                    confirmed.insert(id.clone());
                }
                Some("ask.delivery") => {
                    *deliveries.entry(id.clone()).or_default() += 1;
                }
                _ => {}
            }
            let mut found = Vec::new();
            refs(ev.get("data").unwrap_or(&Value::Null), &mut found);
            for r in found {
                let rel = r["path"].as_str().unwrap_or_default().to_string();
                referenced.insert(rel.clone());
                let p = ws.rf_dir().join(&rel);
                let ok = match std::fs::metadata(&p) {
                    Err(_) => {
                        let mut f = finding("artifact.missing");
                        f.insert("line".into(), n.into());
                        f.insert("path".into(), rel.clone().into());
                        findings.push(Value::Object(f));
                        continue;
                    }
                    Ok(meta) => {
                        meta.len() == r["bytes"].as_u64().unwrap_or(u64::MAX)
                            && digest::sha256_file(&p)? == r["sha256"].as_str().unwrap_or_default()
                    }
                };
                if !ok {
                    let mut f = finding("artifact.digest_mismatch");
                    f.insert("line".into(), n.into());
                    f.insert("path".into(), rel.into());
                    findings.push(Value::Object(f));
                }
            }
        }
    }
    for (n, target) in links {
        if !ids.contains(&target) {
            let mut f = finding("links.dangling");
            f.insert("line".into(), n.into());
            f.insert("id".into(), target.into());
            findings.push(Value::Object(f));
        }
    }
    for (ask_id, count) in deliveries {
        if count > 1 {
            let mut f = finding("delivery.duplicate");
            f.insert("id".into(), ask_id.clone().into());
            findings.push(Value::Object(f));
        }
        if !confirmed.contains(&ask_id) {
            let mut f = finding("delivery.without_confirmation");
            f.insert("id".into(), ask_id.into());
            findings.push(Value::Object(f));
        }
    }
    for sub in ["asks", "evals", "seals"] {
        let base = ws.rf_dir().join(sub);
        for path in walk(&base) {
            let rel = path
                .strip_prefix(ws.rf_dir())
                .expect("under the workspace")
                .to_string_lossy()
                .to_string();
            if !referenced.contains(&rel) {
                let mut f = finding("artifact.unreferenced");
                f.insert("path".into(), rel.into());
                findings.push(Value::Object(f));
            }
        }
    }
    Ok(findings)
}

/// Every file under a directory, sorted, so findings come out in one order.
fn walk(base: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{repo, run, ws_for};
    use serde_json::json;

    fn codes(findings: &[Value]) -> Vec<String> {
        let mut out: Vec<String> =
            findings.iter().map(|f| f["code"].as_str().unwrap().to_string()).collect();
        out.sort();
        out
    }

    #[test]
    fn publish_is_atomic_and_immutable() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let r = publish(&ws, "asks/ask_x/source.txt", b"intent\n", "source_intent").unwrap();
        assert_eq!(
            std::fs::read(ws.rf_dir().join("asks/ask_x/source.txt")).unwrap(),
            b"intent\n"
        );
        assert_eq!(r["bytes"], 7);
        assert_eq!(r["role"], "source_intent");
        assert_eq!(std::fs::read_dir(ws.rf_dir().join("tmp")).unwrap().count(), 0);
        let again = publish(&ws, "asks/ask_x/source.txt", b"other\n", "source_intent");
        assert_eq!(again.unwrap_err().code, "ledger.immutable");
    }

    #[test]
    fn append_canonical_line_and_read_back() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let ev = json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "x", "time": "t",
            "id": "i", "actor": {"kind": "human", "id": "u"}, "links": [],
            "data": {"b": 1, "a": "ü"}
        });
        append(&ws, &ev).unwrap();
        let raw = std::fs::read(ws.rf_dir().join("ledger.jsonl")).unwrap();
        assert!(raw.ends_with(b"\n"));
        assert_eq!(raw.iter().filter(|b| **b == b'\n').count(), 1);
        // Keys sorted, no spaces, and non-ASCII left as UTF-8 rather than escaped.
        let needle: &[u8] = "\"data\":{\"a\":\"ü\",\"b\":1}".as_bytes();
        assert!(
            raw.windows(needle.len()).any(|w| w == needle),
            "{}",
            String::from_utf8_lossy(&raw)
        );
        assert_eq!(events(&ws).unwrap(), vec![ev]);
    }

    #[test]
    fn torn_tail_refuses_append() {
        let repo = repo();
        let ws = ws_for(repo.path());
        std::fs::write(ws.rf_dir().join("ledger.jsonl"), b"{\"partial\": ").unwrap();
        let e = append(&ws, &json!({"schema": "ringframe.ledger/1"})).unwrap_err();
        assert_eq!(e.code, "ledger.torn_tail");
    }

    #[test]
    fn concurrent_appends_never_tear() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let root = repo.path().to_path_buf();
        // Ten writers, a hundred events each, through the same lock. Each one
        // opens its own descriptor, so `flock` really has to serialise them.
        // The Python suite used processes; the cross-process claim is proved
        // against the real binary in `tests/the_cli_holds.rs`, once it exists.
        let writers: Vec<_> = (0..10)
            .map(|w| {
                let root = root.clone();
                std::thread::spawn(move || {
                    let ws = crate::workspace::resolve(Some(&root), None).unwrap();
                    for i in 0..100 {
                        append(
                            &ws,
                            &json!({
                                "schema": "ringframe.ledger/1",
                                "event_id": format!("evt_{w}_{i}"), "type": "t", "time": "t",
                                "id": "i", "actor": {"kind": "human", "id": "u"},
                                "links": [], "data": {}
                            }),
                        )
                        .unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let raw = std::fs::read_to_string(ws.rf_dir().join("ledger.jsonl")).unwrap();
        let lines: Vec<&str> = raw.split('\n').collect();
        assert_eq!(*lines.last().unwrap(), "");
        assert_eq!(lines.len(), 1001);
        let seen: std::collections::HashSet<String> = lines[..1000]
            .iter()
            .map(|l| serde_json::from_str::<Value>(l).unwrap()["event_id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(seen.len(), 1000);
    }

    #[test]
    fn verify_reports_unreferenced_and_missing() {
        let repo = repo();
        let ws = ws_for(repo.path());
        publish(&ws, "asks/ask_a/source.txt", b"x", "source_intent").unwrap(); // never referenced
        let r = publish(&ws, "asks/ask_b/source.txt", b"y", "source_intent").unwrap();
        let r2 = publish(&ws, "asks/ask_b/prompt.txt", b"z", "generated_prompt").unwrap();
        append(
            &ws,
            &json!({
                "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.cancelled",
                "time": "t", "id": "ask_b", "actor": {"kind": "human", "id": "u"}, "links": [],
                "data": {"source": r, "prompt": r2}
            }),
        )
        .unwrap();
        std::fs::remove_file(ws.rf_dir().join("asks/ask_b/prompt.txt")).unwrap();
        assert_eq!(codes(&verify(&ws).unwrap()), ["artifact.missing", "artifact.unreferenced"]);
    }

    #[test]
    fn verify_detects_digest_mismatch_and_dangling_link() {
        let repo = repo();
        let ws = ws_for(repo.path());
        let r = publish(&ws, "asks/ask_b/source.txt", b"y", "source_intent").unwrap();
        run(&["chmod", "600", &ws.rf_dir().join("asks/ask_b/source.txt").to_string_lossy()]);
        std::fs::write(ws.rf_dir().join("asks/ask_b/source.txt"), b"tampered").unwrap();
        append(
            &ws,
            &json!({
                "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.cancelled",
                "time": "t", "id": "ask_b", "actor": {"kind": "human", "id": "u"},
                "links": [{"rel": "revises", "id": "ask_nope"}], "data": {"source": r}
            }),
        )
        .unwrap();
        assert_eq!(codes(&verify(&ws).unwrap()), ["artifact.digest_mismatch", "links.dangling"]);
    }
}
