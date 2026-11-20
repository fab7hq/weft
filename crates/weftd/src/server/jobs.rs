//! Something the machine does that takes a while, told as it goes: a job.
//!
//! A thread doing the work holds a [`Handle`] and says when it moves on a
//! step, counts, or ends. The run loop keeps every job and decides what is
//! sent: nothing until a job has run [`QUIET`], so a short wait shows nothing
//! new; a step change or an end at once; counts at most every [`COUNTS`].
//! A project's jobs go to its clients, a machine's to every client; the copy
//! to the client whose call began it carries that call.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use weft_proto::{Job, JobEnd, JobOn, JobStep, StepState};

use super::Wake;

/// A job is sent only once it has run this long.
pub(super) const QUIET: Duration = Duration::from_millis(400);

/// A job's counts are sent at most this often.
pub(super) const COUNTS: Duration = Duration::from_millis(250);

/// An ended job stays in what a client opening its project is given this long.
pub(super) const KEPT: Duration = Duration::from_secs(5);

/// One step as a job begins with it.
pub(super) fn step(key: &str, args: &[(&str, &str)], unit: Option<&str>) -> JobStep {
    JobStep {
        key: key.to_string(),
        args: args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        state: StepState::Next,
        ms: None,
        done: None,
        total: None,
        unit: unit.map(str::to_string),
    }
}

/// What a thread tells of its job.
pub(super) enum Move {
    Begin {
        kind: String,
        on: JobOn,
        project: Option<usize>,
        call: Option<(u64, u64)>,
        steps: Vec<JobStep>,
    },
    /// Step `i` is under way: the ones before it are done, unless skipped.
    Now(usize),
    /// The step with this key is under way, when the job has one.
    To(String),
    /// Step `i` does not happen in this job.
    Skip(usize),
    /// The step under way counts `done` of `total`.
    Count(u64, Option<u64>),
    /// A value of the step under way's words.
    Arg(String, String),
    /// Every step's key, state and count, as a reader of the record sees
    /// them now.
    Steps(Vec<JobStep>),
    On(JobOn),
    End(Result<(), String>),
}

/// Makes jobs: each gets an id unique for this daemon's run.
#[derive(Clone)]
pub(super) struct Jobs {
    tx: Sender<Wake>,
    next: Arc<AtomicU64>,
}

impl Jobs {
    pub(super) fn new(tx: Sender<Wake>) -> Self {
        Jobs { tx, next: Arc::new(AtomicU64::new(1)) }
    }

    /// A job of `kind` on `on`, in `project` or the machine's when none,
    /// begun by `call` (a client and its call id), with its steps.
    pub(super) fn begin(
        &self,
        kind: &str,
        on: JobOn,
        project: Option<usize>,
        call: Option<(u64, u64)>,
        steps: Vec<JobStep>,
    ) -> Handle {
        let id = format!("job_{}", self.next.fetch_add(1, Ordering::Relaxed));
        let handle = Handle { id, tx: self.tx.clone() };
        handle.tell(Move::Begin { kind: kind.to_string(), on, project, call, steps });
        handle
    }
}

/// A job, as the thread doing it holds it.
#[derive(Clone)]
pub(super) struct Handle {
    pub(super) id: String,
    tx: Sender<Wake>,
}

impl Handle {
    fn tell(&self, change: Move) {
        let _ = self.tx.send(Wake::Progress { id: self.id.clone(), change });
    }

    pub(super) fn now(&self, i: usize) {
        self.tell(Move::Now(i));
    }

    pub(super) fn to(&self, key: &str) {
        self.tell(Move::To(key.to_string()));
    }

    pub(super) fn skip(&self, i: usize) {
        self.tell(Move::Skip(i));
    }

    pub(super) fn count(&self, done: u64, total: Option<u64>) {
        self.tell(Move::Count(done, total));
    }

    pub(super) fn arg(&self, key: &str, value: String) {
        self.tell(Move::Arg(key.to_string(), value));
    }

    pub(super) fn steps(&self, steps: Vec<JobStep>) {
        self.tell(Move::Steps(steps));
    }

    pub(super) fn on(&self, on: JobOn) {
        self.tell(Move::On(on));
    }

    pub(super) fn end(&self, result: Result<(), String>) {
        self.tell(Move::End(result));
    }
}

