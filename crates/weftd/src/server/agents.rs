//! The agents: starting and closing them, which is free for new work, and
//! which session each runs, kept in Weft's own pane map.

use super::*;

/// How long a pane's output must be still before Sense reads it.
const SENSE_SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

/// Whether a pane printed since Sense last read it (`read`, the output's
/// number then) and has since been still for [`SENSE_SETTLE`].
fn settled(seq: u64, read: u64, quiet: std::time::Duration) -> bool {
    seq != read && quiet >= SENSE_SETTLE
}

/// A project's session index as last built, and what it was built from.
#[derive(Default)]
pub(super) struct Index {
    rows: Vec<weft_core::sessions::Row>,
    /// Counts the times the rows came out different.
    version: u64,
    built: Option<Built>,
    /// Something it is built from moved where no count shows it: a
    /// harness's stream opened or closed on one of its sessions.
    pub(super) stale: bool,
    /// The processes its receipts name that ran when it was built: the only
    /// ones asked about until it is built again.
    live: Vec<weft_core::sessions::Process>,
}

/// What a project's clients were last told of its agents.
#[derive(Default)]
pub(super) struct Told {
    /// Each pane's state, when anything was told.
    panes: Option<Vec<Option<Turn>>>,
    /// The index's version and the Mac's day the rows were picked at.
    version: u64,
    day: i64,
    /// The sessions carried as they are (the running, those panes hold, and
    /// the newest of today's), and how many earlier ones there are.
    pub(super) shown: Vec<weft_core::sessions::Row>,
    pub(super) earlier: usize,
}

impl Told {
    pub(super) fn panes(&self) -> &[Option<Turn>] {
        self.panes.as_deref().unwrap_or_default()
    }
}

const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;

/// What an index was built from.
#[derive(PartialEq)]
struct Built {
    turns: u64,
    gone: usize,
    judged: usize,
    ledger: u64,
    firsts: u64,
    /// Each pane, and whether it judges.
    panes: Vec<(crate::turns::Started, bool)>,
}

/// A project's first prompts, as RingFrame last read them.
pub(super) struct Firsts {
    titles: std::collections::BTreeMap<String, String>,
    /// Counts the times they changed.
    moves: u64,
    /// RingFrame's reader, here between reads and away during one.
    reader: Option<ringframe::api::FirstPrompts>,
    asked: Option<std::time::Instant>,
}

impl Default for Firsts {
    fn default() -> Self {
        Firsts {
            titles: Default::default(),
            moves: 0,
            reader: Some(Default::default()),
            asked: None,
        }
    }
}

impl Session {
    /// Start a pane of this harness for an act, on its own command: never
    /// another pane's, which may be resuming someone else's session.
    pub(super) fn start_for(&mut self, client: u64, at: usize, harness: &str) -> Option<u32> {
        let spec = self.projects[at].harnesses.find(harness)?.spec();
        let id = self.spawn(client, at, harness, &spec, None)?;
        self.look_at(at, harness);
        Some(id)
    }

