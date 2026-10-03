//! The loop (remote.md §5): it owns all state and is fed by one channel, from
//! the daemon's clients, the WebSockets, the tunnel and the account thread.
//! Nothing it does waits on the network.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
use weft_proto::{Call, Event};

use crate::account::{Account, Reported, SignIn};
use crate::agents;
use crate::daemon::{self, CONTROL, Heard};
use crate::devices::{self, Devices, Gate};
use crate::socket::{self, Outbox, Socket};
use crate::store::{self, Machine};
use crate::tunnel::{self, Tunnel, TunnelEvent};
use crate::view::View;

mod protocol;

/// The remote protocol's version.
pub const VERSION: u64 = 1;
/// How long the person has to answer a device asking to be allowed.
const ASK_WITHIN: Duration = Duration::from_secs(120);

/// Where everything is, and who is trusted.
pub struct Settings {
    /// `~/.fab7/weft`: `config.toml`, `projects.json`, `remote/`.
    pub weft: PathBuf,
    /// The daemon's socket.
    pub socket: PathBuf,
    pub account: Account,
    /// Fab7's assertion key.
    pub key: VerifyingKey,
}

impl Settings {
    /// This machine's.
    pub fn here() -> Self {
        Settings {
            weft: store::weft_dir(),
            socket: weft_proto::socket_path(),
            account: Account::new(),
            key: devices::fab7_key(),
        }
    }
}

pub enum Wake {
    Daemon(Heard),
    Socket(Socket),
    Tunnel(TunnelEvent),
    SignIn(SignIn),
    Tick,
    /// Enter, after a message's paste (remote.md §4).
    Enter {
        at: usize,
        pane: u32,
    },
    /// A push service said a device's subscription is gone.
    PushGone(crate::push::Gone),
}

impl From<crate::push::Gone> for Wake {
    fn from(g: crate::push::Gone) -> Self {
        Wake::PushGone(g)
    }
}

impl From<Heard> for Wake {
    fn from(h: Heard) -> Self {
        Wake::Daemon(h)
    }
}

impl From<Socket> for Wake {
    fn from(s: Socket) -> Self {
        Wake::Socket(s)
    }
}

/// One WebSocket.
#[derive(Default)]
struct Conn {
    outbox: Outbox,
    /// The device it is, once it said hello with its token.
    device: Option<String>,
    /// The agent it has open: a project and a pane.
    watching: Option<(usize, u32)>,
    /// Frames were dropped: nothing but a whole screen until one comes.
    needs_full: bool,
    /// The agents it has typed into since the Mac last did.
    driving: HashSet<(usize, u32)>,
    /// What it last said it shows: in the foreground, and which agent.
    seen: Option<(bool, Option<String>)>,
}

/// A call made for a device's request, waiting for the daemon's answer.
struct Asked {
    conn: u64,
    req: Value,
    /// What the answer carries back: nothing, a pending, or a read.
    carries: Carries,
}

#[derive(Clone, Copy, PartialEq)]
enum Carries {
    Nothing,
    Pending,
    Text,
    Record,
}

/// A device waiting for the person's yes on this machine.
struct Asking {
    conn: u64,
    device: String,
    name: String,
    login: String,
    since: Instant,
}

/// A project open in the daemon.
struct Open {
    client: daemon::Project,
    agents: agents::Project,
}

struct Remote {
    settings: Settings,
    tx: Sender<Wake>,
    dir: PathBuf,
    epoch: String,
    machine: Option<Machine>,
    devices: Devices,
    gate: Gate,
    asking: Option<Asking>,
    conns: HashMap<u64, Conn>,
    control: daemon::Project,
    projects: Vec<Option<Open>>,
    view: View,
    shown: Value,
    reported: Reported,
    cancel: Arc<AtomicBool>,
    _tunnel: Option<Tunnel>,
    /// Calls waiting for the daemon's answer, by project and call id.
    calls: HashMap<(usize, u64), Asked>,
    /// What is waiting on the person, by pending id: its project and the
    /// `pending` message devices are sent.
    pendings: BTreeMap<String, (usize, Value)>,
    /// When the person last typed on the Mac.
    last_presence: Option<Instant>,
    /// The VAPID public key a device subscribes with.
    vapid: String,
    rules: crate::push::Rules,
    pusher: Sender<crate::push::Job>,
}

