//! sense-xs's golden vectors (sense-xs spec §6), read by Weft's own
//! tokenizer, normalizer and heads on ONNX Runtime's graph, each row with
//! the adapter its job names.

use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use weft_sense::heads::{choice, turn_end};
use weft_sense::manifest::Manifest;
use weft_sense::normalize::{Replacements, candidate_text, entry_text, reply_text, screen_text};
use weft_sense::{Counts, PaneSnapshot, Rule, SenseModel, TranscriptSnippet};

const TOLERANCE: f32 = 1e-4;
const ASK_HEADS: [&str; 3] = ["ask_task", "ask_effects", "ask_route"];

struct Bundle {
    manifest: Manifest,
    table: Replacements,
    model: SenseModel,
    rows: Vec<Value>,
}

/// The bundle in `SENSE_XS_BUNDLE`'s folder.
fn bundle() -> Bundle {
    let dir = PathBuf::from(std::env::var_os("SENSE_XS_BUNDLE").expect("SENSE_XS_BUNDLE"));
    let manifest = Manifest::parse(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let raw = std::fs::read_to_string(dir.join("golden_vectors.jsonl")).unwrap();
    Bundle {
        table: Replacements::new(&manifest.normalize.replacements).unwrap(),
        model: SenseModel::load(&dir).unwrap(),
        rows: raw.lines().map(|l| serde_json::from_str(l).unwrap()).collect(),
        manifest,
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

    /// A golden row's question rows, as Weft builds them, and its logits.
    fn call(&self, row: &Value) -> (Vec<Vec<u32>>, Vec<f32>) {
        let text = row["text"].as_str().unwrap();
        let (text, questions) = match row["head"].as_str().unwrap() {
            "ask_heads" => {
                let qs = ASK_HEADS.iter().flat_map(|h| self.questions(h)).collect();
                (self.model.ask_text(text), qs)
            }
            head @ ("rule_relevance" | "plan") => {
                let template = self.manifest.head(head).unwrap().question.clone().unwrap();
                let documents = strings(&row["documents"]);
                let qs = documents
                    .iter()
                    .map(|d| template.replace("{document}", &candidate_text(d, &self.table)));
                (self.model.ask_text(text), qs.collect())
            }
            head => (text.to_string(), self.questions(head)),
        };
        let qs: Vec<&str> = questions.iter().map(String::as_str).collect();
        let rows = match self.manifest.head(row["head"].as_str().unwrap()).ok().and_then(|h| h.cut)
        {
            Some(cut) => self.model.cut_rows(&text, cut.first, &qs).unwrap(),
            None => self.model.rows(&text, &qs).unwrap(),
        };
        let logits = match &row["logits"] {
            Value::Null => vec![row["logit"].as_f64().unwrap() as f32],
            v => floats(v),
        };
        (rows, logits)
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
    assert!(worst <= TOLERANCE, "{what}: off the golden by {worst}");
    worst
}

/// A golden document `<label>: <when>` as the rule it stands for.
fn rule(id: usize, document: &str, counts: &Value) -> Rule {
    let (label, when) = document.split_once(": ").unwrap();
    let counts = counts.as_array().map(|c| Counts {
        kept: c[0].as_u64().unwrap() as u32,
        candidate: c[1].as_u64().unwrap() as u32,
    });
    Rule { id: format!("r{id}"), label: label.into(), when: when.into(), counts }
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn every_golden_row_holds_to_1e_4_with_its_adapter_alone_and_in_one_batch_per_adapter() {
    let b = bundle();
    let mut batches: BTreeMap<String, (Vec<Vec<u32>>, Vec<f32>)> = BTreeMap::new();
    let mut worst = 0.0_f32;
    for row in b.rows.iter().filter(|r| r["head"] != "normalize") {
        let (head, adapter) = (row["head"].as_str().unwrap(), row["adapter"].as_str().unwrap());
        let (rows, logits) = b.call(row);
        let expected: Vec<Vec<u32>> = serde_json::from_value(row["input_ids"].clone()).unwrap();
        assert_eq!(rows, expected, "{head} rows of {}", row["text"]);
        let alone: Vec<f32> = rows
            .iter()
            .flat_map(|r| b.model.logits(std::slice::from_ref(r), Some(adapter)).unwrap())
            .collect();
        worst = worst.max(assert_close(&alone, &logits, &format!("{head} alone")));
        let call = b.model.logits(&rows, Some(adapter)).unwrap();
        worst = worst.max(assert_close(&call, &logits, &format!("{head} as one call")));
        let batch = batches.entry(adapter.to_string()).or_default();
        batch.0.extend(rows);
        batch.1.extend(logits);
    }
    assert_eq!(batches.keys().collect::<Vec<_>>(), ["ask", "state"]);
    for (adapter, (rows, want)) in &batches {
        let got = b.model.logits(rows, Some(adapter)).unwrap();
        worst = worst.max(assert_close(&got, want, &format!("every {adapter} row in one batch")));
    }
    println!("the most any golden row is off: {worst:.2e}");
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn each_golden_screen_entry_and_reply_reads_as_its_class() {
    let b = bundle();
    for head in ["surface", "conversation"] {
        for row in b.of(head) {
            let (rows, _) = b.call(row);
            let logits = b.model.logits(&rows, Some("state")).unwrap();
            let read = choice(b.manifest.head(head).unwrap(), &logits, &[]);
            assert_eq!(read.pick.as_deref().unwrap_or("unsure"), row["expected"], "{head}");
        }
    }
    for row in b.of("turn_end") {
        let asks = b.model.turn_end(row["text"].as_str().unwrap()).unwrap().asks;
        assert_eq!(asks, row["expected"]["asks"].as_bool().unwrap(), "{}", row["text"]);
        let (rows, _) = b.call(row);
        let logit = b.model.logits(&rows, Some("state")).unwrap()[0];
        assert_eq!(turn_end(b.manifest.head("turn_end").unwrap(), logit).asks, asks);
    }
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn each_golden_ask_gives_its_task_effects_route_and_result() {
    let b = bundle();
    for row in b.of("ask_heads") {
        let a = b.model.ask(row["text"].as_str().unwrap(), &[], &[]).unwrap();
        let want = &row["expected"];
        assert_eq!(a.task.on, strings(&want["ask_task"]), "{}", row["text"]);
        assert_eq!(a.effects.on, strings(&want["ask_effects"]), "{}", row["text"]);
        assert_eq!(a.route.pick.as_deref().unwrap_or("unsure"), want["ask_route"]);
        assert_eq!(a.result, want["ask_result"].as_str().unwrap());
    }
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn each_golden_rule_scores_as_sense_does_with_and_without_its_counts_and_the_sure_are_ticked() {
    let b = bundle();
    for row in b.of("rule_relevance") {
        let text = row["text"].as_str().unwrap();
        let documents = strings(&row["documents"]);
        for (counts, p) in [(&row["counts"], &row["p_counts"]), (&Value::Null, &row["p"])] {
            let rules: Vec<Rule> =
                documents.iter().enumerate().map(|(i, d)| rule(i, d, &counts[i])).collect();
            let a = b.model.ask(text, &[], &rules).unwrap();
            let scored: BTreeMap<&str, f32> = (a.rules.iter().chain(&a.unticked))
                .filter_map(|r| Some((r.id.as_str(), r.p?)))
                .collect();
            let got: Vec<f32> = rules.iter().map(|r| scored[r.id.as_str()]).collect();
            assert_close(&got, &floats(p), &format!("rule p with counts {counts}"));
            if counts.is_null() {
                continue;
            }
            let ticked: Vec<&str> = (a.rules.iter().filter(|r| r.p.is_some()))
                .map(|r| documents[r.id[1..].parse::<usize>().unwrap()].as_str())
                .collect();
            assert_eq!(
                ticked,
                row["ticked"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|t| t.as_str().unwrap())
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn each_golden_plan_pick_gives_its_plan() {
    let b = bundle();
    for row in b.of("plan") {
        let documents = strings(&row["documents"]);
        let candidates: Vec<(String, String)> = documents
            .iter()
            .map(|d| (d.split_once(": ").unwrap().0.to_string(), d.clone()))
            .collect();
        let pick = b.model.plan(row["text"].as_str().unwrap(), &candidates).unwrap();
        let want = row["pick"].as_u64().map(|i| candidates[i as usize].0.clone());
        assert_eq!(pick, want, "plan of {}", row["text"]);
    }
}

#[test]
#[ignore = "needs SENSE_XS_BUNDLE until a Weft release pins sense-xs"]
fn the_golden_normalizing_vectors_match() {
    let b = bundle();
    for row in b.of("normalize") {
        let raw = &row["raw"];
        let text = match row["kind"].as_str().unwrap() {
            "screen" => {
                let lines = strings(&raw["screen"]["bottom_lines"]);
                let (pty, cursor) = (&raw["pty"], &raw["pty"]["cursor"]);
                let u16_of = |v: &Value| v.as_u64().unwrap() as u16;
                let snap = PaneSnapshot {
                    harness: raw["harness"].as_str().unwrap(),
                    alt_screen: pty["alt_screen"].as_bool().unwrap(),
                    rows: u16_of(&raw["dims"]["rows"]),
                    cols: u16_of(&raw["dims"]["cols"]),
                    cursor_row: u16_of(&cursor["row"]),
                    cursor_col: u16_of(&cursor["col"]),
                    ms_since_last_output: pty["ms_since_last_output"].as_u64().unwrap(),
                    bottom_lines: &lines,
                    seq: 0,
                };
                screen_text(&snap, b.manifest.normalize.screen_last_lines, &b.table)
            }
            "reply" => reply_text(raw.as_str().unwrap(), &b.table),
            "entry" => entry_text(
                &TranscriptSnippet {
                    harness: raw["harness"].as_str().unwrap(),
                    entry_type: raw["entry_type"].as_str().unwrap(),
                    snippet: raw["entry_snippet"].as_str().unwrap(),
                },
                &b.table,
            ),
            "ask" => b.model.ask_text(raw.as_str().unwrap()),
            "chosen" => b.model.ask_text(&b.model.chosen_text(
                raw["words"].as_str().unwrap(),
                raw["route"].as_str(),
                raw["plan"].as_str(),
            )),
            "rule" | "plan" => candidate_text(raw.as_str().unwrap(), &b.table),
            kind => panic!("a golden {kind} row Weft does not read"),
        };
        assert_eq!(text, row["normalized"].as_str().unwrap(), "{} normalized", row["kind"]);
    }
}
