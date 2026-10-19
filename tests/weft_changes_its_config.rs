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

    // Every project's turbo: every project's badge moves.
    one.set("machine", &["turbo"], Some(json!(true))).unwrap();
    assert_eq!(two.told().unwrap()["effective"]["turbo"], true);
    assert!(one.heard.iter().any(|e| matches!(e, Event::Turbo { on: true })), "{:?}", one.heard);

    // One project's: that project only.
    let mine = one.set("project", &["routing", "eval"], Some(json!("codex"))).unwrap();
    assert_eq!(mine["effective"]["routing"]["acts"]["eval"], "codex");
    assert_eq!(mine["project"]["routing"]["eval"], "codex");
    let other = two.answer(Call::ConfigGet).unwrap();
    assert_eq!(
        other["effective"]["routing"]["acts"]["eval"], "claude-code",
        "the other keeps every project's"
    );

    // Taken out again: every project's applies, and the project's table goes.
    let back = one.set("project", &["routing", "eval"], None).unwrap();
    assert_eq!(back["effective"]["routing"]["acts"]["eval"], "claude-code");
    assert!(!w.text().contains("[projects."), "{}", w.text());
    drop(two);
    one.answer(Call::Shutdown).ok();
}

/// Turbo is every project's only: the turbo switch writes the machine's, and
/// a project's own is refused.
#[test]
fn turbo_is_every_project_s_only() {
    let w = World::new("turbo", "");
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    assert_eq!(one.answer(Call::Turbo { on: true }).unwrap()["turbo"], true);
    assert_eq!(w.text(), "turbo = true\n");
    assert_eq!(one.answer(Call::ConfigGet).unwrap()["effective"]["turbo"], true);
    let why = one.set("project", &["turbo"], Some(json!(false))).unwrap_err();
    assert!(why.contains("turbo"), "{why}");
    assert_eq!(w.text(), "turbo = true\n", "nothing written");
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

/// What the RingFrame tab reads, through the daemon: the practices and every
/// rule with its source, and the Rules block a route would get.
#[test]
fn the_ringframe_tab_reads_rules_and_a_preview_through_the_daemon() {
    let world = weft::outside::Fixed::new()
        .answers("ringframe", &["deltas", "domains"], 0, r#"{"domains":[{"domain":"autonomous-trading","base":false}]}"#)
        .answers("ringframe", &["deltas", "list"], 0, r#"{"catalogs":[{"catalog":"practices/software-development","entries":[{"id":"practice.kiss","source":"weft"}]}]}"#)
        .answers("ringframe", &["deltas", "render", "--host", "claude-code", "--capability", "native_plan"], 0, "Rules:\n- KISS: keep it plain\n");
    let w = World::in_world(
        "rules",
        "[ringframe.deltas.\"practices/software-development\"]\nentries = [{ id = \"practice.kiss\", enabled = false }]\n",
        world,
    );
    let (mut one, _) = Client::open(&w.socket, &w.root.join("one"));
    let read = |c: &mut Client, what: &str, unit: &str| {
        c.answer(Call::Read {
            what: what.into(),
            unit: unit.into(),
            pane: None,
            lines: None,
            before: None,
            after: None,
        })
    };
    let rules = read(&mut one, "rules", "machine").unwrap();
    assert_eq!(rules["domains"][0]["domain"], "autonomous-trading");
    assert_eq!(rules["catalogs"][0]["entries"][0]["source"], "weft");
    let preview = read(&mut one, "preview", "claude-code|native_plan|implement").unwrap();
    assert_eq!(preview["text"], "Rules:\n- KISS: keep it plain\n");
    assert!(
        read(&mut one, "preview", "claude-code").is_err(),
        "a preview names a host, a route and a task"
    );
    one.answer(Call::Shutdown).ok();
}
