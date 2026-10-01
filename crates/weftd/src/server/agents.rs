//! The agents: starting and closing them, which is free for new work, and
//! which session each runs, kept in Weft's own pane map.

use super::*;

impl Session {
    /// The first of this harness's agents that is free for new work: one
    /// whose latest event is `ready` or `turn_ended` (turn-state.md §4).
    pub(super) fn free_pane_of(&mut self, at: usize, harness: &str) -> Option<u32> {
        let states = self.agents_of(at).0;
        self.projects[at]
            .panes
            .iter()
            .zip(states)
            .find(|(s, state)| s.map.harness == harness && crate::turns::may_type(*state).is_ok())
            .map(|(s, _)| s.map.pane)
    }

    /// Start a pane of this harness for an act, on its own command: never
    /// another pane's, which may be resuming someone else's session.
    pub(super) fn start_for(&mut self, client: u64, at: usize, harness: &str) -> Option<u32> {
        let spec = self.projects[at].harnesses.find(harness)?.spec();
        let id = self.spawn(client, at, harness, &spec, None)?;
        self.look_at(at, harness);
        Some(id)
    }

    pub(super) fn pane_of(&self, at: usize, harness: &str) -> Option<u32> {
        self.projects[at].panes.iter().find(|s| s.map.harness == harness).map(|s| s.map.pane)
    }

    /// This project's panes, as everything outside the daemon sees them.
    pub(super) fn pane_infos(&mut self, project: usize) -> Vec<PaneInfo> {
        let Some(p) = self.projects.get_mut(project) else { return Vec::new() };
        p.panes
            .iter_mut()
            .map(|s| PaneInfo {
                pane: s.map.pane,
                harness: s.map.harness.clone(),
                spec: s.map.spec.clone(),
                running: s.pane.running(),
                turbo: s.turbo,
            })
            .collect()
    }

    /// Each pane's state and every session's latest event, re-read from the
    /// receipts. A new receipt may bind its session to a pane, once.
    pub(super) fn agents_of(&mut self, at: usize) -> Agents {
        let p = &mut self.projects[at];
        let heard = p.turns.refresh(&p.root);
        let mut map: Vec<crate::turns::Started> = p
            .panes
            .iter_mut()
            .map(|slot| {
                slot.map.running = slot.pane.running();
                slot.map.clone()
            })
            .collect();
        let mut moved = false;
        for s in &heard {
            moved |= crate::turns::heard(&mut map, s);
        }
        for (slot, m) in p.panes.iter_mut().zip(map) {
            slot.map = m;
        }
        if moved {
            self.write_map(at);
        }
        let p = &self.projects[at];
        let sessions = p.turns.sessions();
        let map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
        (crate::turns::pane_states(&map, &sessions), sessions)
    }

    /// Weft's own pane map, `<project>/.fab7/weft/panes.json`: which pane
    /// runs which harness and session, and how Weft knows. RingFrame never
    /// reads it; nothing in `.fab7/rf/` names a pane.
    pub(super) fn write_map(&self, at: usize) {
        let p = &self.projects[at];
        let dir = p.root.join(".fab7").join("weft");
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let ignore = dir.join(".gitignore");
        if !ignore.exists() {
            let _ = std::fs::write(&ignore, "*\n");
        }
        let panes: Vec<_> = p.panes.iter().map(|s| &s.map).collect();
        let mut text =
            serde_json::to_string_pretty(&serde_json::json!({"panes": panes})).unwrap_or_default();
        text.push('\n');
        write_whole(&dir.join("panes.json"), text);
    }

    /// One pane's state, read now.
    pub(super) fn pane_state(&mut self, at: usize, pane: u32) -> Option<Turn> {
        let i = self.projects[at].index_of(pane)?;
        self.agents_of(at).0.get(i).copied().flatten()
    }

    /// Take a pane away. The agent is stopped if it is somehow still running:
    /// closing is the person saying they are done with it, and a pane nobody
    /// can see is a process nobody can stop. Every other pane keeps its id,
    /// so the list is all a client needs to be told.
    pub(super) fn close(&mut self, project: usize, pane: u32) {
        let Some(p) = self.projects.get_mut(project) else { return };
        let Some(index) = p.index_of(pane) else { return };
        let mut slot = p.panes.remove(index);
        slot.pane.stop();
        self.write_map(project);
        let panes = self.pane_infos(project);
        self.clients.broadcast(project, &Out::Panes { panes });
    }

    /// Start an agent at the size of the window that asked, and answer with
    /// its pane's id.
    pub(super) fn spawn(
        &mut self,
        client: u64,
        project: usize,
        harness: &str,
        spec: &str,
        resumed: Option<String>,
    ) -> Option<u32> {
        let p = self.projects.get_mut(project)?;
        // The agent's hooks write only into a workspace that exists. Without
        // RingFrame, or outside Git, `init` refuses and the hooks record nothing.
        if !p.root.join(".fab7/rf").is_dir() {
            let _ = crate::ringframe::init(self.outside.as_ref(), &p.root);
        }
        let cwd = p.root.to_string_lossy().into_owned();
        // Turbo mode, when this project's config turns it on: the harness's
        // own flags, after the person's own.
        let flags = if p.turbo {
            p.harnesses.find(harness).map(|h| h.turbo.clone()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let turbo = !flags.is_empty();
        let words = weft_core::harness::with_turbo(spec, &flags);
        let program = words.first().cloned().unwrap_or_default();
        let argv: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();

        let id = p.next_pane;
        p.next_pane += 1;
        let (tx, harness_name) = (self.tx.clone(), harness.to_string());
        let size = self.clients.sizes.get(&client).copied().unwrap_or((24, 80));
        match Pane::spawn_args(harness, &program, &argv, &cwd, size.0, size.1) {
            Ok(mut pane) => {
                // Every byte a pane prints goes to the session, which records
                // it for replay and forwards it to whoever is attached.
                let sink = pane.stream_output();
                std::thread::spawn(move || {
                    while let Ok(bytes) = sink.recv() {
                        if tx.send(Wake::Output { project, pane: id, bytes }).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Wake::Exited { project, pane: id });
                });
                let now = crate::turns::now_millis();
                self.projects[project].panes.push(Slot {
                    map: crate::turns::Started::new(id, harness, spec, now, resumed),
                    pane,
                    replay: Vec::new(),
                    size,
                    wants: HashMap::from([(client, size)]),
                    turbo,
                });
                self.write_map(project);
                self.clients.broadcast(
                    project,
                    &Out::Added { pane: id, harness: harness_name, spec: spec.to_string(), turbo },
                );
                Some(id)
            }
            Err(e) => {
                self.clients.broadcast(
                    project,
                    &Out::Injected { pane: id, refusal: Some(e.to_string()), unrecorded: None },
                );
                None
            }
        }
    }
}
