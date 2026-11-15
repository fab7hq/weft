//! A sense-xs bundle on a fixture graph (`tests/fixtures/xs/make.py`): its
//! base reads `scale · Σ ids + bias`, which the `state` adapter makes `Σ ids`
//! and the `ask` adapter `0.5 − Σ ids`. Each job's rows read with its own.

use serde_json::json;
use std::path::{Path, PathBuf};
use weft_sense::manifest::bundle_for_tests;
use weft_sense::{Rule, SenseModel, sha256_hex};

/// sense-xs 0.0.1's manifest: the bundle whose tokenizer the fixture borrows.
const SENSE_XS_0_0_1: &str = "33971ef7666281d62f74584b08fdedfd0a10e6d8c1e30947e92276f211d09112";

const SURFACE: [&str; 7] = [
    "idle_ready",
    "working",
    "working_post_hook",
    "waiting_permission",
    "waiting_question",
    "waiting_trust_or_auth",
    "error_or_limit",
];
const CONVERSATION: [&str; 5] =
    ["person_message", "agent_reply", "tool_step_ok", "tool_step_failed", "internal_skip"];
const ROUTES: [&str; 4] = ["direct", "plan", "goal", "review"];

fn questions(classes: &[&str]) -> Vec<String> {
    classes.iter().map(|c| format!("Is this {c}?")).collect()
}

/// The fixture bundle in `dir`, every file under the digest its manifest names.
fn bundle(dir: &Path) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xs");
    let tokenizer = bundle_for_tests(SENSE_XS_0_0_1).unwrap().join("tokenizer.json");
    std::fs::create_dir_all(dir.join("adapters")).unwrap();
    let copy = |from: PathBuf, to: &str| {
        std::fs::copy(from, dir.join(to)).unwrap();
        sha256_hex(&std::fs::read(dir.join(to)).unwrap())
    };
    let model = copy(fixture.join("sense-xs.int8.onnx"), "sense-xs.int8.onnx");
    let tokenizer = copy(tokenizer, "tokenizer.json");
    let state = copy(fixture.join("adapters/state.onnx_adapter"), "adapters/state.onnx_adapter");
    let ask = copy(fixture.join("adapters/ask.onnx_adapter"), "adapters/ask.onnx_adapter");
    let manifest = json!({
        "schema": "fab7.sense.xs/1", "version": "0.0.1",
        "normalizer": "fab7.sense.decision.normalize/2",
        "normalize": {"replacements": {}, "screen_last_lines": 15},
        "adapters": [
            {"name": "state", "file": "adapters/state.onnx_adapter", "sha256": state, "heads": [
                {"name": "surface", "kind": "choice", "classes": SURFACE,
                 "questions": questions(&SURFACE), "temperature": 1000.0},
                {"name": "conversation", "kind": "choice", "classes": CONVERSATION,
                 "questions": questions(&CONVERSATION), "temperature": 1000.0},
                {"name": "turn_end", "kind": "noul", "questions": ["Does it ask?"], "sure": 0.8}]},
            {"name": "ask", "file": "adapters/ask.onnx_adapter", "sha256": ask, "heads": [
                {"name": "ask_task", "kind": "noul", "classes": ["implement"],
                 "questions": ["Implements?"], "thresholds": [0.5]},
                {"name": "ask_effects", "kind": "noul", "classes": ["write"],
                 "questions": ["Writes?"], "thresholds": [0.5]},
                {"name": "ask_route", "kind": "choice", "classes": ROUTES,
                 "questions": questions(&ROUTES), "temperature": 1000.0},
                {"name": "ask_result", "kind": "derived", "classes": ["answer"]},
                {"name": "rule_relevance", "kind": "document", "question": "{document}",
                 "temperature": 1000.0, "sure": 0.6, "memory": {"w": 0.05, "a": 1.0},
                 "convention": []},
                {"name": "plan", "kind": "document", "question": "Continues: {document}",
                 "temperature": 1000.0, "sure": 0.0, "margin": 0.0}]}],
        "files": {"model": {"path": "sense-xs.int8.onnx", "sha256": model},
                  "tokenizer": {"path": "tokenizer.json", "sha256": tokenizer}}
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

/// Each question row's Σ ids: the logit the `state` adapter gives it.
fn sums(model: &SenseModel, text: &str, questions: &[String]) -> Vec<i64> {
    let qs: Vec<&str> = questions.iter().map(String::as_str).collect();
    let rows = model.rows(text, &qs).unwrap();
    rows.iter().map(|r| r.iter().map(|&id| i64::from(id)).sum()).collect()
}

fn class_at<'a>(classes: &[&'a str], sums: &[i64], pick: fn(&i64, &i64) -> bool) -> &'a str {
    let best = (0..sums.len()).reduce(|b, i| if pick(&sums[i], &sums[b]) { i } else { b });
    classes[best.unwrap()]
}

