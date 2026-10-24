//! An Ask from Weft is one dialog: the Ask shows the exact prompt, Send
//! compiles it through RingFrame's crate and types it, and the harness is
//! sent nothing else, so its agent makes no tool call before the work. Cancel
//! records nothing.
//!
//! The harness is made up: `claude-code`'s RingFrame profile, with an agent
//! that writes each line it takes to a file. Its own test binary because it
//! sets `HOME` for RingFrame's configuration.

mod support;

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

const WORDS: &str = "Build Phase 1 of plans/uptime/plan.md.";

const PLAN: &str = "# Uptime\n\n## Phase 1\n\n| # | Step | Done when |\n| --- | --- | --- |\n\
                    | 1 | Add `uptime_seconds`. | A test calls it. |\n";

/// A harness file for `claude-code` whose program is the made-up agent.
fn harness(spec: &str) -> String {
    let (program, script) = spec.split_once(' ').expect("/bin/sh and a script");
    format!(
        r#"title = "Claude Code"
program = "{program}"
args = ["{script}"]
resume = ["--resume"]
[config]
default = ".claude"
[plugin]
list = ["plugins"]
[signal]
session = "session_id"
workspace = "cwd"
prompt = ["said.text"]
on = [
  {{ hook = "Began",     state = "ready" }},
  {{ hook = "Submitted", state = "working", forward = "prompt" }},
]
"#
    )
}

/// An agent that says it is ready, then writes every line it takes to
/// `<dir>/took` and reports it through the real `weft signal`.
fn agent(dir: &Path, home: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    let weft = env!("CARGO_BIN_EXE_weft");
    let (d, h) = (dir.display(), home.display());
    let script = format!(
        "#!/bin/sh\n\
         sig() {{ printf '%s' \"$2\" | HOME='{h}' '{weft}' signal claude-code \"$1\"; }}\n\
         sig Began \"{{\\\"session_id\\\":\\\"s1\\\",\\\"cwd\\\":\\\"$PWD\\\"}}\"\n\
         printf '> '\n\
         while IFS= read -r line; do\n\
           printf '%s\\n' \"$line\" >> '{d}/took'\n\
         done\n"
    );
    let path = dir.join("agent.sh");
    std::fs::write(&path, script).unwrap();
    format!("/bin/sh {}", path.display())
}

struct Client {
    stream: UnixStream,
    lines: Lines<UnixStream>,
    id: u64,
}

impl Client {
    fn call(&mut self, call: Call) -> u64 {
        self.id += 1;
        protocol::send(&mut self.stream, &Line::Call { id: self.id, call }.encode()).unwrap();
        self.id
    }

    /// The first event `pick` takes, or the refusal of call `id`.
    fn wait<T>(&mut self, id: u64, mut pick: impl FnMut(Event) -> Option<T>) -> Result<T, String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            match self.lines.next() {
                Ok(Some(Line::Error { id: got, message, .. })) if got == id => return Err(message),
                Ok(Some(Line::Event(e))) => {
                    if let Some(found) = pick(e) {
                        return Ok(found);
                    }
                }
                _ => {}
            }
        }
        panic!("nothing arrived for call {id}");
    }
}

