//! An Eval's judges: harness sessions Weft starts in background panes, in the
//! project's folder, each handed one task at a time until the Eval settles.
//! RingFrame is asked off the run loop, one step at a time; the run loop
//! starts the panes, types into them and closes them. The Evals being judged
//! are kept beside the judges' sessions, so a Weft started again judges them
//! on.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use ringframe::api;

use super::*;

/// How long a judge has for one task before its pane is closed and the task
/// handed out again: RingFrame's own lease.
pub(super) const LEASE: Duration = Duration::from_secs(15 * 60);

/// How often an Eval with nothing new to say is asked again, for a task that
/// became ready on its own (a check that finished).
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// How many times one session is given a task's line before the task is
/// handed back: RingFrame's own attempts.
const TYPED_MAX: usize = 3;

/// The roles whose tasks an idle judge asks for, in the order they run.
const ROLES: [&str; 2] = weft_core::eval_stages::DEBATE_ROLES;

/// The harness, model and effort a judge session was started on.
type Launch = (String, Option<String>, Option<String>);

/// The harness each role's tasks run in.
pub(super) type Hosts = BTreeMap<String, String>;

/// One Eval's judges.
pub(super) struct Bench {
    pub(super) eval_id: String,
    hosts: Hosts,
    /// The most judges that run at once.
    judges: usize,
    /// Whose window size a judge pane starts at.
    client: u64,
    seats: Vec<Seat>,
    /// What each role's tasks were started on, as RingFrame handed them out.
    seen: HashMap<String, Launch>,
    stepping: bool,
    again: bool,
    looked: Instant,
    /// Seated again after Weft started again: the tasks still out went with
    /// the old judges, and are handed back first.
    reclaim: bool,
    /// The work it judges, by its root Ask.
    work: Option<String>,
    /// Its opening, until every judge it started first has been handed its
    /// task, with the count last told.
    opening: Option<(jobs::Handle, (usize, usize))>,
    /// Its readers at work, told until it settles.
    readers: Option<jobs::Handle>,
}

/// What the readers of an Eval are doing, as its record says: each step,
/// the reading one counted, and the harness of the one under way.
pub(super) type Seen = (Vec<weft_proto::JobStep>, String);

/// One judge pane.
struct Seat {
    pane: u32,
    launch: Launch,
    /// The units and requirements its tasks read: no task that reads one of
    /// them again goes to it, so a re-read is always a fresh session.
    read: BTreeSet<String>,
    /// Its process exited.
    gone: bool,
    held: Option<Held>,
}

impl Seat {
    /// A judge started on `launch` for `task`, which it holds.
    fn new(pane: u32, launch: Launch, task: api::Task, gone: bool) -> Self {
        let mut seat = Seat { pane, launch, read: BTreeSet::new(), gone, held: None };
        seat.hold(task);
        seat
    }

    /// Give it a task: what the task reads is read in this session.
    fn hold(&mut self, task: api::Task) {
        self.read.extend(task.reads.iter().cloned());
        self.held = Some(Held::new(task));
    }
}

/// The task a judge holds.
struct Held {
    task: api::Task,
    since: Instant,
    /// How many times its line was typed into this session.
    typed: usize,
    /// Its line is to be typed once the judge's screen is still.
    to_type: bool,
    /// When its line was last set to be typed.
    asked: Instant,
    /// The turn its line started has ended.
    over: bool,
    /// The screen when it last changed, for typing only once it is still.
    screen: (Instant, String),
}

impl Held {
    fn new(task: api::Task) -> Self {
        let now = Instant::now();
        Held {
            task,
            since: now,
            typed: 0,
            to_type: true,
            asked: now,
            over: false,
            screen: (now, String::new()),
        }
    }
}

/// What a step decided, for the run loop to do.
pub(super) enum Did {
    /// Give an idle judge this task.
    Give { pane: u32, task: api::Task },
    /// Type the task it holds into it again: its answer was refused, or not sent.
    Again { pane: u32 },
    /// Start a judge for this task.
    Start { task: api::Task },
    /// Its task is in: the judge is idle.
    Free { pane: u32 },
    /// Close the judge; any task it held was handed back.
    Close { pane: u32 },
    /// The Eval settled, or RingFrame would hand out nothing more: close every judge.
    Done,
}

/// One judge as a step reads it.
struct SeatLook {
    pane: u32,
    launch: Launch,
    read: Vec<String>,
    gone: bool,
    /// Its task, whether its turn is over or its lease has passed, and how
    /// often it was typed.
    held: Option<(String, bool, bool, usize)>,
}