/// Run until told to stop, or the daemon goes.
pub fn serve(settings: Settings) -> anyhow::Result<()> {
    let (tx, rx) = channel();
    let mut remote = Remote::start(settings, tx)?;
    remote.run(rx);
    Ok(())
}

impl Remote {
    fn start(settings: Settings, tx: Sender<Wake>) -> anyhow::Result<Self> {
        let dir = store::remote_dir(&settings.weft)?;
        let control = daemon::Project::control(&settings.socket, tx.clone())?;
        let config = std::fs::read_to_string(settings.weft.join("config.toml")).unwrap_or_default();
        let config = weft_core::config::read(&config, &Default::default());
        let cloudflared = tunnel::find(config.remote().cloudflared.as_deref());
        let port = socket::listen(tx.clone())?;
        let tunnel = cloudflared.map(Tunnel::new);
        if let Some(t) = &tunnel {
            let events = t.start(port);
            let tx = tx.clone();
            std::thread::spawn(move || {
                while let Ok(e) = events.recv() {
                    if tx.send(Wake::Tunnel(e)).is_err() {
                        return;
                    }
                }
            });
        }
        let ticks = tx.clone();
        std::thread::spawn(move || {
            while ticks.send(Wake::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        let key = crate::push::vapid(&dir)?;
        let vapid = crate::push::public(&key);
        let pusher = crate::push::sender(key, tx.clone());
        let machine = Machine::load(&dir);
        let mut view = View { no_cloudflared: tunnel.is_none(), ..View::default() };
        view.login = machine.as_ref().map(|m| m.login.clone());
        let mut remote = Remote {
            devices: Devices::load(&dir),
            epoch: devices::random_token(),
            settings,
            tx,
            dir,
            machine,
            gate: Gate::default(),
            asking: None,
            conns: HashMap::new(),
            control,
            projects: Vec::new(),
            view,
            shown: Value::Null,
            reported: Reported::default(),
            cancel: Arc::default(),
            _tunnel: tunnel,
            calls: HashMap::new(),
            pendings: BTreeMap::new(),
            last_presence: None,
            vapid,
            rules: Default::default(),
            pusher,
        };
        remote.open_projects();
        if remote.machine.is_none() && !remote.view.no_cloudflared {
            remote.sign_in();
        }
        remote.publish();
        Ok(remote)
    }

    fn run(&mut self, rx: Receiver<Wake>) {
        while let Ok(wake) = rx.recv() {
            let go_on = match wake {
                Wake::Daemon(h) => self.daemon(h),
                Wake::Socket(s) => {
                    self.socket(s);
                    true
                }
                Wake::Tunnel(e) => {
                    self.tunnel(e);
                    true
                }
                Wake::SignIn(s) => {
                    self.signed_in(s);
                    true
                }
                Wake::Tick => {
                    self.tick();
                    true
                }
                Wake::PushGone(g) => {
                    if let Some(d) = self.devices.get_mut(&g.device) {
                        d.push = None;
                        let _ = self.devices.save();
                    }
                    true
                }
                Wake::Enter { at, pane } => {
                    self.call(at, Call::Input { pane, bytes: b"\r".to_vec() });
                    true
                }
            };
            if !go_on {
                break;
            }
            self.publish();
        }
        for conn in self.conns.values() {
            conn.outbox.close();
        }
    }

    // --- the daemon ------------------------------------------------------

    /// Open every project `projects.json` lists that is not open yet.
    fn open_projects(&mut self) {
        let open: Vec<PathBuf> =
            self.projects.iter().flatten().map(|o| o.client.path.clone()).collect();
        for path in daemon::projects(&self.settings.weft.join("projects.json")) {
            if open.contains(&path) {
                continue;
            }
            let at = self.projects.len();
            if let Ok(client) =
                daemon::Project::open(&self.settings.socket, &path, at, self.tx.clone())
            {
                let agents = agents::Project::opened(&path, &client.opened);
                let listed: Vec<Value> =
                    client.opened["pendings"].as_array().cloned().unwrap_or_default();
                self.projects.push(Some(Open { client, agents }));
                for p in listed.iter().filter_map(weft_proto::pending_of) {
                    self.pending_added(at, p);
                }
                self.tell_agents();
            }
        }
    }

    /// Whether to go on.
    fn daemon(&mut self, heard: Heard) -> bool {
        match heard {
            Heard::Gone(CONTROL) => return false,
            Heard::Gone(at) => {
                if let Some(slot) = self.projects.get_mut(at) {
                    *slot = None;
                }
                let lost: Vec<_> = self.calls.keys().filter(|(p, _)| *p == at).copied().collect();
                for key in lost {
                    let asked = self.calls.remove(&key).expect("a call");
                    self.error(asked.conn, &asked.req, "gone", "the project closed");
                }
                self.pendings.retain(|_, (p, _)| *p != at);
                self.tell_agents();
            }
            Heard::Event(CONTROL, Event::RemoteAct { act }) => return self.act(&act),
            Heard::Event(_, Event::Presence { .. }) => self.presence(),
            Heard::Event(CONTROL, _) => {}
            Heard::Answer(at, id, answer) => self.answered(at, id, answer),
            Heard::Event(at, Event::Screen { pane, seq, full, rows, cols, bytes }) => {
                self.screen(at, pane, seq, full, rows, cols, &bytes)
            }
            Heard::Event(at, Event::Pending { id, pane, what, why, payload }) => {
                self.pending_added(at, Event::Pending { id, pane, what, why, payload })
            }
            Heard::Event(_, Event::Resolved { id, yes }) => {
                if self.pendings.remove(&id).is_some() {
                    self.to_devices(&json!({"type": "resolved", "id": id, "yes": yes}));
                }
            }
            Heard::Event(at, event) => {
                let changed = self
                    .projects
                    .get_mut(at)
                    .and_then(Option::as_mut)
                    .is_some_and(|o| o.agents.heard(&event));
                if changed {
                    self.tell_agents();
                }
            }
        }
        true
    }

    fn agents_message(&self) -> Value {
        let open: Vec<&agents::Project> =
            self.projects.iter().flatten().map(|o| &o.agents).collect();
        let rows: Vec<Value> = open.iter().flat_map(|p| p.rows()).collect();
        json!({"type": "agents", "agents": rows})
    }

    fn tell_agents(&mut self) {
        let message = self.agents_message();
        self.to_devices(&message);
        let turns: Vec<(String, &str)> =
            self.projects.iter().flatten().flat_map(|o| o.agents.turns()).collect();
        self.rules.turns(&turns);
        self.push_due();
    }

    /// Send what is due (remote.md §10): to every device with pushes on.
    fn push_due(&mut self) {
        let away = self.last_presence.is_none_or(|t| t.elapsed() >= crate::push::AWAY);
        let in_front: HashSet<String> = self
            .conns
            .values()
            .filter_map(|c| match &c.seen {
                Some((true, Some(agent))) => Some(agent.clone()),
                _ => None,
            })
            .collect();
        let count = self.rules.count();
        for agent in self.rules.due(away, &in_front) {
            let Some((at, pane)) = self.find(&agent) else { continue };
            let Some(open) = self.projects[at].as_ref() else { continue };
            let (_, title) = open.agents.harness(pane).unwrap_or_default();
            let payload = crate::push::payload(&agent, title, &open.agents.name, count);
            for d in self.devices.list.iter() {
                if let Some(subscription) = &d.push {
                    let _ = self.pusher.send(crate::push::Job {
                        device: d.id.clone(),
                        subscription: subscription.clone(),
                        agent: agent.clone(),
                        payload: payload.clone(),
                    });
                }
            }
        }
    }

    /// Publish the view to the TUI when it changed.
    fn publish(&mut self) {
        self.view.devices = self.devices.shown();
        self.view.asking = self
            .asking
            .as_ref()
            .map(|a| json!({"device": a.device, "name": a.name, "login": a.login}));
        let view = self.view.json();
        if view != self.shown {
            let _ = self.control.call(Call::RemoteView { view: view.clone() });
            self.shown = view;
        }
    }

    /// An act from the TUI, through the daemon. Whether to go on.
    fn act(&mut self, act: &Value) -> bool {
        let device = act["device"].as_str().unwrap_or_default().to_string();
        match act["act"].as_str() {
            Some("stop") => return false,
            Some("allow") => self.answer_asking(&device, act["yes"] == true),
            Some("remove") => {
                if self.devices.remove(&device) {
                    let _ = self.devices.save();
                }
                for conn in self.conns.values().filter(|c| c.device.as_deref() == Some(&device)) {
                    conn.outbox.close();
                }
            }
            Some("cancel") => {
                self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                self.view.signing_in = None;
            }
            Some("sign_in") if self.machine.is_none() && self.view.signing_in.is_none() => {
                self.sign_in()
            }
            _ => {}
        }
        true
    }

    // --- the account -----------------------------------------------------

    fn sign_in(&mut self) {
        self.cancel = Arc::default();
        self.view.failed = None;
        let (cancel, tx) = (self.cancel.clone(), self.tx.clone());
        let account = self.settings.account.clone();
        std::thread::spawn(move || {
            account.sign_in(&store::host_name(), &cancel, |step| {
                let _ = tx.send(Wake::SignIn(step));
            });
        });
    }

    fn signed_in(&mut self, step: SignIn) {
        match step {
            SignIn::Code { code, url } => self.view.signing_in = Some((code, url)),
            SignIn::Joined(machine) => {
                self.view.signing_in = None;
                self.view.login = Some(machine.login.clone());
                let _ = machine.save(&self.dir);
                self.machine = Some(machine);
                if let Some(address) = self.view.address.clone() {
                    self.report(&address);
                }
            }
            SignIn::Failed(why) => {
                self.view.signing_in = None;
                self.view.failed = (why != "cancelled").then_some(why);
            }
        }
    }

    fn tunnel(&mut self, event: TunnelEvent) {
        match event {
            TunnelEvent::Address(address) => {
                self.view.address = Some(address.clone());
                self.view.up = true;
                self.report(&address);
            }
            TunnelEvent::Ended(_) => self.view.up = false,
        }
    }

    /// Tell the service a new address, off the loop.
    fn report(&mut self, address: &str) {
        let Some(machine) = self.machine.clone() else { return };
        if !self.reported.is_new(address) {
            return;
        }
        let account = self.settings.account.clone();
        let address = address.to_string();
        std::thread::spawn(move || {
            let _ = account.report(&machine, &address);
        });
    }

    // --- devices ---------------------------------------------------------

    fn tick(&mut self) {
        if self.asking.as_ref().is_some_and(|a| a.since.elapsed() >= ASK_WITHIN) {
            let a = self.asking.take().expect("asking");
            self.refuse(a.conn, "expired");
        }
        self.open_projects();
        self.push_due();
    }

    fn socket(&mut self, s: Socket) {
        match s {
            Socket::Opened(id, outbox) => {
                self.conns.insert(id, Conn { outbox, ..Conn::default() });
            }
            Socket::Closed(id) => {
                if let Some(c) = self.conns.remove(&id) {
                    self.unwatch(id, c.watching);
                }
                if self.asking.as_ref().is_some_and(|a| a.conn == id) {
                    self.asking = None;
                }
            }
            Socket::Drained(id) => self.drained(id),
            Socket::Message(id, text) => self.message(id, &text),
        }
    }

    fn send(&self, conn: u64, message: Value) {
        if let Some(c) = self.conns.get(&conn) {
            c.outbox.send(message.to_string());
        }
    }

    /// Say why, then close.
    fn refuse(&self, conn: u64, code: &str) {
        if let Some(c) = self.conns.get(&conn) {
            c.outbox.send(json!({"type": "refused", "code": code}).to_string());
            c.outbox.finish();
        }
    }

    fn close(&self, conn: u64) {
        if let Some(c) = self.conns.get(&conn) {
            c.outbox.close();
        }
    }

    /// To every device that said hello.
    fn to_devices(&self, message: &Value) {
        let text = message.to_string();
        for c in self.conns.values().filter(|c| c.device.is_some()) {
            c.outbox.send(text.clone());
        }
    }

    fn message(&mut self, conn: u64, text: &str) {
        let Ok(m) = serde_json::from_str::<Value>(text) else { return self.close(conn) };
        let device = self.conns.get(&conn).and_then(|c| c.device.clone());
        match (m["type"].as_str(), device) {
            (Some("hello"), None) => self.hello(conn, &m),
            (Some("pair"), None) => self.pair(conn, &m),
            // Nothing else before a hello.
            (_, None) => self.close(conn),
            (_, Some(_)) => self.request(conn, &m),
        }
    }

    fn hello(&mut self, conn: u64, m: &Value) {
        let (device, token) =
            (m["device"].as_str().unwrap_or(""), m["token"].as_str().unwrap_or(""));
        if m["v"] != VERSION || !self.devices.admits(device, token) {
            return self.refuse(conn, "unknown_device");
        }
        if let Some(d) = self.devices.get_mut(device) {
            d.last_seen = agents::now();
        }
        let _ = self.devices.save();
        if let Some(c) = self.conns.get_mut(&conn) {
            c.device = Some(device.to_string());
        }
        let machine = self.machine.as_ref().map(|m| json!({"id": m.id, "name": m.name}));
        let agents = self.agents_message()["agents"].clone();
        let pendings: Vec<Value> = self.pendings.values().map(|(_, p)| p.clone()).collect();
        self.send(
            conn,
            json!({"type": "welcome", "v": VERSION, "epoch": self.epoch, "machine": machine,
                   "vapid": self.vapid, "agents": agents, "pendings": pendings}),
        );
    }

    fn pair(&mut self, conn: u64, m: &Value) {
        let Some(machine) = &self.machine else { return self.refuse(conn, "not_owner") };
        let now = agents::now() / 1000;
        let checked = devices::check(
            &self.settings.key,
            m["assertion"].as_str().unwrap_or(""),
            now,
            &machine.id,
            machine.github_id,
        );
        let device = m["device"].as_str().unwrap_or("");
        let name = m["name"].as_str().unwrap_or("").chars().take(64).collect::<String>();
        let Ok(a) =
            checked.and_then(|a| if a.device == device { Ok(a) } else { Err("another device") })
        else {
            // A stranger's request never reaches the person.
            return self.refuse(conn, "not_owner");
        };
        if m["v"] != VERSION
            || name.is_empty()
            || self.asking.is_some()
            || !self.gate.may_ask(Instant::now())
        {
            return self.refuse(conn, "busy");
        }
        self.asking =
            Some(Asking { conn, device: a.device, name, login: a.login, since: Instant::now() });
        self.send(conn, json!({"type": "asking"}));
    }

    fn answer_asking(&mut self, device: &str, yes: bool) {
        if self.asking.as_ref().is_none_or(|a| a.device != device) {
            return;
        }
        let a = self.asking.take().expect("asking");
        if !yes {
            return self.refuse(a.conn, "denied");
        }
        let token = self.devices.allow(&a.device, &a.name, &a.login, agents::now());
        let _ = self.devices.save();
        self.send(a.conn, json!({"type": "paired", "token": token}));
    }
}
