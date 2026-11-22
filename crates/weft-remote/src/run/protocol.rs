//! The remote protocol, from a device that said hello (remote.md
//! §4, §6, §9). Each message becomes only the call §4 allows, or is refused
//! here; a message with a `req` always gets `ok` or `error`.

use base64ct::{Base64, Encoding};
use serde_json::{Value, json};
use weft_proto::{Call, Event};

use super::{Asked, Carries, Gather, Gathering, PANES, Part, Remote};
use crate::agents;
use crate::socket::Screen;

/// How many lines of history a device is sent.
const HISTORY: u64 = 2_000;

/// Why a request was refused, for the app.
use crate::agents::Refusal;

/// Now, in milliseconds since the epoch.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Remote {
    pub(super) fn request(&mut self, conn: u64, m: &Value) {
        let req = m["req"].clone();
        let outcome = match m["type"].as_str() {
            Some("watch") => self.watch(conn, m),
            Some("size") => self.size(m),
            Some("keys") => self.keys(conn, m, &req),
            Some("text") => self.text(conn, m, &req),
            Some("act") => self.act_on(conn, m, &req),
            Some("start") => self.start_agent(conn, m, &req),
            Some("resolve") => self.resolve(conn, m, &req),
            Some("read") => self.read(conn, m, &req),
            Some("settings") => self.settings(conn, m, &req),
            Some("set") => self.set(conn, m, &req),
            Some("skip_check") => self.skip(conn, m, &req, Call::SkipCheck),
            Some("skip_add") => match (m["file"].as_str(), m["line"].as_str()) {
                (Some(file), Some(line)) => {
                    let call = Call::SkipAdd { file: file.into(), line: line.into() };
                    self.skip(conn, m, &req, call)
                }
                _ => Err(("bad_request", "skip_add names a file and a line".into())),
            },
            Some("seen") => self.seen(conn, m),
            Some("follow") => self.follow(conn, m),
            Some("push") => self.push_subscription(conn, m),
            Some("ping") => {
                self.send(conn, json!({"type": "pong", "t": m["t"]}));
                Ok(false)
            }
            _ => Err(("bad_request", "the remote protocol has no such message".into())),
        };
        match outcome {
            // A call went to the daemon; its answer answers the request.
            Ok(true) => {}
            Ok(false) if !req.is_null() => self.send(conn, json!({"type": "ok", "req": req})),
            Ok(false) => {}
            Err((code, message)) if !req.is_null() => self.error(conn, &req, code, &message),
            // Nothing to answer, and not allowed: the connection ends.
            Err(_) => self.close(conn),
        }
    }

    pub(super) fn error(&self, conn: u64, req: &Value, code: &str, message: &str) {
        self.send(conn, json!({"type": "error", "req": req, "code": code, "message": message}));
    }

    /// Make a call on a project's client; its call id, or nothing if the
    /// project is gone or the call is not allowed.
    pub(super) fn call(&mut self, at: usize, call: Call) -> Option<u64> {
        let open = self.projects.get_mut(at)?.as_mut()?;
        open.client.call(call).ok()
    }

    /// A call whose answer answers `req`. Whether it went.
    fn ask(
        &mut self,
        at: usize,
        call: Call,
        conn: u64,
        req: &Value,
        carries: Carries,
    ) -> Result<bool, Refusal> {
        let id = self.call(at, call).ok_or(("gone", "the project is not open".to_string()))?;
        if req.is_null() {
            return Ok(true);
        }
        self.calls.insert((at, id), Asked { conn, req: req.clone(), carries });
        Ok(true)
    }

    pub(super) fn answered(&mut self, at: usize, id: u64, answer: Result<Value, (String, String)>) {
        let Some(asked) = self.calls.remove(&(at, id)) else { return };
        if !asked.req.is_null() {
            let kept = (asked.conn, asked.req.clone(), std::time::Instant::now());
            self.answered.insert((at, id), kept);
        }
        if let Carries::Follow(conn, at, pane) = asked.carries {
            return self.followed(conn, (at, pane), answer.ok());
        }
        if let Carries::Part(g, part) = asked.carries {
            return self.gathered(g, part, answer.ok());
        }
        if asked.carries == Carries::Starts {
            let changed = match (&answer, self.projects.get_mut(at).and_then(Option::as_mut)) {
                (Ok(v), Some(o)) => o.agents.available(v),
                _ => false,
            };
            if changed {
                let harnesses = self.harnesses();
                self.to_devices(&json!({"type": "harnesses", "harnesses": harnesses}));
            }
            return;
        }
        match answer {
            Ok(v) => {
                let mut ok = json!({"type": "ok", "req": asked.req});
                match asked.carries {
                    Carries::Nothing => {}
                    Carries::Pending => ok["pending"] = v["pending"].clone(),
                    Carries::History(key) => {
                        ok["text"] = v["text"].clone();
                        ok["transcript"] = key.into();
                    }
                    Carries::Agent => {
                        let pane = v["pane"].as_u64().unwrap_or_default() as u32;
                        ok["agent"] = self.agent_id(at, pane).into();
                        if let Some(c) = self.conns.get_mut(&asked.conn) {
                            c.started.insert((at, pane));
                        }
                    }
                    Carries::Conversation(agent) => {
                        let (page, rest) = crate::conversation::page(&v["conversation"], &agent);
                        ok["conversation"] = page;
                        self.send(asked.conn, ok);
                        for m in rest {
                            self.send(asked.conn, m);
                        }
                        return;
                    }
                    Carries::Settings => ok["settings"] = v,
                    Carries::Skip => ok["findings"] = v["findings"].clone(),
                    Carries::Sessions => {
                        let Some(open) = self.projects.get_mut(at).and_then(Option::as_mut) else {
                            return;
                        };
                        ok["sessions"] = open.agents.sessions(&v, now_millis());
                    }
                    Carries::Sealed(what) => ok[what.as_str()] = v,
                    Carries::File => {
                        let f = &v["file"];
                        let device = self.conns.get(&asked.conn).and_then(|c| c.device.clone());
                        let (Some(abs), Some(device)) = (f["abs"].as_str(), device) else {
                            return self.error(asked.conn, &asked.req, "no_file", "not located");
                        };
                        let url = self.links.make(std::path::Path::new(abs), &device);
                        ok["file"] = json!({"path": f["path"], "kind": f["kind"], "size": f["size"],
                            "modified": f["modified"], "url": url});
                    }
                    Carries::Receipt(id) => {
                        let Some(open) = self.projects.get(at).and_then(Option::as_ref) else {
                            return;
                        };
                        match open.agents.receipt(&id, &v) {
                            Ok(r) => ok["receipt"] = r,
                            Err((code, message)) => {
                                return self.error(asked.conn, &asked.req, code, &message);
                            }
                        }
                    }
                    Carries::Starts | Carries::Part(..) | Carries::Follow(..) => {}
                    Carries::Resolve(..) => {}
                }
                self.send(asked.conn, ok);
            }
            Err((code, message)) => self.error(asked.conn, &asked.req, &code, &message),
        }
    }

    /// The agent a message names, as a project and a pane.
    fn agent(&self, m: &Value) -> Result<(usize, u32), Refusal> {
        let id = m["agent"].as_str().ok_or(("bad_request", "no agent".to_string()))?;
        self.find(id).ok_or(("no_agent", format!("there is no agent {id} here")))
    }

    pub(super) fn find(&self, id: &str) -> Option<(usize, u32)> {
        let (key, pane) = id.rsplit_once('-')?;
        let pane: u32 = pane.parse().ok()?;
        self.projects.iter().enumerate().find_map(|(at, o)| {
            let o = o.as_ref()?;
            (o.agents.key == key && o.agents.has(pane)).then_some((at, pane))
        })
    }

    pub(super) fn agent_id(&self, at: usize, pane: u32) -> String {
        let key = self.projects[at].as_ref().map(|o| o.agents.key.clone()).unwrap_or_default();
        agents::agent_id(&key, pane)
    }

    // --- watching ----------------------------------------------------------

    /// At most `PANES` agents per device, one to a pane: one no longer
    /// watched is let go, and one past the most is ignored.
    fn watch(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        // The agent, and those in the device's other panes: `also` is one, or
        // a list of them.
        let also = match &m["also"] {
            Value::Array(list) => list.iter().collect(),
            one => vec![one],
        };
        let mut next = Vec::new();
        for w in std::iter::once(m).chain(also).take(PANES) {
            if w.is_object() && !w["agent"].is_null() {
                let at = self.agent(w)?;
                if !next.iter().any(|(n, _)| *n == at) {
                    next.push((at, w));
                }
            }
        }
        let now: Vec<(usize, u32)> = next.iter().map(|(at, _)| *at).collect();
        let was = self
            .conns
            .get_mut(&conn)
            .map(|c| std::mem::replace(&mut c.watching, now.clone()))
            .unwrap_or_default();
        for gone in was.into_iter().filter(|w| !now.contains(w)) {
            self.unwatch(conn, gone);
        }
        // Watching changes nothing on the Mac: Weft owns an agent's size, and a
        // device draws the agent at it (remote.md §9).
        for ((at, pane), _) in next {
            if let Some(c) = self.conns.get_mut(&conn) {
                c.needs_full.insert((at, pane));
            }
            // Attached again: the daemon starts this pane's frames over with
            // a whole screen, which every device watching it can take.
            self.call(at, Call::Attach { pane, form: "screen".into() });
        }
        Ok(false)
    }

    /// Let a pane go when no device watches it any more.
    pub(super) fn unwatch(&mut self, conn: u64, (at, pane): (usize, u32)) {
        let still =
            self.conns.iter().any(|(id, c)| *id != conn && c.watching.contains(&(at, pane)));
        if !still {
            self.call(at, Call::Attach { pane, form: "none".into() });
        }
    }

    pub(super) fn screen(&mut self, at: usize, screen: Event) {
        let Event::Screen { pane, seq, full, rows, cols, bytes } = screen else { return };
        let agent = self.agent_id(at, pane);
        for c in self.conns.values_mut().filter(|c| c.watching.contains(&(at, pane))) {
            if c.needs_full.contains(&(at, pane)) && !full {
                continue;
            }
            let frame =
                Screen { agent: agent.clone(), seq, full, rows, cols, bytes: bytes.clone() };
            if c.outbox.frame(frame) {
                c.needs_full.remove(&(at, pane));
            }
        }
    }

    /// A device read again after its frames were dropped: start its pane
    /// over with a whole screen.
    pub(super) fn drained(&mut self, conn: u64) {
        let Some(c) = self.conns.get_mut(&conn) else { return };
        let watching = c.watching.clone();
        c.needs_full.extend(watching.iter().copied());
        for (at, pane) in watching {
            self.call(at, Call::Attach { pane, form: "screen".into() });
        }
    }

    // --- the keys ----------------------------------------------------------

    /// A device's size, from an app that still sends one before its keys:
    /// taken and changes nothing, since Weft owns an agent's size (remote.md §9).
    fn size(&mut self, m: &Value) -> Result<bool, Refusal> {
        self.agent(m)?;
        if dimension(m, "rows").is_none() || dimension(m, "cols").is_none() {
            return Err(("bad_request", "a size is rows and cols".into()));
        }
        Ok(false)
    }

    /// This device has the keys for this agent now.
    fn drive(&mut self, conn: u64, at: usize, pane: u32) {
        let agent = self.agent_id(at, pane);
        if let Some(c) = self.conns.get_mut(&conn)
            && c.driving.insert((at, pane))
        {
            c.outbox.send(json!({"type": "control", "agent": agent, "by": "you"}).to_string());
        }
    }

    fn keys(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let bytes = m["bytes"]
            .as_str()
            .and_then(|b| Base64::decode_vec(b).ok())
            .filter(|b| !b.is_empty())
            .ok_or(("bad_request", "keys are base64".to_string()))?;
        self.drive(conn, at, pane);
        self.ask(at, Call::Input { pane, bytes }, conn, req, Carries::Nothing)
    }

    /// A message: the daemon pastes it and presses Enter once the agent has
    /// taken it, as it does for the machine's own message field.
    fn text(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let text = m["text"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or(("bad_request", "no text".to_string()))?
            .to_string();
        self.drive(conn, at, pane);
        self.ask(at, Call::Say { pane, text }, conn, req, Carries::Nothing)
    }

    /// The person typed on the machine: every agent a device was driving is the
    /// machine's again (remote.md §9).
    pub(super) fn presence(&mut self) {
        let mut told = Vec::new();
        for c in self.conns.values_mut() {
            for (at, pane) in c.driving.drain() {
                told.push((c.outbox.clone(), at, pane));
            }
        }
        for (outbox, at, pane) in told {
            let agent = self.agent_id(at, pane);
            outbox.send(json!({"type": "control", "agent": agent, "by": "mac"}).to_string());
        }
        self.last_presence = Some(std::time::Instant::now());
    }

    // --- acts and pendings ---------------------------------------------------

    /// Ask to an agent, or Eval, Seal, Proceed or Follow up on a work: each
    /// becomes a pending the person answers.
    fn act_on(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        // A project reopened after an unclean end, dismissed.
        if m["act"] == "dismiss" {
            let at = self.project_named(m)?;
            return self.ask(at, Call::Dismiss, conn, req, Carries::Nothing);
        }
        let (at, pane) = match m["act"].as_str() {
            // Into a new agent of a harness the project starts (remote.md §6).
            Some("ask" | "follow_up") if m["start"].is_string() => (self.project_named(m)?, None),
            Some("ask" | "follow_up") => self.agent(m).map(|(at, pane)| (at, Some(pane)))?,
            // Eval and Seal act on a unit of work of a project (remote.md §6).
            Some("eval" | "seal") => (self.project_named(m)?, None),
            _ => {
                let id = m["unit"].as_str().ok_or(("bad_request", "no unit".to_string()))?;
                let at = self
                    .projects
                    .iter()
                    .position(|o| o.as_ref().is_some_and(|o| o.agents.unit(id).is_some()));
                (at.ok_or(("no_unit", format!("{id} is not on the board")))?, None)
            }
        };
        // Acted on again: an Eval of this work that could not open is said
        // no more, here or on any device.
        if let (Some(unit), Some(open)) =
            (m["unit"].as_str(), self.projects.get_mut(at).and_then(Option::as_mut))
        {
            let before = open.jobs.all().len();
            open.jobs.acted_on(unit);
            if open.jobs.all().len() != before {
                let told = open.jobs_message();
                self.to_devices(&told);
            }
        }
        let agents = &self.projects[at].as_ref().expect("an open project").agents;
        // No agent of the harness it goes to runs: say so, with the session
        // that route names, as AGENTS reads it, or none.
        if let Some((harness, session)) = agents.missing(m) {
            let message = format!("{harness} is not running.");
            self.send(
                conn,
                json!({"type": "error", "req": req, "code": "not_running", "message": message,
                                   "harness": harness, "session": session}),
            );
            return Ok(true);
        }
        let call = agents.act(m, pane)?;
        self.ask(at, call, conn, req, Carries::Pending)
    }

    // --- what takes a while ------------------------------------------------

    /// A job moved in a project: relayed to every device, with its request
    /// to the device whose request began it.
    pub(super) fn progress(&mut self, at: usize, job: weft_proto::Job) {
        let Some(open) = self.projects.get_mut(at).and_then(Option::as_mut) else { return };
        // The machine's jobs (Updates, Remote) are the desktop's to show.
        if matches!(job.kind.as_str(), "update" | "remote") {
            return;
        }
        // The device whose call began it, while the call waits or since it
        // was answered.
        let asker = job.call.and_then(|call| {
            match (self.calls.get(&(at, call)), self.answered.get(&(at, call))) {
                (Some(asked), _) if !asked.req.is_null() => Some((asked.conn, asked.req.clone())),
                (_, Some((conn, req, _))) => Some((*conn, req.clone())),
                _ => None,
            }
        });
        let relay = open.jobs.heard(job, asker, std::time::Instant::now());
        let project = open.agents.name.clone();
        if let Some(relay) = relay {
            self.relay(&project, relay);
        }
    }

    /// The counts held back that are due now, in every project.
    pub(super) fn jobs_due(&mut self) {
        let now = std::time::Instant::now();
        // A send waits for its agent's receipt up to ten minutes.
        const ANSWERED_KEPT: std::time::Duration = std::time::Duration::from_secs(15 * 60);
        self.answered.retain(|_, (_, _, at)| now.saturating_duration_since(*at) < ANSWERED_KEPT);
        let mut due = Vec::new();
        for open in self.projects.iter_mut().flatten() {
            let project = open.agents.name.clone();
            due.extend(open.jobs.due(now).into_iter().map(|r| (project.clone(), r)));
        }
        for (project, relay) in due {
            self.relay(&project, relay);
        }
    }

    fn relay(&self, project: &str, relay: crate::jobs::Relay) {
        let message = json!({"type": "progress", "project": project, "job": relay.job});
        let asker = relay.asker.as_ref().map(|(conn, _)| *conn);
        for (id, c) in self.conns.iter().filter(|(_, c)| c.device.is_some()) {
            let mut message = message.clone();
            if let Some((_, req)) = relay.asker.as_ref().filter(|_| asker == Some(*id)) {
                message["req"] = req.clone();
            }
            c.outbox.send(message.to_string());
        }
    }

    /// The project a message names by `project`.
    /// The machine's settings, as the project's daemon client has them; with
    /// `what`, what Settings reads of RingFrame through the daemon: `rules`,
    /// a `preview` or a harness's `profile`, by `unit`.
    fn settings(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let at = self.project_named(m)?;
        let call = match m["what"].as_str() {
            None => Call::ConfigGet,
            // What the remote holds itself: no call to make.
            Some("harnesses") => {
                let supported = self.projects[at].as_ref().map(|o| o.agents.supported());
                self.send(conn, json!({"type": "ok", "req": req, "settings": supported}));
                return Ok(true);
            }
            Some(what @ ("rules" | "preview" | "profile" | "ask")) => Call::Read {
                what: what.to_string(),
                unit: m["unit"].as_str().unwrap_or_default().to_string(),
                // The agent an Ask sheet is for, by its pane.
                pane: match m["agent"].is_null() {
                    true => None,
                    false => Some(self.agent(m)?.1),
                },
                lines: None,
                before: None,
                after: None,
                q: None,
            },
            Some(_) => {
                return Err((
                    "bad_request",
                    "settings reads rules, a preview, a profile or the harnesses".into(),
                ));
            }
        };
        self.ask(at, call, conn, req, Carries::Settings)
    }

    /// One setting changed, as the desktop changes it (weft-after-desktop
    /// ADR-0003 D6); every device is told what follows.
    fn set(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let at = self.project_named(m)?;
        let scope = m["scope"].as_str().ok_or(("bad_request", "set names a scope".to_string()))?;
        let key = m["key"]
            .as_array()
            .and_then(|k| k.iter().map(|p| p.as_str().map(str::to_string)).collect::<Option<_>>())
            .ok_or(("bad_request", "set names a key, as a list of names".to_string()))?;
        let value = m.get("value").filter(|v| !v.is_null()).cloned();
        let call = Call::ConfigSet { scope: scope.to_string(), key, value };
        if !crate::daemon::allowed(&call) {
            return Err(("not_allowed", "which program runs is set on the machine".into()));
        }
        self.ask(at, call, conn, req, Carries::Settings)
    }

    /// Which files an Eval should skip, checked again, or one line the
    /// daemon's check proposed written: answered with the project's findings.
    fn skip(&mut self, conn: u64, m: &Value, req: &Value, call: Call) -> Result<bool, Refusal> {
        let at = self.project_named(m)?;
        self.ask(at, call, conn, req, Carries::Skip)
    }

    fn project_named(&self, m: &Value) -> Result<usize, Refusal> {
        let project = m["project"].as_str().ok_or(("bad_request", "no project".to_string()))?;
        let at =
            self.projects.iter().position(|o| o.as_ref().is_some_and(|o| o.agents.name == project));
        at.ok_or(("no_project", format!("{project} is not open here")))
    }

    /// Start a fresh agent of a harness the project offers, or resume a
    /// session by its id (remote.md §6). Never "the latest".
    fn start_agent(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let Some(harness) = m["harness"].as_str() else {
            return Err(("bad_request", "start names a project and a harness".into()));
        };
        // A resume names a session's id, never "the latest".
        if !(m["resume"].is_null() || m["resume"].is_string()) {
            return Err(("bad_request", "resume names a session's id".into()));
        }
        let at = self.project_named(m)?;
        let agents = &self.projects[at].as_ref().expect("an open project").agents;
        let call = match m["resume"].as_str() {
            Some(id) => match agents.resume(harness, id) {
                Ok(call) => call,
                Err(((code, message), agent)) => {
                    self.send(
                        conn,
                        json!({"type": "error", "req": req, "code": code,
                                           "message": message, "agent": agent}),
                    );
                    return Ok(true);
                }
            },
            None => agents.start(harness)?,
        };
        self.ask(at, call, conn, req, Carries::Agent)
    }

    /// Yes or no. A yes is typed whatever the agent's state: the machine
    /// waits a little for one not ready, then types anyway.
    fn resolve(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let id = m["pending"].as_str().ok_or(("bad_request", "no pending".to_string()))?;
        let yes = m["yes"].as_bool().ok_or(("bad_request", "yes or no".to_string()))?;
        let at = self
            .pendings
            .get(id)
            .map(|(at, _)| *at)
            .ok_or(("gone", "that was already answered".to_string()))?;
        // A send can go to another agent here, or a new one (remote.md §6).
        let to = match m["to"].as_str() {
            Some(agent) => match self.find(agent) {
                Some((there, pane)) if there == at => Some(pane),
                _ => return Err(("no_agent", format!("there is no agent {agent} here"))),
            },
            None => None,
        };
        let start = m["start"].as_str().map(str::to_string);
        if let Some(harness) = &start {
            self.projects[at].as_ref().expect("an open project").agents.start(harness)?;
        }
        if (to.is_some() || start.is_some())
            && self.pendings.get(id).is_some_and(|(_, p)| p["sends"].is_null())
        {
            return Err(("not_a_send", "only an Ask's prompt can go to another agent".into()));
        }
        let opens =
            yes && self.pendings.get(id).is_some_and(|(_, p)| p["go"]["word"] == "act.eval");
        let call = Call::Resolve { pending: id.into(), yes, to, start };
        self.ask(at, call, conn, req, Carries::Resolve(id.into(), opens))
    }

    fn read(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let what = m["what"].as_str();
        if what == Some("conversation") {
            if req.is_null() {
                return Err(("bad_request", "a read is answered by its req".into()));
            }
            let (at, pane) = self.agent(m)?;
            let before = match &m["before"] {
                Value::Null => None,
                Value::String(b) => Some(b.clone()),
                _ => return Err(("bad_request", "before is a cursor".into())),
            };
            let call = Call::Read {
                what: "conversation".into(),
                unit: String::new(),
                pane: Some(pane),
                lines: None,
                before,
                after: None,
                q: None,
            };
            let agent = self.agent_id(at, pane);
            return self.ask(at, call, conn, req, Carries::Conversation(agent));
        }
        if what == Some("history") {
            let (at, pane) = self.agent(m)?;
            let call = Call::Read {
                what: "history".into(),
                unit: String::new(),
                pane: Some(pane),
                lines: Some(HISTORY),
                before: None,
                after: None,
                q: None,
            };
            let key = self.projects[at].as_ref().and_then(|o| o.agents.transcript(pane));
            return self.ask(at, call, conn, req, Carries::History(key));
        }
        if req.is_null() {
            return Err(("bad_request", "a read is answered by its req".into()));
        }
        let after = m["after"].as_str();
        // The earlier sessions are weftd's to page and search.
        if what == Some("sessions") {
            let at = self.project_named(m)?;
            let call = Call::Read {
                what: "sessions".into(),
                unit: String::new(),
                pane: None,
                lines: None,
                before: None,
                after: after.map(str::to_string),
                q: m["q"].as_str().map(str::to_string),
            };
            return self.ask(at, call, conn, req, Carries::Sessions);
        }
        // A file an agent's conversation pointed at: weftd locates only
        // those, and the answer carries a private link to it.
        if what == Some("file") {
            let (at, pane) = self.agent(m)?;
            let path = m["path"].as_str().ok_or(("bad_request", "no path".to_string()))?;
            let call = Call::Read {
                what: "file".into(),
                unit: path.into(),
                pane: Some(pane),
                lines: None,
                before: None,
                after: None,
                q: None,
            };
            return self.ask(at, call, conn, req, Carries::File);
        }
        // What the device reads of a project it names, answered at once from
        // what this process holds: no read reaches weftd for a page. A Seal's
        // facts come at once; whether its receipt holds is its own read.
        let local = match what {
            Some("eval" | "work" | "session" | "seal" | "sealed" | "done") => {
                let at = self.project_named(m)?;
                let agents = &self.projects[at].as_ref().expect("an open project").agents;
                let id = |k: &str| m[k].as_str().ok_or(("bad_request", format!("no {k}")));
                Some(match what {
                    Some("eval") => json!({"eval": agents.eval(id("eval")?)?}),
                    Some("work") => json!({"work": agents.work(id("unit")?)?}),
                    Some("session") => json!({"session": agents.session(id("session")?)?}),
                    Some("seal") => json!({"seal": agents.seal(id("seal")?, None, after)?}),
                    Some("sealed") => json!({"sealed": agents.sealed(after)}),
                    _ => json!({"done": agents.done(m["q"].as_str(), after)}),
                })
            }
            _ => None,
        };
        if let Some(mut ok) = local {
            ok["type"] = "ok".into();
            ok["req"] = req.clone();
            self.send(conn, ok);
            return Ok(true);
        }
        let (at, of, reads): (usize, Gather, Vec<(Part, &str, String)>) = match what {
            Some("detail") => {
                let id = m["unit"].as_str().ok_or(("bad_request", "no unit".to_string()))?;
                let found = self.projects.iter().enumerate().find_map(|(at, o)| {
                    o.as_ref().and_then(|o| o.agents.unit(id)).map(|u| (at, u.clone()))
                });
                let (at, unit) = found.ok_or(("no_unit", format!("{id} is not on the board")))?;
                let mut reads = vec![(Part::Source, "source", unit.ask_id.clone())];
                if !unit.requested_only {
                    reads.push((Part::Prompt, "wording", unit.ask_id.clone()));
                }
                (at, Gather::Work(Box::new(unit)), reads)
            }
            // Whether a Seal's receipt still holds: RingFrame checks it
            // against the work as it is now, which takes a while.
            Some("receipt") => {
                let at = self.project_named(m)?;
                let id = m["seal"].as_str().ok_or(("bad_request", "no seal".to_string()))?;
                let agents = &self.projects[at].as_ref().expect("an open project").agents;
                agents.seal(id, None, None)?;
                let call = Call::Read {
                    what: "seal".into(),
                    unit: id.into(),
                    pane: None,
                    lines: None,
                    before: None,
                    after: None,
                    q: None,
                };
                return self.ask(at, call, conn, req, Carries::Receipt(id.into()));
            }
            // For rejected work: a follow-up's words, from what its Eval
            // found, or the commits Undo offers; for a Seal, the questions its
            // Eval left (`unit` the Eval).
            Some(what @ ("remedy" | "commits" | "questions")) => {
                let at = self.project_named(m)?;
                let id = m["unit"].as_str().ok_or(("bad_request", "no unit".to_string()))?;
                let call = Call::Read {
                    what: what.into(),
                    unit: id.into(),
                    pane: None,
                    lines: None,
                    before: None,
                    after: None,
                    q: None,
                };
                return self.ask(at, call, conn, req, Carries::Sealed(what.into()));
            }
            Some("eval_view") => {
                let at = self.project_named(m)?;
                let id = m["eval"].as_str().ok_or(("bad_request", "no eval".to_string()))?;
                (at, Gather::EvalView, vec![(Part::View, "eval", id.to_string())])
            }
            _ => {
                return Err((
                    "not_allowed",
                    "a device reads history, detail, work, eval, seal, receipt, session, sealed, \
                     sessions, done, remedy, commits, questions or eval_view"
                        .into(),
                ));
            }
        };
        let g = self.next_gathering;
        self.next_gathering += 1;
        self.gathering.insert(
            g,
            Gathering {
                conn,
                req: req.clone(),
                at,
                of,
                left: 0,
                source: None,
                prompt: None,
                view: None,
                hunks: Default::default(),
            },
        );
        for (part, what, id) in reads {
            self.read_part(g, part, what, id);
        }
        Ok(true)
    }

    /// Ask the daemon for one part of a gathering.
    fn read_part(&mut self, g: u64, part: Part, what: &str, unit: String) {
        let Some(at) = self.gathering.get(&g).map(|r| r.at) else { return };
        let call = Call::Read {
            what: what.into(),
            unit,
            pane: None,
            lines: None,
            before: None,
            after: None,
            q: None,
        };
        if let Some(id) = self.call(at, call) {
            self.calls.insert(
                (at, id),
                Asked { conn: 0, req: Value::Null, carries: Carries::Part(g, part) },
            );
            if let Some(r) = self.gathering.get_mut(&g) {
                r.left += 1;
            }
        }
    }

    /// A part is in; once all are, the device is answered.
    fn gathered(&mut self, g: u64, part: Part, answer: Option<Value>) {
        let Some(r) = self.gathering.get_mut(&g) else { return };
        r.left = r.left.saturating_sub(1);
        let text =
            |v: &Option<Value>| v.as_ref().and_then(|v| v["text"].as_str()).map(str::to_string);
        match part {
            Part::Source => r.source = text(&answer),
            Part::Prompt => r.prompt = text(&answer),
            Part::Hunk(i) => {
                let window = r.view.as_ref().and_then(|v| v.windows.get(i)).map(|w| w.id.clone());
                if let (Some(w), Some(t)) = (window, text(&answer)) {
                    r.hunks.insert(w, t);
                }
            }
            Part::View => {
                let Some(view) = answer
                    .and_then(|v| serde_json::from_value::<weft_core::eval_view::EvalView>(v).ok())
                else {
                    let r = self.gathering.remove(&g).expect("a gathering");
                    return self.error(r.conn, &r.req, "no_record", "that Eval is not on disk");
                };
                let windows: Vec<String> =
                    view.windows.iter().map(|w| format!("{}/{}", view.eval_id, w.id)).collect();
                r.view = Some(view);
                for (i, w) in windows.into_iter().enumerate() {
                    self.read_part(g, Part::Hunk(i), "window", w);
                }
            }
        }
        if self.gathering.get(&g).is_some_and(|r| r.left > 0) {
            return;
        }
        let Some(r) = self.gathering.remove(&g) else { return };
        let mut ok = json!({"type": "ok", "req": r.req});
        let Some(open) = self.projects.get(r.at).and_then(Option::as_ref) else { return };
        match (&r.of, &r.view) {
            (Gather::EvalView, Some(view)) => {
                ok["eval_view"] =
                    serde_json::to_value(weft_core::eval_view::payload(view, &r.hunks))
                        .unwrap_or_default()
            }
            (Gather::Work(unit), _) => {
                ok["detail"] = open.agents.detail(unit, r.source.as_deref(), r.prompt.as_deref())
            }
            (Gather::EvalView, None) => {
                return self.error(r.conn, &r.req, "no_record", "that Eval is not on disk");
            }
        }
        self.send(r.conn, ok);
    }

    /// A pending, as devices are sent it: `text` is exactly what will be
    /// typed.
    pub(super) fn pending_added(&mut self, at: usize, event: Event) {
        let Event::Pending { id, pane, what, why, payload, sends, go } = event else { return };
        // `sends`: the Ask whose prompt this is, which can go to another agent;
        // `go`: its yes button's words key, and whether it stops something.
        // An Eval's dialog carries what it would read, never text to type.
        let reads = serde_json::from_slice::<Value>(&payload).ok().and_then(|p| {
            let eval = go.as_ref().is_some_and(|g| g.word == "act.eval");
            p.get("reads").filter(|_| eval).cloned()
        });
        let go = go.map(|g| json!({"word": g.word, "warn": g.warn}));
        let text =
            if reads.is_some() { String::new() } else { String::from_utf8_lossy(&payload).into() };
        let mut message = json!({"type": "pending", "id": id, "agent": self.agent_id(at, pane),
                                 "what": what, "why": why,
                                 "text": text, "sends": sends, "go": go});
        if let Some(reads) = reads {
            message["reads"] = reads;
        }
        self.to_devices(&message);
        self.pendings.insert(id, (at, message));
    }

    // --- what the device shows, and its pushes --------------------------------

    fn seen(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        let foreground =
            m["foreground"].as_bool().ok_or(("bad_request", "foreground or not".to_string()))?;
        let agent = m["agent"].as_str().map(str::to_string);
        if foreground && let Some(agent) = &agent {
            self.rules.seen(agent);
        }
        if let Some(c) = self.conns.get_mut(&conn) {
            c.seen = Some((foreground, agent));
        }
        Ok(false)
    }

    fn push_subscription(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        let subscription = &m["subscription"];
        let valid = subscription.is_null()
            || (subscription["endpoint"].as_str().is_some_and(|e| {
                // A push service is https; a recording one in a test, loopback.
                e.starts_with("https://") || e.starts_with("http://127.0.0.1:")
            }) && subscription["keys"]["p256dh"].is_string()
                && subscription["keys"]["auth"].is_string());
        if !valid {
            return Err(("bad_request", "a subscription is an https endpoint and its keys".into()));
        }
        let device = self.conns.get(&conn).and_then(|c| c.device.clone()).unwrap_or_default();
        if let Some(d) = self.devices.get_mut(&device) {
            d.push = (!subscription.is_null()).then(|| subscription.clone());
            let _ = self.devices.save();
        }
        Ok(false)
    }
}

