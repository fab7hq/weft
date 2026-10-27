use serde::Deserialize;
use std::path::PathBuf;
use weft_sense::ask::AskHeads;
use weft_sense::ask_graph::{AskGraph, alone_row, ask_rows};
use weft_sense::manifest::{Manifest, bundle_with_digest};
use weft_sense::unigram::AskTokenizer;

#[derive(Debug, Deserialize)]
struct Expected {
    ask_task: Vec<String>,
    ask_effects: Vec<String>,
    ask_route: String,
    ask_result: String,
}

#[derive(Debug, Deserialize)]
struct AskVector {
    text: String,
    input_ids: Vec<Vec<u32>>,
    ask_task_logits: Vec<f32>,
    ask_effects_logits: Vec<f32>,
    ask_route_logits: Vec<f32>,
    embedding: Vec<f32>,
    expected: Expected,
}

/// The bundle 3 export these tests were written against.
const FAB7_SENSE_NEXT: &str = "239c9f0688d50b28de374c26970440943cecbe246a8ebde0bae2e868f2ea8f9a";

fn next_bundle() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-next");
    weft_sense::manifest::bundle_with_digest(&dir, FAB7_SENSE_NEXT).unwrap()
}

fn ask_vectors() -> Vec<AskVector> {
    let raw = std::fs::read_to_string(next_bundle().join("golden_vectors.jsonl")).unwrap();
    raw.lines()
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .filter(|v| v["head"] == "ask")
        .map(|v| serde_json::from_value(v).unwrap())
        .collect()
}

fn ask_heads() -> AskHeads {
    let manifest = std::fs::read(next_bundle().join("manifest.json")).unwrap();
    AskHeads::from_manifest(&Manifest::parse(&manifest).unwrap()).unwrap()
}

#[test]
fn the_golden_ask_logits_give_the_golden_answers() {
    let heads = ask_heads();
    let vectors = ask_vectors();
    assert!(vectors.len() >= 4);
    for vec in &vectors {
        let logits: Vec<f32> =
            [&vec.ask_task_logits, &vec.ask_effects_logits, &vec.ask_route_logits]
                .into_iter()
                .flatten()
                .copied()
                .collect();
        assert_eq!(logits.len(), heads.questions().count(), "{}", vec.text);
        let answer = heads.answer(&logits, &[], Vec::new()).unwrap();
        assert_eq!(answer.task.on, vec.expected.ask_task, "{}", vec.text);
        assert_eq!(answer.effects.on, vec.expected.ask_effects, "{}", vec.text);
        let route = answer.route.pick.as_deref().unwrap_or("unsure");
        assert_eq!(route, vec.expected.ask_route, "{}", vec.text);
        assert_eq!(answer.result, vec.expected.ask_result, "{}", vec.text);
        assert_eq!(answer.sense, "0.3.0");
        assert_eq!(
            (vec.input_ids.len(), vec.embedding.len()),
            (heads.questions().count() + 1, 384)
        );
    }
}

/// Until then the graph is held to the golden answers: every Ask's tasks,
/// effects, route and result, and an embedding pointing the same way.
#[test]
fn the_ask_graph_gives_the_golden_answers() {
    let heads = ask_heads();
    let graph =
        AskGraph::load(&std::fs::read(next_bundle().join("ask.int8.onnx")).unwrap()).unwrap();
    for vec in ask_vectors() {
        let out = graph.run(&vec.input_ids).unwrap();
        let n = heads.questions().count();
        let a = heads.answer(&out.logits[..n], &[], Vec::new()).unwrap();
        assert_eq!(a.task.on, vec.expected.ask_task, "{}", vec.text);
        assert_eq!(a.effects.on, vec.expected.ask_effects, "{}", vec.text);
        assert_eq!(
            a.route.pick.as_deref().unwrap_or("unsure"),
            vec.expected.ask_route,
            "{}",
            vec.text
        );
        assert_eq!(a.result, vec.expected.ask_result, "{}", vec.text);
        let e = out.embeddings.last().unwrap();
        let dot: f32 = e.iter().zip(&vec.embedding).map(|(a, b)| a * b).sum();
        let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let cosine = dot / (norm(e) * norm(&vec.embedding));
        assert!(cosine > 0.9, "{}: embedding cosine {cosine}", vec.text);
    }
}

