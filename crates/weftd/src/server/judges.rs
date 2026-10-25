//! An Eval's judges: harness sessions Weft starts in background panes, in the
//! project's folder, each handed one task at a time until the Eval settles.
//! RingFrame is asked off the run loop, one step at a time; the run loop
//! starts the panes, types into them and closes them.

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
const ROLES: [&str; 4] = ["requirements", "map", "reduce", "confirm"];

/// The model and effort a judge session was started on.
type Launch = (Option<String>, Option<String>);

/// One Eval's judges.
pub(super) struct Bench {
    pub(super) eval_id: String,
    harness: String,
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
}

/// One judge pane.
struct Seat {
    pane: u32,
    launch: Launch,
    /// It ran a map: a confirm, the second reading, never goes to it.
    ran_map: bool,
    /// Its process exited.
    gone: bool,
    held: Option<Held>,
}

/// The task a judge holds.
struct Held {
    task: api::Task,
    since: Instant,
    /// How many times its line was typed into this session.
    typed: usize,
    /// Its line is to be typed once the judge is ready.
    to_type: bool,
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
    ran_map: bool,
    gone: bool,
    /// Its task, whether its turn is over or its lease has passed, and how
    /// often it was typed.
    held: Option<(String, bool, bool, usize)>,
}

/// Everything a step reads, taken from the bench so it runs off the run loop.
struct Look {
    root: PathBuf,
    eval_id: String,
    judges: usize,
    seats: Vec<SeatLook>,
    seen: HashMap<String, Launch>,
}

impl Bench {
    pub(super) fn new(eval_id: String, stage: &weft_core::eval_stages::Stage, client: u64) -> Self {
        Bench {
            eval_id,
            harness: stage.harness.clone(),
            judges: stage.judges,
            client,
            seats: Vec::new(),
            seen: HashMap::new(),
            stepping: false,
            again: false,
            looked: Instant::now(),
        }
    }

    fn look(&self, root: &Path) -> Look {
        let seats =
            self.seats
                .iter()
                .map(|s| SeatLook {
                    pane: s.pane,
                    launch: s.launch.clone(),
                    ran_map: s.ran_map,
                    gone: s.gone,
                    held: s.held.as_ref().map(|h| {
                        (h.task.task_id.clone(), h.over, h.since.elapsed() >= LEASE, h.typed)
                    }),
                })
                .collect();
        Look {
            root: root.to_path_buf(),
            eval_id: self.eval_id.clone(),
            judges: self.judges,
            seats,
            seen: self.seen.clone(),
        }
    }

    fn seat(&mut self, pane: u32) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.pane == pane)
    }
}

