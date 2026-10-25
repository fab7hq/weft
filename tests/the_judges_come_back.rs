//! Weft started again finishes an open Eval: the judges it ran went with the
//! old daemon, so the new one hands their tasks out again and starts judges
//! of its own once the project is open. A judge pane names its Eval, so a
//! window lists it under that Eval.
//!
//! The test is each judge's model, as in `the_judges_are_sessions`. Its own
//! test binary because it sets `HOME`.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
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

    /// Open the project: the panes it holds, as its answer lists them.
    fn open(&mut self, root: &Path) -> Vec<weft::board::PaneInfo> {
        self.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
        let path = root.to_string_lossy().into_owned();
        let open = self.call(Call::Open { rows: 24, cols: 80, path, raw: true });
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            for line in self.pump(Duration::from_millis(100)) {
                if let Line::Result { id, result } = line
                    && id == open
                {
                    return protocol::panes_of(&result["panes"]).unwrap_or_default();
                }
            }
        }
        panic!("the project did not open");
    }
}

/// A daemon on its own socket, with the stub harness `sh` and one judge at
/// a time; the socket, and where its judges say which pane they are.
fn daemon(name: &str) -> (PathBuf, PathBuf) {
    let socket = protocol::private_socket(name);
    let dir = support::agents_dir(&socket);
    let harnesses = support::stub_harness(&dir, "sh");
    let config = socket.with_file_name("config.toml");
    std::fs::write(&config, "[eval.debate]\njudges = 1\n").expect("config");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
    (socket, dir)
}

fn events(root: &Path, kind: &str) -> Vec<Value> {
    std::fs::read_to_string(root.join(".fab7/rf/ledger.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|e| e["type"] == kind)
        .collect()
}

/// The judge panes the daemon lists now, with the Eval each judges.
fn judges_in(lines: &[Line]) -> Option<Vec<(u32, Option<String>)>> {
    lines.iter().rev().find_map(|l| match l {
        Line::Event(Event::Panes { panes }) => {
            Some(panes.iter().map(|p| (p.pane, p.judge.clone())).collect())
        }
        _ => None,
    })
}

/// Ready the judge in `pane` once it has started; then each task it was
/// typed, and where its answer goes.
fn typed_to(
    socket: &Path,
    dir: &Path,
    root: &Path,
    pane: u32,
    ready: &mut bool,
) -> Vec<(String, PathBuf)> {
    if !*ready {
        if dir.join(format!("pane-{pane}")).exists() {
            support::pane_says(socket, dir, root, "sh", pane, &format!("j-{pane}"), "ready");
            *ready = true;
        }
        return Vec::new();
    }
    let typed = std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default();
    support::task_lines(&typed).into_iter().map(|(task, _, out)| (task, out)).collect()
}

#[test]
fn a_restarted_weft_finishes_an_open_eval() {
    let tmp = std::env::temp_dir().join(format!("weft-judges-back-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let files = [("src/uptime.js", "export const uptime = () => 1;\n".to_string())];
    let ask = support::real_ask_of(
        &root,
        &home,
        "sh",
        "Add the uptime export to src/uptime.js with a test.",
        &files,
    );

    // The first daemon opens the Eval; its judge takes a task and is never
    // answered.
    let (socket, dir) = daemon("weft-judges-back-1");
    let mut c = Client::connect(&socket);
    c.open(&root);
    c.call(Call::Act {
        act: "check".into(),
        unit: Some(ask.clone()),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    });
    let mut pending = None;
    let mut seen = Vec::new();
    let (mut judge, mut ready, mut held) = (None, false, None::<String>);
    let deadline = Instant::now() + Duration::from_secs(30);
    while held.is_none() {
        assert!(Instant::now() < deadline, "no judge took a task");
        let lines = c.pump(Duration::from_millis(100));
        for line in &lines {
            match line {
                Line::Event(Event::Pending { id, .. }) if pending.is_none() => {
                    pending = Some(id.clone());
                    c.call(Call::Resolve {
                        pending: id.clone(),
                        yes: true,
                        force: false,
                        to: None,
                        start: None,
                    });
                }
                Line::Event(Event::Added { pane, .. }) => judge = Some(*pane),
                _ => {}
            }
        }
        seen.extend(lines);
        if let Some(pane) = judge {
            held = typed_to(&socket, &dir, &root, pane, &mut ready).into_iter().next().map(|t| t.0);
        }
    }
    let eval = events(&root, "eval.opened")[0]["id"].as_str().unwrap().to_string();
    let listed = judges_in(&seen).expect("the panes, told again once the judge started");
    assert_eq!(listed, [(judge.unwrap(), Some(eval.clone()))], "a judge names its Eval");
    let held = held.unwrap();
    c.call(Call::Shutdown);
    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&socket).is_ok() {
        assert!(Instant::now() < deadline, "the first daemon did not end");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(events(&root, "eval.completed").is_empty(), "the Eval is still open");

    // A second daemon: opening the project starts the Eval's judges again,
    // with nothing asked of the person, and they finish it.
    let (socket, dir) = daemon("weft-judges-back-2");
    let mut c = Client::connect(&socket);
    let opened = c.open(&root);
    let ws = api::workspace(&root).expect("the workspace");
    let mut judges: Vec<(u32, bool, usize)> = opened
        .iter()
        .filter(|p| p.judge.as_deref() == Some(eval.as_str()))
        .map(|p| (p.pane, false, 0))
        .collect();
    let mut answered = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while events(&root, "eval.completed").is_empty() {
        assert!(
            Instant::now() < deadline,
            "the Eval did not settle: {answered:?}, judges {judges:?}, {}",
            std::fs::read_to_string(root.join(".fab7/weft/judges.json")).unwrap_or_default()
        );
        for line in c.pump(Duration::from_millis(100)) {
            if let Line::Event(Event::Added { pane, .. }) = line {
                judges.push((pane, false, 0));
            }
        }
        for (pane, ready, done) in &mut judges {
            for (task, out) in typed_to(&socket, &dir, &root, *pane, ready).into_iter().skip(*done)
            {
                let file = out.to_string_lossy().replace("/out/", "/tasks/");
                let file: Value =
                    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
                std::fs::write(&out, support::answer(support::kind_of(&task), &file).to_string())
                    .unwrap();
                let said = api::eval_submit(&ws, &task, &out).expect("submitted");
                assert_eq!(said.outcome, api::Outcome::Accepted, "{task}: {:?}", said.errors);
                support::pane_says(
                    &socket,
                    &dir,
                    &root,
                    "sh",
                    *pane,
                    &format!("j-{pane}"),
                    "turn_ended",
                );
                answered.push(task);
                *done += 1;
            }
        }
    }
    assert!(!judges.is_empty(), "the new daemon started judges");
    assert!(
        answered.contains(&held),
        "the task out when Weft stopped went out again: {answered:?}"
    );
    assert_eq!(events(&root, "eval.completed")[0]["id"], eval.as_str());
    c.call(Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
