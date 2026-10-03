//! A recording HTTP server standing in for GitHub, Fab7's account service
//! and push services: each request is kept, and answered from the routes a
//! test gives, in order, the last answer repeating.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

type Routes = Arc<Mutex<HashMap<String, Vec<(u16, String)>>>>;

pub struct Fake {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
    routes: Routes,
}

impl Fake {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let routes: Routes = Arc::default();
        let (s, r) = (seen.clone(), routes.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (s, r) = (s.clone(), r.clone());
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let mut headers = HashMap::new();
                    loop {
                        let mut h = String::new();
                        reader.read_line(&mut h).unwrap();
                        let h = h.trim_end();
                        if h.is_empty() {
                            break;
                        }
                        if let Some((k, v)) = h.split_once(':') {
                            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                        }
                    }
                    let length =
                        headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let key = format!("{method} {path}");
                    let (status, answer) = {
                        let mut routes = r.lock().unwrap();
                        match routes.get_mut(&key) {
                            Some(list) if list.len() > 1 => list.remove(0),
                            Some(list) if !list.is_empty() => list[0].clone(),
                            _ => (404, "{\"code\":\"not_found\"}".to_string()),
                        }
                    };
                    s.lock().unwrap().push(Seen { method, path, headers, body });
                    let mut out = stream;
                    let _ = write!(
                        out,
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                });
            }
        });
        Fake { url, seen, routes }
    }

    /// Answer `method path` with these, in turn; the last repeats.
    pub fn route(&self, method: &str, path: &str, answers: &[(u16, &str)]) {
        let list = answers.iter().map(|(s, a)| (*s, a.to_string())).collect();
        self.routes.lock().unwrap().insert(format!("{method} {path}"), list);
    }

    pub fn requests(&self, method: &str, path: &str) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == method && s.path == path)
            .cloned()
            .collect()
    }
}

// --- a whole weft remote, against a real daemon ---------------------------

use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64ct::{Base64UrlUnpadded, Encoding};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Value, json};
use tungstenite::WebSocket;
use tungstenite::client::IntoClientRequest;
use weft_proto::{Call, Event, Line, Lines, PROTOCOL};

/// Fab7's key, in a test.
pub fn fab7() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

/// An assertion as the account service signs it.
pub fn assertion(key: &SigningKey, fields: Value) -> String {
    let payload = Base64UrlUnpadded::encode_string(fields.to_string().as_bytes());
    let signature = key.sign(payload.as_bytes());
    format!("{payload}.{}", Base64UrlUnpadded::encode_string(&signature.to_bytes()))
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// A good assertion for this harness's machine and owner.
pub fn good(device: &str) -> String {
    assertion(
        &fab7(),
        json!({"iss": "fab7", "sub": 42, "login": "octo", "machine": "m_1", "device": device,
               "exp": now() + 60}),
    )
}

pub struct Harness {
    pub dir: PathBuf,
    pub socket: PathBuf,
    pub port: u16,
    /// A TUI on the daemon, hearing the remote view.
    pub tui: Tui,
}

pub struct Tui {
    out: UnixStream,
    lines: Lines<UnixStream>,
    next: u64,
    pub view: Value,
}

impl Tui {
    fn connect(socket: &Path) -> Self {
        let out = UnixStream::connect(socket).unwrap();
        let lines = Lines::new(out.try_clone().unwrap());
        let mut t = Tui { out, lines, next: 1, view: Value::Null };
        t.call(Call::Hello { client: "weft-tui/test".into(), protocol: PROTOCOL });
        t
    }

    pub fn call(&mut self, call: Call) {
        use std::io::Write;
        let id = self.next;
        self.next += 1;
        self.out.write_all(&Line::Call { id, call }.encode()).unwrap();
    }

    /// An act for weft remote, as the TUI sends it.
    pub fn act(&mut self, act: Value) {
        self.call(Call::Remote { act });
    }

    /// Views until one is picked, keeping each as `view`.
    pub fn wait_view(&mut self, mut pick: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        self.out.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        while Instant::now() < deadline {
            if let Ok(Some(Line::Event(Event::Remote { view }))) = self.lines.next() {
                self.view = view;
                if pick(&self.view) {
                    return self.view.clone();
                }
            }
        }
        panic!("no such view; the last was {}", self.view);
    }

    /// Every view that arrives within `within`.
    pub fn views(&mut self, within: Duration) -> Vec<Value> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        self.out.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        while Instant::now() < deadline {
            if let Ok(Some(Line::Event(Event::Remote { view }))) = self.lines.next() {
                out.push(view);
            }
        }
        out
    }
}

/// A fake cloudflared: says where it was pointed, prints an address, waits.
const CLOUDFLARED: &str = "#!/bin/sh\necho \"$*\" > \"$0.args\"\necho '|  https://test-tunnel.trycloudflare.com  |' >&2\nexec sleep 600\n";

impl Harness {
    /// A daemon, and weft remote beside it, joined to the account as
    /// machine `m_1` of GitHub user 42 (octo).
    pub fn start(name: &str) -> Self {
        Self::with(name, |_| {})
    }