fn launch_of(task: &api::Task) -> Launch {
    (task.model.clone(), task.effort.clone())
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
    use crate::ringframe::{eval_return, eval_task, eval_task_in};
    let root = &look.root;
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
    // An idle judge takes a task of a role started on what it was started on.
    let mut still_idle = Vec::new();
    'seats: for s in idle {
        for role in ROLES {
            let fits = look.seen.get(role) == Some(&s.launch);
            if !fits || (role == "confirm" && s.ran_map) {
                continue;
            }
            match eval_task(root, &look.eval_id, Some(role))? {
                api::EvalStep::Task(task) => {
                    did.push(Did::Give { pane: s.pane, task });
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
        match eval_task(root, &look.eval_id, None)? {
            api::EvalStep::Task(task) => {
                if open >= look.judges {
                    let pane = still_idle.pop().expect("an idle judge");
                    did.push(Did::Close { pane });
                } else {
                    open += 1;
                }
                did.push(Did::Start { task });
            }
            api::EvalStep::Waiting { .. } => break,
            api::EvalStep::Settled { .. } => return Ok(vec![Did::Done]),
        }
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
    ) {
        let benches = &mut self.projects[at].benches;
        if benches.iter().any(|b| b.eval_id == eval_id) {
            return;
        }
        benches.push(Bench::new(eval_id.to_string(), stage, client));
        self.step_judges(at, eval_id);
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
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let did = step(&look);
            let _ = tx.send(Wake::Judged { project: at, eval_id: look.eval_id, did });
        });
    }

    /// Do what a step decided.
    pub(super) fn judged(&mut self, at: usize, eval_id: &str, did: Vec<Did>) {
        let Some(i) = self.projects[at].benches.iter().position(|b| b.eval_id == eval_id) else {
            return;
        };
        self.projects[at].benches[i].stepping = false;
        for d in did {
            let bench = &mut self.projects[at].benches[i];
            match d {
                Did::Give { pane, task } => {
                    bench.seen.insert(task.role.clone(), launch_of(&task));
                    if let Some(seat) = bench.seat(pane) {
                        seat.ran_map |= task.kind == "map";
                        seat.held = Some(Held::new(task));
                    }
                }
                Did::Again { pane } => {
                    if let Some(h) = bench.seat(pane).and_then(|s| s.held.as_mut()) {
                        (h.to_type, h.over) = (true, false);
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
    }

    /// Start a judge pane for a task, on the model and effort RingFrame
    /// handed it out with. One that does not start holds the task as gone, so
    /// the next step hands it back.
    fn start_judge(&mut self, at: usize, i: usize, task: api::Task) {
        let bench = &mut self.projects[at].benches[i];
        let launch = launch_of(&task);
        bench.seen.insert(task.role.clone(), launch.clone());
        let (harness, client, eval_id) =
            (bench.harness.clone(), bench.client, bench.eval_id.clone());
        let spec = self.projects[at]
            .harnesses
            .find(&harness)
            .map(|h| h.judge_spec(launch.0.as_deref(), launch.1.as_deref()));
        let pane = spec.and_then(|spec| self.spawn(client, at, &harness, &spec, None));
        let ran_map = task.kind == "map";
        let seat = match pane {
            Some(pane) => {
                if let Some(slot) = self.projects[at].slot(pane) {
                    slot.judge = Some(eval_id);
                }
                Seat { pane, launch, ran_map, gone: false, held: Some(Held::new(task)) }
            }
            // No pane: a seat for no pane, so the task is handed back.
            None => {
                Seat { pane: u32::MAX, launch, ran_map, gone: true, held: Some(Held::new(task)) }
            }
        };
        let bench = &mut self.projects[at].benches[i];
        bench.seats.push(seat);
        if pane.is_none() {
            bench.again = true;
        }
    }

    /// Type each held task's line into its judge once the judge is ready and
    /// its screen has gone still.
    pub(super) fn type_judges(&mut self, at: usize) {
        let mut seats = self.projects[at].benches.iter().flat_map(|b| &b.seats);
        if !seats.any(|s| s.held.as_ref().is_some_and(|h| h.to_type)) {
            return;
        }
        let (states, _) = self.agents_of(at);
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
                let still = slot.pane.gone_quiet(&mut h.screen, typing::STARTED_QUIET)
                    || h.screen.0.elapsed() >= typing::STARTED_QUIET;
                if !ready.get(&seat.pane).copied().unwrap_or(false) || !still {
                    continue;
                }
                let job = typing::Job {
                    typist: slot.pane.typist(),
                    payload: h.task.prompt.clone().into_bytes(),
                    how: weft_core::inject::Handoff::Whole,
                    project: at,
                    pane: seat.pane,
                    sends: None,
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

/// The sessions judges ran in this project.
pub(super) fn sessions(root: &Path) -> std::collections::HashSet<String> {
    let text = std::fs::read_to_string(judges_file(root)).unwrap_or_default();
    let read: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let listed = read["sessions"].as_array().into_iter().flatten();
    listed.filter_map(|s| s.as_str()).map(str::to_string).collect()
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
        let mut listed: Vec<&String> = p.judged.iter().collect();
        listed.sort();
        let mut text = serde_json::to_string_pretty(&serde_json::json!({"sessions": listed}))
            .unwrap_or_default();
        text.push('\n');
        // `write_map` made the folder, with its `.gitignore`.
        write_whole(&judges_file(&p.root), text);
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
