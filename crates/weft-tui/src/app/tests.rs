//! The window's tests: a real daemon on a scratch project, driven by keys.

use super::*;
use crate::ledger::Sent;
use crossterm::event::KeyCode;
use serde_json::json;
use weft_core::offers::first_line;
use weft_core::readiness::{Gap, Readiness};

/// The machine these tests run on, as each test says it is: nothing of the
/// real machine's reaches the daemon. What a real harness does is a person's
/// test to run, not a unit test's.
pub(crate) type World = weftd::outside::Fixed;

/// A machine set up for RingFrame: its CLI installed and current, and Claude
/// Code and Codex each with the `rf` plugin. `more` goes first, so a test's
/// own answers win over these.
pub(crate) fn set_up(more: impl FnOnce(World) -> World) -> World {
    more(World::new())
        .answers("ringframe", &["--version"], 0, "ringframe 0.1.3\n")
        .answers("ringframe", &["profile", "show", "--host", "codex"], 0, r#"{"invocation_prefix": "$rf:"}"#)
        .answers(
            "ringframe",
            &["profile", "show", "--host", "claude-code"],
            0,
            r#"{"invocation_prefix": "/rf:"}"#,
        )
        .answers("ringframe", &["ask", "preflight"], 0, "{}")
        .answers("ringframe", &["init"], 0, "{}")
        .answers("ringframe", &["ask", "confirm"], 0, "{}")
        .answers("ringframe", &["ask", "submitted"], 0, "{}")
        .answers(
            "codex",
            &["plugin", "list"],
            0,
            r#"{"installed": [{"pluginId": "rf@fab7", "installed": true, "enabled": true}], "available": []}"#,
        )
        .answers(
            "claude",
            &["plugin", "list"],
            0,
            r#"{"installed": [{"id": "rf@fab7", "enabled": true}], "available": []}"#,
        )
        .with_release(Ok(serde_json::json!(
            {"revision": "0.1.3", "latest": "0.1.3", "plugin": "0.1.3", "behind": false}
        )))
}

/// A session backed by a real daemon on a scratch root, because the app is a
/// client and there is no honest way to test it without one; the daemon's
/// machine is the world the test describes.
pub(crate) fn test_session(name: &str) -> (PathBuf, crate::client::Session) {
    test_session_in(name, set_up(|w| w))
}

pub(crate) fn test_session_in(name: &str, world: World) -> (PathBuf, crate::client::Session) {
    test_session_at(&format!("weft-app-{name}"), "", world)
}

/// A project folder named `folder` (a scratch one when empty) in a scratch
/// directory of its own, with a daemon for it.
pub(crate) fn test_session_at(
    scratch: &str,
    folder: &str,
    world: World,
) -> (PathBuf, crate::client::Session) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let mut root = std::env::temp_dir().join(format!("{scratch}-{}-{n}", std::process::id()));
    // Process ids come round again: a folder an earlier run left under this
    // name holds that run's record, which this test must not read.
    let _ = std::fs::remove_dir_all(&root);
    if !folder.is_empty() {
        root.push(folder);
    }
    std::fs::create_dir_all(&root).expect("root");
    // A real workspace is a Git repository with a commit, which is what
    // RingFrame requires before it will compile an Ask. A fixture that is
    // not one tests a situation Weft now refuses.
    for args in [vec!["init", "-q"], vec!["commit", "-q", "--allow-empty", "-m", "fixture"]] {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(&args)
            .env("GIT_AUTHOR_NAME", "weft")
            .env("GIT_AUTHOR_EMAIL", "weft@example.invalid")
            .env("GIT_COMMITTER_NAME", "weft")
            .env("GIT_COMMITTER_EMAIL", "weft@example.invalid")
            .output();
    }
    let socket = weft_proto::private_socket("weft-app");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve_in(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(weft_core::harness::fixture::harnesses()),
            world.shared(),
        );
    });
    let session = connect_when_listening(&socket, &root);
    (root, session)
}

/// A daemon started on a thread listens a moment later.
pub(crate) fn connect_when_listening(
    socket: &std::path::Path,
    root: &std::path::Path,
) -> crate::client::Session {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !weft_proto::is_live(socket) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    crate::client::Session::connect(socket, root, 24, 80).expect("connect")
}

/// Weft's service went and came back at the same socket (a crash with Start
/// at login on, or an update that started it again as the same protocol):
/// the window opens its project on the new one by itself.
#[test]
fn a_window_reconnects_when_its_daemon_comes_back() {
    let (root, session) = test_session("reconnect");
    let socket = session.socket().to_path_buf();
    let mut a = App::with_session(root.clone(), Toggle, session);
    sess!(a).shutdown();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !sess!(a).lost {
        assert!(std::time::Instant::now() < deadline, "the end was not noticed");
        a.turn(Duration::from_millis(50)).unwrap();
    }
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve_in(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(weft_core::harness::fixture::harnesses()),
            set_up(|w| w).shared(),
        );
    });
    while sess!(a).lost {
        assert!(std::time::Instant::now() < deadline, "it never reconnected");
        a.turn(Duration::from_millis(50)).unwrap();
    }
    assert!(!a.quit, "the window goes on");
    sess!(a).shutdown();
    let _ = std::fs::remove_dir_all(root);
}

/// Nothing comes back: the window ends, saying why, rather than freezing.
#[test]
fn a_window_whose_daemon_never_comes_back_ends_and_says_why() {
    let (root, session) = test_session("gone-for-good");
    let mut a = App::with_session(root.clone(), Toggle, session);
    sess!(a).shutdown();
    a.lost_since = Some(std::time::Instant::now() - LOST_FOR);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !a.quit {
        assert!(std::time::Instant::now() < deadline, "it never ended");
        a.turn(Duration::from_millis(50)).unwrap();
    }
    assert_eq!(a.ended.as_deref(), Some("Weft's service ended. Run weft to start it again."));
    let _ = std::fs::remove_dir_all(root);
}

pub(crate) fn app() -> App {
    app_in(set_up(|w| w))
}

/// An agent running in a project, in this world, once the daemon has asked
/// the world about each harness.
pub(crate) fn app_in(world: World) -> App {
    let (root, session) = test_session_in("codex", world);
    let agent = agent_program(&root.with_extension("agents"));
    let mut a = App::with_session(root, Toggle, session);
    a.add("codex", &agent).expect("spawn");
    asked(&mut a);
    agent_says(&mut a, "codex", "fixture", "ready");
    a
}

/// Until the daemon has asked its world about each harness in the project in
/// front: asking what can be started asks about every harness on the
/// machine, as Weft does when it opens a project.
pub(crate) fn asked(a: &mut App) {
    a.look_for_agents(Opening::Nothing);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline
        && ["codex", "claude-code"].iter().any(|h| a.readiness(h) == Readiness::Unknown)
    {
        a.settle();
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An agent that behaves as `cat` and writes the pane variables weftd gave
/// it to `<dir>/pane-<id>`, so a test can signal as its hooks would.
pub(crate) fn agent_program(dir: &std::path::Path) -> String {
    std::fs::create_dir_all(dir).expect("agent dir");
    let path = dir.join("agent.sh");
    let script = "#!/bin/sh\n\
        d=$(dirname \"$0\")\n\
        printf '%s\\n%s\\n' \"$WEFT_PANE_TOKEN\" \"$WEFT_SOCKET\" > \"$d/pane-$WEFT_PANE.tmp\"\n\
        mv \"$d/pane-$WEFT_PANE.tmp\" \"$d/pane-$WEFT_PANE\"\n\
        exec cat\n";
    std::fs::write(&path, script).expect("agent script");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    // Through `/bin/sh`: a script just written may still be open in a process
    // another test thread is forking, and executing it then fails as busy.
    format!("/bin/sh {}", path.display())
}

/// What the first agent's hook would signal, sent as `weft signal` sends it,
/// and waited for until the daemon has taken it.
pub(crate) fn agent_says(a: &mut App, harness: &str, session: &str, event: &str) {
    use std::io::Write;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let told = a.root().with_extension("agents").join("pane-0");
    while std::time::Instant::now() < deadline && !told.exists() {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = std::fs::read_to_string(&told).expect("the agent said where it is");
    let (token, socket) = text.split_once('\n').expect("token and socket");
    let sent = weft_core::signal::Sent {
        harness: harness.into(),
        hook: "Hook".into(),
        session: session.into(),
        workspace: Some(a.root().to_string_lossy().into_owned()),
        state: Some(event.into()),
        pane: Some(0),
        token: Some(token.into()),
        ..Default::default()
    };
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent))).encode();
    let mut stream = std::os::unix::net::UnixStream::connect(socket.trim()).expect("the daemon");
    stream.write_all(&line).expect("sent");
    let want = weft_core::turns::Turn::recorded(event);
    while std::time::Instant::now() < deadline && a.pane_turn(0) != want {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Select the first work in the tree, as the arrows would.
pub(crate) fn on_work(a: &mut App) {
    assert!(a.select(|r| matches!(r, Row::Ask { .. })), "a work in the tree: {:?}", a.rows());
}

/// One work on the board, selected.
fn with_unit(sent: Sent) -> App {
    let mut a = app();
    a.set_units(vec![unit(sent)]);
    on_work(&mut a);
    a
}

/// Put these Asks on the project's real ledger, so the daemon knows them.
pub(crate) fn record_units(app: &App, ids: &[&str]) {
    let lines: String = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            serde_json::json!({
                "schema": "ringframe.ledger/1", "event_id": format!("evt_{i}"),
                "type": "ask.compiled", "time": "2026-09-19T14:02:00Z", "id": id,
                "actor": {"kind": "human", "id": "local-user"}, "links": [],
                "data": {
                    "title": "t", "selected_capability": "native_plan",
                    "delivery_mode": "human_handoff",
                    "host": {"name": "codex", "profile_id": "codex"},
                    "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
                    "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
                    "source_verified": "exact", "limitations": [],
                    "classification": {}, "route_explanation": {}
                }
            })
            .to_string()
                + "\n"
        })
        .collect();
    let rf = app.root().join(".fab7/rf");
    std::fs::create_dir_all(&rf).expect("rf");
    std::fs::write(rf.join("ledger.jsonl"), lines).expect("ledger");
}

/// An Eval of these Asks, opened and gathered, as `ringframe eval open` leaves
/// it: `[E]VAL` then offers its debate without running the CLI.
pub(crate) fn gather_on_record(app: &App, asks: &[&str]) {
    use std::io::Write;
    let ev = |kind: &str, n: u32, data: serde_json::Value| {
        serde_json::json!({"schema": "ringframe.ledger/1", "event_id": format!("evt_g{n}"),
            "type": kind, "time": "2026-09-19T14:04:00Z", "id": "evl_g",
            "actor": {"kind": "human", "id": "local-user"}, "links": [], "data": data})
    };
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(app.root().join(".fab7/rf/ledger.jsonl"))
        .expect("ledger");
    writeln!(f, "{}", ev("eval.opened", 1, serde_json::json!({"basis": {"asks": asks}})))
        .expect("opened");
    writeln!(
        f,
        "{}",
        ev("eval.gathered", 2, serde_json::json!({"eval_id": "evl_g", "host": "weft"}))
    )
    .expect("gathered");
    // The daemon reads the record on its quarter-second tick.
    std::thread::sleep(Duration::from_millis(600));
}

