//! The loop (remote.md §5): it owns all state and is fed by one channel, from
//! the daemon's clients, the WebSockets, the tunnel and the account thread.
//! Nothing it does waits on the network.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

use ed25519_dalek::VerifyingKey;
use serde_json::{Value, json};
use weft_proto::{Call, Event};

use crate::account::{Account, Probe, Report, SignIn};
use crate::agents;
use crate::daemon::{self, CONTROL, Heard};
use crate::devices::{self, Devices, Gate};
use crate::health::{Act, Recovery};
use crate::socket::{self, Outbox, Socket};
use crate::store::{self, Machine};
use crate::tool::Tool;
use crate::tunnel::{self, Tunnel, TunnelEvent};
use crate::view::View;

mod protocol;

/// The remote protocol's version.
pub const VERSION: u64 = 2;
/// The most agents a device may watch at once: one to each of its panes.
pub const PANES: usize = 4;
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
    /// The service said this machine is no longer on an account.
    Left,
    /// What became of a report of this address.
    Reported(String, Result<Report, String>),
    /// What this address answered from outside.
    Probed(String, Probe),
    /// Whether the network answered, with no tunnel up to look at.
    Network(bool),
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
    /// The agents it has open, a project and a pane each: one, or two side
    /// by side.
    watching: Vec<(usize, u32)>,
    /// The agents whose frames were dropped: nothing but a whole screen of
    /// each until one comes.
    needs_full: HashSet<(usize, u32)>,
    /// The agents it has typed into since the machine last did.
    driving: HashSet<(usize, u32)>,
    /// The agents it started or resumed: opening one takes its size.
    started: HashSet<(usize, u32)>,
    /// What it last said it shows: in the foreground, and which agent.
    seen: Option<(bool, Option<String>)>,
    /// The agents whose Conversation it shows, one to each of its panes, and
    /// the cursor each has been read to.
    following: Vec<Following>,
}

/// An agent a device follows: what was added after `after` is read once a
/// second, and only one read at a time.
struct Following {
    at: usize,
    pane: u32,
    after: String,
    reading: bool,
}

/// A call made for a device's request, waiting for the daemon's answer.
struct Asked {
    conn: u64,
    req: Value,
    /// What the answer carries back: nothing, a pending, or a read.
    carries: Carries,
}

#[derive(Clone, PartialEq)]
enum Carries {
    Nothing,
    Pending,
    /// A history, with the key its harness keeps the rest behind.
    History(Option<String>),
    /// The agent `start` made, by its id.
    Agent,
    /// What can be started: the machine's own call, answered to no device.
    Starts,
    /// One part of a read being gathered, by its gathering.
    Part(u64, Part),
    /// A page of an agent's conversation, by the agent's id.
    Conversation(String),
    /// What a followed agent's file grew by, for this device and agent.
    Follow(u64, usize, u32),
    /// What to suggest in the agent's message field, or nothing.
    Suggestion,
}

/// The daemon's reads one device read is gathered from.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Source,
    Prompt,
    Seal,
    View,
    /// The hunk of the view's change at this index.
    Hunk(usize),
}

/// What a read gathers parts for: a work's Details, a Seal's (with the page
/// of its Asks after `after`), or an Eval's view.
enum Gather {
    Work(Box<weft_core::ledger::Unit>),
    Seal { id: String, after: Option<String> },
    EvalView,
}

