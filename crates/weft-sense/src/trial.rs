//! Whether a GPU earns its place: the bundle's golden Asks answered on it
//! must give the CPU's answers, at a lower p50.

use crate::SenseModel;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

/// The golden vectors a bundle is published with.
pub const GOLDEN: &str = "golden_vectors.jsonl";
/// Each golden Ask is answered this many times for its p50.
const RUNS: usize = 10;

/// What a provider made of the golden Asks: each one's answer, and the p50
/// of an Ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trial {
    pub answers: Vec<String>,
    pub p50_ms: f64,
}

impl SenseModel {
    /// The golden Asks of the bundle in `dir`, answered on this model.
    pub fn trial(&self, dir: &Path) -> anyhow::Result<Trial> {
        let golden = std::fs::read_to_string(dir.join(GOLDEN))?;
        let mut answers = Vec::new();
        let mut times = Vec::new();
        for row in golden.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        {
            let Some(text) = row["text"].as_str().filter(|_| row["head"] == "ask") else {
                continue;
            };
            let mut answer = None;
            for _ in 0..RUNS {
                let start = Instant::now();
                answer = Some(self.ask(text, &[], &[])?);
                times.push(start.elapsed().as_secs_f64() * 1e3);
            }
            let a = answer.expect("answered");
            answers.push(format!(
                "{:?} {:?} {:?} {}",
                a.task.on, a.effects.on, a.route.pick, a.result
            ));
        }
        anyhow::ensure!(!answers.is_empty(), "the bundle's golden vectors hold no Ask");
        times.sort_by(f64::total_cmp);
        Ok(Trial { answers, p50_ms: times[times.len() / 2] })
    }
}

/// Whether the GPU is kept: its answers are the CPU's, and it is faster.
pub fn keeps_gpu(cpu: &Trial, gpu: &Trial) -> bool {
    gpu.answers == cpu.answers && gpu.p50_ms < cpu.p50_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trial(answers: &[&str], p50_ms: f64) -> Trial {
        Trial { answers: answers.iter().map(|a| a.to_string()).collect(), p50_ms }
    }

    #[test]
    fn a_gpu_is_kept_only_with_the_cpus_answers_at_a_lower_p50() {
        let cpu = trial(&["a", "b"], 120.0);
        assert!(keeps_gpu(&cpu, &trial(&["a", "b"], 63.0)));
        assert!(!keeps_gpu(&cpu, &trial(&["a", "c"], 63.0)), "an answer differs");
        assert!(!keeps_gpu(&cpu, &trial(&["a", "b"], 120.0)), "not faster");
        assert!(!keeps_gpu(&cpu, &trial(&["a", "b"], 400.0)), "slower");
    }
}
