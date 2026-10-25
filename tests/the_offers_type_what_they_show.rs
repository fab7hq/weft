//! What sealed work offers types what its dialog shows, into the work's own
//! agent, and nothing before Send: Undo on rejected work offers the commits
//! from the one its first Ask was sent on, and types the one picked; Follow
//! up on rejected work starts from what its Eval found and remediates that
//! Eval. An offer for the other decision is
//! refused, and there is no Commit.
//!
//! Its own test binary because it sets `HOME` for RingFrame's configuration.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ringframe::api;
use serde_json::{Value, json};
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

    fn wait<T>(&mut self, mut pick: impl FnMut(&Line) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            self.stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
            if let Ok(Some(line)) = self.lines.next()
                && let Some(found) = pick(&line)
            {
                return found;
            }
        }
        panic!("nothing arrived");
    }

    fn answer(&mut self, call: Call) -> Result<Value, String> {
        let id = self.call(call);
        self.wait(|l| match l {
            Line::Result { id: i, result } if *i == id => Some(Ok(result.clone())),
            Line::Error { id: i, code, .. } if *i == id => Some(Err(code.clone())),
            _ => None,
        })
    }
}

fn offer(act: &str, unit: &str, pane: Option<u32>, text: Option<&str>) -> Call {
    Call::Act {
        act: act.into(),
        unit: Some(unit.into()),
        pane,
        text: text.map(str::to_string),
        eval: None,
        start: None,
        draft: None,
    }
}

/// A harness `sh` whose agent keeps what it is typed, and whose file says
/// where a prompt it received is, so an Ask may go to it.
fn harness(dir: &Path) -> weft_core::harness::Harnesses {
    // Writes the agent's script; its harness file is this one instead.
    let _ = support::stub_harness(dir, "sh");
    let script = dir.join("judge.sh");
    let file = format!(
        "title = \"Stub\"\nprogram = \"/bin/sh\"\nargs = [\"{}\"]\nresume = [\"--resume\"]\n\
         [config]\ndefault = \".stub\"\n[plugin]\nlist = [\"list\"]\n\
         [signal]\nsession = \"session_id\"\nworkspace = \"cwd\"\nprompt = [\"said.text\"]\n\
         on = [{{ hook = \"Began\", state = \"ready\" }}]\n",
        script.display()
    );
    let (read, unread) = weft_core::harness::Harnesses::read(vec![("sh".to_string(), file)]);
    assert!(unread.is_empty(), "the harness file reads: {unread:?}");
    read
}

fn append(root: &Path, events: &[Value]) {
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(root.join(".fab7/rf/ledger.jsonl"))
        .expect("ledger");
    for (n, e) in events.iter().enumerate() {
        let mut e = e.clone();
        e["schema"] = json!("ringframe.ledger/1");
        e["event_id"] = json!(format!("evt_t{n}"));
        e["time"] = json!("2026-10-25T11:32:00Z");
        e["actor"] = json!({"kind": "human", "id": "local-user"});
        writeln!(f, "{e}").unwrap();
    }
}

/// An Ask judged by `eval` and sealed as `decided`.
fn judged_and_sealed(ask: &str, eval: &str, seal: &str, decided: &str) -> Vec<Value> {
    let basis = json!({"asks": [ask]});
    let links = json!([{"rel": "seals", "id": ask}, {"rel": "seals", "id": eval}]);
    vec![
        json!({"type": "eval.opened", "id": eval, "links": [], "data": {"basis": basis, "brief": {}}}),
        json!({"type": "eval.completed", "id": eval, "links": [],
               "data": {"basis": basis, "verdict": "drifted", "confidence": 0.75}}),
        json!({"type": "seal.created", "id": seal, "links": links,
               "data": {"basis": basis, "disposition": decided}}),
    ]
}

