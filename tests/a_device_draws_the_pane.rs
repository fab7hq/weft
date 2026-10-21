//! remote.md §14 test 1: a pane writes 5 MiB with mode changes, and a device
//! attaching at any point draws the same screen as a terminal fed every byte
//! (remote.md D1 to D3).

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use weft::protocol::{self, Call, Event, Line, Lines, PROTOCOL};
use weft::server;
use weft_term::{Snapshot, Term};

const ROWS: u16 = 24;
const COLS: u16 = 80;

/// 5 MiB a harness might print: colour, wide characters, cursor moves,
/// clears, a scroll region, input modes, and the alternate screen entered
/// and left again and again. Ends on the normal screen.
fn five_mib() -> Vec<u8> {
    let mut out = Vec::new();
    let mut n: u64 = 7;
    let mut next = || {
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        n >> 33
    };
    let mut line = 0u64;
    while out.len() < 5 * 1024 * 1024 {
        line += 1;
        match next() % 40 {
            0 => out.extend_from_slice(b"\x1b[?1049h\x1b[H\x1b[2J"),
            1 => out.extend_from_slice(b"\x1b[?1049l"),
            2 => out.extend_from_slice(
                format!("\x1b[{};{}H", next() % 24 + 1, next() % 80 + 1).as_bytes(),
            ),
            3 => out.extend_from_slice(b"\x1b[2J"),
            4 => out.extend_from_slice(b"\x1b[5;20r\x1b[5;1H"),
            5 => out.extend_from_slice(b"\x1b[r"),
            6 => out.extend_from_slice(b"\x1b[?2004h\x1b[?1h\x1b[?25l"),
            7 => out.extend_from_slice(b"\x1b[?2004l\x1b[?1l\x1b[?25h"),
            8 => out.extend_from_slice(b"\x1b[K"),
            _ => {}
        }
        let colour = 31 + next() % 7;
        out.extend_from_slice(
            format!(
                "\x1b[{colour};1mline {line}\x1b[0m 表示 ✓ {}\r\n",
                "x".repeat((next() % 60) as usize)
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(b"\x1b[?1049l\x1b[0mthe end\r\n");
    out
}

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

/// A device: attaches to pane 0 and draws every frame it is sent, until the
/// frames stop. Answers with the screen it drew and how many frames it got.
fn device(socket: PathBuf, project: PathBuf) -> Receiver<(String, Snapshot, usize, bool)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut stream, mut lines) = connect(&socket, &project);
        stream
            .write_all(
                &Line::Call { id: 9, call: Call::Attach { pane: 0, form: "screen".into() } }
                    .encode(),
            )
            .unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut drawn: Option<Term> = None;
        let (mut frames, mut first_full) = (0, false);
        while let Ok(Some(line)) = lines.next() {
            if let Line::Event(Event::Screen { full, rows, cols, bytes, .. }) = line {
                if frames == 0 {
                    first_full = full;
                }
                if full {
                    drawn = Some(Term::new(rows, cols, 0, |_| {}));
                }
                drawn.as_mut().expect("a full frame first").feed(&bytes);
                frames += 1;
            }
        }
        let screen = drawn.expect("frames");
        let _ = tx.send((screen.contents(), screen.snapshot(), frames, first_full));
    });
    rx
}

#[test]
fn a_device_attaching_at_any_point_draws_the_screen_every_byte_draws() {
    let root = std::env::temp_dir().join(format!("weft-screens-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let data = root.join("five-mib.bin");
    std::fs::write(&data, five_mib()).unwrap();
    let script = root.join("print.sh");
    std::fs::write(&script, format!("sleep 0.5\ncat '{}'\nexec sleep 60\n", data.display()))
        .unwrap();

    let socket = protocol::private_socket("weft-screens");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });

    // The reference: a client from before the pane started, fed every byte.
    let (mut stream, mut lines) = connect(&socket, &root);
    stream
        .write_all(
            &Line::Call {
                id: 5,
                call: Call::StartAgent {
                    harness: "sh".into(),
                    spec: format!("/bin/sh {}", script.display()),
                    session: None,
                },
            }
            .encode(),
        )
        .unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut every_byte = Term::new(ROWS, COLS, 0, |_| {});
    let mut seen = 0usize;
    let mut devices = vec![device(socket.clone(), root.clone())];
    let mut marks = vec![1 << 20, 5 << 19, 4 << 20].into_iter().peekable();
    while let Ok(Some(line)) = lines.next() {
        if let Line::Event(Event::Output { bytes, .. }) = line {
            every_byte.feed(&bytes);
            seen += bytes.len();
            if marks.peek().is_some_and(|m| seen >= *m) {
                marks.next();
                devices.push(device(socket.clone(), root.clone()));
            }
        }
    }
    assert!(seen >= 5 * 1024 * 1024, "the whole 5 MiB went by: {seen}");
    devices.push(device(socket.clone(), root.clone()));
    assert_eq!(devices.len(), 5, "before, three times during, and after");

    let want = &every_byte;
    assert!(want.contents().contains("the end"), "{}", want.contents());
    for (i, d) in devices.into_iter().enumerate() {
        let (contents, shown, frames, first_full) =
            d.recv_timeout(Duration::from_secs(30)).unwrap();
        assert!(first_full, "device {i}: its first frame is a full one");
        assert!(frames >= 1, "device {i}");
        assert_eq!(contents, want.contents(), "device {i} after {frames} frames");
        assert!(shown == want.snapshot(), "device {i}: colours, cursor and modes too");
    }
    let _ = stream.write_all(&Line::Call { id: 6, call: Call::Shutdown }.encode());
    std::fs::remove_dir_all(&root).ok();
}