/// A device's read (remote.md §6), answered once every part the daemon was
/// asked for is in. A part that did not come back is said where it would
/// be, as the desktop app says it.
struct Gathering {
    conn: u64,
    req: Value,
    at: usize,
    of: Gather,
    left: usize,
    source: Option<String>,
    prompt: Option<String>,
    seal: Option<Value>,
    /// The Eval view, for `read eval_view`.
    view: Option<weft_core::eval_view::EvalView>,
    hunks: BTreeMap<String, String>,
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
    /// The tree devices were last sent.
    told_tree: Value,
    /// When to look at the tunnel and the network, and to tell the service
    /// the address (remote.md §11a).
    recovery: Recovery,
    cancel: Arc<AtomicBool>,
    tunnel: Option<Tunnel>,
    /// The tunnel adapter chosen, once it is found.
    adapter: Option<Tool>,
    /// The loopback port the tunnel carries.
    port: u16,
    /// Calls waiting for the daemon's answer, by project and call id.
    calls: HashMap<(usize, u64), Asked>,
    /// Reads of a work being gathered, by id.
    gathering: HashMap<u64, Gathering>,
    next_gathering: u64,
    /// What is waiting on the person, by pending id: its project and the
    /// `pending` message devices are sent.
    pendings: BTreeMap<String, (usize, Value)>,
    /// Pendings a device was told its agent is not ready for: only these may it
    /// then send anyway.
    not_ready: HashSet<String>,
    /// When the person last typed on the machine.
    last_presence: Option<Instant>,
    /// The VAPID public key a device subscribes with.
    vapid: String,
    rules: crate::push::Rules,
    pusher: Sender<crate::push::Job>,
    /// A newer Weft installed on this machine, waiting for no agent
    /// to run.
    update: Option<String>,
}

/// Where the tunnel stands: up, with no adapter chosen, or with the chosen
/// adapter's program missing (remote.md §7, §7a).
enum Ready {
    Up(Tunnel, Tool),
    Choose(Vec<Tool>),
    Missing(Tool),
}

/// The chosen tunnel to `port`, if its program is where `[remote] program`
/// says or on `PATH`; its events go to the loop.
fn start_tunnel(weft: &std::path::Path, port: u16, tx: &Sender<Wake>) -> Ready {
    let config = std::fs::read_to_string(weft.join("config.toml")).unwrap_or_default();
    let config = weft_core::config::read(&config, &Default::default());
    let chosen = config.remote().tunnel.as_deref().and_then(|id| Tool::tunnel(weft, id));
    let Some(tool) = chosen else { return Ready::Choose(Tool::tunnels(weft)) };
    let Some(program) = tunnel::find(config.remote().program.as_deref(), &tool.program) else {
        return Ready::Missing(tool);
    };
    // A tunnel a killed weft remote left running is stopped first.
    let kept = weft.join("remote").join("tunnel.pid");
    tunnel::stop_left(&kept, &program);
    let tunnel = Tunnel::new(program, tool.run.clone(), tool.address.clone()).keeping_pid(kept);
    let events = tunnel.start(port);
    let tx = tx.clone();
    std::thread::spawn(move || {
        while let Ok(e) = events.recv() {
            if tx.send(Wake::Tunnel(e)).is_err() {
                return;
            }
        }
    });
    Ready::Up(tunnel, tool)
}