/// A project whose daemon can see this work, because it is really on the
/// ledger. `set_units` puts a unit in front of the person; only the record
/// puts it where an act can reach it.
fn recorded(sent: Sent) -> App {
    recorded_in(sent, set_up(|w| w))
}

fn recorded_in(sent: Sent, world: World) -> App {
    let mut a = app_in(world);
    let mut events = vec![serde_json::json!({
        "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
        "time": "2026-09-19T14:02:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "local-user"}, "links": [],
        "data": {
            "title": "health endpoint", "selected_capability": "native_plan",
            "delivery_mode": "human_handoff",
            "host": {"name": "codex", "profile_id": "codex"},
            "source": {"role": "source_intent", "path": "x", "bytes": 2, "sha256": "a"},
            "prompt": {"role": "generated_prompt", "path": "y", "bytes": 9, "sha256": "b"},
            "source_verified": "exact", "limitations": [],
            "classification": {}, "route_explanation": {}
        }
    })];
    if sent == Sent::ReadyToSend {
        events.push(serde_json::json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_2", "type": "ask.confirmed",
            "time": "2026-09-19T14:03:00Z", "id": "ask_1",
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {"confirmation": {"observed_by": "skill"}}
        }));
    }
    let rf = a.root().join(".fab7/rf");
    std::fs::create_dir_all(&rf).expect("rf");
    let lines: String = events.iter().map(|e| format!("{e}\n")).collect();
    std::fs::write(rf.join("ledger.jsonl"), lines).expect("ledger");
    // Wait for the daemon's next look, then take what it read.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.session_mut().units.is_empty() {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    a.take_the_board();
    a
}

pub(crate) fn unit(sent: Sent) -> Unit {
    Unit {
        ask_id: "ask_1".into(),
        title: "health endpoint".into(),
        harness: "codex".into(),
        session_ref: Some("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into()),
        route: "native_plan".into(),
        asked_at: "2026-09-19T14:02:00Z".into(),
        delivery_mode: "human_handoff".into(),
        confirmed: true,
        sent,
        ..Default::default()
    }
}

/// A key, and whatever answers it brings.
pub(crate) fn press(a: &mut App, code: KeyCode) {
    a.on_key(KeyEvent::new(code, KeyModifiers::NONE)).expect("key");
    a.settle();
}

pub(crate) fn ctrl(a: &mut App, c: char) {
    a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)).expect("key");
    a.settle();
}

