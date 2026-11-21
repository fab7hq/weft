//! Typing into an agent on the person's yes: at once into the person's own
//! agent, once it has drawn into one Weft starts for it, and proven by what
//! the agent reports it received.

use super::*;

/// How long an agent Weft started must be still before it is typed into.
pub(super) const STARTED_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

/// A fresh agent can take this long to say it is ready: the slowest harness
/// measured said so 14.6 s after its start, read by Sense once its screen held
/// still.
pub(super) const STARTED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether a prompt may be typed into an agent Weft started: at once when it
/// says it is ready and its screen has settled (an agent whose hooks say it is
/// ready may have drawn nothing); otherwise once its screen has drawn
/// something and held still, or once the start wait is over. Keys typed
/// before a harness has drawn its input are lost; a state read wrong never
/// holds a prompt.
pub(super) fn may_send(
    ready: bool,
    settled: bool,
    still: bool,
    waited: std::time::Duration,
) -> bool {
    (ready && settled) || still || waited >= STARTED_TIMEOUT
}

/// A send waiting for the agent Weft started for it, until it may be typed or
/// the start wait is over.
pub(super) struct Starting {
    w: Waiting,
    since: std::time::Instant,
    /// The screen when it last changed: only whether it changes is read.
    screen: (std::time::Instant, String),
}

/// How long after Enter a pane that reports nothing at all, neither the
/// prompt it received nor its turn's end, is waited for before Weft says it
/// cannot tell whether the Ask arrived (amended 2026-10-10).
pub(super) const RECEIPT_CAP: std::time::Duration = std::time::Duration::from_secs(600);

/// A send of an Ask, typed and submitted, waiting for what its pane reports
/// it received. Only that report, judged by RingFrame, makes it delivered.
pub(super) struct Due {
    pane: u32,
    ask: String,
    until: std::time::Instant,
    job: Option<jobs::Handle>,
}

/// One send for a pane's worker to type.
pub(super) struct Job {
    pub(super) typist: crate::pane::Typist,
    pub(super) payload: Vec<u8>,
    pub(super) how: weft_core::inject::Handoff,
    pub(super) project: usize,
    pub(super) pane: u32,
    pub(super) sends: Option<String>,
    /// The send it is, told as it goes.
    pub(super) job: Option<jobs::Handle>,
}

/// A pane's worker: it types each send in turn and says how each went.
/// Typing waits on the agent (a paste landing, a mode entered), seconds at
/// worst, so it never runs on the run loop. It ends with its pane.
pub(super) fn typer(tx: Sender<Wake>) -> Sender<Job> {
    let (jobs, rx) = channel::<Job>();
    std::thread::spawn(move || {
        for j in rx {
            let typed = j.typist.inject_as(&j.payload, &j.how);
            let (project, pane, sends, job) = (j.project, j.pane, j.sends, j.job);
            let done = Wake::Typed { project, pane, typed, sends, job };
            if tx.send(done).is_err() {
                return;
            }
        }
    });
    jobs
}

/// The one pane of `waiting` that `fresh` says yes to; none with none or more.
fn only_one(waiting: &[u32], fresh: &dyn Fn(u32) -> bool) -> Option<u32> {
    let mut found = waiting.iter().copied().filter(|p| fresh(*p));
    let one = found.next()?;
    found.next().is_none().then_some(one)
}

/// Whether RingFrame's answer to a received prompt says it is this Ask's
/// submission: the one comparison is RingFrame's (`observe prompt`), with
/// its allowances; Weft compares no bytes.
fn is_the_ask(answer: &serde_json::Value, ask: &str) -> bool {
    answer["submission"]["ask_id"].as_str() == Some(ask)
}

/// Where a send goes instead of the agent it was staged for.
pub(super) enum Into {
    /// Another agent here, by pane.
    Pane(u32),
    /// A new agent of this harness, started once the person says yes.
    Start(String),
}

/// A send's other agent, and the prompt as that agent's harness takes it.
pub(super) struct Elsewhere {
    pub(super) to: Into,
    pub(super) built: crate::acts::Built,
}