#[test]
fn a_screen_reads_with_the_state_adapter_and_an_ask_with_the_ask_adapter() {
    let dir = tempfile::tempdir().unwrap();
    bundle(dir.path());
    let model = SenseModel::load(dir.path()).unwrap();
    assert_eq!(model.version(), "0.0.1");
    let text = "Fix the login bug.";
    // The state adapter gives Σ ids: the surface picks the largest row.
    let surface = sums(&model, text, &questions(&SURFACE));
    let read = model.read_screen(text, 7).unwrap();
    assert_eq!(read.state.as_str(), class_at(&SURFACE, &surface, |a, b| a > b));
    // The ask adapter gives 0.5 − Σ ids: the route is the smallest row.
    let routes = sums(&model, text, &questions(&ROUTES));
    let rules = [Rule { id: "r".into(), label: "R".into(), when: "w".into(), counts: None }];
    let answer = model.ask(text, &[], &rules).unwrap();
    let route = answer.route.pick.unwrap();
    assert_eq!(route, class_at(&ROUTES, &routes, |a, b| a < b));
    assert_eq!(answer.unticked[0].id, "r", "under its cut at 0.5 − Σ ids");
    // The plan's rows read with the ask adapter too: the smallest wins.
    let candidates: Vec<(String, String)> =
        ["site: Site: the site.", "weft: Weft: the app and its many, many parts."]
            .map(|c| (c[..4].to_string(), c.to_string()))
            .into();
    let plan_q: Vec<String> = candidates.iter().map(|(_, c)| format!("Continues: {c}")).collect();
    let plan_sums = sums(&model, text, &plan_q);
    let want = if plan_sums[0] < plan_sums[1] { "site" } else { "weft" };
    assert_eq!(model.plan(text, &candidates).unwrap().as_deref(), Some(want));
}

#[test]
fn an_adapter_not_the_one_its_manifest_names_is_refused_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    bundle(dir.path());
    std::fs::write(dir.path().join("adapters/ask.onnx_adapter"), b"not it").unwrap();
    let err = SenseModel::load(dir.path()).err().unwrap().to_string();
    assert!(err.contains("ask"), "{err}");
}

#[test]
fn an_ask_after_its_heads_alone_reads_only_its_rule_rows_and_answers_the_same() {
    let dir = tempfile::tempdir().unwrap();
    bundle(dir.path());
    let model = SenseModel::load(dir.path()).unwrap();
    let rules = [
        Rule { id: "a".into(), label: "A".into(), when: "w".into(), counts: None },
        Rule { id: "b".into(), label: "B".into(), when: "x".into(), counts: None },
    ];
    let whole = model.ask("Fix the login bug.", &[], &rules).unwrap();
    let heads = model.ask("Add a test.", &[], &[]).unwrap();
    assert!(heads.rules.is_empty() && heads.unticked.is_empty(), "the heads alone");
    let before = model.rows_read();
    let full = model.ask("Add a test.", &[], &rules).unwrap();
    assert_eq!(model.rows_read() - before, 2, "one row per rule, no head row again");
    assert_eq!((full.route, full.task, full.effects), (heads.route, heads.task, heads.effects));
    let before = model.rows_read();
    assert_eq!(
        model.ask("Fix the login bug.", &[], &rules).unwrap(),
        whole,
        "other words: read again"
    );
    assert_eq!(model.rows_read() - before, 1 + 1 + 4 + 2, "every head row and every rule row");
}