#[test]
fn an_agent_that_quits_closes_its_pane_and_keeps_its_work() {
    let mut a = with_unit(Sent::TakenByAgent);
    a.focus = Focus::Agent;
    a.input(0, &[4]).expect("Ctrl+D ends /bin/cat");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() > 0 {
        a.pump();
        a.notice_an_agent_that_ended();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(a.pane_count(), 0, "the pane is gone, not left dead on screen");
    assert!(a.modal.is_none(), "no panel: the row says it");
    assert_eq!(a.focus, Focus::Weft, "keystrokes never go to a dead process");
    assert!(a.hint_text().is_some_and(|h| h.contains("still on the list")));
    assert!(a.rows().iter().any(|r| matches!(r, Row::Ask { .. })), "its work stays listed");
}

/// The loop blocks on one channel that the terminal and the daemon both
/// send into, so a key is handled the moment it arrives, not at the
/// next look.
#[test]
fn a_key_reaches_the_agent_as_soon_as_it_is_pressed() {
    let mut a = app();
    a.focus = Focus::Agent;
    let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let began = std::time::Instant::now();
    a.wakes.send(crate::client::Wake::Terminal(Event::Key(key))).expect("sent");
    a.turn(Duration::from_secs(5)).expect("a turn");
    assert!(began.elapsed() < TICK / 5, "waited {:?}", began.elapsed());
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !a.pane_text(0).unwrap_or_default().contains('q')
    {
        a.turn(Duration::from_millis(50)).expect("a turn");
    }
    assert!(a.pane_text(0).unwrap_or_default().contains('q'), "the agent got it");
}

/// A frame is drawn only when something changed: none while nothing
/// happens, and one for a burst of output that arrived before it.
#[test]
fn frames_are_drawn_for_what_changed_and_no_more() {
    let mut a = app();
    let mut t =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
    // Whatever the start-up still has to say, said: the harness's readiness
    // comes from asking it, which can take seconds on a busy machine.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while std::time::Instant::now() < deadline
        && (a.session_mut().readiness.get("codex").is_none()
            || a.setup_view().is_none()
            || a.frame(&mut t, Duration::from_millis(500)).expect("a frame"))
    {
        a.frame(&mut t, Duration::from_millis(50)).expect("a frame");
    }
    let (mut idle, end) = (0, std::time::Instant::now() + Duration::from_secs(1));
    while std::time::Instant::now() < end {
        idle += a.frame(&mut t, TICK).expect("a frame") as usize;
    }
    assert_eq!(idle, 0, "frames drawn over an idle second");

    let burst: String = (0..200).map(|i| format!("burst line {i}\n")).collect();
    a.input(0, burst.as_bytes()).expect("typed");
    std::thread::sleep(Duration::from_millis(400));
    let mut frames = 0;
    while a.frame(&mut t, TICK).expect("a frame") {
        frames += 1;
    }
    assert_eq!(frames, 1, "one frame for everything that had arrived");
    assert!(a.pane_text(0).unwrap_or_default().contains("burst line 199"));
}

#[test]
fn the_wheel_scrolls_the_agent_only_while_it_has_the_keys() {
    let mut a = app();
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    a.on_mouse(wheel);
    assert!(a.hint_text().is_none(), "in Weft the wheel is the terminal's");
    a.focus = Focus::Agent;
    a.on_mouse(wheel);
    assert!(a.hint_text().is_some(), "in the agent it is Weft's, and says why nothing moved");
}

/// An agent that asked for mouse reports, as Codex does in its full screen,
/// gets the wheel as a terminal sends it; Weft scrolls nothing of its own.
#[test]
fn the_wheel_goes_to_an_agent_that_asked_for_mouse_reports() {
    let mut a = app();
    a.focus = Focus::Agent;
    // `cat` sends back what it is given: the agent turns on SGR mouse reports.
    a.input(0, b"\x1b[?1000h\x1b[?1006h\n").expect("typed");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !a.wants_the_mouse(0) {
        assert!(std::time::Instant::now() < deadline, "the agent never asked for the mouse");
        a.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    let wheel = |kind| MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE };
    a.on_mouse(wheel(MouseEventKind::ScrollUp));
    a.on_mouse(wheel(MouseEventKind::ScrollDown));
    assert!(a.hint_text().is_none(), "nothing to explain: the agent scrolls itself");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        a.pump();
        let text = a.pane_text(0).unwrap_or_default();
        if text.contains("[<64;") && text.contains("[<65;") {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "no wheel report reached the agent: {text}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Enter on an Ask opens its Details (T11.2): the Ask only, under a
/// breadcrumb of where it stands; ← goes back to the tree.
#[test]
fn enter_on_an_ask_opens_its_details_and_back_goes_to_the_tree() {
    let mut a = app();
    let mut u = unit(Sent::TakenByAgent);
    u.harness = "claude-code".into();
    a.set_units(vec![u]);
    a.selected = a.rows().iter().position(|r| matches!(r, Row::Ask { .. })).expect("row");
    press(&mut a, KeyCode::Enter);
    let d = a.detail().expect("its Details");
    assert_eq!(d.view, View::Work("ask_1".into()));
    assert_eq!(d.title, "health endpoint  ›  not evaluated yet");
    assert!(d.lines[0].starts_with("ASK ["), "{:?}", d.lines);
    assert!(!d.lines.iter().any(|l| l.starts_with("EVAL") || l.starts_with("SEAL")), "no round");
    press(&mut a, KeyCode::Left);
    assert!(a.detail().is_none() && a.modal.is_none());
}

/// A session on record is never offered as "the latest" (the
/// `gl` case): what can be started is each harness fresh.
#[test]
fn a_session_on_record_is_never_offered_as_the_latest() {
    // `starts()` only offers a harness that is on this machine; in this
    // world, Codex is.
    let mut a = app();
    // A session run in a terminal: its hook signals with no pane.
    let told = a.root().with_extension("agents").join("pane-0");
    let socket = std::fs::read_to_string(told).expect("the agent said where it is");
    let socket = socket.lines().nth(1).expect("the socket").to_string();
    let sent = weft_core::signal::Sent {
        harness: "codex".into(),
        hook: "Hook".into(),
        session: "01a0bdb6".into(),
        workspace: Some(a.root().to_string_lossy().into_owned()),
        state: Some("turn_ended".into()),
        ..Default::default()
    };
    let line = weft_proto::Line::Event(weft_proto::Event::Signal(Box::new(sent))).encode();
    std::io::Write::write_all(
        &mut std::os::unix::net::UnixStream::connect(socket).expect("the daemon"),
        &line,
    )
    .expect("sent");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline
        && !a.session_mut().sessions.iter().any(|s| s.id == "01a0bdb6")
    {
        std::thread::sleep(Duration::from_millis(20));
        a.pump();
    }
    assert!(a.session_mut().sessions.iter().any(|s| s.id == "01a0bdb6"), "on record");
    a.refresh_for_test();
    let codex: Vec<_> = a.starts().iter().filter(|c| c.harness == "codex").collect();
    assert_eq!(codex.len(), 1, "start fresh, and nothing else");
    assert_eq!(codex[0].spec, "codex");
}

#[test]
fn a_harness_with_no_receipt_here_is_offered_fresh_only() {
    let mut a = app();
    a.refresh_for_test();
    for c in a.starts() {
        assert!(!c.spec.contains("resume"), "and nothing is invented: {}", c.spec);
    }
}

/// A made-up harness, added as nothing but a Weft harness file beside the
/// daemon's `config.toml`, is offered with no change to Weft.
#[test]
fn a_made_up_harness_is_offered_from_its_harness_file_alone() {
    let (root, _) = test_session("zed");
    let home = root.join("machine");
    std::fs::create_dir_all(home.join("harnesses")).expect("harnesses");
    // Its program answers its plugin listing as a harness set up for
    // RingFrame does.
    let program = home.join("zed").display().to_string();
    let zed = weft_core::harness::fixture::text("codex")
        .replace("title = \"Codex\"", "title = \"Zed Agent\"")
        .replace("program = \"codex\"", &format!("program = \"{program}\""));
    std::fs::write(home.join("harnesses/zed-agent.toml"), zed).expect("its file");
    let world = set_up(|w| {
        w.answers(
            &program,
            &["plugin", "list"],
            0,
            r#"{"installed": [{"pluginId": "rf@fab7", "enabled": true, "installed": true}], "available": []}"#,
        )
    });
    let socket = weft_proto::private_socket("weft-app-zed");
    let _ = std::fs::remove_file(&socket);
    let (listening, config) = (socket.clone(), home.join("config.toml"));
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve_in(&listening, &config, None, world.shared());
    });
    let session = connect_when_listening(&socket, &root);
    let mut a = App::with_session(root, Toggle, session);
    // Offered once it has said it is set up: the first look asks it.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        a.look_for_agents(Opening::Nothing);
        a.settle();
        if a.starts().iter().any(|s| s.harness == "zed-agent")
            || std::time::Instant::now() > deadline
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let zed: Vec<_> = a.starts().iter().filter(|s| s.harness == "zed-agent").collect();
    assert_eq!(zed.len(), 1, "offered fresh: {:?}", a.starts());
    assert_eq!(zed[0].spec, program);
    assert_eq!(a.harness_for_program(&program).as_deref(), Some("zed-agent"));
}

#[test]
fn send_is_offered_for_an_ask_nobody_answered() {
    // The defect this closes: a chooser that timed out left a row on the
    // board with nothing anyone could do about it.
    let mut a = app();
    let mut u = unit(Sent::NotSent);
    u.confirmed = false;
    u.unanswered = true;
    a.set_units(vec![u]);
    on_work(&mut a);
    assert_eq!(a.unavailable(Act::Proceed), None, "proceeding is the person's yes");
}

/// A project that routes its acts. Weft reads the file once, so this sets
/// the routing directly rather than going through a temporary HOME.
fn routed(pairs: &[(&str, &str)]) -> App {
    let mut a = app();
    let acts: String = pairs.iter().map(|(k, v)| format!("{k} = \"{v}\"\n")).collect();
    a.routing = weft_core::config::read(
        &format!("[routing]\n{acts}"),
        &weft_core::harness::fixture::harnesses(),
    )
    .routing(a.root());
    a.set_units(vec![unit(Sent::TakenByAgent)]);
    a
}

#[test]
fn an_act_goes_to_the_harness_this_project_routes_it_to() {
    // Codex asks, Claude evaluates. The work was done in codex.
    let a = routed(&[("eval", "claude-code")]);
    assert_eq!(a.deciding_harness(Act::Eval).as_deref(), Some("claude-code"));
    // Seal was not routed, so it still follows the work.
    assert_eq!(a.deciding_harness(Act::Seal).as_deref(), Some("codex"));
    // And Send is never routed: it is the delivery of an Ask already made.
    assert_eq!(weft_core::board::Board::act_key(Act::Proceed), None);
}

#[test]
fn readiness_is_asked_of_the_harness_the_act_will_go_to() {
    // The point of routing: Eval going to Claude Code needs Claude Code
    // set up, whatever the work was done in.
    let mut a = routed(&[("eval", "claude-code")]);
    on_work(&mut a);
    a.set_readiness("codex", Readiness::Ready);
    a.set_readiness("claude-code", Readiness::Missing(Gap::Plugin));
    let said = a.unavailable(Act::Eval).expect("a reason");
    assert!(said.contains("claude-code"), "names the harness it would go to: {said}");
    // And the harness that did the work being ready does not help.
    assert_eq!(a.unavailable(Act::Seal), None, "seal still follows the work");
}

#[test]
fn a_routed_act_with_no_pane_is_still_offered() {
    // The daemon starts a pane of the routed harness when none is free.
    let mut a = routed(&[("eval", "claude-code")]);
    on_work(&mut a);
    a.set_readiness("claude-code", Readiness::Ready);
    assert_eq!(a.unavailable(Act::Eval), None);
}

#[test]
fn a_project_that_routes_nothing_behaves_exactly_as_before() {
    let a = routed(&[]);
    assert!(a.routing().is_empty());
    for act in [Act::Ask, Act::Eval, Act::Seal] {
        assert_eq!(a.deciding_harness(act).as_deref(), Some("codex"), "{act:?}");
    }
}

#[test]
fn asking_opens_on_the_harness_the_project_asks_in() {
    let mut a = routed(&[("ask", "codex")]);
    a.act(Act::Ask);
    let Some(Modal::Ask { target, .. }) = a.modal.clone() else { panic!("{:?}", a.modal) };
    assert_eq!(a.harness_at(target), Some("codex"));
}

#[test]
fn weft_starts_in_weft_not_in_the_agent() {
    assert_eq!(app().focus, Focus::Weft);
}

#[test]
fn the_toggle_moves_between_weft_and_the_agent_both_ways() {
    let mut a = app();
    ctrl(&mut a, ']');
    assert_eq!(a.focus, Focus::Agent);
    ctrl(&mut a, ']');
    assert_eq!(a.focus, Focus::Weft);
}

#[test]
fn coming_back_out_of_the_agent_shows_the_work_list_again() {
    let mut a = app();
    press(&mut a, KeyCode::Char('b'));
    assert!(!a.show_work());
    ctrl(&mut a, ']');
    ctrl(&mut a, ']');
    assert!(a.show_work(), "Ctrl+] back from the agent lands on the list");
}

#[test]
fn in_the_agent_x_is_typed_rather_than_quitting_weft() {
    let mut a = app();
    a.focus = Focus::Agent;
    press(&mut a, KeyCode::Char('x'));
    assert!(a.modal.is_none());
    assert!(!a.quit);
}

#[test]
fn quitting_asks_first_and_leaves_the_agents_running_by_default() {
    let mut a = app();
    press(&mut a, KeyCode::Char('x'));
    assert!(matches!(a.modal, Some(Modal::Quit)));
    assert!(!a.quit, "quitting is never immediate");
    press(&mut a, KeyCode::Enter);
    assert!(a.quit);
    assert!(!a.stop_agents_on_quit);
}

/// A harness with no hook before its first prompt (Antigravity) has named no
/// session yet: its running pane is an agent under AGENTS all the same, and
/// an Ask goes to it.
#[test]
fn an_agent_that_named_no_session_yet_is_listed_and_can_be_asked() {
    let (root, session) = test_session_in("codex", set_up(|w| w));
    let agent = agent_program(&root.with_extension("agents"));
    let mut a = App::with_session(root, Toggle, session);
    a.add("codex", &agent).expect("spawn");
    asked(&mut a);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !crate::ui::sidebar_text(&a).contains("AGENTS") {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    let rows = crate::ui::sidebar_text(&a);
    assert!(rows.contains("AGENTS") && rows.contains("● codex"), "{rows}");
    assert!(a.select(|r| matches!(r, Row::Agent { agent, .. } if agent.session.is_empty())));
    press(&mut a, KeyCode::Char('a'));
    assert!(matches!(a.modal, Some(Modal::Ask { target: 0, .. })), "{:?}", a.modal);
}

#[test]
fn asking_opens_a_box_and_types_nothing_yet() {
    let mut a = app();
    press(&mut a, KeyCode::Char('a'));
    assert!(matches!(a.modal, Some(Modal::Ask { .. })));
}

#[test]
fn an_empty_intent_is_not_sent() {
    let mut a = app();
    press(&mut a, KeyCode::Char('a'));
    press(&mut a, KeyCode::Enter);
    assert!(matches!(a.modal, Some(Modal::Ask { .. })), "still composing");
}

#[test]
fn a_typed_intent_becomes_a_confirmation_not_an_injection() {
    let mut a = app();
    press(&mut a, KeyCode::Char('a'));
    for c in "fix the build".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Enter);
    let Some(Modal::Bar(Bar::Send(p))) = a.modal.clone() else {
        panic!("expected a confirmation, got {:?}", a.modal)
    };
    let text = String::from_utf8(p.payload).unwrap();
    assert!(text.ends_with("ask fix the build"), "got {text:?}");
    assert!(text.contains("rf:"), "the profile's prefix is used: {text:?}");
}

/// `recorded`, plus an Eval over that Ask, once the daemon has read both.
fn recorded_and_evaluated() -> App {
    let mut a = recorded(Sent::TakenByAgent);
    let rf = a.root().join(".fab7/rf/ledger.jsonl");
    let mut ledger = std::fs::read_to_string(&rf).expect("ledger");
    ledger.push_str(
        &(serde_json::json!({
            "schema": "ringframe.ledger/1", "event_id": "evt_9", "type": "eval.completed",
            "time": "2026-09-19T15:00:00Z", "id": "evl_1",
            "actor": {"kind": "human", "id": "local-user"}, "links": [],
            "data": {"verdict": "drifted", "confidence": 0.67, "basis": {"asks": ["ask_1"]}}
        })
        .to_string()
            + "\n"),
    );
    std::fs::write(&rf, ledger).expect("ledger");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && a.session_mut().units.first().is_none_or(|u| u.check.is_none())
    {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    a.take_the_board();
    a
}

fn follow_up_payload(mut a: App, intent: &str) -> String {
    on_work(&mut a);
    press(&mut a, KeyCode::Char('f'));
    for c in intent.chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Enter);
    let Some(Modal::Bar(Bar::Send(p))) = a.modal.clone() else {
        panic!("expected a confirmation, got {:?}", a.modal)
    };
    String::from_utf8(p.payload).unwrap()
}

#[test]
fn a_follow_up_carries_the_ask_or_the_eval_it_follows() {
    let text = follow_up_payload(recorded(Sent::TakenByAgent), "add a retry");
    assert!(text.ends_with("ask [follow-up ask_1] add a retry"), "got {text:?}");
    let text = follow_up_payload(recorded_and_evaluated(), "fix what it found");
    assert!(text.ends_with("ask [follow-up evl_1] fix what it found"), "got {text:?}");
}

#[test]
fn a_plain_ask_carries_no_marker() {
    let mut a = recorded(Sent::TakenByAgent);
    press(&mut a, KeyCode::Char('a'));
    for c in "new thing".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Enter);
    let Some(Modal::Bar(Bar::Send(p))) = a.modal.clone() else { panic!("{:?}", a.modal) };
    let text = String::from_utf8(p.payload).unwrap();
    assert!(!text.contains("[follow-up"), "got {text:?}");
}