    /// The same, after `prepare` has had the Weft folder.
    pub fn with(name: &str, prepare: impl FnOnce(&Path)) -> Self {
        let dir = std::env::temp_dir().join(format!("wr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("remote")).unwrap();
        let program = dir.join("cloudflared");
        std::fs::write(&program, CLOUDFLARED).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            format!("[remote]\nenabled = true\ncloudflared = \"{}\"\n", program.display()),
        )
        .unwrap();
        weft_remote::store::Machine {
            id: "m_1".into(),
            name: "studio".into(),
            github_id: 42,
            login: "octo".into(),
            token: "mt".into(),
        }
        .save(&dir.join("remote"))
        .unwrap();
        prepare(&dir);

        let socket = weft_proto::private_socket("wr");
        let (listening, config) = (socket.clone(), dir.join("config.toml"));
        std::thread::spawn(move || {
            let _ = weftd::server::Session::serve(&listening, &config);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !weft_proto::is_live(&socket) {
            assert!(Instant::now() < deadline, "no daemon");
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut tui = Tui::connect(&socket);
        let settings = weft_remote::run::Settings {
            weft: dir.clone(),
            socket: socket.clone(),
            account: weft_remote::account::Account::at("http://127.0.0.1:9", "http://127.0.0.1:9"),
            key: fab7().verifying_key(),
        };
        std::thread::spawn(move || weft_remote::run::serve(settings).unwrap());
        tui.wait_view(|v| v["state"] == "on");
        let args = loop {
            if let Ok(a) = std::fs::read_to_string(dir.join("cloudflared.args")) {
                break a;
            }
            assert!(Instant::now() < deadline, "cloudflared never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        let port = args.trim().rsplit(':').next().unwrap().parse().unwrap();
        Harness { dir, socket, port, tui }
    }

    pub fn device(&self) -> Device {
        Device::connect(self.port)
    }

    /// A device the person allowed, said hello and was welcomed.
    pub fn allowed(&mut self, id: &str) -> (Device, Value) {
        let mut d = self.device();
        d.send(
            json!({"type": "pair", "v": 1, "device": id, "name": "iPhone", "assertion": good(id)}),
        );
        assert_eq!(d.next()["type"], "asking");
        self.tui.wait_view(|v| v["asking"]["device"] == id);
        self.tui.act(json!({"act": "allow", "device": id, "yes": true}));
        let paired = d.next();
        assert_eq!(paired["type"], "paired", "{paired}");
        let token = paired["token"].as_str().unwrap().to_string();
        d.send(json!({"type": "hello", "v": 1, "device": id, "token": token}));
        let welcome = d.next();
        assert_eq!(welcome["type"], "welcome", "{welcome}");
        (d, welcome)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let mut stop = Tui::connect(&self.socket);
        stop.call(Call::Shutdown);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub struct Device {
    pub ws: WebSocket<TcpStream>,
}

impl Device {
    pub fn connect(port: u16) -> Self {
        let mut request = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
        request.headers_mut().insert("origin", weft_remote::socket::ORIGIN.parse().unwrap());
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (ws, _) = tungstenite::client(request, stream).unwrap();
        Device { ws }
    }

    pub fn send(&mut self, message: Value) {
        self.ws.send(tungstenite::Message::text(message.to_string())).unwrap();
    }

    /// The next message, or panic within five seconds.
    pub fn next(&mut self) -> Value {
        match self.ws.read() {
            Ok(tungstenite::Message::Text(t)) => serde_json::from_str(t.as_str()).unwrap(),
            other => panic!("expected a message, got {other:?}"),
        }
    }

    /// The next message of this type, skipping others.
    pub fn next_of(&mut self, kind: &str) -> Value {
        loop {
            let m = self.next();
            if m["type"] == kind {
                return m;
            }
        }
    }

    /// Whether the socket is closed (after any messages still on their way).
    pub fn closed_within(&mut self, within: Duration) -> bool {
        self.ws.get_mut().set_read_timeout(Some(within)).unwrap();
        let deadline = Instant::now() + within;
        loop {
            match self.ws.read() {
                Ok(tungstenite::Message::Close(_)) | Err(_) if Instant::now() <= deadline => {
                    return true;
                }
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
    }
}

/// A TUI that opened a project and started a shell in it: its client, and
/// the project's folder.
pub struct Shell {
    pub tui: Tui,
    pub project: PathBuf,
}

impl Harness {
    /// A project with a shell running in it, as the TUI starts one.
    pub fn shell(&self, name: &str) -> Shell {
        let project = self.dir.join(name);
        std::fs::create_dir_all(&project).unwrap();
        let mut tui = Tui::connect(&self.socket);
        let path = project.to_string_lossy().into_owned();
        // It never reads a pane's bytes: a client that does not read them
        // would stall the daemon's broadcast.
        tui.call(Call::Open { path, rows: 24, cols: 80, raw: false });
        tui.call(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
        Shell { tui, project }
    }
}

impl Device {
    /// The agents list once it holds `n` rows.
    pub fn agents(&mut self, n: usize) -> Vec<Value> {
        loop {
            let m = self.next();
            if m["type"] == "agents" && m["agents"].as_array().is_some_and(|a| a.len() == n) {
                return m["agents"].as_array().unwrap().clone();
            }
        }
    }

    /// The answer to `req`: `ok` or `error`, skipping everything else.
    pub fn answer(&mut self, req: i64) -> Value {
        loop {
            let m = self.next();
            if (m["type"] == "ok" || m["type"] == "error") && m["req"] == req {
                return m;
            }
        }
    }
}