/// Everything a step reads, taken from the bench so it runs off the run loop.
struct Look {
    root: PathBuf,
    eval_id: String,
    hosts: Hosts,
    judges: usize,
    seats: Vec<SeatLook>,
    seen: HashMap<String, Launch>,
    reclaim: bool,
}

impl Bench {
    pub(super) fn new(eval_id: String, hosts: Hosts, judges: usize, client: u64) -> Self {
        Bench {
            eval_id,
            hosts,
            judges,
            client,
            seats: Vec::new(),
            seen: HashMap::new(),
            stepping: false,
            again: false,
            looked: Instant::now(),
            reclaim: false,
            work: None,
            opening: None,
            readers: None,
        }
    }

    fn look(&self, root: &Path) -> Look {
        let seats =
            self.seats
                .iter()
                .map(|s| SeatLook {
                    pane: s.pane,
                    launch: s.launch.clone(),
                    read: s.read.iter().cloned().collect(),
                    gone: s.gone,
                    held: s.held.as_ref().map(|h| {
                        (h.task.task_id.clone(), h.over, h.since.elapsed() >= LEASE, h.typed)
                    }),
                })
                .collect();
        Look {
            root: root.to_path_buf(),
            eval_id: self.eval_id.clone(),
            hosts: self.hosts.clone(),
            judges: self.judges,
            seats,
            seen: self.seen.clone(),
            reclaim: self.reclaim,
        }
    }

    fn seat(&mut self, pane: u32) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.pane == pane)
    }
}

/// Each role's harness, as the debate resolved it.
pub(super) fn hosts(stage: &weft_core::eval_stages::Stage) -> Hosts {
    ROLES.iter().map(|r| (r.to_string(), stage.harness_of(r).to_string())).collect()
}

fn host<'a>(hosts: &'a Hosts, role: &str) -> &'a str {
    hosts.get(role).map_or("", String::as_str)
}

fn launch_of(hosts: &Hosts, task: &api::Task) -> Launch {
    (host(hosts, &task.role).to_string(), task.model.clone(), task.effort.clone())
}

/// What a step asks RingFrame for when no idle judge took a task: any task,
/// when every role runs in one harness; else each role's in turn, for its
/// own harness, so the record names the harness that runs it.
fn takes(hosts: &Hosts) -> Vec<(Option<&str>, &str)> {
    let mut each = hosts.values();
    match each.next() {
        Some(one) if each.all(|h| h == one) => vec![(None, one.as_str())],
        _ => ROLES.iter().map(|r| (Some(*r), host(hosts, r))).collect(),
    }
}

/// What the readers of an Eval are doing, read from its record: none once it
/// has closed.
fn readers_seen(look: &Look) -> Option<Seen> {
    use weft_core::eval_view::Light;
    use weft_proto::StepState;
    let view = crate::record::eval_view(&look.root, &look.eval_id)?;
    let progress = view.running()?;
    let (read, of) = progress.reads;
    let steps: Vec<weft_proto::JobStep> = progress
        .steps
        .iter()
        .enumerate()
        .map(|(i, lit)| {
            // The whole reading is the step that counts what is read.
            let counted = i == 1 && of > 0;
            weft_proto::JobStep {
                key: lit.key.clone(),
                args: Default::default(),
                state: match lit.light {
                    Light::Done => StepState::Done,
                    Light::Now => StepState::Now,
                    Light::Next => StepState::Next,
                },
                ms: None,
                done: counted.then_some(read as u64),
                total: counted.then_some(of as u64),
                unit: counted.then(|| "reads".to_string()),
            }
        })
        .collect();
    let questions = steps.last().is_some_and(|s| s.state == StepState::Now);
    let role = if questions { "question" } else { "read" };
    Some((steps, host(&look.hosts, role).to_string()))
}

/// One step: settle what each judge finished, give idle judges what they can
/// take, and start judges for what none can, up to the most that run at
/// once. A refusal from RingFrame (the Eval voided) ends the bench.
fn step(look: &Look) -> Vec<Did> {
    match step_or_refused(look) {
        Ok(did) => did,
        Err(crate::ringframe::Error::Refused { message, .. }) => {
            eprintln!("weft: the judges of {} stop: {message}", look.eval_id);
            vec![Did::Done]
        }
    }
}