impl Session {
    /// Check a send can go to another agent, or a new one, and build the
    /// prompt that harness takes, before anything is started or typed.
    pub(super) fn send_elsewhere(
        &mut self,
        at: usize,
        waiting: usize,
        to: Into,
    ) -> Result<Elsewhere, (&'static str, String)> {
        let Some(ask) = self.projects[at].waiting[waiting].sends.clone() else {
            return Err(("not_a_send", "only an Ask's prompt can go to another agent".into()));
        };
        let p = &mut self.projects[at];
        let harness = match &to {
            Into::Pane(pane) => match p.slot(*pane) {
                Some(slot) => {
                    if !slot.pane.running() {
                        return Err(("no_pane", format!("agent {pane} is not running")));
                    }
                    slot.map.harness.clone()
                }
                None => return Err(("no_pane", format!("there is no agent {pane} here"))),
            },
            Into::Start(harness) if p.harnesses.find(harness).is_some() => harness.clone(),
            Into::Start(harness) => {
                return Err(("no_harness", format!("{harness} is not set up here")));
            }
        };
        if let Some(why) = self.cannot_prove_in(at, &harness) {
            return Err(("no_receipt", why));
        }
        let units = self.projects[at].ledger.units();
        let Some(unit) = units.iter().find(|u| u.ask_id == ask) else {
            return Err(("no_unit", "that Ask is not on the board".into()));
        };
        let root = self.projects[at].root.clone();
        match crate::acts::send(&root, unit, &harness) {
            Ok(built) => Ok(Elsewhere { to, built }),
            Err(why) => Err(("refused", why)),
        }
    }

    /// Type a prompt that was said yes to. Into the person's own agent it
    /// goes at once, whatever state the agent reads as: when to send is the
    /// person's, and a harness at work queues what is typed into it. Into an
    /// agent Weft has just `started` for it, it waits on the tick until that
    /// agent has drawn: see [`may_send`].
    pub(super) fn type_it(&mut self, project: usize, w: Waiting, started: bool) {
        if !started {
            self.type_now(project, w);
            return;
        }
        if let Some(job) = &w.job {
            job.to("step.waiting_ready");
        }
        let now = std::time::Instant::now();
        let screen = (now, String::new());
        self.projects[project].starting.push(Starting { w, since: now, screen });
        self.type_what_started(project);
    }

    /// Type each send [`may_send`] lets through now.
    pub(super) fn type_what_started(&mut self, project: usize) {
        let waiting = std::mem::take(&mut self.projects[project].starting);
        for mut s in waiting {
            let ready = crate::turns::may_type(self.pane_state(project, s.w.pane)).is_ok();
            // The words on it are not read, only whether it has gone still.
            let (still, settled) = match self.projects[project].slot(s.w.pane) {
                Some(slot) => {
                    let still = slot.pane.gone_quiet(&mut s.screen, STARTED_QUIET);
                    (still, still || s.screen.0.elapsed() >= STARTED_QUIET)
                }
                None => (true, true),
            };
            if may_send(ready, settled, still, s.since.elapsed()) {
                self.type_now(project, s.w);
            } else {
                self.projects[project].starting.push(s);
            }
        }
    }

    pub(super) fn type_now(&mut self, project: usize, w: Waiting) {
        let tx = self.tx.clone();
        let refusal = match self.projects[project].slot(w.pane) {
            Some(slot) => {
                if let Some(job) = &w.job {
                    job.to("step.typing_prompt");
                    job.to("step.typing_request");
                }
                let job = Job {
                    typist: slot.pane.typist(),
                    payload: w.payload,
                    how: w.how,
                    project,
                    pane: w.pane,
                    sends: w.sends.clone(),
                    job: w.job.clone(),
                };
                match slot.typer.get_or_insert_with(|| typer(tx)).send(job) {
                    Ok(()) => None,
                    Err(_) => Some("NoProcess".to_string()),
                }
            }
            None => Some("NoProcess".to_string()),
        };
        match (refusal, w.sends) {
            // Typed and submitted is not delivered: what the pane reports it
            // received says whether this was the Ask. A report that comes
            // while the worker is still typing answers it too.
            (None, Some(ask)) => {
                let until = std::time::Instant::now() + RECEIPT_CAP;
                let job = w.job;
                self.projects[project].due.push(Due { pane: w.pane, ask, until, job });
            }
            (None, None) => {}
            (refusal, _) => {
                if let (Some(job), Some(why)) = (&w.job, &refusal) {
                    job.end(Err(why.clone()));
                }
                self.clients
                    .broadcast(project, &Out::Injected { pane: w.pane, refusal, unrecorded: None })
            }
        }
    }

