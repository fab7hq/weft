//! Typing into an agent on the person's yes: staged, waited for until the
//! agent is ready, refused with the reason when it is not, and proven by what
//! the agent reports it received (ADR-0033).

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

/// How long after Enter a pane has to report the prompt it received, before
/// Weft says it cannot tell whether the Ask arrived (ADR-0033 D1).
pub(super) const RECEIPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A send of an Ask, typed and submitted, waiting for what its pane reports
/// it received. Only that report, judged by RingFrame, makes it delivered.
pub(super) struct Due {
    pane: u32,
    ask: String,
    until: std::time::Instant,
}

/// Whether RingFrame's answer to a received prompt says it is this Ask's
/// submission: the one comparison is RingFrame's (`observe prompt`), with
/// its allowances; Weft compares no bytes.
fn is_the_ask(answer: &serde_json::Value, ask: &str) -> bool {
    answer["submission"]["ask_id"].as_str() == Some(ask)
}

impl Session {
    /// Type a prompt that was said yes to, once its agent is ready: its turn
    /// is not running, by its hooks (ADR-0013), and its screen has not changed
    /// for [`STARTED_QUIET`] (ADR-0033 D2). A busy agent, or one still
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
        // Weft types on its own only into an agent that reported it is ready
        // (ADR-0013). When the person has been told and said to type anyway,
        // the decision is theirs.
        let not_ready =
            if force { Ok(()) } else { crate::turns::may_type(self.pane_state(project, w.pane)) };
        let mut submitted = false;
        let refusal = match self.projects[project].slot(w.pane) {
            Some(slot) => match not_ready {
                Err(why) if slot.pane.running() => Some(why.to_string()),
                _ => match slot.pane.inject_as(&w.payload, &w.how) {
                    Err(r) => Some(format!("{r:?}")),
                    // The paste never changed the screen: Enter is withheld.
                    Ok(a) if !a.submitted => Some("PasteNotShown".to_string()),
                    Ok(_) => {
                        submitted = true;
                        None
                    }
                },
            },
            None => Some("NoProcess".to_string()),
        };
        // Typed and submitted is not delivered: what the pane reports it
        // received says whether this was the Ask (D1).
        if let Some(ask) = w.sends.filter(|_| submitted) {
            let until = std::time::Instant::now() + RECEIPT_TIMEOUT;
            self.projects[project].due.push(Due { pane: w.pane, ask, until });
        }
        self.clients.broadcast(project, &Out::Injected { pane: w.pane, refusal, unrecorded: None });
    }

    /// When this signal is the next prompt a pane received since Weft sent
    /// it an Ask, what to do with RingFrame's verdict on it.
    pub(super) fn answers_a_send(
        &mut self,
        at: usize,
        sent: &weft_core::signal::Sent,
    ) -> Option<crate::forward::Answered> {
        let pane = sent.pane?;
        if sent.forward.as_deref() != Some("prompt") || sent.prompt.is_none() {
            return None;
        }
        let due = &mut self.projects[at].due;
        let Due { ask, .. } = due.remove(due.iter().position(|d| d.pane == pane)?);
        let tx = self.tx.clone();
        Some(Box::new(move |answer| {
            let _ = tx.send(Wake::Received { project: at, pane, ask, answer });
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
    ) {
        let said = match answer {
            Ok(v) if is_the_ask(&v, ask) => return,
            Ok(_) => Out::Injected { pane, refusal: Some("NotTheAsk".into()), unrecorded: None },
            // RingFrame did not say: the prompt went, unjudged.
            Err(why) => Out::Injected { pane, refusal: None, unrecorded: Some(why) },
        };
        self.clients.broadcast(project, &said);
    }

    /// Sends whose pane reported nothing in time: Weft cannot tell whether
    /// the Ask arrived. A stall does not prove nothing was sent, so no retry.
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
    /// harness file does not say where the prompt it received is (D4).
    pub(super) fn cannot_prove(&mut self, at: usize, pane: u32) -> Option<String> {
        let p = &mut self.projects[at];
        let name = p.slot(pane)?.map.harness.clone();
        let harness = p.harnesses.find(&name);
        if harness.and_then(|h| h.signal.as_ref()).is_some_and(|s| s.prompt.is_some()) {
            return None;
        }
        let title = harness.map_or(name.as_str(), |h| h.title.as_str());
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