fn step_or_refused(look: &Look) -> Result<Vec<Did>, crate::ringframe::Error> {
    use crate::ringframe::{eval_return, eval_return_out, eval_task, eval_task_in};
    let root = &look.root;
    if look.reclaim {
        eval_return_out(root, &look.eval_id)?;
    }
    let mut did = Vec::new();
    let mut idle: Vec<&SeatLook> = Vec::new();
    let mut open = 0;
    for s in &look.seats {
        match &s.held {
            Some((task, over, late, typed)) if s.gone || *over || *late => {
                if eval_task_in(root, task)? {
                    if s.gone {
                        did.push(Did::Close { pane: s.pane });
                        continue;
                    }
                    did.push(Did::Free { pane: s.pane });
                    idle.push(s);
                } else if s.gone || *late || *typed >= TYPED_MAX {
                    eval_return(root, task)?;
                    did.push(Did::Close { pane: s.pane });
                    continue;
                } else {
                    did.push(Did::Again { pane: s.pane });
                }
            }
            Some(_) => {}
            None if s.gone => {
                did.push(Did::Close { pane: s.pane });
                continue;
            }
            None => idle.push(s),
        }
        open += 1;
    }
    // An idle judge takes a task of a role started on what it was started on,
    // its harness included, that reads nothing it has read.
    let mut still_idle = Vec::new();
    'seats: for s in idle {
        for role in ROLES {
            let fits = look.seen.get(role) == Some(&s.launch);
            if !fits {
                continue;
            }
            match eval_task(root, &look.eval_id, Some(role), &s.launch.0, &s.read)? {
                api::EvalStep::Task(task) => {
                    did.push(Did::Give { pane: s.pane, task: *task });
                    continue 'seats;
                }
                api::EvalStep::Waiting { .. } => {}
                api::EvalStep::Settled { .. } => return Ok(vec![Did::Done]),
            }
        }
        still_idle.push(s.pane);
    }
    // What no idle judge could take gets a judge of its own: in a free
    // place, or in an idle judge's, which is closed for it.
    while open < look.judges || !still_idle.is_empty() {
        let mut taken = None;
        for (role, host) in takes(&look.hosts) {
            match eval_task(root, &look.eval_id, role, host, &[])? {
                api::EvalStep::Task(task) => {
                    taken = Some(*task);
                    break;
                }
                api::EvalStep::Waiting { .. } => {}
                api::EvalStep::Settled { .. } => return Ok(vec![Did::Done]),
            }
        }
        let Some(task) = taken else { break };
        if open >= look.judges {
            let pane = still_idle.pop().expect("an idle judge");
            did.push(Did::Close { pane });
        } else {
            open += 1;
        }
        did.push(Did::Start { task });
    }
    Ok(did)
}

impl Session {
    /// An Eval opened on the person's yes: its judges start.
    pub(super) fn seat_judges(
        &mut self,
        at: usize,
        client: u64,
        eval_id: &str,
        stage: &weft_core::eval_stages::Stage,
        opening: Option<jobs::Handle>,
    ) {
        if self.projects[at].benches.iter().any(|b| b.eval_id == eval_id) {
            if let Some(job) = opening {
                job.end(Ok(()));
            }
            return;
        }
        let bench = Bench::new(eval_id.to_string(), hosts(stage), stage.judges, client);
        let bench = Bench { opening: opening.map(|job| (job, (0, 0))), ..bench };
        self.bench(at, bench);
        keep(&self.projects[at]);
        self.step_judges(at, eval_id);
    }

    /// An Eval's judges, from now on: its readers at work are told as a job.
    fn bench(&mut self, at: usize, mut bench: Bench) {
        let units = self.projects[at].ledger.units();
        let work = weft_core::work::work(&units, &self.projects[at].ledger.record());
        bench.work = work.of_eval(&bench.eval_id).map(|u| u.root.clone());
        let on = weft_proto::JobOn {
            work: bench.work.clone(),
            eval: Some(bench.eval_id.clone()),
            harness: Some(host(&bench.hosts, "read").to_string()),
            ..Default::default()
        };
        let steps = [
            "eval_view.step.asked",
            "eval_view.step.reading",
            "eval_view.step.again_next",
            "eval_view.step.questions_next",
        ];
        let steps = steps.iter().map(|k| jobs::step(k, &[], None)).collect();
        bench.readers = Some(self.jobs.begin("readers", on, Some(at), None, steps));
        self.projects[at].benches.push(bench);
    }

