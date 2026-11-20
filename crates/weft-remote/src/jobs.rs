//! What takes a while in a project, as devices are told it (remote.md §6):
//! each job weftd sends, relayed at most once a second unless a step changed
//! or it ended, and kept so a device that connects is told the jobs there
//! are. One that ended is kept a few seconds; an Eval that could not open,
//! until its work is acted on again.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use weft_proto::{Job, StepState};

/// A job's counts reach a device at most this often.
pub const EVERY: Duration = Duration::from_secs(1);

/// An ended job is kept this long for a device that connects.
pub const KEPT: Duration = Duration::from_secs(5);

/// One job as it was relayed.
struct Relayed {
    job: Job,
    /// The device and its request that began it, when one did.
    asker: Option<(u64, Value)>,
    sent: Option<Instant>,
    /// What changes at once: each step's key and state, and how it ended.
    shape: Vec<(String, StepState)>,
    /// A newer count, held back.
    held: bool,
    ended: Option<Instant>,
}

/// A project's jobs.
#[derive(Default)]
pub struct Jobs {
    kept: BTreeMap<String, Relayed>,
}

/// A job to relay now, and the device and request that began it.
pub struct Relay {
    pub job: Job,
    pub asker: Option<(u64, Value)>,
}

fn shape(job: &Job) -> Vec<(String, StepState)> {
    job.steps.iter().map(|s| (s.key.clone(), s.state)).collect()
}

/// An Eval that could not open: kept until its work is acted on again.
fn failed_open(job: &Job) -> bool {
    job.kind == "eval_open" && job.end.as_ref().is_some_and(|e| !e.ok)
}

impl Jobs {
    /// The jobs a project had when it opened here.
    pub fn of(jobs: &Value, at: Instant) -> Self {
        let mut kept = BTreeMap::new();
        for job in jobs.as_array().into_iter().flatten() {
            let Ok(job) = serde_json::from_value::<Job>(job.clone()) else { continue };
            let ended = job.end.is_some().then_some(at);
            let r = Relayed {
                shape: shape(&job),
                job,
                asker: None,
                sent: Some(at),
                held: false,
                ended,
            };
            kept.insert(r.job.id.clone(), r);
        }
        Jobs { kept }
    }

    /// weftd sent a job: what to relay now, if anything. `asker` is the
    /// device and request whose call began it, when it carries that call.
    pub fn heard(
        &mut self,
        mut job: Job,
        asker: Option<(u64, Value)>,
        at: Instant,
    ) -> Option<Relay> {
        job.call = None;
        // Something new about a work answers an Eval of it that could not open.
        if let Some(work) = job.on.work.clone() {
            self.kept.retain(|id, r| {
                *id == job.id || !failed_open(&r.job) || r.job.on.work.as_ref() != Some(&work)
            });
        }
        let r = self.kept.entry(job.id.clone()).or_insert_with(|| Relayed {
            job: job.clone(),
            asker: None,
            sent: None,
            shape: Vec::new(),
            held: false,
            ended: None,
        });
        if r.asker.is_none() {
            r.asker = asker;
        }
        let at_once = shape(&job) != r.shape || job.end.is_some() || r.sent.is_none();
        r.shape = shape(&job);
        r.job = job;
        if r.job.end.is_some() {
            r.ended.get_or_insert(at);
        }
        if at_once || r.sent.is_none_or(|s| at.saturating_duration_since(s) >= EVERY) {
            (r.sent, r.held) = (Some(at), false);
            return Some(Relay { job: r.job.clone(), asker: r.asker.clone() });
        }
        r.held = true;
        None
    }

    /// The counts held back that are due now; ended jobs past [`KEPT`] go.
    pub fn due(&mut self, at: Instant) -> Vec<Relay> {
        self.kept.retain(|_, r| {
            failed_open(&r.job) || r.ended.is_none_or(|e| at.saturating_duration_since(e) < KEPT)
        });
        let mut out = Vec::new();
        for r in self.kept.values_mut() {
            if r.held && r.sent.is_none_or(|s| at.saturating_duration_since(s) >= EVERY) {
                (r.sent, r.held) = (Some(at), false);
                out.push(Relay { job: r.job.clone(), asker: r.asker.clone() });
            }
        }
        out
    }

    /// A device acted on a work, named by any Ask of it: an Eval of it that
    /// could not open is said no more.
    pub fn acted_on(&mut self, ask: &str) {
        self.kept.retain(|_, r| {
            !failed_open(&r.job)
                || (r.job.on.work.as_deref() != Some(ask) && r.job.on.ask.as_deref() != Some(ask))
        });
    }

    /// Every job kept, as a device that connects is told them.
    pub fn all(&self) -> Vec<Job> {
        self.kept.values().map(|r| r.job.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_proto::{JobEnd, JobOn, JobStep};

    fn job(state: StepState, done: u64) -> Job {
        Job {
            id: "job_1".into(),
            kind: "seal".into(),
            on: JobOn { work: Some("ask_1".into()), ..JobOn::default() },
            call: Some(9),
            steps: vec![JobStep {
                key: "step.reading_files_now".into(),
                args: Default::default(),
                state,
                ms: Some(10),
                done: Some(done),
                total: Some(100),
                unit: Some("files".into()),
            }],
            share: None,
            age_ms: 500,
            end: None,
        }
    }

    #[test]
    fn counts_reach_a_device_once_a_second_and_a_step_change_at_once() {
        let (mut jobs, t0) = (Jobs::default(), Instant::now());
        let first = jobs.heard(job(StepState::Now, 1), Some((3, "r1".into())), t0).unwrap();
        assert_eq!(first.job.call, None, "the daemon's call is its own");
        assert_eq!(first.asker, Some((3, Value::from("r1"))));
        let soon = t0 + Duration::from_millis(300);
        assert!(jobs.heard(job(StepState::Now, 2), None, soon).is_none(), "held back");
        assert!(jobs.due(soon).is_empty());
        let due = jobs.due(t0 + EVERY);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].job.steps[0].done, Some(2));
        assert_eq!(due[0].asker, Some((3, Value::from("r1"))), "still the asker's");
        let changed = jobs.heard(job(StepState::Done, 100), None, t0 + EVERY).unwrap();
        assert_eq!(changed.job.steps[0].state, StepState::Done);
    }

    #[test]
    fn an_ended_job_is_kept_a_few_seconds_and_a_failed_open_until_acted_on() {
        let (mut jobs, t0) = (Jobs::default(), Instant::now());
        let mut ended = job(StepState::Done, 100);
        ended.end = Some(JobEnd { ok: true, why: None });
        assert!(jobs.heard(ended, None, t0).is_some());
        assert_eq!(jobs.all().len(), 1);
        jobs.due(t0 + KEPT);
        assert!(jobs.all().is_empty());

        let mut failed = job(StepState::Failed, 0);
        (failed.kind, failed.end) =
            ("eval_open".into(), Some(JobEnd { ok: false, why: Some("no".into()) }));
        jobs.heard(failed, None, t0);
        jobs.due(t0 + KEPT * 10);
        assert_eq!(jobs.all().len(), 1, "kept until acted on");
        jobs.acted_on("ask_1");
        assert!(jobs.all().is_empty());
    }
}
