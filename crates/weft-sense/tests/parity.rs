use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use weft_sense::model::calibrated_argmax;
use weft_sense::{ConversationEntry, SenseModel, SurfaceState};

#[derive(Debug, Deserialize)]
struct GoldenVector {
    head: String,
    text: String,
    input_ids: Vec<u32>,
    expected_class_index: usize,
    expected_class: String,
    confidence_bp: u16,
    surface_logits: [f32; 8],
    conversation_logits: [f32; 5],
}

fn test_bundle_dir() -> PathBuf {
    let workspace_bundle =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../sense/dist/fab7-sense-v1");
    if workspace_bundle.exists() {
        return workspace_bundle;
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".fab7/sense/fab7-sense-v1")
}

#[test]
fn golden_vectors_parity_and_p99_latency_under_5ms() {
    let dir = test_bundle_dir();
    let model = SenseModel::load(&dir).expect("SenseModel::load should succeed");
    assert_eq!(model.version(), "0.1.0");

    let golden_raw =
        fs::read_to_string(dir.join("golden_vectors.jsonl")).expect("read golden_vectors.jsonl");
    let vectors: Vec<GoldenVector> = golden_raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse golden vector"))
        .collect();
    assert!(vectors.len() >= 13);

    let mut surface_count = 0;
    let mut conv_count = 0;

    for (idx, vec) in vectors.iter().enumerate() {
        let (s_logits, c_logits, ids) = model.infer_raw_text(&vec.text);
        assert_eq!(ids, vec.input_ids, "golden vector {idx} token IDs must match 100%");

        for (a, b) in s_logits.iter().zip(vec.surface_logits.iter()) {
            assert!(
                (a - b).abs() < 0.06,
                "golden vector {idx} surface logit diff too large: {a} vs {b}"
            );
        }
        for (a, b) in c_logits.iter().zip(vec.conversation_logits.iter()) {
            assert!(
                (a - b).abs() < 0.06,
                "golden vector {idx} conv logit diff too large: {a} vs {b}"
            );
        }

        if vec.head == "surface" {
            surface_count += 1;
            let (pred_idx, conf_bp) = calibrated_argmax(&s_logits, model.temperature());
            assert_eq!(
                pred_idx, vec.expected_class_index,
                "golden vector {idx} surface class index mismatch"
            );
            assert_eq!(SurfaceState::from_index(pred_idx).unwrap().as_str(), vec.expected_class);
            assert!(
                (conf_bp as i32 - vec.confidence_bp as i32).abs() <= 60,
                "golden vector {idx} surface confidence_bp mismatch: {conf_bp} vs {}",
                vec.confidence_bp
            );
        } else if vec.head == "conversation" {
            conv_count += 1;
            let (pred_idx, conf_bp) =
                calibrated_argmax(&c_logits, model.conversation_temperature());
            assert_eq!(
                pred_idx, vec.expected_class_index,
                "golden vector {idx} conversation class index mismatch"
            );
            assert_eq!(
                ConversationEntry::from_index(pred_idx).unwrap().as_str(),
                vec.expected_class
            );
            assert!(
                (conf_bp as i32 - vec.confidence_bp as i32).abs() <= 60,
                "golden vector {idx} conversation confidence_bp mismatch: {conf_bp} vs {}",
                vec.confidence_bp
            );
        }
    }
    assert_eq!(surface_count, 8);
    assert_eq!(conv_count, 5);

    // Measure CPU latency across 104 inference runs (8 passes over all 13 vectors)
    let mut latencies_ms = Vec::with_capacity(vectors.len() * 8);
    for _ in 0..8 {
        for vec in &vectors {
            let t0 = Instant::now();
            let _ = model.infer_raw_text(&vec.text);
            latencies_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50_ms = latencies_ms[latencies_ms.len() / 2];
    let p99_idx = ((latencies_ms.len() as f64) * 0.99).floor() as usize;
    let p99_ms = latencies_ms[p99_idx.min(latencies_ms.len() - 1)];
    eprintln!("weft-sense CPU latency: p50={p50_ms:.2}ms, p99={p99_ms:.2}ms");
    assert!(p99_ms < 5.0, "expected p99 CPU latency < 5.0 ms, got {p99_ms:.2} ms");
}