/// The tunnel adapters to choose from, as the Remote view lists them.
fn tunnel_list(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| json!({"id": t.id, "title": t.title, "note": t.note, "guide": t.guide}))
        .collect()
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
        let port = socket::listen(tx.clone())?;
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
        let view = View { login: machine.as_ref().map(|m| m.login.clone()), ..Default::default() };
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
            told_tree: Value::Null,
            recovery: Recovery::new(Instant::now(), SystemTime::now()),
            cancel: Arc::default(),
            tunnel: None,
            adapter: None,
            port,
            calls: HashMap::new(),
            gathering: HashMap::new(),
            not_ready: HashSet::new(),
            next_gathering: 0,
            pendings: BTreeMap::new(),
            last_presence: None,
            vapid,
            rules: Default::default(),
            pusher,
            update: None,
        };
        remote.open_projects();
        remote.look();
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
                Wake::Reported(address, said) => {
                    self.reported(&address, said);
                    true
                }
                Wake::Probed(address, seen) => {
                    self.probed(&address, seen);
                    true
                }
                Wake::Network(online) => {
                    let acts = self.recovery.network(Instant::now(), online);
                    self.perform(acts);
                    true
                }
                Wake::PushGone(g) => {
                    if let Some(d) = self.devices.get_mut(&g.device) {
                        d.push = None;
                        let _ = self.devices.save();
                    }
                    true
                }
                Wake::Left => {
                    self.left();
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
                if let Some(v) = client.opened["update"].as_str() {
                    self.update = Some(v.to_string());
                }
                let listed: Vec<Value> =
                    client.opened["pendings"].as_array().cloned().unwrap_or_default();
                self.projects.push(Some(Open { client, agents }));
                self.look_for_starts(at);
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
            Heard::Event(_, Event::Update { version }) => {
                if version != self.update {
                    self.update = version;
                    let update = self.update_json();
                    self.to_devices(&json!({"type": "update", "update": update}));
                }
            }
            Heard::Event(_, Event::Presence { .. }) => self.presence(),
            Heard::Event(CONTROL, _) => {}
            Heard::Answer(at, id, answer) => self.answered(at, id, answer),
            Heard::Event(at, screen @ Event::Screen { .. }) => self.screen(at, screen),
            Heard::Event(at, pending @ Event::Pending { .. }) => self.pending_added(at, pending),
            Heard::Event(_, Event::Resolved { id, yes }) => {
                if self.pendings.remove(&id).is_some() {
                    self.to_devices(&json!({"type": "resolved", "id": id, "yes": yes}));
                }
            }
            Heard::Event(at, event) => {
                if matches!(event, Event::Readiness { .. }) {
                    self.look_for_starts(at);
                }
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

    /// The machine's `update` (remote.md §6): null, or `{version}`.
    fn update_json(&self) -> Value {
        self.update.as_ref().map_or(Value::Null, |v| json!({"version": v}))
    }

    fn agents_message(&self) -> Value {
        let open: Vec<&agents::Project> =
            self.projects.iter().flatten().map(|o| &o.agents).collect();
        let rows: Vec<Value> = open.iter().flat_map(|p| p.rows()).collect();
        json!({"type": "agents", "agents": rows})
    }

    /// Ask the daemon what this project can start: once it opens, and when a
    /// harness's readiness changes.
    fn look_for_starts(&mut self, at: usize) {
        if let Some(id) = self.call(at, Call::Available) {
            self.calls
                .insert((at, id), Asked { conn: 0, req: Value::Null, carries: Carries::Starts });
        }
    }

    /// What the person can start, across every open project.
    fn harnesses(&self) -> Vec<Value> {
        self.projects.iter().flatten().flat_map(|o| o.agents.harnesses()).collect()
    }

    /// Every open project's rows of the tree (remote.md §6).
    fn tree(&self) -> Value {
        self.projects.iter().flatten().map(|o| o.agents.tree()).collect()
    }

    fn tell_agents(&mut self) {
        let message = self.agents_message();
        self.to_devices(&message);
        let tree = self.tree();
        if tree != self.told_tree {
            self.to_devices(&json!({"type": "tree", "tree": tree}));
            self.told_tree = tree;
        }
        let turns: Vec<(String, &str)> =
            self.projects.iter().flatten().flat_map(|o| o.agents.turns()).collect();
        self.rules.turns(&turns);
        self.push_due();
    }

    /// Send what is due (remote.md §10): to every device with pushes on.
    fn push_due(&mut self) {
        let away = self.away();
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

    /// Nobody has typed on the Mac for a while (remote.md §9, §10).
    fn away(&self) -> bool {
        self.last_presence.is_none_or(|t| t.elapsed() >= crate::push::AWAY)
    }

    /// Publish the view to the desktop app when it changed.
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

    /// An act from the desktop app, through the daemon. Whether to go on.
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
            // Check: its program was missing; look again, and start if the
            // person has installed it.
            Some("look") if self.tunnel.is_none() => self.look(),
            // The tunnel the person picked from the list. Another than the
            // one before forgets where that one's program was.
            Some("choose") if self.tunnel.is_none() => {
                if let Some(tool) =
                    act["tunnel"].as_str().and_then(|id| Tool::tunnel(&self.settings.weft, id))
                {
                    let config = self.settings.weft.join("config.toml");
                    let text = std::fs::read_to_string(&config).unwrap_or_default();
                    let before =
                        weft_core::config::read(&text, &Default::default()).remote().tunnel.clone();
                    let mut set = weft_core::config::set_tunnel(&text, &tool.id);
                    if before.is_some_and(|b| b != tool.id) {
                        set = set.and_then(|t| weft_core::config::set_program(&t, ""));
                    }
                    if let Ok(text) = set {
                        let _ = std::fs::write(&config, text);
                    }
                    self.look();
                }
            }
            // Stop the tunnel and choose again, the current one picked.
            Some("change_tunnel")
                if self.view.can_change_tunnel && self.view.signing_in.is_none() =>
            {
                self.tunnel = None;
                self.view.up = false;
                self.view.address = None;
                self.view.tunnel_why = None;
                self.view.missing = None;
                self.view.note = None;
                self.view.chosen = self.adapter.take().map(|t| t.id);
                self.view.choose = Some(tunnel_list(&Tool::tunnels(&self.settings.weft)));
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
                self.report();
            }
            SignIn::Failed(why) => {
                self.view.signing_in = None;
                self.view.failed = (why != "cancelled").then_some(why);
            }
        }
    }

    /// Where the tunnel stands, again: start it if it can be, else say what
    /// is missing; sign in once it is up and this machine has not joined.
    fn look(&mut self) {
        self.view.choose = None;
        self.view.chosen = None;
        self.view.missing = None;
        self.view.tunnel_why = None;
        self.view.can_change_tunnel = Tool::tunnels(&self.settings.weft).len() > 1;
        match start_tunnel(&self.settings.weft, self.port, &self.tx) {
            Ready::Up(tunnel, tool) => {
                self.view.note = tool.note.clone();
                self.view.tunnel_title = Some(tool.title.clone());
                self.tunnel = Some(tunnel);
                self.adapter = Some(tool);
                if self.machine.is_none() && self.view.signing_in.is_none() {
                    self.sign_in();
                }
            }
            Ready::Choose(tools) => self.view.choose = Some(tunnel_list(&tools)),
            Ready::Missing(tool) => {
                self.view.missing = Some(json!({"id": tool.id, "title": tool.title,
                                                "program": tool.program, "guide": tool.guide}));
                self.adapter = Some(tool);
            }
        }
    }

    fn tunnel(&mut self, event: TunnelEvent) {
        match event {
            TunnelEvent::Address(address) => {
                self.view.tunnel_why = None;
                self.view.address = Some(address);
                self.view.up = true;
                // A new name is looked at once it is known everywhere.
                self.recovery.health.new_name(Instant::now());
                self.report();
            }
            TunnelEvent::Ended(why) => {
                self.view.up = false;
                self.view.tunnel_why.get_or_insert(why);
            }
            TunnelEvent::Said(line) if !self.view.up => self.view.tunnel_why = Some(line),
            TunnelEvent::Said(_) => {}
        }
    }

    /// Tell the service the address when it does not have it, off the loop;
    /// one that did not arrive is tried again.
    fn report(&mut self) {
        let address = self.view.address.clone().filter(|_| self.view.up);
        let joined = self.machine.is_some();
        let act = self.recovery.report(Instant::now(), address.as_deref(), joined);
        self.perform(act.into_iter().collect());
    }

    /// Do what the recovery rules say.
    fn perform(&mut self, acts: Vec<Act>) {
        for act in acts {
            let (account, tx) = (self.settings.account.clone(), self.tx.clone());
            match act {
                Act::LookAtTunnel(address) => {
                    std::thread::spawn(move || {
                        let seen = account.probe(&address);
                        let _ = tx.send(Wake::Probed(address, seen));
                    });
                }
                Act::LookAtNetwork => {
                    std::thread::spawn(move || {
                        let _ = tx.send(Wake::Network(account.online()));
                    });
                }
                Act::Report(address) => {
                    let Some(machine) = self.machine.clone() else { continue };
                    std::thread::spawn(move || {
                        let said = account.report(&machine, &address);
                        let _ = tx.send(Wake::Reported(address, said));
                    });
                }
                Act::PokeTunnel => {
                    if let Some(t) = &self.tunnel {
                        t.back();
                    }
                }
                Act::Recycle => {
                    self.view.up = false;
                    if let Some(t) = &self.tunnel {
                        t.recycle();
                    }
                }
            }
        }
    }

    fn reported(&mut self, address: &str, said: Result<Report, String>) {
        match said {
            Ok(Report::Taken) => self.recovery.reported.taken(address, Instant::now()),
            Ok(Report::NotOnAnAccount) => self.left(),
            Err(_) => self.recovery.reported.failed(Instant::now()),
        }
    }

    fn probed(&mut self, address: &str, probe: Probe) {
        let now = Instant::now();
        if self.view.address.as_deref() != Some(address) {
            return self.recovery.health.lost(now);
        }
        let acts = self.recovery.seen(now, probe);
        self.perform(acts);
    }

    /// Removed from the account on the web (D1.6): forget its place there and
    /// join again as a machine that never had, so W → R shows the code.
    fn left(&mut self) {
        self.machine = None;
        let _ = std::fs::remove_file(self.dir.join("machine.toml"));
        self.view.login = None;
        self.recovery.reported = Default::default();
        if self.tunnel.is_some() && self.view.signing_in.is_none() {
            self.sign_in();
        }
    }

    // --- devices ---------------------------------------------------------

    fn tick(&mut self) {
        // Look at the tunnel from outside now and then, and at once after
        // the machine wakes: a quick tunnel Cloudflare dropped while it slept
        // still runs here and still shows its address, but carries nothing.
        // With no tunnel up, look at the network, so its return is noticed.
        let address = self.view.address.clone().filter(|_| self.view.up);
        let acts = self.recovery.tick(
            Instant::now(),
            SystemTime::now(),
            address.as_deref(),
            self.tunnel.is_some(),
            self.machine.is_some(),
        );
        self.perform(acts);
        if self.asking.as_ref().is_some_and(|a| a.since.elapsed() >= ASK_WITHIN) {
            let a = self.asking.take().expect("asking");
            self.refuse(a.conn, "expired");
        }
        self.open_projects();
        self.push_due();
        self.read_followed();
    }

    fn socket(&mut self, s: Socket) {
        match s {
            Socket::Opened(id, outbox) => {
                self.conns.insert(id, Conn { outbox, ..Conn::default() });
            }
            Socket::Closed(id) => {
                if let Some(c) = self.conns.remove(&id) {
                    for w in c.watching {
                        self.unwatch(id, w);
                    }
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
        // A page from another version is told so, rather than forgotten.
        if m["v"] != VERSION {
            return self.refuse_version(conn, &m["v"]);
        }
        if !self.devices.admits(device, token) {
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
                   "vapid": self.vapid, "agents": agents, "tree": self.tree(),
                   "harnesses": self.harnesses(), "pendings": pendings,
                   "update": self.update_json(), "panes": PANES, "split": true}),
        );
    }

    /// Refused for its version, saying which side is the newer, so the page
    /// says what to do: reload it, or update Weft here.
    fn refuse_version(&self, conn: u64, v: &Value) {
        let newer = if v.as_u64().is_some_and(|v| v > VERSION) { "page" } else { "machine" };
        if let Some(c) = self.conns.get(&conn) {
            c.outbox
                .send(json!({"type": "refused", "code": "version", "newer": newer}).to_string());
            c.outbox.finish();
        }
    }

    fn pair(&mut self, conn: u64, m: &Value) {
        if m["v"] != VERSION {
            return self.refuse_version(conn, &m["v"]);
        }
        // Not signed in to Weft's service: only the person here can fix it.
        let Some(machine) = &self.machine else { return self.refuse(conn, "signed_out") };
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
        if name.is_empty() || self.asking.is_some() || !self.gate.may_ask(Instant::now()) {
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
