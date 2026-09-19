//! The session server.
//!
//! Spec: `plans/weft/spec/runtime.md`, [ADR-0003]. The server owns the pane
//! processes. A client is one view of them: closing it, losing an SSH
//! connection, or quitting Weft leaves the agents working.
//!
//! What a client sees on attaching is a replay of everything the panes have
//! printed, so a client that arrives late reconstructs exactly the screen a
//! client that never left would be showing.

use std::collections::HashMap;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

use anyhow::Result;

use crate::pane::Pane;
use crate::protocol::{self, Frames, PaneInfo, ToClient, ToServer};

/// Everything a pane has printed since it started, so a late client can be
/// shown the same screen as an early one.
///
/// Bounded: a long-running agent would otherwise grow without limit. Dropping
/// the oldest bytes loses scrollback, never the current screen, because a
/// terminal's state is rebuilt by replaying what remains.
const REPLAY_LIMIT: usize = 4 * 1024 * 1024;

struct Slot {
    pane: Pane,
    harness: String,
    replay: Vec<u8>,
}

impl Slot {
    fn record(&mut self, bytes: &[u8]) {
        self.replay.extend_from_slice(bytes);
        if self.replay.len() > REPLAY_LIMIT {
            let cut = self.replay.len() - REPLAY_LIMIT;
            self.replay.drain(..cut);
        }
    }
}

#[derive(Default)]
struct Clients {
    next: u64,
    sinks: HashMap<u64, UnixStream>,
}

impl Clients {
    fn add(&mut self, stream: UnixStream) -> u64 {
        let id = self.next;
        self.next += 1;
        self.sinks.insert(id, stream);
        id
    }

    /// Send to everyone attached. A client that has gone is simply dropped —
    /// its departure is not an error, it is the normal way a client leaves.
    fn broadcast(&mut self, message: &ToClient) {
        let wire = message.encode();
        self.sinks.retain(|_, s| protocol::send(s, &wire).is_ok());
    }

    fn send_to(&mut self, id: u64, message: &ToClient) {
        let wire = message.encode();
        if let Some(s) = self.sinks.get_mut(&id) {
            if protocol::send(s, &wire).is_err() {
                self.sinks.remove(&id);
            }
        }
    }
}

/// What the accept loop and the pane readers hand to the one thread that owns
/// the panes, so nothing needs a lock held across a blocking read.
enum Event {
    Client(UnixStream),
    Request { client: u64, message: ToServer },
    Output { pane: u32, bytes: Vec<u8> },
    Exited { pane: u32 },
}

pub struct Session {
    root: PathBuf,
    panes: Vec<Slot>,
    clients: Clients,
    tx: Sender<Event>,
}