/// SEND TO is a list of the agents: ↑ ↓ pick one, and Enter asks that one.
#[test]
fn send_to_picks_the_agent_with_the_arrows_and_asks_it() {
    let mut a = recorded(Sent::TakenByAgent);
    a.add("claude-code", "/bin/cat").expect("a second agent");
    a.settle();
    a.set_readiness("claude-code", Readiness::Ready);
    press(&mut a, KeyCode::Char('a'));
    let target = |a: &App| match &a.modal {
        Some(Modal::Ask { target, .. }) => *target,
        other => panic!("{other:?}"),
    };
    assert_eq!(target(&a), 0);
    press(&mut a, KeyCode::Down);
    assert_eq!(target(&a), 1);
    press(&mut a, KeyCode::Down);
    assert_eq!(target(&a), 0, "it wraps");
    press(&mut a, KeyCode::Up);
    assert_eq!(target(&a), 1);
    for c in "new thing".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Enter);
    let Some(Modal::Bar(Bar::Send(p))) = a.modal.clone() else { panic!("{:?}", a.modal) };
    assert_eq!(a.harness_at(p.pane), Some("claude-code"), "asked of the agent picked");
}

/// A confirmation is a bar: Enter sends, ← cancels and takes it off the
/// daemon's queue.
#[test]
fn the_send_bar_sends_on_enter_and_cancels_on_left() {
    for (key, sent) in [(KeyCode::Enter, true), (KeyCode::Left, false)] {
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char('a'));
        for c in "new thing".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        press(&mut a, KeyCode::Enter);
        assert!(matches!(a.modal, Some(Modal::Bar(Bar::Send(_)))), "{:?}", a.modal);
        press(&mut a, key);
        assert!(a.modal.is_none(), "{key:?}: {:?}", a.modal);
        assert_eq!(a.focus == Focus::Agent, sent, "{key:?}: sending goes where the keys went");
    }
}

#[test]
fn backspace_edits_the_intent() {
    let mut a = app();
    press(&mut a, KeyCode::Char('a'));
    for c in "abc".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Backspace);
    let Some(Modal::Ask { text, .. }) = a.modal.clone() else { panic!() };
    assert_eq!(text, "ab");
}

#[test]
fn eval_and_seal_always_confirm_before_typing() {
    for (key, expect) in [('e', "eval"), ('s', "seal")] {
        // Eval goes on with a gathered round; Seal seals a judged one.
        let mut a = if key == 'e' {
            let a = recorded(Sent::TakenByAgent);
            gather_on_record(&a, &["ask_1"]);
            a
        } else {
            recorded_and_evaluated()
        };
        a.settle();
        a.take_the_board();
        // Eval acts on Open, Seal on an Eval.
        let on = |r: &Row| match key {
            'e' => matches!(r, Row::Open { .. }),
            _ => matches!(r, Row::Eval { .. }),
        };
        assert!(a.select(on), "{key}: {:?}", a.rows());
        press(&mut a, KeyCode::Char(key));
        let Some(Modal::Bar(Bar::Send(p))) = a.modal.clone() else {
            panic!("{key} must confirm first, got {:?}", a.modal)
        };
        let text = String::from_utf8(p.payload).unwrap();
        // Whatever words follow, the command token is closed, so Enter
        // means send rather than pick a completion. `$` is what this world's
        // RingFrame profile gives Codex.
        assert!(text.starts_with(&format!("$rf:{expect} ")), "got {text:?}");
    }
}

#[test]
fn an_agent_asking_its_person_is_asked_about_first_and_can_be_overruled() {
    // Its hook said it is asking; typing now would answer it. Weft asks
    // first; sending is the default, and Cancel is one key
    // away.
    let mut a = recorded(Sent::ReadyToSend);
    a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
    assert!(a.waiting(0));
    a.do_inject(&pending_for(0));
    let Some(Modal::Bar(Bar::SendAnyway { why, .. })) = a.modal.clone() else {
        panic!("it must ask first, got {:?}", a.modal)
    };
    assert_eq!(why, "the agent is asking you something");
    // ← cancels; Enter is SEND ANYWAY.
    press(&mut a, KeyCode::Left);
    assert!(a.modal.is_none(), "{:?}", a.modal);
}

fn pending_for(pane: usize) -> Pending {
    Pending {
        staged: "pnd_1".into(),
        pane,
        payload: b"anything".to_vec(),
        what: "send".into(),
        why: Vec::new(),
    }
}

/// turn-state.md §6 tests 3 and 4, on the panel: with no event, or with
/// one that is not ready, Weft asks and says why; the person may still
/// type it anyway.
#[test]
fn an_agent_that_has_not_said_it_is_ready_is_asked_about_first() {
    use weft_core::turns::Turn;
    for (state, why) in [
        (None, "Weft can't tell whether the agent is ready"),
        (Some(Turn::Working), "the agent is working"),
        (Some(Turn::Waiting), "the agent is asking you something"),
    ] {
        let mut a = recorded(Sent::ReadyToSend);
        a.session_mut().set_turns(vec![state]);
        a.do_inject(&pending_for(0));
        let Some(Modal::Bar(Bar::SendAnyway { why: said, .. })) = a.modal.clone() else {
            panic!("{state:?} must ask first, got {:?}", a.modal)
        };
        assert_eq!(said, why);
        let drawn = crate::ui::panel_text(&a).join("\n");
        assert!(drawn.contains(why), "{drawn}");
    }
    for state in [Turn::Ready, Turn::TurnEnded] {
        let mut a = recorded(Sent::ReadyToSend);
        a.session_mut().set_turns(vec![Some(state)]);
        a.do_inject(&pending_for(0));
        assert!(a.modal.is_none(), "{state:?} is typed into: {:?}", a.modal);
    }
}

/// §6 test 1, drawn: an agent's row carries its state from its receipt;
/// Asks under a round carry none.
#[test]
fn an_agents_row_shows_its_state_from_its_receipt() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    agent_says(&mut a, "codex", "fixture", "working");
    let rows = crate::ui::sidebar_text(&a);
    let agent = rows.lines().find(|l| l.contains("● codex")).expect("its row");
    assert!(agent.contains("working"), "{rows}");
    let work = rows.lines().find(|l| l.contains("health endpoint")).expect("the work");
    assert!(!work.contains("working"), "{rows}");
}

/// NEEDS YOU is an agent asking its person for input, and nothing
/// else: not a prompt ready to send, not a finished turn. Its harness's row carries the badge, and Space goes to the agent.
#[test]
fn only_an_agent_asking_for_input_needs_you_and_space_goes_to_it() {
    let mut a = app();
    let mut ready = unit(Sent::ReadyToSend);
    ready.ask_id = "ask_2".into();
    a.set_units(vec![unit(Sent::Arrived { exact: true }), ready]);
    agent_says(&mut a, "codex", "fixture", "turn_ended");
    assert_eq!(a.needs_you(), 0, "a finished turn and a prompt to send ask nothing");
    press(&mut a, KeyCode::Char(' '));
    assert_eq!(a.hint_text(), Some("Nothing needs your input."));
    assert!(a.detail().is_none(), "Space opens no Ask");
    let rows = crate::ui::sidebar_text(&a);
    assert!(!rows.contains("needs you") && !rows.contains("· 1 ●"), "{rows}");

    agent_says(&mut a, "codex", "fixture", "waiting");
    assert_eq!(a.needs_you(), 1);
    let rows = crate::ui::sidebar_text(&a);
    let badge = rows.lines().find(|l| l.contains("needs you")).expect("its row");
    assert!(badge.contains("codex") && !badge.contains("ASKED"), "{rows}");
    press(&mut a, KeyCode::Char(' '));
    assert_eq!(a.pane_focus, 0);
    assert!(!a.show_work && a.detail().is_none(), "Space shows the agent asking");
}

/// Space into a folded project opens it to the harness row of the agent
/// asking, so that row is picked and an arrow moves on from it.
#[test]
fn space_into_a_folded_project_opens_it_to_the_agent_asking() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    agent_says(&mut a, "codex", "fixture", "waiting");
    a.selected = 0;
    press(&mut a, KeyCode::Enter);
    assert!(a.rows()[0].folded(), "folded");
    press(&mut a, KeyCode::Char(' '));
    let rows = a.rows();
    assert!(
        matches!(&rows[a.selected], Row::Agent { agent, .. } if agent.harness == "codex"),
        "{:?} at {}",
        rows,
        a.selected
    );
    let at = a.selected;
    press(&mut a, KeyCode::Down);
    assert!(a.selected != at && a.rows()[a.selected].selectable(), "the arrow moved on from it");
}

/// The list shows an agent the moment it is opened, before anything is
/// asked, and OPEN counts it: OPEN is the harnesses open, not the Asks.
#[test]
fn an_agent_is_on_the_list_and_counted_as_soon_as_it_is_opened() {
    let mut a = app();
    let screen = drawn(&mut a);
    assert!(screen.contains("● codex"), "drawn before anything is asked:\n{screen}");
    assert!(screen.contains("0 open"), "no Ask is Open yet: {screen}");
    assert_eq!(a.open_count(), 1);
    a.set_units(vec![unit(Sent::ReadyToSend), unit(Sent::Arrived { exact: true })]);
    assert_eq!(a.open_count(), 1, "Asks are not agents");
    let drawn = drawn(&mut a);
    assert!(drawn.lines().next().is_some_and(|t| t.contains("OPEN  1")), "{drawn}");
}

/// The owner's case: someone asks, quits before the prompt is composed,
/// and comes back. RingFrame recorded the Ask as asked, so the list shows
/// it as ASKING and its view shows what was asked.
#[test]
fn an_ask_recorded_as_asked_is_on_the_list_and_its_view_shows_the_words() {
    let mut a = app();
    let rf = a.root().join(".fab7/rf");
    std::fs::create_dir_all(rf.join("asks/ask_1")).expect("asks");
    std::fs::write(rf.join("asks/ask_1/source.txt"), "fix the login bug\nplease").expect("src");
    let requested = serde_json::json!({
        "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.requested",
        "time": "2026-09-27T10:00:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "local-user"}, "links": [],
        "data": {"title": "fix the login bug", "captured_by": "prompt_hook",
                 "source": {"role": "source_intent", "path": "asks/ask_1/source.txt",
                            "bytes": 24, "sha256": "a"},
                 "host": {"name": "codex", "session_ref": "fixture"}}
    });
    std::fs::write(rf.join("ledger.jsonl"), format!("{requested}\n")).expect("ledger");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.session_mut().units.is_empty() {
        a.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    a.take_the_board();
    let rows = crate::ui::sidebar_text(&a);
    let row = rows.lines().find(|l| l.contains("fix the login bug") && l.contains("ASKING"));
    assert!(row.is_some(), "{rows}");
    on_work(&mut a);
    press(&mut a, KeyCode::Enter);
    let lines = a.detail().expect("its view").lines.clone();
    assert!(lines.contains(&"ASK [ASKING]".to_string()), "{lines:?}");
    assert!(lines.contains(&"  please".to_string()), "the words as asked: {lines:?}");
    assert!(!a.proceeds(), "nothing to send yet");
}

/// An agent with nothing asked of it has a row too, so its badge has
/// somewhere to be.
#[test]
fn an_agent_with_no_asks_still_has_a_row_for_its_badge() {
    let mut a = app();
    agent_says(&mut a, "codex", "fixture", "waiting");
    let rows = crate::ui::sidebar_text(&a);
    assert!(rows.lines().any(|l| l.contains("codex") && l.contains("needs you")), "{rows}");
}

/// An agent the person is not looking at that starts asking
/// for input sends one notification; a turn that ends sends none; the agent in
/// front sends none; one that stays waiting is not announced again;
/// turned off, nothing is sent.
#[test]
fn an_agent_you_are_not_looking_at_tells_you_once_when_it_needs_you() {
    use weft_core::turns::Turn;
    let mut a = app();
    a.add("codex", "/bin/cat").expect("a second agent");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() < 2 {
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    a.pane_focus = 0;
    let set = |a: &mut App, states: Vec<Option<Turn>>| {
        a.session_mut().set_turns(states);
        a.notices()
    };
    assert!(set(&mut a, vec![Some(Turn::Working), Some(Turn::Working)]).is_empty(), "first sight");
    assert!(
        set(&mut a, vec![Some(Turn::Working), Some(Turn::TurnEnded)]).is_empty(),
        "a turn that ended is not an alarm"
    );
    let told = set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("codex") && told[0].contains("needs your input"), "{told:?}");
    assert!(
        set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]).is_empty(),
        "once per change"
    );
    assert!(
        set(&mut a, vec![Some(Turn::TurnEnded), Some(Turn::Waiting)]).is_empty(),
        "the agent in front is never announced"
    );
    a.session_mut().notify = false;
    set(&mut a, vec![Some(Turn::Working), Some(Turn::Working)]);
    assert!(set(&mut a, vec![Some(Turn::Working), Some(Turn::Waiting)]).is_empty(), "turned off");
}

