//! Fixtures the tests share.

use std::path::Path;
use std::process::Command;

pub use tempfile::TempDir;

pub fn tmp_dir() -> TempDir {
    tempfile::tempdir().expect("a temporary directory")
}

pub fn run(argv: &[&str]) {
    let out = Command::new(argv[0]).args(&argv[1..]).output().expect(argv[0]);
    assert!(out.status.success(), "{argv:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// A fresh Git worktree root to act as a consumer workspace.
pub fn repo() -> TempDir {
    let dir = tmp_dir();
    let root = dir.path().to_string_lossy().to_string();
    run(&["git", "init", "-q", &root]);
    std::fs::write(dir.path().join("README.md"), "fixture\n").unwrap();
    run(&["git", "-C", &root, "add", "-A"]);
    run(&[
        "git",
        "-C",
        &root,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "init",
    ]);
    dir
}

/// The workspace for a fixture repository, with `.fab7/rf/` already made.
pub fn ws_for(path: &Path) -> crate::workspace::Workspace {
    let ws = crate::workspace::resolve(Some(path), None).expect("resolve");
    ws.ensure().expect("ensure");
    ws
}

/// `HOME` is one value per process, and Rust runs tests in parallel threads.
/// Everything that reads the config home takes this lock, so the tests that
/// need their own configuration cannot see each other's.
static CONFIG_HOME: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn fixture_config() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config")
}

/// A real config home, installed from the fixture bundle. The package ships no
/// configuration; this stands in for a synced one.
pub fn with_config_home<T>(body: impl FnOnce(&Path) -> T) -> T {
    let guard = CONFIG_HOME.lock().unwrap_or_else(|e| e.into_inner());
    let home = tmp_dir();
    let was = std::env::var_os("HOME");
    // SAFETY: every reader of HOME in these tests holds the lock above.
    unsafe { std::env::set_var("HOME", home.path()) };
    let installed = crate::workspace::install_config(Some(&fixture_config()));
    let out = installed
        .map_err(|e| format!("installing the fixture configuration: {e}"))
        .map(|_| body(home.path()));
    // SAFETY: as above.
    unsafe {
        match was {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
    drop(guard);
    out.unwrap()
}

// ---- Eval and Seal fixtures -------------------------------------------------
//
// Shared here so both modules' tests can reach them.

use serde_json::{Value, json};

use crate::workspace::Workspace;

pub fn head(root: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(root).args(["rev-parse", "HEAD"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Write files (a `None` body deletes) and commit them.
pub fn commit(root: &Path, files: &[(&str, Option<&str>)], message: &str) -> String {
    for (name, body) in files {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        match body {
            None => std::fs::remove_file(&p).unwrap(),
            Some(text) => std::fs::write(&p, text).unwrap(),
        }
    }
    let root = root.to_string_lossy().to_string();
    run(&["git", "-C", &root, "add", "-A"]);
    run(&[
        "git",
        "-C",
        &root,
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        message,
    ]);
    head(Path::new(&root))
}

/// An Ask of these words, compiled and confirmed as Weft sends one: the
/// prompt is the words verbatim, then the route's rules.
pub fn confirm_ask(ws: &Workspace, words: &[u8]) -> Value {
    confirm_ask_as(ws, words, "native_plan")
}

/// As `confirm_ask`, linked: `follows`, `revises` or `remediates` another.
pub fn confirm_ask_linked(ws: &Workspace, links: Vec<Value>) -> Value {
    confirm_ask_with(ws, b"More.\n", "native_plan", links)
}

/// As `confirm_ask`, routed to `capability`: `native_goal` for work an Eval
/// judges (a planning Ask gives it no requirement).
pub fn confirm_ask_as(ws: &Workspace, words: &[u8], capability: &str) -> Value {
    confirm_ask_with(ws, words, capability, Vec::new())
}

fn confirm_ask_with(ws: &Workspace, words: &[u8], capability: &str, links: Vec<Value>) -> Value {
    // Work, not a plan; and an objective rather than a change, since a goal
    // for a change works a plan, which these workspaces do not hold.
    let classification = match capability {
        "native_plan" => json!({"task": ["plan"], "result": "plan", "effects": ["read"]}),
        "native_goal" => json!({"task": ["implement"], "result": "continuing_objective",
                                "effects": ["workspace_write"]}),
        _ => {
            json!({"task": ["implement"], "result": "workspace_change", "effects": ["workspace_write"]})
        }
    };
    crate::compile::compile(
        ws,
        crate::compile::Request {
            source: words,
            host: json!({"name": "claude-code", "version": "2.1.260", "session_ref": "s1"}),
            capability,
            classification,
            plan: None,
            practices: Vec::new(),
            classified_by: json!({"by": "person"}),
            selected_by: json!({"by": "person"}),
            links,
            overrides: &crate::config::Overrides::default(),
            via: "weft",
        },
    )
    .unwrap()
}

/// Base commit → Ask A confirmed → Ask B confirmed → work commit touching
/// `src/`, `tests/` and `docs/`.
pub fn two_asks_and_work(ws: &Workspace) -> (String, String, String) {
    let a = confirm_ask(ws, b"Fix the login bug.\n");
    let b = confirm_ask(ws, b"Skip the cache.\n");
    let sha = commit(
        &ws.root,
        &[
            ("src/uptime.js", Some("export const uptime = () => 1;\n")),
            ("tests/uptime.test.js", Some("test\n")),
            ("docs/notes.md", Some("unrelated\n")),
        ],
        "work",
    );
    (a["ask_id"].as_str().unwrap().to_string(), b["ask_id"].as_str().unwrap().to_string(), sha)
}

pub struct Opened {
    pub a: String,
    pub b: String,
    /// The work commit. Not every test reads it; the ones about anchors do.
    #[allow(dead_code)]
    pub sha: String,
    pub out: Value,
}

/// `opened`, with one Ask that asks for something the change does: its
/// requirement can be met.
pub fn opened_with_requirement(ws: &Workspace) -> Opened {
    let a =
        confirm_ask_as(ws, b"Add the uptime export to src/uptime.js with a test.\n", "native_goal");
    let b = confirm_ask(ws, b"Skip the cache.\n");
    let sha = commit(
        &ws.root,
        &[
            ("src/uptime.js", Some("export const uptime = () => 1;\n")),
            ("tests/uptime.test.js", Some("test('uptime', () => uptime());\n")),
            ("docs/notes.md", Some("The server reports its uptime.\n")),
        ],
        "work",
    );
    let out = crate::evaluate::open_eval(ws, crate::evaluate::Open::default()).unwrap();
    let id = |v: &Value| v["ask_id"].as_str().unwrap().to_string();
    Opened { a: id(&a), b: id(&b), sha, out }
}

pub fn opened(ws: &Workspace) -> Opened {
    let (a, b, sha) = two_asks_and_work(ws);
    let out = crate::evaluate::open_eval(ws, crate::evaluate::Open::default()).unwrap();
    Opened { a, b, sha, out }
}

/// A config home, a repository, and a workspace: what every Eval test needs.
pub fn eval_bench<T>(body: impl FnOnce(&Workspace) -> T) -> T {
    with_config_home(|_| {
        let repo = repo();
        let ws = ws_for(repo.path());
        body(&ws)
    })
}

/// An Eval's tasks answered one at a time until it settles: `fine`, by
/// `answer_task`; otherwise every window is unexplained and every requirement
/// not met. Returns the record.
pub fn settled(ws: &Workspace, eval_id: &str, fine: bool) -> Value {
    for _ in 0..40 {
        let got = crate::tasks::take(ws, eval_id, None).unwrap();
        if got["state"] == "done" {
            return crate::evaluate::load_record(ws, eval_id).unwrap().unwrap();
        }
        let task = got["task"]["task_id"].as_str().expect("a task").to_string();
        if fine {
            answer_task(ws, &task);
        } else {
            answer_task_wrong(ws, &task);
        }
        let (code, out) = crate::tasks::submit(ws, &task).unwrap();
        assert_eq!(code, 0, "{out}");
    }
    panic!("the Eval did not settle");
}

/// A judge that finds nothing done: every window unexplained, quoting its
/// first added line, and every requirement not met.
pub fn answer_task_wrong(ws: &Workspace, task_id: &str) {
    let eval_id = task_id.split_once('~').expect("a task id").0;
    let dir = ws.rf_dir().join("tmp").join(format!("eval-{eval_id}"));
    let read = |p: &Path| std::fs::read_to_string(p).unwrap();
    let file: Value =
        serde_json::from_str(&read(&dir.join(format!("tasks/{task_id}.json")))).unwrap();
    let windows = read(Path::new(file["windows_file"].as_str().unwrap()));
    let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
    let doc = if file["kind"] == "requirements" {
        listed_requirements(&file)
    } else if file["kind"] == "map" {
        let listed = list("windows");
        let ids: Vec<&str> = listed.iter().map(|w| w["id"].as_str().unwrap()).collect();
        let quotable = ids.iter().find(|w| quote_in(&windows, w).is_some()).copied();
        json!({"windows": ids.iter().map(|id| match quote_in(&windows, id) {
            Some(quote) => json!({"window": id, "unexplained": format!("no request asks for {id}"), "quote": quote}),
            // Nothing to quote: it goes with a window that has.
            None => json!({"window": id, "follows_from": quotable.unwrap_or(id)}),
        }).collect::<Vec<_>>()})
    } else {
        json!({"requirements": list("requirements").iter().map(|r| {
            json!({"id": r["id"], "vote": "not_met", "reason": format!("{} is not done", r["id"]),
                   "missing": "a change that does it"})
        }).collect::<Vec<_>>()})
    };
    std::fs::write(file["output"].as_str().unwrap(), doc.to_string()).unwrap();
}

/// A judge that finds everything done: every window serves every
/// requirement, and every requirement is met, citing its first window.
/// Written where RingFrame said the answer goes.
pub fn answer_task(ws: &Workspace, task_id: &str) {
    let eval_id = task_id.split_once('~').expect("a task id").0;
    let dir = ws.rf_dir().join("tmp").join(format!("eval-{eval_id}"));
    let read = |p: &Path| std::fs::read_to_string(p).unwrap();
    let file: Value =
        serde_json::from_str(&read(&dir.join(format!("tasks/{task_id}.json")))).unwrap();
    let windows = read(Path::new(file["windows_file"].as_str().unwrap()));
    let text = |key: &str| file[key].as_str().unwrap_or_default().to_string();
    let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
    let ids = |key: &str, field: &str| -> Vec<String> {
        list(key).iter().map(|v| v[field].as_str().unwrap().to_string()).collect()
    };
    let requirements = ids("requirements", "id");
    let mut doc = json!({});
    if text("kind") == "requirements" {
        doc = listed_requirements(&file);
    } else if text("kind") == "map" {
        doc["windows"] = json!(
            ids("windows", "id")
                .iter()
                .map(|w| json!({"window": w, "serves": requirements}))
                .collect::<Vec<_>>()
        );
    } else {
        doc["requirements"] = json!(
            list("requirements")
                .iter()
                .map(|r| {
                    let id = r["id"].as_str().unwrap();
                    // A rule of the prompt that no change touches.
                    let cited = r["windows"].as_array().into_iter().flatten().find_map(|w| {
                        let w = w.as_str()?;
                        quote_in(&windows, w).map(|q| (w.to_string(), q))
                    });
                    let Some((w, quote)) = cited else {
                        return json!({"id": id, "vote": "unknown",
                                      "reason": "no window bears on it",
                                      "missing": "a change that shows it"});
                    };
                    json!({"id": id, "vote": "met",
                           "reason": format!("{id} is done ({})", text("role")),
                           "citations": [{"window": w, "quote": quote}]})
                })
                .collect::<Vec<_>>()
        );
        if !list("found").is_empty() {
            doc["findings"] = json!([{"finding": "as listed", "means": "no change to the vote"}]);
        }
    }
    std::fs::write(text("output"), doc.to_string()).unwrap();
}

/// The start of the first added line of window `w` in a task's windows text.
/// A requirements task's answer: one requirement for each Ask it lists,
/// quoting the person's words.
fn listed_requirements(file: &Value) -> Value {
    let asks = file["asks"].as_array().cloned().unwrap_or_default();
    json!({"requirements": asks.iter().map(|a| {
        let words: String = a["text"].as_str().unwrap_or_default().trim().chars().take(300).collect();
        json!({"ask": a["id"], "kind": "local", "text": format!("what {} asked for", a["id"]),
               "quote": words})
    }).collect::<Vec<_>>()})
}

/// The first added line of a window long enough to quote.
fn quote_in(windows: &str, w: &str) -> Option<String> {
    let head = format!("=== {w} ");
    windows
        .lines()
        .skip_while(|l| !l.starts_with(&head))
        .skip(1)
        .take_while(|l| !l.starts_with("=== "))
        .find(|l| l.starts_with('+') && l.len() > 8)
        .map(|l| l[1..].chars().take(24).collect())
}
