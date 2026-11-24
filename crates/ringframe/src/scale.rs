//! What the product's knowledge and an Eval cost as the product grows: files,
//! repositories and decisions. Run by hand, one at a time:
//!
//! `cargo nextest run -p ringframe --run-ignored only scale --no-capture`
//!
//! Decisions are written to the ledger as Seals would write them, each pinned
//! to two files by their content's hash; everything measured runs RingFrame's
//! own code over real repositories.

use crate::testing::*;
use crate::workspace::Workspace;
use serde_json::{Value, json};
use std::path::Path;
use std::time::Instant;

const PER_FOLDER: usize = 100;

fn body(i: usize) -> String {
    format!("// file {i}\npub fn f{i}() -> usize {{\n    {i}\n}}\n")
}

fn file(i: usize) -> String {
    format!("src/d{:04}/f{i:06}.rs", i / PER_FOLDER)
}

/// `n` source files, an instruction file in every tenth folder, one commit.
fn fill(root: &Path, n: usize) {
    for i in 0..n {
        let p = root.join(file(i));
        if i % PER_FOLDER == 0 {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            if (i / PER_FOLDER).is_multiple_of(10) {
                std::fs::write(p.parent().unwrap().join("AGENTS.md"), "Keep it short.\n").unwrap();
            }
        }
        std::fs::write(p, body(i)).unwrap();
    }
    std::fs::write(root.join("AGENTS.md"), "Write tests.\n").unwrap();
    commit(root, &[], "product");
}

/// `count` decisions in Seals of ten, each pinned to two of the product's
/// files, spread over its repositories.
fn decide(ws: &Workspace, repos: &[(String, usize)], count: usize) {
    let heads: Vec<String> = repos.iter().map(|(name, _)| commit_of(ws, name)).collect();
    let mut events = Vec::new();
    for s in 0..count.div_ceil(10) {
        let seal = format!("sl_scale{s:06}");
        let decisions: Vec<Value> = (0..10.min(count - s * 10))
            .map(|d| {
                let i = s * 10 + d;
                let r = i % repos.len();
                let (name, n) = &repos[r];
                let pins: Vec<Value> = [i * 7919, i * 104_729 + 13]
                    .iter()
                    .map(|k| {
                        let f = k % n;
                        json!({"repo": name, "path": file(f), "commit": heads[r],
                               "sha256": crate::digest::sha256_bytes(body(f).as_bytes())})
                    })
                    .collect();
                json!({"id": format!("{seal}/d{}", d + 1), "behaviour": format!("Keep f{i} as it is."),
                       "answer": "wanted", "pins": pins})
            })
            .collect();
        events.push(json!({"type": "seal.created", "id": seal, "time": "2026-11-24T00:00:00Z",
                           "data": {"decisions": decisions, "basis": {"asks": []}}}));
    }
    crate::store::append_all(ws, &events).unwrap();
}

fn commit_of(ws: &Workspace, repo: &str) -> String {
    crate::knowledge::commit_of(&crate::knowledge::repo_root(ws, repo), "HEAD").unwrap()
}