/// Panes are told apart by id, so one opening while another starts
/// asking loses nothing and announces only the one asking.
#[test]
fn an_agent_opening_while_another_asks_sends_one_notification() {
    use weft_core::turns::Turn;
    let mut a = app();
    a.session_mut().set_turns(vec![Some(Turn::Working)]);
    assert!(a.notices().is_empty(), "first sight");
    a.add("codex", "/bin/cat").expect("a second agent");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() < 2 {
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    a.pane_focus = 1;
    a.session_mut().set_turns(vec![Some(Turn::Waiting), None]);
    let told = a.notices();
    assert_eq!(told.len(), 1, "{told:?}");
}

#[test]
fn a_notification_goes_through_the_terminal_or_rings_its_bell() {
    assert_eq!(
        notification(Some("iTerm.app"), "codex: turn ended"),
        b"\x1b]9;codex: turn ended\x07"
    );
    assert_eq!(notification(Some("ghostty"), "x"), b"\x1b]9;x\x07");
    assert_eq!(notification(Some("Apple_Terminal"), "x"), b"\x07", "the bell where there is none");
    assert_eq!(notification(None, "x"), b"\x07");
    assert_eq!(
        notification(Some("WezTerm"), "a\x07b\x1bc"),
        b"\x1b]9;abc\x07",
        "nothing in a message can end the sequence early"
    );
}

fn drawn(a: &mut App) -> String {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
    terminal.draw(|frame| crate::ui::draw(frame, a)).expect("draw");
    let b = terminal.backend().buffer().clone();
    (0..30)
        .map(|y| (0..120).map(|x| b.cell((x, y)).map_or(" ", |c| c.symbol())).collect())
        .collect::<Vec<String>>()
        .join("\n")
}

fn click(a: &mut App, column: u16, row: u16) {
    for kind in [MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left)] {
        a.on_mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE });
    }
    a.settle();
}

/// A click on a row selects it, as the arrows would,
/// and types nothing into any agent.
#[test]
fn a_click_on_a_row_selects_it() {
    let mut a = app();
    let mut second = unit(Sent::Arrived { exact: true });
    second.ask_id = "ask_2".into();
    second.title = "the second one".into();
    a.set_units(vec![unit(Sent::Arrived { exact: true }), second]);
    let screen = drawn(&mut a);
    let (y, line) =
        screen.lines().enumerate().find(|(_, l)| l.contains("the second one")).expect("drawn");
    let x = line.find("the second one").expect("x") as u16;
    let before = a.pane_text(0);
    a.focus = Focus::Agent;
    click(&mut a, x, y as u16);
    assert_eq!(a.selected_unit().map(|u| u.ask_id.as_str()), Some("ask_2"));
    assert_eq!(a.focus, Focus::Weft, "the list has the keys, as after an arrow");
    std::thread::sleep(Duration::from_millis(200));
    a.pump();
    assert_eq!(a.pane_text(0), before, "a click never types into an agent");
}

/// A click in the agent, on the line a list row is drawn on, is the
/// agent's: it selects nothing and the keys stay where they were.
#[test]
fn a_click_in_the_agent_beside_a_row_selects_nothing() {
    let mut a = app();
    let mut second = unit(Sent::Arrived { exact: true });
    second.ask_id = "ask_2".into();
    second.title = "the second one".into();
    a.set_units(vec![unit(Sent::Arrived { exact: true }), second]);
    let screen = drawn(&mut a);
    let y = screen.lines().position(|l| l.contains("the second one")).expect("drawn");
    let before = a.selected;
    a.focus = Focus::Agent;
    click(&mut a, 100, y as u16);
    assert_eq!(a.selected, before, "no row was selected");
    assert_eq!(a.focus, Focus::Agent, "the keys stay with the agent");
}

/// And a click on an agent's tab goes to that agent, as its number would.
#[test]
fn a_click_on_an_agents_tab_goes_to_it() {
    let mut a = app();
    a.add("codex", "/bin/cat").expect("a second agent");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() < 2 {
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    a.pane_focus = 0;
    let screen = drawn(&mut a);
    let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains("· codex")).expect("tabs");
    let x = line
        .char_indices()
        .filter(|(_, c)| *c == '2')
        .map(|(b, _)| line[..b].chars().count())
        .next()
        .expect("x") as u16;
    click(&mut a, x + 2, y as u16);
    assert_eq!(a.pane_focus, 1);
    click(&mut a, 0, 29);
    assert_eq!(a.pane_focus, 1, "a click elsewhere goes nowhere");
}

/// Where `needle` starts on the screen, in cells: a wide character such
/// as `⚡` takes two.
fn cell_of(a: &mut App, needle: &str) -> (u16, u16) {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).expect("terminal");
    terminal.draw(|frame| crate::ui::draw(frame, a)).expect("draw");
    let b = terminal.backend().buffer().clone();
    for y in 0..30 {
        for x in 0..120 {
            let from: String = (x..120).map(|x| b[(x, y)].symbol()).collect();
            if from.starts_with(needle) {
                return (x, y);
            }
        }
    }
    panic!("{needle} is not on the screen")
}

/// `⚡` is two cells wide, and a tab after a turbo tab is where it is drawn.
#[test]
fn a_click_on_the_tab_after_a_turbo_tab_goes_to_it() {
    let mut a = app();
    a.add("codex", "/bin/cat").expect("a second agent");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() < 2 {
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    a.session_mut().panes[0].turbo = true;
    a.pane_focus = 0;
    let screen = drawn(&mut a);
    let tabs = screen.lines().nth(1).expect("tabs");
    let second = tabs
        .rsplit_once("2 ")
        .map(|(_, t)| format!("2 {}", t.split("    ").next().unwrap_or("")))
        .expect("the second tab");
    let (x, y) = cell_of(&mut a, &second);
    click(&mut a, x + second.chars().count() as u16 - 1, y);
    assert_eq!(a.pane_focus, 1, "its last cell is its own");
}

/// A send made from the detail view closes it, so the person lands in the
/// agent the keys went to rather than on a view that blocks the screen.
#[test]
fn proceeding_from_the_detail_view_closes_it_and_lands_in_the_agent() {
    // The prompt is still to be sent, so [P]ROCEED sends it.
    let mut a = recorded_in(
        Sent::ReadyToSend,
        set_up(|w| w.answers("ringframe", &["ask", "copy"], 0, "Ship it.\n")),
    );
    on_work(&mut a);
    press(&mut a, KeyCode::Enter); // the row's detail view
    assert!(a.detail().is_some(), "the detail view is open");
    press(&mut a, KeyCode::Char('p'));
    assert!(
        matches!(a.modal, Some(Modal::Bar(Bar::Send(_)))),
        "{:?} / {:?}",
        a.modal,
        a.hint_text()
    );
    press(&mut a, KeyCode::Enter);
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert!(a.detail().is_none(), "the detail view closed");
    assert_eq!(a.focus, Focus::Agent, "and the keys are the agent's");
}

/// T11.2: `[P]ROCEED` stays on a sent Ask's Details, and pressing it says
/// there is nothing to send rather than sending it again.
#[test]
fn proceed_on_a_sent_asks_details_says_why_and_sends_nothing() {
    let mut a = recorded(Sent::Arrived { exact: true });
    on_work(&mut a);
    press(&mut a, KeyCode::Enter);
    assert!(a.detail().is_some(), "the detail view is open");
    press(&mut a, KeyCode::Char('p'));
    assert!(a.modal.is_none(), "nothing is sent: {:?}", a.modal);
    let said = a.hint_text().unwrap_or_default().to_string();
    assert!(said.ends_with("has nothing to send. [F] FOLLOW UP asks for more."), "{said}");
}

/// Sent on the person's yes, and what the agent reported it received was not
/// the Ask, or nothing came: the pane's notice says so, in words, so the
/// person is not offered the send again blind.
#[test]
fn a_send_that_did_not_arrive_as_the_ask_is_said() {
    let mut a = recorded(Sent::ReadyToSend);
    for (code, key) in
        [("NotTheAsk", "not_typed.not_the_ask"), ("NoReceipt", "not_typed.no_receipt")]
    {
        a.session_mut().last_refusal = Some(code.into());
        let said = drawn(&mut a);
        assert!(said.contains(&word(key)[..40]), "{code}: {said}");
    }
    // RingFrame could not judge what arrived: sent, and not recorded.
    a.session_mut().last_refusal = None;
    a.session_mut().unrecorded = Some("ringframe exit 2".into());
    let said = drawn(&mut a).lines().last().unwrap_or_default().to_string();
    assert!(said.contains("RingFrame did not record it"), "{said}");
}

/// Turbo mode is switched from Weft, by `[T]` or a click on it in the
/// title bar, and the title bar says where it stands.
#[test]
fn turbo_mode_is_switched_by_its_key_or_a_click_and_shown_in_the_title() {
    let mut a = app();
    let title = |a: &mut App| drawn(a).lines().next().unwrap_or("").to_string();
    assert!(title(&mut a).contains("[T]URBO OFF"), "{}", title(&mut a));
    press(&mut a, KeyCode::Char('t'));
    assert!(a.turbo());
    let on = title(&mut a);
    assert!(on.contains('⚡') && on.contains("[T]URBO ON"), "{on}");
    assert!(a.hint_text().is_some_and(|h| h.contains("next")), "{:?}", a.hint_text());
    // One character per cell on the drawn line, so this is the column.
    let x = on.find("[T]URBO").map(|b| on[..b].chars().count()).expect("x") as u16;
    click(&mut a, x + 3, 0);
    assert!(!a.turbo(), "a click switches it back");
}

/// An agent started in turbo mode says so on its tab, and only it.
#[test]
fn an_agent_in_turbo_mode_says_so_on_its_tab() {
    let mut a = app();
    let tabs = |a: &mut App| drawn(a).lines().nth(1).unwrap_or("").to_string();
    assert!(!tabs(&mut a).contains('⚡'), "{}", tabs(&mut a));
    a.session_mut().panes[0].turbo = true;
    assert!(tabs(&mut a).contains("· codex ⚡"), "{}", tabs(&mut a));
}

/// An agent that has exited carries no badge: no `⚡` on its tab, and
/// its harness's row says it is not running.
#[test]
fn an_agent_that_exited_carries_no_running_badge() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    a.session_mut().panes[0].turbo = true;
    a.session_mut().panes[0].running = false;
    let tabs = drawn(&mut a).lines().nth(1).unwrap_or("").to_string();
    assert!(!tabs.contains('⚡'), "{tabs}");
}

