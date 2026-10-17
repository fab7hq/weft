//! The desktop's link against a real daemon: opening a project, starting an
//! agent and seeing what it prints, and reading through the daemon. Nothing
//! here waits but connecting.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use weft_core::harness::{Harness, Harnesses};
use weft_desktop::daemon::{Again, Answered, ProjectClient, Then};

/// A harness of this name that runs `command`, from a fixture profile.
fn harness(name: &str, command: &str) -> Harness {
    let mut p = weft_core::harness::fixture::file("codex");
    let mut words = command.split_whitespace();
    p["program"] = words.next().unwrap_or_default().into();
    p["args"] = words.collect::<Vec<_>>().into();
    Harness::of(name, &p).expect("a harness")
}

/// A daemon on its own socket, serving a project with one harness that runs `cat`.
fn daemon(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("weft-desktop-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("root");
    let socket = weft_proto::private_socket(&format!("weft-desktop-{name}"));
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let config = listening.with_extension("no-config.toml");
        let harnesses = Harnesses(vec![harness("cat", "cat")]);
        let _ = weftd::server::Session::serve_with(&listening, &config, Some(harnesses));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !weft_proto::is_live(&socket) {
        assert!(Instant::now() < deadline, "the daemon never listened");
        std::thread::sleep(Duration::from_millis(10));
    }
    (socket, root)
}

/// Pump until `done` says yes of what came; what was answered on the way.
fn until(
    c: &mut ProjectClient,
    done: impl Fn(&ProjectClient, &[Answered]) -> bool,
) -> Vec<Answered> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut answered = Vec::new();
    loop {
        answered.extend(c.pump().answered);
        if done(c, &answered) {
            return answered;
        }
        assert!(Instant::now() < deadline, "nothing came: {answered:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn shutdown(socket: &Path, root: &Path) {
    let _ = weft_proto::stop(socket);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_agent_started_through_the_link_prints_into_its_pane() {
    let (socket, root) = daemon("start");
    let mut c = ProjectClient::connect(&socket, &root, 24, 80).expect("connected");
    assert!(c.is_connected());
    assert_eq!(c.harnesses.find("cat").map(|h| h.program.as_str()), Some("cat"));

    let spec = c.harnesses.find("cat").unwrap().spec();
    c.spawn("cat", &spec, None).expect("asked");
    let answered = until(&mut c, |_, a| !a.is_empty());
    let Answered { then, answer } = &answered[0];
    assert_eq!(*then, Then::Started { session: None });
    let pane = answer.as_ref().expect("started")["pane"].as_u64().expect("its pane") as u32;
    until(&mut c, |c, _| c.view(pane).is_some());

    c.input(pane, b"hello from the desktop\r").expect("typed");
    until(&mut c, |c, _| {
        c.view(pane).is_some_and(|v| v.contents().contains("hello from the desktop"))
    });
    let shown: String =
        c.view(pane).unwrap().screen()[0].iter().map(|cell| cell.text.as_str()).collect();
    assert!(shown.starts_with("hello from the desktop"), "{shown:?}");
    assert!(!c.owes_any(|_| true), "every answer taken");
    shutdown(&socket, &root);
}

#[test]
fn a_read_is_answered_into_the_project_and_a_refusal_is_handed_back() {
    let (socket, root) = daemon("read");
    let mut c = ProjectClient::connect(&socket, &root, 24, 80).expect("connected");
    // Nothing on record: the daemon refuses, and nothing is kept as read.
    c.read("judges", "evl_1", None, Then::Judges { eval: "evl_1".into() }).expect("asked");
    until(&mut c, |c, _| !c.owes_any(|_| true));
    assert!(c.read.is_empty());
    c.act("eval", None, None, None, None, None).expect("asked");
    let answered = until(&mut c, |_, a| !a.is_empty());
    assert_eq!(answered[0].then, Then::Refusal);
    assert!(answered[0].answer.is_err(), "no agent, no Eval: {answered:?}");
    shutdown(&socket, &root);
}

/// A daemon of another protocol at a socket of its own: it refuses every hello.
fn other_version(name: &str) -> PathBuf {
    use std::io::{BufRead, BufReader, Write};
    let socket = weft_proto::private_socket(name);
    let _ = std::fs::remove_file(&socket);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else { break };
                let m: serde_json::Value = serde_json::from_str(&line).unwrap();
                let no = serde_json::json!({"id": m["id"], "error": {"code": "protocol",
                    "message": "this daemon speaks protocol 2, not 3"}});
                writeln!(writer, "{no}").unwrap();
            }
        }
    });
    socket
}

/// A window that finds another version of Weft running says so, and how to
/// end it, rather than a bare protocol number.
#[test]
fn a_window_refused_by_another_version_says_how_to_end_it() {
    let socket = other_version("weft-desktop-other-version");
    let refused =
        ProjectClient::connect(&socket, Path::new("."), 24, 80).err().expect("refused").to_string();
    assert_eq!(
        refused,
        "Another version of Weft is running (this daemon speaks protocol 2, not 3). \
         `weft stop` ends it and its agents."
    );
}

/// A project whose daemon went is opened again only on one that listens: a
/// window never starts the daemon the person stopped. One of another
/// protocol is a newer Weft, which the window starts again as.
#[test]
fn a_lost_project_comes_back_only_to_a_daemon_that_listens() {
    let gone = weft_proto::private_socket("weft-desktop-gone");
    assert!(matches!(ProjectClient::again(&gone, Path::new("."), 24, 80), Again::Waiting));
    assert!(!gone.exists(), "no daemon was started");

    let newer = other_version("weft-desktop-newer");
    assert!(matches!(ProjectClient::again(&newer, Path::new("."), 24, 80), Again::Newer(_)));

    let (socket, root) = daemon("again");
    match ProjectClient::again(&socket, &root, 24, 80) {
        Again::Back(c) => assert!(c.is_connected()),
        _ => panic!("the daemon listens"),
    }
    shutdown(&socket, &root);
}
