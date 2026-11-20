//! An Eval's dialog says which files in its work's range an Eval should
//! skip, each with its line. A yes writes that line, and the same dialog is
//! said again, under its id: the finding added, and the files it reads
//! fewer.
//!
//! Its own test binary because it sets `HOME`, which is global: one test, one
//! process.

mod support;

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

/// The next event `pick` takes, or the answer to call `id`.
fn wait<T>(
    frames: &mut Lines<UnixStream>,
    stream: &UnixStream,
    mut pick: impl FnMut(&Line) -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        stream.set_read_timeout(Some(Duration::from_millis(100))).expect("timeout");
        if let Ok(Some(line)) = frames.next()
            && let Some(found) = pick(&line)
        {
            return found;
        }
    }
    panic!("nothing arrived");
}

/// An Eval dialog's reads, from its payload.
fn reads_of(payload: &[u8]) -> serde_json::Value {
    serde_json::from_slice::<serde_json::Value>(payload).expect("a payload")["reads"].clone()
}

#[test]
fn an_eval_dialog_says_what_to_skip_and_a_line_written_says_it_again() {
    let tmp = std::env::temp_dir().join(format!("weft-skip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (root, home) = (tmp.join("project"), tmp.join("home"));
    // SAFETY: this binary holds exactly one test.
    unsafe {
        std::env::set_var("HOME", &home);
    }
    let ask = support::real_ask(&root, &home, "sh");
    // Output the work made and nobody committed, over what an Eval reads whole.
    std::fs::create_dir_all(root.join("runs/a")).unwrap();
    let text = "a line of output, written by a run\n".repeat(1200);
    for i in 0..30 {
        std::fs::write(root.join(format!("runs/a/{i}.log")), &text).unwrap();
    }

    let socket = protocol::private_socket("weft-skip");
    let agent = support::agent(&support::agents_dir(&socket));
    let harnesses = support::stub_harness(&socket.with_extension("judges"), "sh");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let config = listening.with_extension("no-config.toml");
        let _ = server::Session::serve_with(&listening, &config, Some(harnesses));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        if let Ok(s) = UnixStream::connect(&socket) {
            break s;
        }
        assert!(Instant::now() < deadline, "no daemon");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut frames = Lines::new(stream.try_clone().expect("clone"));
    let mut id = 0;
    let mut call = |stream: &mut UnixStream, c: Call| {
        id += 1;
        stream.write_all(&Line::Call { id, call: c }.encode()).expect("write");
        id
    };
    call(&mut stream, Call::Hello { client: "test".into(), protocol: PROTOCOL });
    let path = root.to_string_lossy().into_owned();
    let open = call(&mut stream, Call::Open { rows: 24, cols: 80, path, raw: true });
    wait(&mut frames, &stream, |l| {
        matches!(l, Line::Result { id, .. } if *id == open).then_some(())
    });
    call(&mut stream, Call::StartAgent { harness: "sh".into(), spec: agent, session: None });
    wait(&mut frames, &stream, |l| matches!(l, Line::Event(Event::Added { .. })).then_some(()));
    support::pane_says(&socket, &support::agents_dir(&socket), &root, "sh", 0, "s1", "ready");
    let check = Call::Act {
        act: "check".into(),
        unit: Some(ask),
        pane: None,
        text: None,
        eval: None,
        start: None,
        draft: None,
    };
    call(&mut stream, check);
    let (pnd, payload) = wait(&mut frames, &stream, |l| match l {
        Line::Event(Event::Pending { id, payload, .. }) => Some((id.clone(), payload.clone())),
        _ => None,
    });
    let reads = reads_of(&payload);
    let found = reads["findings"].as_array().expect("findings").clone();
    assert_eq!(found.len(), 1, "{reads}");
    assert_eq!((&found[0]["kind"], &found[0]["path"]), (&"untracked_bulk".into(), &"runs/".into()));
    let fix = &found[0]["fix"];
    assert_eq!((&fix["file"], &fix["line"]), (&".gitignore".into(), &"/runs/".into()));
    assert_eq!(reads["files_before"], serde_json::Value::Null, "{reads}");
    let files = reads["files"].as_u64().expect("files");

    let add = Call::SkipAdd { file: ".gitignore".into(), line: "/runs/".into() };
    call(&mut stream, add);
    let (again, payload) = wait(&mut frames, &stream, |l| match l {
        Line::Event(Event::Pending { id, payload, .. }) => Some((id.clone(), payload.clone())),
        _ => None,
    });
    assert_eq!(again, pnd, "the same dialog, said again");
    let reads = reads_of(&payload);
    assert_eq!(reads["findings"][0]["added"], true, "{reads}");
    assert_eq!(reads["findings"].as_array().map(Vec::len), Some(1), "{reads}");
    assert_eq!(reads["files_before"].as_u64(), Some(files), "{reads}");
    // The thirty no longer read; the new `.gitignore` is one more.
    assert_eq!(reads["files"].as_u64(), Some(files - 30 + 1), "{reads}");
    let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "/runs/"), "{ignore}");
    let _ = UnixStream::connect(&socket)
        .map(|mut s| protocol::send(&mut s, &Line::Call { id: 1, call: Call::Shutdown }.encode()));
    let _ = std::fs::remove_dir_all(&tmp);
}