/// The owner's case: an Ask sent through Weft, which no hook saw arrive.
/// Nothing asks for the send again: [P] has nothing to do, and what
/// comes next is the person's, through [F] FOLLOW UP or the bar's acts.
#[test]
fn a_sent_ask_is_never_sent_again_by_proceed() {
    let mut a = app();
    let u = unit(Sent::Unconfirmed);
    a.set_units(vec![u]);
    agent_says(&mut a, "codex", "fixture", "turn_ended");
    assert_eq!(a.needs_you(), 0, "a finished turn asks nothing");
    on_work(&mut a);
    press(&mut a, KeyCode::Char('p'));
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert_eq!(
        a.hint_text(),
        Some("health endpoint has nothing to send. [F] FOLLOW UP asks for more.")
    );
}

#[test]
fn cancelling_a_confirmation_types_nothing() {
    let mut a = recorded(Sent::TakenByAgent);
    gather_on_record(&a, &["ask_1"]);
    a.settle();
    a.take_the_board();
    assert!(a.select(|r| matches!(r, Row::Open { .. })), "Open: {:?}", a.rows());
    press(&mut a, KeyCode::Char('e'));
    assert!(matches!(a.modal, Some(Modal::Bar(Bar::Send(_)))));
    press(&mut a, KeyCode::Left); // [←] CANCEL
    assert!(a.modal.is_none());
    assert_eq!(a.focus, Focus::Weft, "cancelling never moves you into the agent");
}

#[test]
fn confirming_types_it_and_puts_you_in_the_agent() {
    let mut a = recorded(Sent::TakenByAgent);
    gather_on_record(&a, &["ask_1"]);
    a.settle();
    a.take_the_board();
    assert!(a.select(|r| matches!(r, Row::Open { .. })), "Open: {:?}", a.rows());
    press(&mut a, KeyCode::Char('e'));
    press(&mut a, KeyCode::Enter);
    assert!(a.modal.is_none());
    assert_eq!(a.focus, Focus::Agent, "you land where the work is happening");
}

#[test]
fn an_unavailable_action_explains_itself_in_the_hint_and_opens_nothing() {
    let mut a = app();
    press(&mut a, KeyCode::Char('e'));
    assert!(a.modal.is_none(), "never a dialog: {:?}", a.modal);
    assert_eq!(a.hint_text(), Some("Select Open to Eval its Asks."));
}

#[test]
fn proceed_does_the_step_the_selected_row_is_actually_waiting_on() {
    // A confirmed Ask nobody has submitted: there is a step to take, so
    // the verb is live. What it then types is the send path's own test.
    let mut a = recorded(Sent::ReadyToSend);
    on_work(&mut a);
    assert_eq!(a.unavailable(Act::Proceed), None);

    // An Ask that was compiled and never answered is waiting on the
    // harness's chooser, not on the person, so there is nothing to send.
    let mut a = recorded(Sent::NotSent);
    on_work(&mut a);
    press(&mut a, KeyCode::Char('p'));
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert_eq!(
        a.hint_text(),
        Some("health endpoint has nothing to send. [F] FOLLOW UP asks for more.")
    );
}

/// Each act acts where it applies: pressed on another row it
/// says what to select, in the cards' words.
#[test]
fn an_act_pressed_on_a_row_it_does_not_act_on_says_what_to_select() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    let row = |a: &App, f: &dyn Fn(&Row) -> bool| a.rows().iter().position(f).expect("row");
    a.selected = row(&a, &|r| matches!(r, Row::Ask { .. }));
    for (key, said) in [
        ('e', "Select Open to Eval its Asks."),
        ('s', "Select an Eval to Seal it."),
        ('a', "Select an agent to Ask."),
    ] {
        press(&mut a, KeyCode::Char(key));
        assert_eq!(a.hint_text(), Some(said), "{key}");
        assert!(a.modal.is_none());
    }
    a.selected = row(&a, &|r| matches!(r, Row::Open { .. }));
    press(&mut a, KeyCode::Char('a'));
    assert_eq!(a.hint_text(), Some("Select an agent to Ask."));
    press(&mut a, KeyCode::Char('s'));
    assert_eq!(a.hint_text(), Some("Select an Eval to Seal it."));
}

#[test]
fn enter_unfolds_what_is_closed_and_acts_on_what_is_open() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    // The selection lands on the work, so the first `[Enter]` opens it.
    a.selected = a.rows().iter().position(|r| matches!(r, Row::Ask { .. })).expect("work");
    assert!(matches!(a.rows()[a.selected], Row::Ask { .. }));
    press(&mut a, KeyCode::Enter);
    assert!(a.detail().is_some(), "an action opens its detail view");
    press(&mut a, KeyCode::Left);
    assert!(a.detail().is_none());

    // On a project it only ever folds and unfolds. Nothing in the sidebar
    // removes anything on `[Enter]`.
    a.selected = 0;
    assert!(matches!(a.rows()[0], Row::Project { folded: false, .. }));
    press(&mut a, KeyCode::Enter);
    assert!(matches!(a.rows()[0], Row::Project { folded: true, .. }));
    assert_eq!(a.rows().len(), 1, "folded, it hides its harnesses");
    press(&mut a, KeyCode::Enter);
    assert!(matches!(a.rows()[0], Row::Project { folded: false, .. }));
}

#[test]
fn back_folds_the_level_rather_than_quitting_anything() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    a.selected = 0;
    press(&mut a, KeyCode::Left);
    assert!(a.rows()[0].folded());
    assert!(!a.quit);
}

#[test]
fn folding_stays_folded_when_the_selection_moves() {
    // A fold is a decision about the tree, not about the cursor. It used
    // to be undone by moving away, which made it useless for tidying.
    let mut a = app();
    let mut second = unit(Sent::NotSent);
    second.ask_id = "ask_2".into();
    a.set_units(vec![unit(Sent::NotSent), second]);
    a.selected = 0;
    press(&mut a, KeyCode::Enter);
    assert!(a.rows()[0].folded());
    press(&mut a, KeyCode::Down);
    assert!(a.rows()[0].folded(), "still folded after moving");
}

#[test]
fn the_verb_is_live_when_there_is_a_step_and_dim_when_there_is_not() {
    // A prompt still to be sent is the one thing [P] carries. A sent one
    // is not carried at all: its Eval is the bar's [E]VAL, when the
    // person wants it. A closed one only follows up.
    let a = with_unit(Sent::ReadyToSend);
    assert!(a.proceeds());
    assert_eq!(a.unavailable(Act::Proceed), None);

    let a = with_unit(Sent::Arrived { exact: true });
    assert!(!a.proceeds());
    assert!(a.unavailable(Act::Proceed).is_some_and(|s| s.contains("FOLLOW UP")));
    assert_eq!(a.unavailable(Act::Eval), None, "its Eval is the person's to run");

    let mut a = app();
    let mut sealed = unit(Sent::Arrived { exact: true });
    (sealed.sealed, sealed.seal_id) = (Some("accepted".into()), Some("sel_1".into()));
    a.set_units(vec![sealed]);
    // Sealed, it is under its Seal, in no round to act on: its Details are
    // where Proceed is pressed.
    a.open_view(View::Work("ask_1".into()));
    a.settle();
    assert!(a.unavailable(Act::Proceed).is_some_and(|s| s.contains("FOLLOW UP")));
}

#[test]
fn two_projects_are_two_groups_and_neither_reads_the_other() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    let (other, _) = test_session("second");
    a.open_project(&other.to_string_lossy());
    assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 2);

    // The second project holds nothing, so it is a heading and no more.
    let second = a
        .rows()
        .iter()
        .position(|r| matches!(r, Row::Project { project: 1, .. }))
        .expect("the second project");
    a.selected = second;
    a.follow_the_selection();
    assert_eq!(a.at, 1);
    assert!(a.selected_unit().is_none(), "nothing has been asked for in it");

    // Selecting the first project's work moves the focus back, and an act
    // lands where you are looking.
    let work = a
        .rows()
        .iter()
        .position(|r| matches!(r, Row::Ask { project: 0, .. }))
        .expect("the first project's work");
    a.selected = work;
    a.follow_the_selection();
    assert_eq!(a.at, 0);
    assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("health endpoint".into()));
}

#[test]
fn a_project_closes_out_of_the_window_and_leaves_the_rest() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    let (other, _) = test_session("second");
    a.open_project(&other.to_string_lossy());
    assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 2);

    let second = a
        .rows()
        .iter()
        .position(|r| matches!(r, Row::Project { project: 1, .. }))
        .expect("the second project");
    a.selected = second;
    press(&mut a, KeyCode::Backspace);
    assert!(matches!(a.modal, Some(Modal::CloseProject { .. })), "it asks first");
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.rows().iter().filter(|r| matches!(r, Row::Project { .. })).count(), 1);
    assert!(
        a.rows()
            .iter()
            .any(|r| matches!(r, Row::Ask { ask, .. } if ask.title == "health endpoint"))
    );
}

#[test]
fn the_last_project_is_not_closed_out_from_under_you() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    a.selected = 0;
    press(&mut a, KeyCode::Backspace);
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert_eq!(a.hint_text(), Some("This is the only project open. [X] QUIT closes the window."));
}

