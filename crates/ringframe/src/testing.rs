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

/// A YAML document from a JSON one, for tests that rewrite a catalog with
/// every string quoted.
///
/// Quoting every string keeps the output clear of the 1.1/1.2 ambiguities the
/// lint refuses.
pub fn to_yaml(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_yaml(value, 0, &mut out);
    out
}

fn write_yaml(value: &serde_json::Value, indent: usize, out: &mut String) {
    use serde_json::Value;
    let pad = "  ".repeat(indent);
    match value {
        Value::Object(map) if map.is_empty() => out.push_str("{}\n"),
        Value::Object(map) => {
            for (k, v) in map {
                out.push_str(&format!("{pad}{}:", yaml_scalar(&Value::String(k.clone()))));
                write_child(v, indent, out);
            }
        }
        Value::Array(items) if items.is_empty() => out.push_str("[]\n"),
        Value::Array(items) => {
            for item in items {
                out.push_str(&format!("{pad}-"));
                write_child(item, indent, out);
            }
        }
        scalar => out.push_str(&format!("{}\n", yaml_scalar(scalar))),
    }
}

fn write_child(v: &serde_json::Value, indent: usize, out: &mut String) {
    use serde_json::Value;
    match v {
        Value::Object(m) if !m.is_empty() => {
            out.push('\n');
            write_yaml(v, indent + 1, out);
        }
        Value::Array(a) if !a.is_empty() => {
            out.push('\n');
            write_yaml(v, indent + 1, out);
        }
        other => {
            out.push(' ');
            write_yaml(other, 0, out);
        }
    }
}

fn yaml_scalar(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        other => serde_json::to_string(other).expect("a scalar"),
    }
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

/// A staging directory with `source.txt` and one prompt form.
pub fn stage_in(ws: &Workspace, name: &str, source: &[u8], prompt: &[u8]) -> std::path::PathBuf {
    let mut d = ws.rf_dir().join("tmp").join("stage-1");
    let mut n = 1;
    while d.exists() {
        n += 1;
        d = ws.rf_dir().join("tmp").join(format!("stage-{n}"));
    }
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("source.txt"), source).unwrap();
    std::fs::write(d.join(name), prompt).unwrap();
    d
}

/// Compile an Ask and confirm it in one step.
pub fn confirm_ask(ws: &Workspace, title: &str, source: &[u8], prompt: &[u8]) -> Value {
    let staged = stage_in(ws, "prompt.txt", source, prompt);
    let out = crate::ask::compile(
        ws,
        crate::ask::Compile {
            staged: &staged,
            title,
            capability: "native_plan",
            classification: json!({"task": ["plan"], "result": "plan",
                               "interaction": "approval_gated", "horizon": "session",
                               "effects": ["read"]}),
            route: json!({"fits": "bounded", "alternatives": [], "continuation": "plan review",
                      "effects": "reads", "gaps": []}),
            host: json!({"name": "claude-code", "version": "2.1.260", "surface": "native-tui",
                     "session_ref": "s1"}),
            links: Vec::new(),
            limitations: Vec::new(),
            actor: None,
        },
    )
    .unwrap();
    crate::ask::confirm(ws, out["ask_id"].as_str().unwrap(), None).unwrap();
    out
}

pub fn judge(angle: &str) -> Value {
    json!({"host": "claude-code", "model": "claude-sonnet-5", "angle": angle,
           "independence": "sub_agent"})
}

pub fn intent_doc(brief_sha: &str, items: Value) -> Value {
    json!({"schema": "ringframe.eval-intent/1", "brief_sha256": brief_sha,
           "judge": judge("intent"), "items": items})
}

/// The work commit of `two_asks_and_work`.
pub const CHANGED: [&str; 3] = ["docs/notes.md", "src/uptime.js", "tests/uptime.test.js"];

/// Every changed path classified (`required` unless the caller says
/// otherwise), as the CLI demands of every judge.
///
/// A judge that ran a command by default, because an uncited `yes` is not
/// decisive and these fixtures are about aggregation rather than about
/// citation. Pass an empty `commands_run` to make one that cites nothing.
pub fn judgement(brief_sha: &str, angle: &str, votes: &[(&str, &str)]) -> Value {
    judgement_over(brief_sha, angle, votes, &[], &CHANGED)
}

pub fn judgement_over(
    brief_sha: &str,
    angle: &str,
    votes: &[(&str, &str)],
    drift: &[(&str, &str)],
    paths: &[&str],
) -> Value {
    let mut full: std::collections::BTreeMap<&str, &str> =
        paths.iter().map(|p| (*p, "required")).collect();
    for (p, c) in drift {
        full.insert(p, c);
    }
    json!({
        "schema": "ringframe.eval-judgement/1", "brief_sha256": brief_sha,
        "judge": judge(angle),
        "votes": votes.iter().map(|(k, v)| json!({
            "item": k, "vote": v, "reason": format!("{angle} says {v}")
        })).collect::<Vec<_>>(),
        "drift": full.iter().map(|(p, c)| json!({
            "path": p, "finding": "seen", "classification": c
        })).collect::<Vec<_>>(),
        "commands_run": ["npm test"],
    })
}

/// Base commit → Ask A confirmed → Ask B confirmed → work commit touching
/// `src/`, `tests/` and `docs/`.
pub fn two_asks_and_work(ws: &Workspace) -> (String, String, String) {
    let a = confirm_ask(ws, "Add uptime endpoint", b"fix the login bug\n", b"Fix the login bug.\n");
    let b = confirm_ask(ws, "Skip the cache", b"skip the cache\n", b"Skip the cache.\n");
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
    pub brief_sha: String,
}

pub fn opened(ws: &Workspace) -> Opened {
    let (a, b, sha) = two_asks_and_work(ws);
    let out = crate::evaluate::open_eval(ws, crate::evaluate::Open::default()).unwrap();
    let brief_sha = out["brief"]["sha256"].as_str().unwrap().to_string();
    Opened { a, b, sha, out, brief_sha }
}

/// A config home, a repository, and a workspace: what every Eval test needs.
pub fn eval_bench<T>(body: impl FnOnce(&Workspace) -> T) -> T {
    with_config_home(|_| {
        let repo = repo();
        let ws = ws_for(repo.path());
        body(&ws)
    })
}
