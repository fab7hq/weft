//! Learning from the person's edits of Sense's defaults, on their own
//! machine: a logit bias per class, a rule score fitted by IRLS, and τ's
//! step. A fit is adopted only when its leave-one-out log-loss beats the
//! shipped model's.

/// The fewest overrides before a class bias is fitted.
pub const MIN_OVERRIDES: usize = 20;
/// The fewest rule decisions before a rule score is fitted.
pub const MIN_RULE_DECISIONS: usize = 30;
/// How far τ moves in one step, and the range it stays in.
pub const TAU_STEP: f32 = 0.05;
const TAU_RANGE: (f32, f32) = (0.05, 0.95);
const NEWTON_STEPS: usize = 25;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// One Ask's choice among a `choice` head's classes: the shipped logits
/// and the class the person sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub logits: Vec<f64>,
    pub class: usize,
}

/// The bias per class that best explains `rows`, L2-penalized by `l2`, by
/// Newton's method on the softmax log-likelihood.
pub fn fit_bias(rows: &[Chosen], l2: f64) -> Vec<f64> {
    let n = rows.first().map_or(0, |r| r.logits.len());
    let mut bias = vec![0.0; n];
    for _ in 0..NEWTON_STEPS {
        let mut grad: Vec<f64> = bias.iter().map(|b| 2.0 * l2 * b).collect();
        let mut hess = vec![vec![0.0; n]; n];
        for (i, row) in hess.iter_mut().enumerate() {
            row[i] = 2.0 * l2;
        }
        for r in rows {
            let p = softmax_with(&r.logits, &bias);
            for i in 0..n {
                grad[i] += p[i] - f64::from(u8::from(i == r.class));
                for j in 0..n {
                    hess[i][j] += p[i] * (f64::from(u8::from(i == j)) - p[j]);
                }
            }
        }
        let Some(step) = solve(hess, grad) else { break };
        bias.iter_mut().zip(&step).for_each(|(b, s)| *b -= s);
    }
    bias
}

/// A fitted bias, if there are enough overrides (rows whose class is not
/// the shipped top) and its leave-one-out log-loss beats no bias at all.
pub fn adopt_bias(rows: &[Chosen], l2: f64) -> Option<Vec<f64>> {
    let overrides = rows.iter().filter(|r| r.class != argmax(&r.logits)).count();
    if overrides < MIN_OVERRIDES {
        return None;
    }
    let loss =
        |r: &Chosen, bias: &Vec<f64>| -softmax_with(&r.logits, bias)[r.class].max(1e-12).ln();
    let shipped = rows.iter().map(|r| loss(r, &vec![0.0; r.logits.len()])).sum::<f64>();
    let held_out = leave_one_out(rows, |train| fit_bias(train, l2), loss);
    (held_out < shipped).then(|| fit_bias(rows, l2))
}

/// One rule Sense proposed and what the person did with it: its features
/// and whether they kept it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleDecision {
    /// `[s(r), ln(1+applied), (1+kept)/(2+applied), removed_last_30d,
    /// harness…]`; the fit adds its own intercept.
    pub features: Vec<f64>,
    /// The shipped model's probability, `s(r)`.
    pub shipped: f64,
    pub kept: bool,
}

/// L2 logistic regression by iteratively reweighted least squares: the
/// weights, the intercept first.
pub fn fit_irls(decisions: &[RuleDecision], l2: f64) -> Vec<f64> {
    let n = decisions.first().map_or(0, |d| d.features.len()) + 1;
    let mut w = vec![0.0; n];
    for _ in 0..NEWTON_STEPS {
        let mut grad: Vec<f64> = w.iter().map(|v| 2.0 * l2 * v).collect();
        let mut hess = vec![vec![0.0; n]; n];
        for (i, row) in hess.iter_mut().enumerate() {
            row[i] = 2.0 * l2;
        }
        for d in decisions {
            let x = with_intercept(&d.features);
            let p = sigmoid(dot(&w, &x));
            let weight = p * (1.0 - p);
            for i in 0..n {
                grad[i] += (p - f64::from(u8::from(d.kept))) * x[i];
                for j in 0..n {
                    hess[i][j] += weight * x[i] * x[j];
                }
            }
        }
        let Some(step) = solve(hess, grad) else { break };
        w.iter_mut().zip(&step).for_each(|(v, s)| *v -= s);
    }
    w
}

