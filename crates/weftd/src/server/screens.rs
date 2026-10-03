//! Screen frames for a client that attached to a pane (remote.md D2, D3).
//!
//! On each 100 ms tick, every attachment whose pane printed since its last
//! frame is sent the pane's visible screen: whole the first time and after a
//! resize, else as the difference from the screen it was sent last. The
//! screen is read from the pane's own parser under its lock, with the number
//! of the last chunk in it, so a frame always shows bytes the pane has seen.
//! A quiet pane sends nothing.
//!
//! Taken from the pane at each frame rather than kept by a parser per
//! attachment, as remote.md first had it: a parser started from
//! `state_formatted()` while the pane is in the alternate screen has no normal
//! screen behind it, and shows the wrong one when the harness leaves (Phase 6
//! step 7's note). The pane's visible screen is small; its scrollback is never
//! copied.

use super::*;

pub(super) struct Attachment {
    pub(super) client: u64,
    pub(super) project: usize,
    pub(super) pane: u32,
    /// The pane printed since the last frame.
    pub(super) dirty: bool,
    /// What this client was sent last, as a screen to diff against.
    last: Option<vt100::Screen>,
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
                dirty: true,
                last: None,
            }),
            "none" => {}
            other => return Answer::No("unknown_form", format!("there is no form {other}")),
        }
        Answer::nothing()
    }

    /// Mark every attachment of this pane as having something new.
    pub(super) fn printed(&mut self, project: usize, pane: u32) {
        for a in self.attached.iter_mut().filter(|a| a.project == project && a.pane == pane) {
            a.dirty = true;
        }
    }

    /// The 100 ms tick: one frame to each attachment with something new.
    pub(super) fn frames(&mut self) {
        let clients = &self.clients.sinks;
        self.attached.retain(|a| clients.contains_key(&a.client));
        let mut attached = std::mem::take(&mut self.attached);
        attached.retain_mut(|a| {
            let Some(slot) = self.projects.get_mut(a.project).and_then(|p| p.slot(a.pane)) else {
                return false;
            };
            // The size the pane was last given, which its parser has.
            let size = slot.size;
            let resized = a.last.as_ref().is_some_and(|l| l.size() != size);
            if !(a.dirty || resized) {
                return true;
            }
            a.dirty = false;
            let (formatted, seq) = slot.pane.screen_at();
            let mut now = vt100::Parser::new(size.0, size.1, 0);
            now.process(&formatted);
            let (full, bytes) = match &a.last {
                Some(last) if !resized => (false, now.screen().state_diff(last)),
                _ => (true, formatted),
            };
            if !full && bytes.is_empty() {
                return true;
            }
            a.last = Some(now.screen().clone());
            let (rows, cols) = size;
            let frame = Out::Screen { pane: a.pane, seq, full, rows, cols, bytes };
            self.clients.send_to(a.client, &Line::Event(frame));
            true
        });
        self.attached = attached;
    }
}
