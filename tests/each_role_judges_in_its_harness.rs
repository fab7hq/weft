//! Each role of an Eval's debate may run in a harness of its own: its judges
//! start in that harness, with its launch words, and are typed only that
//! role's tasks. Here the map runs in the debate's harness, and the reduce
//! and the second readings in another; the requirements task goes with the
//! reduce. The record names the harness each judge ran in.
//!
//! The test is each judge's model, as in `the_judges_are_sessions`. Its own
//! test binary because it sets `HOME`.

mod support;

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ringframe::api;
use serde_json::Value;
use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

struct Client {
    stream: UnixStream,
    lines: Lines<UnixStream>,
    id: u64,
}

impl Client {
    fn connect(socket: &Path) -> Client {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            if let Ok(s) = UnixStream::connect(socket) {
                break s;
            }
            assert!(Instant::now() < deadline, "no daemon");
            std::thread::sleep(Duration::from_millis(20));
        };
        let lines = Lines::new(stream.try_clone().expect("clone"));
        Client { stream, lines, id: 0 }
    }

    fn call(&mut self, call: Call) -> u64 {
        self.id += 1;
        self.stream.write_all(&Line::Call { id: self.id, call }.encode()).expect("write");
        self.id
    }

    /// Every line that arrives within `within`.
    fn pump(&mut self, within: Duration) -> Vec<Line> {
        let deadline = Instant::now() + within;
        let mut got = Vec::new();
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(20))).expect("timeout");
            if let Ok(Some(line)) = self.lines.next() {
                got.push(line);
            }
        }
        got
    }
}

/// The ledger's events of `kind`.
fn events(root: &Path, kind: &str) -> Vec<Value> {
    std::fs::read_to_string(root.join(".fab7/rf/ledger.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["type"] == kind)
        .collect()
}

/// The harness a task runs in, by its id: a second reading in the confirm's,
/// a map in the debate's, the rest in the reduce's.
fn expected(task: &str) -> &'static str {
    match support::kind_of(task) {
        _ if support::is_confirm(task) => "sh-b",
        "map" => "sh-a",
        _ => "sh-b",
    }
}

/// One judge pane: its harness, whether it said it is ready, and the tasks
/// the test has answered.
struct Judge {
    harness: String,
    ready: bool,
    tasks: Vec<String>,
}

#[test]
fn each_judge_starts_in_its_roles_harness() {
    let tmp = std::env::temp_dir().join(format!("weft-role-harness-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let files = [("src/uptime.js", "export const uptime = () => process.uptime();\n".into())];
    let source = "Add the uptime export to src/uptime.js with a test.";
    let ask = support::real_ask_of(&root, &home, "sh-a", source, &files);

    let socket = protocol::private_socket("weft-role-harness");
    let dir = support::agents_dir(&socket);
    let harnesses = support::stub_harnesses(&dir, &["sh-a", "sh-b"]);
    let config = socket.with_file_name("config.toml");
    std::fs::write(
        &config,
        "[eval.debate]\nharness = \"sh-a\"\njudges = 2\n\
         reduce = { harness = \"sh-b\", effort = \"high\" }\nconfirm = { harness = \"sh-b\" }\n",
    )
    .expect("config");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
    let mut c = Client::connect(&socket);
    c.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    let open = c.call(Call::Open {
        rows: 24,
        cols: 80,
        path: root.to_string_lossy().into_owned(),
        raw: true,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    'opened: while Instant::now() < deadline {
        for line in c.pump(Duration::from_millis(100)) {
            if matches!(line, Line::Result { id, .. } if id == open) {
                break 'opened;
            }
        }
    }

    // One dialog, naming both harnesses; said yes to.
    c.call(Call::Act {
        act: "check".into(),
        unit: Some(ask),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let (pending, why) = 'asked: loop {
        assert!(Instant::now() < deadline, "no dialog");
        for line in c.pump(Duration::from_millis(100)) {
            if let Line::Event(Event::Pending { id, why, .. }) = line {
                break 'asked (id, why);
            }
        }
    };
    assert!(why.contains("sh-a") && why.contains("sh-b"), "{why}");
    c.call(Call::Resolve { pending, yes: true, force: false, to: None, start: None });

    let ws = api::workspace(&root).expect("the workspace");
    let mut judges: BTreeMap<u32, Judge> = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let eval = loop {
        assert!(Instant::now() < deadline, "the Eval did not settle");
        for line in c.pump(Duration::from_millis(100)) {
            if let Line::Event(Event::Added { pane, harness, spec, .. }) = line {
                // Its own harness's command, with its launch words.
                let own = format!("/bin/sh {}", dir.join(format!("judge-{harness}.sh")).display());
                assert!(spec.starts_with(&own), "{harness}: {spec}");
                judges.insert(pane, Judge { harness, ready: false, tasks: Vec::new() });
            }
        }
        for (&pane, j) in &mut judges {
            if !j.ready {
                if dir.join(format!("pane-{pane}")).exists() {
                    let id = format!("j-{pane}");
                    support::pane_says(&socket, &dir, &root, &j.harness, pane, &id, "ready");
                    j.ready = true;
                }
                continue;
            }
            let typed =
                std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default();
            for (task, _, out) in support::task_lines(&typed).into_iter().skip(j.tasks.len()) {
                assert_eq!(j.harness, expected(&task), "{task} was typed into pane {pane}");
                j.tasks.push(task.clone());
                let file = out.to_string_lossy().replace("/out/", "/tasks/");
                let file: Value =
                    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
                let kind = support::kind_of(&task);
                std::fs::write(&out, support::answer(kind, &file).to_string()).unwrap();
                let said = api::eval_submit(&ws, &task, &out).expect("submitted");
                assert_eq!(said.outcome, api::Outcome::Accepted, "{task}: {:?}", said.errors);
                let id = format!("j-{pane}");
                support::pane_says(&socket, &dir, &root, &j.harness, pane, &id, "turn_ended");
            }
        }
        if let Some(done) = events(&root, "eval.completed").first() {
            break done["id"].as_str().expect("an Eval id").to_string();
        }
    };

    // Every kind of task ran, each in its role's harness.
    let ran: Vec<&String> = judges.values().flat_map(|j| &j.tasks).collect();
    for (what, found) in [
        ("requirements", ran.iter().any(|t| support::kind_of(t) == "requirements")),
        ("a map", ran.iter().any(|t| support::kind_of(t) == "map" && !support::is_confirm(t))),
        (
            "a reduce",
            ran.iter().any(|t| support::kind_of(t) == "reduce" && !support::is_confirm(t)),
        ),
        ("a second reading", ran.iter().any(|t| support::is_confirm(t))),
    ] {
        assert!(found, "{what} ran: {ran:?}");
    }
    // A judge for the reduce started with the reduce's effort.
    for (&pane, j) in &judges {
        if j.tasks.iter().any(|t| support::kind_of(t) == "reduce" && !support::is_confirm(t)) {
            let env = support::pane_env(&dir, pane);
            let args = env.iter().find(|(k, _)| k == "ARGS").map(|(_, v)| v.clone());
            assert!(args.unwrap_or_default().contains("--effort high"), "pane {pane}");
        }
    }

    // The record names the harness each judge ran in.
    let record = api::eval_record(&ws, &eval).expect("read").expect("a record");
    let named = record["judges"].as_array().cloned().unwrap_or_default();
    assert!(!named.is_empty(), "{record}");
    for j in &named {
        let want = if j["angle"] == "map" { "sh-a" } else { "sh-b" };
        assert_eq!(j["host"], want, "{named:?}");
    }

    c.call(Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