/// The probability a fitted rule score gives a decision's features.
pub fn rule_p(weights: &[f64], features: &[f64]) -> f64 {
    sigmoid(dot(weights, &with_intercept(features)))
}

/// A fitted rule score, if there are enough decisions and its
/// leave-one-out log-loss beats the shipped probabilities.
pub fn adopt_rule_score(decisions: &[RuleDecision], l2: f64) -> Option<Vec<f64>> {
    if decisions.len() < MIN_RULE_DECISIONS {
        return None;
    }
    let log_loss = |p: f64, kept: bool| -(if kept { p } else { 1.0 - p }).clamp(1e-12, 1.0).ln();
    let shipped = decisions.iter().map(|d| log_loss(d.shipped, d.kept)).sum::<f64>();
    let loss = |d: &RuleDecision, w: &Vec<f64>| log_loss(rule_p(w, &d.features), d.kept);
    let held_out = leave_one_out(decisions, |train| fit_irls(train, l2), loss);
    (held_out < shipped).then(|| fit_irls(decisions, l2))
}

/// What the person did with the rules of one recent Ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleEdits {
    pub shown: usize,
    pub removed: usize,
    pub added: usize,
}

/// τ after the last Asks' edits: one step up when more than a fifth of the
/// rules shown were removed; one step down when rules were added and none
/// removed; else as it was.
pub fn step_tau(tau: f32, recent: &[RuleEdits]) -> f32 {
    let shown: usize = recent.iter().map(|e| e.shown).sum();
    let removed: usize = recent.iter().map(|e| e.removed).sum();
    let added: usize = recent.iter().map(|e| e.added).sum();
    let stepped = if shown > 0 && removed * 5 > shown {
        tau + TAU_STEP
    } else if added > 0 && removed == 0 {
        tau - TAU_STEP
    } else {
        tau
    };
    stepped.clamp(TAU_RANGE.0, TAU_RANGE.1)
}

/// The `ask` label rows Sense answered, oldest first.
fn sensed(rows: &[serde_json::Value]) -> impl Iterator<Item = &serde_json::Value> {
    rows.iter().filter(|r| r["kind"] == "ask" && r["sense"].is_object())
}

/// Each sensed Ask's route choice: Sense's route probabilities as logits,
/// in its classes' order, and the route the person sent, when it is one.
pub fn route_choices(rows: &[serde_json::Value]) -> Vec<Chosen> {
    sensed(rows)
        .filter_map(|r| {
            let p = r["sense"]["route_p"].as_object()?;
            let sent = r["final"]["route"].as_str()?;
            let sent = sent.strip_prefix("native_").unwrap_or(sent);
            let class = p.keys().position(|c| c == sent)?;
            let logits = p.values().map(|v| v.as_f64().unwrap_or(0.0).max(1e-9).ln()).collect();
            Some(Chosen { logits, class })
        })
        .collect()
}

/// What the person did with each sensed Ask's rules.
pub fn rule_edits(rows: &[serde_json::Value]) -> Vec<RuleEdits> {
    let count = |r: &serde_json::Value, key: &str| r["final"][key].as_array().map_or(0, Vec::len);
    sensed(rows)
        .map(|r| RuleEdits {
            shown: r["sense"]["rules"].as_array().map_or(0, Vec::len),
            removed: count(r, "rules_removed"),
            added: count(r, "rules_added"),
        })
        .collect()
}