/// A size's rows or cols, as a device sends them.
fn dimension(m: &Value, key: &str) -> Option<u16> {
    m[key].as_u64().filter(|n| (2..=1000).contains(n)).map(|n| n as u16)
}

// --- following a conversation ----------------------------------------------

impl Remote {
    /// `follow {agent, after, also}`: this device shows these agents'
    /// Conversations, each from its cursor, at most `PANES`; one it already
    /// follows keeps the cursor it was read to. `follow null` stops them all.
    fn follow(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        let also = match &m["also"] {
            Value::Array(list) => list.iter().collect(),
            Value::Null => Vec::new(),
            one => vec![one],
        };
        let was =
            self.conns.get_mut(&conn).map(|c| std::mem::take(&mut c.following)).unwrap_or_default();
        let mut next: Vec<super::Following> = Vec::new();
        for f in std::iter::once(m).chain(also).filter(|f| !f["agent"].is_null()).take(PANES) {
            let (at, pane) = self.agent(f)?;
            if next.iter().any(|n| (n.at, n.pane) == (at, pane)) {
                continue;
            }
            let after = f["after"].as_str().ok_or(("bad_request", "no after".to_string()))?;
            next.push(match was.iter().find(|w| (w.at, w.pane) == (at, pane)) {
                Some(w) => {
                    super::Following { at, pane, after: w.after.clone(), reading: w.reading }
                }
                None => super::Following { at, pane, after: after.into(), reading: false },
            });
        }
        if let Some(c) = self.conns.get_mut(&conn) {
            c.following = next;
        }
        Ok(false)
    }

