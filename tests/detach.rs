//! Gate W2's open criterion: a client can leave and come back, and the agents
//! keep working while it is gone.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use weft::protocol::{Frames, ToClient, ToServer};
use weft::server;

struct Client {
    stream: UnixStream,
    frames: Frames<UnixStream>,
}

impl Client {
    fn attach(socket: &PathBuf) -> Self {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the server never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let frames = Frames::new(stream.try_clone().expect("clone"));
        let mut c = Client { stream, frames };
        c.send(ToServer::Attach { rows: 24, cols: 80 });
        c
    }

    fn send(&mut self, m: ToServer) {
        self.stream.write_all(&m.encode()).expect("write");
        self.stream.flush().expect("flush");
    }

    /// Collect output until `needle` shows up, or give up.
    fn wait_for(&mut self, needle: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut seen = String::new();
        while Instant::now() < deadline {
            self.stream
                .set_read_timeout(Some(Duration::from_millis(250)))
                .expect("timeout");
            match self.frames.next() {
                Ok(Some((kind, payload))) => {
                    if let Some(ToClient::Output { bytes, .. }) = ToClient::decode(kind, &payload) {
                        seen.push_str(&String::from_utf8_lossy(&bytes));
                        if seen.contains(needle) {
                            return seen;
                        }
                    }
                }
                Ok(None) => break,
                Err(_) => continue, // a read timeout, not a failure
            }
        }
        seen
    }

    fn hello(&mut self) -> Vec<weft::protocol::PaneInfo> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.stream
                .set_read_timeout(Some(Duration::from_millis(250)))
                .expect("timeout");
            if let Ok(Some((kind, payload))) = self.frames.next() {
                if let Some(ToClient::Hello { panes }) = ToClient::decode(kind, &payload) {
                    return panes;
                }
            }
        }
        panic!("no Hello");
    }
}

fn session(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = server::socket_path(&root);
    let _ = std::fs::remove_file(&socket);
    let serving = root.clone();
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(serving, &listening);
    });
    (root, socket)
}

#[test]
fn an_agent_keeps_working_while_no_client_is_attached() {
    let (root, socket) = session("detach");

    // A client starts an agent and gives it something slow to do.
    let mut first = Client::attach(&socket);
    first.hello();
    first.send(ToServer::Spawn { harness: "sh".into(), spec: "/bin/sh".into() });
    first.send(ToServer::Input {
        pane: 0,
        bytes: b"printf 'before-the-client-left\\n'\n".to_vec(),
    });
    assert!(
        first.wait_for("before-the-client-left", 5).contains("before-the-client-left"),
        "the first client sees its own output"
    );

    // It leaves. Nothing is stopped.
    first.send(ToServer::Detach);
    drop(first);
    std::thread::sleep(Duration::from_millis(400));

    // A new client arrives and is shown what happened while it was away.
    let mut second = Client::attach(&socket);
    let panes = second.hello();
    assert_eq!(panes.len(), 1, "the pane outlived the client");
    assert!(panes[0].running, "and its process is still alive");

    let replayed = second.wait_for("before-the-client-left", 5);
    assert!(
        replayed.contains("before-the-client-left"),
        "a client that arrives late is shown what it missed: {replayed:?}"
    );

    // And it is a live pane, not a recording.
    second.send(ToServer::Input {
        pane: 0,
        bytes: b"printf 'after-the-client-returned\\n'\n".to_vec(),
    });
    assert!(
        second.wait_for("after-the-client-returned", 5).contains("after-the-client-returned"),
        "the agent still takes input from the new client"
    );

    second.send(ToServer::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn two_clients_see_the_same_session_at_once() {
    let (root, socket) = session("two");

    let mut a = Client::attach(&socket);
    a.hello();
    a.send(ToServer::Spawn { harness: "sh".into(), spec: "/bin/sh".into() });
    std::thread::sleep(Duration::from_millis(300));

    let mut b = Client::attach(&socket);
    assert_eq!(b.hello().len(), 1, "the second client sees the first's pane");

    // Input from one is seen by both.
    a.send(ToServer::Input { pane: 0, bytes: b"printf 'shared-output\\n'\n".to_vec() });
    assert!(a.wait_for("shared-output", 5).contains("shared-output"));
    assert!(
        b.wait_for("shared-output", 5).contains("shared-output"),
        "both clients are views of one session"
    );

    a.send(ToServer::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_socket_left_behind_by_a_dead_server_does_not_block_a_new_one() {
    let root = std::env::temp_dir().join(format!("weft-stale-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = server::socket_path(&root);
    std::fs::create_dir_all(socket.parent().unwrap()).expect("dir");
    std::fs::write(&socket, b"not a server").expect("stale file");

    assert!(!server::is_live(&socket), "a leftover file is not a session");
    server::clear_dead(&socket);

    let serving = root.clone();
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(serving, &listening);
    });
    let mut c = Client::attach(&socket);
    assert!(c.hello().is_empty(), "a fresh session starts with no panes");

    c.send(ToServer::Shutdown);
    std::fs::remove_dir_all(&root).ok();
}
