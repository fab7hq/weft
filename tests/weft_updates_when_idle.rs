//! Versions side by side, and an update that finishes when no agent runs
//! (ADR-0034 D5, Phase 6.9 step 6), with the real program: two versions of
//! it in a scratch home, and `weft` a link to the newest, as `install.sh`
//! lays them out.

use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};

struct Machine {
    _home: tempfile::TempDir,
    home: PathBuf,
    bin: PathBuf,
    versions: PathBuf,
    socket: PathBuf,
}

impl Machine {
    /// Version `a` installed, and `weft` naming it.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().canonicalize().unwrap();
        let (bin, versions) = (home.join("bin"), home.join(".fab7/weft/versions"));
        std::fs::create_dir_all(&bin).unwrap();
        let m =
            Machine { _home: dir, socket: home.join("run/weft/weft.sock"), home, bin, versions };
        m.install("a");
        m
    }

    /// Version `v` beside the others and `weft` naming it, as `install.sh`
    /// leaves them.
    fn install(&self, v: &str) {
        std::fs::create_dir_all(self.versions.join(v)).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_weft"), self.versions.join(v).join("weft")).unwrap();
        let link = self.bin.join("weft");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(self.versions.join(v).join("weft"), link).unwrap();
    }

    /// `weft --serve`, through the link, detached as a service would run it.
    fn serve(&self) -> u32 {
        std::fs::create_dir_all(self.home.join("run")).unwrap();
        let mut child = std::process::Command::new(self.bin.join("weft"))
            .arg("--serve")
            .env("HOME", &self.home)
            .env("XDG_RUNTIME_DIR", self.home.join("run"))
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        self.wait_live();
        let pid = child.id();
        // It runs until the test stops it; its exit is collected on the way.
        std::thread::spawn(move || child.wait());
        pid
    }

    fn wait_live(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !protocol::is_live(&self.socket) {
            assert!(Instant::now() < deadline, "the daemon never listened");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// What a process runs, as `ps` says it: its arguments.
fn args_of(pid: u32) -> String {
    let out = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Every process whose arguments hold `needle`.
fn running(needle: &str) -> Vec<String> {
    let out = std::process::Command::new("ps").args(["-axo", "args="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains(needle))
        .map(str::to_string)
        .collect()
}

struct Client {
    next: u64,
    stream: UnixStream,
    lines: Lines<UnixStream>,
    heard: Vec<Event>,
    opened: serde_json::Value,
}

impl Client {
    fn open(socket: &Path, project: &Path) -> Self {
        let stream = UnixStream::connect(socket).expect("the daemon");
        let lines = Lines::new(stream.try_clone().unwrap());
        let mut c =
            Client { next: 1, stream, lines, heard: Vec::new(), opened: Default::default() };
        c.answer(Call::Hello { client: "test".into(), protocol: PROTOCOL }).expect("hello");
        let path = project.to_string_lossy().into_owned();
        c.opened = c.answer(Call::Open { path, rows: 24, cols: 80, raw: false }).expect("open");
        c
    }

    fn answer(&mut self, call: Call) -> Result<serde_json::Value, String> {
        let id = self.next;
        self.next += 1;
        self.stream.write_all(&Line::Call { id, call }.encode()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let _ = self.stream.set_read_timeout(Some(Duration::from_millis(100)));
            match self.lines.next() {
                Ok(Some(Line::Result { id: got, result })) if got == id => return Ok(result),
                Ok(Some(Line::Error { id: got, code, .. })) if got == id => return Err(code),
                Ok(Some(Line::Event(e))) => self.heard.push(e),
                Ok(None) => return Err("closed".into()),
                _ => continue,
            }
        }
        panic!("no answer to call {id}");
    }

    /// Whether the daemon closed this connection within `within`.
    fn closed_within(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            let _ = self.stream.set_read_timeout(Some(Duration::from_millis(100)));
            match self.lines.next() {
                Ok(None) => return true,
                Ok(Some(Line::Event(e))) => self.heard.push(e),
                _ => {}
            }
        }
        false
    }
}

#[test]
fn an_update_waits_for_the_last_agent_and_the_daemon_becomes_the_new_version() {
    let m = Machine::new();
    let project = m.home.join("p");
    std::fs::create_dir_all(&project).unwrap();
    let pid = m.serve();
    let (a, b) = (m.versions.join("a/weft"), m.versions.join("b/weft"));

    let mut c = Client::open(&m.socket, &project);
    let pane =
        c.answer(Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None });
    assert!(pane.is_ok(), "{pane:?}");

    // `install.sh` put b beside a and pointed `weft` at it. `weft remote`,
    // started after that, is still the daemon's own version.
    m.install("b");
    c.answer(Call::Remote { act: serde_json::json!({"act": "on"}) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let remote = loop {
        let found = running(" remote");
        if let Some(r) = found.iter().find(|r| r.contains(m.home.to_str().unwrap())) {
            break r.clone();
        }
        assert!(Instant::now() < deadline, "weft remote never started: {found:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(remote.starts_with(a.to_str().unwrap()), "{remote} is not version a's");
    c.answer(Call::Remote { act: serde_json::json!({"act": "off"}) }).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while running(&format!("{} remote", a.display())).iter().any(|_| true) {
        assert!(Instant::now() < deadline, "weft remote did not stop");
        std::thread::sleep(Duration::from_millis(50));
    }

    // `weft update` tells it; it waits while the agent runs.
    assert_eq!(
        c.answer(Call::Update { version: "b".into() }),
        Ok(serde_json::json!({"update": "b"}))
    );
    assert!(c.heard.contains(&Event::Update { version: Some("b".into()) }), "{:?}", c.heard);
    assert_eq!(
        c.answer(Call::Update { version: "../a".into() }),
        Err("no_version".into()),
        "only a version's folder"
    );
    assert!(!c.closed_within(Duration::from_millis(1500)), "an agent runs: it waits");
    assert!(args_of(pid).contains("--serve"));

    // The last agent ends: the same process starts again as b, and the
    // connection with it.
    c.answer(Call::CloseAgent { pane: 0 }).ok();
    assert!(c.closed_within(Duration::from_secs(10)), "it did not start again");
    m.wait_live();
    let mut again = Client::open(&m.socket, &project);
    assert_eq!(args_of(pid), format!("{} --serve", b.display()), "the same process, as b");
    assert_eq!(again.opened["reopened"], serde_json::Value::Null, "an update ends nothing");
    assert_eq!(again.opened["update"], serde_json::Value::Null);

    // Nothing runs a any more: its folder is gone; b's stays.
    assert!(!a.exists(), "the old version was removed");
    assert!(b.exists());
    let _ = again.answer(Call::Shutdown);
}