    /// The device's follow of this agent, if it still has one.
    fn following(&mut self, conn: u64, (at, pane): (usize, u32)) -> Option<&mut super::Following> {
        let c = self.conns.get_mut(&conn)?;
        c.following.iter_mut().find(|f| (f.at, f.pane) == (at, pane))
    }

    /// Once a second: what each followed agent's file added, one read at a
    /// time for each. Nothing is read while no device follows one.
    pub(super) fn read_followed(&mut self) {
        let due: Vec<(u64, usize, u32, String)> = self
            .conns
            .iter_mut()
            .flat_map(|(id, c)| {
                c.following.iter_mut().filter(|f| !f.reading).map(move |f| {
                    f.reading = true;
                    (*id, f.at, f.pane, f.after.clone())
                })
            })
            .collect();
        for (conn, at, pane, after) in due {
            let call = Call::Read {
                what: "conversation".into(),
                unit: String::new(),
                pane: Some(pane),
                lines: None,
                before: None,
                after: Some(after),
                q: None,
            };
            match self.call(at, call) {
                Some(id) => {
                    let carries = Carries::Follow(conn, at, pane);
                    let asked = Asked { conn, req: Value::Null, carries };
                    self.calls.insert((at, id), asked);
                }
                None => {
                    if let Some(f) = self.following(conn, (at, pane)) {
                        f.reading = false;
                    }
                }
            }
        }
    }

