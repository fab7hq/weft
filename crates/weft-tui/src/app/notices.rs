//! What the person is told without looking: an agent asking them, through
//! the terminal (ADR-0017), and an agent that ended.

use super::*;

/// One notification, through the terminal Weft runs in: OSC 9 where the
/// terminal shows it, the bell where it shows none. Nothing leaves the
/// terminal (ADR-0017).
pub fn notification(term_program: Option<&str>, message: &str) -> Vec<u8> {
    const SHOWS_OSC_9: [&str; 3] = ["iTerm.app", "WezTerm", "ghostty"];
    if !term_program.is_some_and(|t| SHOWS_OSC_9.contains(&t)) {
        return b"\x07".to_vec();
    }
    let text: String = message.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]9;{text}\x07").into_bytes()
}

impl App {
    /// What to tell the person, once per change: an agent they are not
    /// looking at started asking them something (ADR-0017).
    /// The agent in front is never announced, and nothing is said with
    /// `notify = false`.
    pub fn notices(&mut self) -> Vec<String> {
        use weft_core::turns;
        let mut out = Vec::new();
        let (at, focus) = (self.at, self.pane_focus);
        for (i, p) in self.projects.iter_mut().enumerate() {
            let now = p.session.panes.iter().map(|v| (v.id, v.turn)).collect();
            let Some(before) = p.heard.replace(now) else { continue };
            if !p.session.notify {
                continue;
            }
            for (pane, v) in p.session.panes.iter().enumerate() {
                // Only an agent asking for input: when a turn ends is the
                // person's workflow, not an alarm. A pane that just opened
                // was asking nothing before.
                let was = before.get(&v.id).copied().flatten();
                if !turns::is_asking(v.turn) || was == v.turn || (i == at && pane == focus) {
                    continue;
                }
                out.push(format!("{} in {}: needs your input", v.harness, p.name));
            }
        }
        out
    }

    /// An agent quit from inside — `Ctrl+D`, `/exit`, or it simply stopped.
    /// The pane is still on screen showing its last frame, which is worth
    /// keeping; what must not happen is keystrokes going to a dead process.
    pub fn notice_an_agent_that_ended(&mut self) {
        let panes = &sess!(self).panes;
        let Some(pane) = panes.iter().position(|p| !p.running && !self.announced.contains(&p.id))
        else {
            return;
        };
        self.announced.insert(panes[pane].id);
        let harness = self.harness_at(pane).unwrap_or("the agent").to_string();
        // The work is in the record, so the pane goes and the row says so.
        self.close_pane(pane);
        self.say(format!("{harness} has ended. Its work is still on the list."));
    }
}