/// One job as the run loop keeps it.
struct Kept {
    job: Job,
    project: Option<usize>,
    call: Option<(u64, u64)>,
    began: Instant,
    /// When each step began and ended.
    times: Vec<(Option<Instant>, Option<Instant>)>,
    sent: Option<Instant>,
    /// Moved since it was last sent.
    dirty: bool,
    ended: Option<Instant>,
    /// Kept past [`KEPT`]: an Eval that could not open, until its work's
    /// Eval is pressed again.
    keep: bool,
}

/// A job to send now: to whom, and as what.
pub(super) struct Send {
    pub(super) job: Job,
    pub(super) project: Option<usize>,
    pub(super) call: Option<(u64, u64)>,
}

/// Every job, as the run loop keeps it.
#[derive(Default)]
pub(super) struct Board {
    kept: BTreeMap<String, Kept>,
}

impl Kept {
    fn now(&self) -> Option<usize> {
        self.job.steps.iter().position(|s| s.state == StepState::Now)
    }

    fn set(&mut self, i: usize, state: StepState, at: Instant) {
        let Some(step) = self.job.steps.get_mut(i) else { return };
        let was = step.state;
        step.state = state;
        let (began, ended) = &mut self.times[i];
        if state == StepState::Now && was != StepState::Now {
            (*began, *ended) = (Some(at), None);
        }
        if was == StepState::Now && state != StepState::Now {
            *ended = Some(at);
        }
    }

    /// Step `i` under way: those before it done, unless skipped or failed,
    /// those after it to come.
    fn move_to(&mut self, i: usize, at: Instant) {
        for j in 0..self.job.steps.len() {
            let state = self.job.steps[j].state;
            let next = match j.cmp(&i) {
                std::cmp::Ordering::Less if matches!(state, StepState::Skipped) => state,
                std::cmp::Ordering::Less => StepState::Done,
                std::cmp::Ordering::Equal => StepState::Now,
                std::cmp::Ordering::Greater if state == StepState::Skipped => state,
                std::cmp::Ordering::Greater => StepState::Next,
            };
            self.set(j, next, at);
        }
    }

    /// The job as a client is sent it now.
    fn shown(&self, at: Instant) -> Job {
        let mut job = self.job.clone();
        let end = self.ended.unwrap_or(at);
        job.age_ms = end.saturating_duration_since(self.began).as_millis() as u64;
        for (step, (began, ended)) in job.steps.iter_mut().zip(&self.times) {
            step.ms =
                began.map(|b| ended.unwrap_or(end).saturating_duration_since(b).as_millis() as u64);
        }
        job.share = share(&job);
        job
    }
}

/// How much of a job is done, 0 to 1: its steps done, and the share of the
/// one under way its count says. None for a single step with no count.
fn share(job: &Job) -> Option<f32> {
    if job.end.as_ref().is_some_and(|e| e.ok) {
        return Some(1.0);
    }
    let steps: Vec<&JobStep> = job.steps.iter().filter(|s| s.state != StepState::Skipped).collect();
    let part = |s: &JobStep| match (s.done, s.total) {
        (Some(d), Some(t)) if t > 0 => Some((d as f32 / t as f32).min(1.0)),
        _ => None,
    };
    let now = steps.iter().find(|s| s.state == StepState::Now);
    let counted = now.and_then(|s| part(s));
    if steps.len() < 2 && counted.is_none() {
        return None;
    }
    let done = steps.iter().filter(|s| s.state == StepState::Done).count() as f32;
    Some(((done + counted.unwrap_or(0.0)) / steps.len().max(1) as f32).min(1.0))
}