    /// A message the person wrote, typed by the pane's worker as Weft types
    /// a send: pasted, and Enter only once the pane has taken the paste. Written
    /// with its Enter in one go, or Enter a fixed moment after, a harness
    /// still taking the text reads that Enter as a new line, and the message
    /// sits unsent. A message that opens with a command has it typed, so the
    /// harness reads it as one.
    pub(super) fn say(&mut self, project: usize, pane: u32, text: String) -> Answer {
        let how = match weft_core::inject::leading_command(text.as_bytes()) {
            Some(command) => {
                let start = text.len() - text.trim_start().len() + command.len();
                let at = start + usize::from(text.as_bytes().get(start) == Some(&b' '));
                weft_core::inject::Handoff::TypeCommand { command, at }
            }
            None => weft_core::inject::Handoff::Whole,
        };
        let tx = self.tx.clone();
        let Some(slot) = self.projects[project].slot(pane) else {
            return Answer::No("no_pane", "there is no such pane here".into());
        };
        let job = Job {
            typist: slot.pane.typist(),
            payload: text.into_bytes(),
            how,
            project,
            pane,
            sends: None,
            job: None,
        };
        match slot.typer.get_or_insert_with(|| typer(tx)).send(job) {
            Ok(()) => Answer::Ok(serde_json::json!({})),
            Err(_) => Answer::No("not_running", format!("agent {pane} is not running")),
        }
    }

    /// A pane's worker finished typing a send. One it could not type waits
    /// for no report.
    pub(super) fn typed(
        &mut self,
        project: usize,
        pane: u32,
        typed: Result<crate::pane::Attempt, weft_core::inject::Refusal>,
        sends: Option<String>,
        job: Option<jobs::Handle>,
    ) {
        let refusal = match typed {
            Err(r) => Some(format!("{r:?}")),
            Ok(_) => None,
        };
        // Typed: a send of an Ask waits for its agent to say it got it;
        // anything else is done.
        if let Some(job) = &job {
            match (&refusal, &sends) {
                (Some(why), _) => job.end(Err(why.clone())),
                (None, Some(_)) => job.to("step.waiting_received"),
                (None, None) => job.end(Ok(())),
            }
        }
        // A judge's line is the judges' own, and no one is told of it.
        if self
            .projects
            .get(project)
            .is_some_and(|p| p.panes.iter().any(|s| s.map.pane == pane && s.judge.is_some()))
        {
            return;
        }
        if refusal.is_some()
            && let Some(ask) = sends
            && let Some(p) = self.projects.get_mut(project)
            && let Some(i) = p.due.iter().position(|d| d.pane == pane && d.ask == ask)
        {
            p.due.remove(i);
        }
        self.clients.broadcast(project, &Out::Injected { pane, refusal, unrecorded: None });
    }

    /// When this signal is the next prompt a pane received since Weft sent
    /// it an Ask, what to do with RingFrame's verdict on it.
    pub(super) fn answers_a_send(
        &mut self,
        at: usize,
        sent: &weft_core::signal::Sent,
    ) -> Option<crate::forward::Answered> {
        if sent.forward.as_deref() != Some("prompt") || sent.prompt.is_none() {
            return None;
        }
        // A hook from the harness's shared server names no pane: it answers
        // the send of the one pane of that harness that waits on one and is
        // tied to no session yet, and, if it is the Ask, ties that pane.
        let (pane, tie) = match sent.pane {
            Some(pane) => (pane, None),
            None if sent.shared => {
                let p = &self.projects[at];
                let fresh = |pane: u32| {
                    p.panes.iter().any(|s| {
                        s.map.pane == pane
                            && s.map.harness == sent.harness
                            && s.map.running
                            && s.map.session.is_none()
                    })
                };
                let waiting: Vec<u32> = p.due.iter().map(|d| d.pane).collect();
                (only_one(&waiting, &fresh)?, Some(sent.session.clone()))
            }
            None => return None,
        };
        let due = &mut self.projects[at].due;
        let Due { ask, job, .. } = due.remove(due.iter().position(|d| d.pane == pane)?);
        let tx = self.tx.clone();
        Some(Box::new(move |answer| {
            if let Some(job) = &job {
                let theirs = matches!(&answer, Ok(v) if !is_the_ask(v, &ask));
                job.end(if theirs { Err("NotTheAsk".into()) } else { Ok(()) });
            }
            let _ = tx.send(Wake::Received { project: at, pane, ask, answer, tie });
        }))
    }

    /// RingFrame's verdict on the prompt a pane received after Weft's send.
    /// The Ask's prompt is its observed submission, already on record, and
    /// there is nothing to say. Anything else is not the Ask, and the person
    /// is told; Weft presses no key to stop the agent and never retries.
    pub(super) fn received(
        &mut self,
        project: usize,
        pane: u32,
        ask: &str,
        answer: Result<serde_json::Value, String>,
        tie: Option<String>,
    ) {
        let said = match answer {
            Ok(v) if is_the_ask(&v, ask) => {
                if let Some(session) = tie {
                    self.tie(project, pane, &session);
                    self.tell_agents(project);
                }
                return;
            }
            Ok(_) => Out::Injected { pane, refusal: Some("NotTheAsk".into()), unrecorded: None },
            // RingFrame did not say: the prompt went, unjudged.
            Err(why) => Out::Injected { pane, refusal: None, unrecorded: Some(why) },
        };
        self.clients.broadcast(project, &said);
    }