    /// How many of the judges an Eval's opening started have been handed
    /// their task: once they are started, the opening waits for each to be
    /// ready (its screen drawn and still, at most the start wait) and counts
    /// them as their task is typed. It has opened once they all have, or once
    /// it started none.
    fn tell_judges_started(&mut self, at: usize) {
        for bench in &mut self.projects[at].benches {
            let Some((job, told)) = &mut bench.opening else { continue };
            let total = bench.seats.len();
            // Its first step has not said yet what it starts.
            if total == 0 && bench.stepping {
                continue;
            }
            if total > 0 && told.1 == 0 {
                job.count(total as u64, Some(total as u64));
                job.to("step.waiting_ready");
            }
            let started = bench
                .seats
                .iter()
                .filter(|s| s.gone || s.held.as_ref().is_none_or(|h| h.typed > 0))
                .count();
            if (started, total) != *told {
                job.count(started as u64, Some(total as u64));
                *told = (started, total);
            }
            if started == total {
                job.end(Ok(()));
                bench.opening = None;
            }
        }
    }

    /// The project opened in a Weft started again: the Evals the last one
    /// was judging here get judges again.
    pub(super) fn resume_judges(&mut self, at: usize, client: u64) {
        for (eval_id, hosts) in std::mem::take(&mut self.projects[at].unseated) {
            let stages = self.projects[at].routing.eval_stages.as_ref();
            let judges = weft_core::eval_stages::resolve(stages, None, "").judges;
            let bench =
                Bench { reclaim: true, ..Bench::new(eval_id.clone(), hosts, judges, client) };
            self.bench(at, bench);
            self.step_judges(at, &eval_id);
        }
    }