/// Every rule Sense proposed and whether it was kept, with the features
/// of what came before it: how often that rule was shown and kept, how
/// often removed in the 30 days before, and the harness.
pub fn rule_decisions(rows: &[serde_json::Value]) -> Vec<RuleDecision> {
    let mut harnesses: Vec<&str> = sensed(rows).filter_map(|r| r["harness"].as_str()).collect();
    harnesses.sort_unstable();
    harnesses.dedup();
    let mut past: Vec<(u64, &str, bool)> = Vec::new();
    let mut out = Vec::new();
    for r in sensed(rows) {
        let at = r["at"].as_u64().unwrap_or(0);
        let kept_ids = r["final"]["rules_kept"].as_array().cloned().unwrap_or_default();
        let harness = r["harness"].as_str().unwrap_or_default();
        let shown = r["sense"]["rules"].as_array().into_iter().flatten();
        let mut now = Vec::new();
        for pair in shown {
            let (Some(id), Some(p)) = (pair[0].as_str(), pair[1].as_f64()) else { continue };
            let kept = kept_ids.iter().any(|k| k == id);
            let before = past.iter().filter(|(_, rule, _)| *rule == id);
            let applied = before.clone().count() as f64;
            let kept_before = before.clone().filter(|(_, _, k)| *k).count() as f64;
            let recent = |t: &u64| at.saturating_sub(*t) <= 30 * DAY_MS;
            let removed = before.filter(|(t, _, k)| !k && recent(t)).count() as f64;
            let mut features =
                vec![p, (1.0 + applied).ln(), (1.0 + kept_before) / (2.0 + applied), removed];
            features.extend(harnesses.iter().map(|h| f64::from(u8::from(*h == harness))));
            out.push(RuleDecision { features, shipped: p, kept });
            now.push((at, id, kept));
        }
        past.extend(now);
    }
    out
}

/// The summed loss on each row of a fit made without it.
fn leave_one_out<R: Clone, M>(
    rows: &[R],
    fit: impl Fn(&[R]) -> M,
    loss: impl Fn(&R, &M) -> f64,
) -> f64 {
    (0..rows.len())
        .map(|i| {
            let train: Vec<R> =
                rows.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, r)| r.clone()).collect();
            loss(&rows[i], &fit(&train))
        })
        .sum()
}

fn softmax_with(logits: &[f64], bias: &[f64]) -> Vec<f64> {
    let z: Vec<f64> = logits.iter().zip(bias).map(|(l, b)| l + b).collect();
    let max = z.iter().copied().fold(f64::MIN, f64::max);
    let e: Vec<f64> = z.iter().map(|v| (v - max).exp()).collect();
    let sum: f64 = e.iter().sum();
    e.iter().map(|v| v / sum).collect()
}

fn argmax(v: &[f64]) -> usize {
    v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i)
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn with_intercept(features: &[f64]) -> Vec<f64> {
    [&[1.0][..], features].concat()
}