    /// A pane whose turn ended with no prompt received since Weft's send:
    /// Weft cannot tell whether the Ask arrived. No retry.
    pub(super) fn received_nothing(&mut self, project: usize, pane: u32) {
        let due = &mut self.projects[project].due;
        if let Some(i) = due.iter().position(|d| d.pane == pane) {
            if let Some(job) = due.remove(i).job {
                job.end(Err("NoReceipt".into()));
            }
            let said = Out::Injected { pane, refusal: Some("NoReceipt".into()), unrecorded: None };
            self.clients.broadcast(project, &said);
        }
    }

    /// Sends whose pane reported nothing at all by the cap: Weft cannot tell
    /// whether the Ask arrived. A stall does not prove nothing was sent, so
    /// no retry.
    pub(super) fn nothing_received(&mut self, project: usize) {
        let now = std::time::Instant::now();
        let due = std::mem::take(&mut self.projects[project].due);
        let (late, waiting): (Vec<Due>, Vec<Due>) = due.into_iter().partition(|d| now >= d.until);
        self.projects[project].due = waiting;
        for d in late {
            if let Some(job) = &d.job {
                job.end(Err("NoReceipt".into()));
            }
            let said =
                Out::Injected { pane: d.pane, refusal: Some("NoReceipt".into()), unrecorded: None };
            self.clients.broadcast(project, &said);
        }
    }

    /// Why Weft cannot prove a send to this pane, when it cannot: its
    /// harness file does not say where the prompt it received is.
    pub(super) fn cannot_prove(&mut self, at: usize, pane: u32) -> Option<String> {
        let name = self.projects[at].slot(pane)?.map.harness.clone();
        self.cannot_prove_in(at, &name)
    }

    /// The same, for a harness by name, before any agent of it is started.
    pub(super) fn cannot_prove_in(&self, at: usize, name: &str) -> Option<String> {
        let harness = self.projects[at].harnesses.find(name);
        if harness.and_then(|h| h.signal.as_ref()).is_some_and(|s| s.prompt.is_some()) {
            return None;
        }
        let title = harness.map_or(name, |h| h.title.as_str());
        Some(weft_core::words::fill("send.no_prompt", &[("harness", title)]))
    }

    /// A send said yes to, as a job: recording its Ask when it is one not
    /// yet recorded, starting its agent when it is a new one, waiting for
    /// the agent, typing, and, for an Ask, waiting for the agent to say it
    /// got it.
    pub(super) fn send_job(
        &mut self,
        client: u64,
        call_id: u64,
        at: usize,
        w: &Waiting,
        into: Option<&Elsewhere>,
        pane: Option<u32>,
    ) -> jobs::Handle {
        let start = match into.map(|e| &e.to) {
            Some(Into::Start(harness)) => Some(harness.clone()),
            _ => None,
        };
        let agent = start
            .clone()
            .or_else(|| pane.and_then(|p| self.projects[at].slot(p)).map(|s| s.map.harness.clone()))
            .unwrap_or_default();
        let ask = w.compile.is_some() || w.sends.is_some();
        let units = self.projects[at].ledger.units();
        let work = weft_core::work::work(&units, &self.projects[at].ledger.record());
        let on = weft_proto::JobOn {
            work: w.sends.as_deref().and_then(|a| work.of_ask(a)).map(|u| u.root.clone()),
            ask: w.sends.clone(),
            pane,
            harness: Some(agent.clone()),
            ..Default::default()
        };
        let agent = [("agent", agent.as_str())];
        let steps = vec![
            jobs::step("step.recording_ask", &[], None),
            jobs::step("step.starting", &[("harness", start.as_deref().unwrap_or_default())], None),
            jobs::step("step.waiting_ready", &agent, None),
            jobs::step("step.typing_prompt", &[], None),
            jobs::step("step.waiting_received", &agent, None),
        ];
        let job = self.jobs.begin("send", on, Some(at), Some((client, call_id)), steps);
        if w.compile.is_none() {
            job.skip(0);
        }
        if start.is_none() {
            // The person's own agent is typed into at once.
            job.skip(1);
            job.skip(2);
        }
        if !ask {
            job.skip(4);
        }
        job.now(match (w.compile.is_some(), start.is_some()) {
            (true, _) => 0,
            (false, true) => 1,
            (false, false) => 3,
        });
        job
    }