    /// Ask RingFrame what the judges of this Eval do next, off the run loop;
    /// once more after, when something happened while it was asked.
    fn step_judges(&mut self, at: usize, eval_id: &str) {
        let root = self.projects[at].root.clone();
        let Some(bench) = self.projects[at].benches.iter_mut().find(|b| b.eval_id == eval_id)
        else {
            return;
        };
        if bench.stepping {
            bench.again = true;
            return;
        }
        (bench.stepping, bench.again, bench.looked) = (true, false, Instant::now());
        let look = bench.look(&root);
        bench.reclaim = false;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let did = step(&look);
            let seen = readers_seen(&look);
            let _ = tx.send(Wake::Judged { project: at, eval_id: look.eval_id, did, seen });
        });
    }

    /// Do what a step decided.
    pub(super) fn judged(&mut self, at: usize, eval_id: &str, did: Vec<Did>, seen: Option<Seen>) {
        let Some(i) = self.projects[at].benches.iter().position(|b| b.eval_id == eval_id) else {
            return;
        };
        self.projects[at].benches[i].stepping = false;
        let bench = &self.projects[at].benches[i];
        if let (Some(job), Some((steps, harness))) = (&bench.readers, seen) {
            job.steps(steps);
            job.on(weft_proto::JobOn {
                work: bench.work.clone(),
                eval: Some(bench.eval_id.clone()),
                harness: Some(harness),
                ..Default::default()
            });
        }
        for d in did {
            let bench = &mut self.projects[at].benches[i];
            match d {
                Did::Give { pane, task } => {
                    bench.seen.insert(task.role.clone(), launch_of(&bench.hosts, &task));
                    if let Some(seat) = bench.seat(pane) {
                        seat.hold(task);
                    }
                }
                Did::Again { pane } => {
                    if let Some(h) = bench.seat(pane).and_then(|s| s.held.as_mut()) {
                        (h.to_type, h.over, h.asked) = (true, false, Instant::now());
                    }
                }
                Did::Free { pane } => {
                    if let Some(seat) = bench.seat(pane) {
                        seat.held = None;
                    }
                }
                Did::Close { pane } => {
                    bench.seats.retain(|s| s.pane != pane);
                    self.close(at, pane);
                }
                Did::Start { task } => self.start_judge(at, i, task),
                Did::Done => {
                    let bench = self.projects[at].benches.remove(i);
                    for job in bench.opening.iter().map(|(j, _)| j).chain(&bench.readers) {
                        job.end(Ok(()));
                    }
                    keep(&self.projects[at]);
                    for s in bench.seats {
                        self.close(at, s.pane);
                    }
                    return;
                }
            }
        }
        if self.projects[at].benches[i].again {
            self.step_judges(at, eval_id);
        }
        self.type_judges(at);
        self.tell_judges_started(at);
    }

    /// Start a judge pane for a task, in its role's harness, on the model and
    /// effort RingFrame handed it out with. One that does not start holds the
    /// task as gone, so the next step hands it back.
    fn start_judge(&mut self, at: usize, i: usize, task: api::Task) {
        let bench = &mut self.projects[at].benches[i];
        let launch = launch_of(&bench.hosts, &task);
        bench.seen.insert(task.role.clone(), launch.clone());
        let (client, eval_id) = (bench.client, bench.eval_id.clone());
        let (harness, model, effort) = launch.clone();
        for value in [&model, &effort].into_iter().flatten() {
            if !weft_core::harness::launch_word(value) {
                eprintln!(
                    "weft: a judge of {eval_id} starts without '{value}': not one word its \
                     harness can take, so it runs on {harness}'s own"
                );
            }
        }
        let spec = self.projects[at]
            .harnesses
            .find(&harness)
            .map(|h| h.judge_spec(model.as_deref(), effort.as_deref()));
        // It starts in the task's own empty folder, outside every repository,
        // and reads the product from the copies its brief lists.
        let dir = task.dir.clone().filter(|d| d.is_dir());
        let pane =
            spec.and_then(|spec| self.spawn_in(client, at, &harness, &spec, None, dir.as_deref()));
        let seat = match pane {
            Some(pane) => {
                if let Some(slot) = self.projects[at].slot(pane) {
                    slot.judge = Some(eval_id);
                }
                // Each window lists it under its Eval from now on.
                let panes = self.pane_infos(at);
                self.clients.broadcast(at, &Out::Panes { panes });
                Seat::new(pane, launch, task, false)
            }
            // No pane: a seat for no pane, so the task is handed back.
            None => Seat::new(u32::MAX, launch, task, true),
        };
        let bench = &mut self.projects[at].benches[i];
        bench.seats.push(seat);
        if pane.is_none() {
            bench.again = true;
        }
    }

    /// Type each held task's line into its judge once it is ready, or its
    /// screen has gone still, or the start wait is over: see [`typing::may_send`].
    pub(super) fn type_judges(&mut self, at: usize) {
        let mut seats = self.projects[at].benches.iter().flat_map(|b| &b.seats);
        if !seats.any(|s| s.held.as_ref().is_some_and(|h| h.to_type)) {
            return;
        }
        let states = self.states_of(at);
        let ready: HashMap<u32, bool> = self.projects[at]
            .panes
            .iter()
            .zip(states)
            .map(|(s, state)| (s.map.pane, crate::turns::may_type(state).is_ok()))
            .collect();
        let tx = self.tx.clone();
        let p = &mut self.projects[at];
        for bench in &mut p.benches {
            for seat in &mut bench.seats {
                let Some(h) = seat.held.as_mut().filter(|h| h.to_type) else { continue };
                let Some(slot) = p.panes.iter_mut().find(|s| s.map.pane == seat.pane) else {
                    continue;
                };
                let still = slot.pane.gone_quiet(&mut h.screen, typing::STARTED_QUIET);
                let settled = still || h.screen.0.elapsed() >= typing::STARTED_QUIET;
                let ready = ready.get(&seat.pane).copied().unwrap_or(false);
                if !typing::may_send(ready, settled, still, h.asked.elapsed()) {
                    continue;
                }
                let job = typing::Job {
                    typist: slot.pane.typist(),
                    payload: h.task.prompt.clone().into_bytes(),
                    how: weft_core::inject::Handoff::Whole,
                    project: at,
                    pane: seat.pane,
                    sends: None,
                    job: None,
                };
                if slot.typer.get_or_insert_with(|| typing::typer(tx.clone())).send(job).is_ok() {
                    (h.to_type, h.over, h.typed) = (false, false, h.typed + 1);
                }
            }
        }
    }

    /// A judge's turn ended: the step that follows reads whether its answer is in.
    pub(super) fn judge_turn_ended(&mut self, at: usize, pane: u32) {
        let Some(eval_id) = self.held_by(at, pane, |h| {
            if !h.to_type && h.typed > 0 {
                h.over = true;
            }
        }) else {
            return;
        };
        self.step_judges(at, &eval_id);
    }

    /// A judge's process exited: its task, if it held one, is handed out again.
    pub(super) fn judge_exited(&mut self, at: usize, pane: u32) {
        let found = self.projects[at].benches.iter_mut().find_map(|b| {
            let seat = b.seat(pane)?;
            seat.gone = true;
            Some(b.eval_id.clone())
        });
        if let Some(eval_id) = found {
            self.step_judges(at, &eval_id);
        }
    }

    /// On the tick: a judge past its lease, or an Eval not asked for a while,
    /// is asked about again.
    pub(super) fn look_at_judges(&mut self, at: usize) {
        let due: Vec<String> = self.projects[at]
            .benches
            .iter()
            .filter(|b| {
                let late = b
                    .seats
                    .iter()
                    .any(|s| s.held.as_ref().is_some_and(|h| h.since.elapsed() >= LEASE));
                !b.stepping && (late || b.looked.elapsed() >= LOOK_EVERY)
            })
            .map(|b| b.eval_id.clone())
            .collect();
        for eval_id in due {
            self.step_judges(at, &eval_id);
        }
        self.type_judges(at);
        self.tell_judges_started(at);
    }

    /// The Eval whose judge this pane is, after `f` saw the task it holds.
    fn held_by(&mut self, at: usize, pane: u32, f: impl FnOnce(&mut Held)) -> Option<String> {
        let bench = self.projects[at]
            .benches
            .iter_mut()
            .find(|b| b.seats.iter().any(|s| s.pane == pane))?;
        f(bench.seat(pane)?.held.as_mut()?);
        Some(bench.eval_id.clone())
    }
}

