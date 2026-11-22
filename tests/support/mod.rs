//! What the daemon tests share: an agent that behaves as `cat` and tells the
//! test which pane it runs in, and the hooks such an agent would signal.

#![allow(dead_code)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use weft::protocol::{Event, Line};
use weft_core::signal::Sent;

/// An agent's command line: it writes the pane variables weftd gave it to
/// `<dir>/pane-<id>`, one `NAME=value` a line, then behaves as `cat`. Run
/// through `/bin/sh`, because a script written a moment ago can still be
/// open in a process another test thread is forking, and executing it then
/// fails as busy.
pub fn agent(dir: &Path) -> String {
    std::fs::create_dir_all(dir).expect("agent dir");
    let path = dir.join("agent.sh");
    let script = "#!/bin/sh\n\
        d=$(dirname \"$0\")\n\
        printf 'WEFT_PANE=%s\\nWEFT_PANE_TOKEN=%s\\nWEFT_SOCKET=%s\\n' \
        \"$WEFT_PANE\" \"$WEFT_PANE_TOKEN\" \"$WEFT_SOCKET\" > \"$d/pane-$WEFT_PANE.tmp\"\n\
        mv \"$d/pane-$WEFT_PANE.tmp\" \"$d/pane-$WEFT_PANE\"\n\
        exec cat\n";
    std::fs::write(&path, script).expect("agent script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    format!("/bin/sh {}", path.display())
}

/// The variables the agent in `pane` was started with, once it has said.
pub fn pane_env(dir: &Path, pane: u32) -> Vec<(String, String)> {
    let path = dir.join(format!("pane-{pane}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(Instant::now() < deadline, "the agent in pane {pane} never said where it is");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::read_to_string(path)
        .expect("pane file")
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

pub fn token_of(dir: &Path, pane: u32) -> String {
    pane_env(dir, pane).into_iter().find(|(k, _)| k == "WEFT_PANE_TOKEN").expect("token").1
}

/// One `signal` line to the daemon, as `weft signal` sends it.
pub fn signal(socket: &Path, sent: Sent) {
    let mut s = UnixStream::connect(socket).expect("the daemon");
    s.write_all(&Line::Event(Event::Signal(Box::new(sent))).encode()).expect("sent");
}

/// What the agent in `pane` (started from [`agent`] in `dir`) signals:
/// `event`, of `session`, in `root`.
pub fn pane_says(
    socket: &Path,
    dir: &Path,
    root: &Path,
    harness: &str,
    pane: u32,
    session: &str,
    event: &str,
) {
    signal(
        socket,
        Sent {
            harness: harness.into(),
            hook: "Hook".into(),
            session: session.into(),
            workspace: Some(root.to_string_lossy().into_owned()),
            state: Some(event.into()),
            pane: Some(pane),
            token: Some(token_of(dir, pane)),
            ..Sent::default()
        },
    );
}

/// Where a test's agents tell it which pane they are in.
pub fn agents_dir(socket: &Path) -> PathBuf {
    socket.with_extension("agents")
}

/// A Git repository at `root` with one empty commit, and how to run Git in it.
pub fn git_repo(root: &Path) -> impl Fn(&[&str]) + '_ {
    std::fs::create_dir_all(root).unwrap();
    let git = move |args: &[&str]| {
        let ok = std::process::Command::new("/usr/bin/git")
            .arg("-C")
            .arg(root)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .status()
            .unwrap();
        assert!(ok.success(), "git {args:?}");
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    git
}

/// A real RingFrame record at `root`, as the daemon finds one: a Git
/// repository, RingFrame's test configuration in `home`, and an Ask for the
/// harness `host`, compiled and confirmed, followed by a commit of work.
/// `HOME` must already be `home`. Returns the Ask's id.
pub fn real_ask(root: &Path, home: &Path, host: &str) -> String {
    real_ask_of(root, home, host, "Ship it.", &[("work.txt", "the work\n".into())])
}

/// The same, for the words `source` and a commit of these files.
pub fn real_ask_of(
    root: &Path,
    home: &Path,
    host: &str,
    source: &str,
    files: &[(&str, String)],
) -> String {
    use ringframe::api;
    let git = git_repo(root);
    assert_eq!(std::env::var_os("HOME").as_deref(), Some(home.as_os_str()));
    let config =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/ringframe/tests/fixtures/config");
    ringframe::workspace::install_config(Some(&config)).unwrap();
    let ws = api::workspace(root).unwrap();
    ws.ensure().unwrap();
    let overrides = ringframe::config::Overrides::default();
    let compiled = api::compile(
        &ws,
        api::Compile {
            source,
            harness: api::Harness { name: host, version: None, session_ref: None },
            capability: "human_handoff",
            classification: api::Classification {
                task: vec!["plan".into()],
                result: "plan".into(),
                effects: vec!["read".into()],
            },
            plan: None,
            practices: Vec::new(),
            offered: Vec::new(),
            classified_by: api::Chooser::Person,
            selected_by: api::Chooser::Person,
            links: Vec::new(),
            overrides: &overrides,
            via: "weft",
        },
    )
    .unwrap();
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "work"]);
    compiled.ask_id
}

/// A harness `name` whose sessions are a script in `dir`: it writes the pane
/// variables weftd gave it, its process id and its arguments to
/// `<dir>/pane-<id>`, then behaves as `cat`, keeping what it is typed in
/// `<dir>/typed-<id>`. A judge of it starts with
/// `--model` and `--effort`, as its `[eval.launch]` says.
pub fn stub_harness(dir: &Path, name: &str) -> weft_core::harness::Harnesses {
    let file = stub_file(dir, "judge.sh", "Stub");
    let (read, unread) = weft_core::harness::Harnesses::read(vec![(name.to_string(), file)]);
    assert!(unread.is_empty(), "the stub harness file reads");
    read
}

/// A stub harness, as [`stub_harness`], whose hooks say the prompt its
/// agent received: an Ask can be sent into it.
pub fn stub_harness_proving(dir: &Path, name: &str) -> weft_core::harness::Harnesses {
    let file = stub_file(dir, "judge.sh", "Stub")
        + "[signal]\nsession = \"session_id\"\nworkspace = \"cwd\"\nprompt = [\"said.text\"]\non = []\n";
    let (read, unread) = weft_core::harness::Harnesses::read(vec![(name.to_string(), file)]);
    assert!(unread.is_empty(), "the stub harness file reads: {unread:?}");
    read
}

/// Stub harnesses, as [`stub_harness`], each titled with its name and run
/// from its own script, `<dir>/judge-<name>.sh`.
pub fn stub_harnesses(dir: &Path, names: &[&str]) -> weft_core::harness::Harnesses {
    let files = names
        .iter()
        .map(|n| (n.to_string(), stub_file(dir, &format!("judge-{n}.sh"), n)))
        .collect();
    let (read, unread) = weft_core::harness::Harnesses::read(files);
    assert!(unread.is_empty(), "the stub harness files read");
    read
}

/// A stub harness's script, written to `<dir>/<script>`, and its harness file.
fn stub_file(dir: &Path, script: &str, title: &str) -> String {
    std::fs::create_dir_all(dir).expect("agent dir");
    let path = dir.join(script);
    let script = "#!/bin/sh\n\
        d=$(dirname \"$0\")\n\
        printf 'WEFT_PANE=%s\\nWEFT_PANE_TOKEN=%s\\nWEFT_SOCKET=%s\\nPID=%s\\nARGS=%s\\n' \
        \"$WEFT_PANE\" \"$WEFT_PANE_TOKEN\" \"$WEFT_SOCKET\" \"$$\" \"$*\" > \"$d/pane-$WEFT_PANE.tmp\"\n\
        mv \"$d/pane-$WEFT_PANE.tmp\" \"$d/pane-$WEFT_PANE\"\n\
        exec tee \"$d/typed-$WEFT_PANE\"\n";
    std::fs::write(&path, script).expect("judge script");
    format!(
        "title = \"{title}\"\nprogram = \"/bin/sh\"\nargs = [\"{}\"]\nresume = [\"--resume\"]\n\
         [config]\ndefault = \".stub\"\n[plugin]\nlist = [\"list\"]\n\
         [eval]\nefforts = [\"low\", \"high\"]\neffort = \"model\"\n\
         default = {{ read = \"low\", question = \"low\" }}\n\
         [eval.launch]\nmodel = [\"--model\", \"{{model}}\"]\neffort = [\"--effort\", \"{{effort}}\"]\n",
        path.display()
    )
}

/// The task lines typed into a judge, in order: each task, and where its
/// answer goes.
pub fn task_lines(typed: &str) -> Vec<(String, PathBuf, PathBuf)> {
    typed
        .match_indices("Eval task ")
        .filter_map(|(i, _)| {
            let rest = &typed[i + "Eval task ".len()..];
            let (task, rest) = rest.split_once(": read ")?;
            let (brief, rest) = rest.split_once("; answer with ringframe ")?;
            let (_, rest) = rest.split_once("eval submit --task ")?;
            let (_, rest) = rest.split_once(" --file ")?;
            let end = rest.find(".json")? + ".json".len();
            Some((task.to_string(), brief.into(), rest[..end].into()))
        })
        .collect()
}

/// A question reader's answer, from its brief: each item in a question of
/// its own, of the first kind it allows; a `what_did_you_mean` quotes the
/// start of the first Ask.
pub fn questions_for(brief: &str) -> Value {
    let words: String = brief
        .split("## The requests\n\n=== ")
        .nth(1)
        .and_then(|r| r.lines().nth(1))
        .unwrap_or_default()
        .chars()
        .take(20)
        .collect();
    let questions: Vec<Value> = brief
        .lines()
        .filter_map(|l| l.strip_prefix("### `"))
        .filter_map(|l| {
            let (key, rest) = l.split_once("` (allows ")?;
            let kind = rest.trim_end_matches(')').split(", ").next()?.to_string();
            Some((key.to_string(), kind))
        })
        .enumerate()
        .map(|(n, (key, kind))| {
            let mut q = json!({"ask": kind, "behaviour": format!("Behaviour number {n} changed."),
                               "why": format!("Reason number {n} for asking."), "items": [key]});
            if kind == "what_did_you_mean" {
                q["quote"] = json!(words);
                q["readings"] = json!(["one way", "another way"]);
            }
            q
        })
        .collect();
    json!({"questions": questions})
}

/// What a task asks for, by its id: the person's requirements, or a reading
/// of the change.
pub fn kind_of(task: &str) -> &'static str {
    match task.rsplit('~').next().unwrap_or_default() {
        l if l.starts_with('q') => "requirements",
        _ => "read",
    }
}

/// Whether a task reads again what an earlier reading left open.
pub fn is_reread(task: &str) -> bool {
    task.rsplit('~').next().is_some_and(|l| l.starts_with("rr"))
}

/// A reader's answer to its task file: the requirements quoting the Ask's
/// words; every unit serving the first requirement; every requirement
/// unknown, with what would settle it, so each is read once more.
pub fn answer(kind: &str, file: &Value) -> Value {
    let list = |key: &str| file[key].as_array().cloned().unwrap_or_default();
    if file["kind"] == "question" {
        let out = file["output"].as_str().unwrap_or_default();
        let brief = out.replace("/out/", "/tasks/").replace(".json", ".brief.md");
        return questions_for(&std::fs::read_to_string(brief).unwrap_or_default());
    }
    if kind == "requirements" {
        return json!({"requirements": list("asks").iter().map(|a| {
            json!({"ask": a["id"], "kind": "local", "text": "the uptime export",
                   "quote": a["text"].as_str().unwrap().trim()})
        }).collect::<Vec<_>>()});
    }
    let items: Vec<String> =
        list("items").iter().filter_map(|i| i.as_str().map(str::to_string)).collect();
    let of = |q: &str| -> Vec<String> {
        items.iter().filter_map(|i| i.strip_prefix(q).map(str::to_string)).collect()
    };
    let reqs = of("r|");
    json!({
        "windows": of("w|").iter().map(|w| json!({"id": w, "answer": "serves", "serves": [reqs[0]]}))
            .collect::<Vec<_>>(),
        "requirements": reqs.iter().map(|r| json!({"id": r, "vote": "unknown",
            "reason": format!("{r} is not shown either way"),
            "missing": format!("a test showing {r}")})).collect::<Vec<_>>(),
    })
}
