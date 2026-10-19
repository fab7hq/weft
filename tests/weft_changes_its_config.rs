//! `config.get` and `config.set`: Weft's settings read and changed from any
//! client, written to `config.toml` and applied to every open project at once,
//! with no restart. What Weft would not read is refused, and nothing is written.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};

struct Client {
    next: u64,
    stream: UnixStream,
    lines: Lines<UnixStream>,
    /// Events that came while an answer was awaited.
    heard: Vec<Event>,
}

impl Client {
    fn open(socket: &Path, project: &Path) -> (Self, Value) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "the server never came up");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().unwrap());
        let mut c = Client { next: 1, stream, lines, heard: Vec::new() };
        c.answer(Call::Hello { client: "weft-desktop".into(), protocol: PROTOCOL }).expect("hello");
        let path = project.to_string_lossy().into_owned();
        let opened = c.answer(Call::Open { path, rows: 24, cols: 80, raw: false }).expect("open");
        (c, opened)
    }

    fn answer(&mut self, call: Call) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        self.stream.write_all(&Line::Call { id, call }.encode()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, message, .. })) if got == id => return Err(message),
                Ok(Some(Line::Event(e))) => self.heard.push(e),
                Ok(None) => break,
                _ => continue,
            }
        }
        panic!("no answer to call {id}");
    }

    /// The next `config` event, from what was heard or what comes.
    fn told(&mut self) -> Option<Value> {
        if let Some(i) = self.heard.iter().position(|e| matches!(e, Event::Config { .. })) {
            let Event::Config { config } = self.heard.remove(i) else { unreachable!() };
            return Some(config);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Event(Event::Config { config }))) => return Some(config),
                Ok(Some(Line::Event(e))) => self.heard.push(e),
                _ => {}
            }
        }
        None
    }

    fn set(&mut self, scope: &str, key: &[&str], value: Option<Value>) -> Result<Value, String> {
        let key = key.iter().map(|k| k.to_string()).collect();
        self.answer(Call::ConfigSet { scope: scope.into(), key, value })
    }
}

struct World {
    root: PathBuf,
    socket: PathBuf,
    config: PathBuf,
}

impl World {
    fn new(name: &str, text: &str) -> Self {
        Self::in_world(name, text, weft::outside::Fixed::new())
    }