/// `a · x = b` by Gaussian elimination with partial pivoting; none when
/// `a` is singular.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, below) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (row, r) in below.iter_mut().enumerate() {
            let f = r[col] / pivot_row[col];
            r[col..].iter_mut().zip(&pivot_row[col..]).for_each(|(v, p)| *v -= f * p);
            b[col + 1 + row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let rest: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - rest) / a[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows where Sense says plan (class 1) but the person picks direct
    /// (class 0) `overridden` times in `n`.
    fn route_rows(n: usize, overridden: usize) -> Vec<Chosen> {
        (0..n)
            .map(|i| Chosen { logits: vec![0.0, 1.0, -1.0], class: usize::from(i >= overridden) })
            .collect()
    }

    #[test]
    fn a_class_bias_moves_toward_what_the_person_picks() {
        let bias = fit_bias(&route_rows(40, 30), 0.01);
        let p = softmax_with(&[0.0, 1.0, -1.0], &bias);
        assert!((p[0] - 0.75).abs() < 0.03, "direct is picked 3 times in 4: {p:?}");
        assert!(p[2] < 0.05, "a class never picked sinks: {p:?}");
    }

    #[test]
    fn a_bias_is_adopted_only_with_enough_overrides_and_a_better_held_out_loss() {
        assert!(adopt_bias(&route_rows(40, 30), 0.01).is_some(), "30 overrides, a clear lean");
        assert_eq!(adopt_bias(&route_rows(40, 19), 0.01), None, "19 overrides are too few");
        // Sense already fits: picks in softmax([0, 1, -1])'s proportions,
        // 0.245, 0.665 and 0.090 of 200.
        let classes = [0; 49].into_iter().chain([1; 133]).chain([2; 18]);
        let fitting: Vec<Chosen> =
            classes.map(|class| Chosen { logits: vec![0.0, 1.0, -1.0], class }).collect();
        assert_eq!(adopt_bias(&fitting, 0.01), None, "no better than the shipped model");
    }

    /// Decisions kept when the rule was kept often before, whatever Sense said.
    fn decisions(n: usize) -> Vec<RuleDecision> {
        (0..n)
            .map(|i| {
                let habit = (i % 4) as f64 / 3.0;
                let kept = habit > 0.5;
                RuleDecision { features: vec![0.6, 1.0, habit, 0.0], shipped: 0.6, kept }
            })
            .collect()
    }

    #[test]
    fn irls_learns_the_feature_that_decides() {
        let w = fit_irls(&decisions(40), 0.01);
        assert!(rule_p(&w, &[0.6, 1.0, 1.0, 0.0]) > 0.9);
        assert!(rule_p(&w, &[0.6, 1.0, 0.0, 0.0]) < 0.1);
    }

    #[test]
    fn a_rule_score_is_adopted_only_with_enough_decisions_and_a_better_held_out_loss() {
        assert!(adopt_rule_score(&decisions(40), 0.01).is_some());
        assert_eq!(adopt_rule_score(&decisions(29), 0.01), None, "29 decisions are too few");
        // Shipped probabilities that already say what happens are not beaten.
        let perfect: Vec<RuleDecision> = decisions(40)
            .into_iter()
            .map(|d| RuleDecision { shipped: if d.kept { 0.999 } else { 0.001 }, ..d })
            .collect();
        assert_eq!(adopt_rule_score(&perfect, 0.01), None);
    }

    fn row(
        at: u64,
        harness: &str,
        sent: &str,
        shown: &[&str],
        kept: &[&str],
        added: &[&str],
    ) -> serde_json::Value {
        let removed: Vec<&&str> = shown.iter().filter(|r| !kept.contains(r)).collect();
        serde_json::json!({
            "schema": "weft.label/1", "kind": "ask", "at": at, "harness": harness,
            "sense": {"version": "0.3.0", "route": ["plan", 0.7],
                      "route_p": {"direct": 0.2, "goal": 0.1, "plan": 0.7},
                      "rules": shown.iter().map(|r| serde_json::json!([r, 0.6])).collect::<Vec<_>>()},
            "final": {"route": sent, "rules_kept": kept, "rules_removed": removed, "rules_added": added},
        })
    }

    #[test]
    fn the_ask_rows_give_route_choices_rule_decisions_and_edits() {
        let rows = vec![
            row(0, "codex", "native_direct", &["a", "b"], &["a"], &[]),
            serde_json::json!({"kind": "ask", "sense": null, "final": {"route": "native_plan"}}),
            serde_json::json!({"kind": "seal"}),
            row(DAY_MS, "claude-code", "native_plan", &["a"], &[], &["c"]),
        ];
        let choices = route_choices(&rows);
        assert_eq!(choices.len(), 2, "only Asks Sense answered");
        assert_eq!((choices[0].class, choices[1].class), (0, 2), "direct, then plan");
        assert!((choices[1].logits[2] - 0.7_f64.ln()).abs() < 1e-12);
        let edits = rule_edits(&rows);
        assert_eq!(edits[0], RuleEdits { shown: 2, removed: 1, added: 0 });
        assert_eq!(edits[1], RuleEdits { shown: 1, removed: 1, added: 1 });
        let d = rule_decisions(&rows);
        assert_eq!(d.len(), 3);
        assert!(d[0].kept && !d[1].kept && !d[2].kept);
        // The second Ask's `a`: shown once and kept once before, none removed; on claude-code.
        assert_eq!(d[2].features, vec![0.6, 2.0_f64.ln(), 2.0 / 3.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn tau_steps_up_on_removals_down_on_additions_and_stays_in_range() {
        let edits = |shown, removed, added| RuleEdits { shown, removed, added };
        assert_eq!(step_tau(0.4, &[edits(10, 3, 0)]), 0.4 + TAU_STEP, "30% removed");
        assert_eq!(step_tau(0.4, &[edits(10, 2, 0)]), 0.4, "20% is not more than a fifth");
        assert_eq!(step_tau(0.4, &[edits(10, 0, 1)]), 0.4 - TAU_STEP, "added, none removed");
        assert_eq!(step_tau(0.4, &[edits(10, 1, 1)]), 0.4, "added and removed");
        assert_eq!(step_tau(0.95, &[edits(10, 9, 0)]), 0.95);
        assert_eq!(step_tau(0.05, &[edits(10, 0, 3)]), 0.05);
    }
}