fn typed(dir: &Path, pane: u32, wanted: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(dir.join(format!("typed-{pane}"))).unwrap_or_default();
        if text.contains(wanted) {
            return text;
        }
        assert!(Instant::now() < deadline, "{wanted:?} was not typed: {text:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn each_offer_types_what_its_dialog_shows() {
    let tmp = std::env::temp_dir().join(format!("weft-offers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let files = [("src/health.rs", "pub fn health() -> u16 { 200 }\n".to_string())];
    let rejected = support::real_ask_of(&root, &home, "sh", "Add a health endpoint.", &files);
    let ws = api::workspace(&root).expect("the workspace");
    let overrides = ringframe::config::Overrides::default();
    let accepted = api::compile(
        &ws,
        api::Compile {
            source: "Show the build number.",
            harness: api::Harness { name: "sh", version: None, session_ref: None },
            capability: "human_handoff",
            classification: api::Classification {
                task: vec!["plan".into()],
                result: "plan".into(),
                effects: vec!["read".into()],
            },
            plan: None,
            practices: Vec::new(),
            classified_by: api::Chooser::Person,
            selected_by: api::Chooser::Person,
            links: Vec::new(),
            overrides: &overrides,
            via: "weft",
        },
    )
    .expect("a second Ask")
    .ask_id;
    let mut events = judged_and_sealed(&rejected, "evl_1", "sel_1", "rejected");
    events.extend(judged_and_sealed(&accepted, "evl_2", "sel_2", "accepted"));
    append(&root, &events);
    let found = json!({"findings": [{"kind": "unclaimed_rewrite",
                                     "detail": "src/db/pool.rs rewrites the retry loop"}]});
    let evl_1 = root.join(".fab7/rf/evals/evl_1");
    std::fs::create_dir_all(&evl_1).unwrap();
    std::fs::write(evl_1.join("changes.json"), found.to_string()).expect("what the Eval found");
    let base = std::fs::read_to_string(root.join(".fab7/rf/ledger.jsonl"))
        .unwrap()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|e| e["type"] == "ask.compiled" && e["id"] == rejected.as_str())
        .and_then(|e| e["data"]["base_commit"].as_str().map(str::to_string))
        .expect("RingFrame recorded the commit it was asked on");

    let socket = protocol::private_socket("weft-offers");
    let dir = support::agents_dir(&socket);
    let harnesses = harness(&dir);
    let config = socket.with_file_name("config.toml");
    std::fs::write(&config, "").expect("config");
    let serving = (socket.clone(), config);
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&serving.0, &serving.1, Some(harnesses));
    });
    let mut c = Client::connect(&socket);
    c.call(Call::Hello { client: "test".into(), protocol: PROTOCOL });
    c.answer(Call::Open { rows: 24, cols: 80, path: root.to_string_lossy().into(), raw: true })
        .expect("opened");
    let spec = format!("/bin/sh {}", dir.join("judge.sh").display());
    c.call(Call::StartAgent { harness: "sh".into(), spec, session: None });
    let pane = c.wait(|l| match l {
        Line::Event(Event::Added { pane, .. }) => Some(*pane),
        _ => None,
    });
    while !dir.join(format!("pane-{pane}")).exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    support::pane_says(&socket, &dir, &root, "sh", pane, "work-1", "ready");

    // Done lists both, each with its verdict and decision.
    let done = c
        .answer(Call::Read {
            what: "done".into(),
            unit: String::new(),
            pane: None,
            lines: None,
            before: None,
            after: None,
        })
        .expect("Done");
    let rows: Vec<(String, String, String)> = done["done"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|d| {
            let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
            (s("title"), s("verdict"), s("decision"))
        })
        .collect();
    assert_eq!(rows.len(), 2, "{done}");
    assert!(
        rows.contains(&("Add a health endpoint.".into(), "drifted".into(), "rejected".into())),
        "{done}"
    );

    // Each offer is for one decision, and the person commits for themselves.
    assert_eq!(c.answer(offer("undo", &accepted, None, None)), Err("not_offered".into()));
    assert_eq!(c.answer(offer("commit", &accepted, None, None)), Err("unknown_act".into()));

    // Undo: the commits from the one the work's first Ask was sent on up to
    // now, newest first, that one marked where it started; the commit the
    // person picked in its dialog is typed at once, with Undo's limits.
    let listed = c
        .answer(Call::Read {
            what: "commits".into(),
            unit: rejected.clone(),
            pane: None,
            lines: None,
            before: None,
            after: None,
        })
        .expect("the commits");
    let commits: Vec<(String, String, bool)> = listed["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let s = |k: &str| c[k].as_str().unwrap_or_default().to_string();
            (s("id"), s("subject"), c["start"] == true)
        })
        .collect();
    assert_eq!(commits.len(), 2, "{listed}");
    assert_eq!((commits[0].1.as_str(), commits[0].2), ("work", false), "{listed}");
    assert_eq!((commits[1].0.as_str(), commits[1].2), (base.as_str(), true), "{listed}");
    let none = offer("undo", &rejected, None, Some("0000000"));
    assert_eq!(c.answer(none), Err("no_commit".into()), "only a listed commit");
    let later = commits[0].0.clone();
    c.answer(offer("undo", &rejected, None, Some(&later))).expect("typed at once");
    let said = typed(&dir, pane, &format!("commit {later}."));
    assert!(said.contains("ask me before you discard any change"), "{said}");

    // Follow up: the sheet's words are what the Eval found, and the Ask
    // remediates that Eval.
    let remedy = c
        .answer(Call::Read {
            what: "remedy".into(),
            unit: rejected.clone(),
            pane: None,
            lines: None,
            before: None,
            after: None,
        })
        .expect("the follow-up's words");
    let words = remedy["words"].as_str().unwrap_or_default().to_string();
    assert!(words.contains("unclaimed rewrite: src/db/pool.rs rewrites the retry loop"), "{words}");
    assert_eq!((remedy["eval"].as_str(), remedy["n"].as_u64()), (Some("evl_1"), Some(1)));
    c.call(offer("follow_up", &rejected, Some(pane), Some(&words)));
    let prompt = c.wait(|l| match l {
        Line::Event(Event::Pending { payload, .. }) => {
            Some(String::from_utf8_lossy(payload).into_owned())
        }
        Line::Error { code, message, .. } => panic!("refused: {code} {message}"),
        _ => None,
    });
    assert!(prompt.contains("Read .fab7/rf/evals/evl_1/eval.md first"), "{prompt}");
    assert!(prompt.contains("src/db/pool.rs rewrites the retry loop"), "{prompt}");
    let _ = std::fs::remove_dir_all(&tmp);
}
