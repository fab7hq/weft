//! A send is proven by what the harness received: after
//! Enter, weftd waits for the pane's next forwarded prompt and takes
//! `ringframe observe prompt`'s verdict on it. The Ask's prompt is delivered
//! and observed; anything else is not, and the pane's notice says so; nothing
//! in 10 s is "cannot tell", with no retry. Weft never records a submission
//! of its own (`ask submitted`), and does not send to a harness whose file
//! does not say where its received prompt is.
//!
//! The harnesses are made up: `loom`, whose hooks report the line its
//! composer took, through the real `weft signal`; `weave`, the same under
//! another name, which takes a prompt with its own command in front; and
//! `mute`, whose `[signal]` names no prompt. Its own test binary because it puts a
//! stand-in `ringframe` first on PATH.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft_core::turns::Turn;

const WORDING: &str = "Ship the endpoint.";

const LOOM: &str = r#"title = "Loom"
program = "loom"
resume = ["--resume"]
[config]
default = ".loom"
[plugin]
list = ["plugins"]
[signal]
session = "session_id"
workspace = "cwd"
prompt = ["said.text", { file = "log", newest = true, last = { kind = "goal" }, field = "text" }]
on = [
  { hook = "Began",     state = "ready" },
  { hook = "Submitted", state = "working", forward = "prompt" },
  { hook = "Busy",      state = "working" },
  { hook = "Idle",      state = "turn_ended" },
  { hook = "Done",      state = "turn_ended", forward = "prompt" },
]
"#;

const MUTE: &str = r#"title = "Mute"
program = "mute"
resume = ["--resume"]
[config]
default = ".mute"
[plugin]
list = ["plugins"]
[signal]
session = "session_id"
workspace = "cwd"
on = [
  { hook = "Began", state = "ready" },
]
"#;

/// A `ringframe` that hands over the Ask's wording and logs every call into
/// the workspace it is given. `observe prompt` answers as RingFrame does:
/// the Ask's submission when what arrived is its prompt, none otherwise.
fn stand_in() -> PathBuf {
    static ONCE: Once = Once::new();
    let bin = std::env::temp_dir().join(format!("weft-proven-bin-{}", std::process::id()));
    ONCE.call_once(|| {
        std::fs::create_dir_all(&bin).unwrap();
        let script = format!(
            "#!/bin/sh\nws=$2\nprintf '%s\\n' \"$*\" >> \"$ws/calls.log\"\ncase \"$*\" in\n  \
             *\"ask copy\"*\"--host spool\"*) \
               printf '{{\"prompt\":\"{WORDING}\",\"delivery\":{{\"prefix_bytes\":0}}}}' ;;\n  \
             *\"ask copy\"*\"--host weave\"*) \
               printf '{{\"prompt\":\"/weave {WORDING}\",\"delivery\":{{\"prefix_bytes\":0}}}}' ;;\n  \
             *\"ask copy\"*) printf '{WORDING}' ;;\n  \
             *\"observe prompt\"*) got=$(cat)\n    \
               if [ \"$got\" = '{WORDING}' ] || [ \"$got\" = '/weave {WORDING}' ]; then \
                 printf '{{\"recorded\":true,\"submission\":{{\"ask_id\":\"ask_1\"}}}}'; \
               else printf '{{\"recorded\":true,\"submission\":null}}'; fi ;;\n  \
             *\"profile list\"*) printf '{{\"profiles\":[]}}' ;;\n  \
             *) printf '{{}}' ;;\nesac\nexit 0\n"
        );
        let path = bin.join("ringframe");
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: set once, before any daemon in this binary runs a program.
        unsafe { std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display())) };
    });
    bin
}