    /// A Send's Ask is recorded, or would not be: what it types goes now,
    /// into a new agent once it has started.
    pub(super) fn compiled(
        &mut self,
        (client, _): (u64, u64),
        at: usize,
        send: Compiling,
        compiled: Result<Option<String>, String>,
    ) -> Answer {
        let Compiling { mut w, into } = send;
        let job = w.job.clone();
        let fail = |s: &mut Self, code: &'static str, why: String| {
            if let Some(job) = &job {
                s.end_job(job, Err(why.clone()));
            }
            Answer::No(code, why)
        };
        if self.projects[at].closed {
            return fail(self, "no_project", "the project closed".into());
        }
        match compiled {
            Ok(Some(ask_id)) => {
                w.sends = Some(ask_id);
                // Counted off the send: what the person kept teaches Sense.
                let (model, root) = (Arc::clone(&self.sense), self.projects[at].root.clone());
                let weft_dir = self.weft_dir();
                std::thread::spawn(move || crate::memory::after_compile(&model, &weft_dir, &root));
            }
            Ok(None) => {}
            Err(message) => {
                let why = format!("RingFrame would not record it: {message}. Nothing was typed.");
                return fail(self, "not_recorded", why);
            }
        }
        let mut started = false;
        if let Some(into) = into {
            let pane = match into.to {
                Into::Pane(pane) => pane,
                Into::Start(harness) => {
                    if let Some(job) = &job {
                        job.to("step.starting");
                    }
                    started = true;
                    match self.start_for(client, at, &harness) {
                        Some(pane) => pane,
                        None => {
                            return fail(
                                self,
                                "no_pane",
                                format!("Weft could not start {harness}"),
                            );
                        }
                    }
                }
            };
            (w.pane, w.payload, w.how) = (pane, into.built.payload, into.built.how);
        }
        // The Ask it sends and the agent it goes to are known now.
        if let Some(job) = &job {
            let harness = self.projects[at].slot(w.pane).map(|s| s.map.harness.clone());
            let ask = w.sends.clone();
            job.on(weft_proto::JobOn { ask, pane: Some(w.pane), harness, ..Default::default() });
        }
        self.type_it(at, w, started);
        Answer::nothing()
    }

    /// Put a built prompt in front of everyone watching, and answer with its id.
    pub(super) fn stage(&mut self, at: usize, pane: u32, built: crate::acts::Built) -> Answer {
        let id = format!("pnd_{}", self.next_pending);
        self.next_pending += 1;
        let (what, why) = (built.asking.what, built.asking.why.join("\n"));
        let shown = if built.hidden { Vec::new() } else { built.payload.clone() };
        let message = Out::Pending {
            id: id.clone(),
            pane,
            what: what.clone(),
            why: why.clone(),
            payload: shown,
            sends: built.ask_id.clone(),
            go: built.go.clone(),
        };
        self.projects[at].waiting.push(Waiting {
            id: id.clone(),
            pane,
            what,
            why,
            payload: built.payload,
            how: built.how,
            sends: built.ask_id.clone(),
            compile: built.compile,
            eval: built.eval,
            stop: built.stop,
            go: built.go,
            hidden: built.hidden,
            job: None,
            reads: built.reads,
        });
        self.clients.broadcast(at, &message);
        Answer::Ok(serde_json::json!({"pending": id}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A prompt is typed at once into an agent that says it is ready and has
    /// settled; otherwise once its screen is still, or once the start wait is
    /// over for a screen that never holds still, whatever state it reads as.
    #[test]
    fn a_prompt_is_typed_once_ready_still_or_waited_for() {
        let (none, some) = (std::time::Duration::ZERO, std::time::Duration::from_secs(1));
        assert!(may_send(true, true, false, none), "ready and settled");
        assert!(!may_send(true, false, false, some), "ready, still drawing");
        assert!(may_send(false, true, true, none), "read as busy, but still");
        assert!(!may_send(false, true, false, some), "read as busy, blank");
        assert!(may_send(false, false, false, STARTED_TIMEOUT), "waited out");
    }

    #[test]
    fn a_shared_prompt_answers_only_the_one_fresh_pane_waiting() {
        let fresh = |p: u32| p != 9;
        assert_eq!(only_one(&[2], &fresh), Some(2));
        assert_eq!(only_one(&[9, 2], &fresh), Some(2), "a tied pane is not fresh");
        assert_eq!(only_one(&[2, 3], &fresh), None, "two waiting: cannot tell which");
        assert_eq!(only_one(&[], &fresh), None);
    }
}