    fn in_world(name: &str, text: &str, world: weft::outside::Fixed) -> Self {
        let root = std::env::temp_dir().join(format!("weft-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for p in ["one", "two"] {
            std::fs::create_dir_all(root.join(p)).unwrap();
        }
        let config = root.join("config.toml");
        std::fs::write(&config, text).unwrap();
        let socket = protocol::private_socket("weft-config");
        let harnesses = weft_core::harness::fixture::harnesses();
        let world = world.shared();
        let (listening, read) = (socket.clone(), config.clone());
        std::thread::spawn(move || {
            let _ = weft::server::Session::serve_in(&listening, &read, Some(harnesses), world);
        });
        World { root, socket, config }
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.config).unwrap()
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_set_from_one_client_reaches_every_open_project_and_every_client_at_once() {
    let w = World::new("live", "# mine\n[routing]   # who does what\nask = \"codex\"\n");
    let (mut one, opened) = Client::open(&w.socket, &w.root.join("one"));
    let (mut two, _) = Client::open(&w.socket, &w.root.join("two"));
    assert_eq!(opened["turbo"], false);
    assert_eq!(opened["routing"]["acts"], json!({"ask": "codex"}));

    let got = one.answer(Call::ConfigGet).unwrap();
    assert_eq!(got["machine"]["routing"], json!({"ask": "codex"}));
    assert_eq!(got["project"], Value::Null, "nothing set for this project yet");
    assert_eq!(got["effective"]["routing"]["acts"], json!({"ask": "codex"}));

    // The machine's: every project.
    let answered = one.set("machine", &["routing", "eval"], Some(json!("claude-code"))).unwrap();
    assert_eq!(
        answered["effective"]["routing"]["acts"],
        json!({"ask": "codex", "eval": "claude-code"})
    );
    assert!(
        w.text().starts_with(
            "# mine\n[routing]   # who does what\nask = \"codex\"\neval = \"claude-code\"\n"
        ),
        "{}",
        w.text()
    );
    let heard = two.told().expect("the other project's client is told");
    assert_eq!(
        heard["effective"]["routing"]["acts"],
        json!({"ask": "codex", "eval": "claude-code"})
    );

    // One project's: that project only, and its turbo badge moves.
    one.set("machine", &["turbo"], Some(json!(true))).unwrap();
    assert_eq!(two.told().unwrap()["effective"]["turbo"], true);
    let mine = one.set("project", &["turbo"], Some(json!(false))).unwrap();
    assert_eq!(mine["effective"]["turbo"], false);
    assert_eq!(mine["project"]["turbo"], false);
    assert!(one.heard.iter().any(|e| matches!(e, Event::Turbo { on: false })), "{:?}", one.heard);
    let other = two.answer(Call::ConfigGet).unwrap();
    assert_eq!(other["effective"]["turbo"], true, "the other project keeps the machine's");

    // Taken out again: the machine's applies, and the project's table goes.
    let back = one.set("project", &["turbo"], None).unwrap();
    assert_eq!(back["effective"]["turbo"], true);
    assert!(!w.text().contains("[projects."), "{}", w.text());
    drop(two);
    one.answer(Call::Shutdown).ok();
}

#[test]
fn the_turbo_switch_writes_the_project_s_turbo() {
    let w = World::new("turbo", "");
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    assert_eq!(one.answer(Call::Turbo { on: true }).unwrap()["turbo"], true);
    let root = w.root.join("one");
    assert!(
        w.text().contains(&format!("[projects.\"{}\"]\nturbo = true", root.display())),
        "{}",
        w.text()
    );
    assert_eq!(one.answer(Call::ConfigGet).unwrap()["effective"]["turbo"], true);
    one.answer(Call::Shutdown).ok();
}

#[test]
fn what_weft_would_not_read_is_refused_and_nothing_is_written() {
    let start = "[routing]\nask = \"codex\"\n";
    let w = World::new("refused", start);
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    let why = one.set("project", &["notify"], Some(json!(false))).unwrap_err();
    assert!(why.contains("notify"), "{why}");
    let why = one.set("machine", &["turbo"], Some(json!("yes"))).unwrap_err();
    assert!(why.contains("turbo"), "{why}");
    let why = one.set("machine", &["routing", "ask"], Some(json!("aider"))).unwrap_err();
    assert!(why.contains("aider"), "{why}");
    assert!(one.set("everywhere", &["turbo"], Some(json!(true))).is_err());
    assert!(one.set("machine", &[], Some(json!(true))).is_err());
    assert_eq!(w.text(), start, "nothing written");
    one.answer(Call::Shutdown).ok();
}

/// A RingFrame setting is RingFrame's to read: a change it would refuse is
/// refused with its words, and one it reads is written and reaches the
/// `--override` every `/rf:` command carries.
#[test]
fn a_ringframe_setting_is_checked_by_ringframe_before_it_is_written() {
    let refusing = weft::outside::Fixed::new().answers(
        "ringframe",
        &["deltas", "list", "--override"],
        2,
        r#"{"error":"config","message":"practice entry mine.plain incomplete"}"#,
    );
    let w = World::in_world("rf-no", "", refusing);
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    let rule = json!([{"id": "mine.plain", "enabled": true}]);
    let key = ["ringframe", "deltas", "practices/software-development", "entries"];
    let why = one.set("project", &key, Some(rule.clone())).unwrap_err();
    assert!(why.contains("mine.plain incomplete"), "{why}");
    assert_eq!(w.text(), "", "nothing written");
    one.answer(Call::Shutdown).ok();

    let reading = weft::outside::Fixed::new().answers(
        "ringframe",
        &["deltas", "list", "--override"],
        0,
        "{}",
    );
    let w = World::in_world("rf-yes", "", reading);
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    let got = one.set("project", &key, Some(rule.clone())).unwrap();
    let layers = &got["effective"]["ringframe"];
    assert_eq!(layers[0]["layer"], "weft-project");
    assert_eq!(layers[0]["ringframe"]["deltas"]["practices/software-development"]["entries"], rule);
    one.answer(Call::Shutdown).ok();
}
