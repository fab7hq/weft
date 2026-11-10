//! How a head reads its rows' logits, by its kind (sense-d spec/bundle.md
//! §3): a `choice` head by a softmax across its options, a `noul` head by
//! each class's own sigmoid and threshold; since bundle 2, a turn end by its
//! `sure` cut and a plan pick by `sure` and `margin` (§6).

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

/// A turn end's answer (bundle 2): the probability that the reply asks its
/// person, and whether it does, at or above the head's `sure` cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnEnd {
    pub probability: f32,
    pub asks: bool,
}

/// The `turn_end` head's answer on its one logit.
pub fn turn_end(head: &Head, logit: f32) -> TurnEnd {
    let probability = sigmoid(logit / head.temperature);
    TurnEnd { probability, asks: probability >= head.sure.unwrap_or(1.0) }
}

/// The `plan` head's default among each candidate's logit: the top one, at
/// or above `sure` and ahead of the runner-up by `margin`; otherwise none.
pub fn plan_default(head: &Head, logits: &[(String, f32)]) -> Option<String> {
    let mut p: Vec<(f32, &String)> =
        logits.iter().map(|(slug, z)| (sigmoid(z / head.temperature), slug)).collect();
    p.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (top, slug) = *p.first()?;
    let runner_up = p.get(1).map_or(0.0, |r| r.0);
    let sure = top >= head.sure.unwrap_or(1.0) && top - runner_up >= head.margin.unwrap_or(1.0);
    sure.then(|| slug.clone())
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

    fn bundle_2_head(sure: f32, margin: Option<f32>) -> Head {
        let json = serde_json::json!({"name": "h", "kind": "noul_per_item", "temperature": 1.0,
            "sure": sure, "margin": margin});
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_turn_end_asks_at_or_above_its_sure_cut_and_says_nothing_below() {
        let h = bundle_2_head(0.8, None);
        let asks = turn_end(&h, 3.0);
        assert!(asks.asks && (asks.probability - 0.952_574).abs() < 1e-5);
        assert!(!turn_end(&h, 0.0).asks);
    }

    #[test]
    fn the_plan_default_is_the_top_candidate_only_when_sure_and_ahead() {
        let h = bundle_2_head(0.7, Some(0.15));
        let pick = |logits: &[(&str, f32)]| {
            let owned: Vec<(String, f32)> =
                logits.iter().map(|(s, z)| (s.to_string(), *z)).collect();
            plan_default(&h, &owned)
        };
        assert_eq!(pick(&[("site", 3.0), ("weft", -2.0)]).as_deref(), Some("site"));
        assert_eq!(pick(&[("site", 0.2), ("weft", -2.0)]), None, "below the sure cut");
        assert_eq!(pick(&[("site", 3.0), ("weft", 2.9)]), None, "not ahead of the runner-up");
        assert_eq!(pick(&[]), None);
    }

    #[test]
    fn a_noul_class_is_on_at_or_above_its_own_threshold_after_temperature() {
        // At T = 2: σ(2) = 0.88 on, σ(0) = 0.5 on at its threshold, σ(1) = 0.73 under 0.9.
        let (on, p) = noul(&head(HeadKind::Noul, 2.0), &[4.0, 0.0, 2.0]);
        assert_eq!(on, ["a", "b"]);
        assert!((p["a"] - 0.880_797).abs() < 1e-5);
    }
}
