//! A mark on a closed Eval is one dialog: "I asked for this" on a change the
//! judges left unexplained or split on, "Not what I meant" on a requirement.
//! Its Mark appends one `mark` row to the project's label stream; Cancel keeps
//! none, and the judges' record is never rewritten.
//!
//! The Eval is a real one RingFrame closed on a planted change, put on disk
//! as RingFrame left it.

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

const EVAL: &str = "evl_01M5R4X3GVVV0Z2CD1F32EZ99W";
const ASK: &str = "ask_01M37RDYJCHWWSD12A5DGEYE08";
const SPLIT: &str = "w_3ae3752a6a3b";
const REQUIRED: &str = "w_65bbe3bb595a";

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

    /// Ask to mark `item` of the Eval: its dialog's title and lines, or the
    /// refusal.
    fn mark(&mut self, item: &str) -> Result<(String, String, String), String> {
        let id = self.call(Call::Act {
            act: "mark".into(),
            unit: None,
            pane: None,
            text: Some(item.into()),
            eval: Some(EVAL.into()),
            start: None,
            draft: None,
        });
        self.wait(id, |e| match e {
            Event::Pending { id, what, why, .. } => Some((id, what, why)),
            _ => None,
        })
    }

    fn answer(&mut self, pending: String, yes: bool) {
        self.call(Call::Resolve { pending, yes, force: false, to: None, start: None });
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Every row of the label streams beside `config`.
fn labels(config: &Path) -> Vec<Value> {
    let dir = config.with_file_name("labels");
    let files = std::fs::read_dir(dir).into_iter().flatten().flatten();
    let text: String = files.map(|f| std::fs::read_to_string(f.path()).unwrap()).collect();
    text.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn event(kind: &str, id: &str, data: Value) -> String {
    json!({"schema": "ringframe.ledger/1", "event_id": format!("evt_{kind}_{id}"), "type": kind,
           "time": "2026-10-24T23:40:00Z", "id": id,
           "actor": {"kind": "human", "id": "local-user"}, "links": [], "data": data})
    .to_string()
}

/// The closed Eval's files, its judges' answers, and the ledger lines that
/// name them, under `root`.
fn closed_eval(root: &Path) {
    let rf = root.join(".fab7/rf");
    let dir = rf.join("evals").join(EVAL);
    std::fs::create_dir_all(dir.join("tasks")).unwrap();
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/weft-core/tests/fixtures/eval-split");
    for f in ["record", "changes", "evidence", "brief", "windows"] {
        std::fs::copy(fixtures.join(format!("{f}.json")), dir.join(format!("{f}.json"))).unwrap();
    }
    let outputs: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixtures.join("outputs.json")).unwrap()).unwrap();
    let mut ledger = vec![
        event(
            "ask.compiled",
            ASK,
            json!({"title": "Build account erasure", "selected_capability": "native_goal",
                   "delivery_mode": "native_goal", "host": {"name": "codex"}}),
        ),
        event("eval.opened", EVAL, json!({"brief": {}, "basis": {"asks": [ASK]}})),
    ];
    for out in &outputs {
        let task = out["task_id"].as_str().unwrap();
        let path = format!("evals/{EVAL}/tasks/{}.json", task.replace('~', "."));
        std::fs::write(rf.join(&path), out.to_string()).unwrap();
        ledger.push(event(
            "eval.task",
            EVAL,
            json!({"eval_id": EVAL, "task_id": task, "outcome": "accepted",
                   "artifact": {"path": path}}),
        ));
    }
    ledger.push(event(
        "eval.completed",
        EVAL,
        json!({"verdict": "incomplete", "confidence": 1.0, "basis": {"asks": [ASK]}}),
    ));
    std::fs::write(rf.join("ledger.jsonl"), ledger.join("\n") + "\n").unwrap();
}

#[test]
fn a_mark_shows_its_item_and_mark_appends_one_label_row() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("project");
    std::fs::create_dir_all(&root).unwrap();
    closed_eval(&root);
    let record = root.join(".fab7/rf/evals").join(EVAL).join("record.json");
    let judged = std::fs::read(&record).unwrap();

    let socket = protocol::private_socket("weft-mark-dialog");
    let config = tmp.path().join("config.toml");
    let (listening, config_at) = (socket.clone(), config.clone());
    std::thread::spawn(move || {
        let _ = server::Session::serve_with(&listening, &config_at, None);
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

    // The split change: its dialog names it. Cancel keeps nothing.
    let (pending, what, why) = c.mark(SPLIT).expect("a split change's dialog");
    assert_eq!(what, "I asked for this");
    assert!(why.starts_with("src/jobs.js  L"), "{why}");
    c.answer(pending, false);
    assert!(labels(&config).is_empty(), "a Cancel keeps no label");

    // Mark appends one row.
    let (pending, _, _) = c.mark(SPLIT).unwrap();
    c.answer(pending, true);
    let rows = labels(&config);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (&rows[0]["schema"], &rows[0]["kind"], &rows[0]["harness"], &rows[0]["eval_id"]),
        (&json!("weft.label/1"), &json!("mark"), &json!("codex"), &json!(EVAL))
    );
    assert_eq!(
        (&rows[0]["window"], &rows[0]["requirement"], &rows[0]["mark"]),
        (&json!(SPLIT), &Value::Null, &json!("asked_for"))
    );

    // A requirement: "Not what I meant".
    let (pending, what, why) = c.mark(&format!("{ASK}/f0")).expect("a requirement's dialog");
    assert_eq!(what, "Not what I meant");
    assert!(why.starts_with("f0  The change must satisfy"), "{why}");
    c.answer(pending, true);
    let rows = labels(&config);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(
        (&rows[1]["window"], &rows[1]["requirement"], &rows[1]["mark"]),
        (&Value::Null, &json!(format!("{ASK}/f0")), &json!("not_meant"))
    );

    // A change the judges settled takes no mark.
    let refused = c.mark(REQUIRED);
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(std::fs::read(&record).unwrap(), judged, "the judges' record is as it was");
    c.call(Call::Shutdown);
}