#[test]
fn arrows_move_through_every_level_of_the_tree() {
    let mut a = app();
    let mut second = unit(Sent::NotSent);
    second.ask_id = "ask_2".into();
    second.title = "second".into();
    a.set_units(vec![unit(Sent::NotSent), second]);
    // project, AGENTS and its agent, WORK, Open and its two Asks; the
    // arrows step over the gaps and the headings.
    let kinds: Vec<_> = a.rows().iter().map(|r| r.selectable()).collect();
    assert_eq!(kinds, [true, false, false, true, false, false, true, true, true]);
    a.selected = 0;
    press(&mut a, KeyCode::Down);
    assert!(matches!(a.rows()[a.selected], Row::Agent { .. }));
    press(&mut a, KeyCode::Down);
    assert!(matches!(a.rows()[a.selected], Row::Open { .. }));
    press(&mut a, KeyCode::Up);
    press(&mut a, KeyCode::Up);
    assert_eq!(a.selected, 0);
    press(&mut a, KeyCode::Up);
    assert!(matches!(a.rows()[a.selected], Row::Ask { .. }), "it wraps");
}

#[test]
fn readiness_follows_the_harness_that_owns_the_work() {
    // Claude Code ready, Codex not: the same action is available on one
    // and not the other, because the plugin is installed per harness.
    let mut a = app();
    a.add("claude-code", "/bin/cat").expect("a second pane");
    a.settle();
    a.set_readiness("codex", Readiness::Missing(Gap::Plugin));
    a.set_readiness("claude-code", Readiness::Ready);
    let mut codex_row = unit(Sent::Arrived { exact: true });
    codex_row.harness = "codex".into();
    let mut claude_row = unit(Sent::Arrived { exact: true });
    claude_row.ask_id = "ask_2".into();
    claude_row.harness = "claude-code".into();
    a.set_units(vec![codex_row, claude_row]);

    // Harnesses are listed by name, so Codex's work is found, not assumed first.
    a.selected = a
        .rows()
        .iter()
        .position(|r| matches!(r, Row::Ask { unit, .. } if a.units()[*unit].harness == "codex"))
        .expect("the codex row");
    let blocked = a.unavailable(Act::Eval).expect("codex is not set up");
    assert!(blocked.contains("codex is not set up"), "{blocked}");
    // The two units hang under different harnesses, so reaching the
    // second means walking past its harness heading.
    let claude = a
        .rows()
        .iter()
        .position(
            |r| matches!(r, Row::Ask { unit, .. } if a.units()[*unit].harness == "claude-code"),
        )
        .expect("the claude-code row");
    a.selected = claude;
    assert_eq!(a.unavailable(Act::Eval), None, "claude-code is ready");
}

#[test]
fn u_opens_the_ringframe_view_with_no_agent_running() {
    // Setting a harness up is what a person does before any agent runs.
    let (root, session) = test_session("bare-u");
    let mut a = App::with_session(root, Toggle, session);
    assert_eq!(a.pane_count(), 0);
    press(&mut a, KeyCode::Char('u'));
    assert_eq!(a.modal, Some(Modal::RingFrame));
    press(&mut a, KeyCode::Left);
    assert!(a.modal.is_none());
}

#[test]
fn a_key_in_the_weft_menu_does_what_it_names() {
    let mut a = app();
    press(&mut a, KeyCode::Char('w'));
    assert_eq!(a.modal, Some(Modal::Weft));
    press(&mut a, KeyCode::Char('o'));
    assert!(matches!(a.modal, Some(Modal::OpenProject { .. })), "{:?}", a.modal);
}

#[test]
fn a_name_config_toml_does_not_know_is_said_once() {
    let mut a = app();
    a.session_mut().routing.ignored = vec!["eval.debate.judge".into()];
    a.take_the_board();
    assert_eq!(
        a.hint_text(),
        Some("config.toml names something Weft does not know (eval.debate.judge). It is ignored.")
    );
}

#[test]
fn a_leftover_json_file_is_said_once() {
    let mut a = app();
    a.session_mut().routing.leftover = vec!["routing.json".into(), "eval.json".into()];
    a.take_the_board();
    assert_eq!(
        a.hint_text(),
        Some(
            "Weft no longer reads routing.json, eval.json; move what it holds into \
             ~/.fab7/weft/config.toml."
        )
    );
}

#[test]
fn a_harness_weft_cannot_ask_is_unknown_rather_than_missing() {
    let mut a = app();
    a.set_readiness("codex", Readiness::Unknown);
    let why = a.unavailable(Act::Ask).expect("not ready");
    assert!(why.contains("could not ask"), "{why}");
    assert!(!why.contains("[U]PDATE"), "nothing to offer: {why}");
}

#[test]
fn an_ask_this_workspace_could_not_finish_is_refused_before_it_is_typed() {
    // RingFrame refuses at compile, which is after the skill has
    // classified, composed and staged. Weft asks first, so nothing is
    // typed and no turn is spent.
    let mut a = app();
    a.set_workspace_gap(Some("This is not a Git repository.".into()));
    press(&mut a, KeyCode::Char('a'));
    assert!(a.modal.is_none(), "nothing was opened: {:?}", a.modal);
    assert_eq!(a.hint_text(), Some("This is not a Git repository."));
}

#[test]
fn a_refusal_is_shown_as_its_first_sentence() {
    assert_eq!(
        first_line("/tmp/x is not in a Git repository; RingFrame evaluates it. Run `git init`."),
        "/tmp/x is not in a Git repository; RingFrame evaluates it."
    );
    assert_eq!(first_line("one line only"), "one line only");
}

#[test]
fn w_hides_the_work_list_and_brings_it_back() {
    let mut a = app();
    assert!(a.show_work());
    press(&mut a, KeyCode::Char('b'));
    assert!(!a.show_work());
    press(&mut a, KeyCode::Char('b'));
    assert!(a.show_work());
}

#[test]
fn space_never_opens_an_ask() {
    // A prompt ready to send is the person's to send when they choose: it
    // is not NEEDS YOU, so Space leaves the selection where it was.
    let mut a = app();
    let mut ready = unit(Sent::ReadyToSend);
    ready.ask_id = "ask_2".into();
    a.set_units(vec![unit(Sent::TakenByAgent), ready]);
    let before = a.selected;
    press(&mut a, KeyCode::Char(' '));
    assert_eq!(a.selected, before);
    assert!(a.detail().is_none());
    assert_eq!(a.needs_you(), 0);
}

#[test]
fn explaining_a_waiting_agent_says_its_hook_said_so() {
    let mut a = app();
    a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
    press(&mut a, KeyCode::Char('y'));
    let hint = a.hint_text().expect("a sentence");
    assert!(hint.contains("reported, through its hook"), "{hint}");
    a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Working)]);
    press(&mut a, KeyCode::Char('y'));
    assert!(a.hint_text().expect("a sentence").contains("has not reported"));
}

#[test]
fn weft_never_answers_a_waiting_agent_itself() {
    // [Enter] ANSWER IT puts the person in the pane; it types nothing.
    let mut a = app();
    a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
    press(&mut a, KeyCode::Char(' '));
    assert!(!a.show_work(), "Space shows the pane that is waiting");
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.focus, Focus::Agent);
    assert!(a.modal.is_none());
}

#[test]
fn the_board_is_read_from_a_real_ledger_on_disk() {
    let dir = std::env::temp_dir().join(format!("weft-app-{}", std::process::id()));
    std::fs::create_dir_all(dir.join(".fab7/rf")).unwrap();
    let event = json!({
        "schema": "ringframe.ledger/1", "event_id": "evt_1", "type": "ask.compiled",
        "time": "2026-09-19T14:02:00Z", "id": "ask_1",
        "actor": {"kind": "human", "id": "me"}, "links": [],
        "data": {
            "title": "health endpoint", "selected_capability": "native_plan",
            "delivery_mode": "human_handoff", "host": {"name": "codex"},
            "source": {}, "prompt": {}, "source_verified": "exact",
            "limitations": [], "classification": {}, "route_explanation": {}
        }
    });
    std::fs::write(
        dir.join(".fab7/rf/ledger.jsonl"),
        format!("{}\n", serde_json::to_string(&event).unwrap()),
    )
    .unwrap();

    let socket = weft_proto::private_socket("weft-app");
    let _ = std::fs::remove_file(&socket);
    let listening = socket.clone();
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve_in(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(weft_core::harness::fixture::harnesses()),
            set_up(|w| w).shared(),
        );
    });
    let session = connect_when_listening(&socket, &dir);
    let mut a = App::with_session(dir.clone(), Toggle, session);
    a.refresh_for_test();
    assert_eq!(a.units().len(), 1);
    assert_eq!(a.units()[0].title, "health endpoint");
    std::fs::remove_dir_all(&dir).ok();
}

mod start_tests {
    use super::*;
    use crate::ledger::Sent;
    use crossterm::event::KeyCode;

    fn bare() -> App {
        let (root, session) = super::test_session("bare");
        App::with_session(root, Toggle, session)
    }

    #[test]
    fn weft_starts_no_agent_on_its_own() {
        let a = bare();
        assert_eq!(a.pane_count(), 0, "the panel stays empty until asked");
        assert_eq!(a.focus, Focus::Weft);
    }

    #[test]
    fn n_offers_the_agents_that_are_installed() {
        let mut a = bare();
        a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)).expect("key");
        a.settle();
        match a.modal {
            Some(Modal::StartAgent { .. }) => {}
            None => assert!(
                a.hint_text().is_some_and(|h| h.contains("No harness is set up")),
                "either a picker or a plain sentence: {:?}",
                a.hint_text()
            ),
            ref other => panic!("expected a picker, got {other:?}"),
        }
    }

    #[test]
    fn s_sends_rather_than_doing_nothing() {
        // The action bar advertised [S]end it while nothing was wired to it.
        let mut a = bare();
        a.add("codex", "/bin/cat").expect("spawn");
        a.settle();
        a.set_units(vec![Unit {
            ask_id: "ask_1".into(),
            title: "t".into(),
            harness: "codex".into(),
            session_ref: Some("01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into()),
            route: "native_plan".into(),
            asked_at: "now".into(),
            delivery_mode: "human_handoff".into(),
            confirmed: true,
            sent: Sent::ReadyToSend,
            ..Default::default()
        }]);
        on_work(&mut a);
        a.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)).expect("key");
        a.settle();
        assert!(
            a.modal.is_some() || a.hint_text().is_some(),
            "S must do something when the bar offers it"
        );
    }
}

mod picker_tests {
    use super::press;
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn the_agent_picker_moves_and_wraps() {
        let mut a = super::app();
        let choices = a.fresh_starts().len();
        if choices < 2 {
            eprintln!("skipped: needs two agents installed");
            return;
        }
        press(&mut a, KeyCode::Char('n'));
        assert!(matches!(a.modal, Some(Modal::StartAgent { .. })));
        press(&mut a, KeyCode::Down);
        assert_eq!(a.modal_choice, 1);
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::Up);
        assert_eq!(a.modal_choice, choices - 1, "the same wrap");
    }
}