/// A made-up agent. Each line its composer takes it reports through its
/// `Submitted` hook, as `mode` says: `true` as typed, `mangle` without its
/// first two characters (a composer that swallowed the start of a paste),
/// `silent` not at all, and then ends its turn; `transcript` records it only
/// in a transcript, then a later step, and names that transcript only at the
/// end of its turn 12 s later, twice; `busy` reports each as typed, but starts
/// with a turn running that ends 3 s later; `shared` signals nothing of its
/// own, as an agent whose hooks run in its harness's shared server, and the
/// test sends them; `quiet` says nothing until its first prompt, as Codex;
/// `deaf` never shows what it is given, so a paste never lands. Every line
/// also goes to `<dir>/took`.
fn agent(dir: &Path, home: &Path, harness: &str, mode: &str) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let weft = env!("CARGO_BIN_EXE_weft");
    let (d, h) = (dir.display(), home.display());
    let script = format!(
        "#!/bin/sh\n\
         sig() {{ [ {mode} = shared ] || printf '%s' \"$2\" | HOME='{h}' '{weft}' signal {harness} \"$1\"; }}\n\
         [ {mode} = quiet ] || sig Began \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\"}}\"\n\
         if [ {mode} = busy ]; then\n\
           sig Busy \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\"}}\"\n\
           (sleep 3; sig Idle \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\"}}\") &\n\
         fi\n\
         [ {mode} = deaf ] && stty -echo\n\
         printf '> '\n\
         while IFS= read -r line; do\n\
           printf '%s\\n' \"$line\" >> '{d}/took'\n\
           case {mode} in mangle) line=${{line#??}} ;; esac\n\
           case {mode} in\n\
           silent) sig Done \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\"}}\" ;;\n\
           transcript)\n\
             printf '{{\"kind\":\"goal\",\"text\":\"%s\"}}\\n{{\"kind\":\"step\"}}\\n' \"$line\" >> '{d}/log.jsonl'\n\
             sleep 12\n\
             sig Done \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\",\\\"log\\\":\\\"{d}/log.jsonl\\\"}}\"\n\
             sig Done \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\",\\\"log\\\":\\\"{d}/log.jsonl\\\"}}\" ;;\n\
           *) sig Submitted \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\",\\\"said\\\":{{\\\"text\\\":\\\"$line\\\"}}}}\" ;;\n\
           esac\n\
         done\n"
    );
    let path = dir.join("agent.sh");
    std::fs::write(&path, script).unwrap();
    format!("/bin/sh {}", path.display())
}

/// `weave`'s file: `loom`'s, under its own name, started as the made-up
/// agent `spec` runs.
fn weave(spec: &str) -> String {
    let (program, script) = spec.split_once(' ').expect("/bin/sh and a script");
    LOOM.replace("title = \"Loom\"", "title = \"Weave\"")
        .replace("program = \"loom\"", &format!("program = \"{program}\"\nargs = [\"{script}\"]"))
}

/// `spool`'s file: `loom`'s hooks, said only from the first prompt on, and a
/// server whose stream says when a session began, ready, and where: its
/// socket is `srv.sock` in `server`.
fn spool(spec: &str, server: &Path) -> String {
    let (program, script) = spec.split_once(' ').expect("/bin/sh and a script");
    let listen = r#"
[listen]
socket = "srv.sock"
transport = "lines"
session = "params.threadId"
on = [
  { method = "thread/started", when = { "params.thread.ephemeral" = false }, session = "params.thread.id", workspace = "params.thread.cwd", state = "ready" },
]
"#;
    let file = LOOM
        .replace("title = \"Loom\"", "title = \"Spool\"")
        .replace("program = \"loom\"", &format!("program = \"{program}\"\nargs = [\"{script}\"]"))
        .replace("default = \".loom\"", &format!("default = \"{}\"", server.display()));
    format!("{file}{listen}")
}

struct Bench {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    agent: PathBuf,
    /// Where `spool`'s server keeps its socket.
    server: PathBuf,
    /// What a `spool` agent took.
    spooled: PathBuf,
    /// Where a `weave` agent, started for a send, writes what it took.
    weave: PathBuf,
    socket: PathBuf,
    stream: UnixStream,
    lines: Lines<UnixStream>,
    id: u64,
    shared: bool,
}