    /// Ask RingFrame for the first prompts of the sessions listed here, at
    /// most every few seconds and never while it is still reading: a prompt
    /// typed now names its agent soon enough. It reads off the run loop, as
    /// a read can block (a system prompt over the folder), and nothing here
    /// waits for it.
    pub(super) fn read_first_prompts(&mut self, at: usize) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(5);
        let p = &mut self.projects[at];
        if p.first_prompts.asked.is_some_and(|asked| asked.elapsed() < EVERY) {
            return;
        }
        let Some(mut reader) = p.first_prompts.reader.take() else { return };
        p.first_prompts.asked = Some(std::time::Instant::now());
        let listed: Vec<(String, String)> = p
            .index
            .rows
            .iter()
            .filter(|r| weft_core::sessions::is_id(&r.id))
            .map(|r| (r.harness.clone(), r.id.clone()))
            .collect();
        let (tx, root) = (self.tx.clone(), p.root.clone());
        std::thread::spawn(move || {
            let titles = ringframe::api::workspace(&root)
                .map(|ws| reader.of(&ws, &listed))
                .unwrap_or_default();
            let _ = tx.send(Wake::FirstPrompts { project: at, reader, titles });
        });
    }

    /// What RingFrame read of a project's first prompts: kept, and told when
    /// it changed a title.
    pub(super) fn first_prompts_read(
        &mut self,
        at: usize,
        reader: ringframe::api::FirstPrompts,
        titles: std::collections::BTreeMap<String, String>,
    ) {
        let Some(p) = self.projects.get_mut(at).filter(|p| !p.closed) else { return };
        let firsts = &mut p.first_prompts;
        firsts.reader = Some(reader);
        if firsts.titles != titles {
            firsts.titles = titles;
            firsts.moves += 1;
            self.tell_agents(at);
        }
    }

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
                judge: s.judge.clone(),
            })
            .collect()
    }

    /// Each pane's state and the project's session index (sessions.md, "The
    /// session index").
    pub(super) fn agents_of(&mut self, at: usize) -> Agents {
        let states = self.states_of(at);
        (states, self.projects[at].index.rows.clone())
    }

    /// Each pane's state, with the session index brought up to date.
    pub(super) fn states_of(&mut self, at: usize) -> Vec<Option<Turn>> {
        self.index_now(at);
        let p = &self.projects[at];
        let map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
        // Only the sessions the panes run are looked up.
        let bound: Vec<_> = map
            .iter()
            .filter_map(|m| p.turns.session(&m.harness, &m.session.as_ref()?.session).cloned())
            .collect();
        crate::turns::pane_states(&map, &bound)
            .into_iter()
            .zip(p.panes.iter())
            .map(|(hook_turn, slot)| {
                if !slot.map.running {
                    return hook_turn;
                }
                // Sense's reading counts only for the screen as it is now.
                let read = slot
                    .sensed_surface
                    .filter(|r| r.seq == slot.pane.current_seq())
                    .map(|r| r.state);
                let gaps = gaps_of(&p.harnesses, &slot.map.harness);
                crate::turns::pane_turn(hook_turn, read, slot.last_output_at.elapsed(), gaps)
            })
            .collect()
    }

    /// The project's session index, from Weft's own receipts, the pane map
    /// and the record. A pane's session is the one its signals named; a
    /// session no pane holds runs while the process its receipts name does,
    /// or while its harness's open stream says a turn runs.
    ///
    /// Only the processes of sessions not known ended are asked about each
    /// time. The index is built again only when what it is built from moved:
    /// a receipt, a process found gone, a pane, the record, a stream, a
    /// first prompt.
    fn index_now(&mut self, at: usize) {
        let p = &mut self.projects[at];
        for slot in p.panes.iter_mut() {
            slot.map.running = slot.pane.running();
        }
        for proc in &p.index.live {
            let key = (proc.pid, proc.start.clone());
            if !p.gone.contains(&key) && !crate::process::alive(proc) {
                p.gone.insert(key);
            }
        }
        let from = |p: &Project| Built {
            turns: p.turns.moves(),
            gone: p.gone.len(),
            judged: p.judged.len(),
            ledger: p.ledger.moves(),
            firsts: p.first_prompts.moves,
            panes: p.panes.iter().map(|s| (s.map.clone(), s.judge.is_some())).collect(),
        };
        let now = from(p);
        if !p.index.stale && p.index.built.as_ref() == Some(&now) {
            return;
        }
        let mut heard = p.turns.heard();
        let mut live = Vec::new();
        for h in &mut heard {
            let Some(proc) = h.process.as_ref() else { continue };
            // A process gone with its start time never comes back.
            let key = (proc.pid, proc.start.clone());
            if p.gone.contains(&key) {
                continue;
            }
            if !crate::process::alive(proc) {
                p.gone.insert(key);
                continue;
            }
            live.push(proc.clone());
            // A harness's shared server holds every session, so it is no
            // session's process, however old the receipt that names it.
            let server = p.harnesses.find(&h.session.harness).map(|f| f.server_args.as_slice());
            if crate::process::is_server(proc, server.unwrap_or(&[])) {
                h.process = None;
            }
        }
        let streamed = &self.streamed;
        let p = &mut self.projects[at];
        // A judge's session is no agent of the person's.
        let theirs: Vec<_> = heard
            .into_iter()
            .filter(|h| !p.judged.contains(&h.session.id))
            .map(|h| {
                let key = (h.session.harness.clone(), h.session.id.clone());
                weft_core::sessions::Heard { streamed: streamed.contains(&key), ..h }
            })
            .collect();
        let agents: Vec<_> =
            p.panes.iter().filter(|s| s.judge.is_none()).map(|s| s.map.clone()).collect();
        let gone = &p.gone;
        let alive =
            |proc: &weft_core::sessions::Process| !gone.contains(&(proc.pid, proc.start.clone()));
        let mut rows = weft_core::sessions::index(&theirs, &agents, &p.ledger.units(), &alive);
        weft_core::sessions::first_prompt_titles(&mut rows, &p.first_prompts.titles);
        let built = Some(from(p));
        let index = &mut p.index;
        if rows != index.rows {
            index.rows = rows;
            index.version += 1;
        }
        (index.live, index.stale, index.built) = (live, false, built);
    }

    /// Tell a project's clients what moved: each pane's state, and the
    /// sessions carried as they are that changed, came or went. The rows are
    /// picked again only when the index or the Mac's day moved.
    pub(super) fn tell_agents(&mut self, at: usize) {
        use weft_core::sessions::{Row, carried};
        let panes = self.states_of(at);
        let zone = crate::turns::zone();
        let now = crate::turns::now_millis();
        let day = (now + zone).div_euclid(DAY_MILLIS);
        let p = &mut self.projects[at];
        let told = &mut p.told;
        let (version, first, was_earlier) = (p.index.version, told.panes.is_none(), told.earlier);
        let (mut changed, mut gone) = (Vec::new(), Vec::new());
        if first || told.version != version || told.day != day {
            let shown = carried(&p.index.rows, now, zone);
            let was: HashMap<String, &Row> = told.shown.iter().map(|r| (r.key(), r)).collect();
            changed = shown.iter().filter(|r| was.get(&r.key()) != Some(r)).cloned().collect();
            let keys: std::collections::HashSet<String> = shown.iter().map(Row::key).collect();
            gone = told.shown.iter().map(Row::key).filter(|k| !keys.contains(k)).collect();
            told.earlier = p.index.rows.len() - shown.len();
            (told.shown, told.version, told.day) = (shown, version, day);
        }
        let still = changed.is_empty() && gone.is_empty() && told.earlier == was_earlier;
        if !first && still && told.panes.as_ref() == Some(&panes) {
            return;
        }
        told.panes = Some(panes.clone());
        let earlier = told.earlier;
        self.clients.broadcast(at, &Out::Agents { panes, changed, gone, earlier, zone });
    }

    /// `read` `what: "sessions"`: the sessions past those carried as they
    /// are, newest first, a page at a time after the row `after` names;
    /// with `q`, only those a search for it finds, among them all.
    pub(super) fn earlier_sessions(
        &mut self,
        at: usize,
        after: Option<&str>,
        q: Option<&str>,
    ) -> Answer {
        let p = &self.projects[at];
        let shown: std::collections::HashSet<String> =
            p.told.shown.iter().map(|r| r.key()).collect();
        let q = q.map(str::trim).filter(|q| !q.is_empty());
        let found: Vec<&weft_core::sessions::Row> = p
            .index
            .rows
            .iter()
            .filter(|r| !shown.contains(&r.key()))
            .filter(|r| q.is_none_or(|q| weft_core::sessions::matches(r, q)))
            .collect();
        let start = match after {
            Some(a) => found.iter().position(|r| r.key() == a).map_or(found.len(), |i| i + 1),
            None => 0,
        };
        let end = (start + weft_core::sessions::PAGE).min(found.len());
        let next = (end < found.len()).then(|| found[end - 1].key());
        Answer::Ok(serde_json::json!({
            "rows": &found[start..end],
            "total": found.len(),
            "next": next,
        }))
    }

    /// Archive this project's ended sessions when it opens, and once a day
    /// after.
    pub(super) fn archive_when_due(&mut self, at: usize) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
        let p = &mut self.projects[at];
        if p.closed || p.archived.is_some_and(|a| a.elapsed() < EVERY) {
            return;
        }
        p.archived = Some(std::time::Instant::now());
        let keep = crate::routing::read_at(&self.config, &p.harnesses).keep_days();
        let archive = self.config.with_file_name("archive").join("sessions");
        self.archive(at, keep, &archive, crate::turns::now_millis());
    }

    /// Move to `archive` the receipts of every session here that has not run
    /// for `keep` days and belongs to no work still to be sealed, and stop
    /// listing them. They can be moved back by hand. RingFrame's own files
    /// in the project are not touched; 0 days keeps every session.
    fn archive(&mut self, at: usize, keep: u64, archive: &Path, now: i64) {
        if keep == 0 {
            return;
        }
        self.index_now(at);
        let p = &mut self.projects[at];
        // The sessions any unsealed work was asked in, or arrived in.
        let units = p.ledger.units();
        let work = weft_core::work::work(&units, &p.ledger.record());
        let mut working = std::collections::HashSet::new();
        for u in work.units.iter().filter(|u| !u.sealed()) {
            for ask in units.iter().filter(|a| u.asks.contains(&a.ask_id)) {
                working.extend(ask.session_ref.clone());
                working.extend(ask.arrived.as_ref().map(|a| a.session.clone()));
            }
        }
        let before = now - keep as i64 * DAY_MILLIS;
        let dir = p.turns.dir().to_path_buf();
        let Some(key) = dir.file_name() else { return };
        let mut moved = Vec::new();
        for r in &p.index.rows {
            let old = r.state != weft_core::sessions::State::Running && r.last < before;
            if !old || !weft_core::sessions::is_id(&r.id) || working.contains(&r.id) {
                continue;
            }
            let file = format!("{}.jsonl", r.id);
            let (from, to) = (dir.join(&r.harness).join(&file), archive.join(key).join(&r.harness));
            if std::fs::create_dir_all(&to).is_ok()
                && std::fs::rename(&from, to.join(&file)).is_ok()
            {
                moved.push(from);
            }
        }
        for harness in moved.iter().filter_map(|f| f.parent()) {
            let _ = std::fs::remove_dir(harness);
        }
        p.turns.forget(&moved);
    }

    /// Evaluate `weft-sense` Head 1 on each pane whose PTY output has been
    /// still for [`SENSE_SETTLE`] since Sense last read it. Looked at on the
    /// frames tick, never waited for. Until Sense has loaded nothing is read,
    /// so a screen that settled meanwhile is read once it has.
    pub(super) fn sense_what_settled(&mut self) {
        if self.sense.read().ok().is_none_or(|m| m.is_none()) {
            return;
        }
        for at in 0..self.projects.len() {
            let p = &self.projects[at];
            if p.closed {
                continue;
            }
            let quiet: Vec<u32> = p
                .panes
                .iter()
                .filter(|s| {
                    settled(s.pane.current_seq(), s.sense_settled_seq, s.last_output_at.elapsed())
                })
                .map(|s| s.map.pane)
                .collect();
            // Receipts are written only on a signal, which reads them at
            // once: the turns are current.
            for pane in &quiet {
                self.sense_pane(at, *pane);
            }
            // A screen still long enough for Sense to speak (a gap its
            // harness names, or a pane no hook has spoken for) is told once,
            // as no output or hook will tell it.
            let p = &mut self.projects[at];
            let mut still = false;
            for slot in p.panes.iter_mut() {
                let seq = slot.pane.current_seq();
                let current = slot.sensed_surface.is_some_and(|r| r.seq == seq);
                if current
                    && slot.gap_told != seq
                    && slot.last_output_at.elapsed() >= crate::turns::GAP_QUIET
                {
                    slot.gap_told = seq;
                    still = true;
                }
            }
            if !quiet.is_empty() || still {
                self.tell_agents(at);
            }
        }
    }

    /// Evaluate `weft-sense` Head 1 on `pane` now, when a hook signal
    /// arrives, its receipt already read.
    pub(super) fn sense_pane_now(&mut self, at: usize, pane: u32) {
        self.sense_pane(at, pane);
    }

    /// Evaluate `weft-sense` Head 1 on `pane`'s screen, off the run loop: a
    /// read takes a fifth of a second. Nothing while Sense has not loaded,
    /// while a read of this pane is under way, or when its screen reads as
    /// the words Sense last read.
    fn sense_pane(&mut self, at: usize, pane: u32) {
        let model = Arc::clone(&self.sense);
        let p = &mut self.projects[at];
        let Some(slot) = p.panes.iter_mut().find(|s| s.map.pane == pane) else { return };
        if slot.sensing {
            return;
        }
        let crate::pane::Surface {
            alt: alt_screen,
            size: (rows, cols),
            cursor: (cursor_row, cursor_col),
            bottom: bottom_lines,
            seq,
        } = slot.pane.surface_snapshot();
        let ms_since_last_output = slot.last_output_at.elapsed().as_millis().max(150) as u64;
        let text = model.read().ok().and_then(|guard| {
            Some(guard.as_ref()?.screen_text(&weft_sense::PaneSnapshot {
                harness: &slot.map.harness,
                alt_screen,
                rows,
                cols,
                cursor_row,
                cursor_col,
                ms_since_last_output,
                bottom_lines: &bottom_lines,
                seq,
            }))
        });
        let Some(text) = text else { return };
        slot.sense_settled_seq = seq;
        // A blank screen holds nothing to read: it reads as ready, which
        // counts only once it has stayed still (a harness draws within a
        // second), so a harness that never draws is not waited on forever.
        if bottom_lines.is_empty() {
            let state = weft_sense::SurfaceState::IdleReady;
            slot.sensed_surface =
                Some(weft_sense::SurfaceReading { state, confidence_bp: 10000, seq });
            slot.sensed_text = None;
            return;
        }
        // The same words read the same: the reading holds for this screen too.
        if slot.sensed_text.as_deref() == Some(text.as_str()) {
            if let Some(reading) = slot.sensed_surface.as_mut() {
                reading.seq = seq;
            }
            return;
        }
        slot.sensing = true;
        let (root, tx) = (p.root.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let read = |m: &weft_sense::SenseModel| m.read_screen(&text, seq);
            let reading = model.read().ok().and_then(|guard| guard.as_ref().and_then(read));
            let _ = tx.send(super::Wake::Sensed { root, pane, text, reading });
        });
    }

    /// Sense's reading of a pane's screen, kept and told; then the pane is
    /// looked at again, as a hook or its screen may have moved meanwhile.
    pub(super) fn sensed(
        &mut self,
        root: &Path,
        pane: u32,
        text: String,
        reading: Option<weft_sense::SurfaceReading>,
    ) {
        let Some(at) = self.projects.iter().position(|p| p.root == root && !p.closed) else {
            return;
        };
        let Some(slot) = self.projects[at].panes.iter_mut().find(|s| s.map.pane == pane) else {
            return;
        };
        slot.sensing = false;
        slot.sensed_text = Some(text);
        slot.sensed_surface = reading;
        self.sense_pane(at, pane);
        self.tell_agents(at);
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
        let file = self.projects[at].harnesses.find(harness);
        // Resuming joins it where the harness says so, unless a Weft pane
        // holds it already: that pane, not another.
        let joins = row.pane.is_none() && file.is_some_and(|h| h.resume_joins);
        if row.state == weft_core::sessions::State::Running && !joins {
            let held = row.pane.map_or("another program".to_string(), |p| format!("pane {p}"));
            return Err(Answer::No("running", format!("{id} is running, in {held}")));
        }
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
        self.states_of(at).get(i).copied().flatten()
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
        self.spawn_in(client, project, harness, spec, resumed, None)
    }

    /// [`Self::spawn`], started in `dir` instead of the project's root.
    pub(super) fn spawn_in(
        &mut self,
        client: u64,
        project: usize,
        harness: &str,
        spec: &str,
        resumed: Option<String>,
        dir: Option<&std::path::Path>,
    ) -> Option<u32> {
        let p = self.projects.get_mut(project)?;
        let cwd = dir.unwrap_or(&p.root).to_string_lossy().into_owned();
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
        // A device never sizes an agent, not even one it started: the agent
        // takes a window's size once a window shows it (ADR-0038).
        let wants = if self.clients.named(client, "weft-remote") {
            HashMap::new()
        } else {
            HashMap::from([(client, size)])
        };
        // Its hooks tell this daemon what it is doing, from inside this pane.
        let Some(token) = new_token() else {
            let refusal = Some("the system gave no randomness for the pane's token".into());
            self.clients.broadcast(project, &Out::Injected { pane: id, refusal, unrecorded: None });
            return None;
        };
        if let Some(h) = p.harnesses.find(harness) {
            crate::harness::name_session_in_title(h);
        }
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
                // Every byte a pane prints goes to the session, which forwards
                // it, numbered, to whoever is attached.
                let sink = pane.stream_output();
                std::thread::spawn(move || {
                    while let Ok((seq, bytes)) = sink.recv() {
                        if tx.send(Wake::Output { project, pane: id, seq, bytes }).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Wake::Exited { project, pane: id });
                });
                let now = crate::turns::now_millis();
                self.projects[project].panes.push(Slot {
                    map: crate::turns::Started::new(id, harness, spec, now, resumed),
                    pane,
                    size,
                    wants,
                    turbo,
                    token,
                    transcript: None,
                    files: Default::default(),
                    last_output_at: std::time::Instant::now(),
                    // Read once settled, even a pane that never draws.
                    sense_settled_seq: u64::MAX,
                    sensed_surface: None,
                    sensed_text: None,
                    sensing: false,
                    gap_told: 0,
                    typer: None,
                    judge: None,
                    folder: None,
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
        // Its harness's own stream gives this session's turn.
        if self.streamed.contains(&(sent.harness.clone(), sent.session.clone())) {
            sent.state = None;
        }
        once(&mut self.forwarded, &mut sent);
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
                let ours = self.projects[at]
                    .panes
                    .iter()
                    .any(|s| s.map.pane == pane && s.map.harness == sent.harness);
                if ours {
                    self.tie(at, pane, &sent.session);
                    // Where its agent says it works: its Ask sheet's plans.
                    if let Some(dir) = &sent.workspace
                        && let Some(slot) = self.projects[at].slot(pane)
                    {
                        slot.folder = Some(dir.into());
                    }
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
                // A session a pane holds is that pane's: only its own signals
                // move it. A hook from the harness's shared server names no
                // pane, so for that session it is the pane's all the same.
                let held = at.and_then(|at| {
                    self.projects[at].panes.iter().find(|s| {
                        s.map.running
                            && s.map.session.as_ref().is_some_and(|b| b.session == sent.session)
                    })
                });
                if let Some(slot) = held {
                    if !sent.shared {
                        return;
                    }
                    sent.pane = Some(slot.map.pane);
                } else if sent.shared
                    && let Some(at) = at
                {
                    sent.pane = self.titled(at, &sent.harness, &sent.session);
                }
                (at, root)
            }
        };
        // The transcript a hook names is its pane's, for its Conversation.
        if let (Some(at), Some(pane)) = (at, sent.pane)
            && let Some(slot) = self.projects[at]
                .panes
                .iter_mut()
                .find(|s| s.map.pane == pane && s.map.harness == sent.harness)
            && let Some(path) = sent.transcript.take()
        {
            slot.transcript = Some(path.into());
        }
        if let Some(at) = at {
            self.listen(at, &sent.harness);
        }
        // A turn that ended: whether its last reply asks the person, by Sense.
        if let Some(said) = sent.said.take() {
            let model = self.sense.read().ok();
            sent.asked = model.as_ref().and_then(|m| m.as_ref()?.turn_end(&said)).map(|t| t.asks);
        }
        self.write_receipt(&root, &sent);
        // A shared server's open stream speaks of every session it holds,
        // this one too, even if its turn began before the stream opened.
        if let Some(at) = at.filter(|_| sent.shared && self.streams.contains(&sent.harness))
            && self.streamed.insert((sent.harness.clone(), sent.session.clone()))
        {
            self.projects[at].index.stale = true;
        }
        if let Some(at) = at {
            if let Some(pane) = sent.pane {
                self.sense_pane_now(at, pane);
            }
            self.tell_agents(at);
        }
        // A judge's prompts and tools are no evidence of the person's work.
        let judge = at.zip(sent.pane).is_some_and(|(at, pane)| {
            self.projects[at].panes.iter().any(|s| s.map.pane == pane && s.judge.is_some())
        });
        if judge {
            if let (Some(at), Some(pane)) = (at, sent.pane)
                && matches!(sent.state.as_deref(), Some("turn_ended" | "ended"))
            {
                self.judge_turn_ended(at, pane);
            }
            return;
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
    /// Tie a pane to the session it runs, in Weft's pane map.
    /// The person typed into a pane whose turn ended asking them: the
    /// question is being answered, so it no longer needs them.
    pub(super) fn answered(&mut self, at: usize, pane: u32) {
        let p = &self.projects[at];
        let Some(slot) = p.panes.iter().find(|s| s.map.pane == pane) else { return };
        let Some(bound) = slot.map.session.as_ref() else { return };
        let (harness, session) = (slot.map.harness.clone(), bound.session.clone());
        let asked = p.turns.session(&harness, &session).is_some_and(|s| s.asked);
        if !asked {
            return;
        }
        let sent = weft_core::signal::Sent {
            harness,
            hook: "typed".into(),
            session,
            asked: Some(false),
            ..Default::default()
        };
        let root = p.root.clone();
        self.write_receipt(&root, &sent);
        self.tell_agents(at);
    }

    /// Record a signal in Weft's receipts and, when it added a line, in the
    /// turns of the open project it belongs to. weftd writes every receipt,
    /// so this is how the turns follow them without reading the folder.
    fn write_receipt(&mut self, root: &Path, sent: &weft_core::signal::Sent) {
        let now = crate::turns::now_millis();
        if !matches!(crate::receipts::record(&self.receipts, root, sent, now), Ok(true)) {
            return;
        }
        let dir = crate::receipts::project_dir(&self.receipts, root);
        for p in self.projects.iter_mut().filter(|p| !p.closed && p.turns.dir() == dir) {
            p.turns.reread(&sent.harness, &sent.session);
        }
    }

    pub(super) fn tie(&mut self, at: usize, pane: u32, session: &str) {
        let p = &mut self.projects[at];
        let mut map: Vec<_> = p.panes.iter().map(|s| s.map.clone()).collect();
        if crate::turns::signalled(&mut map, pane, session) {
            for (slot, m) in p.panes.iter_mut().zip(map) {
                slot.map = m;
            }
            self.write_map(at);
        }
        self.judge_session(at, pane, session);
    }

    /// The pane whose window title names `session`, tied to it: under a
    /// harness's shared server no hook names its pane, and a pane that began
    /// another session in place says so only there.
    fn titled(&mut self, at: usize, harness: &str, session: &str) -> Option<u32> {
        let p = &mut self.projects[at];
        let panes: Vec<_> = p
            .panes
            .iter_mut()
            .filter(|s| s.judge.is_none())
            .map(|s| weft_core::turns::Started { running: s.pane.running(), ..s.map.clone() })
            .collect();
        let p = &self.projects[at];
        let title = |pane: u32| p.panes.iter().find(|s| s.map.pane == pane)?.pane.title();
        let pane = weft_core::turns::titled(&panes, harness, session, &title)?;
        self.tie(at, pane, session);
        Some(pane)
    }

    /// Open a harness's own stream, once, when its harness file names one
    /// and its server's socket is there; with none, hooks alone apply. It is
    /// opened again on the next pane or hook after its server closes it,
    /// never on a timer.
    pub(super) fn listen(&mut self, at: usize, harness: &str) {
        use crate::harness::OnThisMachine;
        if self.streams.contains(harness) {
            return;
        }
        let Some(h) = self.projects.get(at).and_then(|p| p.harnesses.find(harness)) else { return };
        let Some(listen) = h.listen.clone() else { return };
        let socket = h.config_home().join(&listen.socket);
        // An agent just started may be the one that starts its harness's
        // server: its socket is waited for while the agent starts, so the
        // stream is open when the server says the agent's session began.
        let starting = self.projects[at]
            .panes
            .iter()
            .any(|s| s.map.harness == harness && s.map.session.is_none() && s.map.running);
        if !socket.exists() && !starting {
            return;
        }
        self.streams.insert(harness.to_string());
        let (tx, harness) = (self.tx.clone(), harness.to_string());
        std::thread::spawn(move || {
            let until = std::time::Instant::now() + super::typing::STARTED_TIMEOUT;
            while !socket.exists() && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            eprintln!("weft: listening to {harness}'s stream at {}", socket.display());
            let ended = crate::listen::run(&socket, &listen, |heard| {
                let _ = tx.send(Wake::Heard { harness: harness.clone(), heard });
            });
            match ended {
                Ok(()) => eprintln!("weft: {harness}'s stream closed"),
                Err(e) => eprintln!("weft: {harness}'s stream ended: {e}"),
            }
            let _ = tx.send(Wake::Unheard { harness });
        });
    }

    /// A session's turn as its harness's stream said it, recorded as a hook's
    /// would be, in the open project whose receipts already hold the session.
    /// The stream names no workspace, so a session no hook tied to a project
    /// is no one's.
    pub(super) fn heard(&mut self, harness: String, heard: weft_core::listen::Heard) {
        let weft_core::listen::Heard { session, state, workspace } = heard;
        if !weft_core::sessions::is_id(&harness) || !weft_core::sessions::is_id(&session) {
            return;
        }
        let file = |root: &Path| {
            crate::receipts::project_dir(&self.receipts, root)
                .join(&harness)
                .join(format!("{session}.jsonl"))
        };
        let known = self.projects.iter().position(|p| !p.closed && file(&p.root).exists());
        let Some(at) = known.or_else(|| self.began(&harness, &session, workspace.as_deref()))
        else {
            return;
        };
        if self.streamed.insert((harness.clone(), session.clone())) {
            self.projects[at].index.stale = true;
        }
        let ended = matches!(state.as_str(), "turn_ended" | "ended");
        let root = self.projects[at].root.clone();
        let sent = weft_core::signal::Sent {
            harness,
            session: session.clone(),
            state: Some(state),
            ..Default::default()
        };
        self.write_receipt(&root, &sent);
        let holds = |s: &&Slot| s.map.session.as_ref().is_some_and(|b| b.session == session);
        let held_pane = self.projects[at].panes.iter().find(holds).map(|s| s.map.pane);
        let held_pane = held_pane.or_else(|| self.titled(at, &sent.harness, &session));
        if let Some(pane) = held_pane {
            self.sense_pane_now(at, pane);
        }
        self.tell_agents(at);
        // A send into the pane that holds it was waiting for this turn's end.
        if ended && let Some(pane) = held_pane {
            self.received_nothing(at, pane);
            self.judge_turn_ended(at, pane);
        }
    }
}

impl Session {
    /// A session no hook has named yet, begun in an open project's
    /// directory: the one agent of its harness there that runs no session
    /// yet is the one that began it, and is tied to it. With none, or two
    /// that could be it, nothing is tied.
    fn began(&mut self, harness: &str, session: &str, workspace: Option<&str>) -> Option<usize> {
        let at = workspace.map(Path::new).and_then(|dir| {
            let dir = dir.canonicalize().ok()?;
            self.projects
                .iter()
                .position(|p| !p.closed && p.root.canonicalize().ok().as_deref() == Some(&*dir))
        })?;
        let fresh: Vec<u32> = self.projects[at]
            .panes
            .iter_mut()
            .filter_map(|s| {
                let fresh = s.map.harness == harness && s.map.session.is_none();
                (fresh && s.pane.running()).then_some(s.map.pane)
            })
            .collect();
        let [pane] = fresh[..] else { return None };
        self.tie(at, pane, session);
        Some(at)
    }
}

impl Session {
    /// The project whose plans `pane`'s agent reads: the folder it last said
    /// it works in, under this project's root ([`crate::ringframe::plan_scope`]).
    pub(super) fn plan_scope(&self, at: usize, pane: u32) -> Option<String> {
        let p = &self.projects[at];
        let slot = p.panes.iter().find(|s| s.map.pane == pane)?;
        crate::ringframe::plan_scope(&p.root, slot.folder.as_deref()?)
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

impl Session {
    /// `read conversation`: a pane's newest page, the page before a cursor,
    /// or what was added after one. A harness without `[conversation]`, or no
    /// file known, found or allowed, reads as unreadable, never refused.
    pub(super) fn conversation(
        &mut self,
        at: usize,
        pane: Option<u32>,
        before: Option<String>,
        after: Option<String>,
    ) -> Answer {
        use crate::conversation as c;
        use crate::harness::OnThisMachine;
        use weft_core::conversation::Cursor;
        let p = &self.projects[at];
        let Some(slot) = pane.and_then(|n| p.panes.iter().find(|s| s.map.pane == n)) else {
            return Answer::No("no_pane", "there is no such pane here".into());
        };
        let cursor = |t: Option<String>| t.map(|t| Cursor::of(&t).ok_or(())).transpose();
        let (Ok(before), Ok(after)) = (cursor(before), cursor(after)) else {
            return Answer::No("bad_request", "that cursor is not one weftd gave".into());
        };
        let Some(h) = p.harnesses.find(&slot.map.harness) else {
            return Answer::Ok(c::Answer::Unreadable.json());
        };
        let Some(mapping) = h.conversation.as_ref() else {
            return Answer::Ok(c::Answer::Unreadable.json());
        };
        let session = slot.map.session.as_ref().map(|b| b.session.as_str());
        let path =
            session.and_then(|s| c::file(mapping, &h.config_home(), s, slot.transcript.as_deref()));
        let sense_guard = self.sense.read().ok();
        let sense = sense_guard.as_ref().and_then(|g| g.as_ref());
        let mut read = match after {
            Some(after) => {
                c::after_with_sense(mapping, &slot.map.harness, path.as_deref(), after, sense)
            }
            None => c::page_with_sense(mapping, &slot.map.harness, path.as_deref(), before, sense),
        };
        drop(sense_guard);
        // The files its items point at, found on the disk now: a device may
        // read only these.
        if let c::Answer::Page { items, .. } | c::Answer::Grown { items, .. } = &mut read {
            let (root, home) = (p.root.clone(), std::env::var_os("HOME").map(PathBuf::from));
            let base = crate::files::Base { root: &root, home: home.as_deref() };
            let pane = slot.map.pane;
            if let Some(slot) = self.projects[at].panes.iter_mut().find(|s| s.map.pane == pane) {
                crate::files::refs(items, &base, &mut slot.files);
            }
        }
        Answer::Ok(read.json())
    }

    /// Where a file the pane's conversation pointed at (`unit`) is.
    pub(super) fn file_located(&self, at: usize, pane: Option<u32>, unit: &str) -> Answer {
        let p = &self.projects[at];
        let Some(slot) = pane.and_then(|n| p.panes.iter().find(|s| s.map.pane == n)) else {
            return Answer::No("no_pane", "there is no such pane here".into());
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let base = crate::files::Base { root: &p.root, home: home.as_deref() };
        match crate::files::located(&base, &slot.files, unit) {
            Some(v) => Answer::Ok(v),
            None => Answer::No("no_file", "that file is not one this agent pointed at".into()),
        }
    }
}

/// A line of a file is forwarded once per session: a later hook that finds
/// the same newest line has received nothing new, and a record of the
/// submission already forwarded is not one again.
fn once(forwarded: &mut std::collections::HashSet<String>, sent: &mut weft_core::signal::Sent) {
    let mut first = |key: Option<String>| {
        key.is_none_or(|key| forwarded.insert(format!("{}\0{}\0{key}", sent.harness, sent.session)))
    };
    if !first(sent.prompt_key.take()) {
        sent.prompt = None;
        sent.also.clear();
    }
    sent.also.retain_mut(|r| first(r.key.take()));
}

/// The gaps `harness`'s file names: the moves Sense may make on its panes.
fn gaps_of<'a>(
    harnesses: &'a crate::harness::Harnesses,
    harness: &str,
) -> &'a [weft_core::signal::Gap] {
    harnesses.find(harness).and_then(|h| h.signal.as_ref()).map_or(&[], |s| s.gaps.as_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use weft_core::signal::{Received, Sent};

    /// Sense reads a pane's output once, when it has been still for the
    /// settle time, and not again until it prints more.
    #[test]
    fn output_is_read_once_it_has_been_still_and_not_again() {
        let still = SENSE_SETTLE;
        assert!(settled(5, 4, still), "printed since the last reading, and still");
        assert!(!settled(5, 4, still / 2), "still printing");
        assert!(!settled(5, 5, still * 10), "already read");
    }

    #[test]
    fn a_line_is_forwarded_once_as_the_prompt_or_as_another_record() {
        let mut forwarded = std::collections::HashSet::new();
        let line = |key: &str| Received { text: "ship it".into(), key: Some(key.into()) };
        let hook = |also: Vec<Received>| Sent {
            harness: "claude-code".into(),
            session: "s1".into(),
            prompt: Some("Set: ship it".into()),
            also,
            ..Sent::default()
        };
        let mut sent = hook(vec![line("/r:9")]);
        once(&mut forwarded, &mut sent);
        assert_eq!(sent.also.len(), 1, "first seen");
        let mut sent = hook(vec![line("/r:9"), line("/r:20")]);
        once(&mut forwarded, &mut sent);
        assert_eq!(sent.prompt.as_deref(), Some("Set: ship it"), "the hook's own text is no line");
        assert_eq!(
            sent.also,
            [Received { text: "ship it".into(), key: None }],
            "only the new line"
        );
        let mut sent =
            Sent { prompt_key: Some("/r:20".into()), also: vec![line("/r:31")], ..hook(vec![]) };
        once(&mut forwarded, &mut sent);
        assert_eq!(
            (sent.prompt, sent.also.len()),
            (None, 0),
            "a line forwarded already, and nothing after it"
        );
        let mut sent = hook(vec![line("/r:31")]);
        once(&mut forwarded, &mut sent);
        assert_eq!(sent.also.len(), 1, "a line not forwarded with nothing is still new");
    }
}

/// The session index, its pages, its archive and first prompts, in a daemon
/// of their own.
#[cfg(test)]
mod index_tests {
    use std::time::{Duration, Instant};

    use weft_core::sessions::{Row, State};

    use super::*;

    const DAY: i64 = 24 * 60 * 60 * 1000;

    /// A daemon of its own, with one project in it, and the project's
    /// sessions as its answers and events have told them.
    struct Daemon {
        dir: tempfile::TempDir,
        root: PathBuf,
        receipts: PathBuf,
        writer: UnixStream,
        reader: Lines<UnixStream>,
        next: u64,
        rows: Vec<Row>,
        earlier: usize,
    }

    impl Daemon {
        fn start() -> Self {
            Self::with_config("")
        }

        fn with_config(config: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let weft_dir = dir.path().join("weft");
            let root = dir.path().join("proj");
            std::fs::create_dir_all(&weft_dir).unwrap();
            std::fs::create_dir_all(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let (sock, cfg) = (weft_dir.join("weft.sock"), weft_dir.join("config.toml"));
            std::fs::write(&cfg, config).unwrap();
            let world = crate::outside::Fixed::new()
                .with_release(Err("no release in this world".into()))
                .shared();
            let harnesses = weft_core::harness::fixture::harnesses();
            let (sock_srv, cfg_srv) = (sock.clone(), cfg.clone());
            std::thread::spawn(move || {
                let _ = Session::serve_in(&sock_srv, &cfg_srv, Some(harnesses), world);
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while !weft_proto::is_live(&sock) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let stream = UnixStream::connect(&sock).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut daemon = Daemon {
                dir,
                root,
                receipts: crate::receipts::base_of(&cfg),
                reader: Lines::new(stream.try_clone().unwrap()),
                writer: stream,
                next: 1,
                rows: Vec::new(),
                earlier: 0,
            };
            daemon.call(Call::Hello { client: "test".into(), protocol: PROTOCOL }).unwrap();
            daemon
        }

        /// A call's answer, or its refusal's code.
        fn call(&mut self, call: Call) -> Result<serde_json::Value, String> {
            self.call_seeing(call).0
        }

        /// A call's answer, and the events that came before it, each taken.
        fn call_seeing(&mut self, call: Call) -> (Result<serde_json::Value, String>, Vec<Out>) {
            let id = self.next;
            self.next += 1;
            weft_proto::send(&mut self.writer, &Line::Call { id, call }.encode()).unwrap();
            let mut events = Vec::new();
            loop {
                match self.reader.next() {
                    Ok(Some(Line::Result { id: got, result })) if got == id => {
                        return (Ok(result), events);
                    }
                    Ok(Some(Line::Error { id: got, code, .. })) if got == id => {
                        return (Err(code), events);
                    }
                    Ok(Some(Line::Event(ev))) => {
                        self.take(&ev);
                        events.push(ev);
                    }
                    Ok(Some(_)) => {}
                    other => panic!("no answer to call {id}: {other:?}"),
                }
            }
        }

        fn take(&mut self, ev: &Out) {
            if let Out::Agents { changed, gone, earlier, .. } = ev {
                weft_core::sessions::apply(&mut self.rows, changed, gone);
                self.earlier = *earlier;
            }
        }

        /// The project's sessions carried as they are, as opening it answers
        /// them.
        fn open(&mut self) -> Vec<Row> {
            let path = self.root.display().to_string();
            let opened = self.call(Call::Open { path, rows: 24, cols: 80, raw: false }).unwrap();
            self.rows = serde_json::from_value(opened["agents"]["sessions"].clone()).unwrap();
            self.earlier = opened["agents"]["earlier"].as_u64().unwrap() as usize;
            self.rows.clone()
        }

        /// One page of the earlier sessions.
        fn page(&mut self, after: Option<&str>, q: Option<&str>) -> serde_json::Value {
            self.call(Call::Read {
                what: "sessions".into(),
                unit: String::new(),
                pane: None,
                lines: None,
                before: None,
                after: after.map(str::to_string),
                q: q.map(str::to_string),
            })
            .unwrap()
        }

        /// Wait until the sessions, as told, hold.
        fn told(&mut self, within: Duration, holds: impl Fn(&[Row]) -> bool) -> bool {
            let until = Instant::now() + within;
            while Instant::now() < until {
                if holds(&self.rows) {
                    return true;
                }
                if let Ok(Some(Line::Event(ev))) = self.reader.next() {
                    self.take(&ev);
                }
            }
            holds(&self.rows)
        }

        /// A receipt, written as if by weftd, but behind this one's back.
        fn receipt(&self, session: &str, state: &str, at: i64, pid: Option<(u32, String)>) {
            let sent = weft_core::signal::Sent {
                harness: "claude-code".into(),
                session: session.into(),
                state: Some(state.into()),
                pid: pid.as_ref().map(|p| p.0),
                pid_start: pid.map(|p| p.1),
                ..Default::default()
            };
            assert!(crate::receipts::record(&self.receipts, &self.root, &sent, at).unwrap());
        }

        fn signal(&mut self, sent: weft_core::signal::Sent) {
            let line = Line::Event(Out::Signal(Box::new(sent))).encode();
            weft_proto::send(&mut self.writer, &line).unwrap();
        }

        fn receipt_file(&self, session: &str) -> PathBuf {
            crate::receipts::project_dir(&self.receipts, &self.root)
                .join(format!("claude-code/{session}.jsonl"))
        }

        fn archived_file(&self, session: &str) -> PathBuf {
            let key = weft_core::sessions::project_key(&self.root);
            self.dir.path().join(format!("weft/archive/sessions/{key}/claude-code/{session}.jsonl"))
        }
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = weft_proto::send(
                &mut self.writer,
                &Line::Call { id: u64::MAX, call: Call::Shutdown }.encode(),
            );
        }
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> Option<&'a Row> {
        rows.iter().find(|r| r.id == id)
    }

    fn now() -> i64 {
        crate::turns::now_millis()
    }

    #[test]
    fn an_ended_sessions_receipt_is_read_when_the_project_opens_and_not_on_a_tick() {
        let mut d = Daemon::start();
        d.receipt("s-old", "ended", now(), None);
        let rows = d.open();
        assert_eq!(row(&rows, "s-old").map(|r| r.state), Some(State::Ended));
        // Changed behind weftd's back: no tick reads the folder, so it stays
        // as weftd last read it.
        d.receipt("s-old", "working", now(), None);
        std::thread::sleep(Duration::from_millis(1_000));
        let rows = d.open();
        let old = row(&rows, "s-old").unwrap();
        assert_eq!((old.state, old.turn), (State::Ended, None), "{rows:?}");
    }

    #[test]
    fn a_receipt_weftd_writes_moves_its_session_at_once() {
        let mut d = Daemon::start();
        assert!(d.open().is_empty());
        d.signal(weft_core::signal::Sent {
            harness: "claude-code".into(),
            session: "s-new".into(),
            state: Some("working".into()),
            workspace: Some(d.root.display().to_string()),
            ..Default::default()
        });
        // The signal comes before the call on the same connection, so the
        // daemon has told it by the time it answers: no tick in between.
        let (_, events) = d.call_seeing(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let told = events.iter().any(|e| {
            matches!(e, Out::Agents { changed, .. }
                if row(changed, "s-new").is_some_and(|r| r.turn == Some(Turn::Working)))
        });
        assert!(told, "{events:?}");
    }

    #[test]
    fn a_process_found_gone_ends_its_session() {
        let mut d = Daemon::start();
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let start = crate::process::info(child.id()).unwrap().start;
        d.receipt("s-run", "working", now(), Some((child.id(), start)));
        let rows = d.open();
        assert_eq!(row(&rows, "s-run").map(|r| r.state), Some(State::Running), "{rows:?}");
        child.kill().unwrap();
        child.wait().unwrap();
        let ended = d.told(Duration::from_secs(3), |rows| {
            row(rows, "s-run").is_some_and(|r| r.state == State::Ended)
        });
        assert!(ended, "a tick asks the live process, finds it gone, and ends its session");
    }

    /// A first prompt whose read blocks (a FIFO stands for a system prompt
    /// over the folder) holds no call: the daemon answers meanwhile, and the
    /// title comes once the read does.
    #[test]
    fn a_first_prompt_read_that_blocks_holds_no_call() {
        use std::os::unix::fs::OpenOptionsExt;
        let mut d = Daemon::start();
        d.receipt("s-slow", "turn_ended", now(), None);
        let folder = d.root.join(".fab7/rf/sessions/claude-code/s-slow");
        std::fs::create_dir_all(&folder).unwrap();
        let fifo = folder.join("prompts.jsonl");
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid, NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(row(&d.open(), "s-slow").is_some());
        // Ticks go by: the read starts and blocks on the FIFO.
        std::thread::sleep(Duration::from_millis(600));
        for _ in 0..5 {
            let asked = Instant::now();
            d.call(Call::Hello { client: "test".into(), protocol: PROTOCOL }).unwrap();
            assert!(asked.elapsed() < Duration::from_secs(1), "answered while the read waits");
        }
        // A reader is waiting on it (opening it for writing without one fails).
        let mut w = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
            .expect("the read is under way");
        use std::io::Write as _;
        w.write_all(b"{\"text\":\"fix the flaky test\"}\n").unwrap();
        drop(w);
        let titled = d.told(Duration::from_secs(3), |rows| {
            row(rows, "s-slow").and_then(|r| r.title.as_deref()) == Some("fix the flaky test")
        });
        assert!(titled, "the title comes once the read does");
    }

    #[test]
    fn a_client_of_another_protocol_is_refused() {
        let mut d = Daemon::start();
        let refused = d.call(Call::Hello { client: "old".into(), protocol: PROTOCOL - 1 });
        assert_eq!(refused, Err("protocol".into()));
        assert_eq!(PROTOCOL, 11);
    }

    /// Opening carries the running and today's; the earlier ones come 50 at a
    /// time, newest first, and a search finds among them all.
    #[test]
    fn earlier_sessions_come_a_page_at_a_time_and_a_search_reaches_them_all() {
        let mut d = Daemon::start();
        let yesterday = now() - 2 * DAY;
        for i in 0..120 {
            d.receipt(&format!("old-{i:03}"), "turn_ended", yesterday + i * 1000, None);
        }
        d.receipt("today-1", "turn_ended", now(), None);
        let rows = d.open();
        assert_eq!((rows.len(), d.earlier), (1, 120), "{rows:?}");
        let first = d.page(None, None);
        assert_eq!(
            (first["total"].as_u64(), first["rows"].as_array().unwrap().len()),
            (Some(120), 50)
        );
        assert_eq!(first["rows"][0]["id"], "old-119", "newest first");
        let next = first["next"].as_str().unwrap().to_string();
        assert_eq!(next, "claude-code/old-070");
        let second = d.page(Some(&next), None);
        assert_eq!(second["rows"][0]["id"], "old-069");
        let third = d.page(second["next"].as_str(), None);
        assert_eq!(third["rows"].as_array().unwrap().len(), 20);
        assert!(third["next"].is_null(), "the last page");
        let found = d.page(None, Some(" OLD-01 "));
        let ids: Vec<_> =
            found["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(found["total"], 10, "{ids:?}");
        assert_eq!(ids.first(), Some(&"old-019"));
        assert_eq!(d.page(None, Some("today"))["total"], 0, "today's are not earlier");
    }

    /// A day of many sessions: opening carries only the newest 20 of the
    /// day's ended ones; the rest of the day comes first among the earlier.
    #[test]
    fn a_busy_day_carries_its_newest_twenty_and_pages_the_rest() {
        let mut d = Daemon::start();
        let at = now() - 60_000;
        for i in 0..45 {
            d.receipt(&format!("day-{i:02}"), "turn_ended", at + i * 100, None);
        }
        d.receipt("older", "turn_ended", now() - 2 * DAY, None);
        let rows = d.open();
        let cap = weft_core::sessions::TODAY_ENDED;
        assert_eq!((rows.len(), d.earlier), (cap, 45 - cap + 1), "{rows:?}");
        assert_eq!((rows[0].id.as_str(), rows[cap - 1].id.as_str()), ("day-44", "day-25"));
        let first = d.page(None, None);
        assert_eq!(first["total"], 26);
        assert_eq!(first["rows"][0]["id"], "day-24", "the day's older ones, then before");
        assert_eq!(first["rows"][25]["id"], "older");
    }

    /// An `agents` event carries only what moved: the row that changed, and
    /// the key of one that went; a client applying them holds what opening
    /// again answers.
    #[test]
    fn an_agents_event_carries_the_rows_that_changed_and_the_keys_that_went() {
        let mut d = Daemon::start();
        d.receipt("a", "turn_ended", now() - 60_000, None);
        d.receipt("b", "turn_ended", now() - 30_000, None);
        d.open();
        let token_file = d.dir.path().join("token");
        let script = d.dir.path().join("agent.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$WEFT_PANE_TOKEN\" > '{}'\nexec /bin/cat\n",
                token_file.display()
            ),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let spec = script.display().to_string();
        let started =
            d.call(Call::StartAgent { harness: "claude-code".into(), spec, session: None });
        let pane = started.unwrap()["pane"].as_u64().unwrap() as u32;
        let unnamed = format!("claude-code/pane {pane}");
        assert!(d.told(Duration::from_secs(3), |rows| rows.iter().any(|r| r.key() == unnamed)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !token_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let token = std::fs::read_to_string(&token_file).unwrap();
        // The pane names its session: its unnamed row goes, the session's comes.
        d.signal(weft_core::signal::Sent {
            pane: Some(pane),
            token: Some(token),
            harness: "claude-code".into(),
            session: "c".into(),
            state: Some("working".into()),
            ..Default::default()
        });
        let (_, events) = d.call_seeing(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let moved: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                Out::Agents { changed, gone, .. } => Some((changed.clone(), gone.clone())),
                _ => None,
            })
            .collect();
        let (changed, gone) = moved.last().expect("told at once");
        let keys: Vec<_> = changed.iter().map(Row::key).collect();
        assert_eq!(keys, ["claude-code/c"], "only the row that moved");
        assert_eq!(gone, &[unnamed]);
        let held: Vec<_> = d.rows.iter().map(Row::key).collect();
        assert_eq!(held, d.open().iter().map(Row::key).collect::<Vec<_>>());
    }

    /// Ended sessions quiet longer than `keep_days` are archived as the
    /// project opens, unless they are in work not yet sealed; RingFrame's
    /// files are left where they are.
    #[test]
    fn an_ended_session_quiet_past_its_days_is_archived_unless_its_work_is_open() {
        let mut d = Daemon::start();
        d.receipt("eight", "ended", now() - 8 * DAY, None);
        d.receipt("six", "ended", now() - 6 * DAY, None);
        d.receipt("working", "ended", now() - 8 * DAY, None);
        let rf = d.root.join(".fab7/rf");
        std::fs::create_dir_all(rf.join("sessions/claude-code/eight")).unwrap();
        std::fs::write(rf.join("sessions/claude-code/eight/prompts.jsonl"), "{}\n").unwrap();
        let compiled = serde_json::json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
            "time": "2026-09-19T14:02:00Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {
                "title": "still open", "selected_capability": "native_plan",
                "delivery_mode": "native_plan",
                "host": {"name": "claude-code", "profile_id": "claude-code",
                         "session_ref": "working", "session_ref_source": "capture"},
                "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                "source_verified": "exact", "limitations": [],
                "classification": {}, "route_explanation": {}
            }
        });
        std::fs::write(rf.join("ledger.jsonl"), format!("{compiled}\n")).unwrap();
        d.open();
        assert!(!d.receipt_file("eight").exists());
        assert!(d.archived_file("eight").exists(), "moved, to be moved back by hand");
        assert!(d.receipt_file("six").exists(), "within seven days");
        assert!(d.receipt_file("working").exists(), "its work is not sealed");
        assert!(rf.join("sessions/claude-code/eight/prompts.jsonl").exists(), "RingFrame's");
        let listed = d.page(None, None);
        let ids: Vec<_> =
            listed["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["six", "working"], "no longer listed");
    }

    #[test]
    fn zero_days_keeps_every_session() {
        let mut d = Daemon::with_config("[sessions]\nkeep_days = 0\n");
        d.receipt("old", "ended", now() - 400 * DAY, None);
        d.open();
        assert!(d.receipt_file("old").exists());
        assert_eq!(d.earlier, 1);
    }
}