/// A stand-in for `weft remote` on the daemon: it publishes a view and hears
/// the acts the TUI sends it.
struct Remote {
    out: std::os::unix::net::UnixStream,
    lines: weft_proto::Lines<std::os::unix::net::UnixStream>,
}

impl Remote {
    fn on(a: &App, view: serde_json::Value) -> Self {
        use std::io::Write;
        let socket = a.socket().to_path_buf();
        let mut out = std::os::unix::net::UnixStream::connect(&socket).expect("daemon");
        let lines = weft_proto::Lines::new(out.try_clone().unwrap());
        for (id, call) in [
            weft_proto::Call::Hello {
                client: "weft-remote/test".into(),
                protocol: weft_proto::PROTOCOL,
            },
            weft_proto::Call::RemoteView { view },
        ]
        .into_iter()
        .enumerate()
        {
            out.write_all(&weft_proto::Line::Call { id: id as u64, call }.encode()).unwrap();
        }
        Remote { out, lines }
    }

    /// The next act the TUI sent.
    fn act(&mut self) -> serde_json::Value {
        self.out.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        loop {
            match self.lines.next() {
                Ok(Some(weft_proto::Line::Event(weft_proto::Event::RemoteAct { act }))) => {
                    return act;
                }
                Ok(Some(_)) => continue,
                other => panic!("no act: {other:?}"),
            }
        }
    }
}

/// Pump until the remote view's state is `state`.
fn remote_state(a: &mut App, state: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while a.session_mut().remote.as_ref().is_none_or(|v| v["state"] != state) {
        assert!(std::time::Instant::now() < deadline, "{:?}", a.session_mut().remote);
        a.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// remote.md §7a: turning Remote on lists the tunnels and `Enter`
/// chooses one; with its program missing `I` asks "Run this?" first, `N`
/// sends nothing and `Y` sends `install`.
#[test]
fn choosing_a_tunnel_then_installing_it_asks_before_it_runs() {
    let mut a = app();
    press(&mut a, KeyCode::Char('r'));
    press(&mut a, KeyCode::Enter);
    remote_state(&mut a, "starting");
    let mut remote = Remote::on(
        &a,
        serde_json::json!({"state": "choose",
            "tunnels": [{"id": "alpha", "title": "Alpha"}, {"id": "cloudflared", "title": "Cloudflare Tunnel"}]}),
    );
    remote_state(&mut a, "choose");
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Enter);
    assert_eq!(remote.act(), serde_json::json!({"act": "choose", "tunnel": "cloudflared"}));

    let mut remote = Remote::on(
        &a,
        serde_json::json!({"state": "not_installed",
            "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"},
            "offer": {"how": "brew", "argv": "brew install cloudflared"}}),
    );
    remote_state(&mut a, "not_installed");
    press(&mut a, KeyCode::Char('i'));
    assert!(a.remote_run, "asked, not run");
    press(&mut a, KeyCode::Char('n'));
    assert!(!a.remote_run);
    // Nothing went: the next act is the look that follows.
    press(&mut a, KeyCode::Enter);
    assert_eq!(remote.act(), serde_json::json!({"act": "look"}));
    // There is one Install: D is not a second way.
    press(&mut a, KeyCode::Char('d'));
    assert!(!a.remote_run);
    press(&mut a, KeyCode::Char('i'));
    press(&mut a, KeyCode::Char('y'));
    assert_eq!(remote.act(), serde_json::json!({"act": "install"}));
    assert_eq!(a.modal, Some(Modal::Remote), "the view stays open to show it installing");
}

/// remote.md §12's keys: `Enter` turns it on or looks again, `Esc` cancels
/// signing in, `D` removes the selected device, `O` turns it off.
#[test]
fn the_remote_views_keys_do_what_its_bar_says() {
    let mut a = app();
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('r'));
    press(&mut a, KeyCode::Enter);
    remote_state(&mut a, "starting");

    let mut remote = Remote::on(
        &a,
        serde_json::json!({"state": "not_installed",
            "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"}}),
    );
    remote_state(&mut a, "not_installed");
    press(&mut a, KeyCode::Enter);
    assert_eq!(remote.act(), serde_json::json!({"act": "look"}));

    let mut remote = Remote::on(
        &a,
        serde_json::json!({"state": "signing_in", "code": "AB-CD", "url": "github.com/login/device"}),
    );
    remote_state(&mut a, "signing_in");
    press(&mut a, KeyCode::Esc);
    assert_eq!(remote.act(), serde_json::json!({"act": "cancel"}));

    let devices = serde_json::json!([
        {"id": "d_1", "name": "iPhone", "login": "octo", "last_seen": 0},
        {"id": "d_2", "name": "iPad", "login": "octo", "last_seen": 0},
    ]);
    let mut remote =
        Remote::on(&a, serde_json::json!({"state": "on", "login": "octo", "devices": devices}));
    remote_state(&mut a, "on");
    assert_eq!(a.modal, Some(Modal::Remote), "still open");
    press(&mut a, KeyCode::Down);
    // D asks first (T8.16): N keeps it, Y removes it.
    press(&mut a, KeyCode::Char('d'));
    assert_eq!(a.remote_removing.as_deref(), Some("d_2"), "asked, not removed");
    press(&mut a, KeyCode::Char('n'));
    assert_eq!(a.remote_removing, None);
    press(&mut a, KeyCode::Char('d'));
    press(&mut a, KeyCode::Char('y'));
    assert_eq!(remote.act(), serde_json::json!({"act": "remove", "device": "d_2"}));
    press(&mut a, KeyCode::Char('o'));
    assert_eq!(
        remote.act(),
        serde_json::json!({"act": "stop"}),
        "turned off: weft remote is told to stop"
    );
    remote_state(&mut a, "off");
}

/// Turn the loop until the open question is `want`.
fn modal_is(a: &mut App, want: Option<Modal>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while a.modal != want {
        assert!(std::time::Instant::now() < deadline, "the modal is {:?}", a.modal);
        let _ = a.turn(Duration::from_millis(20));
    }
}

/// remote.md §8, §12: a device asking to be allowed opens the question by
/// itself on any screen, the agent's included; `Y` and `N` answer it; a
/// request that goes closes it and puts back what was in front.
#[test]
fn a_device_asking_opens_the_question_by_itself() {
    let mut a = app();
    let asking = |device: &str| {
        serde_json::json!({"state": "on", "login": "octo", "devices": [],
                           "asking": {"device": device, "name": "iPhone", "login": "octo"}})
    };
    let allow = |device: &str| {
        Some(Modal::Allow { device: device.into(), name: "iPhone".into(), login: "octo".into() })
    };
    // Remote is on: a `weft remote` speaking while it is off is told to stop.
    press(&mut a, KeyCode::Char('r'));
    press(&mut a, KeyCode::Enter);
    remote_state(&mut a, "starting");
    press(&mut a, KeyCode::Esc);
    a.focus = Focus::Agent;
    let mut remote = Remote::on(&a, asking("d_1"));
    modal_is(&mut a, allow("d_1"));
    press(&mut a, KeyCode::Char('x'));
    assert_eq!(a.modal, allow("d_1"), "only Y and N answer it");
    press(&mut a, KeyCode::Char('y'));
    assert_eq!(remote.act(), serde_json::json!({"act": "allow", "device": "d_1", "yes": true}));
    assert_eq!(a.modal, None);

    let mut remote = Remote::on(&a, asking("d_2"));
    modal_is(&mut a, allow("d_2"));
    press(&mut a, KeyCode::Char('n'));
    assert_eq!(remote.act(), serde_json::json!({"act": "allow", "device": "d_2", "yes": false}));

    // Over the help; it expires, and the help is back.
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('h'));
    assert_eq!(a.modal, Some(Modal::Help));
    let _remote = Remote::on(&a, asking("d_3"));
    modal_is(&mut a, allow("d_3"));
    let _expired =
        Remote::on(&a, serde_json::json!({"state": "on", "login": "octo", "devices": []}));
    modal_is(&mut a, Some(Modal::Help));
}

/// A scratch home whose `systemctl` does nothing: Weft's service as a test
/// may write it, never the machine's.
fn scratch_service(name: &str) -> (PathBuf, weftd::service::Service) {
    use std::os::unix::fs::PermissionsExt as _;
    let scratch = std::env::temp_dir().join(format!("weft-app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let (home, tools) = (scratch.join("home"), scratch.join("tools"));
    std::fs::create_dir_all(&home).expect("home");
    std::fs::create_dir_all(&tools).expect("tools");
    let systemctl = tools.join("systemctl");
    std::fs::write(&systemctl, "#!/bin/sh\nexit 0\n").expect("systemctl");
    std::fs::set_permissions(&systemctl, std::fs::Permissions::from_mode(0o755)).expect("mode");
    let service =
        weftd::service::Service::new(weftd::service::System::Systemd, home, 501).with_tools(tools);
    (scratch, service)
}

/// T13.2's L: Weft's own "Start at login" turned on and off in place from
/// the Weft menu, the service written to run `weft --serve`.
#[test]
fn l_in_the_weft_menu_turns_start_at_login_on_and_off() {
    let (scratch, service) = scratch_service("login");
    let (root, session) = test_session_in("login", set_up(|w| w.with_service(service.clone())));
    let mut a = App::with_session(root, Toggle, session);
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('l'));
    assert_eq!(a.modal, Some(Modal::Weft), "the menu stays open");
    assert_eq!(WEFT_MENU[a.modal_choice].1, "Start at login", "and L is picked");
    assert!(sess!(a).start_at_login, "{:?}", a.hint_text());
    let unit = std::fs::read_to_string(service.file()).expect("the service is written");
    assert!(unit.contains("--serve"), "{unit}");
    // Enter on it turns it off again.
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.modal, Some(Modal::Weft));
    assert!(!sess!(a).start_at_login);
    assert!(!service.installed());
    let _ = std::fs::remove_dir_all(scratch);
}

/// Where Weft has no service, L says so in one line and the option stays
/// off; in the Remote view (T13.3) L asks the same.
#[test]
fn start_at_login_that_cannot_be_turned_on_says_why_and_stays_off() {
    let (root, session) = test_session("no-login");
    let mut a = App::with_session(root, Toggle, session);
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('l'));
    assert!(!sess!(a).start_at_login);
    assert_eq!(
        a.hint_text(),
        Some("Could not turn on Start at login: Weft has no login service on this system")
    );
    press(&mut a, KeyCode::Esc);
    sess!(a).remote = Some(json!({"state": "on", "login": "octo", "address": "https://a.example",
        "tunnel": "up", "note": "n", "devices": []}));
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('r'));
    press(&mut a, KeyCode::Char('l'));
    assert_eq!(a.modal, Some(Modal::Remote));
    assert!(a.hint_text().is_some_and(|h| h.starts_with("Could not turn on Start at login")));
}