fn ms<T>(f: impl FnOnce() -> T) -> (T, u128) {
    let t = Instant::now();
    let out = f();
    (out, t.elapsed().as_millis())
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The first reading task of an Eval: the bytes of the brief its reader is
/// handed.
fn read_brief(ws: &Workspace, eval: &str) -> u64 {
    let got = crate::tasks::take(ws, eval, None).unwrap();
    let task = got["task"]["task_id"].as_str().expect("a task").to_string();
    let dir = ws.rf_dir().join("tmp").join(format!("eval-{eval}")).join("tasks");
    size(&dir.join(format!("{task}.brief.md")))
}

struct Product {
    _dirs: Vec<TempDir>,
    ws: Workspace,
    repos: Vec<(String, usize)>,
}

/// A product of `files` source files over `repos` repositories (the
/// workspace's own and siblings), with `decisions` decisions.
fn product(files: usize, repos: usize, decisions: usize) -> Product {
    let mut dirs = vec![repo()];
    let ws = ws_for(dirs[0].path());
    let per = files / repos;
    fill(&ws.root, per);
    let mut named = vec![(".".to_string(), per)];
    for _ in 1..repos {
        let other = repo();
        fill(other.path(), per);
        named.push((format!("../{}", other.path().file_name().unwrap().to_string_lossy()), per));
        dirs.push(other);
    }
    decide(&ws, &named, decisions);
    Product { _dirs: dirs, ws, repos: named }
}

/// Refresh, rebuild and the decisions list, then an Eval of a one-file change.
fn measure(files: usize, repos: usize, decisions: usize) {
    with_config_home(|_| {
        let (p, setup) = ms(|| product(files, repos, decisions));
        let ws = &p.ws;
        let (_, rebuild) = ms(|| crate::knowledge::rebuild(ws).unwrap());
        let (_, unchanged) = ms(|| crate::knowledge::refresh(ws).unwrap());
        // One pinned file changes in each repository.
        for (name, _) in &p.repos {
            let root = crate::knowledge::repo_root(ws, name);
            commit(&root, &[(&file(0), Some("// changed\n"))], "one file");
        }
        let (_, changed) = ms(|| crate::knowledge::refresh(ws).unwrap());
        let (shown, show) = ms(|| crate::knowledge::show(ws, None).unwrap());
        let stale = shown["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["stale"] == json!(true))
            .count();
        let view = size(&ws.rf_dir().join("knowledge/view.json"));
        // An Eval of a one-file change.
        let ask = confirm_ask(ws, b"Change f1.\n");
        commit(&ws.root, &[(&file(1), Some("// f1 changed\n"))], "work");
        let ask = ask["ask_id"].as_str().unwrap().to_string();
        let open = crate::evaluate::Open { ask: Some(&ask), ..Default::default() };
        let (out, eval_open) = ms(|| crate::evaluate::open_eval(ws, open).unwrap());
        let eval = out["eval_id"].as_str().unwrap();
        let brief = read_brief(ws, eval);
        eprintln!(
            "files {files:>6} repos {repos} decisions {decisions:>5} | setup {setup:>6} ms | rebuild {rebuild:>5} ms, \
             refresh unchanged {unchanged:>4} ms, after a change {changed:>5} ms, view {view:>8} B | \
             decisions list {show:>6} ms ({stale} stale) | eval open {eval_open:>6} ms, reader brief {brief:>6} B"
        );
    });
}

/// A unit evaluated after four more were committed, against the Eval of the
/// whole sequence.
fn unit_and_sequence(files: usize, decisions: usize) {
    with_config_home(|_| {
        let p = product(files, 1, decisions);
        let ws = &p.ws;
        let mut asks = Vec::new();
        for u in 0..5 {
            let ask = confirm_ask(ws, format!("Change unit {u}.\n").as_bytes());
            asks.push(ask["ask_id"].as_str().unwrap().to_string());
            let changed: Vec<(String, Option<&str>)> =
                (0..3).map(|k| (file(100 * u + k), Some("// changed\n"))).collect();
            let borrowed: Vec<(&str, Option<&str>)> =
                changed.iter().map(|(f, b)| (f.as_str(), *b)).collect();
            commit(&ws.root, &borrowed, &format!("unit {u}"));
        }
        let changes = |eval: &str| -> usize {
            let doc: Value = serde_json::from_slice(
                &std::fs::read(ws.rf_dir().join("evals").join(eval).join("changes.json")).unwrap(),
            )
            .unwrap();
            doc["units"].as_array().map(Vec::len).unwrap_or(0)
        };
        let open = crate::evaluate::Open { ask: Some(&asks[0]), ..Default::default() };
        let (out, unit_ms) = ms(|| crate::evaluate::open_eval(ws, open).unwrap());
        let unit = out["eval_id"].as_str().unwrap().to_string();
        let (unit_files, unit_brief) = (changes(&unit), read_brief(ws, &unit));
        crate::evaluate::void_eval(ws, &unit, "measured", None).unwrap();
        let (out, all_ms) = ms(|| crate::evaluate::open_eval(ws, Default::default()).unwrap());
        let all = out["eval_id"].as_str().unwrap().to_string();
        let (all_files, all_brief) = (changes(&all), read_brief(ws, &all));
        eprintln!(
            "files {files:>6} decisions {decisions:>5} | first of five units: {unit_files} files, open {unit_ms} ms, \
             brief {unit_brief} B | the sequence: {all_files} files, open {all_ms} ms, brief {all_brief} B"
        );
    });
}

#[test]
#[ignore = "a measurement, run by hand"]
fn scale_files() {
    for files in [1_000, 10_000, 50_000] {
        measure(files, 1, 1_000);
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn scale_decisions() {
    for decisions in [100, 1_000, 5_000] {
        measure(10_000, 1, decisions);
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn scale_repositories() {
    for repos in [1, 3] {
        measure(30_000, repos, 1_000);
    }
}

#[test]
#[ignore = "a measurement, run by hand"]
fn scale_unit_against_sequence() {
    unit_and_sequence(10_000, 1_000);
}