impl Board {
    /// Take a thread's move; say what to send now, if anything.
    pub(super) fn apply(&mut self, id: &str, change: Move, at: Instant) -> Option<Send> {
        if let Move::Begin { kind, on, project, call, steps } = change {
            let times = vec![(None, None); steps.len()];
            let job = Job {
                id: id.to_string(),
                kind,
                on,
                call: None,
                steps,
                share: None,
                age_ms: 0,
                end: None,
            };
            let kept = Kept {
                job,
                project,
                call,
                began: at,
                times,
                sent: None,
                dirty: true,
                ended: None,
                keep: false,
            };
            self.kept.insert(id.to_string(), kept);
            return None;
        }
        let k = self.kept.get_mut(id)?;
        if k.ended.is_some() {
            return None;
        }
        let mut at_once = false;
        let before = (k.job.steps.clone(), k.job.on.clone());
        match change {
            Move::Begin { .. } => unreachable!("taken above"),
            Move::Now(i) => {
                at_once = k.now() != Some(i);
                k.move_to(i, at);
            }
            Move::To(key) => {
                let i = k.job.steps.iter().position(|s| s.key == key)?;
                at_once = k.now() != Some(i);
                k.move_to(i, at);
            }
            Move::Skip(i) => {
                k.set(i, StepState::Skipped, at);
                at_once = true;
            }
            Move::Count(done, total) => {
                let i = k.now()?;
                let step = &mut k.job.steps[i];
                (step.done, step.total) = (Some(done), total);
            }
            Move::Arg(key, value) => {
                let i = k.now()?;
                k.job.steps[i].args.insert(key, value);
            }
            Move::Steps(steps) => {
                for (i, new) in steps.into_iter().enumerate() {
                    if i >= k.job.steps.len() {
                        k.job.steps.push(step(&new.key, &[], None));
                        k.times.push((None, None));
                    }
                    at_once |= k.job.steps[i].state != new.state;
                    k.set(i, new.state, at);
                    let old = &mut k.job.steps[i];
                    (old.key, old.done, old.total, old.unit) =
                        (new.key, new.done, new.total, new.unit);
                }
            }
            Move::On(on) => {
                at_once = k.job.on != on;
                k.job.on = on;
            }
            Move::End(result) => {
                if let Some(i) = k.now() {
                    let state = if result.is_ok() { StepState::Done } else { StepState::Failed };
                    k.set(i, state, at);
                }
                let failed = result.is_err();
                k.keep = failed && k.job.kind == "eval_open";
                k.job.end = Some(JobEnd { ok: !failed, why: result.err() });
                k.ended = Some(at);
                // A short wait that ended well shows nothing new; a failure
                // is always said.
                if k.sent.is_none() && !failed && at.saturating_duration_since(k.began) < QUIET {
                    return None;
                }
                return Some(sent(k, at));
            }
        }
        // Told again only when something a client shows moved.
        if (&k.job.steps, &k.job.on) == (&before.0, &before.1) {
            return None;
        }
        k.dirty = true;
        if at.saturating_duration_since(k.began) < QUIET {
            return None;
        }
        let due = k.sent.is_none_or(|s| at.saturating_duration_since(s) >= COUNTS);
        (at_once || due).then(|| sent(k, at))
    }

    /// The jobs that have waited long enough to be sent, or whose counts
    /// held back are due; and the ended jobs past [`KEPT`] let go.
    pub(super) fn due(&mut self, at: Instant) -> Vec<Send> {
        self.kept.retain(|_, k| {
            k.keep || k.ended.is_none_or(|e| at.saturating_duration_since(e) < KEPT)
        });
        let mut out = Vec::new();
        for k in self.kept.values_mut() {
            let ran = at.saturating_duration_since(k.began) >= QUIET;
            let counts = k.sent.is_none_or(|s| at.saturating_duration_since(s) >= COUNTS);
            if k.ended.is_none() && k.dirty && ran && counts {
                out.push(sent(k, at));
            }
        }
        out
    }

    /// What a client opening `project` is given: its jobs and the
    /// machine's that were sent, ended in the last few seconds among them,
    /// or that have run long enough to be.
    pub(super) fn of(&self, project: usize, at: Instant) -> Vec<Job> {
        self.kept
            .values()
            .filter(|k| k.project.is_none_or(|p| p == project))
            .filter(|k| {
                k.sent.is_some()
                    || (k.ended.is_none() && at.saturating_duration_since(k.began) >= QUIET)
            })
            .map(|k| k.shown(at))
            .collect()
    }

    /// A work's Eval was pressed again: an earlier one that could not open
    /// is said no more.
    pub(super) fn forget_failed_open(&mut self, project: usize, work: &str) {
        self.kept.retain(|_, k| {
            !(k.keep && k.project == Some(project) && k.job.on.work.as_deref() == Some(work))
        });
    }

    /// The jobs of a project that closed go with it.
    pub(super) fn forget_project(&mut self, project: usize) {
        self.kept.retain(|_, k| k.project != Some(project));
    }
}

