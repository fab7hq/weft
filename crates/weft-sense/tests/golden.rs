//! sense-d 0.1.0's golden vectors (sense-d spec/bundle.md §5), read by
//! Weft's own tokenizer, normalizer and heads, on ONNX Runtime's graph.

use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;
use weft_sense::heads::choice;
use weft_sense::manifest::{Manifest, bundle_for_tests};
use weft_sense::normalize::{Replacements, entry_text, screen_text};
use weft_sense::{PaneSnapshot, Rule, SenseModel, TranscriptSnippet};

/// sense-d 0.1.0's manifest, as published on Hugging Face `fab7hq/sense-d`.
const SENSE_D_0_1_0: &str = "b611d83967761d4fc45890d47122ea428030f62d2bb9df758da90998d2fae286";
const TOLERANCE: f32 = 1e-4;
const ASK_HEADS: [&str; 3] = ["ask_task", "ask_effects", "ask_route"];

struct Bundle {
    dir: PathBuf,
    manifest: Manifest,
    raw_manifest: Value,
    rows: Vec<Value>,
}

fn bundle() -> Bundle {
    let dir = bundle_for_tests(SENSE_D_0_1_0).unwrap();
    let bytes = std::fs::read(dir.join("manifest.json")).unwrap();
    let raw = std::fs::read_to_string(dir.join("golden_vectors.jsonl")).unwrap();
    Bundle {
        manifest: Manifest::parse(&bytes).unwrap(),
        raw_manifest: serde_json::from_slice(&bytes).unwrap(),
        rows: raw.lines().map(|l| serde_json::from_str(l).unwrap()).collect(),
        dir,
    }
}

impl Bundle {
    fn of(&self, head: &str) -> Vec<&Value> {
        let rows: Vec<&Value> = self.rows.iter().filter(|r| r["head"] == head).collect();
        assert!(!rows.is_empty(), "no golden {head} rows");
        rows
    }

    fn questions(&self, head: &str) -> Vec<String> {
        self.manifest.head(head).unwrap().questions.clone()
    }

    /// A golden row's text, its questions and its logits.
    fn call(&self, row: &Value) -> (String, Vec<String>, Vec<f32>) {
        let text = |key: &str| row[key].as_str().unwrap().to_string();
        match row["head"].as_str().unwrap() {
            "ask" => {
                let qs = ASK_HEADS.iter().flat_map(|h| self.questions(h)).collect();
                let logits = ASK_HEADS.iter().flat_map(|h| floats(&row["logits"][h])).collect();
                (text("text"), qs, logits)
            }
            "rule_relevance" => {
                let rules = row["rules"].as_object().unwrap();
                let qs = rules.values().map(|w| w.as_str().unwrap().to_string()).collect();
                let logits = rules.keys().map(|id| row["logits"][id].as_f64().unwrap() as f32);
                (text("text"), qs, logits.collect())
            }
            "recall_net" => {
                let heads = self.raw_manifest["heads"].as_array().unwrap();
                let recall = heads.iter().find(|h| h["name"] == "recall_net").unwrap();
                let template = recall["question_template"].as_str().unwrap();
                let q = template.replace("{requirement}", row["requirement"].as_str().unwrap());
                (text("window"), vec![q], vec![row["logit"].as_f64().unwrap() as f32])
            }
            head => (text("text"), self.questions(head), floats(&row["logits"])),
        }
    }
}

fn floats(v: &Value) -> Vec<f32> {
    serde_json::from_value(v.clone()).unwrap()
}

fn strings(v: &Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap()
}

