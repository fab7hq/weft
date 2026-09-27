//! Typing into an agent on the person's yes: staged, waited for while the
//! agent starts, and refused with the reason when it is not ready.

use super::*;

/// How long a pane Weft started for an act must be still before it is typed
/// into, and how long Weft waits for that.
pub(super) const STARTED_QUIET: std::time::Duration = std::time::Duration::from_secs(1);

pub(super) const STARTED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// A send into an agent that is still starting, until it may be typed or its
/// deadline passes.
pub(super) struct Starting {
    w: Waiting,
    force: bool,
    until: std::time::Instant,
    /// The screen when it last changed, for a send the person forced.
    screen: (std::time::Instant, String),
}

/// The person's yes, on record before anything is typed.
///
/// The order is the point: if the record cannot be written there is no
/// confirmed Ask, and typing the prompt anyway would be a send with nothing
/// behind it. An Ask that was already confirmed has nothing to write.
pub(super) fn recorded_first(root: &Path, confirm: Option<&str>) -> Result<(), String> {
    let Some(ask_id) = confirm else { return Ok(()) };
    crate::ringframe::ask_confirm(root, ask_id)
        .map_err(|e| format!("RingFrame would not record your yes: {e:?}. Nothing was typed."))
}

/// Why Enter was withheld. Written once here because it is the daemon that
/// knows, and the person has to be told the text is sitting in their agent.
pub(super) fn withheld(a: &crate::pane::Attempt) -> String {
    match &a.folded_command {
        // The prompt is all there, but folded into a placeholder, and a folded
        // paste is not read for commands. Sending it would ask for an ordinary
        // answer instead of the mode.
        Some(command) => format!(
            "the agent folded the paste, so {command} would not run — it would be taken as \
             ordinary text. The prompt is in its composer; type {command} there yourself, or \
             press Enter to send it without the mode."
        ),
        None => "the agent did not show the whole prompt, so it was not sent. The text is in \
                 its composer."
            .to_string(),
    }
}

impl Session {
    /// Type a prompt that was said yes to. The refusal, if there is one, is
    /// the person's to see: it says what is sitting unsent in their agent.
    ///
    /// An agent Weft just started is not ready until it says so, and one that
    /// never will still draws its composer a moment after it starts. Either
    /// is waited for on the tick, never here.
    pub(super) fn type_it(&mut self, project: usize, w: Waiting, force: bool) {
        if w.fresh {
            let now = std::time::Instant::now();
            let screen = (now, String::new());
            let until = now + STARTED_TIMEOUT;
            self.projects[project].starting.push(Starting { w, force, until, screen });
            return;
        }
        self.type_now(project, w, force);
    }

    /// Type each send whose agent is ready now, or whose wait is over.
    pub(super) fn type_what_started(&mut self, project: usize) {
        let waiting = std::mem::take(&mut self.projects[project].starting);
        let now = std::time::Instant::now();
        for mut s in waiting {
            let go = now >= s.until
                || if s.force {
                    // The words on it are not read, only whether it has gone
                    // still.
                    match self.projects[project].slot(s.w.pane) {
                        Some(slot) => slot.pane.gone_quiet(&mut s.screen, STARTED_QUIET),
                        None => true,
                    }
                } else {
                    crate::turns::may_type(self.pane_state(project, s.w.pane)).is_ok()
                };
            if go {
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
        let mut unrecorded = None;
        let refusal = match self.projects[project].slot(w.pane) {
            Some(slot) => match not_ready {
                Err(why) if slot.pane.running() => Some(why.to_string()),
                _ => match slot.pane.inject_as(&w.payload, &w.how) {
                    Err(r) => Some(format!("{r:?}")),
                    Ok(a) if !a.submitted => Some(withheld(&a)),
                    Ok(_) => {
                        slot.map.enter(crate::turns::now_millis());
                        // Sent on the person's yes: their submission, on
                        // record, so the Ask moves on to its Eval even
                        // where no hook sees it arrive. When RingFrame will
                        // not record it, the person is told: the row would
                        // otherwise offer the send again.
                        if let Some(ask) = &w.sends {
                            let root = self.projects[project].root.clone();
                            unrecorded = crate::ringframe::ask_submitted(&root, ask).err().map(
                                |e| match e {
                                    crate::ringframe::Error::NotInstalled => {
                                        "ringframe is not installed".to_string()
                                    }
                                    crate::ringframe::Error::Refused { message, .. } => {
                                        weft_core::offers::first_line(&message).to_string()
                                    }
                                },
                            );
                        }
                        None
                    }
                },
            },
            None => Some("NoProcess".to_string()),
        };
        self.clients.broadcast(project, &Out::Injected { pane: w.pane, refusal, unrecorded });
    }

    /// Put a built prompt in front of everyone watching, and answer with its id.
    pub(super) fn stage(
        &mut self,
        at: usize,
        pane: u32,
        built: crate::acts::Built,
        fresh: bool,
    ) -> Answer {
        let id = format!("pnd_{}", self.next_pending);
        self.next_pending += 1;
        let message = Out::Pending {
            id: id.clone(),
            pane,
            what: built.asking.what,
            why: built.asking.why.join("\n"),
            payload: built.payload.clone(),
        };
        self.projects[at].waiting.push(Waiting {
            id: id.clone(),
            pane,
            payload: built.payload,
            how: built.how,
            confirm: built.confirm_first.then(|| built.ask_id.clone()).flatten(),
            fresh,
            sends: built.ask_id.clone(),
        });
        self.clients.broadcast(at, &message);
        Answer::Ok(serde_json::json!({"pending": id}))
    }
}