impl Session {
    /// Serve until told to shut down. The socket is removed on the way out.
    pub fn serve(root: PathBuf, socket: &Path) -> Result<()> {
        if let Some(dir) = socket.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A socket left by a server that died is not a live server.
        let _ = std::fs::remove_file(socket);
        let listener = UnixListener::bind(socket)?;

        let (tx, rx) = channel();
        let accept_tx = tx.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if accept_tx.send(Event::Client(stream)).is_err() {
                    return;
                }
            }
        });

        let mut session = Session { root, panes: Vec::new(), clients: Clients::default(), tx };
        let outcome = session.run(rx);
        let _ = std::fs::remove_file(socket);
        outcome
    }

    fn run(&mut self, rx: Receiver<Event>) -> Result<()> {
        while let Ok(event) = rx.recv() {
            match event {
                Event::Client(stream) => self.accept(stream),
                Event::Request { client, message } => {
                    if self.request(client, message)? {
                        return Ok(());
                    }
                }
                Event::Output { pane, bytes } => {
                    if let Some(slot) = self.panes.get_mut(pane as usize) {
                        slot.record(&bytes);
                    }
                    self.clients.broadcast(&ToClient::Output { pane, bytes });
                }
                Event::Exited { pane } => self.clients.broadcast(&ToClient::Exited { pane }),
            }
        }
        Ok(())
    }

    fn accept(&mut self, stream: UnixStream) {
        let Ok(reading) = stream.try_clone() else { return };
        let id = self.clients.add(stream);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut frames = Frames::new(reading);
            while let Ok(Some((kind, payload))) = frames.next() {
                let Some(message) = ToServer::decode(kind, &payload) else { continue };
                let leaving = message == ToServer::Detach;
                if tx.send(Event::Request { client: id, message }).is_err() || leaving {
                    return;
                }
            }
        });
    }

    /// Returns true when the session should end.
    fn request(&mut self, client: u64, message: ToServer) -> Result<bool> {
        match message {
            ToServer::Attach { rows, cols } => {
                let panes = self
                    .panes
                    .iter_mut()
                    .enumerate()
                    .map(|(i, s)| PaneInfo {
                        pane: i as u32,
                        harness: s.harness.clone(),
                        running: s.pane.running(),
                    })
                    .collect();
                self.clients.send_to(client, &ToClient::Hello { panes });
                // Replay, so this client is shown what every other client sees.
                for i in 0..self.panes.len() {
                    let bytes = self.panes[i].replay.clone();
                    if !bytes.is_empty() {
                        self.clients
                            .send_to(client, &ToClient::Output { pane: i as u32, bytes });
                    }
                }
                for slot in &mut self.panes {
                    let _ = slot.pane.resize(rows, cols);
                }
            }
            ToServer::Input { pane, bytes } => {
                if let Some(slot) = self.panes.get_mut(pane as usize) {
                    let _ = slot.pane.send(&bytes);
                }
            }
            ToServer::Spawn { harness, spec } => self.spawn(&harness, &spec),
            ToServer::Resize { pane, rows, cols } => {
                if let Some(slot) = self.panes.get_mut(pane as usize) {
                    let _ = slot.pane.resize(rows, cols);
                }
            }
            ToServer::Inject { pane, bytes } => {
                let refusal = match self.panes.get_mut(pane as usize) {
                    Some(slot) => {
                        let screen = slot.pane.with_screen(|s| s.contents());
                        let blocked = crate::blocked::looks_blocked(&screen).is_some();
                        match slot.pane.inject(&bytes, blocked) {
                            Err(r) => Some(format!("{r:?}")),
                            // Written, but the composer never showed the whole
                            // prompt, so Enter was withheld. The person has to
                            // know: the text is sitting in the agent unsent.
                            Ok(a) if !a.submitted => Some(
                                "the agent did not show the whole prompt, so it was not sent. \
                                 The text is in its composer."
                                    .to_string(),
                            ),
                            Ok(_) => None,
                        }
                    }
                    None => Some("NoProcess".to_string()),
                };
                self.clients.broadcast(&ToClient::Injected { pane, refusal });
            }
            // A client leaving is not the session ending. That is the point.
            ToServer::Detach => {}
            ToServer::Shutdown => return Ok(true),
        }
        Ok(false)
    }

    fn spawn(&mut self, harness: &str, spec: &str) {
        let cwd = self.root.to_string_lossy().into_owned();
        let mut parts = spec.split_whitespace();
        let program = parts.next().unwrap_or(spec).to_string();
        let args: Vec<String> = parts.map(str::to_string).collect();
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();

        let id = self.panes.len() as u32;
        let (tx, harness_name) = (self.tx.clone(), harness.to_string());
        match Pane::spawn_args(harness, &program, &argv, &cwd, 24, 80) {
            Ok(mut pane) => {
                // Every byte a pane prints goes to the session, which records
                // it for replay and forwards it to whoever is attached.
                let sink = pane.stream_output();
                std::thread::spawn(move || {
                    while let Ok(bytes) = sink.recv() {
                        if tx.send(Event::Output { pane: id, bytes }).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(Event::Exited { pane: id });
                });
                self.panes.push(Slot { pane, harness: harness_name.clone(), replay: Vec::new() });
                self.clients.broadcast(&ToClient::Added { pane: id, harness: harness_name });
            }
            Err(e) => {
                self.clients
                    .broadcast(&ToClient::Injected { pane: id, refusal: Some(e.to_string()) });
            }
        }
    }
}

