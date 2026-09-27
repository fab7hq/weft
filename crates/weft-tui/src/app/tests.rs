//! The window's tests: a real daemon on a scratch project, driven by keys.

use super::*;
use crate::ledger::Sent;
use crossterm::event::KeyCode;
use serde_json::json;
use weft_core::offers::first_line;
use weft_core::readiness::{Gap, Readiness};

/// A session backed by a real server on a scratch root, because the app is
/// a client now and there is no honest way to test it without one.
/// Whether this machine has Codex at all. Some tests are about what Weft
/// offers for a harness that exists, and there is nothing to offer when it
/// does not.
pub(crate) fn codex_on_path() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join("codex").is_file()))
}

pub(crate) fn ringframe_on_path() -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| dir.join("ringframe").is_file())
    })
}

pub(crate) fn test_session(name: &str) -> (PathBuf, crate::client::Session) {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("weft-app-{name}-{}-{n}", std::process::id()));
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
        let _ = weftd::server::Session::serve_with(
            &listening,
            &listening.with_extension("no-config.toml"),
            Some(weft_core::harness::fixture::harnesses()),
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

pub(crate) fn app() -> App {
    let (root, session) = test_session("codex");
    let mut a = App::with_session(root, Toggle, session);
    a.add("codex", "/bin/cat").expect("spawn");
    a.settle();
    // Say what this fixture's readiness is instead of inheriting the
    // machine's. Otherwise these tests pass on a laptop with Codex and the
    // plugin installed and fail everywhere else, which is not a fact about
    // the code. Tests about readiness itself set their own.
    a.set_readiness("codex", Readiness::Ready);
    a.set_readiness("claude-code", Readiness::Ready);
    agent_says(&mut a, "codex", "fixture", "ready");
    a
}

/// Now, as a receipt writes it.
fn now() -> String {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("t");
    weft_core::turns::stamp(ms.as_millis() as i64)
}

/// A hook's receipt, written where RingFrame's plugin writes it, now, and
/// waited for until the daemon has read it.
pub(crate) fn agent_says(a: &mut App, harness: &str, session: &str, event: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.session_mut().panes.is_empty() {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    let dir = a.root().join(".fab7/rf/sessions").join(harness).join(session);
    std::fs::create_dir_all(&dir).expect("session dir");
    let line = serde_json::json!({"event": event, "session_id": session,
                                  "time": now()});
    std::fs::write(dir.join("turns.jsonl"), format!("{line}\n")).expect("receipt");
    let want = weft_core::turns::Turn::recorded(event);
    while std::time::Instant::now() < deadline && a.pane_turn(0) != want {
        a.session_mut().pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn with_unit(sent: Sent) -> App {
    let mut a = app();
    a.set_units(vec![unit(sent)]);
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

/// A project whose daemon can see this work, because it is really on the
/// ledger. `set_units` puts a unit in front of the person; only the record
/// puts it where an act can reach it.
fn recorded(sent: Sent) -> App {
    let mut a = app();
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

fn ctrl(a: &mut App, c: char) {
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
    assert!(
        a.rows().iter().any(|r| matches!(r, Row::Harness { running: false, .. })),
        "its work stays, marked not running"
    );
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

#[test]
fn resuming_is_offered_only_when_the_record_names_a_session() {
    assert_eq!(pick_up_choices(None), vec![PickUp::Fresh]);
    let known = Recorded { id: "01a0bdb6".into(), at: 0, last: String::new() };
    assert_eq!(pick_up_choices(Some(&known)), vec![PickUp::Resume, PickUp::Fresh]);
}

#[test]
fn open_work_with_no_agent_offers_the_session_it_was_asked_in() {
    let mut a = app();
    let mut u = unit(Sent::TakenByAgent);
    u.harness = "claude-code".into();
    u.session_ref = Some("dfffa9be".into());
    a.set_units(vec![u]);
    a.selected = a.rows().iter().position(|r| matches!(r, Row::Action { .. })).expect("row");
    press(&mut a, KeyCode::Enter);
    let Some(Modal::PickUp { harness, session: Some(s), then: None }) = a.modal.clone() else {
        panic!("the choice, naming its own session: {:?}", a.modal)
    };
    assert_eq!((harness.as_str(), s.id.as_str()), ("claude-code", "dfffa9be"));
}

#[test]
fn a_harness_with_nothing_open_and_no_agent_is_not_listed() {
    let mut a = app();
    let mut u = unit(Sent::TakenByAgent);
    u.harness = "claude-code".into();
    u.sealed = Some("accepted".into());
    a.set_units(vec![u]);
    assert!(
        !a.rows().iter().any(|r| matches!(r, Row::Harness { name, .. } if name == "claude-code"))
    );
}

#[test]
fn the_latest_recorded_session_is_found_for_picking_up() {
    // `starts()` only offers a harness that is on this machine, so this
    // is about what Weft does when one is. A runner has no Codex.
    if !codex_on_path() {
        eprintln!("skipped: codex is not on PATH");
        return;
    }
    let mut a = app();
    let root = a.root().to_path_buf();
    let dir = root.join(".fab7/rf/sessions/codex/01a0bdb6");
    std::fs::create_dir_all(&dir).expect("dir");
    let line = serde_json::json!({
        "session_id": "01a0bdb6", "time": "2026-09-20T07:27:14.769Z",
        "prompt": "$rf:ask research crypto trading", "cwd": root.to_string_lossy(),
    });
    std::fs::write(dir.join("prompts.jsonl"), format!("{line}\n")).expect("receipt");
    a.refresh_for_test();

    let starts = a.starts();
    let codex: Vec<_> = starts.iter().filter(|c| c.harness == "codex").collect();
    assert_eq!(codex.len(), 2, "pick it up, or start fresh");
    assert_eq!(codex[0].spec, "codex resume 01a0bdb6", "picking it up comes first");
    assert!(codex[0].session.is_some());
    assert_eq!(codex[1].spec, "codex");
    assert!(codex[1].session.is_none());
}

#[test]
fn a_harness_with_no_receipt_here_is_offered_fresh_only() {
    let mut a = app();
    a.refresh_for_test();
    for c in a.starts() {
        assert!(c.session.is_none(), "nothing is on record in a new workspace");
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
    let zed = weft_core::harness::fixture::text("codex")
        .replace("title = \"Codex\"", "title = \"Zed Agent\"")
        .replace("program = \"codex\"", "program = \"cat\""); // on every machine's PATH
    std::fs::write(home.join("harnesses/zed-agent.toml"), zed).expect("its file");
    let socket = weft_proto::private_socket("weft-app-zed");
    let _ = std::fs::remove_file(&socket);
    let (listening, config) = (socket.clone(), home.join("config.toml"));
    std::thread::spawn(move || {
        let _ = weftd::server::Session::serve(&listening, &config);
    });
    let session = connect_when_listening(&socket, &root);
    let mut a = App::with_session(root, Toggle, session);
    a.look_for_agents(Opening::Nothing);
    a.settle();
    let zed: Vec<_> = a.starts().iter().filter(|s| s.harness == "zed-agent").collect();
    assert_eq!(zed.len(), 1, "offered fresh: {:?}", a.starts());
    assert_eq!(zed[0].spec, "cat");
    assert_eq!(a.harness_for_program("/bin/cat").as_deref(), Some("zed-agent"));
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
    let Some(Modal::Confirm(p)) = a.modal.clone() else {
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
    press(&mut a, KeyCode::Char('f'));
    for c in intent.chars() {
        press(&mut a, KeyCode::Char(c));
    }
    press(&mut a, KeyCode::Enter);
    let Some(Modal::Confirm(p)) = a.modal.clone() else {
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
    let Some(Modal::Confirm(p)) = a.modal.clone() else { panic!("{:?}", a.modal) };
    let text = String::from_utf8(p.payload).unwrap();
    assert!(!text.contains("[follow-up"), "got {text:?}");
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
        let mut a = recorded(Sent::TakenByAgent);
        press(&mut a, KeyCode::Char(key));
        let Some(Modal::Confirm(p)) = a.modal.clone() else {
            panic!("{key} must confirm first, got {:?}", a.modal)
        };
        let text = String::from_utf8(p.payload).unwrap();
        // Whatever words follow, the command token is closed, so Enter
        // means send rather than pick a completion.
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
    let Some(Modal::SendAnyway { why, .. }) = a.modal.clone() else {
        panic!("it must ask first, got {:?}", a.modal)
    };
    assert_eq!(why, "the agent is asking you something");
    assert_eq!(a.modal_choice, 0, "Type it anyway is the default");
    press(&mut a, KeyCode::Down);
    assert_eq!(a.modal_choice, 1, "Cancel is one key away");
    press(&mut a, KeyCode::Enter);
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
        let Some(Modal::SendAnyway { why: said, .. }) = a.modal.clone() else {
            panic!("{state:?} must ask first, got {:?}", a.modal)
        };
        assert_eq!(said, why);
        let drawn = crate::ui::panel_text(&a).join("\n");
        assert!(drawn.contains(why), "{drawn}");
        assert!(drawn.contains("Type it anyway"), "{drawn}");
        // Only an agent asking is one the person would answer instead.
        let answer = drawn.contains("Cancel — I will answer the agent");
        assert_eq!(answer, state == Some(Turn::Waiting), "{drawn}");
    }
    for state in [Turn::Ready, Turn::TurnEnded] {
        let mut a = recorded(Sent::ReadyToSend);
        a.session_mut().set_turns(vec![Some(state)]);
        a.do_inject(&pending_for(0));
        assert!(a.modal.is_none(), "{state:?} is typed into: {:?}", a.modal);
    }
}

/// §6 test 1, drawn: the row carries the agent's state from its receipt.
#[test]
fn the_row_of_an_ask_shows_its_agents_state() {
    use weft_core::turns::{Session, Turn};
    let mut a = with_unit(Sent::Arrived { exact: true });
    let before = crate::ui::sidebar_text(&a);
    assert!(!before.contains("working"), "no receipt, no state: {before}");
    a.session_mut().turns = vec![Session {
        harness: "codex".into(),
        id: "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into(),
        first: 1,
        latest: Turn::Working,
        at: 1,
    }];
    let after = crate::ui::sidebar_text(&a);
    assert!(after.contains("working · ASKED"), "{after}");
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
    assert!(!crate::ui::sidebar_text(&a).contains('●'), "{}", crate::ui::sidebar_text(&a));

    agent_says(&mut a, "codex", "fixture", "waiting");
    assert_eq!(a.needs_you(), 1);
    let rows = crate::ui::sidebar_text(&a);
    let badge = rows.lines().find(|l| l.contains("needs your input")).expect("a badge");
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
        matches!(&rows[a.selected], Row::Harness { name, .. } if name == "codex"),
        "{:?} at {}",
        rows,
        a.selected
    );
    press(&mut a, KeyCode::Down);
    assert!(matches!(a.rows()[a.selected], Row::Action { .. }), "the arrow moved on from it");
}

/// The list shows an agent the moment it is opened, before anything is
/// asked, and OPEN counts it: OPEN is the harnesses open, not the Asks.
#[test]
fn an_agent_is_on_the_list_and_counted_as_soon_as_it_is_opened() {
    let mut a = app();
    let screen = drawn(&mut a);
    assert!(screen.contains("▾ codex"), "drawn before anything is asked:\n{screen}");
    assert!(screen.contains("1 open"), "{screen}");
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
    let row = rows.lines().find(|l| l.contains("fix the login bug")).expect("the row");
    assert!(row.contains("ASKING"), "{rows}");
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
    assert!(rows.lines().any(|l| l.contains("codex") && l.contains("needs your input")), "{rows}");
}

/// ADR-0017: an agent the person is not looking at that starts asking
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
    a.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
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
    let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains("2 codex")).expect("tabs");
    let x = line.find("2 codex").expect("x") as u16;
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
    let (x, y) = cell_of(&mut a, "2 codex");
    click(&mut a, x + "2 codex".len() as u16 - 1, y);
    assert_eq!(a.pane_focus, 1, "its last cell is its own");
}

/// A send made from the detail view closes it, so the person lands in the
/// agent the keys went to rather than on a view that blocks the screen.
#[test]
fn proceeding_from_the_detail_view_closes_it_and_lands_in_the_agent() {
    // The prompt is still to be sent, so [P]ROCEED sends it.
    let mut a = recorded(Sent::ReadyToSend);
    std::fs::write(a.root().join(".fab7/rf/y"), "Ship it.\n").expect("the prompt");
    press(&mut a, KeyCode::Enter); // the row's detail view
    assert!(a.detail().is_some(), "the detail view is open");
    press(&mut a, KeyCode::Char('p'));
    assert!(matches!(a.modal, Some(Modal::Confirm(_))), "{:?} / {:?}", a.modal, a.hint_text());
    press(&mut a, KeyCode::Enter);
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert!(a.detail().is_none(), "the detail view closed");
    assert_eq!(a.focus, Focus::Agent, "and the keys are the agent's");
}

/// Sent on the person's yes, and RingFrame would not record that it went:
/// the person is told, rather than being offered the send again blind.
#[test]
fn a_send_ringframe_would_not_record_is_said() {
    if !ringframe_on_path() {
        eprintln!("skipped: ringframe is not on PATH");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let mut a = recorded(Sent::ReadyToSend);
    std::fs::write(a.root().join(".fab7/rf/y"), "Ship it.\n").expect("the prompt");
    let ledger = a.root().join(".fab7/rf/ledger.jsonl");
    std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o444)).expect("ro");
    press(&mut a, KeyCode::Char('p'));
    assert!(matches!(a.modal, Some(Modal::Confirm(_))), "{:?} / {:?}", a.modal, a.hint_text());
    press(&mut a, KeyCode::Enter);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && a.session_mut().unrecorded.is_none() {
        a.pump();
        std::thread::sleep(Duration::from_millis(20));
    }
    let said = drawn(&mut a).lines().last().unwrap_or_default().to_string();
    assert!(said.contains("RingFrame did not record it"), "{said}");
    assert!(said.contains("ledger.io"), "and why: {said}");
    std::fs::set_permissions(&ledger, std::fs::Permissions::from_mode(0o644)).expect("rw");
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

/// A harness row says which harness has an agent running in turbo mode.
#[test]
fn a_harness_running_in_turbo_mode_carries_its_badge() {
    let mut a = app();
    assert!(!crate::ui::sidebar_text(&a).contains('⚡'));
    a.session_mut().panes[0].turbo = true;
    let rows = crate::ui::sidebar_text(&a);
    assert!(rows.lines().any(|l| l.contains("codex") && l.contains('⚡')), "{rows}");
}

/// An agent started in turbo mode says so on its tab, and only it.
#[test]
fn an_agent_in_turbo_mode_says_so_on_its_tab() {
    let mut a = app();
    let tabs = |a: &mut App| drawn(a).lines().nth(1).unwrap_or("").to_string();
    assert!(!tabs(&mut a).contains('⚡'), "{}", tabs(&mut a));
    a.session_mut().panes[0].turbo = true;
    assert!(tabs(&mut a).contains("1 codex ⚡"), "{}", tabs(&mut a));
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
    let rows = crate::ui::sidebar_text(&a);
    assert!(rows.lines().any(|l| l.contains("codex") && l.contains("not running")), "{rows}");
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
    press(&mut a, KeyCode::Char('e'));
    assert!(matches!(a.modal, Some(Modal::Confirm(_))));
    press(&mut a, KeyCode::Left); // [←] CANCEL
    assert!(a.modal.is_none());
    assert_eq!(a.focus, Focus::Weft, "cancelling never moves you into the agent");
}

#[test]
fn confirming_types_it_and_puts_you_in_the_agent() {
    let mut a = recorded(Sent::TakenByAgent);
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
    assert_eq!(a.hint_text(), Some("Nothing to work on yet."));
}

#[test]
fn proceed_does_the_step_the_selected_row_is_actually_waiting_on() {
    // A confirmed Ask nobody has submitted: there is a step to take, so
    // the verb is live. What it then types is the send path's own test.
    let a = recorded(Sent::ReadyToSend);
    assert_eq!(a.unavailable(Act::Proceed), None);

    // An Ask that was compiled and never answered is waiting on the
    // harness's chooser, not on the person, so there is nothing to send.
    let mut a = recorded(Sent::NotSent);
    press(&mut a, KeyCode::Char('p'));
    assert!(a.modal.is_none(), "{:?}", a.modal);
    assert_eq!(
        a.hint_text(),
        Some("health endpoint has nothing to send. [F] FOLLOW UP asks for more.")
    );
}

#[test]
fn a_unit_whose_agent_is_not_open_is_not_silently_dropped() {
    let mut a = app();
    let mut u = unit(Sent::Arrived { exact: true });
    u.harness = "claude-code".into();
    a.set_units(vec![u]);
    press(&mut a, KeyCode::Char('e'));
    let Some(Modal::PickUp { harness, then, .. }) = a.modal.clone() else {
        panic!("the missing agent is offered, not refused: {:?}", a.modal)
    };
    assert_eq!((harness.as_str(), then), ("claude-code", Some(Act::Eval)));
}

#[test]
fn enter_unfolds_what_is_closed_and_acts_on_what_is_open() {
    let mut a = with_unit(Sent::Arrived { exact: true });
    // The selection lands on the work, so the first `[Enter]` opens it.
    assert!(matches!(a.rows()[a.selected], Row::Action { .. }));
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
    sealed.sealed = Some("accepted".into());
    a.set_units(vec![sealed]);
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
        .position(|r| matches!(r, Row::Action { project: 0, .. }))
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
    assert_eq!(a.selected_unit().map(|u| u.title.clone()), Some("health endpoint".into()));
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
    // project, harness, two actions
    assert_eq!(a.rows().len(), 4);
    a.selected = 0;
    press(&mut a, KeyCode::Down);
    assert_eq!(a.selected, 1);
    press(&mut a, KeyCode::Up);
    assert_eq!(a.selected, 0);
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

    let blocked = a.unavailable(Act::Eval).expect("codex is not set up");
    assert!(blocked.contains("codex is not set up"), "{blocked}");
    // The two units hang under different harnesses, so reaching the
    // second means walking past its harness heading.
    let claude = a
        .rows()
        .iter()
        .position(
            |r| matches!(r, Row::Action { unit, .. } if a.units()[*unit].harness == "claude-code"),
        )
        .expect("the claude-code row");
    a.selected = claude;
    assert_eq!(a.unavailable(Act::Eval), None, "claude-code is ready");
}

#[test]
fn the_ringframe_view_runs_nothing_before_proceed() {
    let mut a = app();
    press(&mut a, KeyCode::Char('u'));
    assert_eq!(a.modal, Some(Modal::RingFrame));
    // Until the daemon has said what is behind, there is nothing to run.
    press(&mut a, KeyCode::Char('p'));
    assert!(a.sync_view().is_none());
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
        let _ =
            weftd::server::Session::serve(&listening, &listening.with_extension("no-config.toml"));
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
                a.hint_text().is_some_and(|h| h.contains("No coding agent")),
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