fn sent(k: &mut Kept, at: Instant) -> Send {
    (k.sent, k.dirty) = (Some(at), false);
    Send { job: k.shown(at), project: k.project, call: k.call }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn begun(board: &mut Board, at: Instant) {
        let steps = vec![step("step.a", &[], Some("files")), step("step.b", &[], None)];
        let begin = Move::Begin {
            kind: "seal".into(),
            on: JobOn::default(),
            project: Some(0),
            call: Some((7, 3)),
            steps,
        };
        assert!(board.apply("job_1", begin, at).is_none());
    }

    #[test]
    fn a_job_is_sent_only_once_it_has_run_long_enough() {
        let (mut board, t0) = (Board::default(), Instant::now());
        begun(&mut board, t0);
        assert!(board.apply("job_1", Move::Now(0), t0).is_none(), "too soon");
        assert!(board.due(t0 + Duration::from_millis(100)).is_empty());
        let sent = board.due(t0 + QUIET);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].job.steps[0].state, StepState::Now);
        assert_eq!(sent[0].call, Some((7, 3)));
        assert!(board.due(t0 + QUIET + Duration::from_millis(10)).is_empty(), "nothing moved");
    }

    #[test]
    fn a_short_job_that_ends_well_is_never_sent_and_a_failure_always_is() {
        let (mut board, t0) = (Board::default(), Instant::now());
        begun(&mut board, t0);
        assert!(board.apply("job_1", Move::End(Ok(())), t0).is_none());
        assert!(board.due(t0 + QUIET).is_empty());
        assert!(board.of(0, t0 + QUIET).is_empty(), "nor given to a window opening");
        let mut board = Board::default();
        begun(&mut board, t0);
        let failed = board.apply("job_1", Move::End(Err("no".into())), t0).unwrap();
        assert_eq!(failed.job.end, Some(JobEnd { ok: false, why: Some("no".into()) }));
    }

    #[test]
    fn counts_are_held_back_but_a_step_change_is_sent_at_once() {
        let (mut board, t0) = (Board::default(), Instant::now());
        begun(&mut board, t0);
        board.apply("job_1", Move::Now(0), t0);
        let t = t0 + QUIET;
        assert!(board.apply("job_1", Move::Count(1, Some(10)), t).is_some());
        let soon = t + Duration::from_millis(50);
        assert!(board.apply("job_1", Move::Count(2, Some(10)), soon).is_none(), "held back");
        let changed = board.apply("job_1", Move::Now(1), soon).expect("a step change");
        assert_eq!(changed.job.steps[0].state, StepState::Done);
        assert_eq!(changed.job.share, Some(0.5));
        let ended = board.apply("job_1", Move::End(Ok(())), soon).unwrap();
        assert_eq!(ended.job.share, Some(1.0));
        assert_eq!(ended.job.steps[1].state, StepState::Done);
        assert_eq!(board.of(0, soon + KEPT - Duration::from_millis(1)).len(), 1);
        board.due(soon + KEPT);
        assert!(board.of(0, soon + KEPT).is_empty(), "let go after a few seconds");
    }

    #[test]
    fn a_count_fills_the_share_of_its_step() {
        let (mut board, t0) = (Board::default(), Instant::now());
        begun(&mut board, t0);
        board.apply("job_1", Move::Now(0), t0);
        let sent = board.apply("job_1", Move::Count(5, Some(10)), t0 + QUIET).unwrap();
        assert_eq!(sent.job.share, Some(0.25));
        assert_eq!(sent.job.steps[0].done, Some(5));
        assert!(sent.job.steps[0].ms.is_some_and(|ms| ms >= 400));
    }

    #[test]
    fn an_eval_that_could_not_open_is_kept_until_pressed_again() {
        let (mut board, t0) = (Board::default(), Instant::now());
        let on = JobOn { work: Some("ask_1".into()), ..JobOn::default() };
        let steps = vec![step("step.reading_change", &[], Some("files"))];
        let begin =
            Move::Begin { kind: "eval_open".into(), on, project: Some(0), call: None, steps };
        board.apply("job_1", begin, t0);
        board.apply("job_1", Move::End(Err("git exited 128".into())), t0);
        board.due(t0 + KEPT * 10);
        assert_eq!(board.of(0, t0 + KEPT * 10).len(), 1);
        board.forget_failed_open(0, "ask_1");
        assert!(board.of(0, t0 + KEPT * 10).is_empty());
    }
}
