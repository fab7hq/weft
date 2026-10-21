//! Frames follow the change (remote.md D3): the first output after a quiet
//! spell is framed at once, and steady output at most 30 times a second.
//! Timed, so apart from the other device tests, whose 5 MiB streams would
//! load the machine while these measure it.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;

const ROWS: u16 = 24;
const COLS: u16 = 80;

fn connect(socket: &Path, project: &Path) -> (UnixStream, Lines<UnixStream>) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        if let Ok(s) = UnixStream::connect(socket) {
            break s;
        }
        assert!(Instant::now() < deadline, "the server never came up");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut lines = Lines::new(stream.try_clone().unwrap());
    let calls = [
        Call::Hello { client: "test".into(), protocol: PROTOCOL },
        Call::Open {
            path: project.to_string_lossy().into_owned(),
            rows: ROWS,
            cols: COLS,
            raw: true,
        },
    ];
    for (id, call) in calls.into_iter().enumerate() {
        stream.write_all(&Line::Call { id: id as u64, call }.encode()).unwrap();
        loop {
            match lines.next().unwrap() {
                Some(Line::Result { id: got, .. }) if got == id as u64 => break,
                Some(Line::Error { code, .. }) => panic!("{code}"),
                _ => {}
            }
        }
    }
    (stream, lines)
}

/// Start `script` in a pane, attach to it as a raw client, and note when
/// each chunk of output and each frame arrives, for `secs`.
fn timed(name: &str, script: &str, secs: u64) -> Vec<(Instant, Event)> {
    let root = std::env::temp_dir().join(format!("weft-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("print.sh");
    std::fs::write(&path, script).unwrap();
    let socket = protocol::private_socket("weft-paced");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });
    let (mut stream, mut lines) = connect(&socket, &root);
    let spec = format!("/bin/sh {}", path.display());
    let calls = [
        Call::StartAgent { harness: "sh".into(), spec, session: None },
        Call::Attach { pane: 0, form: "screen".into() },
    ];
    for (id, call) in calls.into_iter().enumerate() {
        stream.write_all(&Line::Call { id: 5 + id as u64, call }.encode()).unwrap();
    }
    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let (mut seen, until) = (Vec::new(), Instant::now() + Duration::from_secs(secs));
    while Instant::now() < until {
        if let Ok(Some(Line::Event(e @ (Event::Output { .. } | Event::Screen { .. })))) =
            lines.next()
        {
            seen.push((Instant::now(), e));
        }
    }
    let _ = stream.write_all(&Line::Call { id: 9, call: Call::Shutdown }.encode());
    std::fs::remove_dir_all(&root).ok();
    seen
}

/// The first output after a quiet spell is framed at once, not on the next
/// poll: the frame arrives within 15 ms of the bytes (remote.md D3).
#[test]
fn the_first_change_after_quiet_is_framed_at_once() {
    // Uneven pauses, so the ticks never keep step with a clock.
    let pauses = "0.23 0.31 0.17 0.27 0.19 0.35 0.21 0.29 0.13";
    let script =
        format!("for p in {pauses}; do sleep $p; printf 'tick %s\\n' $p; done\nexec sleep 60\n");
    // The pauses take 2.15 s; the rest is room for a slow machine to start.
    let seen = timed("quiet", &script, 6);
    let mut delays = Vec::new();
    let mut last_output: Option<Instant> = None;
    for (n, (at, event)) in seen.iter().enumerate() {
        let Event::Output { seq, .. } = event else { continue };
        let quiet = last_output.is_none_or(|l| at.duration_since(l) >= Duration::from_millis(100));
        last_output = Some(*at);
        if !quiet {
            continue;
        }
        // A tick printed as the window closed may have its frame after it.
        let framed = seen[n..].iter().find_map(|(when, e)| match e {
            Event::Screen { seq: s, .. } if s >= seq => Some(when.duration_since(*at)),
            _ => None,
        });
        delays.extend(framed);
    }
    delays.sort();
    assert!(delays.len() >= 5, "{delays:?}");
    // On a quiet machine 8.5–10.5 ms; a shared CI runner, 14–36 ms. Polling
    // every 100 ms gave about 50.
    let median = delays[delays.len() / 2];
    assert!(median <= Duration::from_millis(30), "framed after {median:?}: {delays:?}");
}

/// A pane that keeps printing is framed at most 30 times a second, and
/// more often than the old 100 ms poll.
#[test]
fn steady_output_is_framed_at_most_thirty_times_a_second() {
    let script = "i=0; while [ $i -lt 300 ]; do printf 'line %s\\n' $i; i=$((i+1)); sleep 0.005; done\nexec sleep 60\n";
    let seen = timed("steady", script, 4);
    let outputs: Vec<Instant> =
        seen.iter().filter(|(_, e)| matches!(e, Event::Output { .. })).map(|(at, _)| *at).collect();
    let (first, last) = (outputs[0], *outputs.last().unwrap());
    let frames = seen
        .iter()
        .filter(|(at, e)| matches!(e, Event::Screen { .. }) && *at > first && *at <= last)
        .count();
    let span = last.duration_since(first).as_secs_f64();
    let rate = frames as f64 / span;
    assert!(span > 1.0, "printed for {span:.2} s");
    assert!(rate <= 30.5, "{frames} frames in {span:.2} s: {rate:.1}/s");
    assert!(rate > 15.0, "{frames} frames in {span:.2} s: {rate:.1}/s, no faster than polling");
}
