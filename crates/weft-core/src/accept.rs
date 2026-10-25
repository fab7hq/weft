//! Whether you still tend to turn down an Eval that is ready to seal: a small
//! logistic model over your own past Seals. It can only demote "Ready to
//! seal" to "Needs you", never promote.

use std::collections::BTreeSet;

use crate::board::Notice;
use crate::eval_view::EvalView;

/// It learns nothing from fewer Seals than this.
pub const MIN_SEALS: usize = 30;
/// The share of "Ready to seal" that may turn out wrong, by leave-one-out.
pub const WRONG_READY: f64 = 0.05;
/// The L2 penalty on every weight but the intercept.
const L2: f64 = 1.0;

/// One past Seal: what its Eval showed, whether it was ready to seal, and
/// whether you accepted it.
#[derive(Debug, Clone, PartialEq)]
pub struct Seal {
    pub features: Vec<f64>,
    pub ready: bool,
    pub accepted: bool,
}

/// What an Eval showed, as the model reads it. The first is the intercept.
pub fn features(view: &EvalView) -> Vec<f64> {
    let verdict = |v: &str| f64::from(u8::from(view.verdict.as_deref() == Some(v)));
    let files: BTreeSet<&str> = view.changes.iter().map(|c| c.path.as_str()).collect();
    vec![
        1.0,
        verdict("aligned"),
        verdict("drifted"),
        view.confidence.unwrap_or(0.0),
        view.commission.unwrap_or(0.0),
        view.omission.unwrap_or(0.0),
        view.findings.len() as f64,
        view.split.len() as f64,
        f64::from(u8::from(!view.checks_passed)),
        (view.changes.len() as f64).ln_1p(),
        (files.len() as f64).ln_1p(),
    ]
}

/// The learned model, and the p(accept) below which a "Ready" is demoted.
#[derive(Debug, Clone, PartialEq)]
pub struct Demotion {
    weights: Vec<f64>,
    threshold: f64,
}

impl Demotion {
    /// `None` below [`MIN_SEALS`]. The threshold is the lowest at which the
    /// "Ready" it would keep were wrong at most [`WRONG_READY`] of the time,
    /// each Seal scored by a model fitted without it.
    pub fn learn(seals: &[Seal]) -> Option<Demotion> {
        if seals.len() < MIN_SEALS {
            return None;
        }
        let held_out = leave_one_out(seals);
        let ready: Vec<(f64, bool)> = seals
            .iter()
            .zip(&held_out)
            .filter(|(s, _)| s.ready)
            .map(|(s, p)| (*p, s.accepted))
            .collect();
        let mut candidates: Vec<f64> = ready.iter().map(|(p, _)| *p).collect();
        candidates.push(0.0);
        candidates.push(f64::INFINITY);
        candidates.sort_by(f64::total_cmp);
        let threshold = candidates
            .into_iter()
            .find(|t| {
                let kept: Vec<bool> =
                    ready.iter().filter(|(p, _)| p >= t).map(|(_, a)| *a).collect();
                let wrong = kept.iter().filter(|a| !**a).count();
                wrong as f64 <= WRONG_READY * kept.len() as f64
            })
            .unwrap_or(f64::INFINITY);
        Some(Demotion { weights: fit(seals), threshold })
    }

    pub fn p_accept(&self, features: &[f64]) -> f64 {
        sigmoid(dot(&self.weights, features))
    }

    /// "Ready to seal" stays only while p(accept) is high enough; anything
    /// else is left as it was.
    pub fn apply(&self, notice: Notice, features: &[f64]) -> Notice {
        if notice == Notice::ReadyToSeal && self.p_accept(features) < self.threshold {
            Notice::NeedsYou
        } else {
            notice
        }
    }
}

/// Each Seal's p(accept) from a model fitted on every other Seal.
fn leave_one_out(seals: &[Seal]) -> Vec<f64> {
    (0..seals.len())
        .map(|i| {
            let rest: Vec<Seal> =
                seals.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, s)| s.clone()).collect();
            sigmoid(dot(&fit(&rest), &seals[i].features))
        })
        .collect()
}

/// L2-penalised logistic regression by iteratively reweighted least squares
/// (Newton's method); the intercept, the first feature, is not penalised.
fn fit(seals: &[Seal]) -> Vec<f64> {
    let d = seals.first().map_or(0, |s| s.features.len());
    let mut w = vec![0.0; d];
    for _ in 0..50 {
        let mut grad: Vec<f64> = w.iter().enumerate().map(|(k, wk)| penalty(k) * wk).collect();
        let mut hess = vec![vec![0.0; d]; d];
        for (k, row) in hess.iter_mut().enumerate() {
            row[k] = penalty(k).max(1e-9);
        }
        for s in seals {
            let p = sigmoid(dot(&w, &s.features));
            let y = f64::from(u8::from(s.accepted));
            let weight = p * (1.0 - p);
            for (k, xk) in s.features.iter().enumerate() {
                grad[k] += (p - y) * xk;
                for (l, xl) in s.features.iter().enumerate() {
                    hess[k][l] += weight * xk * xl;
                }
            }
        }
        let step = solve(hess, grad);
        for (wk, sk) in w.iter_mut().zip(&step) {
            *wk -= sk;
        }
        if step.iter().map(|s| s * s).sum::<f64>() < 1e-16 {
            break;
        }
    }
    w
}

