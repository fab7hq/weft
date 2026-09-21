//! Fixtures the ported tests share. The Python suite had these in `conftest`.

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
        "git", "-C", &root, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init",
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
        .and_then(|_| Ok(body(home.path())));
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

/// A YAML document from a JSON one, for tests that rewrite a catalog the way
/// the Python suite's `yaml.safe_dump` did.
///
/// Every string is quoted, which keeps the output clear of the 1.1/1.2
/// ambiguities the lint refuses.
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
