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

    /// Each pane's state and the project's session index, re-read from
    /// Weft's own receipts, the pane map and the record (sessions.md, "The
    /// session index"). A pane's session is the one its signals named; a
    /// session no pane holds runs while the process its receipts name does,
    /// or while its harness's open stream says a turn runs (ADR-0035 D4).
    pub(super) fn agents_of(&mut self, at: usize) -> Agents {
        let dir = crate::receipts::project_dir(&self.receipts, &self.projects[at].root);
        let p = &mut self.projects[at];
        p.turns.refresh(&dir);
        for slot in p.panes.iter_mut() {
            slot.map.running = slot.pane.running();
        }
        let heard = p.turns.heard();
        // Ask the machine about each recorded process not yet known gone.
        for proc in heard.iter().filter_map(|h| h.process.as_ref()) {
            let key = (proc.pid, proc.start.clone());
            if !p.gone.contains(&key) && !crate::process::alive(proc) {
                p.gone.insert(key);
            }
        }
        let streamed = |h: &weft_core::sessions::Heard| {
            self.streamed.contains(&(h.session.harness.clone(), h.session.id.clone()))
        };
        let heard: Vec<_> = heard
            .into_iter()
            .map(|h| weft_core::sessions::Heard { streamed: streamed(&h), ..h })
            .collect();
        let p = &self.projects[at];
        let sessions = p.turns.sessions();
        let map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
        let alive =
            |proc: &weft_core::sessions::Process| !p.gone.contains(&(proc.pid, proc.start.clone()));
        let index = weft_core::sessions::index(&heard, &map, &p.ledger.units(), &alive);
        (crate::turns::pane_states(&map, &sessions), index)
    }

    /// The command line that resumes `id`, when the index holds it for this
    /// harness and it can be resumed; else the refusal.
    pub(super) fn resumable(
        &mut self,
        at: usize,
        harness: &str,
        id: &str,
    ) -> Result<String, Answer> {
        let index = self.agents_of(at).1;
        let row = index
            .iter()
            .find(|r| r.harness == harness && r.id == id && weft_core::sessions::is_id(id))
            .filter(|r| r.resumable);
        let Some(row) = row else {
            return Err(Answer::No(
                "no_session",
                format!("there is no {harness} session {id} here"),
            ));
        };
        if row.state == weft_core::sessions::State::Running {
            let held = row.pane.map_or("another program".to_string(), |p| format!("pane {p}"));
            return Err(Answer::No("running", format!("{id} is running, in {held}")));
        }
        let file = self.projects[at].harnesses.find(harness);
        file.and_then(|h| h.resume_spec(id))
            .ok_or_else(|| Answer::No("no_harness", format!("{harness} has no resume line")))
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
        // Its hooks tell this daemon what it is doing, from inside this pane.
        let Some(token) = new_token() else {
            let refusal = Some("the system gave no randomness for the pane's token".into());
            self.clients.broadcast(project, &Out::Injected { pane: id, refusal, unrecorded: None });
            return None;
        };
        let unset = p.harnesses.find(harness).map(|h| h.unset_env.clone()).unwrap_or_default();
        let mut env: Vec<(&str, Option<String>)> =
            unset.iter().map(|k| (k.as_str(), None)).collect();
        env.extend([
            ("WEFT_PANE", Some(id.to_string())),
            ("WEFT_PANE_TOKEN", Some(token.clone())),
            ("WEFT_SOCKET", Some(self.socket.to_string_lossy().into_owned())),
        ]);
        match Pane::spawn_args(harness, &program, &argv, &cwd, &env, size.0, size.1) {
            Ok(mut pane) => {
                // Every byte a pane prints goes to the session, which records
                // it for replay and forwards it to whoever is attached.
                let sink = pane.stream_output();
                std::thread::spawn(move || {
                    while let Ok((_, bytes)) = sink.recv() {
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
                    token,
                });
                self.write_map(project);
                self.listen(project, &harness_name);
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

    /// One hook, as `weft signal` sent it. An event
    /// naming a pane counts only with that pane's token, and then its session
    /// is the pane's. One naming no pane is a session no pane holds, in the
    /// project its workspace is in, and changes no pane. Accepted, it is
    /// recorded in Weft's receipts; anything else is dropped unrecorded.
    pub(super) fn signal(&mut self, mut sent: weft_core::signal::Sent) {
        // A session or harness that is not only an id names no file and is
        // passed to no one: both become paths and arguments.
        use weft_core::sessions::is_id;
        if !is_id(&sent.session) || !is_id(&sent.harness) {
            return;
        }
        // Its harness's own stream gives this session's turn (ADR-0035 D3).
        if self.streamed.contains(&(sent.harness.clone(), sent.session.clone())) {
            sent.state = None;
        }
        // A line of a file is forwarded once per session: a later hook that
        // finds the same newest line has received nothing new.
        if let Some(key) = sent.prompt_key.take() {
            let seen = format!("{}\0{}\0{key}", sent.harness, sent.session);
            if !self.forwarded.insert(seen) {
                sent.prompt = None;
            }
        }
        let (at, root) = match sent.pane {
            Some(pane) => {
                let token = sent.token.as_deref().unwrap_or_default();
                let found = self.projects.iter().position(|p| {
                    !p.closed
                        && p.panes
                            .iter()
                            .any(|s| s.map.pane == pane && s.token == token && !token.is_empty())
                });
                let Some(at) = found else { return };
                // Everything the agent runs inherits the token: a session of
                // another harness started inside the pane is recorded, and
                // takes the pane from no one.
                let p = &mut self.projects[at];
                let mut map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
                let ours = map.iter().any(|m| m.pane == pane && m.harness == sent.harness);
                if ours && crate::turns::signalled(&mut map, pane, &sent.session) {
                    for (slot, m) in p.panes.iter_mut().zip(map) {
                        slot.map = m;
                    }
                    self.write_map(at);
                }
                (Some(at), self.projects[at].root.clone())
            }
            None => {
                let Some(root) = sent.workspace.as_deref().and_then(project_of) else { return };
                // An open project is keyed by the path it was opened at.
                let at = self
                    .projects
                    .iter()
                    .position(|p| !p.closed && p.root.canonicalize().is_ok_and(|r| r == root));
                let root = at.map_or(root, |at| self.projects[at].root.clone());
                // A session a pane holds is that pane's: only its own signals move it.
                let held = at.is_some_and(|at| {
                    self.projects[at].panes.iter().any(|s| {
                        s.map.running
                            && s.map.session.as_ref().is_some_and(|b| b.session == sent.session)
                    })
                });
                if held {
                    return;
                }
                (at, root)
            }
        };
        if let Some(at) = at {
            self.listen(at, &sent.harness);
        }
        let _ = crate::receipts::record(&self.receipts, &root, &sent, crate::turns::now_millis());
        if let Some(at) = at {
            self.tell_agents(at);
        }
        let answered = at.and_then(|at| self.answers_a_send(at, &sent));
        // A turn that ended with nothing received: the send's wait is over.
        if answered.is_none()
            && let (Some(at), Some(pane)) = (at, sent.pane)
            && matches!(sent.state.as_deref(), Some("turn_ended" | "ended"))
        {
            self.received_nothing(at, pane);
        }
        self.forwarders.send(&root, sent, answered);
    }
}

impl Session {
    /// Open a harness's own stream, once, when its harness file names one
    /// and its server's socket is there; with none, hooks alone apply. It is
    /// opened again on the next pane or hook after its server closes it,
    /// never on a timer (ADR-0035 D2).
    pub(super) fn listen(&mut self, at: usize, harness: &str) {
        use crate::harness::OnThisMachine;
        if self.streams.contains(harness) {
            return;
        }
        let Some(h) = self.projects.get(at).and_then(|p| p.harnesses.find(harness)) else { return };
        let Some(listen) = h.listen.clone() else { return };
        let socket = h.config_home().join(&listen.socket);
        if !socket.exists() {
            return;
        }
        self.streams.insert(harness.to_string());
        let (tx, harness) = (self.tx.clone(), harness.to_string());
        std::thread::spawn(move || {
            let _ = crate::listen::run(&socket, &listen, |session, state| {
                let _ = tx.send(Wake::Heard { harness: harness.clone(), session, state });
            });
            let _ = tx.send(Wake::Unheard { harness });
        });
    }

    /// A session's turn as its harness's stream said it, recorded as a hook's
    /// would be, in the open project whose receipts already hold the session.
    /// The stream names no workspace, so a session no hook tied to a project
    /// is no one's (ADR-0035 D3).
    pub(super) fn heard(&mut self, harness: String, session: String, state: String) {
        if !weft_core::sessions::is_id(&harness) || !weft_core::sessions::is_id(&session) {
            return;
        }
        let file = |root: &Path| {
            crate::receipts::project_dir(&self.receipts, root)
                .join(&harness)
                .join(format!("{session}.jsonl"))
        };
        let Some(at) = self.projects.iter().position(|p| !p.closed && file(&p.root).exists())
        else {
            return;
        };
        self.streamed.insert((harness.clone(), session.clone()));
        let ended = matches!(state.as_str(), "turn_ended" | "ended");
        let root = self.projects[at].root.clone();
        let sent = weft_core::signal::Sent {
            harness,
            session: session.clone(),
            state: Some(state),
            ..Default::default()
        };
        let _ = crate::receipts::record(&self.receipts, &root, &sent, crate::turns::now_millis());
        self.tell_agents(at);
        // A send into the pane that holds it was waiting for this turn's end.
        let holds = |s: &&Slot| s.map.session.as_ref().is_some_and(|b| b.session == session);
        if ended && let Some(pane) = self.projects[at].panes.iter().find(holds).map(|s| s.map.pane)
        {
            self.received_nothing(at, pane);
        }
    }
}

/// 128 random bits, as hex, or none when the system gives none: a pane is
/// never started with a token anyone could guess.
fn new_token() -> Option<String> {
    use std::io::Read as _;
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The project a workspace path is in, as RingFrame resolves a workspace: the
/// nearest folder at or above it that holds a record (`.fab7/rf/`), looking
/// no higher than its Git repository, else the path itself. A
/// path that is not an absolute folder is none. `~/.fab7/` alone, which
/// every machine with Weft has, makes no project.
fn project_of(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return None;
    }
    let path = path.canonicalize().ok().filter(|p| p.is_dir())?;
    for dir in path.ancestors() {
        if dir.join(".fab7").join("rf").is_dir() {
            return Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    Some(path)
}