/// Open the project as a `raw` client: the answer's screens drawn, then
/// every chunk printed after them, until output stops. Answers with what it
/// drew.
fn late_client(socket: PathBuf, project: PathBuf) -> Receiver<(String, Snapshot)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            if let Ok(s) = UnixStream::connect(&socket) {
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
        let mut opened = serde_json::Value::Null;
        for (id, call) in calls.into_iter().enumerate() {
            stream.write_all(&Line::Call { id: id as u64, call }.encode()).unwrap();
            loop {
                match lines.next().unwrap() {
                    Some(Line::Result { id: got, result }) if got == id as u64 => {
                        opened = result;
                        break;
                    }
                    Some(Line::Error { code, .. }) => panic!("{code}"),
                    _ => {}
                }
            }
        }
        let screen = opened["screens"]
            .as_array()
            .and_then(|s| s.first())
            .and_then(protocol::screen_of)
            .expect("the pane's screen in the answer");
        let mut drawn = Term::new(screen.rows, screen.cols, 0, |_| {});
        drawn.feed(&screen.bytes);
        let mut held = screen.seq;
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        while let Ok(Some(line)) = lines.next() {
            if let Line::Event(Event::Output { seq, bytes, .. }) = line
                && seq > held
            {
                assert_eq!(seq, held + 1, "no chunk missing");
                held = seq;
                drawn.feed(&bytes);
            }
        }
        let _ = tx.send((drawn.contents(), drawn.snapshot()));
    });
    rx
}

#[test]
fn a_client_opening_late_draws_modes_set_long_before() {
    let root = std::env::temp_dir().join(format!("weft-late-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    // The modes first, then over 5 MiB on the alternate screen.
    let mut out = b"\x1b[?2004h\x1b[?1049h\x1b[?1h".to_vec();
    let mut line = 0u64;
    while out.len() < 5 * 1024 * 1024 {
        line += 1;
        out.extend_from_slice(
            format!("\x1b[3{}mline {line}\x1b[0m 表示 ✓\r\n", line % 7).as_bytes(),
        );
    }
    out.extend_from_slice(b"the end\r\n");
    let data = root.join("late.bin");
    std::fs::write(&data, out).unwrap();
    let script = root.join("print.sh");
    std::fs::write(&script, format!("sleep 0.5\ncat '{}'\nexec sleep 60\n", data.display()))
        .unwrap();

    let socket = protocol::private_socket("weft-late");
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
    });

    let (mut stream, mut lines) = connect(&socket, &root);
    stream
        .write_all(
            &Line::Call {
                id: 5,
                call: Call::StartAgent {
                    harness: "sh".into(),
                    spec: format!("/bin/sh {}", script.display()),
                    session: None,
                },
            }
            .encode(),
        )
        .unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    let mut every_byte = Term::new(ROWS, COLS, 0, |_| {});
    let (mut seen, mut late) = (0usize, None);
    while let Ok(Some(line)) = lines.next() {
        if let Line::Event(Event::Output { bytes, .. }) = line {
            every_byte.feed(&bytes);
            seen += bytes.len();
            if late.is_none() && seen >= 9 << 19 {
                late = Some(late_client(socket.clone(), root.clone()));
            }
        }
    }
    assert!(seen >= 5 * 1024 * 1024, "the whole 5 MiB went by: {seen}");
    assert!(every_byte.bracketed_paste() && every_byte.alt_screen());
    let (contents, shown) =
        late.expect("opened late").recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(contents, every_byte.contents());
    assert!(shown == every_byte.snapshot(), "the same modes and colours, as well as the text");
    let _ = stream.write_all(&Line::Call { id: 6, call: Call::Shutdown }.encode());
    std::fs::remove_dir_all(&root).ok();
}
