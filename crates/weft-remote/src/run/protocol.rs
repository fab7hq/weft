//! The remote protocol, from a device that said hello (remote.md
//! §4, §6, §9). Each message becomes only the call §4 allows, or is refused
//! here; a message with a `req` always gets `ok` or `error`.

use base64ct::{Base64, Encoding};
use serde_json::{Value, json};
use weft_proto::{Call, Event};

use super::{Asked, Carries, Gather, Gathering, PANES, Part, Remote, Wake};
use crate::agents;

/// How many lines of history a device is sent.
const HISTORY: u64 = 2_000;
/// Between a message's paste and its Enter (remote.md §4).
const ENTER_AFTER: std::time::Duration = std::time::Duration::from_millis(60);

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
            Some("seen") => self.seen(conn, m),
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
                    Carries::Starts | Carries::Part(..) => {}
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
        for ((at, pane), w) in next {
            // Nobody at the Mac, or an agent this device started: the device's
            // size and the keys, so it fills the device's pane (remote.md §9).
            let mine = self.conns.get(&conn).is_some_and(|c| c.started.contains(&(at, pane)));
            if let (Some(rows), Some(cols)) = (dimension(w, "rows"), dimension(w, "cols"))
                && (self.away() || mine)
            {
                self.call(at, Call::Resize { pane, rows, cols });
                self.drive(conn, at, pane);
            }
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
        let message =
            json!({"type": "screen", "agent": self.agent_id(at, pane), "seq": seq, "full": full,
                             "rows": rows, "cols": cols, "bytes": Base64::encode_string(&bytes)})
            .to_string();
        for c in self.conns.values_mut().filter(|c| c.watching.contains(&(at, pane))) {
            if c.needs_full.contains(&(at, pane)) && !full {
                continue;
            }
            if c.outbox.frame(message.clone()) {
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

    /// The size the device draws this agent at: the pane takes it before the
    /// device's first key (remote.md §9).
    fn size(&mut self, m: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let (Some(rows), Some(cols)) = (dimension(m, "rows"), dimension(m, "cols")) else {
            return Err(("bad_request", "a size is rows and cols".into()));
        };
        self.call(at, Call::Resize { pane, rows, cols });
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

    /// A message: pasted, then Enter on its own a moment later, the way a
    /// person's paste and Enter arrive (remote.md §4).
    fn text(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let text = m["text"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or(("bad_request", "no text".to_string()))?;
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        self.drive(conn, at, pane);
        let went = self.ask(at, Call::Input { pane, bytes }, conn, req, Carries::Nothing)?;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(ENTER_AFTER);
            let _ = tx.send(Wake::Enter { at, pane });
        });
        Ok(went)
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
            Some("ask" | "follow_up") => self.agent(m).map(|(at, pane)| (at, Some(pane)))?,
            // Eval and Seal act on a round of a project (remote.md §6).
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

    /// The project a message names by `project`.
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

    /// Yes or no. Weft types on its own only into an agent that has said it is
    /// ready: a yes for one that has not is refused with why, and only then may
    /// the device say to send it anyway (`force`).
    fn resolve(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let id = m["pending"].as_str().ok_or(("bad_request", "no pending".to_string()))?;
        let yes = m["yes"].as_bool().ok_or(("bad_request", "yes or no".to_string()))?;
        let force = m["force"].as_bool().unwrap_or(false);
        if force && !self.not_ready.contains(id) {
            return Err(("not_allowed", "send anyway follows not_ready".into()));
        }
        let (at, pane) = self
            .pendings
            .get(id)
            .map(|(at, p)| {
                (
                    *at,
                    p["agent"]
                        .as_str()
                        .and_then(|a| a.rsplit_once('-'))
                        .and_then(|(_, n)| n.parse::<u32>().ok()),
                )
            })
            .ok_or(("gone", "that was already answered".to_string()))?;
        if yes && !force {
            let turn = self.projects[at].as_ref().zip(pane).and_then(|(o, p)| o.agents.turn_of(p));
            if let Err(why) = weft_core::turns::may_type(turn) {
                self.not_ready.insert(id.to_string());
                return Err((
                    "not_ready",
                    weft_core::words::fill("not_typed.why", &[("why", why)]),
                ));
            }
        }
        self.not_ready.remove(id);
        self.ask(
            at,
            Call::Resolve { pending: id.into(), yes, force: force && yes },
            conn,
            req,
            Carries::Nothing,
        )
    }

    fn read(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let what = m["what"].as_str();
        if what == Some("history") {
            let (at, pane) = self.agent(m)?;
            let call = Call::Read {
                what: "history".into(),
                unit: String::new(),
                pane: Some(pane),
                lines: Some(HISTORY),
            };
            let key = self.projects[at].as_ref().and_then(|o| o.agents.transcript(pane));
            return self.ask(at, call, conn, req, Carries::History(key));
        }
        if req.is_null() {
            return Err(("bad_request", "a read is answered by its req".into()));
        }
        let after = m["after"].as_str();
        // What the device reads of a project it names, answered at once from
        // what this process holds: no read reaches weftd for a page.
        let local = match what {
            Some("eval" | "session" | "sealed" | "sessions") => {
                let at = self.project_named(m)?;
                let agents = &self.projects[at].as_ref().expect("an open project").agents;
                let id = |k: &str| m[k].as_str().ok_or(("bad_request", format!("no {k}")));
                Some(match what {
                    Some("eval") => json!({"eval": agents.eval(id("eval")?)?}),
                    Some("session") => json!({"session": agents.session(id("session")?)?}),
                    Some("sealed") => json!({"sealed": agents.sealed(after)}),
                    _ => json!({"sessions": agents.sessions(m["q"].as_str(), after, now_millis())}),
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
            Some("seal") => {
                let at = self.project_named(m)?;
                let id = m["seal"].as_str().ok_or(("bad_request", "no seal".to_string()))?;
                let agents = &self.projects[at].as_ref().expect("an open project").agents;
                agents.seal(id, None, None)?;
                let of = Gather::Seal { id: id.into(), after: after.map(str::to_string) };
                (at, of, vec![(Part::Seal, "seal", id.to_string())])
            }
            Some("eval_view") => {
                let at = self.project_named(m)?;
                let id = m["eval"].as_str().ok_or(("bad_request", "no eval".to_string()))?;
                (at, Gather::EvalView, vec![(Part::View, "eval", id.to_string())])
            }
            _ => {
                return Err((
                    "not_allowed",
                    "a device reads history, detail, eval, seal, session, sealed, sessions or \
                     eval_view"
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
                seal: None,
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
        let call = Call::Read { what: what.into(), unit, pane: None, lines: None };
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
            Part::Seal => r.seal = answer,
            Part::Hunk(i) => {
                let window =
                    r.view.as_ref().and_then(|v| v.changes.get(i)).map(|c| c.window.clone());
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
                    view.changes.iter().map(|c| format!("{}/{}", view.eval_id, c.window)).collect();
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
            (Gather::Seal { id, after }, _) => {
                match open.agents.seal(id, r.seal.as_ref(), after.as_deref()) {
                    Ok(v) => ok["seal"] = v,
                    Err((code, message)) => return self.error(r.conn, &r.req, code, &message),
                }
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
        let Event::Pending { id, pane, what, why, payload } = event else { return };
        let message = json!({"type": "pending", "id": id, "agent": self.agent_id(at, pane),
                             "what": what, "why": why,
                             "text": String::from_utf8_lossy(&payload)});
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