/// Every row of the label streams beside `config`.
fn labels(config: &Path) -> Vec<serde_json::Value> {
    let dir = config.with_file_name("labels");
    let files = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let text: String = files.map(|f| std::fs::read_to_string(f.path()).unwrap()).collect();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn events(root: &Path) -> Vec<serde_json::Value> {
    let ws = ringframe::api::workspace(root).unwrap();
    ringframe::api::events(&ws).unwrap()
}

#[test]
fn an_ask_shows_its_exact_prompt_and_send_types_only_that() {
    let tmp = tempfile::tempdir().unwrap();
    let (root, home) = (tmp.path().join("project"), tmp.path().join("home"));
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this binary holds exactly one test.
    unsafe { std::env::set_var("HOME", &home) };
    let config =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/ringframe/tests/fixtures/config");
    ringframe::workspace::install_config(Some(&config)).unwrap();
    let git = support::git_repo(&root);
    std::fs::create_dir_all(root.join("plans/uptime")).unwrap();
    std::fs::write(root.join("plans/uptime/plan.md"), PLAN).unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "plan"]);
    ringframe::api::init(&ringframe::api::workspace(&root).unwrap()).unwrap();

    let dir = tmp.path().join("agent");
    let spec = agent(&dir, &home);
    // `weft signal` reads the harness file from its home.
    let files = home.join(".fab7/weft/harnesses");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("claude-code.toml"), harness(&spec)).unwrap();
    let harnesses =
        weft::harness_table::Harnesses::read(vec![("claude-code".into(), harness(&spec))]).0;
    let socket = protocol::private_socket("weft-ask-dialog");
    let config = tmp.path().join("config.toml");
    let listening = socket.clone();
    let config_at = config.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config_at, Some(harnesses));
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
    let mut c = Client { stream, lines, id: 0 };
    c.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    let path = root.to_string_lossy().into_owned();
    let open = c.call(Call::Open { rows: 24, cols: 80, path, raw: true });
    let _ = c.wait(open, |e| matches!(e, Event::Agents { .. }).then_some(()));
    let start = c.call(Call::StartAgent { harness: "claude-code".into(), spec, session: None });
    let pane = c.wait(start, |e| match e {
        Event::Added { pane, .. } => Some(pane),
        _ => None,
    });
    let pane = pane.unwrap();
    let draft = weft_core::ask::Draft {
        route: Some("native_goal".into()),
        task: Some("implement".into()),
        plan: Some("plans/uptime/plan.md".into()),
        rules: Some(vec!["practice.kiss".into()]),
        reason: None,
    };
    let ask = |c: &mut Client| {
        let id = c.call(Call::Act {
            act: "ask".into(),
            unit: None,
            pane: Some(pane),
            text: Some(WORDS.into()),
            eval: None,
            start: None,
            draft: Some(draft.clone()),
        });
        c.wait(id, |e| match e {
            Event::Pending { id, what, payload, sends, .. } => Some((id, what, payload, sends)),
            _ => None,
        })
        .expect("the Ask's dialog")
    };

    // The dialog: its title names the harness, and it shows the exact prompt.
    let (pending, what, payload, sends) = ask(&mut c);
    let shown = String::from_utf8(payload).unwrap();
    assert_eq!(what, "Ask Claude Code?");
    assert_eq!(sends, None, "nothing is compiled before Send");
    assert!(shown.starts_with(&format!("/goal {WORDS}\n")), "{shown}");
    for rule in ["- KISS: ", "- Step Loop: ", "- Done Check: "] {
        assert!(shown.contains(rule), "{rule} verbatim in {shown}");
    }
    assert!(!shown.contains("/rf:"), "no RingFrame skill is typed: {shown}");
    let compiled = |root: &Path| {
        events(root).into_iter().filter(|e| e["type"] == "ask.compiled").collect::<Vec<_>>()
    };
    assert!(compiled(&root).is_empty(), "nothing recorded before Send");

    // Cancel records nothing.
    c.call(Call::Resolve { pending, yes: false, force: false, to: None, start: None });
    std::thread::sleep(Duration::from_millis(300));
    assert!(compiled(&root).is_empty(), "a Cancel records nothing");
    assert!(labels(&config).is_empty(), "and keeps no label");

    // Send: compiled, confirmed by Weft, and typed as shown.
    let (pending, _, _, _) = ask(&mut c);
    let yes = c.call(Call::Resolve { pending, yes: true, force: false, to: None, start: None });
    let refusal = c.wait(yes, |e| match e {
        Event::Injected { refusal, .. } => Some(refusal),
        _ => None,
    });
    assert_eq!(refusal, Ok(None), "typed");
    let asks = compiled(&root);
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0]["data"]["host"]["surface"], "weft");
    let confirmed: Vec<_> =
        events(&root).into_iter().filter(|e| e["type"] == "ask.confirmed").collect();
    assert_eq!(confirmed[0]["data"]["confirmation"]["via"], "weft");
    let ask_id = asks[0]["id"].as_str().unwrap();
    let rows = labels(&config);
    assert_eq!(rows.len(), 1, "one label row: {rows:?}");
    assert_eq!((&rows[0]["kind"], &rows[0]["ask_id"]), (&"ask".into(), &ask_id.into()));
    assert_eq!(rows[0]["final"]["route"], "native_goal");
    let ws = ringframe::api::workspace(&root).unwrap();
    assert_eq!(ringframe::api::prompt(&ws, ask_id, None).unwrap().text, shown, "as shown");
    let deadline = Instant::now() + Duration::from_secs(5);
    let took = || std::fs::read_to_string(dir.join("took")).unwrap_or_default();
    while !took().contains("- Done Check: ") {
        assert!(Instant::now() < deadline, "the prompt never arrived: {:?}", took());
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(300));
    let took = took();
    assert!(took.contains("/goal") && !took.contains("/rf:"), "{took}");
    for line in shown.lines().skip(1).filter(|l| !l.is_empty()) {
        assert!(took.contains(line), "{line:?} arrived as shown: {took}");
    }
    c.call(Call::Shutdown);
}
