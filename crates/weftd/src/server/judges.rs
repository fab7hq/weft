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

/// How long a judge whose harness has reported nothing yet must hold its
/// screen still before it is fed: a harness may report a session only from
/// its first prompt.
const SILENT_QUIET: Duration = Duration::from_secs(3);

/// The roles whose tasks an idle judge asks for, in the order they run.
const ROLES: [&str; 4] = ["requirements", "map", "reduce", "confirm"];

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
}

/// One judge pane.
struct Seat {
    pane: u32,
    launch: Launch,
    /// It ran a map: a confirm, the second reading, never goes to it.
    ran_map: bool,
    /// The windows and requirements its tasks read: no task that reads one
    /// of them again goes to it.
    read: BTreeSet<String>,
    /// Its process exited.
    gone: bool,
    held: Option<Held>,
}

impl Seat {
    /// A judge started on `launch` for `task`, which it holds.
    fn new(pane: u32, launch: Launch, task: api::Task, gone: bool) -> Self {
        let mut seat =
            Seat { pane, launch, ran_map: false, read: BTreeSet::new(), gone, held: None };
        seat.hold(task);
        seat
    }

    /// Give it a task: what the task reads is read in this session.
    fn hold(&mut self, task: api::Task) {
        self.ran_map |= task.kind == "map";
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

/// Whether a judge may be typed its task: its harness says it is ready and
/// its screen has settled; or its harness has said nothing yet and its screen
/// has held still for [`SILENT_QUIET`]. Weft started the session for the Eval
/// the person said yes to, and it holds no one's work.
fn may_feed(state: Option<crate::turns::Turn>, settled: bool, silent_still: bool) -> bool {
    match state {
        None => silent_still,
        known => settled && crate::turns::may_type(known).is_ok(),
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
            if !fits || (role == "confirm" && s.ran_map) {
                continue;
            }
            match eval_task(root, &look.eval_id, Some(role), &s.launch.0, &s.read)? {
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
        let mut taken = None;
        for (role, host) in takes(&look.hosts) {
            match eval_task(root, &look.eval_id, role, host, &[])? {
                api::EvalStep::Task(task) => {
                    taken = Some(task);
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
    ) {
        let benches = &mut self.projects[at].benches;
        if benches.iter().any(|b| b.eval_id == eval_id) {
            return;
        }
        benches.push(Bench::new(eval_id.to_string(), hosts(stage), stage.judges, client));
        keep(&self.projects[at]);
        self.step_judges(at, eval_id);
    }

    /// The project opened in a Weft started again: the Evals the last one
    /// was judging here get judges again.
    pub(super) fn resume_judges(&mut self, at: usize, client: u64) {
        for (eval_id, hosts) in std::mem::take(&mut self.projects[at].unseated) {
            let stages = self.projects[at].routing.eval_stages.as_ref();
            let judges = weft_core::eval_stages::resolve(stages, None, "").judges;
            let bench =
                Bench { reclaim: true, ..Bench::new(eval_id.clone(), hosts, judges, client) };
            self.projects[at].benches.push(bench);
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
                    bench.seen.insert(task.role.clone(), launch_of(&bench.hosts, &task));
                    if let Some(seat) = bench.seat(pane) {
                        seat.hold(task);
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
        let spec = self.projects[at]
            .harnesses
            .find(&harness)
            .map(|h| h.judge_spec(model.as_deref(), effort.as_deref()));
        let pane = spec.and_then(|spec| self.spawn(client, at, &harness, &spec, None));
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

    /// Type each held task's line into its judge once the judge is ready and
    /// its screen has gone still.
    pub(super) fn type_judges(&mut self, at: usize) {
        let mut seats = self.projects[at].benches.iter().flat_map(|b| &b.seats);
        if !seats.any(|s| s.held.as_ref().is_some_and(|h| h.to_type)) {
            return;
        }
        let (states, _) = self.agents_of(at);
        let states: HashMap<u32, Option<crate::turns::Turn>> = self.projects[at]
            .panes
            .iter()
            .zip(states)
            .map(|(s, state)| (s.map.pane, state))
            .collect();
        let tx = self.tx.clone();
        let p = &mut self.projects[at];
        for bench in &mut p.benches {
            for seat in &mut bench.seats {
                let Some(h) = seat.held.as_mut().filter(|h| h.to_type) else { continue };
                let Some(slot) = p.panes.iter_mut().find(|s| s.map.pane == seat.pane) else {
                    continue;
                };
                let settled = slot.pane.gone_quiet(&mut h.screen, typing::STARTED_QUIET)
                    || h.screen.0.elapsed() >= typing::STARTED_QUIET;
                let silent_still = slot.pane.gone_quiet(&mut h.screen, SILENT_QUIET);
                let Some(&state) = states.get(&seat.pane) else { continue };
                if !may_feed(state, settled, silent_still) {
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

    /// A judge is fed when its harness says it is ready and its screen has
    /// settled; one whose harness has said nothing yet, as a harness that
    /// reports a session only from its first prompt, once its screen has held
    /// still for longer. A judge at work or asking is never fed.
    #[test]
    fn a_judge_is_fed_once_ready_or_still_and_silent() {
        use weft_core::turns::Turn;
        assert!(may_feed(Some(Turn::Ready), true, false));
        assert!(may_feed(Some(Turn::TurnEnded), true, false));
        assert!(!may_feed(Some(Turn::Ready), false, false));
        assert!(may_feed(None, false, true));
        assert!(!may_feed(None, true, false), "silent and only briefly still");
        assert!(!may_feed(Some(Turn::Working), true, true));
        assert!(!may_feed(Some(Turn::Waiting), true, true));
    }

    /// A judge's answer, written where its task says: the map calls the
    /// telemetry window unexplained and the rest serve the first requirement,
    /// and its second reading says the telemetry serves it too; the reduce
    /// finds nothing met, its confirm all it can cite met. Each second
    /// reading contradicts the first, so each is read a third time.
    fn answer(task: &api::Task) {
        let file = task.brief.to_string_lossy().replace(".brief.md", ".json");
        let file: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        let text = std::fs::read_to_string(file["windows_file"].as_str().unwrap()).unwrap();
        let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
        // The first added line of a window, as a quote.
        let quote = |w: &str| -> Option<String> {
            let head = format!("=== {w} ");
            let mut lines = text.lines().skip_while(|l| !l.starts_with(&head)).skip(1);
            let line = lines.find(|l| l.starts_with('+') && l.len() > 12)?;
            Some(line[1..].chars().take(24).collect())
        };
        let local = task.task_id.rsplit('~').next().unwrap();
        let doc = match task.kind.as_str() {
            "requirements" => json!({"requirements": list("asks").iter().map(|a| {
                json!({"ask": a["id"], "kind": "local", "text": "the uptime export",
                       "quote": a["text"].as_str().unwrap().trim()})
            }).collect::<Vec<_>>()}),
            "map" => {
                let first = list("requirements")[0]["id"].clone();
                json!({"windows": list("windows").iter().map(|w| {
                    let id = w["id"].as_str().unwrap();
                    if w["path"] == "src/telemetry.js" && !local.starts_with("cw") {
                        json!({"window": id, "unexplained": format!("posts outside ({local})"),
                               "quote": quote(id).unwrap()})
                    } else {
                        json!({"window": id, "serves": [first]})
                    }
                }).collect::<Vec<_>>()})
            }
            _ => json!({"requirements": list("requirements").iter().map(|r| {
                let id = r["id"].as_str().unwrap();
                let cited = r["windows"].as_array().into_iter().flatten()
                    .filter_map(|w| Some((w.as_str()?, quote(w.as_str()?)?))).next();
                match cited {
                    Some((w, q)) if !local.starts_with('r') => json!({"id": id, "vote": "met",
                        "reason": format!("{id} is shown ({local})"),
                        "citations": [{"window": w, "quote": q}]}),
                    _ => json!({"id": id, "vote": "not_met", "reason": format!("{id} is absent ({local})"),
                                "missing": format!("a change doing {id} ({local})")}),
                }
            }).collect::<Vec<_>>()}),
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

    /// Judges stepped as Weft steps them, each answering its task at once:
    /// no idle session is given a reading of a window or a requirement it
    /// has read, so no second or third reading comes from an earlier reader.
    /// Each role runs in its own harness, and the record names it.
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
        let eval_id = eval_open(&repo, &ask, None, None).unwrap();
        let ws = ringframe::api::workspace(&repo).unwrap();
        let debate = json!({"debate": {"harness": "sh-a", "reduce": {"harness": "sh-b"},
                                       "confirm": {"harness": "sh-b"}}});
        let stage = weft_core::eval_stages::resolve(Some(&debate), None, "sh");
        let mut bench = Bench::new(eval_id, hosts(&stage), 2, 0);
        let host_of = |role: &str| if role == "map" { "sh-a" } else { "sh-b" };
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
                        assert!(task.role != "confirm" || !seat.ran_map, "{}", task.task_id);
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
                for stage in ["cr1", "ct1", "cw1", "cwt1"] {
                    assert!(locals.contains(&stage), "{stage} ran: {locals:?}");
                }
                // The record names the harness every judge ran in.
                let record = api::eval_record(&ws, &bench.eval_id).unwrap().unwrap();
                let judges = record["judges"].as_array().unwrap();
                let roles: Vec<&str> = judges.iter().filter_map(|j| j["angle"].as_str()).collect();
                assert!(roles.contains(&"map") && roles.contains(&"confirm"), "{judges:?}");
                for j in judges {
                    assert_eq!(j["host"], host_of(j["angle"].as_str().unwrap()), "{judges:?}");
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