fn assert_close(got: &[f32], want: &[f32], what: &str) -> f32 {
    assert_eq!(got.len(), want.len(), "{what}");
    let worst = got.iter().zip(want).map(|(g, w)| (g - w).abs()).fold(0.0, f32::max);
    assert!(worst <= TOLERANCE, "{what}: off the golden logits by {worst}");
    worst
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

#[test]
fn every_golden_row_holds_to_1e_4_alone_as_its_call_and_in_one_batch_of_all() {
    let b = bundle();
    let model = SenseModel::load(&b.dir).unwrap();
    let (mut every, mut want, mut worst) = (Vec::new(), Vec::new(), 0.0_f32);
    for row in b.rows.iter().filter(|r| r["head"] != "normalize") {
        let (text, questions, logits) = b.call(row);
        let rows = model.rows(&text, &refs(&questions)).unwrap();
        let expected: Vec<Vec<u32>> = serde_json::from_value(row["input_ids"].clone()).unwrap();
        assert_eq!(rows, expected, "{} rows of {text:?}", row["head"]);
        let alone: Vec<f32> =
            rows.iter().flat_map(|r| model.logits(std::slice::from_ref(r)).unwrap()).collect();
        worst = worst.max(assert_close(&alone, &logits, &format!("{} alone", row["head"])));
        let call = model.logits(&rows).unwrap();
        worst = worst.max(assert_close(&call, &logits, &format!("{} as one call", row["head"])));
        every.extend(rows);
        want.extend(logits);
    }
    let batched = model.logits(&every).unwrap();
    worst = worst.max(assert_close(&batched, &want, "every golden row in one batch"));
    println!("{} golden rows; the most any is off: {worst:.2e}", every.len());
}

#[test]
fn each_golden_screen_and_entry_reads_as_its_class() {
    let b = bundle();
    let model = SenseModel::load(&b.dir).unwrap();
    for head in ["surface", "conversation"] {
        for row in b.of(head) {
            let (text, questions, _) = b.call(row);
            let logits = model.logits(&model.rows(&text, &refs(&questions)).unwrap()).unwrap();
            let read = choice(b.manifest.head(head).unwrap(), &logits, &[]);
            let pick = read.pick.unwrap_or("unsure".into());
            assert_eq!(pick, row["expected"].as_str().unwrap(), "{head} of {text:?}");
        }
    }
}

#[test]
fn each_golden_ask_gives_its_task_effects_route_result_and_rules() {
    let b = bundle();
    let model = SenseModel::load(&b.dir).unwrap();
    for row in b.of("ask") {
        let a = model.ask(row["text"].as_str().unwrap(), &[], &[]).unwrap();
        let want = &row["expected"];
        assert_eq!(a.task.on, strings(&want["ask_task"]), "{}", row["text"]);
        assert_eq!(a.effects.on, strings(&want["ask_effects"]), "{}", row["text"]);
        assert_eq!(a.route.pick.as_deref().unwrap_or("unsure"), want["ask_route"]);
        assert_eq!(a.result, want["ask_result"].as_str().unwrap());
    }
    for row in b.of("rule_relevance") {
        let rules: Vec<Rule> = row["rules"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, when)| Rule { id: id.clone(), when: when.as_str().unwrap().into() })
            .collect();
        let a = model.ask(row["text"].as_str().unwrap(), &[], &rules).unwrap();
        assert_eq!(a.task.on, strings(&row["task"]));
        assert_eq!(a.effects.on, strings(&row["effects"]));
        let ids: Vec<&str> = a.rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, refs(&strings(&row["expected"])));
    }
}

#[test]
fn the_golden_normalizing_vectors_match() {
    let b = bundle();
    let table = Replacements::new(&b.manifest.normalize.replacements).unwrap();
    for row in b.of("normalize") {
        let raw = &row["raw"];
        let text = match row["kind"].as_str().unwrap() {
            "screen" => {
                let lines = strings(&raw["screen"]["bottom_lines"]);
                let (pty, cursor) = (&raw["pty"], &raw["pty"]["cursor"]);
                let u16_of = |v: &Value| v.as_u64().unwrap() as u16;
                let snap = PaneSnapshot {
                    harness: raw["harness"].as_str().unwrap(),
                    hook_state: raw["hook"]["hook_state"].as_str(),
                    last_hook: raw["hook"]["last_hook"].as_str(),
                    alt_screen: pty["alt_screen"].as_bool().unwrap(),
                    rows: u16_of(&raw["dims"]["rows"]),
                    cols: u16_of(&raw["dims"]["cols"]),
                    cursor_row: u16_of(&cursor["row"]),
                    cursor_col: u16_of(&cursor["col"]),
                    ms_since_last_output: pty["ms_since_last_output"].as_u64().unwrap(),
                    bottom_lines: &lines,
                    seq: 0,
                };
                screen_text(&snap, b.manifest.normalize.screen_last_lines, &table)
            }
            _ => entry_text(
                &TranscriptSnippet {
                    harness: raw["harness"].as_str().unwrap(),
                    entry_type: raw["entry_type"].as_str().unwrap(),
                    snippet: raw["entry_snippet"].as_str().unwrap(),
                },
                &table,
            ),
        };
        assert_eq!(text, row["normalized"].as_str().unwrap(), "{} normalized", row["kind"]);
    }
}

/// Not a check: the CPU time of a screen, an entry and an Ask, printed for
/// the plan's record. Run with `--ignored --nocapture` on a quiet machine.
#[test]
#[ignore]
fn p50_per_screen_entry_and_ask() {
    let b = bundle();
    let model = SenseModel::load(&b.dir).unwrap();
    for head in ["surface", "conversation", "ask"] {
        let calls: Vec<Vec<Vec<u32>>> = b
            .of(head)
            .iter()
            .map(|r| {
                let (text, questions, _) = b.call(r);
                model.rows(&text, &refs(&questions)).unwrap()
            })
            .collect();
        let mut times = Vec::new();
        for _ in 0..10 {
            for rows in &calls {
                let start = Instant::now();
                model.logits(rows).unwrap();
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
        }
        times.sort_by(f64::total_cmp);
        println!("{head}: p50 {:.1} ms over {} calls", times[times.len() / 2], times.len());
    }
}