impl Bench {
    /// A project whose record holds one confirmed Ask for `harness`, a daemon
    /// that knows `loom` and `mute`, and that harness's agent, `mode`, in a
    /// pane that has said it is ready.
    fn new(name: &str, harness: &str, mode: &str) -> Bench {
        stand_in();
        let tmp = tempfile::tempdir().unwrap();
        let (root, home) = (tmp.path().join("project"), tmp.path().join("home"));
        std::fs::create_dir_all(root.join(".fab7/rf")).unwrap();
        let files = home.join(".fab7/weft/harnesses");
        std::fs::create_dir_all(&files).unwrap();
        std::fs::write(files.join("loom.toml"), LOOM).unwrap();
        std::fs::write(files.join("mute.toml"), MUTE).unwrap();
        let event = |kind: &str, time: &str, data: serde_json::Value| {
            serde_json::json!({"schema": "ringframe.ledger/1", "event_id": format!("evt_{kind}"),
                "type": kind, "time": time, "id": "ask_1", "actor": {"kind": "human", "id": "me"},
                "links": [], "data": data})
        };
        let ledger = [
            event(
                "ask.compiled",
                "2026-10-10T10:00:00Z",
                serde_json::json!({
                    "title": "Ship it", "selected_capability": "native_plan",
                    "delivery_mode": "human_handoff", "host": {"name": harness},
                    "source": {}, "prompt": {}, "source_verified": "exact", "limitations": [],
                    "classification": {}, "route_explanation": {}}),
            ),
            event("ask.confirmed", "2026-10-10T10:00:01Z", serde_json::json!({})),
        ];
        let text: String = ledger.iter().map(|e| format!("{e}\n")).collect();
        std::fs::write(root.join(".fab7/rf/ledger.jsonl"), text).unwrap();

        let dir = tmp.path().join("agent");
        let spec = agent(&dir, &home, harness, mode);
        let woven = tmp.path().join("weave");
        let weave = weave(&agent(&woven, &home, "weave", "true"));
        std::fs::write(files.join("weave.toml"), &weave).unwrap();
        let (spooled, server) = (tmp.path().join("spool"), tmp.path().join("server"));
        std::fs::create_dir_all(&server).unwrap();
        let spool = spool(&agent(&spooled, &home, "spool", "quiet"), &server);
        std::fs::write(files.join("spool.toml"), &spool).unwrap();
        let socket = protocol::private_socket(name);
        let (listening, config) = (socket.clone(), tmp.path().join("config.toml"));
        let harnesses = weft::harness_table::Harnesses::read(vec![
            ("loom".into(), LOOM.into()),
            ("mute".into(), MUTE.into()),
            ("weave".into(), weave),
            ("spool".into(), spool),
        ])
        .0;
        std::thread::spawn(move || {
            let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(&socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "no daemon");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().unwrap());
        let mut b = Bench {
            _tmp: tmp,
            root,
            agent: dir,
            weave: woven,
            server,
            spooled,
            socket,
            stream,
            lines,
            id: 0,
            shared: mode == "shared",
        };
        b.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let path = b.root.to_string_lossy().into_owned();
        // A window waits for its project's answer, as Weft's own do.
        let open = b.call(Call::Open { rows: 24, cols: 80, path, raw: true });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(b.next(Duration::from_millis(100)), Some(Line::Result { id, .. }) if id == open)
        {
            assert!(Instant::now() < deadline, "the project was never opened");
        }
        b.call(Call::StartAgent { harness: harness.into(), spec, session: None });
        b.wait(Duration::from_secs(5), |e| matches!(e, Event::Added { .. }).then_some(()));
        // Ready, by its hook: its turn is not running; or a busy one, running.
        // One whose hooks are its server's has said nothing yet.
        let state = if mode == "busy" { Turn::Working } else { Turn::Ready };
        let deadline = Instant::now() + Duration::from_secs(5);
        while mode != "shared" && !b.agents_are(state) {
            assert!(Instant::now() < deadline, "the agent never said it was {state:?}");
        }
        b
    }

    fn call(&mut self, call: Call) -> u64 {
        self.id += 1;
        protocol::send(&mut self.stream, &Line::Call { id: self.id, call }.encode()).unwrap();
        self.id
    }

    fn next(&mut self, within: Duration) -> Option<Line> {
        self.stream.set_read_timeout(Some(within)).unwrap();
        self.lines.next().ok().flatten()
    }

    fn wait<T>(&mut self, within: Duration, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Some(Line::Event(e)) = self.next(Duration::from_millis(100))
                && let Some(found) = pick(e)
            {
                return found;
            }
        }
        panic!("nothing arrived in {within:?}");
    }