    /// What a followed agent's file grew by: sent to the device that follows
    /// it, or `reset` when it must read the newest page again (the file
    /// changed, or the device fell behind), or `readable: false`.
    fn followed(&mut self, conn: u64, (at, pane): (usize, u32), answer: Option<Value>) {
        // A read for an agent the device no longer follows is dropped.
        let Some(f) = self.following(conn, (at, pane)) else { return };
        f.reading = false;
        let queued = self.conns.get(&conn).map_or(0, |c| c.outbox.queued());
        let read = answer.map(|v| v["conversation"].clone()).unwrap_or(Value::Null);
        let agent = self.agent_id(at, pane);
        // Only this agent's follow ends; the device's other panes keep theirs.
        let stop = |s: &mut Self| {
            if let Some(c) = s.conns.get_mut(&conn) {
                c.following.retain(|f| (f.at, f.pane) != (at, pane));
            }
        };
        let told = |s: &mut Self, m: Value| {
            stop(s);
            s.send(conn, m);
        };
        if read["reset"] == true {
            return told(self, json!({"type": "conversation", "agent": agent, "reset": true}));
        }
        if read["readable"] != true {
            return told(self, json!({"type": "conversation", "agent": agent, "readable": false}));
        }
        let (Some(items), Some(after)) = (read["items"].as_array(), read["after"].as_str()) else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let messages = crate::conversation::grown(items, after, &agent);
        // A device this far behind starts over from the newest page rather
        // than be sent more than its queue holds.
        if queued + messages.len() > crate::socket::QUEUE {
            return told(self, json!({"type": "conversation", "agent": agent, "reset": true}));
        }
        if let Some(f) = self.following(conn, (at, pane)) {
            f.after = after.to_string();
        }
        for m in messages {
            self.send(conn, m);
        }
    }
}