/// Where a project's session listens. One session per project directory.
/// A Unix socket path is length-limited by `sun_path` — 104 bytes on macOS,
/// 108 on Linux — and the limit is on the path, not on the name. A deep
/// `TMPDIR`, which an isolated host environment routinely has, overruns it.
const SUN_LEN: usize = 100;

pub fn socket_path(root: &Path) -> PathBuf {
    let key = root.to_string_lossy();
    // A short, stable name: socket paths are length-limited on macOS.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    socket_path_in(&base, format!("{h:016x}.sock"))
}

fn socket_path_in(base: &Path, name: String) -> PathBuf {
    let chosen = base.join("weft").join(&name);
    if chosen.as_os_str().len() <= SUN_LEN {
        return chosen;
    }
    // Too deep to bind. `/tmp` is the one directory that is always short, and
    // the name already carries the project, so two projects still never meet.
    PathBuf::from("/tmp").join("weft").join(name)
}

/// Is a session already listening there?
pub fn is_live(socket: &Path) -> bool {
    socket.exists() && UnixStream::connect(socket).is_ok()
}

/// Drop a stale socket so a new server can bind.
pub fn clear_dead(socket: &Path) {
    if socket.exists() && UnixStream::connect(socket).is_err() {
        let _ = std::fs::remove_file(socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_project_has_one_socket_and_two_projects_do_not_share() {
        let a = socket_path(Path::new("/tmp/project-a"));
        let b = socket_path(Path::new("/tmp/project-b"));
        assert_eq!(a, socket_path(Path::new("/tmp/project-a")), "stable");
        assert_ne!(a, b, "separate projects, separate sessions");
        assert!(a.to_string_lossy().ends_with(".sock"));
    }

    #[test]
    fn a_deep_tmpdir_still_yields_a_path_a_socket_can_bind() {
        // Found by the W3 probe: an isolated host environment puts TMPDIR deep
        // enough that the socket path overran sun_path and no session started.
        let deep = std::path::Path::new(
            "/Users/someone/Documents/works/fab7/sandbox/hostlab/hosts/claude/g30/runtime/tmp",
        );
        let path = socket_path_in(deep, "7e1ede271e5eceef.sock".into());
        assert!(
            path.as_os_str().len() <= SUN_LEN,
            "{} is {} bytes",
            path.display(),
            path.as_os_str().len()
        );
    }

    #[test]
    fn two_projects_never_share_a_session_however_deep_the_tmpdir() {
        assert_ne!(
            socket_path(std::path::Path::new("/a/project")),
            socket_path(std::path::Path::new("/b/project"))
        );
    }

    #[test]
    fn a_socket_nothing_is_listening_on_is_not_a_live_session() {
        let path = std::env::temp_dir().join(format!("weft-dead-{}.sock", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        assert!(!is_live(&path), "a leftover file is not a server");
        clear_dead(&path);
        assert!(!path.exists(), "and it is cleared out of the way");
    }

    #[test]
    fn a_replay_keeps_the_recent_past_and_drops_the_distant_one() {
        let mut slot_replay = Vec::new();
        let mut record = |bytes: &[u8]| {
            slot_replay.extend_from_slice(bytes);
            if slot_replay.len() > REPLAY_LIMIT {
                let cut = slot_replay.len() - REPLAY_LIMIT;
                slot_replay.drain(..cut);
            }
        };
        record(&vec![b'o'; REPLAY_LIMIT]);
        record(b"the newest output");
        assert_eq!(slot_replay.len(), REPLAY_LIMIT, "bounded");
        assert!(slot_replay.ends_with(b"the newest output"), "the present survives");
    }
}