/// Where the sessions judges ran in a project are kept: Weft's own folder.
fn judges_file(root: &Path) -> PathBuf {
    root.join(".fab7").join("weft").join("judges.json")
}

fn judges_read(root: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(judges_file(root)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

/// The sessions judges ran in this project.
pub(super) fn sessions(root: &Path) -> std::collections::HashSet<String> {
    let read = judges_read(root);
    let listed = read["sessions"].as_array().into_iter().flatten();
    listed.filter_map(|s| s.as_str()).map(str::to_string).collect()
}

/// The Evals Weft was judging in this project, each with its roles'
/// harnesses. One kept with a single harness ran every role in it.
pub(super) fn benched(root: &Path) -> Vec<(String, Hosts)> {
    let read = judges_read(root);
    let listed = read["evals"].as_object().into_iter().flatten();
    listed
        .filter_map(|(e, kept)| {
            let hosts = match kept {
                serde_json::Value::String(h) => {
                    ROLES.iter().map(|r| (r.to_string(), h.clone())).collect()
                }
                serde_json::Value::Object(each) => each
                    .iter()
                    .filter_map(|(r, h)| Some((r.clone(), h.as_str()?.to_string())))
                    .collect(),
                _ => return None,
            };
            Some((e.clone(), hosts))
        })
        .collect()
}

/// The Evals being judged, as they are kept: each with its roles' harnesses.
fn evals(benches: &[Bench]) -> serde_json::Value {
    let each = benches.iter().map(|b| (b.eval_id.clone(), serde_json::json!(b.hosts)));
    serde_json::Value::Object(each.collect())
}

/// Keep the sessions judges ran here, and the Evals being judged.
fn keep(p: &Project) {
    let mut sessions: Vec<&String> = p.judged.iter().collect();
    sessions.sort();
    let evals = evals(&p.benches);
    let mut text =
        serde_json::to_string_pretty(&serde_json::json!({"sessions": sessions, "evals": evals}))
            .unwrap_or_default();
    text.push('\n');
    // `write_map` made the folder, with its `.gitignore`.
    write_whole(&judges_file(&p.root), text);
}

impl Session {
    /// A judge pane named its session: it is kept as a judge's, so it is
    /// never shown as the person's agent, here or after a restart.
    pub(super) fn judge_session(&mut self, at: usize, pane: u32, session: &str) {
        let p = &mut self.projects[at];
        let judge = p.panes.iter().any(|s| s.map.pane == pane && s.judge.is_some());
        if !judge || !p.judged.insert(session.to_string()) {
            return;
        }
        keep(p);
    }
}

impl Session {
    /// Put a running Eval's Stop dialog in front of everyone watching. An
    /// Eval that has settled, or was voided, has nothing to stop.
    pub(super) fn stage_stop(&mut self, at: usize, eval: Option<&str>) -> Answer {
        self.projects[at].ledger.refresh();
        let record = self.projects[at].ledger.record();
        let running = eval
            .and_then(|id| record.evals.iter().find(|e| e.id == id && !e.completed() && !e.voided));
        let Some(e) = running else {
            return Answer::No("not_running", "that Eval is not running".into());
        };
        let units = self.projects[at].ledger.units();
        let title = e.asks.first().and_then(|a| units.iter().find(|u| &u.ask_id == a));
        let title = title.map_or(e.id.clone(), |u| u.title.clone());
        let built = crate::acts::stop(&e.id.clone(), &title);
        self.stage(at, NO_PANE, built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ringframe::tests::{fixture_home, folder};
    use crate::ringframe::{AskDraft, compile, eval_open, set_up};
    use serde_json::{Value, json};

    /// A judge's answer, written where its task says: the map calls the
    /// telemetry window unexplained and the rest serve the first requirement,
    /// and its second reading says the telemetry serves it too; the reduce
    /// finds nothing met, its confirm all it can cite met. Each second
    /// reading contradicts the first, so each is read a third time.
    fn answer(task: &api::Task) {
        let file = task.brief.to_string_lossy().replace(".brief.md", ".json");
        let file: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let brief = std::fs::read_to_string(&task.brief).unwrap();
        let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
        // A unit's path, and the first added line of it as a quote.
        let unit = |w: &str| -> (String, Option<String>) {
            let head = format!("=== {w} ");
            let mut lines = brief.lines().skip_while(|l| !l.starts_with(&head));
            let path = lines.next().map_or("", |l| &l[head.len()..]).to_string();
            let line = lines.find(|l| l.starts_with('+') && l.len() > 12);
            (path, line.map(|l| l[1..].chars().take(24).collect()))
        };
        let local = task.task_id.rsplit('~').next().unwrap();
        let items: Vec<String> =
            list("items").iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
        let of = |q: &str| -> Vec<String> {
            items.iter().filter_map(|i| i.strip_prefix(q).map(str::to_string)).collect()
        };
        // The first unit with a line to quote, for a requirement's citation.
        let cited = brief
            .lines()
            .filter_map(|l| l.strip_prefix("=== w_"))
            .filter_map(|l| l.split(' ').next().map(|id| format!("w_{id}")))
            .find_map(|w| unit(&w).1.map(|q| (w, q)));
        let doc = match task.kind.as_str() {
            "question" => crate::ringframe::tests::questions_for(&brief),
            "requirements" => json!({"requirements": list("asks").iter().map(|a| {
                json!({"ask": a["id"], "kind": "local", "text": "the uptime export",
                       "quote": a["text"].as_str().unwrap().trim()})
            }).collect::<Vec<_>>()}),
            _ => {
                let reqs = of("r|");
                // A re-read may ask about no requirement, and still lists them.
                let first = brief
                    .split("## The requirements\n")
                    .nth(1)
                    .and_then(|r| r.lines().find_map(|l| l.strip_prefix("- `")))
                    .and_then(|l| l.split('`').next())
                    .map(str::to_string);
                json!({
                    "windows": of("w|").iter().map(|w| {
                        let (path, quote) = unit(w);
                        // Unexplained at first, then a split: a third reading.
                        if path == "src/telemetry.js" && local != "rr1" {
                            json!({"id": w, "answer": "unexplained", "quote": quote.unwrap(),
                                   "reason": format!("It posts outside ({local}).")})
                        } else {
                            json!({"id": w, "answer": "serves", "serves": [first.clone().unwrap()]})
                        }
                    }).collect::<Vec<_>>(),
                    "requirements": reqs.iter().map(|r| {
                        let (w, q) = cited.clone().unwrap();
                        json!({"id": r, "vote": "met", "reason": format!("{r} is shown ({local})"),
                               "citations": [{"window": w, "quote": q}]})
                    }).collect::<Vec<_>>(),
                })
            }
        };
        std::fs::write(&task.answer, doc.to_string()).unwrap();
    }

    /// An Eval Weft was judging is kept with each role's harness; one kept
    /// by an earlier Weft, with one harness for all, is read as every role's.
    #[test]
    fn an_eval_is_kept_with_each_roles_harness() {
        let root = std::env::temp_dir().join(format!("weft-benched-{}", std::process::id()));
        let file = judges_file(&root);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mixed = json!({"map": "agy", "reduce": "claude-code", "requirements": "claude-code",
                           "confirm": "codex"});
        std::fs::write(&file, json!({"evals": {"e1": "codex", "e2": mixed}}).to_string()).unwrap();
        let mut kept = benched(&root);
        kept.sort();
        let all = |h: &str| -> Hosts { ROLES.iter().map(|r| (r.to_string(), h.into())).collect() };
        assert_eq!(kept[0], ("e1".into(), all("codex")));
        assert_eq!(kept[1].1.get("map").map(String::as_str), Some("agy"));
        assert_eq!(kept[1].1.get("confirm").map(String::as_str), Some("codex"));
        assert_eq!(
            evals(&[Bench::new("e1".into(), all("codex"), 2, 0)]),
            json!({"e1": all("codex")})
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Readers stepped as Weft steps them, each answering its task at once:
    /// no idle session is given a reading of a unit or a requirement it has
    /// read, so every re-read is a fresh session. Each role runs in its own
    /// harness, and the record names it.
    #[test]
    fn no_session_reads_an_item_twice() {
        fixture_home();
        let repo = folder("judges-read", true);
        assert_eq!(set_up(&repo), None);
        let draft = AskDraft {
            words: "Add the uptime export to src/uptime.js with a test.".into(),
            harness: "claude-code".into(),
            route: "native_direct".into(),
            task: "implement".into(),
            effects: vec!["read".into(), "write".into()],
            plan: None,
            rules: Vec::new(),
            link: None,
            sense: None,
            session: None,
        };
        let ask = compile(&repo, &draft, None).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for (path, text) in [
            ("src/uptime.js", "export const uptime = () => process.uptime();\n"),
            ("src/telemetry.js", "export const beacon = () => fetch('https://example.com/c');\n"),
        ] {
            std::fs::write(repo.join(path), text).unwrap();
        }
        for args in [&["add", "src"][..], &["commit", "-q", "-m", "work"]] {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .status()
                .unwrap();
            assert!(ok.success(), "git {args:?}");
        }
        let eval_id = eval_open(&repo, &ask, None, None, Vec::new(), &|_| {}).unwrap();
        let ws = ringframe::api::workspace(&repo).unwrap();
        let debate = json!({"debate": {"harness": "sh-a", "question": {"harness": "sh-b"}}});
        let stage = weft_core::eval_stages::resolve(Some(&debate), None, "sh");
        let mut bench = Bench::new(eval_id, hosts(&stage), 2, 0);
        let host_of = |role: &str| if role == "read" { "sh-a" } else { "sh-b" };
        let mut given: Vec<String> = Vec::new();
        let mut next = 0;
        for _ in 0..60 {
            let mut done = false;
            for d in step(&bench.look(&repo)) {
                match d {
                    Did::Give { pane, task } => {
                        bench.seen.insert(task.role.clone(), launch_of(&bench.hosts, &task));
                        let seat = bench.seat(pane).unwrap();
                        assert_eq!(seat.launch.0, host_of(&task.role), "{}", task.task_id);
                        let again: Vec<&String> =
                            task.reads.iter().filter(|i| seat.read.contains(*i)).collect();
                        assert!(again.is_empty(), "{} read {again:?} again", task.task_id);
                        given.push(task.task_id.clone());
                        answer(&task);
                        seat.hold(task);
                    }
                    Did::Start { task } => {
                        next += 1;
                        let launch = launch_of(&bench.hosts, &task);
                        assert_eq!(launch.0, host_of(&task.role), "{}", task.task_id);
                        bench.seen.insert(task.role.clone(), launch.clone());
                        given.push(task.task_id.clone());
                        answer(&task);
                        bench.seats.push(Seat::new(next, launch, task, false));
                    }
                    Did::Free { pane } => bench.seat(pane).unwrap().held = None,
                    Did::Close { pane } => bench.seats.retain(|s| s.pane != pane),
                    Did::Again { pane } => panic!("pane {pane}'s answer was refused"),
                    Did::Done => done = true,
                }
            }
            if done {
                let locals: Vec<&str> = given.iter().filter_map(|t| t.rsplit('~').next()).collect();
                for stage in ["rd1", "rr1", "rr2"] {
                    assert!(locals.contains(&stage), "{stage} ran: {locals:?}");
                }
                // The record names the harness every reader ran in.
                let record = api::eval_record(&ws, &bench.eval_id).unwrap().unwrap();
                let readers = record["readers"].as_array().unwrap();
                assert!(!readers.is_empty());
                for r in readers {
                    assert_eq!(r["host"], host_of(r["role"].as_str().unwrap()), "{readers:?}");
                }
                let _ = std::fs::remove_dir_all(repo);
                return;
            }
            // Each answer is submitted, and its judge's turn is over.
            for seat in &mut bench.seats {
                if let Some(h) = seat.held.as_mut().filter(|h| !h.over) {
                    let said = api::eval_submit(&ws, &h.task.task_id, &h.task.answer).unwrap();
                    assert_eq!(said.outcome, api::Outcome::Accepted, "{:?}", said.errors);
                    (h.over, h.typed) = (true, 1);
                }
            }
        }
        panic!("the Eval did not settle: {given:?}");
    }
}
