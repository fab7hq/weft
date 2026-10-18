//! Typing into an agent on the person's yes: staged, waited for until the
//! agent is ready, refused with the reason when it is not, and proven by what
//! the agent reports it received.

use super::*;

/// How long an agent must be still before it is typed into, and how long
/// Weft waits for an agent to be ready.
pub(super) const STARTED_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) const STARTED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A send waiting for its agent to be ready, until it may be typed or its
/// deadline passes.
pub(super) struct Starting {
    w: Waiting,
    force: bool,
    until: std::time::Instant,
    /// The screen when it last changed: only whether it changes is read.
    screen: (std::time::Instant, String),
}

/// The person's yes, on record before anything is typed.
///
/// The order is the point: if the record cannot be written there is no
/// confirmed Ask, and typing the prompt anyway would be a send with nothing
/// behind it. An Ask that was already confirmed has nothing to write.
pub(super) fn recorded_first(
    out: &dyn crate::outside::Outside,
    root: &Path,
    confirm: Option<&str>,
) -> Result<(), String> {
    let Some(ask_id) = confirm else { return Ok(()) };
    crate::ringframe::ask_confirm(out, root, ask_id)
        .map_err(|e| format!("RingFrame would not record your yes: {e:?}. Nothing was typed."))
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
}

/// One send for a pane's worker to type.
pub(super) struct Job {
    typist: crate::pane::Typist,
    payload: Vec<u8>,
    how: weft_core::inject::Handoff,
    project: usize,
    pane: u32,
    sends: Option<String>,
}

/// A pane's worker: it types each send in turn and says how each went.
/// Typing waits on the agent (a paste landing, a mode entered), seconds at
/// worst, so it never runs on the run loop. It ends with its pane.
fn typer(tx: Sender<Wake>) -> Sender<Job> {
    let (jobs, rx) = channel::<Job>();
    std::thread::spawn(move || {
        for j in rx {
            let typed = j.typist.inject_as(&j.payload, &j.how);
            let done = Wake::Typed { project: j.project, pane: j.pane, typed, sends: j.sends };
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
        match crate::acts::send(self.outside.as_ref(), &root, unit, &harness) {
            Ok(built) => Ok(Elsewhere { to, built }),
            Err(why) => Err(("refused", why)),
        }
    }

    /// Type a prompt that was said yes to, once its agent is ready: its turn
    /// is not running, by its hooks, and its screen has not changed
    /// for [`STARTED_QUIET`]. A busy agent, or one still
    /// starting, is waited for on the tick, never here. The refusal, if
    /// there is one, is the person's to see.
    pub(super) fn type_it(&mut self, project: usize, w: Waiting, force: bool) {
        let now = std::time::Instant::now();
        let screen = (now, String::new());
        let until = now + STARTED_TIMEOUT;
        self.projects[project].starting.push(Starting { w, force, until, screen });
        self.type_what_started(project);
    }

    /// Type each send whose agent is ready now, or whose wait is over. A
    /// screen that never holds still (a spinner) does not stop a ready agent
    /// past the deadline; a turn that is running does.
    pub(super) fn type_what_started(&mut self, project: usize) {
        let waiting = std::mem::take(&mut self.projects[project].starting);
        let now = std::time::Instant::now();
        for mut s in waiting {
            let ready =
                s.force || crate::turns::may_type(self.pane_state(project, s.w.pane)).is_ok();
            // The words on it are not read, only whether it has gone still. An
            // agent whose hooks say it is ready may have drawn nothing; one the
            // person forced must at least have drawn something.
            let still = match self.projects[project].slot(s.w.pane) {
                Some(slot) => {
                    slot.pane.gone_quiet(&mut s.screen, STARTED_QUIET)
                        || (!s.force && s.screen.0.elapsed() >= STARTED_QUIET)
                }
                None => true,
            };
            if now >= s.until || (ready && still) {
                self.type_now(project, s.w, s.force);
            } else {
                self.projects[project].starting.push(s);
            }
        }
    }

    pub(super) fn type_now(&mut self, project: usize, w: Waiting, force: bool) {
        // Weft types on its own only into an agent that reported it is
        // ready. When the person has been told and said to type anyway,
        // the decision is theirs.
        let not_ready =
            if force { Ok(()) } else { crate::turns::may_type(self.pane_state(project, w.pane)) };
        let tx = self.tx.clone();
        let refusal = match self.projects[project].slot(w.pane) {
            Some(slot) => match not_ready {
                Err(why) if slot.pane.running() => Some(why.to_string()),
                _ => {
                    let job = Job {
                        typist: slot.pane.typist(),
                        payload: w.payload,
                        how: w.how,
                        project,
                        pane: w.pane,
                        sends: w.sends.clone(),
                    };
                    match slot.typer.get_or_insert_with(|| typer(tx)).send(job) {
                        Ok(()) => None,
                        Err(_) => Some("NoProcess".to_string()),
                    }
                }
            },
            None => Some("NoProcess".to_string()),
        };
        match (refusal, w.sends) {
            // Typed and submitted is not delivered: what the pane reports it
            // received says whether this was the Ask. A report that comes
            // while the worker is still typing answers it too.
            (None, Some(ask)) => {
                let until = std::time::Instant::now() + RECEIPT_CAP;
                self.projects[project].due.push(Due { pane: w.pane, ask, until });
            }
            (None, None) => {}
            (refusal, _) => self
                .clients
                .broadcast(project, &Out::Injected { pane: w.pane, refusal, unrecorded: None }),
        }
    }

    /// A pane's worker finished typing a send. One it could not submit waits
    /// for no report.
    pub(super) fn typed(
        &mut self,
        project: usize,
        pane: u32,
        typed: Result<crate::pane::Attempt, weft_core::inject::Refusal>,
        sends: Option<String>,
    ) {
        let refusal = match typed {
            Err(r) => Some(format!("{r:?}")),
            // The paste never changed the screen: Enter is withheld.
            Ok(a) if !a.submitted => Some("PasteNotShown".to_string()),
            Ok(_) => None,
        };
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
        let Due { ask, .. } = due.remove(due.iter().position(|d| d.pane == pane)?);
        let tx = self.tx.clone();
        Some(Box::new(move |answer| {
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
            due.remove(i);
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

    /// Put a built prompt in front of everyone watching, and answer with its id.
    pub(super) fn stage(&mut self, at: usize, pane: u32, built: crate::acts::Built) -> Answer {
        let id = format!("pnd_{}", self.next_pending);
        self.next_pending += 1;
        let (what, why) = (built.asking.what, built.asking.why.join("\n"));
        let message = Out::Pending {
            id: id.clone(),
            pane,
            what: what.clone(),
            why: why.clone(),
            payload: built.payload.clone(),
            sends: built.ask_id.clone(),
        };
        self.projects[at].waiting.push(Waiting {
            id: id.clone(),
            pane,
            what,
            why,
            payload: built.payload,
            how: built.how,
            confirm: built.confirm_first.then(|| built.ask_id.clone()).flatten(),
            sends: built.ask_id.clone(),
        });
        self.clients.broadcast(at, &message);
        Answer::Ok(serde_json::json!({"pending": id}))
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn a_shared_prompt_answers_only_the_one_fresh_pane_waiting() {
        let fresh = |p: u32| p != 9;
        assert_eq!(only_one(&[2], &fresh), Some(2));
        assert_eq!(only_one(&[9, 2], &fresh), Some(2), "a tied pane is not fresh");
        assert_eq!(only_one(&[2, 3], &fresh), None, "two waiting: cannot tell which");
        assert_eq!(only_one(&[], &fresh), None);
    }
}
