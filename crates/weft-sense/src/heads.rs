//! How a head reads its rows' logits, by its kind (sense-d spec/bundle.md
//! §3): a `choice` head by a softmax across its options, a `noul` head by
//! each class's own sigmoid and threshold.

use crate::manifest::Head;
use std::collections::BTreeMap;

/// A `choice` head's answer: its pick, none when unsure, and every option's
/// probability.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub pick: Option<String>,
    pub top_p: f32,
    pub p: BTreeMap<String, f32>,
}

/// A `choice` head's probabilities, and its pick among `allowed` (any, when
/// empty): their top, renormalized among them, unless under `unsure_below`.
pub fn choice(head: &Head, logits: &[f32], allowed: &[&str]) -> Choice {
    let p = softmax(logits, head.temperature);
    let open = |c: &str| allowed.is_empty() || allowed.contains(&c);
    let candidates = head.classes.iter().zip(&p).filter(|(c, _)| open(c));
    let total: f32 = candidates.clone().map(|(_, p)| p).sum();
    let best = candidates.max_by(|a, b| a.1.total_cmp(b.1));
    let top_p = best.map_or(0.0, |(_, p)| if total > 0.0 { p / total } else { 0.0 });
    let sure = top_p >= head.unsure_below.unwrap_or(0.0);
    let pick = best.filter(|_| sure).map(|(c, _)| c.clone());
    Choice { pick, top_p, p: head.classes.iter().cloned().zip(p).collect() }
}

/// A `noul` head's classes on at their own thresholds, and every class's
/// probability.
pub fn noul(head: &Head, logits: &[f32]) -> (Vec<String>, BTreeMap<String, f32>) {
    let p: Vec<f32> = logits.iter().map(|l| sigmoid(l / head.temperature)).collect();
    let classes = head.classes.iter().zip(&p).zip(&head.thresholds);
    let on = classes.filter(|((_, p), t)| p >= t).map(|((c, _), _)| c.clone()).collect();
    (on, head.classes.iter().cloned().zip(p).collect())
}

pub fn sigmoid(z: f32) -> f32 {
    1.0 / (1.0 + (-z).exp())
}

fn softmax(logits: &[f32], temperature: f32) -> Vec<f32> {
    let scaled: Vec<f32> = logits.iter().map(|l| l / temperature).collect();
    let max = scaled.iter().copied().fold(f32::MIN, f32::max);
    let exp: Vec<f32> = scaled.iter().map(|s| (s - max).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::HeadKind;

    fn head(kind: HeadKind, temperature: f32) -> Head {
        let kind = if kind == HeadKind::Noul { "noul" } else { "choice" };
        let json = serde_json::json!({"name": "h", "kind": kind, "classes": ["a", "b", "c"],
            "questions": ["qa", "qb", "qc"], "temperature": temperature,
            "thresholds": [0.5, 0.5, 0.9], "unsure_below": 0.6});
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_choice_picks_its_top_option_or_none_under_unsure_below() {
        let h = head(HeadKind::Choice, 1.0);
        let c = choice(&h, &[0.0, 3.0, 0.0], &[]);
        assert_eq!(c.pick.as_deref(), Some("b"));
        assert!((c.p.values().sum::<f32>() - 1.0).abs() < 1e-5);
        assert_eq!(choice(&h, &[0.0, 0.5, 0.0], &[]).pick, None, "b is 0.45, under 0.6");
    }

    #[test]
    fn a_choice_among_allowed_options_renormalizes_and_keeps_every_probability() {
        let h = head(HeadKind::Choice, 1.0);
        let c = choice(&h, &[0.0, 3.0, 2.0], &["a", "c"]);
        assert_eq!(c.pick.as_deref(), Some("c"), "c is 0.88 of what a and c hold");
        assert!(c.p["b"] > 0.6, "b's probability stays the head's");
    }

    #[test]
    fn a_noul_class_is_on_at_or_above_its_own_threshold_after_temperature() {
        // At T = 2: σ(2) = 0.88 on, σ(0) = 0.5 on at its threshold, σ(1) = 0.73 under 0.9.
        let (on, p) = noul(&head(HeadKind::Noul, 2.0), &[4.0, 0.0, 2.0]);
        assert_eq!(on, ["a", "b"]);
        assert!((p["a"] - 0.880_797).abs() < 1e-5);
    }
}