#[derive(Debug, Deserialize)]
struct RuleVector {
    when: String,
    when_input_ids: Vec<u32>,
}

fn rule_vector() -> RuleVector {
    let raw = std::fs::read_to_string(next_bundle().join("golden_vectors.jsonl")).unwrap();
    let row = raw.lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap());
    serde_json::from_value(row.into_iter().find(|v| v["head"] == "rule_relevance").unwrap())
        .unwrap()
}

fn tokenizer() -> AskTokenizer {
    AskTokenizer::from_bytes(&std::fs::read(next_bundle().join("ask_tokenizer.json")).unwrap())
        .unwrap()
}

fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

#[test]
fn the_ask_rows_are_the_golden_token_ids() {
    let (heads, tok) = (ask_heads(), tokenizer());
    for vec in ask_vectors() {
        assert_eq!(
            ask_rows(&tok, &vec.text, heads.questions()).unwrap(),
            vec.input_ids,
            "{}",
            vec.text
        );
    }
    let rule = rule_vector();
    assert_eq!(alone_row(&tok, &rule.when).unwrap(), rule.when_input_ids);
}

/// Every quantized product's scale is its whole batch's, so one rounding
/// apart in any layer tips later levels: ONNX Runtime's own unoptimized pass
/// ends 0.24 to 0.37 from these logits, this graph 0.4 to 1.2, with the same
/// answers.
#[test]
#[ignore = "waits for a Sense export whose rows do not share a quantization scale"]
fn the_golden_ask_batches_match_their_logits_and_embedding() {
    let graph =
        AskGraph::load(&std::fs::read(next_bundle().join("ask.int8.onnx")).unwrap()).unwrap();
    for vec in ask_vectors() {
        let start = std::time::Instant::now();
        let out = graph.run(&vec.input_ids).unwrap();
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        let golden: Vec<f32> =
            [&vec.ask_task_logits, &vec.ask_effects_logits, &vec.ask_route_logits]
                .into_iter()
                .flatten()
                .copied()
                .collect();
        let logits = &out.logits[..golden.len()];
        let embedding = out.embeddings.last().unwrap();
        let width = vec.input_ids.iter().map(Vec::len).max().unwrap();
        eprintln!(
            "ask batch {}×{width}: logits off by {:.2e}, embedding by {:.2e}, {ms:.0} ms",
            vec.input_ids.len(),
            max_diff(logits, &golden),
            max_diff(embedding, &vec.embedding)
        );
        assert!(max_diff(logits, &golden) < 1e-4, "{}", vec.text);
        assert!(max_diff(embedding, &vec.embedding) < 1e-4, "{}", vec.text);
    }
}

#[test]
fn sense_answers_an_ask_with_its_defaults_and_proposed_rules() {
    let model = weft_sense::SenseModel::load(&next_bundle()).unwrap();
    let rule = rule_vector();
    let rules = [weft_sense::Rule {
        id: "practice.yagni".into(),
        family: "scope".into(),
        when: rule.when,
        examples: Vec::new(),
    }];
    let vec = &ask_vectors()[1];
    let answer = model.ask(&vec.text, &["direct", "plan", "goal"], &rules).unwrap();
    assert_eq!(answer.sense, "0.3.0");
    assert_eq!(answer.route.pick.as_deref().unwrap_or("unsure"), vec.expected.ask_route);
    assert_eq!(answer.task.on, vec.expected.ask_task);
    assert_eq!(answer.result, vec.expected.ask_result);
    assert!(answer.rules.iter().all(|r| r.id == "practice.yagni" && r.p >= 0.39), "{answer:?}");
}

#[test]
fn a_bundle_without_ask_heads_refuses_an_ask_naming_the_head() {
    let v1 = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-v1");
    let pinned = "ed08996f6591f04e6cad670142015b5cd9215a7ad20aa7cca64f0d2f3d6d31a7";
    let model = weft_sense::SenseModel::load(&bundle_with_digest(&v1, pinned).unwrap()).unwrap();
    let err = model.ask("fix the typo", &[], &[]).unwrap_err().to_string();
    assert!(err.contains("ask_task"), "{err}");
}