    fn agents_are(&mut self, state: Turn) -> bool {
        match self.next(Duration::from_millis(100)) {
            Some(Line::Event(Event::Agents { panes, .. })) => panes.contains(&Some(state)),
            _ => false,
        }
    }

    /// The person's yes to sending the Ask: what the daemon answered the
    /// send itself, when it refused it, else the first `injected` after.
    fn send(&mut self) -> Result<Option<String>, String> {
        self.send_to(None, None)
    }

    /// As `send`, into another agent here (`to`) or a new one (`start`).
    fn send_to(&mut self, to: Option<u32>, start: Option<&str>) -> Result<Option<String>, String> {
        let act = self.call(Call::Act {
            act: "send".into(),
            unit: Some("ask_1".into()),
            pane: None,
            text: None,
            eval: None,
            start: None,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let pending = loop {
            assert!(Instant::now() < deadline, "the send was never staged");
            match self.next(Duration::from_millis(100)) {
                Some(Line::Error { id, message, .. }) if id == act => return Err(message),
                Some(Line::Event(Event::Pending { id, .. })) => break id,
                _ => {}
            }
        };
        // An agent that has said nothing is sent to on the person's say-so.
        let start = start.map(str::to_string);
        let yes = self.call(Call::Resolve { pending, yes: true, force: self.shared, to, start });
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            match self.next(Duration::from_millis(100)) {
                Some(Line::Error { id, message, .. }) if id == yes => return Err(message),
                Some(Line::Event(Event::Injected { refusal, .. })) => return Ok(refusal),
                _ => {}
            }
        }
        panic!("nothing was typed");
    }

    /// Start a `weave` agent here, as the person would; its pane, once it
    /// said it is ready.
    fn start_weave(&mut self) -> u32 {
        let spec = format!("/bin/sh {}", self.weave.join("agent.sh").display());
        self.call(Call::StartAgent { harness: "weave".into(), spec, session: None });
        let pane = self.wait(Duration::from_secs(5), |e| match e {
            Event::Added { pane, harness, .. } if harness == "weave" => Some(pane),
            _ => None,
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.agents_are_all(Turn::Ready) {
            assert!(Instant::now() < deadline, "weave never said it was ready");
        }
        pane
    }

    fn agents_are_all(&mut self, state: Turn) -> bool {
        match self.next(Duration::from_millis(100)) {
            Some(Line::Event(Event::Agents { panes, .. })) => {
                panes.len() > 1 && panes.iter().all(|p| *p == Some(state))
            }
            _ => false,
        }
    }

    fn woven(&self) -> String {
        std::fs::read_to_string(self.weave.join("took")).unwrap_or_default()
    }

    /// The next `injected` within `within`, if one comes.
    fn notice(&mut self, within: Duration) -> Option<Option<String>> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Some(Line::Event(Event::Injected { refusal, .. })) =
                self.next(Duration::from_millis(100))
            {
                return Some(refusal);
            }
        }
        None
    }

    /// A hook of session `s1` that ran in its harness's shared server: it
    /// names no pane.
    fn shared_prompt(&self, prompt: &str) {
        let sent = weft_core::signal::Sent {
            harness: "loom".into(),
            hook: "Submitted".into(),
            session: "s1".into(),
            workspace: Some(self.root.to_string_lossy().into_owned()),
            state: Some("working".into()),
            forward: Some("prompt".into()),
            prompt: Some(prompt.into()),
            shared: true,
            ..Default::default()
        };
        let mut s = UnixStream::connect(&self.socket).unwrap();
        use std::io::Write;
        s.write_all(&Line::Event(Event::Signal(Box::new(sent))).encode()).unwrap();
    }

    /// The session Weft's pane map ties pane 0 to, within `within`.
    fn tied(&self, within: Duration) -> Option<String> {
        let deadline = Instant::now() + within;
        loop {
            let map = std::fs::read(self.root.join(".fab7/weft/panes.json")).unwrap_or_default();
            let map: serde_json::Value = serde_json::from_slice(&map).unwrap_or_default();
            if let Some(s) = map["panes"][0]["session"]["session"].as_str() {
                return Some(s.to_string());
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.root.join("calls.log")).unwrap_or_default()
    }

    fn took(&self) -> String {
        std::fs::read_to_string(self.agent.join("took")).unwrap_or_default()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.call(Call::Shutdown);
        let _ = std::fs::remove_file(&self.socket);
    }
}

#[test]
fn the_asks_prompt_arriving_is_the_delivery_and_weft_records_none_of_its_own() {
    let mut b = Bench::new("weft-proven-true", "loom", "true");
    assert_eq!(b.send(), Ok(None), "typed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !b.calls().contains("observe prompt") {
        assert!(
            Instant::now() < deadline,
            "no verdict was asked for: {} took {:?}",
            b.calls(),
            b.took()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(b.notice(Duration::from_secs(2)), None, "delivered: nothing to tell the person");
    assert!(!b.calls().contains("ask submitted"), "the receipt is the record:\n{}", b.calls());
    assert_eq!(b.took(), format!("{WORDING}\n"), "typed once");
}

/// Typing waits on the pane (here the 2.5 s for a paste that never shows),
/// never on the daemon: it answers every call while the typing waits.
#[test]
fn a_send_that_waits_on_its_pane_leaves_the_daemon_answering() {
    let mut b = Bench::new("weft-proven-deaf", "loom", "deaf");
    b.call(Call::Act {
        act: "send".into(),
        unit: Some("ask_1".into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
    });
    let pending = b.wait(Duration::from_secs(5), |e| match e {
        Event::Pending { id, .. } => Some(id),
        _ => None,
    });
    b.call(Call::Resolve { pending, yes: true, force: false, to: None, start: None });
    // A call every 100 ms until the typing gives up, each timed to its answer.
    let mut asked = std::collections::HashMap::new();
    let mut slowest = Duration::ZERO;
    let mut said = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    while (said.is_none() || !asked.is_empty()) && Instant::now() < deadline {
        if said.is_none() {
            let id = b.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
            asked.insert(id, Instant::now());
        }
        let next = Instant::now() + Duration::from_millis(100);
        while Instant::now() < next {
            match b.next(Duration::from_millis(20)) {
                Some(Line::Result { id, .. }) => {
                    if let Some(at) = asked.remove(&id) {
                        slowest = slowest.max(at.elapsed());
                    }
                }
                Some(Line::Event(Event::Injected { refusal, .. })) => said = Some(refusal),
                _ => {}
            }
        }
    }
    assert_eq!(said, Some(Some("PasteNotShown".into())), "Enter withheld");
    assert!(asked.is_empty(), "every call answered");
    assert!(slowest < Duration::from_millis(500), "a call waited {slowest:?} on the typing");
}

/// 37ed5a4: a composer that swallowed the start of the paste submitted a
/// prompt nobody asked for. It is not recorded as the Ask, and the person is
/// told at once.
#[test]
fn another_prompt_arriving_is_not_the_ask_and_the_person_is_told() {
    let mut b = Bench::new("weft-proven-mangle", "loom", "mangle");
    assert_eq!(b.send(), Ok(None), "typed");
    let said = b.notice(Duration::from_secs(5)).expect("the person is told");
    assert_eq!(said.as_deref(), Some("NotTheAsk"));
    assert!(!b.calls().contains("ask submitted"), "{}", b.calls());
    assert_eq!(b.took(), format!("{WORDING}\n"), "no retry, no keys pressed");
}

/// A turn that ends with nothing received is "cannot tell" at once: the
/// wait ends at the turn's end, not after a fixed time. No
/// retry.
#[test]
fn a_turn_that_ends_with_nothing_received_is_cannot_tell_and_no_retry() {
    let mut b = Bench::new("weft-proven-silent", "loom", "silent");
    let sent = Instant::now();
    assert_eq!(b.send(), Ok(None), "typed");
    let said = b.notice(Duration::from_secs(8)).expect("the person is told");
    assert_eq!(said.as_deref(), Some("NoReceipt"));
    assert!(sent.elapsed() < Duration::from_secs(8), "at the turn's end: {:?}", sent.elapsed());
    assert!(!b.calls().contains("ask submitted"), "{}", b.calls());
    assert!(!b.calls().contains("observe prompt"), "{}", b.calls());
    assert_eq!(b.took(), format!("{WORDING}\n"), "no retry");
}

/// A harness that records the prompt only in its transcript, and names the
/// transcript only when its turn ends, past the old 10 s: the send is
/// delivered when that hook comes, and the same line is forwarded
/// once.
#[test]
fn a_prompt_recorded_only_in_a_transcript_arrives_with_a_later_hook() {
    let mut b = Bench::new("weft-proven-transcript", "loom", "transcript");
    assert_eq!(b.send(), Ok(None), "typed");
    assert_eq!(b.notice(Duration::from_secs(16)), None, "delivered: nothing to tell the person");
    let deadline = Instant::now() + Duration::from_secs(5);
    while b.calls().matches("observe prompt").count() < 1 {
        assert!(Instant::now() < deadline, "no verdict was asked for: {}", b.calls());
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(b.calls().matches("observe prompt").count(), 1, "once: {}", b.calls());
    assert!(!b.calls().contains("ask submitted"), "{}", b.calls());
}

/// An agent whose turn is running is not typed into: the send waits until
/// its hooks say the turn ended, then goes, and is proven as any other.
#[test]
fn a_busy_agent_is_waited_for() {
    let mut b = Bench::new("weft-proven-busy", "loom", "busy");
    let asked = Instant::now();
    assert_eq!(b.send(), Ok(None), "typed once the turn ended");
    assert!(
        asked.elapsed() >= Duration::from_secs(2),
        "not while it worked: {:?}",
        asked.elapsed()
    );
    assert_eq!(b.notice(Duration::from_secs(2)), None, "and it arrived as the Ask");
    assert_eq!(b.took(), format!("{WORDING}\n"));
}

#[test]
fn a_harness_that_does_not_say_where_its_prompt_is_is_not_sent_to() {
    let mut b = Bench::new("weft-proven-mute", "mute", "true");
    let refused = b.send().expect_err("refused before anything is staged");
    assert!(refused.contains("Mute"), "names the harness: {refused}");
    assert!(refused.contains("prompt"), "and why: {refused}");
    assert_eq!(b.took(), "", "nothing typed");
}

/// A fresh pane whose harness's hooks run in a shared server names no pane:
/// the prompt Weft typed coming back, judged the Ask, ties the pane to that
/// session.
#[test]
fn a_fresh_pane_under_a_shared_server_is_tied_by_wefts_send() {
    let mut b = Bench::new("weft-proven-shared", "loom", "shared");
    assert_eq!(b.send(), Ok(None), "typed");
    b.shared_prompt(WORDING);
    assert_eq!(b.tied(Duration::from_secs(5)).as_deref(), Some("s1"));
    assert_eq!(b.notice(Duration::from_secs(1)), None, "delivered");
}

/// Another prompt coming back is not the Ask: the person is told, and the
/// pane is tied to nothing.
#[test]
fn a_shared_servers_prompt_that_is_not_the_ask_ties_nothing() {
    let mut b = Bench::new("weft-proven-shared-other", "loom", "shared");
    assert_eq!(b.send(), Ok(None), "typed");
    b.shared_prompt("something else");
    let said = b.notice(Duration::from_secs(5)).expect("the person is told");
    assert_eq!(said.as_deref(), Some("NotTheAsk"));
    assert_eq!(b.tied(Duration::from_millis(500)), None);
}

/// A send can go into another agent running here, of another harness: it
/// takes the same wording in its own way, and what it reports receiving is
/// the Ask's arrival.
#[test]
fn a_send_goes_into_another_agent_here_in_its_harnesss_own_way() {
    let mut b = Bench::new("weft-elsewhere-pane", "loom", "true");
    let pane = b.start_weave();
    assert_eq!(b.send_to(Some(pane), None), Ok(None), "typed");
    assert_eq!(b.notice(Duration::from_secs(3)), None, "delivered: nothing to tell the person");
    assert_eq!(b.woven(), format!("/weave {WORDING}\n"));
    assert_eq!(b.took(), "", "not into the agent that wrote it");
}

/// A send can go into a new agent: Weft starts it on the person's yes, waits
/// until it is ready, then types.
#[test]
fn a_send_goes_into_a_new_agent_started_for_it() {
    let mut b = Bench::new("weft-elsewhere-new", "loom", "true");
    assert_eq!(b.send_to(None, Some("weave")), Ok(None), "typed");
    assert_eq!(b.notice(Duration::from_secs(3)), None, "delivered");
    assert_eq!(b.woven(), format!("/weave {WORDING}\n"));
    assert_eq!(b.took(), "");
}

/// No new agent is started for a send Weft could not prove, or of a harness
/// that is not set up here; nothing is typed.
#[test]
fn a_new_agent_weft_could_not_prove_a_send_to_is_not_started() {
    let mut b = Bench::new("weft-elsewhere-refused", "loom", "true");
    let refused = b.send_to(None, Some("mute")).expect_err("no receipt");
    assert!(refused.contains("Mute"), "{refused}");
    let refused = b.send_to(None, Some("nowhere")).expect_err("not set up");
    assert!(refused.contains("not set up"), "{refused}");
    assert_eq!((b.took(), b.woven()), (String::new(), String::new()));
}

/// An Ask can go into a new agent: started on the person's Enter, and the
/// Ask staged for it there.
#[test]
fn an_ask_into_a_new_agent_starts_it_and_stages_the_ask_there() {
    let mut b = Bench::new("weft-elsewhere-ask", "loom", "true");
    b.call(Call::Act {
        act: "ask".into(),
        unit: None,
        pane: None,
        text: Some("Ship the docs.".into()),
        eval: None,
        start: Some("weave".into()),
    });
    let started = b.wait(Duration::from_secs(5), |e| match e {
        Event::Added { pane, harness, .. } if harness == "weave" => Some(pane),
        _ => None,
    });
    let staged = b.wait(Duration::from_secs(5), |e| match e {
        Event::Pending { pane, sends, .. } => Some((pane, sends)),
        _ => None,
    });
    assert_eq!(staged, (started, None), "an Ask sends no Ask's prompt yet");
}

/// A new agent of a harness that says nothing until its first prompt is ready
/// when its server's stream says its session began in this project: the one
/// fresh agent of that harness here is tied to it, and typed into. The server
/// starts after the agent, as Codex's first TUI starts it.
#[test]
fn a_new_agent_is_ready_when_its_servers_stream_says_its_session_began() {
    let mut b = Bench::new("weft-stream-began", "loom", "true");
    let (server, root) = (b.server.join("srv.sock"), b.root.clone());
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let listener = std::os::unix::net::UnixListener::bind(&server).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        // Another program's short session in the same directory first.
        for (id, ephemeral) in [("t-title", true), ("s1", false)] {
            let line = serde_json::json!({"method": "thread/started", "params": {"thread": {
                "id": id, "cwd": root, "ephemeral": ephemeral, "status": {"type": "idle"}}}});
            use std::io::Write;
            writeln!(stream, "{line}").unwrap();
        }
        std::thread::sleep(Duration::from_secs(30));
    });
    assert_eq!(b.send_to(None, Some("spool")), Ok(None), "typed, unforced");
    assert_eq!(b.notice(Duration::from_secs(3)), None, "delivered");
    let took = std::fs::read_to_string(b.spooled.join("took")).unwrap_or_default();
    assert_eq!(took, format!("{WORDING}\n"));
    assert_eq!(b.took(), "", "not into the agent that wrote it");
}
