//! Screen frames for a client that attached to a pane (remote.md D2, D3).
//!
//! Frames follow the change: output after a quiet spell is framed within
//! `GATHER`, and while the pane keeps printing, at most one frame in each
//! `FRAME_EVERY`. A frame is the pane's visible screen: whole the first time,
//! after a resize and when the alternate screen switched, else as the
//! difference from the screen it was sent last. The screen is read from the
//! pane's own terminal under its lock, with the number of the last chunk in
//! it, so a frame always shows bytes the pane has seen, and never while the
//! program is in the middle of a redraw. A quiet pane sends nothing.
//!
//! Only the visible screen is sent, never the normal screen behind the
//! alternate one: a device that starts while the alternate screen is on gets
//! the whole screen again when the harness leaves it. The pane's scrollback
//! is never copied.

use std::time::{Duration, Instant};

use super::*;

/// How long output after a quiet spell waits for the rest of its burst.
const GATHER: Duration = Duration::from_millis(8);
/// While a pane keeps printing, at most one frame in this long.
const FRAME_EVERY: Duration = Duration::from_millis(33);

pub(super) struct Attachment {
    pub(super) client: u64,
    pub(super) project: usize,
    pub(super) pane: u32,
    /// When the next frame is due: set when the pane prints, cleared once
    /// the frame is sent.
    due: Option<Instant>,
    /// When the last frame was sent, and what it showed, to diff against.
    sent_at: Option<Instant>,
    last: Option<weft_term::Snapshot>,
}

/// When output arriving `now` is framed: after `GATHER`, and no sooner than
/// `FRAME_EVERY` after the last frame.
fn frame_due(now: Instant, sent_at: Option<Instant>) -> Instant {
    let soonest = now + GATHER;
    sent_at.map_or(soonest, |sent| soonest.max(sent + FRAME_EVERY))
}

impl Session {
    /// `pane.attach`: start frames for this client, or stop them.
    pub(super) fn attach(&mut self, client: u64, pane: u32, form: &str) -> Answer {
        let Some((at, project)) = self.watched(client) else { return Answer::no_project() };
        if project.slot(pane).is_none() {
            return Answer::No("no_pane", format!("there is no pane {pane} here"));
        }
        self.attached.retain(|a| !(a.client == client && a.project == at && a.pane == pane));
        match form {
            "screen" => self.attached.push(Attachment {
                client,
                project: at,
                pane,
                due: Some(Instant::now()),
                sent_at: None,
                last: None,
            }),
            "none" => {}
            other => return Answer::No("unknown_form", format!("there is no form {other}")),
        }
        Answer::nothing()
    }

    /// The pane printed: a frame falls due for each attachment of it that
    /// has none due yet.
    pub(super) fn printed(&mut self, project: usize, pane: u32) {
        let now = Instant::now();
        for a in self.attached.iter_mut().filter(|a| a.project == project && a.pane == pane) {
            a.due.get_or_insert_with(|| frame_due(now, a.sent_at));
        }
    }

    /// The next event for the run loop, or none once every sender has gone.
    /// Frames that fall due meanwhile are sent, even while events keep
    /// arriving.
    pub(super) fn next_wake(&mut self, rx: &Receiver<Wake>) -> Option<Wake> {
        use std::sync::mpsc::RecvTimeoutError;
        loop {
            let now = Instant::now();
            match self.attached.iter().filter_map(|a| a.due).min() {
                None => return rx.recv().ok(),
                Some(due) if due <= now => self.frames(),
                Some(due) => match rx.recv_timeout(due - now) {
                    Ok(event) => return Some(event),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return None,
                },
            }
        }
    }

    /// One frame to each attachment whose frame is due, or whose pane was
    /// resized.
    pub(super) fn frames(&mut self) {
        let now = Instant::now();
        let clients = &self.clients.sinks;
        self.attached.retain(|a| clients.contains_key(&a.client));
        let mut attached = std::mem::take(&mut self.attached);
        attached.retain_mut(|a| {
            let Some(slot) = self.projects.get_mut(a.project).and_then(|p| p.slot(a.pane)) else {
                return false;
            };
            // The size the pane was last given, which its terminal has.
            let size = slot.size;
            let resized = a.last.as_ref().is_some_and(|l| l.size() != size);
            if !(a.due.is_some_and(|due| due <= now) || resized) {
                return true;
            }
            // Mid-redraw: asked again shortly, until the redraw ends.
            let Some(frame) = slot.pane.frame(a.last.as_ref()) else {
                a.due = Some(now + GATHER);
                return true;
            };
            a.due = None;
            if !frame.full && frame.bytes.is_empty() {
                return true;
            }
            a.sent_at = Some(now);
            a.last = Some(frame.shown);
            let (rows, cols) = size;
            let frame = Out::Screen {
                pane: a.pane,
                seq: frame.seq,
                full: frame.full,
                rows,
                cols,
                bytes: frame.bytes,
            };
            self.clients.send_to(a.client, &Line::Event(frame));
            true
        });
        self.attached = attached;
    }
}
