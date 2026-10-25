//! After an accepted Seal, a rule is kept in one dialog, the client's.
//! Opening it types the draft request into the work's agent, which writes
//! the draft to the file the request names, and every client is told the
//! draft. Save keeps the rule, as the person edited it, in their own layer,
//! or for the team in the project's `.fab7/rules.toml`, through RingFrame.
//!
//! The test is the agent's model: it reads the request it was typed and
//! writes the draft where the request says.
//!
//! Its own test binary because it sets `HOME`, which is global: one test, one
//! process.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use ringframe::api;
use serde_json::{Value, json};
use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft_core::after_seal::Drafted;

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

    /// The first event or answer `pick` takes, within ten seconds.
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

    /// A call's answer: its result, or its refusal's code.
    fn answer(&mut self, call: Call) -> Result<Value, String> {
        let id = self.call(call);
        self.wait(|l| match l {
            Line::Result { id: i, result } if *i == id => Some(Ok(result.clone())),
            Line::Error { id: i, code, .. } if *i == id => Some(Err(code.clone())),
            _ => None,
        })
    }
}

fn act(act: &str, unit: &str, draft: Option<weft_core::ask::Draft>) -> Call {
    Call::Act {
        act: act.into(),
        unit: Some(unit.into()),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft,
    }
}

/// Lines appended to the ledger as RingFrame would write them.
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

#[test]
fn a_kept_rule_lands_in_the_layer_chosen() {
    let tmp = std::env::temp_dir().join(format!("weft-rule-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let files = [("src/health.rs", "pub fn health() -> u16 { 200 }\n".to_string())];
    let ask = support::real_ask_of(&root, &home, "sh", "Add a health endpoint.", &files);
    let ws = api::workspace(&root).expect("the workspace");
    let overrides = ringframe::config::Overrides::default();
    let follow_up = api::compile(
        &ws,
        api::Compile {
            source: "Return 503 while the database is down.",
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
            links: vec![api::Link { rel: "follows", id: &ask }],
            overrides: &overrides,
            via: "weft",
        },
    )
    .expect("the follow-up")
    .ask_id;
    // Judged aligned and sealed accepted, as RingFrame records it.
    let basis = json!({"asks": [ask, follow_up]});
    let seals = json!([{"rel": "seals", "id": ask}, {"rel": "seals", "id": follow_up},
                       {"rel": "seals", "id": "evl_1"}]);
    append(
        &root,
        &[
            json!({"type": "eval.opened", "id": "evl_1", "links": [],
                   "data": {"basis": basis, "brief": {}}}),
            json!({"type": "eval.completed", "id": "evl_1", "links": [],
                   "data": {"basis": basis, "verdict": "aligned", "confidence": 1.0}}),
            json!({"type": "seal.created", "id": "sel_1", "links": seals,
                   "data": {"basis": basis, "disposition": "accepted"}}),
        ],
    );

    let socket = protocol::private_socket("weft-rule");
    let dir = support::agents_dir(&socket);
    let harnesses = support::stub_harness(&dir, "sh");
    let config = socket.with_file_name("config.toml");
    std::fs::write(&config, "").expect("config");
    let serving = (socket.clone(), config.clone());
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

    // Keep a rule is one dialog, the client's: opening it has the work's
    // agent write the draft at once, with no dialog of Weft's before.
    c.answer(act("draft_rule", &ask, None)).expect("drafting");

    // The agent reads what it was typed and writes its draft where it says.
    let typed_file = dir.join(format!("typed-{pane}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    let typed = loop {
        let typed = std::fs::read_to_string(&typed_file).unwrap_or_default();
        if typed.contains("- Return 503 while the database is down.") {
            assert!(typed.contains("What I asked: Add a health endpoint."), "{typed}");
            break typed;
        }
        assert!(Instant::now() < deadline, "nothing was typed: {typed:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    let path = typed
        .split("Write it to ")
        .nth(1)
        .and_then(|rest| rest.split(" as these four lines").next())
        .expect("the request names its file")
        .to_string();
    std::fs::write(
        &path,
        "name: Degraded health\nrule: Health checks answer 503 when a dependency is down.\n\
         when: The work changes a health endpoint.\nfamily: domain\n",
    )
    .expect("the draft");
    let drafted = c.wait(|l| match l {
        Line::Event(Event::Drafted { unit, harness, rule }) if *unit == ask => {
            assert_eq!(harness, "sh");
            Some(rule.clone())
        }
        _ => None,
    });
    assert_eq!(drafted.rule, "Health checks answer 503 when a dependency is down.");
    assert!(!Path::new(&path).exists(), "the draft file is taken once read");

    // Dialog two: the client's. Save keeps the rule as edited, in the
    // person's own layer.
    let kept = |rule: &str, share: bool| weft_core::ask::Draft {
        rule: Some(Box::new(Drafted { rule: rule.into(), ..drafted.clone() })),
        share,
        ..Default::default()
    };
    let mine = "Health checks answer 503 and name what is down.";
    let saved = c.answer(act("keep_rule", &ask, Some(kept(mine, false)))).expect("kept");
    assert_eq!(saved["layer"], "weft");
    let file: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
    let entries = &file["ringframe"]["deltas"]["practices/software-development"]["entries"];
    let own = entries.as_array().and_then(|e| e.first()).expect("the person's rule");
    assert_eq!(own["text"].as_str(), Some(mine));
    assert_eq!(own["label"].as_str(), Some("Degraded health"));
    assert_eq!(own["when"].as_str(), Some("The work changes a health endpoint."));
    assert_eq!(own["family"].as_str(), Some("domain"));
    assert_eq!(own["applies_to"]["task"].as_array().map(Vec::len), Some(1));
    assert_eq!(saved["id"].as_str(), own["id"].as_str());

    // Shared with the team, it goes to the project's file and not the person's.
    let team = "A health check names each dependency it waited on.";
    let shared = c.answer(act("keep_rule", &ask, Some(kept(team, true)))).expect("shared");
    assert_eq!(shared["layer"], "repo");
    let repo = std::fs::read_to_string(root.join(".fab7/rules.toml")).expect("the team's file");
    assert!(repo.contains(team), "{repo}");
    let config_text = std::fs::read_to_string(&config).unwrap();
    assert!(!config_text.contains(team), "{config_text}");
    assert!(config_text.contains(mine), "the person's rule stays theirs");

    // Work not sealed accepted offers no rule.
    let refused = c.answer(act("keep_rule", "ask_none", Some(kept(team, false))));
    assert_eq!(refused, Err("no_unit".into()));
    let _ = std::fs::remove_dir_all(&tmp);
}
