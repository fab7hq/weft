//! remote.md §5, §6: one socket client per project the daemon has open, and
//! one `agents` list across them with ids that stay the same.

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use weft_proto::{Call, Line, Lines, PROTOCOL};
use weft_remote::agents::{self, Project};
use weft_remote::daemon::{self, Heard};

fn daemon(dir: &Path) -> PathBuf {
    let socket = weft_proto::private_socket("weft-remote-agents");
    let (listening, config) = (socket.clone(), dir.join("config.toml"));
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve(&listening, &config);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !weft_proto::is_live(&socket) {
        assert!(Instant::now() < deadline, "no daemon");
        std::thread::sleep(Duration::from_millis(20));
    }
    socket
}

/// A TUI that opens `project` and starts a shell in it.
fn start_agent(socket: &Path, project: &Path) -> std::os::unix::net::UnixStream {
    use std::io::Write;
    let mut s = std::os::unix::net::UnixStream::connect(socket).unwrap();
    let mut lines = Lines::new(s.try_clone().unwrap());
    let calls = [
        Call::Hello { client: "weft-tui/test".into(), protocol: PROTOCOL },
        Call::Open { path: project.to_string_lossy().into_owned(), rows: 24, cols: 80, raw: true },
        Call::StartAgent { harness: "sh".into(), spec: "/bin/sh".into(), session: None },
    ];
    for (id, call) in calls.into_iter().enumerate() {
        s.write_all(&Line::Call { id: id as u64, call }.encode()).unwrap();
        while !matches!(lines.next().unwrap(), Some(Line::Result { id: got, .. }) if got == id as u64)
        {
        }
    }
    s
}

#[test]
fn two_projects_make_one_list_with_ids_that_stay_the_same() {
    let dir = std::env::temp_dir().join(format!("weft-remote-agents-{}", std::process::id()));
    let (a, b) = (dir.join("alpha"), dir.join("beta"));
    for p in [&a, &b] {
        std::fs::create_dir_all(p).unwrap();
    }
    let socket = daemon(&dir);
    let _tui_a = start_agent(&socket, &a);
    let _tui_b = start_agent(&socket, &b);

    let listed = |socket: &Path| {
        let (tx, rx) = channel::<Heard>();
        let open = daemon::projects(&dir.join("projects.json"));
        assert_eq!(open.len(), 2, "the daemon's projects.json: {open:?}");
        let mut projects = Vec::new();
        for (at, path) in open.iter().enumerate() {
            let p = daemon::Project::open(socket, path, at, tx.clone()).unwrap();
            assert_eq!(p.opened["replay"], serde_json::json!([]), "opened with no raw output");
            projects.push(Project::opened(path, &p.opened));
        }
        // Anything that arrives meanwhile is taken, as the loop would.
        while let Ok(Heard::Event(at, e)) = rx.recv_timeout(Duration::from_millis(300)) {
            projects[at].heard(&e);
        }
        agents::message(&projects)
    };
    let first = listed(&socket);
    assert_eq!(first["type"], "agents");
    let rows = first["agents"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{first}");
    let mut names: Vec<_> = rows.iter().map(|r| r["project"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["alpha", "beta"]);
    for r in rows {
        let id = r["agent"].as_str().unwrap();
        let (key, pane) = id.split_once('-').unwrap();
        assert_eq!(key.len(), 12, "{id}");
        assert_eq!(pane, "0");
        assert_eq!(r["harness"], "sh");
        assert_eq!(r["turn"], "ready", "running, and no turn heard yet");
    }
    let ids = |v: &serde_json::Value| {
        let mut ids: Vec<String> = v["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["agent"].as_str().unwrap().into())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(&listed(&socket)), ids(&first), "stable");
    assert_ne!(ids(&first)[0], ids(&first)[1], "two projects, two keys");
    std::fs::remove_dir_all(&dir).ok();
}
