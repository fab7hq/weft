//! The remote protocol, version 1, from a device that said hello (remote.md
//! §4, §6, §9). Each message becomes only the call §4 allows, or is refused
//! here; a message with a `req` always gets `ok` or `error`.

use base64ct::{Base64, Encoding};
use serde_json::{Value, json};
use weft_proto::{Call, Event};

use super::{Asked, Carries, Remote, Wake};
use crate::agents;

/// How many lines of history a device is sent.
const HISTORY: u64 = 2_000;
/// Between a message's paste and its Enter (remote.md §4).
const ENTER_AFTER: std::time::Duration = std::time::Duration::from_millis(60);

/// Why a request was refused, for the app.
type Refusal = (&'static str, String);

impl Remote {
    pub(super) fn request(&mut self, conn: u64, m: &Value) {
        let req = m["req"].clone();
        let outcome = match m["type"].as_str() {
            Some("watch") => self.watch(conn, m),
            Some("size") => self.size(m),
            Some("keys") => self.keys(conn, m, &req),
            Some("text") => self.text(conn, m, &req),
            Some("act") => self.act_on(conn, m, &req),
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
        match answer {
            Ok(v) => {
                let mut ok = json!({"type": "ok", "req": asked.req});
                match asked.carries {
                    Carries::Nothing => {}
                    Carries::Pending => ok["pending"] = v["pending"].clone(),
                    Carries::Text => ok["text"] = v["text"].clone(),
                    Carries::Record => ok["record"] = v,
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

    /// One agent per device: the previous one is let go.
    fn watch(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        let next = if m["agent"].is_null() { None } else { Some(self.agent(m)?) };
        let was = self.conns.get_mut(&conn).and_then(|c| std::mem::replace(&mut c.watching, next));
        if was != next {
            self.unwatch(conn, was);
        }
        if let Some((at, pane)) = next {
            if let Some(c) = self.conns.get_mut(&conn) {
                c.needs_full = true;
            }
            // Attached again: the daemon starts this pane's frames over with
            // a whole screen, which every device watching it can take.
            self.call(at, Call::Attach { pane, form: "screen".into() });
        }
        Ok(false)
    }

    /// Let a pane go when no device watches it any more.
    pub(super) fn unwatch(&mut self, conn: u64, was: Option<(usize, u32)>) {
        let Some((at, pane)) = was else { return };
        let still = self.conns.iter().any(|(id, c)| *id != conn && c.watching == Some((at, pane)));
        if !still {
            self.call(at, Call::Attach { pane, form: "none".into() });
        }
    }

    pub(super) fn screen(
        &mut self,
        at: usize,
        pane: u32,
        seq: u64,
        full: bool,
        rows: u16,
        cols: u16,
        bytes: &[u8],
    ) {
        let message =
            json!({"type": "screen", "agent": self.agent_id(at, pane), "seq": seq, "full": full,
                             "rows": rows, "cols": cols, "bytes": Base64::encode_string(bytes)})
            .to_string();
        for c in self.conns.values_mut().filter(|c| c.watching == Some((at, pane))) {
            if c.needs_full && !full {
                continue;
            }
            if c.outbox.frame(message.clone()) {
                c.needs_full = false;
            }
        }
    }

    /// A device read again after its frames were dropped: start its pane
    /// over with a whole screen.
    pub(super) fn drained(&mut self, conn: u64) {
        let Some(c) = self.conns.get_mut(&conn) else { return };
        c.needs_full = true;
        if let Some((at, pane)) = c.watching {
            self.call(at, Call::Attach { pane, form: "screen".into() });
        }
    }

    // --- the keys ----------------------------------------------------------

    /// The size the device draws this agent at: the pane takes it before the
    /// device's first key (remote.md §9).
    fn size(&mut self, m: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let n = |k: &str| m[k].as_u64().filter(|n| (2..=1000).contains(n)).map(|n| n as u16);
        let (Some(rows), Some(cols)) = (n("rows"), n("cols")) else {
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

    /// The person typed on the Mac: every agent a device was driving is the
    /// Mac's again (remote.md §9).
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

    /// Ask, Eval or Seal: each becomes a pending the person answers.
    fn act_on(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        let unit = self.projects[at]
            .as_ref()
            .and_then(|o| o.agents.unit_for(pane))
            .map(|u| u.ask_id.clone());
        let call = match m["act"].as_str() {
            Some("ask") => {
                let text = m["text"].as_str().filter(|t| !t.trim().is_empty());
                let text = text.ok_or(("bad_request", "an Ask needs its words".to_string()))?;
                Call::Act {
                    act: "ask".into(),
                    unit: None,
                    pane: Some(pane),
                    text: Some(text.into()),
                }
            }
            Some(act @ ("eval" | "seal")) => {
                let unit =
                    unit.ok_or(("no_unit", "this agent has no open Ask to check".to_string()))?;
                let act = if act == "eval" { "check" } else { "decide" };
                Call::Act { act: act.into(), unit: Some(unit), pane: None, text: None }
            }
            _ => return Err(("not_allowed", "the acts are ask, eval and seal".into())),
        };
        self.ask(at, call, conn, req, Carries::Pending)
    }

    fn resolve(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        if m.get("force").is_some() {
            return Err(("not_allowed", "a device never forces".into()));
        }
        let id = m["pending"].as_str().ok_or(("bad_request", "no pending".to_string()))?;
        let yes = m["yes"].as_bool().ok_or(("bad_request", "yes or no".to_string()))?;
        let at = self
            .pendings
            .get(id)
            .map(|(at, _)| *at)
            .ok_or(("gone", "that was already answered".to_string()))?;
        self.ask(
            at,
            Call::Resolve { pending: id.into(), yes, force: false },
            conn,
            req,
            Carries::Nothing,
        )
    }

    fn read(&mut self, conn: u64, m: &Value, req: &Value) -> Result<bool, Refusal> {
        let (at, pane) = self.agent(m)?;
        match m["what"].as_str() {
            Some("history") => {
                let call = Call::Read {
                    what: "history".into(),
                    unit: String::new(),
                    pane: Some(pane),
                    lines: Some(HISTORY),
                };
                self.ask(at, call, conn, req, Carries::Text)
            }
            Some("eval") => {
                let eval = self.projects[at]
                    .as_ref()
                    .and_then(|o| o.agents.unit_for(pane))
                    .and_then(|u| u.check.as_ref())
                    .map(|c| c.eval_id.clone())
                    .ok_or(("no_record", "this agent's Ask has no Eval".to_string()))?;
                let call = Call::Read { what: "eval".into(), unit: eval, pane: None, lines: None };
                self.ask(at, call, conn, req, Carries::Record)
            }
            _ => Err(("not_allowed", "a device reads eval or history".into())),
        }
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
        if let Some(c) = self.conns.get_mut(&conn) {
            c.seen = Some((foreground, agent));
        }
        Ok(false)
    }

    fn push_subscription(&mut self, conn: u64, m: &Value) -> Result<bool, Refusal> {
        let subscription = &m["subscription"];
        let valid = subscription.is_null()
            || (subscription["endpoint"].as_str().is_some_and(|e| e.starts_with("https://"))
                && subscription["keys"]["p256dh"].is_string()
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