fn penalty(k: usize) -> f64 {
    if k == 0 { 0.0 } else { L2 }
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// `a x = b` by Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Vec<f64> {
    let n = b.len();
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs())).unwrap();
        a.swap(col, pivot);
        b.swap(col, pivot);
        let (top, below) = a.split_at_mut(col + 1);
        let pivot_row = &top[col];
        for (i, row) in below.iter_mut().enumerate() {
            let f = row[col] / pivot_row[col];
            for (r, p) in row[col..].iter_mut().zip(&pivot_row[col..]) {
                *r -= f * p;
            }
            b[col + 1 + i] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let rest: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - rest) / a[row][row];
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed stream of numbers in [0, 1), so the synthetic Seals are the
    /// same on every run.
    fn stream(seed: u64) -> impl FnMut() -> f64 {
        let mut s = seed;
        move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (s >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Ready Seals over one feature, `risk`: you accept below 0.7 and turn
    /// down above it, three in a hundred either way flipped.
    fn ready_seals(n: usize, seed: u64) -> Vec<Seal> {
        let mut next = stream(seed);
        (0..n)
            .map(|_| {
                let risk = next();
                let flip = next() < 0.03;
                Seal { features: vec![1.0, risk], ready: true, accepted: (risk < 0.7) != flip }
            })
            .collect()
    }

    #[test]
    fn the_fit_finds_what_predicts_a_turn_down() {
        let w = fit(&ready_seals(200, 7));
        assert!(w[1] < -1.0, "more risk, less accepted: {w:?}");
        let p = |risk: f64| sigmoid(dot(&w, &[1.0, risk]));
        assert!(p(0.1) > 0.8 && p(0.95) < 0.5, "{} {}", p(0.1), p(0.95));
    }

    #[test]
    fn it_learns_nothing_below_thirty_seals() {
        assert_eq!(Demotion::learn(&ready_seals(MIN_SEALS - 1, 1)), None);
        assert!(Demotion::learn(&ready_seals(MIN_SEALS, 1)).is_some());
    }

    /// The threshold keeps wrong "Ready" at or under 5%, as leave-one-out
    /// measures it, and demotes no more than that takes.
    #[test]
    fn it_demotes_only_as_far_as_keeps_wrong_ready_at_five_percent_by_leave_one_out() {
        let seals = ready_seals(80, 3);
        let model = Demotion::learn(&seals).unwrap();
        let held_out = leave_one_out(&seals);
        let kept: Vec<bool> = seals
            .iter()
            .zip(&held_out)
            .filter(|(_, p)| **p >= model.threshold)
            .map(|(s, _)| s.accepted)
            .collect();
        let wrong = kept.iter().filter(|a| !**a).count() as f64;
        assert!(!kept.is_empty(), "something is still ready");
        assert!(wrong <= WRONG_READY * kept.len() as f64, "{wrong} wrong of {}", kept.len());
        let below = seals.iter().zip(&held_out).filter(|(_, p)| **p < model.threshold);
        let tighter = below.map(|(_, p)| *p).fold(f64::NEG_INFINITY, f64::max);
        let at_tighter: Vec<bool> = seals
            .iter()
            .zip(&held_out)
            .filter(|(_, p)| **p >= tighter)
            .map(|(s, _)| s.accepted)
            .collect();
        let wrong_tighter = at_tighter.iter().filter(|a| !**a).count() as f64;
        assert!(
            wrong_tighter > WRONG_READY * at_tighter.len() as f64,
            "one step lower would let more than 5% wrong through"
        );
        assert_eq!(model.apply(Notice::ReadyToSeal, &[1.0, 0.95]), Notice::NeedsYou);
        assert_eq!(model.apply(Notice::ReadyToSeal, &[1.0, 0.05]), Notice::ReadyToSeal);
    }

    #[test]
    fn it_never_promotes() {
        let model = Demotion::learn(&ready_seals(80, 3)).unwrap();
        assert_eq!(model.apply(Notice::NeedsYou, &[1.0, 0.0]), Notice::NeedsYou);
    }

    #[test]
    fn a_ready_you_always_accept_is_left_alone() {
        let seals: Vec<Seal> =
            ready_seals(40, 5).into_iter().map(|s| Seal { accepted: true, ..s }).collect();
        let model = Demotion::learn(&seals).unwrap();
        assert_eq!(model.threshold, 0.0);
        assert_eq!(model.apply(Notice::ReadyToSeal, &[1.0, 0.99]), Notice::ReadyToSeal);
    }

    #[test]
    fn the_features_are_what_the_eval_showed() {
        let view = EvalView {
            closed: true,
            verdict: Some("aligned".into()),
            confidence: Some(0.9),
            commission: Some(0.1),
            omission: Some(0.0),
            findings: vec!["scaffolding: x".into()],
            checks_passed: true,
            ..Default::default()
        };
        assert_eq!(features(&view), [1.0, 1.0, 0.0, 0.9, 0.1, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
    }
}
