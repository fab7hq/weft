//! An Eval's judges are sessions Weft starts: at most the debate's `judges`
//! panes at once, in the project's folder, each typed one task's line at a
//! time and nothing else. A judge whose answer is refused is given the same
//! task again; one that exits has its task handed to another; a confirm, the
//! second reading, never goes to a session that ran a map; no judge shows as
//! an agent; and nothing is typed into the work's own pane. Stop voids an
//! Eval and closes its judges.
//!
//! The test is each judge's model: it reads the line its judge was typed,
//! writes the answer and submits it through the crate, as the judge's
//! `ringframe eval submit` would.
//!
//! Its own test binary because it sets `HOME`, which is global: one test, one
//! process.

mod support;

use std::collections::{BTreeMap, BTreeSet};
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

    /// The first event `pick` takes, within ten seconds.
    fn wait<T>(&mut self, mut pick: impl FnMut(&Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            match self.lines.next() {
                Ok(Some(Line::Event(e))) => {
                    if let Some(found) = pick(&e) {
                        return found;
                    }
                }
                Ok(Some(Line::Error { code, message, .. })) => eprintln!("no: {code} {message}"),
                _ => {}
            }
        }
        panic!("nothing arrived");
    }

    /// An act's dialog, said yes to: its title and why.
    fn act_and_say_yes(
        &mut self,
        act: &str,
        unit: Option<&str>,
        eval: Option<&str>,
    ) -> (String, String) {
        self.call(Call::Act {
            act: act.into(),
            unit: unit.map(str::to_string),
            pane: None,
            text: None,
            eval: eval.map(str::to_string),
            start: None,
            draft: None,
        });
        let (pending, what, why) = self.wait(|e| match e {
            Event::Pending { id, what, why, .. } => Some((id.clone(), what.clone(), why.clone())),
            _ => None,
        });
        self.call(Call::Resolve { pending, yes: true, force: false, to: None, start: None });
        (what, why)
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

/// One judge pane, as the test has played it.
#[derive(Default)]
struct Judge {
    ready: bool,
    /// The lines it was typed and the test has answered, in order.
    tasks: Vec<String>,
}

/// A file of about `kb` kilobytes of distinct lines.
fn big(name: &str, kb: usize) -> String {
    (0..kb * 1024 / 40).map(|i| format!("export const {name}{i} = () => {i}; // ok\n")).collect()
}

#[test]
fn the_judges_of_an_eval_are_sessions_weft_starts() {
    let tmp = std::env::temp_dir().join(format!("weft-judges-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    // Two files of 60 KB: more than one map's worth, so two maps at once.
    let files = [("src/uptime.js", big("uptime", 60)), ("src/export.js", big("export", 60))];
    let ask = support::real_ask_of(
        &root,
        &home,
        "sh",
        "Add the uptime export to src/uptime.js with a test.",
        &files,
    );

    let socket = protocol::private_socket("weft-judges");
    let dir = support::agents_dir(&socket);
    let harnesses = support::stub_harness(&dir, "sh");
    let config = socket.with_file_name("config.toml");
    std::fs::write(&config, "[eval.debate]\njudges = 2\n").expect("config");
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

    // The work's own agent, ready, which nothing must be typed into.
    let work_agent = support::agent(&tmp.join("work-agent"));
    c.call(Call::StartAgent { harness: "sh".into(), spec: work_agent, session: None });
    let work = c.wait(|e| match e {
        Event::Added { pane, .. } => Some(*pane),
        _ => None,
    });
    support::pane_says(&socket, &tmp.join("work-agent"), &root, "sh", work, "work-1", "ready");

    // One dialog; its Eval types nothing into any agent's pane.
    let (what, why) = c.act_and_say_yes("check", Some(&ask), None);
    assert_eq!(what, "Eval this work?");
    assert!(why.contains("Nothing is typed into the agent's pane."), "{why}");

    // The first judge runs its harness's own command, with its launch words.
    let (first, spec) = c.wait(|e| match e {
        Event::Added { pane, spec, .. } if *pane != work => Some((*pane, spec.clone())),
        _ => None,
    });
    let own = format!("/bin/sh {}", dir.join("judge.sh").display());
    assert!(spec.starts_with(&own), "its harness's own command: {spec}");
    // While it starts, and before it says it is ready, Weft goes on: another
    // window is answered, and the work's own output keeps arriving.
    let began = Instant::now();
    let mut other = Client::connect(&socket);
    let hello = other.call(Call::Hello { client: "other".into(), protocol: PROTOCOL });
    let answered = other
        .pump(Duration::from_secs(2))
        .iter()
        .any(|l| matches!(l, Line::Result { id, .. } if *id == hello));
    assert!(answered && began.elapsed() < Duration::from_secs(3), "another window waited");
    let opened = other.call(Call::Open {
        rows: 24,
        cols: 80,
        path: root.to_string_lossy().into_owned(),
        raw: false,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    'other: while Instant::now() < deadline {
        for line in other.pump(Duration::from_millis(100)) {
            if matches!(line, Line::Result { id, .. } if id == opened) {
                break 'other;
            }
        }
    }
    let mut watched = false;
    c.call(Call::Input { pane: work, bytes: b"still-moving\n".to_vec() });
    let moved = c.pump(Duration::from_secs(1)).iter().any(|l| {
        matches!(l, Line::Event(Event::Output { pane, bytes, .. })
            if *pane == work && String::from_utf8_lossy(bytes).contains("still-moving"))
    });
    assert!(moved, "the work's output stopped arriving");

    let ws = api::workspace(&root).expect("the workspace");
    let mut judges: BTreeMap<u32, Judge> = BTreeMap::new();
    let mut alive: BTreeSet<u32> = BTreeSet::from([first]);
    let (mut most, mut refused, mut exited) = (0, None::<String>, None::<String>);
    let mut sessions_seen: BTreeSet<String> = BTreeSet::new();
    let deadline = Instant::now() + Duration::from_secs(120);
    let eval = loop {
        if Instant::now() >= deadline {
            for (pane, j) in &judges {
                let typed =
                    std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default();
                eprintln!(
                    "pane {pane} alive {} tasks {:?} typed {} lines",
                    alive.contains(pane),
                    j.tasks,
                    support::task_lines(&typed).len()
                );
            }
            for e in events(&root, "eval.task") {
                eprintln!("{} {}", e["data"]["task_id"], e["data"]["outcome"]);
            }
            panic!("the Eval did not settle");
        }
        for line in c.pump(Duration::from_millis(100)) {
            match line {
                Line::Event(Event::Added { pane, .. }) if pane != work => {
                    alive.insert(pane);
                }
                Line::Event(Event::Panes { panes }) => {
                    alive = panes.iter().map(|p| p.pane).filter(|p| *p != work).collect();
                }
                Line::Event(Event::Agents { sessions, .. }) => {
                    sessions_seen.extend(sessions.into_iter().map(|s| s.id));
                }
                _ => {}
            }
        }
        most = most.max(alive.len());
        for &pane in &alive {
            let j = judges.entry(pane).or_default();
            if !j.ready {
                if dir.join(format!("pane-{pane}")).exists() {
                    support::pane_says(
                        &socket,
                        &dir,
                        &root,
                        "sh",
                        pane,
                        &format!("j-{pane}"),
                        "ready",
                    );
                    j.ready = true;
                }
                continue;
            }
            let typed =
                std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default();
            for (task, _, out) in support::task_lines(&typed).into_iter().skip(j.tasks.len()) {
                if support::is_confirm(&task) {
                    assert!(
                        !j.tasks
                            .iter()
                            .any(|t| support::kind_of(t) == "map" && !support::is_confirm(t)),
                        "the confirm {task} went to pane {pane}, which ran a map: {:?}",
                        j.tasks
                    );
                }
                // A judge's pane can be opened and watched: another window
                // reads its screen while it works.
                if !watched {
                    let read = other.call(Call::Read {
                        what: "history".into(),
                        unit: String::new(),
                        pane: Some(pane),
                        lines: Some(50),
                        before: None,
                        after: None,
                    });
                    let screen =
                        other.pump(Duration::from_millis(500)).into_iter().find_map(|l| match l {
                            Line::Result { id, result } if id == read => {
                                result["text"].as_str().map(str::to_string)
                            }
                            _ => None,
                        });
                    let screen = screen.expect("the judge's screen");
                    assert!(screen.contains(&format!("Judge task {task}")), "{screen}");
                    eprintln!("watched judge pane {pane}: its screen shows \"Judge task {task}\"");
                    watched = true;
                }
                j.tasks.push(task.clone());
                let says = |state: &str| {
                    support::pane_says(
                        &socket,
                        &dir,
                        &root,
                        "sh",
                        pane,
                        &format!("j-{pane}"),
                        state,
                    )
                };
                let kind = support::kind_of(&task);
                let first_map = kind == "map" && !support::is_confirm(&task);
                // The first map's first answer is refused; its judge is
                // given the task again.
                if first_map && refused.is_none() {
                    std::fs::write(&out, r#"{"not": "an answer"}"#).unwrap();
                    let said = api::eval_submit(&ws, &task, &out).expect("submitted");
                    assert_eq!(said.outcome, api::Outcome::Refused);
                    refused = Some(task.clone());
                    says("turn_ended");
                    continue;
                }
                // The second map's judge exits mid-task.
                if first_map && exited.is_none() && refused.as_ref() != Some(&task) {
                    let env = support::pane_env(&dir, pane);
                    let pid = env.iter().find(|(k, _)| k == "PID").expect("a pid").1.clone();
                    std::process::Command::new("kill").arg(&pid).status().unwrap();
                    exited = Some(task.clone());
                    continue;
                }
                let file = out.to_string_lossy().replace("/out/", "/tasks/");
                let file: Value =
                    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
                std::fs::write(&out, support::answer(kind, &file).to_string()).unwrap();
                let said = api::eval_submit(&ws, &task, &out).expect("submitted");
                assert_eq!(said.outcome, api::Outcome::Accepted, "{task}: {:?}", said.errors);
                // Only the turn's end: each signal is a connection of its own,
                // and two sent at once can arrive in either order.
                says("turn_ended");
            }
        }
        if let Some(done) = events(&root, "eval.completed").first() {
            break done["id"].as_str().expect("an Eval id").to_string();
        }
    };

    assert!(watched, "a judge's pane was watched");
    assert_eq!(most, 2, "two judges at once, and no more");
    let refused = refused.expect("an answer was refused");
    let again = judges.values().find(|j| j.tasks.iter().filter(|t| **t == refused).count() == 2);
    assert!(again.is_some(), "the refused task went to its judge again");
    let exited = exited.expect("a judge exited");
    let held = judges.values().filter(|j| j.tasks.contains(&exited)).count();
    assert_eq!(held, 2, "the task of the judge that exited went to another");
    let confirms =
        judges.values().flat_map(|j| &j.tasks).filter(|t| support::is_confirm(t)).count();
    assert!(confirms > 0, "a second reading ran");

    // No judge is an agent; the work's own agent is.
    assert!(sessions_seen.contains("work-1"), "{sessions_seen:?}");
    assert!(!sessions_seen.iter().any(|s| s.starts_with("j-")), "{sessions_seen:?}");
    let kept: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".fab7/weft/judges.json")).expect("judges.json"),
    )
    .unwrap();
    assert!(kept["sessions"].as_array().is_some_and(|s| s.len() >= 3), "{kept}");

    // The Eval settled: every judge's pane closes.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !alive.is_empty() {
        assert!(Instant::now() < deadline, "judges left open: {alive:?}");
        for line in c.pump(Duration::from_millis(100)) {
            if let Line::Event(Event::Panes { panes }) = line {
                alive = panes.iter().map(|p| p.pane).filter(|p| *p != work).collect();
            }
        }
    }

    // Each judge started in the project's folder, on the stub harness with
    // its launch words, and nothing was typed into the work's pane.
    let args = support::pane_env(&dir, *judges.keys().next().unwrap());
    let args = args.iter().find(|(k, _)| k == "ARGS").map(|(_, v)| v.clone()).unwrap_or_default();
    assert!(!args.contains("/rf:"), "{args}");
    let history = c.call(Call::Read {
        what: "history".into(),
        unit: String::new(),
        pane: Some(work),
        lines: Some(200),
        before: None,
        after: None,
    });
    let text = c
        .pump(Duration::from_millis(500))
        .into_iter()
        .find_map(|l| match l {
            Line::Result { id, result } if id == history => {
                result["text"].as_str().map(str::to_string)
            }
            _ => None,
        })
        .expect("the work pane's history");
    assert!(!text.contains("Judge task") && !text.contains("/rf:eval"), "{text}");
    let _ = eval;

    // Stop: more work, a second Eval, and Stop while its first judge works.
    std::fs::write(root.join("src/uptime.js"), big("later", 4)).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?}");
    };
    git(&["commit", "-qam", "more"]);
    std::thread::sleep(Duration::from_millis(600));
    c.act_and_say_yes("check", Some(&ask), None);
    let judge = c.wait(|e| match e {
        Event::Added { pane, .. } if *pane != work => Some(*pane),
        _ => None,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let second = loop {
        let opened = events(&root, "eval.opened");
        if opened.len() == 2 {
            break opened[1]["id"].as_str().unwrap().to_string();
        }
        assert!(Instant::now() < deadline, "no second Eval");
        std::thread::sleep(Duration::from_millis(50));
    };
    let (what, _) = c.act_and_say_yes("stop", None, Some(&second));
    assert_eq!(what, "Stop this Eval?");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut open = true;
    while events(&root, "eval.voided").is_empty() || open {
        assert!(Instant::now() < deadline, "the Eval was not stopped");
        for line in c.pump(Duration::from_millis(100)) {
            if let Line::Event(Event::Panes { panes }) = line {
                open = panes.iter().any(|p| p.pane == judge);
            }
        }
    }
    assert_eq!(events(&root, "eval.voided")[0]["id"], second.as_str());

    c.call(Call::Shutdown);
    std::fs::remove_dir_all(&tmp).ok();
}
