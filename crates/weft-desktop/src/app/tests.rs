use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use weft_core::conversation::{Item, Kind};
use weft_core::harness::{Harness, Harnesses};
use weft_core::ledger::{Check, Report, Sent, Unit};
use weft_core::onboarding::{Row as SetupRow, State as SetupState, View as SetupView};
use weft_core::sessions::{Row, State};
use weft_core::state::{
    PaneViewMode, SavedPaneTab, SavedTabGroup, SplitLayout, WeftState, WindowBounds,
};
use weft_core::turns::Turn;
use weft_core::words;
use weft_proto::{Call, Event, Line, Lines};

use super::*;
use crate::action::{
    Action, Answer, BarAct, Focus, PaneAct, Setting, SettingsTab, SheetAct, TreeAct, WindowAct,
};
use crate::gpui::{Element, palette};

/// The daemon's end of a project's link, played by the test: it reads the
/// calls the desktop writes and writes answers and events back.
pub(crate) struct Wire {
    lines: Lines<UnixStream>,
    out: UnixStream,
}

impl Wire {
    pub(crate) fn on(client: &mut ProjectClient) -> Wire {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        client.wire(ours);
        theirs.set_read_timeout(Some(Duration::from_millis(50))).expect("timeout");
        let out = theirs.try_clone().expect("a second handle");
        Wire { lines: Lines::new(theirs), out }
    }

    /// Every call written since, once the writer has had a moment.
    pub(crate) fn calls(&mut self) -> Vec<(u64, Call)> {
        let mut got = Vec::new();
        while let Ok(Some(line)) = self.lines.next() {
            if let Line::Call { id, call } = line {
                got.push((id, call));
            }
        }
        got
    }

    /// The next call the desktop writes.
    pub(crate) fn call(&mut self) -> (u64, Call) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(Some(Line::Call { id, call })) = self.lines.next() {
                return (id, call);
            }
        }
        panic!("the desktop made no call");
    }

    pub(crate) fn answer(&mut self, id: u64, result: Value) {
        self.out.write_all(&Line::Result { id, result }.encode()).expect("written");
    }

    pub(crate) fn refuse(&mut self, id: u64, message: &str) {
        let line = Line::Error { id, code: "refused".into(), message: message.into() };
        self.out.write_all(&line.encode()).expect("written");
    }

    /// A refusal with the daemon's own code.
    pub(crate) fn refuse_as(&mut self, id: u64, code: &str, message: &str) {
        let line = Line::Error { id, code: code.into(), message: message.into() };
        self.out.write_all(&line.encode()).expect("written");
    }

    pub(crate) fn event(&mut self, event: Event) {
        self.out.write_all(&Line::Event(event).encode()).expect("written");
    }

    /// The daemon goes.
    pub(crate) fn hang_up(self) {
        let _ = self.out.shutdown(std::net::Shutdown::Both);
    }
}

/// Take what the daemon sent until `done` holds.
pub(crate) fn pumped(app: &mut WeftDesktopApp, done: impl Fn(&WeftDesktopApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        app.pump();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "the desktop never took what the daemon sent");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Start an agent of `harness` from the `+` and answer as the daemon does:
/// the pane added, then its id. Its pane.
pub(crate) fn start(app: &mut WeftDesktopApp, wire: &mut Wire, harness: &str) -> u32 {
    let root = app.clients[0].root.clone();
    app.dispatch(TreeAct::StartAgent { project: root, harness: harness.into() });
    let (id, call) = wire.call();
    let Call::StartAgent { harness: h, spec, .. } = call else { panic!("{call:?}") };
    let pane = app.clients[0].panes.iter().map(|p| p.id + 1).max().unwrap_or(0);
    wire.event(Event::Added { pane, harness: h, spec, turbo: false });
    wire.answer(id, json!({"pane": pane}));
    pumped(app, |a| a.active_tab().is_some_and(|t| t.pane == pane));
    pane
}

fn sample_harnesses() -> Harnesses {
    let mk = |name: &str, title: &str, prog: &str| Harness {
        name: name.to_string(),
        title: title.to_string(),
        program: prog.to_string(),
        args: Vec::new(),
        config_env: None,
        config_default: format!(".{name}"),
        list: vec!["list".into()],
        add_marketplace: None,
        install_plugin: None,
        update_marketplace: None,
        update_plugin: None,
        listing: None,
        manifest: None,
        resume: vec!["--resume".into()],
        transcript: None,
        turbo: vec!["--turbo".into()],
        unset_env: Vec::new(),
        signal: None,
        listen: None,
        resume_joins: false,
        server_args: Vec::new(),
        window_title: None,
        conversation: None,
        eval: None,
    };
    // Codex says what a judge there takes, as its harness file does.
    let codex = Harness {
        eval: weft_core::harness::fixture::harnesses().find("codex").and_then(|h| h.eval.clone()),
        ..mk("codex", "Codex", "codex")
    };
    Harnesses(vec![
        mk("antigravity", "Antigravity", "agy"),
        mk("claude", "Claude Code", "claude"),
        codex,
        mk("grok", "Grok Build", "grok"),
    ])
}

fn populate_sample_project(c: &mut ProjectClient) {
    c.harnesses = sample_harnesses();
    let u1 = Unit {
        ask_id: "ask_1".into(),
        title: "fix login".into(),
        harness: "antigravity".into(),
        confirmed: true,
        sent: Sent::TakenByAgent,
        ..Default::default()
    };
    let u2 = Unit {
        ask_id: "ask_2".into(),
        title: "telemetry".into(),
        harness: "claude".into(),
        confirmed: false,
        sent: Sent::TakenByAgent,
        requested_only: true,
        ..Default::default()
    };
    let check = Check { eval_id: "evl_3".into(), report: Report::default(), judged_by: None };
    let u3 = Unit {
        ask_id: "ask_3".into(),
        title: "health endpoint".into(),
        harness: "codex".into(),
        confirmed: true,
        sent: Sent::TakenByAgent,
        check: Some(check.clone()),
        ..Default::default()
    };
    let u4 = Unit {
        ask_id: "ask_4".into(),
        title: "build number".into(),
        harness: "codex".into(),
        confirmed: true,
        sent: Sent::TakenByAgent,
        check: Some(Check { eval_id: "evl_2".into(), ..check }),
        sealed: Some("accepted".into()),
        seal_id: Some("sel_4".into()),
        ..Default::default()
    };
    c.units = vec![u1, u2, u3, u4];
    for (u, minute) in c.units.iter_mut().zip([29, 20, 10, 5]) {
        u.asked_at = format!("2026-10-08T11:{minute:02}:00Z");
    }

    let mut r1 = Row::of("antigravity", "s1", State::Running, Some(Turn::Waiting), 1124);
    r1.title = Some("fix login".into());
    r1.pane = Some(0);
    let mut r2 = Row::of("codex", "s2", State::Running, Some(Turn::Working), 1131);
    r2.pane = Some(1);
    let mut r3 = Row::of("claude", "s3", State::Running, Some(Turn::TurnEnded), 1052);
    r3.title = Some("health endpoint".into());
    r3.pane = Some(2);
    let mut r4 = Row::of("claude", "s4", State::Ended, None, 912);
    r4.title = Some("readme fix".into());
    c.sessions = vec![r1, r2, r3, r4];
    // Earlier ones the daemon counts, and the first page of them read.
    c.earlier = 14;
    c.earlier_rows.rows = (0..14)
        .map(|i| Row::of("claude", &format!("old_{i}"), State::Ended, None, 800 - i))
        .collect();
    c.earlier_rows.total = 14;

    // health endpoint judged by Eval 3; build number sealed by Seal 4 on Eval 2.
    let record = weft_core::work::record(&[
        json!({"type": "eval.opened", "id": "evl_2", "time": "2026-10-08T11:06:00Z",
               "data": {"basis": {"asks": ["ask_4"]}}}),
        json!({"type": "eval.completed", "id": "evl_2", "time": "2026-10-08T11:07:00Z",
               "data": {"basis": {"asks": ["ask_4"]}, "report": {"faults": 0}}}),
        json!({"type": "seal.created", "id": "sel_4", "time": "2026-10-08T11:08:00Z",
               "actor": {"kind": "human"}, "links": [{"rel": "seals", "id": "evl_2"}],
               "data": {"disposition": "accepted"}}),
        json!({"type": "eval.opened", "id": "evl_3", "time": "2026-10-08T11:11:00Z",
               "data": {"basis": {"asks": ["ask_3"]}}}),
        json!({"type": "eval.completed", "id": "evl_3", "time": "2026-10-08T11:12:00Z",
               "data": {"basis": {"asks": ["ask_3"]}, "report": {"faults": 0}}}),
    ]);
    c.work = weft_core::work::work(&c.units, &record);
}

fn home_with(state: WeftState) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    save_state_to_home(tmp.path(), &state).unwrap();
    tmp
}

pub(crate) fn sample_app(turbo: bool) -> (tempfile::TempDir, WeftDesktopApp) {
    let tmp = home_with(WeftState {
        open_projects: vec![PathBuf::from("/work/weft")],
        ..Default::default()
    });
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    app.clients[0].turbo = turbo;
    app.clients[0].config = Some(serde_json::json!({"machine": {"turbo": turbo}}));
    populate_sample_project(&mut app.clients[0]);
    (tmp, app)
}

/// The sample app, linked to a wire the test answers on.
pub(crate) fn wired_app() -> (tempfile::TempDir, WeftDesktopApp, Wire) {
    let (tmp, mut app) = sample_app(false);
    // Typing pauses are the one test's to wait out.
    app.ask_pause = Duration::from_millis(20);
    let wire = Wire::on(&mut app.clients[0]);
    (tmp, app, wire)
}

fn seal(id: &str, n: usize, eval: Option<&str>) -> weft_core::work::Seal {
    weft_core::work::Seal {
        id: id.into(),
        n,
        at: String::new(),
        disposition: "accepted".into(),
        by: "you".into(),
        eval: eval.map(str::to_string),
        reason: None,
    }
}

/// What the window writes, written: the state as `state.json` now holds it.
fn saved(app: &mut WeftDesktopApp) -> WeftState {
    if let Some((home, state)) = app.unsaved_state() {
        save_state_to_home(&home, &state).unwrap();
    }
    load_state_from_home(&app.home)
}

fn weft() -> PathBuf {
    PathBuf::from("/work/weft")
}

/// Every element of a tree, depth first.
fn every(el: &Element) -> Vec<&Element> {
    let mut out = vec![el];
    for c in &el.children {
        out.extend(every(c));
    }
    out
}

fn style<'a>(el: &'a Element, k: &str) -> Option<&'a str> {
    el.styles.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str())
}

fn item(id: &str, kind: Kind, at: Option<i64>, text: Option<&str>) -> Item {
    Item {
        id: id.into(),
        kind,
        at,
        text: text.map(str::to_string),
        tool: None,
        arg: None,
        call: None,
        failed: None,
    }
}

#[test]
fn restores_window_bounds_and_open_projects_from_durable_state_json() {
    let mut initial = WeftState {
        open_projects: vec![PathBuf::from("/work/weft"), PathBuf::from("/work/docs")],
        folded_projects: vec![PathBuf::from("/work/docs")],
        ..Default::default()
    };
    initial.settings.sidebar_open = true;
    initial.settings.window_bounds =
        Some(WindowBounds { x: 120.0, y: 80.0, width: 1440.0, height: 900.0 });
    let tmp = home_with(initial);
    let home = tmp.path().to_path_buf();

    let mut app = WeftDesktopApp::new(home.clone(), "studio");
    assert_eq!(app.clients.len(), 2);
    assert_eq!(app.clients[0].name, "weft");
    assert_eq!(app.clients[1].name, "docs");
    assert_eq!(
        app.state.settings.window_bounds,
        Some(WindowBounds { x: 120.0, y: 80.0, width: 1440.0, height: 900.0 })
    );
    let projects = app.tree_projects();
    assert!(!projects[0].project.folded);
    assert!(projects[1].project.folded, "docs was folded in state.json");

    app.dispatch(TreeAct::CloseProject(PathBuf::from("/work/docs")));
    app.dispatch(BarAct::ConfirmCloseProject(PathBuf::from("/work/docs")));
    assert_eq!(saved(&mut app).open_projects, vec![weft()]);
}

#[test]
fn renders_header_and_collapsible_tree() {
    let (_tmp, mut app) = sample_app(true);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s1".into() });
    let root = app.render();
    assert!(root.find_layer("the list's toggle").is_some());
    // Turbo is on the agents' titles and Updates in ☰, never in the bar.
    assert!(root.find_layer("turbo pill").is_none());
    assert!(root.find_layer("updates pill").is_none());
    assert_eq!(
        root.find_layer("needs you tag").expect("needs you tag").all_text(),
        "■ NEEDS YOU 1 ›"
    );

    let tree = root.find_layer("tree").expect("tree open");
    for layer in [
        "search",
        "row weft",
        "section AGENTS",
        "row agent antigravity fix login",
        "row earlier sessions",
        "section WORK",
        "row unit fix login",
        "row unit telemetry",
        "row unit health endpoint",
        "row unit build number",
    ] {
        assert!(tree.find_layer(layer).is_some(), "{layer}");
    }

    app.search.set_text("login");
    let searched = app.render();
    let stree = searched.find_layer("tree").unwrap();
    assert!(stree.all_text().contains("2 match"));
    assert!(stree.find_layer("row unit fix login").is_some());
    assert!(stree.find_layer("row unit telemetry").is_none());
    app.dispatch(TreeAct::ClearSearch);
    assert!(app.search.text().is_empty());

    app.dispatch(WindowAct::ToggleSidebar);
    assert!(!app.state.settings.sidebar_open);
    let collapsed = app.render();
    assert!(collapsed.find_layer("tree").is_none());
    let toggle = collapsed.find_layer("the list's toggle").unwrap();
    assert_eq!(toggle.aria_label.as_deref(), Some("Show the list"));
    assert!(!saved(&mut app).settings.sidebar_open);
}

#[test]
fn an_agent_starts_from_the_plus_and_its_tab_opens_when_the_daemon_gives_its_pane() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    app.dispatch(TreeAct::TogglePlus(weft()));
    assert_eq!(app.plus_popover_project.as_deref(), Some(weft().as_path()));
    let root = app.render();
    let pop = root.find_layer("plus popover").expect("plus popover shown");
    assert!(pop.all_text().contains("NEW AGENT IN WEFT"));
    for h in ["antigravity", "claude", "codex", "grok"] {
        assert!(pop.find_layer(&format!("plus harness {h}")).is_some(), "{h}");
    }

    app.dispatch(TreeAct::StartAgent { project: weft(), harness: "antigravity".into() });
    assert!(app.plus_popover_project.is_none(), "popover closes on start");
    let (id, call) = wire.call();
    assert!(matches!(call, Call::StartAgent { ref harness, .. } if harness == "antigravity"));
    assert!(app.state.panes.groups.is_empty(), "no tab before the daemon says which pane");
    wire.answer(id, json!({"pane": 7}));
    pumped(&mut app, |a| !a.state.panes.groups.is_empty());

    let saved = saved(&mut app);
    assert_eq!(saved.panes.groups[0].tabs[0].pane, 7);
    assert_eq!(saved.panes.groups[0].tabs[0].project, weft());
}

#[test]
fn splits_up_to_four_panes_and_restores_layout_across_restarts() {
    let (tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    for h in ["claude", "codex", "grok", "antigravity"] {
        start(&mut app, &mut wire, h);
    }
    assert_eq!(app.state.panes.groups.len(), 1);
    assert_eq!(app.state.panes.groups[0].tabs.len(), 4);

    app.dispatch(PaneAct::ToggleLayoutMenu(0));
    assert!(app.render().find_layer("layout menu").is_some());
    app.dispatch(PaneAct::SplitRight(0));
    assert_eq!(app.state.panes.layout, SplitLayout::TwoColumns);
    app.dispatch(PaneAct::SplitDown(0));
    assert_eq!(app.state.panes.layout, SplitLayout::ThreeRight);
    app.dispatch(PaneAct::SplitRight(0));
    assert_eq!(app.state.panes.layout, SplitLayout::FourGrid);
    app.dispatch(PaneAct::SplitRight(0));
    assert_eq!(app.state.panes.groups.len(), 4);
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: words::word("web.why.four_panes").to_string() }
    );

    saved(&mut app);
    let restarted = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    assert_eq!(restarted.state.panes.layout, SplitLayout::FourGrid);

    app.dispatch(PaneAct::JoinAll);
    assert_eq!(app.state.panes.groups.len(), 1);
    assert_eq!(app.state.panes.groups[0].tabs.len(), 4);
    assert_eq!(app.state.panes.layout, SplitLayout::Single);
}

#[test]
fn the_conversation_is_read_through_the_daemon_and_drawn_with_its_waiting_line() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "antigravity");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Waiting);

    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, call) = wire.call();
    assert_eq!(
        call,
        Call::Read {
            what: "conversation".into(),
            unit: String::new(),
            pane: Some(pane),
            lines: None,
            before: None,
            after: None,
            q: None,
        }
    );
    let mut step = item("20.0", Kind::Step, Some(1121), None);
    step.tool = Some("Read".into());
    step.arg = Some("src/login.rs".into());
    step.failed = Some(false);
    let items = vec![
        item("0.0", Kind::Person, Some(1118), Some("fix login")),
        item("10.0", Kind::Agent, None, Some("I'll update src/login.rs.\nThen run the tests.")),
        step,
        item("50.0", Kind::Agent, Some(1124), Some("Should a failed refresh retry once?")),
    ];
    wire.answer(id, json!({"conversation": {"items": items}}));
    pumped(&mut app, |a| a.conversations.contains_key(&(weft(), pane)));

    let dom = app.render();
    assert!(dom.find_layer("seg Conversation selected").is_some());
    assert!(dom.find_layer("msg you").is_some());
    let first = dom.find_layer("msg antigravity").expect("its first reply");
    assert!(first.all_text().contains("I'll update src/login.rs.\nThen run the tests."));
    assert!(!first.all_text().contains("11:20"), "no time where none was given");
    assert!(dom.find_layer("tool steps folded").is_some());
    let waiting = dom.find_layer("waiting line").expect("waiting line");
    assert!(waiting.all_text().contains("antigravity is waiting for you —"));

    app.dispatch(PaneAct::ToggleSteps { project: weft(), pane, block: 0 });
    assert!(app.render().find_layer("step Read").is_some());
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Terminal });
    assert!(app.render().find_layer("seg Terminal selected").is_some());
}

#[test]
fn a_restored_split_keeps_each_agent_in_its_own_pane() {
    let tab = |pane| SavedPaneTab { project: weft(), pane, view: PaneViewMode::Terminal };
    let mut initial = WeftState { open_projects: vec![weft()], ..Default::default() };
    initial.panes.layout = SplitLayout::TwoColumns;
    initial.panes.groups = vec![
        SavedTabGroup { active_tab: 0, tabs: vec![tab(0)] },
        SavedTabGroup { active_tab: 0, tabs: vec![tab(1)] },
    ];
    let tmp = home_with(initial.clone());
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    let mut client = ProjectClient::disconnected(weft(), "weft");
    client.panes = vec![
        PaneView::new(0, "sh".into(), "sh".into()),
        PaneView::new(1, "sh".into(), "sh".into()),
    ];
    app.integrate_connected_client(client);
    assert_eq!(app.state.panes.groups, initial.panes.groups, "nothing shown twice, nothing moved");
}

#[test]
fn a_judge_pane_is_listed_under_its_eval_and_never_as_an_agent() {
    let tmp = home_with(WeftState { open_projects: vec![weft()], ..Default::default() });
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    let mut client = ProjectClient::disconnected(weft(), "weft");
    client.notify = true;
    let judge = |id, eval: &str| {
        let mut p = PaneView::new(id, "codex".into(), "codex -m gpt-6".into());
        (p.judge, p.turn) = (Some(eval.into()), Some(Turn::Waiting));
        p
    };
    client.panes =
        vec![PaneView::new(0, "sh".into(), "sh".into()), judge(1, "evl_1"), judge(2, "evl_2")];
    app.integrate_connected_client(client);
    let tabs: Vec<u32> =
        app.state.panes.groups.iter().flat_map(|g| &g.tabs).map(|t| t.pane).collect();
    assert_eq!(tabs, [0], "a judge opens no tab of its own");
    let under = |eval| app.clients[0].judges(eval).iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!((under("evl_1"), under("evl_2")), (vec![1], vec![2]));
    app.notice_waiting();
    assert!(app.notices.is_empty(), "a judge waiting is not the person's agent waiting");
}

#[test]
fn four_panes_each_tell_the_daemon_their_own_size_once() {
    let (_tmp, mut app, mut wire) = wired_app();
    let sizes = [(1, 22, 97), (2, 22, 69), (3, 17, 97), (4, 17, 69)];
    for (pane, rows, cols) in sizes {
        app.resize_pane(&weft(), pane, rows, cols);
        app.resize_pane(&weft(), pane, rows, cols);
    }
    let told: Vec<_> = wire
        .calls()
        .into_iter()
        .filter_map(|(_, c)| match c {
            Call::Resize { pane, rows, cols } => Some((pane, rows, cols)),
            _ => None,
        })
        .collect();
    assert_eq!(told, sizes, "one each, and never twice for the same size");
}

#[test]
fn an_ask_goes_to_the_daemon_and_its_staged_send_is_answered() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);

    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::Ask { .. })));
    assert_eq!(app.focus, Focus::Ask);
    app.ask.set_text("refresh token 60s before expiry");
    app.dispatch(SheetAct::SubmitAsk);
    let call = wire
        .calls()
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| matches!(c, Call::Act { .. }))
        .expect("the Ask");
    assert_eq!(
        call,
        Call::Act {
            act: "ask".into(),
            unit: None,
            pane: Some(pane),
            text: Some("refresh token 60s before expiry".into()),
            eval: None,
            start: None,
            draft: None,
        }
    );
    // Nothing waits on the person until the daemon stages it.
    assert!(app.render().find_layer("Cancel / Send bar").is_none());
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "Ask Codex?".into(),
        why: "This is the exact prompt Codex will receive.".into(),
        payload: b"Refresh the token.\n\nRules:\n- KISS: keep it plain".to_vec(),
        sends: None,
        go: None,
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    let bar = app.render();
    assert!(bar.find_layer("btn Send").is_some());
    // The dialog shows its title, why and the exact payload, never a
    // "Weft will type <title>".
    let said = bar.find_layer("Cancel / Send bar").unwrap().all_text();
    for line in [
        "Ask Codex?",
        "the exact prompt Codex will receive",
        "Refresh the token.",
        "- KISS: keep it plain",
    ] {
        assert!(said.contains(line), "{line} in {said}");
    }
    assert!(!said.contains("Weft will type"), "{said}");
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::Resolve { pending: "p1".into(), yes: true, force: false, to: None, start: None }
    );

    // The agent is working: Weft says it has not said it is ready.
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Working);
    let bar = app.render();
    let bar = bar.find_layer("Cancel / Send bar").expect("still waiting until resolved");
    assert!(bar.all_text().contains("Weft did not type it: the agent is working."));
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Anyway });
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Resolve { force: true, yes: true, .. }));
    wire.event(Event::Resolved { id: "p1".into(), yes: true });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());

    // A refusal is said in the bar, and stays until the person acts.
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Act { ref act, .. } if act == "eval"));
    wire.refuse(id, "no agent can run it");
    pumped(&mut app, |a| a.failed.as_deref() == Some("no agent can run it"));
    assert!(!app.expire_why_not(Instant::now() + Duration::from_secs(60)));
    let bar = app.render();
    assert_eq!(bar.find_layer("failed sentence").unwrap().all_text(), "no agent can run it");
}

/// With Sense (17.22, 17.23): while the words are read the sheet shows one
/// panel; when no route was picked for them, the person picks one before Ask
/// sends, and only the proposed practice is kept.
#[test]
fn the_ask_sheet_shows_the_words_read_and_waits_for_a_route_sense_left_open() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    let rule =
        |id: &str| json!({"id": id, "label": id, "text": "", "family": null, "layer": "shipped"});
    let sheet = json!({
        "routes": ["native_plan", "native_goal"], "route": null, "tasks": ["plan"], "task": "plan",
        "plan": null, "plans": [], "route_rules": [rule("route.step_loop")],
        "practices": [rule("practice.kiss"), rule("practice.yagni"), rule("practice.gall"), rule("practice.knuth")],
        "proposed": ["practice.gall"], "senses": true,
    });
    // Each read is answered as it comes; the words change once one is shown.
    let answer_reads = |app: &mut WeftDesktopApp,
                        wire: &mut Wire,
                        until: &dyn Fn(&WeftDesktopApp) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut seen = Vec::new();
        while !until(app) {
            assert!(Instant::now() < deadline, "the sheet never came");
            app.pump();
            for (id, c) in wire.calls() {
                if matches!(c, Call::Read { ref what, .. } if what == "ask") && !seen.contains(&id)
                {
                    wire.answer(id, sheet.clone());
                    seen.push(id);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    answer_reads(&mut app, &mut wire, &|a| a.ask_sheet.is_some());
    app.ask.set_text("the health check says the build is unknown");
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    assert!(drawn.contains("Sensing your best fit…"), "{drawn}");
    for hidden in ["SEND TO", "ROUTE", "PRACTICES", "Pick a route yourself"] {
        assert!(!drawn.contains(hidden), "one panel while the words are read: {drawn}");
    }
    answer_reads(&mut app, &mut wire, &|a| {
        a.ask_sheet_for.as_deref() == Some("the health check says the build is unknown")
    });
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    for said in [
        "Pick a route",
        "They show once you pick a route.",
        "PRACTICES · 1",
        "practice.gall",
        "+ 3 more",
        "Pick a route to ask.",
    ] {
        assert!(drawn.contains(said), "{said} in {drawn}");
    }
    app.dispatch(SheetAct::SubmitAsk);
    assert_eq!(app.bottom_bar, BottomBarState::WhyNot { sentence: "Pick a route to ask.".into() });
    assert!(
        !wire
            .calls()
            .into_iter()
            .any(|(_, c)| matches!(c, Call::Act { ref act, .. } if act == "ask")),
        "nothing asked"
    );
}

/// The words are read once the person stops typing: until then the reading
/// panel stands where SEND TO, the route and the rules will be, and nothing
/// is asked of Sense.
#[test]
fn the_words_are_read_once_the_person_stops_typing() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    let sheet = json!({
        "routes": ["native_direct"], "route": "native_direct", "tasks": ["implement"],
        "task": "implement", "plan": null, "plans": [], "route_rules": [], "practices": [],
        "proposed": [], "senses": true,
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.ask_sheet.is_none() {
        assert!(Instant::now() < deadline, "the sheet never came");
        app.pump();
        for (id, c) in wire.calls() {
            if matches!(c, Call::Read { ref what, .. } if what == "ask") {
                wire.answer(id, sheet.clone());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    app.ask_pause = ASK_PAUSE;
    let reads = |wire: &mut Wire| {
        std::thread::sleep(Duration::from_millis(50));
        let calls = wire.calls().into_iter();
        calls
            .filter_map(|(_, c)| match c {
                Call::Read { what, unit, .. } if what == "ask" => Some(unit),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let t0 = Instant::now();
    app.ask.set_text("fix the");
    app.want_ask_sheet_at(t0);
    app.ask.set_text("fix the login");
    app.want_ask_sheet_at(t0 + Duration::from_millis(1000));
    app.want_ask_sheet_at(t0 + Duration::from_millis(2000));
    assert_eq!(reads(&mut wire), Vec::<String>::new(), "nothing is read while the person types");
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    assert!(drawn.contains("Sensing your best fit…"), "{drawn}");
    assert!(!drawn.contains("SEND TO") && !drawn.contains("ROUTE"), "{drawn}");
    app.want_ask_sheet_at(t0 + Duration::from_millis(2600));
    let read = reads(&mut wire);
    assert!(read.len() == 1 && read[0].contains("fix the login"), "{read:?}");
}

/// Sense's heads come first (17.1 with 17.23 where the rules go): SEND TO,
/// the route and its rules show while the plan and the practices are read,
/// and Ask waits for them. An answer for words the person changed is never
/// shown.
#[test]
fn the_sheet_shows_the_route_at_senses_heads_and_the_rules_once_read() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    let rule =
        |id: &str| json!({"id": id, "label": id, "text": "", "family": null, "layer": "shipped"});
    let sheet = |stage: &str, route: &str| {
        json!({
            "routes": ["native_plan", "native_goal"], "route": route, "tasks": ["plan"],
            "task": "plan", "plan": null, "plans": [], "route_rules": [rule("route.step_loop")],
            "practices": [rule("practice.kiss")], "proposed": if stage == "full" { json!(["practice.kiss"]) } else { json!(null) },
            "senses": true, "stage": stage,
        })
    };
    app.ask_pause = Duration::ZERO;
    // The read of `stage` for `words`, once the desktop asks for it.
    let read = |app: &mut WeftDesktopApp, wire: &mut Wire, words: &str, stage: &str| {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            assert!(Instant::now() < deadline, "no {stage} read of {words:?}");
            app.want_ask_sheet_at(Instant::now());
            app.pump();
            let found = wire.calls().into_iter().find_map(|(id, c)| match c {
                Call::Read { what, unit, pane: to, .. } if what == "ask" => {
                    let q: Value = serde_json::from_str(&unit).unwrap();
                    // For the agent it goes to: the plans of its folder.
                    assert_eq!(to, Some(pane), "the read names the agent's pane");
                    (q["words"] == words && q["stage"] == stage).then_some(id)
                }
                _ => None,
            });
            if let Some(id) = found {
                return id;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    app.ask.set_text("plan the export");
    let heads = read(&mut app, &mut wire, "plan the export", "heads");
    wire.answer(heads, sheet("heads", "native_plan"));
    let full = read(&mut app, &mut wire, "plan the export", "full");
    assert!(app.ask_sheet.as_ref().is_some_and(|s| s["stage"] == "heads"));
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    for said in
        ["SEND TO", "ROUTE", "Plan", "ROUTE RULES", "route.step_loop", "Sensing your best fit…"]
    {
        assert!(drawn.contains(said), "{said} in {drawn}");
    }
    for later in ["PLAN", "PRACTICES"] {
        assert!(!drawn.contains(later), "{later} waits for the rules: {drawn}");
    }
    app.dispatch(SheetAct::SubmitAsk);
    let asked = |wire: &mut Wire| {
        wire.calls()
            .into_iter()
            .any(|(_, c)| matches!(c, Call::Act { ref act, .. } if act == "ask"))
    };
    assert!(!asked(&mut wire), "Ask waits for the rules");

    // The words change while the full read runs: its answer is never shown.
    app.ask.set_text("plan the import");
    let _ = read(&mut app, &mut wire, "plan the import", "heads");
    wire.answer(full, sheet("full", "native_goal"));
    for _ in 0..20 {
        app.want_ask_sheet_at(Instant::now());
        app.pump();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_ne!(app.ask_sheet.as_ref().unwrap()["route"], "native_goal", "the old words' answer");
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    assert!(drawn.contains("Sensing your best fit…"), "{drawn}");
}

/// 7.1 and 17.24 are one warning: a harness Weft reads as not set up for
/// RingFrame stops no act. The Ask sheet opens and says so in amber; Eval
/// goes, and its dialog says so too. The reading may be wrong.
#[test]
fn a_harness_read_as_not_set_up_warns_in_the_dialog_and_never_stops_the_act() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    let gap = "no_plugin";
    app.clients[0].readiness = json!({"claude": gap, "codex": gap, "antigravity": gap});
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::Ask { .. })), "{:?}", app.bottom_bar);
    let warning = "antigravity is not set up for RingFrame. Updates… in ☰ sets it up.";
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    assert!(drawn.contains(warning), "{drawn}");
    app.dispatch(SheetAct::Close);

    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    let eval = wire
        .calls()
        .into_iter()
        .any(|(_, c)| matches!(c, Call::Act { ref act, .. } if act == "eval"));
    assert!(eval, "Eval goes: {:?}", app.bottom_bar);
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "Eval this work?".into(),
        why: String::new(),
        payload: Vec::new(),
        sends: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    let said = app.render().find_layer("Cancel / Send bar").unwrap().all_text();
    assert!(said.contains("is not set up for RingFrame. Updates… in ☰ sets it up."), "{said}");
}

/// The Eval dialog's staged for `harness`: the daemon's dialog `id`, named
/// in its answer to the act.
pub(crate) fn eval_staged(
    app: &mut WeftDesktopApp,
    wire: &mut Wire,
    id: &str,
    start: Option<&str>,
) {
    let call = loop {
        match wire.call() {
            (call, Call::Act { act, start: s, .. }) if act == "eval" => break (call, s),
            _ => continue,
        }
    };
    assert_eq!(call.1.as_deref(), start, "read by");
    wire.event(Event::Pending {
        id: id.into(),
        pane: u32::MAX,
        what: words::word("dialog.eval.title").into(),
        why: format!(
            "fix login · 1 Ask\n{} reads the whole change.",
            start.unwrap_or("Antigravity")
        ),
        payload: Vec::new(),
        sends: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
    });
    wire.answer(call.0, json!({"pending": id}));
    let id = id.to_string();
    pumped(app, |a| a.eval_pick.as_ref().and_then(|p| p.pending.as_ref()) == Some(&id));
}

/// The Eval dialog's Read by starts on who would read it, saying why; picking
/// another stages the dialog again for it, the one shown going once the new
/// one is there, and nothing runs until Eval.
#[test]
fn the_eval_dialog_picks_who_reads_it_and_runs_only_on_eval() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    eval_staged(&mut app, &mut wire, "p1", None);
    let dom = app.render();
    let row = dom.find_layer("read by").expect("Read by");
    assert_eq!(row.find_layer("read by choice").unwrap().all_text(), "Antigravity ▾");
    assert!(row.all_text().contains("the work’s latest agent"), "{}", row.all_text());

    app.dispatch(BarAct::ToggleReadBy);
    let dom = app.render();
    let list = dom.find_layer("read by list").expect("every harness set up here");
    for h in ["antigravity", "claude", "codex", "grok"] {
        assert!(list.find_layer(&format!("read by {h}")).is_some(), "{h}");
    }
    assert!(list.find_layer("read by antigravity").unwrap().all_text().contains('✓'));

    app.dispatch(BarAct::ReadBy("codex".into()));
    let dom = app.render();
    let row = dom.find_layer("read by").expect("still the dialog shown");
    assert_eq!(row.find_layer("read by choice").unwrap().all_text(), "Codex ▾", "kept");
    assert!(!row.all_text().contains("latest agent"), "not the one it started on");
    eval_staged(&mut app, &mut wire, "p2", Some("codex"));
    // The dialog it replaced is answered no, once the new one is there.
    let no = wire.calls().into_iter().any(
        |(_, c)| matches!(c, Call::Resolve { ref pending, yes: false, .. } if pending == "p1"),
    );
    assert!(no, "the first dialog goes");
    wire.event(Event::Resolved { id: "p1".into(), yes: false });
    pumped(&mut app, |a| a.clients[0].waiting.len() == 1);
    let dom = app.render();
    assert_eq!(dom.find_layer("read by choice").unwrap().all_text(), "Codex ▾");
    let said = dom.find_layer("Cancel / Send bar").unwrap().all_text();
    assert!(said.contains("codex reads the whole change."), "the daemon's sentence: {said}");
    let ran = wire.calls().into_iter().any(|(_, c)| matches!(c, Call::Resolve { yes: true, .. }));
    assert!(!ran, "nothing runs on a pick");

    app.dispatch(BarAct::Resolve { project: weft(), pending: "p2".into(), answer: Answer::Yes });
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Resolve { ref pending, yes: true, .. } if pending == "p2"));
    assert!(app.eval_pick.is_none(), "its pick goes with it");
}

/// Read by set in Settings says so; a pick the daemon refuses leaves the
/// dialog shown and its pick, and says why.
#[test]
fn the_eval_dialog_says_a_reader_set_in_settings_and_keeps_its_pick_when_refused() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].routing = weft_core::routing::Routing::of(
        json!({"eval": "claude"}),
        Value::Null,
        Value::Null,
        Value::Null,
        Value::Null,
    );
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    eval_staged(&mut app, &mut wire, "p1", None);
    let row = app.render().find_layer("read by").unwrap().clone();
    assert_eq!(row.find_layer("read by choice").unwrap().all_text(), "Claude Code ▾");
    assert!(row.all_text().contains("set in Settings"), "{}", row.all_text());

    app.dispatch(BarAct::ReadBy("grok".into()));
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Act { ref start, .. } if start.as_deref() == Some("grok")));
    wire.refuse(id, "grok is not set up here");
    pumped(&mut app, |a| a.eval_pick.as_ref().is_some_and(|p| p.pending.is_some()));
    let dom = app.render();
    assert_eq!(dom.find_layer("read by choice").unwrap().all_text(), "Claude Code ▾");
    let said = dom.find_layer("Cancel / Send bar").unwrap().all_text();
    assert!(said.contains("grok is not set up here"), "{said}");
    let gone = wire.calls().into_iter().any(|(_, c)| matches!(c, Call::Resolve { .. }));
    assert!(!gone, "the dialog shown stays");
}

/// 17.24: RingFrame refuses the sheet's read for the agent in SEND TO. The
/// sheet warns with RingFrame's reason, and Ask stays on: a wrong reading
/// never holds the person up.
#[test]
fn a_refused_ask_sheet_warns_with_the_reason_and_ask_still_sends() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    app.ask.set_text("show the build number");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut refused = Vec::new();
    while app.ask_refused.is_none() {
        assert!(Instant::now() < deadline, "the refusal is never kept");
        app.pump();
        for (id, c) in wire.calls() {
            if matches!(c, Call::Read { ref what, .. } if what == "ask") && !refused.contains(&id) {
                wire.refuse(id, "Codex is not set up for RingFrame.");
                refused.push(id);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    let said = "This agent may not take an Ask: Codex is not set up for RingFrame.";
    assert!(drawn.contains(said), "{said} in {drawn}");
    app.dispatch(SheetAct::SubmitAsk);
    assert!(
        wire.calls()
            .into_iter()
            .any(|(_, c)| matches!(c, Call::Act { ref act, .. } if act == "ask")),
        "asked anyway"
    );
}

/// The Ask's draft (17.1): the daemon offers the route, plan and rules for the
/// harness and the words; the person picks a route, takes a practice out, and
/// Ask sends what they kept.
#[test]
fn the_ask_sheet_offers_a_route_a_plan_and_rules_and_sends_what_was_kept() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    app.ask.set_text("show the build number");
    let rule = |id: &str, label: &str| json!({"id": id, "label": label, "text": format!("{label} text"), "family": null, "layer": "shipped"});
    let sheet = |route: &str| {
        json!({
            "routes": ["native_plan", "native_goal"], "route": route, "tasks": ["plan", "implement"],
            "task": if route == "native_plan" { "plan" } else { "implement" },
            "plan": {"slug": "build", "path": "plans/build/plan.md", "title": "Build"},
            "plans": [{"slug": "build", "path": "plans/build/plan.md", "title": "Build"}],
            "practices": [rule("practice.kiss", "KISS"), rule("practice.yagni", "YAGNI"), rule("practice.gall", "Gall"), rule("practice.knuth", "Knuth")],
            "route_rules": [rule("route.step_loop", "Step Loop")],
        })
    };
    // The sheet is read for the harness and the words, and drawn when it
    // comes: a read for the words before is answered, and never drawn.
    let answered = |app: &mut WeftDesktopApp, wire: &mut Wire, route: &str| {
        let mut answered_before = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        let (id, query) = loop {
            assert!(Instant::now() < deadline, "the sheet for the words is never read");
            app.pump();
            let read = wire.calls().into_iter().rev().find_map(|(id, c)| match c {
                Call::Read { what, unit, .. } if what == "ask" => Some((id, unit)),
                _ => None,
            });
            let Some((id, query)) = read.filter(|(id, _)| !answered_before.contains(id)) else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            let query: Value = serde_json::from_str(&query).unwrap();
            if query["words"] == "show the build number" {
                break (id, query);
            }
            wire.answer(id, sheet(route));
            answered_before.push(id);
        };
        assert_eq!(query["words"], "show the build number");
        wire.answer(id, sheet(route));
        pumped(app, |a| a.ask_sheet.as_ref().is_some_and(|s| s["route"] == route));
    };
    answered(&mut app, &mut wire, "native_plan");
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    for said in [
        "ROUTE",
        "Plan",
        "PLAN",
        "plans/build/plan.md",
        "ROUTE RULES",
        "Step Loop",
        "PRACTICES · 4",
        "KISS",
        "+ 1 more",
    ] {
        assert!(drawn.contains(said), "{said} in {drawn}");
    }
    app.dispatch(SheetAct::AskRoute("native_goal".into()));
    answered(&mut app, &mut wire, "native_goal");
    app.dispatch(SheetAct::AskRuleOut("practice.yagni".into()));
    let drawn = app.render().find_layer("sheet Ask").unwrap().all_text();
    assert!(drawn.contains("PRACTICES · 3") && !drawn.contains("YAGNI"), "{drawn}");
    app.dispatch(SheetAct::SubmitAsk);
    let draft = wire.calls().into_iter().find_map(|(_, c)| match c {
        Call::Act { act, draft, .. } if act == "ask" => draft,
        _ => None,
    });
    assert_eq!(
        draft,
        Some(weft_core::ask::Draft {
            route: Some("native_goal".into()),
            task: Some("implement".into()),
            plan: Some("plans/build/plan.md".into()),
            rules: Some(vec![
                "practice.kiss".into(),
                "practice.gall".into(),
                "practice.knuth".into()
            ]),
            ..Default::default()
        })
    );
}

/// WORK is a row per unit of work with one status; a unit opens as its
/// workflow, and its Eval and Seal name it.
#[test]
fn a_unit_of_work_opens_as_its_workflow_and_its_acts_name_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    let tree = app.render().find_layer("tree").unwrap().all_text();
    for said in ["asked", "asking", "evaluated", "1 Ask since your last Seal", "sealed · accepted"]
    {
        assert!(tree.contains(said), "{said} in {tree}");
    }
    assert!(!tree.contains("Open") && !tree.contains("Eval 2"), "no rounds: {tree}");

    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_3".into() });
    let dom = app.render();
    let sheet = dom.find_layer("sheet Unit").expect("the unit's Details");
    let text = sheet.all_text();
    for said in ["health endpoint", "1 Ask · codex · since", "WORKFLOW", "EVAL 2", "not sealed yet"]
    {
        assert!(text.contains(said), "{said} in {text}");
    }
    assert!(sheet.find_layer("step 3").is_some(), "Ask, Eval, not sealed");
    assert!(dom.find_layer("tree bar").unwrap().all_text().contains("selected: health endpoint"));

    // A follow-up's Ask names the work it follows and the id it carries.
    app.dispatch(SheetAct::FollowUp { project: weft(), unit: "ask_3".into() });
    let sheet = app.render().find_layer("sheet Ask").expect("the Ask").all_text();
    assert!(sheet.contains("FOLLOW UP · HEALTH ENDPOINT · "), "the eyebrow: {sheet}");
    // It goes to the work's own harness (codex), not the agent in front.
    let to = app.render().find_layer("send to choice").expect("its SEND TO").all_text();
    assert!(to.contains("codex"), "to the work's harness: {to}");
    app.active_sheet = None;

    // Seal decides on the unit's Eval; Eval names the unit.
    super::seal_tests::no_questions(&mut app, &mut wire);
    assert!(matches!(
        &app.bottom_bar,
        BottomBarState::SealDisposition { eval, .. } if eval == "evl_3"
    ));
    let dialog = app.render().find_layer("Seal disposition bar").expect("the Seal").all_text();
    for said in ["Seal this work?", "health endpoint", "DECISION", "Nothing is typed."] {
        assert!(dialog.contains(said), "{said} in {dialog}");
    }
    // Its one dialog sends the disposition and the reason picked there.
    let (project, eval) = (weft(), "evl_3".to_string());
    app.dispatch(BarAct::PickDisposition {
        project: project.clone(),
        eval: eval.clone(),
        disposition: "rejected".into(),
    });
    assert!(app.render().find_layer("reason not_what_i_asked").is_some(), "a reject's reasons");
    app.dispatch(BarAct::PickReason {
        project: project.clone(),
        eval: eval.clone(),
        disposition: "rejected".into(),
        reason: Some("not_what_i_asked".into()),
    });
    app.dispatch(BarAct::ConfirmSeal {
        project,
        eval,
        disposition: "rejected".into(),
        reason: Some("not_what_i_asked".into()),
    });
    let sealed = wire.calls().into_iter().rev().find_map(|(_, c)| match c {
        Call::Act { act, text, draft, .. } if act == "seal" => Some((text, draft)),
        _ => None,
    });
    let (text, draft) = sealed.expect("a Seal");
    assert_eq!(text.as_deref(), Some("rejected"));
    assert_eq!(draft.and_then(|d| d.reason).as_deref(), Some("not_what_i_asked"));
    app.bottom_bar = BottomBarState::None;
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    let (_, call) =
        wire.calls().into_iter().find(|(_, c)| matches!(c, Call::Act { .. })).expect("an act");
    assert!(matches!(call, Call::Act { ref act, ref unit, .. }
        if act == "eval" && unit.as_deref() == Some("ask_1")));
    // A sealed unit says why it cannot be sealed again.
    app.dispatch(BarAct::Cancel);
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_4".into() });
    app.dispatch(BarAct::Seal);
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: "build number is closed. Follow up asks again.".into() }
    );
    // Folded away, it says so all the same.
    app.dispatch(TreeAct::ToggleProject(weft()));
    app.bottom_bar = BottomBarState::None;
    app.dispatch(SheetAct::SealWork { project: weft(), unit: "ask_4".into() });
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: "build number is closed. Follow up asks again.".into() }
    );
}

/// The last act the desktop asked of the daemon, past any reads.
fn acted(wire: &mut Wire) -> Call {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some((_, call)) =
            wire.calls().into_iter().rev().find(|(_, c)| matches!(c, Call::Act { .. }))
        {
            return call;
        }
        assert!(Instant::now() < deadline, "no act was asked");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Accepted work offers nothing after its Seal: no Keep a rule, no Commit.
#[test]
fn accepted_work_offers_nothing() {
    let (_tmp, mut app, _wire) = wired_app();
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_4".into() });
    let unit = app.render().find_layer("sheet Unit").expect("the unit").all_text();
    for gone in ["AFTER THE SEAL", "Keep a rule", "Commit", "Undo"] {
        assert!(!unit.contains(gone), "no {gone}: {unit}");
    }
    // Nor the acts of open work: sealed, it is done with.
    let sheet = app.render().find_layer("sheet Unit").unwrap().clone();
    for gone in ["btn Follow up", "btn Eval", "btn Seal"] {
        assert!(sheet.find_layer(gone).is_none(), "no {gone} on accepted work");
    }
    assert!(sheet.find_layer("btn Close").is_some());
}

/// Undo is one dialog: it reads the commits from where the work started up
/// to now, newest first, that one picked already; the person may pick a
/// later one, and Undo sends the one picked.
#[test]
fn undo_puts_the_work_back_to_the_commit_picked() {
    use weft_core::after_seal::Offer;
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::Offer { project: weft(), unit: "ask_4".into(), offer: Offer::Undo });
    let (id, call) = wire.call();
    assert!(
        matches!(&call, Call::Read { what, unit, .. } if what == "commits" && unit == "ask_4"),
        "{call:?}"
    );
    let commit = |id: &str, subject: &str, start: bool| json!({"id": format!("{id}ffff"), "short": id, "subject": subject, "when": "11:02", "start": start});
    wire.answer(
        id,
        json!({"commits": [
            commit("e81b7c2", "Return 503 while the database is down", false),
            commit("7ab3d10", "Add a health endpoint", false),
            commit("4f2a9c1", "Tidy the router", true),
        ]}),
    );
    pumped(&mut app, |a| matches!(a.bottom_bar, BottomBarState::Undo { .. }));
    let bar = app.render().find_layer("Undo bar").expect("its dialog").all_text();
    for said in
        ["Undo this work?", "PUT BACK TO", "3 commits", "Tidy the router", "where it started"]
    {
        assert!(bar.contains(said), "{said} in {bar}");
    }
    assert!(!bar.contains("puts your files back"), "no sentence: {bar}");
    // The list takes the dialog's width, and its acts sit at the right (17.17).
    let dom = app.render();
    let stretched = |layer: &str| {
        dom.find_layer(layer)
            .unwrap_or_else(|| panic!("{layer}"))
            .styles
            .iter()
            .any(|(k, v)| k == "align-self" && v == "stretch")
    };
    assert!(stretched("undo list") && stretched("undo acts"), "both stretch across the dialog");
    assert!(matches!(app.bottom_bar, BottomBarState::Undo { picked: 2, .. }), "where it started");

    app.dispatch(BarAct::PickCommit(1));
    app.dispatch(BarAct::Undo);
    let call = acted(&mut wire);
    assert!(
        matches!(&call, Call::Act { act, unit, text, .. }
            if act == "undo" && unit.as_deref() == Some("ask_4") && text.as_deref() == Some("7ab3d10ffff")),
        "{call:?}"
    );
    assert_eq!(app.bottom_bar, BottomBarState::None, "Undo closes it");
}

/// A project's Git note of each Seal is off until its own settings turn it
/// on; turning it on asks once, showing what a note holds; off asks nothing.
#[test]
fn a_projects_git_note_is_turned_on_in_one_dialog() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.active_sheet = Some(ActiveSheet::ProjectSettings { project: weft() });
    let sheet = app.render().find_layer("sheet Project settings").expect("its settings").all_text();
    assert!(sheet.contains("RECORD EACH SEAL IN GIT") && sheet.contains("Git note"), "{sheet}");
    app.dispatch(SheetAct::GitNote { project: weft(), on: true });
    let bar = app.render().find_layer("Git note bar").expect("its one dialog").all_text();
    assert!(bar.contains("Record Seals as Git notes?") && bar.contains("Decision:"), "{bar}");
    app.dispatch(BarAct::TurnOnGitNote);
    let set = |call: Call| match call {
        Call::ConfigSet { scope, key, value } => Some((scope, key, value)),
        _ => None,
    };
    let key = vec!["export".to_string(), "git_note".to_string()];
    let calls: Vec<_> = wire.calls().into_iter().filter_map(|(_, c)| set(c)).collect();
    assert_eq!(calls, [("project".to_string(), key.clone(), Some(json!(true)))]);
    assert_eq!(app.bottom_bar, BottomBarState::None);
    app.dispatch(SheetAct::GitNote { project: weft(), on: false });
    let calls: Vec<_> = wire.calls().into_iter().filter_map(|(_, c)| set(c)).collect();
    assert_eq!(calls, [("project".to_string(), key, None)], "off asks nothing");
}

/// "n sealed earlier ›" turns the sidebar into Done: every sealed unit, the
/// search narrowing it by its words, a row opening its unit on the right, and
/// ‹ going back to the tree.
#[test]
fn sealed_work_is_found_in_done_in_the_sidebar() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(TreeAct::OpenDone(weft()));
    let side = |app: &mut WeftDesktopApp| {
        app.render().find_layer("sidebar Done").map(|l| l.all_text()).unwrap_or_default()
    };
    let shown = side(&mut app);
    for said in ["‹ weft", "Done", "SEALED", "build number", "accepted · nothing found"] {
        assert!(shown.contains(said), "{said} in {shown}");
    }
    assert!(!shown.contains("health endpoint"), "open work is not done: {shown}");
    app.search.set_text("rejected");
    assert!(!side(&mut app).contains("build number"), "narrowed by its decision");
    app.search.set_text("accepted");
    assert!(side(&mut app).contains("build number"), "found by its decision");
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_4".into() });
    assert!(matches!(&app.active_sheet, Some(ActiveSheet::Unit { unit, .. }) if unit == "ask_4"));
    assert!(side(&mut app).contains("build number"), "Done stays in the sidebar");
    app.dispatch(TreeAct::LeaveDone);
    assert!(app.render().find_layer("sidebar Done").is_none(), "back to the tree");
    assert!(app.search.text().is_empty());
}

/// A step row opens its stage, and the stage leads back to its unit; so
/// does the Eval view opened from it.
#[test]
fn a_units_stage_opens_from_its_row_and_leads_back_to_it() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_3".into() });
    let dom = app.render();
    let step = dom.find_layer("step 1").expect("the Ask's step");
    assert!(step.action.is_some(), "the whole row opens it");
    assert!(dom.find_layer("step 2").and_then(|s| s.action.clone()).is_some(), "the Eval's too");
    app.dispatch(TreeAct::SelectAsk { project: weft(), unit: "ask_3".into() });
    let back = app.render().find_layer("sheet back").expect("a way back").all_text();
    assert_eq!(back, "‹ health endpoint");
    app.dispatch(SheetAct::Back);
    assert!(matches!(&app.active_sheet, Some(ActiveSheet::Unit { unit, .. }) if unit == "ask_3"));
    assert!(app.render().find_layer("sheet back").is_none(), "the unit is where it began");

    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    assert!(app.render().all_text().contains("‹ health endpoint"));
    app.dispatch(SheetAct::EvalBack);
    assert!(matches!(&app.active_sheet, Some(ActiveSheet::Unit { .. })));
    // Chosen from the tree, a unit starts again: nothing to go back to.
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() });
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    assert!(app.sheet_back.is_empty());
}

/// A tab is an agent of its project: once the daemon has said which panes a
/// project has, a tab of a pane it does not have (one from before the daemon
/// started again) goes, and the project's own agents stay.
#[test]
fn a_tab_of_a_pane_the_daemon_does_not_have_goes() {
    let (_tmp, mut app) = sample_app(false);
    app.state.panes.groups.clear();
    app.clients[0].panes = vec![PaneView::new(1, "claude".into(), "claude".into())];
    for pane in [1, 7] {
        app.ensure_tab_in_active_pane(SavedPaneTab {
            project: weft(),
            pane,
            view: PaneViewMode::Terminal,
        });
    }
    app.drop_gone_tabs();
    assert_eq!(app.state.panes.groups[0].tabs.len(), 2, "kept until the daemon has said");
    app.clients[0].opened = true;
    app.drop_gone_tabs();
    let panes: Vec<u32> = app.state.panes.groups[0].tabs.iter().map(|t| t.pane).collect();
    assert_eq!(panes, [1]);
}

/// What the app keeps per agent, its conversation and draft, and
/// the pick of where a staged send goes, leave with the agent or its project.
#[test]
fn what_is_kept_for_a_gone_agent_or_a_closed_project_goes() {
    let (_tmp, mut app) = sample_app(false);
    app.clients[0].panes = vec![PaneView::new(1, "claude".into(), "claude".into())];
    app.clients[0].opened = true;
    let staged = |id: &str| crate::daemon::Staged {
        id: id.into(),
        pane: 1,
        what: String::new(),
        why: String::new(),
        payload: Vec::new(),
        sends: None,
        go: None,
    };
    app.clients[0].waiting = vec![staged("pending_1")];
    let elsewhere = PathBuf::from("/work/closed");
    for key in [(weft(), 1), (weft(), 7), (elsewhere.clone(), 1)] {
        app.conversations.insert(key.clone(), Vec::new());
        app.messages.insert(key, TextField::new(Shape::Line));
    }
    app.staged_to.insert("pending_1".into(), "2".into());
    app.staged_to.insert("pending_gone".into(), "2".into());
    app.drop_gone_tabs();
    let kept = [(weft(), 1)];
    assert_eq!(app.conversations.keys().cloned().collect::<Vec<_>>(), kept);
    assert_eq!(app.messages.keys().cloned().collect::<Vec<_>>(), kept);
    assert_eq!(app.staged_to.keys().collect::<Vec<_>>(), ["pending_1"]);
    app.close_project(&weft());
    assert!(app.conversations.is_empty() && app.messages.is_empty());
    assert!(app.staged_to.is_empty());
}

/// Resume is in a session's Details only: an ended row selected opens them,
/// Resume their act, and the row carries none.
#[test]
fn resume_is_in_a_sessions_details_not_on_its_row() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(TreeAct::SelectAgent { project: weft(), session: "s4".into(), pane: None });
    let dom = app.render();
    let row = dom.find_layer("row agent claude readme fix").expect("the ended row");
    assert!(!row.all_text().contains(words::word("act.resume")), "{}", row.all_text());
    let sheet = dom.find_layer("sheet SessionDetail").expect("its Details");
    assert!(sheet.find_layer("btn Resume").is_some());
}

/// Why an act did not run is said for a while: the person's next act takes
/// it away, and so does time, so a line about how things were never stays.
#[test]
fn a_why_not_goes_with_the_next_act_or_after_a_while() {
    let (_tmp, mut app) = sample_app(false);
    app.state.panes.groups.clear();
    app.dispatch(PaneAct::SplitRight(0));
    let said = BottomBarState::WhyNot { sentence: words::word("why.split_needs_two").into() };
    assert_eq!(app.bottom_bar, said);
    let shown = std::time::Instant::now();
    app.expire_why_not(shown);
    app.expire_why_not(shown + std::time::Duration::from_secs(3));
    assert_eq!(app.bottom_bar, said, "still there a moment later");
    app.expire_why_not(shown + std::time::Duration::from_secs(7));
    assert_eq!(app.bottom_bar, BottomBarState::None, "gone after a while");

    app.dispatch(PaneAct::SplitRight(0));
    assert_eq!(app.bottom_bar, said);
    app.dispatch(WindowAct::ToggleSidebar);
    assert_eq!(app.bottom_bar, BottomBarState::None, "the next act takes it away");
}

#[test]
fn proceed_shows_only_where_ringframe_says_a_prompt_waits() {
    let (_tmp, mut app) = sample_app(false);
    // ask_2 is not confirmed, and is only requested: nothing is ready to send.
    app.dispatch(TreeAct::SelectAsk { project: weft(), unit: "ask_2".into() });
    let dom = app.render();
    assert!(dom.find_layer("sheet WorkDetail").is_some());
    assert!(dom.find_layer("btn Proceed").is_none());
    assert!(dom.find_layer("btn Follow up").is_some());
    // One whose prompt waits on the person's yes.
    app.clients[0].units[1].requested_only = false;
    app.clients[0].units[1].sent = Sent::ReadyToSend;
    assert!(app.render().find_layer("btn Proceed").is_some());
}

#[test]
fn what_was_asked_is_read_through_the_daemon_and_nothing_is_shown_until_it_is() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectAsk { project: weft(), unit: "ask_1".into() });
    let reads: Vec<_> = (0..2)
        .map(|_| wire.call())
        .map(|(id, c)| match c {
            Call::Read { what, unit, .. } => (id, what, unit),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(reads[0].1, "source");
    assert_eq!(reads[1].1, "wording");
    let before = app.render().find_layer("sheet WorkDetail").unwrap().clone();
    assert!(!before.all_text().contains("fix the login flow"), "nothing typed before it is read");
    assert!(before.find_layer("loading").is_some(), "lines breathe while it is read");
    wire.answer(reads[0].0, json!({"text": "fix the login flow"}));
    wire.answer(reads[1].0, json!({"text": "Fix login: the token refresh."}));
    pumped(&mut app, |a| a.clients[0].read.len() == 2);
    let after = app.render().find_layer("sheet WorkDetail").unwrap().all_text();
    assert!(after.contains("fix the login flow"));
    assert!(after.contains("The prompt antigravity composed:"));
    assert!(after.contains("Fix login: the token refresh."));
}

#[test]
fn a_seals_receipt_is_checked_through_the_daemon_and_not_claimed_before() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].work.record.seals = vec![seal("sel_4", 4, Some("evl_2"))];
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() });
    let (id, call) = wire.call();
    assert!(
        matches!(call, Call::Read { ref what, ref unit, .. } if what == "seal" && unit == "sel_4")
    );
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().all_text();
    assert!(sheet.contains(words::word("wait.checking_receipt")), "{sheet}");
    assert!(!sheet.contains(words::word("web.seal.holds")));
    wire.answer(id, json!({"seal_id": "sel_4", "codes": ["receipt_hash_mismatch"]}));
    pumped(&mut app, |a| !a.clients[0].read.is_empty());
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().all_text();
    assert!(sheet.contains(words::word("web.seal.broken")), "{sheet}");
    assert!(!sheet.contains(words::word("web.seal.holds")));
}

#[test]
fn an_eval_shows_no_report_the_record_does_not_give() {
    let (_tmp, mut app) = sample_app(false);
    let e = app.clients[0].work.record.evals.iter_mut().find(|e| e.id == "evl_3").unwrap();
    e.report = None;
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_3".into() });
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().all_text();
    assert!(!sheet.contains("nothing found") && !sheet.contains("aligned"), "{sheet}");
    assert!(!sheet.contains("1.0"), "{sheet}");
    // An Eval the record does not hold says so, with no number made up.
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_9".into() });
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().all_text();
    assert!(sheet.contains("evl_9") && !sheet.contains("Eval 1"), "{sheet}");
    // The Seal of an Eval the record does not name opens no made-up Eval view.
    app.clients[0].work.record.seals = vec![seal("sel_9", 9, None)];
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_9".into() });
    let dom = app.render();
    assert!(dom.find_layer("btn Eval view").is_none());
    assert!(!dom.all_text().contains("evl_1"));
}

#[test]
fn updates_with_no_project_shows_what_weft_s_own_link_was_told() {
    let tmp = home_with(WeftState::default());
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    assert!(app.clients.is_empty());
    assert_eq!(app.wants_own_link(), Some(true), "made, starting the daemon");
    app.dispatch(WindowAct::OpenUpdates);
    let asking = app.render().find_layer("sheet Updates").unwrap().all_text();
    assert!(asking.contains(words::word("desktop.updates.asking")), "{asking}");

    let mut own = ProjectClient::disconnected(PathBuf::new(), String::new());
    own.setup = Some(SetupView {
        rows: vec![SetupRow {
            name: "claude".into(),
            title: "Claude Code".into(),
            program: "claude".into(),
            state: SetupState::NotSetUp,
            version: Some("2.1.316".into()),
        }],
        ..Default::default()
    });
    app.integrate_own_link(own);
    let upd = app.render();
    let row = upd.find_layer("update row Claude Code").unwrap().all_text();
    assert!(row.contains("2.1.316") || row.contains(&SetupState::NotSetUp.words()), "{row}");
    assert!(upd.find_layer("btn Set up claude").is_some());
    // This one is not connected: it is made again, without starting a daemon
    // the person may have stopped.
    assert_eq!(app.wants_own_link(), Some(false));
}

#[test]
fn updates_says_each_state_with_cores_own_mark_and_words() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(WindowAct::OpenUpdates);
    let asking = app.render().find_layer("sheet Updates").unwrap().all_text();
    assert!(asking.contains(words::word("desktop.updates.asking")), "no view yet: {asking}");
    assert!(!asking.contains("Ready"), "{asking}");

    let row = |name: &str, state| SetupRow {
        name: name.into(),
        title: name.into(),
        program: name.into(),
        state,
        version: None,
    };
    app.clients[0].setup = Some(SetupView {
        weft: "0.4.3 → 0.4.4".into(),
        weft_latest: Some("0.4.4".into()),
        weft_behind: true,
        configuration: "0.4.4".into(),
        latest: Some("0.4.4".into()),
        sense: "0.1.0 → 0.1.1".into(),
        sense_latest: Some("0.1.1".into()),
        sense_behind: true,
        rows: vec![
            row("claude", SetupState::Behind { have: "0.4.1".into(), latest: "0.4.4".into() }),
            row("codex", SetupState::NotFound),
            row("grok", SetupState::CannotInstall),
            row("agy", SetupState::Unknown),
        ],
        ..Default::default()
    });
    let dom = app.render();
    let upd = dom.find_layer("sheet Updates").unwrap();
    // The daemon's line once, never the latest added again.
    let weft_row = upd.find_layer("update row Weft").unwrap().all_text();
    assert!(weft_row.contains("0.4.3 → 0.4.4 · update available"), "{weft_row}");
    assert!(!weft_row.contains("0.4.4 → 0.4.4"), "{weft_row}");
    // Sense's row says where it stands, as Weft's does.
    let sense_row = upd.find_layer("update row Sense").unwrap().all_text();
    assert!(sense_row.contains("0.1.0 → 0.1.1 · update available"), "{sense_row}");
    assert!(upd.find_layer("btn Find app… codex").is_some());
    assert!(upd.find_layer("btn Update all").is_some());
    // Weft is updated the way it was installed: its row says so, with no act.
    assert!(upd.find_layer("btn Update Weft").is_none());
    assert!(upd.find_layer("btn Update RingFrame").is_none(), "RingFrame is current");
    for (name, state) in [("grok", SetupState::CannotInstall), ("agy", SetupState::Unknown)] {
        let r = upd.find_layer(&format!("update row {name}")).unwrap().all_text();
        assert!(r.starts_with(state.mark()), "{r}");
        assert!(r.contains(&state.words()), "{r}");
        assert!(!r.contains("Ready"), "{r}");
    }
    for forbidden in ["sha256:", "/Users/", ".onnx", "weft-sense"] {
        assert!(!upd.all_text().contains(forbidden), "{forbidden}");
    }
}

#[test]
fn the_allow_bar_names_the_login_weft_remote_gave_and_no_other() {
    let (_tmp, mut app) = sample_app(false);
    app.bottom_bar = BottomBarState::AllowDevice {
        device_id: "d_1".into(),
        device_name: "iPad".into(),
        login: Some("octo".into()),
    };
    let bar = app.render().find_layer("Allow a device bar").unwrap().all_text();
    assert!(bar.contains("Allow \"iPad\" to connect to studio?"));
    assert!(bar.contains("Signed in as octo"));
    assert!(!bar.contains("0xnairb"));
    app.bottom_bar = BottomBarState::AllowDevice {
        device_id: "d_1".into(),
        device_name: "iPad".into(),
        login: None,
    };
    let bar = app.render().find_layer("Allow a device bar").unwrap().all_text();
    assert!(!bar.contains("Signed in as"), "{bar}");
}

#[test]
fn remote_lists_only_what_weft_remote_said_and_changes_nothing_itself() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].remote = Some(json!({
        "state": "on",
        "devices": [{"id": "d_1"}, {"id": "d_2", "name": "Mac · Chrome", "login": "octo"}]
    }));
    app.dispatch(WindowAct::OpenRemote);
    let dom = app.render();
    let rem = dom.find_layer("sheet Remote").unwrap();
    // A device that gave no name is shown by its id; nothing says it is online.
    assert!(rem.find_layer("device row d_1").is_some());
    let text = rem.all_text();
    for invented in ["iPhone", "dev-1", "Connected now", "Last seen recently"] {
        assert!(!text.contains(invented), "{invented}");
    }
    app.dispatch(SheetAct::AskRemoveDevice { device: "d_2".into(), name: "Mac · Chrome".into() });
    assert!(app.render().find_layer("Remove device bar").is_some());
    app.dispatch(SheetAct::RemoveDevice("d_2".into()));
    let (_, call) = wire.call();
    assert_eq!(call, Call::Remote { act: json!({"act": "remove", "device": "d_2"}) });
    assert_eq!(
        app.clients[0].remote.as_ref().unwrap()["devices"].as_array().unwrap().len(),
        2,
        "the list changes when weft remote says it did"
    );
    app.dispatch(SheetAct::RemoteOff);
    let (_, call) = wire.call();
    assert_eq!(call, Call::Remote { act: json!({"act": "off"}) });
    assert_eq!(app.clients[0].remote.as_ref().unwrap()["state"], "on");
    app.dispatch(BarAct::AllowDevice { device: "d_3".into(), yes: true });
    let (_, call) = wire.call();
    assert_eq!(call, Call::Remote { act: json!({"act": "allow", "device": "d_3", "yes": true}) });
}

/// Every popover is drawn over the backdrop that closes them, so a click on
/// one is its own: a layout menu in a pane as much as ☰.
#[test]
fn a_popover_is_drawn_over_the_backdrop_that_closes_it() {
    let (_tmp, mut app) = sample_app(false);
    app.clients[0].panes.push(PaneView::new(0, "claude".into(), "claude".into()));
    app.ensure_tab_in_active_pane(SavedPaneTab {
        project: weft(),
        pane: 0,
        view: PaneViewMode::Terminal,
    });
    fn order(e: &crate::gpui::Element, out: &mut Vec<String>) {
        if let Some(l) = &e.layer_name {
            out.push(l.clone());
        }
        for c in &e.children {
            order(c, out);
        }
    }
    for (open, popover) in [
        (Action::from(PaneAct::ToggleLayoutMenu(0)), "layout menu"),
        (WindowAct::ToggleMenu.into(), "menu popover"),
        (TreeAct::TogglePlus(weft()).into(), "plus popover"),
    ] {
        app.dispatch(open.clone());
        let mut drawn = Vec::new();
        order(&app.render(), &mut drawn);
        let at = |l: &str| drawn.iter().position(|d| d == l);
        let (backdrop, menu) = (at("popover backdrop"), at(popover));
        assert!(backdrop.is_some() && menu.is_some(), "{popover}: {drawn:?}");
        assert!(backdrop < menu, "{popover} is under the backdrop");
        app.dispatch(WindowAct::ClosePopovers);
    }
}

#[test]
fn the_menu_and_quit() {
    let (_tmp, mut app) = sample_app(true);
    app.clients[0].panes.push(PaneView::new(0, "claude".into(), "claude".into()));
    app.dispatch(WindowAct::ToggleMenu);
    let dom = app.render();
    assert!(dom.find_layer("popover backdrop").is_some());
    let menu = dom.find_layer("menu popover").expect("menu popover");
    for layer in [
        "menu Open project",
        "menu Turbo",
        "menu Start at login",
        "menu Remote",
        "menu Updates",
        "menu Quit Weft",
    ] {
        assert!(menu.find_layer(layer).is_some(), "{layer}");
    }
    assert!(
        menu.find_layer("menu Remote")
            .unwrap()
            .all_text()
            .contains(words::word("web.link.connecting")),
        "no daemon has said anything yet"
    );
    app.dispatch(WindowAct::ClosePopovers);
    assert!(!app.menu_open);
    app.dispatch(WindowAct::RequestQuit);
    assert_eq!(app.bottom_bar, BottomBarState::QuitConfirm);
    let quit = app.render();
    let bar = quit.find_layer("Quit Weft bar").expect("Quit Weft bar");
    assert!(bar.find_layer("btn Stop agents and quit").is_some());
    app.dispatch(BarAct::LeaveRunningAndQuit);
    assert!(app.quit_requested);
}

#[test]
fn opening_a_project_lists_it_at_once_for_the_window_to_connect() {
    let (_tmp, mut app) = sample_app(false);
    app.open_project_path(PathBuf::from("/work/new-repo"));
    assert!(app.state.open_projects.contains(&PathBuf::from("/work/new-repo")));
    assert!(app.disconnected().contains(&(PathBuf::from("/work/new-repo"), false)));
    assert!(saved(&mut app).open_projects.contains(&PathBuf::from("/work/new-repo")));
}

#[test]
fn nothing_is_asked_of_a_daemon_not_connected_and_the_bar_says_so() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(TreeAct::StartAgent { project: weft(), harness: "claude".into() });
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: words::word("desktop.not_connected").into() }
    );
    assert!(app.clients[0].panes.is_empty(), "no pane made up");
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    assert!(app.clients[0].waiting.is_empty(), "no send staged without the daemon");
}

#[test]
fn a_lost_daemon_is_noticed_and_the_project_is_connected_again() {
    let (_tmp, mut app, wire) = wired_app();
    assert!(app.disconnected().is_empty());
    wire.hang_up();
    pumped(&mut app, |a| !a.clients[0].is_connected());
    assert_eq!(app.disconnected(), vec![(weft(), true)], "the window's thread connects it again");
}

#[test]
fn answers_are_taken_once_and_none_is_kept() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::StartAgent { project: weft(), harness: "claude".into() });
    let (id, _) = wire.call();
    // An answer nothing waits for is dropped.
    wire.answer(id + 1000, json!({"pane": 3}));
    wire.answer(id, json!({"pane": 3}));
    pumped(&mut app, |a| a.active_tab().is_some_and(|t| t.pane == 3));
    assert!(!app.clients[0].owes_any(|_| true), "nothing is owed once answered");
}

#[test]
fn two_projects_of_the_same_folder_name_stay_two() {
    let tmp = home_with(WeftState {
        open_projects: vec![PathBuf::from("/a/app"), PathBuf::from("/b/app:x")],
        ..Default::default()
    });
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    let mut wire = Wire::on(&mut app.clients[1]);
    for c in &mut app.clients {
        c.harnesses = sample_harnesses();
    }
    app.clients[1].name = "app".into();
    let listed = app.tree_projects();
    assert_eq!(listed.len(), 2);
    assert_ne!(listed[0].root, listed[1].root);
    // A path with a colon in it reaches its own project.
    app.dispatch(TreeAct::StartAgent {
        project: PathBuf::from("/b/app:x"),
        harness: "codex".into(),
    });
    let (_, call) = wire.call();
    assert!(matches!(call, Call::StartAgent { ref harness, .. } if harness == "codex"));
    app.dispatch(TreeAct::ToggleProject(PathBuf::from("/a/app")));
    let listed = app.tree_projects();
    assert!(listed[0].project.folded && !listed[1].project.folded);
}

#[test]
fn harness_files_are_the_daemons_and_never_read_from_disk_to_draw() {
    let (tmp, mut app) = sample_app(false);
    let dir = tmp.path().join(".fab7/weft/harnesses");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.toml"), "name = \"x\"\ntitle = \"X\"\nprogram = \"x\"\n").unwrap();
    app.clients[0].harnesses = Harnesses::default();
    assert!(app.harnesses().0.is_empty());
}

#[test]
fn every_word_the_views_use_is_in_the_table() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![src.clone()];
    let mut missing = Vec::new();
    while let Some(p) = files.pop() {
        if p.is_dir() {
            files.extend(std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()));
            continue;
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for call in ["word(\"", "fill(\"", "say(\""] {
            for (at, _) in text.match_indices(call) {
                let rest = &text[at + call.len()..];
                let key = &rest[..rest.find('"').unwrap()];
                if !words::WORDS.iter().any(|(k, _)| *k == key) {
                    missing.push(format!("{}: {key}", p.display()));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

#[test]
fn every_sheet_closes_with_the_same_button() {
    let (_tmp, mut app) = sample_app(false);
    let mut closes = Vec::new();
    for a in [
        Action::from(TreeAct::SelectAsk { project: weft(), unit: "ask_1".into() }),
        TreeAct::SelectEval { project: weft(), eval: "evl_3".into() }.into(),
        SheetAct::EvalView { project: weft(), eval: "evl_3".into() }.into(),
        TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() }.into(),
        TreeAct::SelectAgent { project: weft(), session: "s4".into(), pane: None }.into(),
        WindowAct::OpenUpdates.into(),
        WindowAct::OpenRemote.into(),
    ] {
        app.dispatch(a);
        let dom = app.render();
        closes.push(dom.find_layer("btn Close").expect("a Close").clone());
    }
    assert!(closes.windows(2).all(|w| w[0] == w[1]), "{closes:#?}");
}

/// Each agent row is two lines (card 11.12): its status shape and name, then
/// its harness, state and time. An agent nothing has named shows its harness,
/// and its second line leaves the harness out.
#[test]
fn the_agent_rows_are_the_name_then_harness_state_and_time() {
    let (_tmp, app) = sample_app(true);
    let dom = app.render();
    let tree = dom.find_layer("tree").expect("tree");
    assert_eq!(style(tree, "width"), Some("340px"));
    for (layer, shape, name, about) in [
        ("row agent antigravity fix login", "■", "fix login", "antigravity · needs you · "),
        ("row agent codex", "●", "codex", "running · "),
        ("row agent claude health endpoint", "●", "health endpoint", "claude · running · "),
        ("row agent claude readme fix", "□", "readme fix", "claude · ended · "),
    ] {
        let row = tree.find_layer(layer).unwrap_or_else(|| panic!("{layer}"));
        assert_eq!(style(row, "height"), Some("52px"), "{layer}");
        assert_eq!(row.children[0].all_text(), shape, "{layer}");
        assert_eq!(row.find_layer("agent name").unwrap().all_text(), name, "{layer}");
        let said = row.find_layer("agent about").unwrap().all_text();
        assert!(said.starts_with(about), "{layer}: {said}");
        assert!(said.ends_with(|c: char| c.is_ascii_digit()), "{layer} ends with its time: {said}");
    }
    assert!(tree.find_layer("row agent codex").unwrap().all_text().contains('⚡'));
    assert!(!tree.find_layer("row agent claude readme fix").unwrap().all_text().contains('⚡'));
    assert_eq!(tree.find_layer("section AGENTS").unwrap().all_text(), "AGENTS");
    assert_eq!(tree.find_layer("section WORK").unwrap().all_text(), "WORK");
}

#[test]
fn once_the_turn_has_ended_the_bar_outlines_the_next_act_for_the_agent_in_fronts_work() {
    let (_tmp, mut app) = sample_app(false);
    app.state.panes.groups.clear();
    app.ensure_tab_in_active_pane(SavedPaneTab {
        project: weft(),
        pane: 2,
        view: Default::default(),
    });
    let outlined = |app: &WeftDesktopApp| {
        let dom = app.render();
        let bar = dom.find_layer("tree bar").expect("the bar").clone();
        ["btn Ask", "btn Eval", "btn Seal"]
            .into_iter()
            .filter(|b| {
                bar.find_layer(b)
                    .and_then(|e| style(e, "border"))
                    .is_some_and(|v| v.contains(palette::TEAL) || v.contains(palette::AMBER))
            })
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    app.clients[0].units[0].session_ref = Some("s3".into());
    app.clients[0].sessions[2].turn = Some(Turn::TurnEnded);
    assert_eq!(app.suggested_act(), Some(Act::Eval));
    assert_eq!(outlined(&app), ["btn Eval"]);
    app.clients[0].units[0].check =
        Some(Check { eval_id: "evl_9".into(), report: Report::default(), judged_by: None });
    assert_eq!(outlined(&app), ["btn Seal"]);
    app.clients[0].sessions[2].turn = Some(Turn::Working);
    assert!(outlined(&app).is_empty());
}

fn tab_panes(app: &WeftDesktopApp) -> Vec<u32> {
    app.state.panes.groups.iter().flat_map(|g| &g.tabs).map(|t| t.pane).collect()
}

#[test]
fn every_tab_has_a_close_and_it_hides_the_tab_while_its_agent_runs_on() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let claude = start(&mut app, &mut wire, "claude");
    let codex = start(&mut app, &mut wire, "codex");
    let strip = app.render().find_layer("tab strip").expect("tab strip").clone();
    let closes = every(&strip)
        .into_iter()
        .filter(|e| e.layer_name.as_deref().is_some_and(|n| n.starts_with("close ")))
        .count();
    assert_eq!(closes, 2, "a ✕ on every tab, not only the one in front");

    let sent = wire.calls().len();
    app.dispatch(PaneAct::CloseTab { group: 0, tab: 1 });
    assert_eq!(tab_panes(&app), [claude]);
    let calls = wire.calls();
    assert!(
        !calls[sent..].iter().any(|(_, c)| matches!(c, Call::CloseAgent { .. })),
        "the ✕ stops no agent: {calls:?}"
    );
    assert!(app.clients[0].panes.iter().any(|p| p.id == codex && p.running), "it runs on");

    let root = app.clients[0].root.clone();
    app.dispatch(TreeAct::SelectAgent { project: root, session: String::new(), pane: Some(codex) });
    assert_eq!(tab_panes(&app), [claude, codex], "selected in the tree, its tab shows again");
    assert!(app.state.panes.hidden.is_empty());
}

#[test]
fn a_hidden_tab_stays_hidden_when_its_project_connects_again() {
    let tab = |pane| SavedPaneTab { project: weft(), pane, view: PaneViewMode::Terminal };
    let mut initial = WeftState { open_projects: vec![weft()], ..Default::default() };
    initial.panes.groups = vec![SavedTabGroup { active_tab: 0, tabs: vec![tab(0)] }];
    initial.panes.hidden = vec![tab(1), tab(7)];
    let tmp = home_with(initial);
    let mut app = WeftDesktopApp::new(tmp.path().to_path_buf(), "studio");
    let mut client = ProjectClient::disconnected(weft(), "weft");
    client.panes = vec![
        PaneView::new(0, "sh".into(), "sh".into()),
        PaneView::new(1, "sh".into(), "sh".into()),
        PaneView::new(2, "sh".into(), "sh".into()),
    ];
    app.integrate_connected_client(client);
    assert_eq!(tab_panes(&app), [0, 2], "the hidden one stays hidden, a new one shows");
    assert_eq!(app.state.panes.hidden, [tab(1)], "an agent no longer running is forgotten");
}

#[test]
fn a_tab_whose_agent_is_gone_leads_with_the_ended_shape() {
    let (_tmp, mut app) = sample_app(false);
    app.state.panes.groups.clear();
    app.ensure_tab_in_active_pane(SavedPaneTab {
        project: weft(),
        pane: 99,
        view: Default::default(),
    });
    let strip = app.render().find_layer("tab strip").expect("tab strip").clone();
    assert!(strip.all_text().starts_with('□'), "{}", strip.all_text());
    assert!(!strip.all_text().contains("agent"), "no harness made up");
}

#[test]
fn each_agent_keeps_its_own_draft_and_switching_never_carries_one_across() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let first = start(&mut app, &mut wire, "claude");
    start(&mut app, &mut wire, "codex");
    app.message_field().type_text("fix the lint");
    app.dispatch(PaneAct::SelectTab { group: 0, tab: 0 });
    assert_eq!(app.message_field().text(), "", "claude's own field");
    app.message_field().type_text("run the tests");
    app.dispatch(PaneAct::SelectTab { group: 0, tab: 1 });
    assert_eq!(app.message_field().text(), "fix the lint");
    app.dispatch(PaneAct::SendMessage);
    assert_eq!(app.message_field().text(), "", "sent");
    let sent: Vec<_> =
        wire.calls().into_iter().filter(|(_, c)| matches!(c, Call::Say { .. })).collect();
    assert_eq!(sent.len(), 1);
    app.dispatch(PaneAct::SelectTab { group: 0, tab: 0 });
    assert_eq!(app.message_field().text(), "run the tests");
    assert_eq!(app.active_tab().unwrap().pane, first);
}

#[test]
fn the_workspace_is_edge_to_edge_panes_or_the_blueprint_grid() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let empty = app.render();
    let grid = empty.find_layer("empty panes").expect("the empty workspace");
    assert_eq!(style(grid, "background"), Some(palette::TERMINAL_BG));
    assert!(grid.find_layer("hairline grid").is_some());
    assert!(grid.find_layer("woven mark").is_some());
    assert_eq!(grid.find_layer("woven said").unwrap().all_text(), words::word("workspace.empty"));

    start(&mut app, &mut wire, "claude");
    start(&mut app, &mut wire, "codex");
    let one = app.render();
    let strip = one.find_layer("tab strip").expect("tab strip");
    assert_eq!(style(strip, "height"), Some("38px"));
    assert!(strip.find_layer("view switch").is_some());
    assert!(one.find_layer("split toolbar").is_none());
    let pane = one.find_layer("pane").expect("the pane");
    assert_eq!(style(pane, "border"), None, "alone, no frame");

    app.dispatch(PaneAct::SplitRight(0));
    let two = app.render();
    let bar = two.find_layer("split toolbar").expect("one bar for the split");
    assert!(bar.all_text().contains("2 PANES"));
    let switches =
        every(&two).into_iter().filter(|e| e.layer_name.as_deref() == Some("view switch")).count();
    assert_eq!(switches, 1, "the view switch once, not per pane");
    assert_eq!(style(two.find_layer("pane").unwrap(), "border"), Some("2px solid #58a6ff"));

    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s2".into() });
    app.dispatch(BarAct::Ask);
    let asking = app.render();
    let workspace = asking.find_layer("workspace").expect("the panes stay");
    assert!(workspace.find_layer("sheet Ask").is_some(), "docked in the workspace");
    assert!(workspace.find_layer("the message row").is_none());
}

/// Every screen the sample app shows, one after another.
fn screens(app: &mut WeftDesktopApp) -> Vec<Element> {
    let mut out = vec![app.render()];
    app.clients[0].panes.push(PaneView::new(0, "claude".into(), "claude".into()));
    app.ensure_tab_in_active_pane(SavedPaneTab {
        project: weft(),
        pane: 0,
        view: Default::default(),
    });
    for a in [
        Action::from(WindowAct::ToggleMenu),
        WindowAct::ClosePopovers.into(),
        TreeAct::TogglePlus(weft()).into(),
        WindowAct::ClosePopovers.into(),
        TreeAct::SelectAsk { project: weft(), unit: "ask_1".into() }.into(),
        TreeAct::SelectEval { project: weft(), eval: "evl_3".into() }.into(),
        SheetAct::EvalView { project: weft(), eval: "evl_3".into() }.into(),
        TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() }.into(),
        TreeAct::SelectAgent { project: weft(), session: "s4".into(), pane: None }.into(),
        WindowAct::OpenUpdates.into(),
        WindowAct::OpenRemote.into(),
        SheetAct::Close.into(),
        PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation }.into(),
        BarAct::Seal.into(),
        WindowAct::RequestQuit.into(),
    ] {
        app.dispatch(a);
        out.push(app.render());
    }
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s1".into() });
    app.dispatch(BarAct::Ask);
    out.push(app.render());
    out
}

#[test]
fn corners_are_the_guidelines_two_radii_and_every_button_is_flat() {
    let (_tmp, mut app) = sample_app(false);
    let radii = [format!("{}px", palette::RADIUS_CONTROL), format!("{}px", palette::RADIUS_PANEL)];
    for screen in screens(&mut app) {
        for el in every(&screen) {
            if el.styles.iter().any(|(k, v)| k == "border" && v.starts_with("1px")) {
                assert!(
                    style(el, "border-radius").is_some(),
                    "{:?} is boxed but square",
                    el.layer_name
                );
            }
            for (k, v) in &el.styles {
                assert!(
                    k != "border-radius" || radii.contains(v),
                    "{:?} rounds by {v}",
                    el.layer_name
                );
                assert!(k != "box-shadow", "{:?} casts a shadow ({v})", el.layer_name);
            }
        }
    }
}

#[test]
fn every_style_the_views_use_is_one_the_window_draws() {
    let (_tmp, mut app) = sample_app(false);
    for screen in screens(&mut app) {
        for el in every(&screen) {
            for (k, v) in &el.styles {
                assert!(
                    crate::window::STYLES.contains(&k.as_str()),
                    "{:?} uses {k}: {v}",
                    el.layer_name
                );
            }
        }
    }
}

#[test]
fn a_conversation_is_read_again_when_the_agent_moves_not_when_its_read_is_answered() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "claude");
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, _) = wire.call();
    wire.answer(id, json!({"conversation": {"items": []}}));
    pumped(&mut app, |a| a.conversations.contains_key(&(weft(), pane)));
    for _ in 0..5 {
        app.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(wire.calls().is_empty(), "its own answer asks nothing again");
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Working)],
        changed: Vec::new(),
        gone: Vec::new(),
        earlier: 0,
        zone: 0,
    });
    pumped(&mut app, |a| a.clients[0].owes(&Then::Conversation { pane }));
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Read { ref what, .. } if what == "conversation"));
}

#[test]
fn a_conversation_reads_its_older_pages_and_what_comes_after_the_newest() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "claude");
    let key = (weft(), pane);
    let ids =
        |a: &WeftDesktopApp| a.conversations[&key].iter().map(|i| i.id.clone()).collect::<Vec<_>>();
    let read = |wire: &mut Wire| match wire.call() {
        (id, Call::Read { before, after, .. }) => (id, before, after),
        (_, call) => panic!("{call:?}"),
    };

    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, before, after) = read(&mut wire);
    assert_eq!((before, after), (None, None), "the newest page first");
    let newest = vec![item("50.0", Kind::Person, None, Some("two"))];
    wire.answer(id, json!({"conversation": {"items": newest, "before": "1:50", "after": "1:90"}}));
    pumped(&mut app, |a| a.conversations.contains_key(&key));

    app.dispatch(PaneAct::Older { project: weft(), pane });
    let (id, before, after) = read(&mut wire);
    assert_eq!((before.as_deref(), after), (Some("1:50"), None));
    let older = vec![item("0.0", Kind::Person, None, Some("one"))];
    wire.answer(id, json!({"conversation": {"items": older, "before": null, "after": "1:90"}}));
    pumped(&mut app, |a| a.conversations[&key].len() == 2);
    assert_eq!(ids(&app), ["0.0", "50.0"]);
    app.dispatch(PaneAct::Older { project: weft(), pane });
    assert!(wire.calls().is_empty(), "nothing is older than the file's start");

    app.refresh_active_conversations();
    let (id, before, after) = read(&mut wire);
    assert_eq!((before, after.as_deref()), (None, Some("1:90")), "only what was added");
    let added = vec![item("90.0", Kind::Agent, None, Some("three"))];
    wire.answer(id, json!({"conversation": {"items": added, "after": "1:120"}}));
    pumped(&mut app, |a| a.conversations[&key].len() == 3);
    assert_eq!(ids(&app), ["0.0", "50.0", "90.0"]);
}

#[test]
fn a_read_refused_leaves_the_bar_and_a_refusal_never_covers_a_question() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectAsk { project: weft(), unit: "ask_2".into() });
    let (id, _) = wire.call();
    wire.refuse(id, "what was asked is not on disk");
    pumped(&mut app, |a| !a.clients[0].owes(&Then::Source { unit: "ask_2".into() }));
    assert_eq!(app.bottom_bar, BottomBarState::None);
    app.clients[0].panes.push(PaneView::new(0, "claude".into(), "claude".into()));
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    app.bottom_bar = BottomBarState::QuitConfirm;
    let (id, _) = wire.calls().into_iter().find(|(_, c)| matches!(c, Call::Act { .. })).unwrap();
    wire.refuse(id, "no agent can run it");
    pumped(&mut app, |a| !a.clients[0].owes(&Then::Refusal));
    assert_eq!(app.bottom_bar, BottomBarState::QuitConfirm, "the question stays");
}

#[test]
fn a_seal_opened_again_claims_nothing_until_checked_again() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].work.record.seals = vec![seal("sel_4", 4, None)];
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() });
    let (id, _) = wire.call();
    wire.answer(id, json!({"seal_id": "sel_4", "codes": []}));
    pumped(&mut app, |a| !a.clients[0].read.is_empty());
    assert!(app.render().all_text().contains(words::word("web.seal.holds")));
    app.dispatch(SheetAct::Close);
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() });
    let text = app.render().all_text();
    assert!(!text.contains(words::word("web.seal.holds")), "the old check is not shown");
    assert!(text.contains(words::word("wait.checking_receipt")));
}

#[test]
fn nothing_written_is_lost_while_the_daemon_is_not_connected() {
    let (_tmp, mut app) = sample_app(false);
    app.ensure_tab_in_active_pane(SavedPaneTab {
        project: weft(),
        pane: 0,
        view: Default::default(),
    });
    app.message_field().type_text("run the tests");
    app.dispatch(PaneAct::SendMessage);
    assert_eq!(app.message_field().text(), "run the tests", "kept until sent");
    assert!(matches!(app.bottom_bar, BottomBarState::WhyNot { .. }));
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s1".into() });
    app.dispatch(BarAct::Ask);
    app.ask.set_text("fix the login");
    app.dispatch(SheetAct::SubmitAsk);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::Ask { .. })), "the Ask stays open");
    assert_eq!(app.ask.text(), "fix the login");
}

/// The machine goes by what the daemon calls it: its settings' name once
/// they are told, what it said when it answered before that, and the name
/// the window started with until it answers.
#[test]
fn the_machine_is_named_by_the_daemon_once_it_answers() {
    let (_tmp, mut app, _wire) = wired_app();
    app.clients[0].config = None;
    app.clients[0].machine_name = None;
    assert_eq!(app.machine_name(), "studio", "before the daemon answers");
    app.clients[0].machine_name = Some("Ada’s MacBook Pro".into());
    assert_eq!(app.machine_name(), "Ada’s MacBook Pro", "as it said when it answered");
    app.clients[0].config = Some(json!({"machine": {}, "effective": {"name": "Lab"}}));
    assert_eq!(app.machine_name(), "Lab", "as its settings say now");
}

/// Settings › General names the machine: its title and the Name field say
/// what the daemon calls it; the field starts as that name, and what is
/// typed is set through the daemon, emptied as nothing set.
#[test]
fn settings_name_the_machine_and_its_name_is_set_through_the_daemon() {
    let (_tmp, mut app, mut wire) = wired_app();
    let long = "Ada’s MacBook Pro with a name far longer than any field it is shown in";
    app.clients[0].config =
        Some(json!({"machine": {}, "project": null, "effective": {"name": long}}));
    app.dispatch(WindowAct::OpenSettings);
    let dom = app.render();
    let sheet = dom.find_layer("sheet Settings").expect("the Settings sheet");
    assert!(sheet.all_text().contains(&format!("Settings · {long}")), "the title names it");
    let row = sheet.find_layer("setting Name").expect("the Name row");
    assert_eq!(row.find_layer("setting value").unwrap().all_text(), long);

    let typed = |app: &mut WeftDesktopApp, text: &str| {
        app.dispatch(SheetAct::EditSetting { project: None, key: vec!["name".into()] });
        assert_eq!(app.setting_field.text(), long, "it starts as the name now");
        app.setting_field.set_text(text);
        app.dispatch(SheetAct::CommitSetting);
    };
    typed(&mut app, "  Studio ");
    let set = |value| Call::ConfigSet { scope: "machine".into(), key: vec!["name".into()], value };
    assert_eq!(wire.call().1, set(Some(json!("Studio"))));
    typed(&mut app, "   ");
    assert_eq!(wire.call().1, set(None), "emptied: the system's own name again");
}

/// Keep ended sessions: the days that apply, typed and set through the
/// daemon as a number; emptied, the default again; anything else, nothing.
#[test]
fn settings_keep_ended_sessions_for_the_days_typed() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config =
        Some(json!({"machine": {}, "project": null, "effective": {"keep_days": 7}}));
    app.dispatch(WindowAct::OpenSettings);
    let dom = app.render();
    let row = dom.find_layer("setting Keep ended sessions").expect("the row");
    assert_eq!(row.find_layer("setting value").unwrap().all_text(), "7");
    assert!(row.all_text().contains("days"));
    let key = vec!["sessions".to_string(), "keep_days".to_string()];
    let typed = |app: &mut WeftDesktopApp, text: &str| {
        app.dispatch(SheetAct::EditSetting { project: None, key: key.clone() });
        assert_eq!(app.setting_field.text(), "7", "it starts as the days now");
        app.setting_field.set_text(text);
        app.dispatch(SheetAct::CommitSetting);
    };
    let set = |value| Call::ConfigSet { scope: "machine".into(), key: key.clone(), value };
    typed(&mut app, " 30 ");
    assert_eq!(wire.call().1, set(Some(json!(30))));
    typed(&mut app, "");
    assert_eq!(wire.call().1, set(None), "emptied: seven again");
    typed(&mut app, "a week");
    typed(&mut app, "0");
    assert_eq!(wire.call().1, set(Some(json!(0))), "words set nothing; 0 keeps them all");
}

/// The daemon carries today's sessions and counts the earlier ones: the tree
/// follows what moved, counts them, and the first page of them is read.
#[test]
fn the_tree_follows_what_moved_and_counts_the_earlier_sessions() {
    let (_tmp, mut app, mut wire) = wired_app();
    let kept: Vec<String> = app.clients[0].sessions.iter().map(Row::key).collect();
    let mut s1 = app.clients[0].sessions[0].clone();
    s1.turn = Some(Turn::Working);
    let today = Row::of("codex", "s-new", State::Ended, None, 1200);
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Working)],
        changed: vec![s1, today],
        gone: vec![kept[3].clone()],
        earlier: 1_204,
        zone: 0,
    });
    pumped(&mut app, |a| a.clients[0].earlier == 1_204);
    let ids: Vec<_> = app.clients[0].sessions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["s2", "s1", "s3", "s-new"], "s4 went; s1 moved in place");
    let (id, call) = wire.call();
    assert!(matches!(&call, Call::Read { what, after: None, q: None, .. } if what == "sessions"));
    wire.answer(id, earlier_page(1_204, 0, ""));
    pumped(&mut app, |a| a.clients[0].earlier_rows.rows.len() == 50);
    let dom = app.render();
    let earlier = dom.find_layer("row earlier sessions").expect("counted");
    assert!(earlier.all_text().contains("1,204"), "{}", earlier.all_text());
}

/// `total` earlier sessions `old-<n>`, newest first, as the daemon answers
/// a page of them from row `from`; a search's finds are named for it.
fn earlier_page(total: usize, from: usize, q: &str) -> Value {
    let end = (from + 50).min(total);
    let rows: Vec<Row> = (from..end)
        .map(|i| {
            let mut r = Row::of("codex", &format!("old-{i}"), State::Ended, None, 1_000 - i as i64);
            r.title = Some(format!("{q} {i}").trim().to_string());
            r
        })
        .collect();
    let next = (end < total).then(|| format!("codex/old-{}", end - 1));
    json!({"rows": rows, "total": total, "next": next})
}

/// Opened, the earlier sessions list 50, newest first, then "Show 50 more"
/// with how many of all are shown; each page appends, the last says how
/// many are left, and once all are shown there is no such row.
#[test]
fn earlier_sessions_open_a_page_at_a_time() {
    let (_tmp, mut app, mut wire) = wired_app();
    let root = app.clients[0].root.clone();
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Working)],
        changed: vec![],
        gone: vec![],
        earlier: 120,
        zone: 0,
    });
    pumped(&mut app, |a| a.clients[0].earlier == 120);
    let (id, _) = wire.call();
    wire.answer(id, earlier_page(120, 0, ""));
    pumped(&mut app, |a| a.clients[0].earlier_rows.rows.len() == 50);
    let tree = |app: &WeftDesktopApp| app.render().find_layer("tree").unwrap().clone();
    let folded = tree(&app);
    assert!(folded.find_layer("row agent codex 0").is_none(), "folded: none listed");
    assert!(folded.all_text().contains("120 earlier sessions ›"), "{}", folded.all_text());

    app.dispatch(TreeAct::OpenEarlier(root.clone()));
    let opened = tree(&app);
    let head = opened.find_layer("row earlier sessions").unwrap().all_text();
    assert_eq!(head, "▾ Earlier sessions 120");
    let more = opened.find_layer("row earlier more").expect("more to show");
    assert_eq!(more.all_text(), "Show 50 more 50 of 120");
    assert!(opened.find_layer("row agent codex 49").is_some(), "the 50th listed");
    assert!(opened.find_layer("row agent codex 50").is_none(), "not the 51st");
    // An earlier one selected opens its Details, as a carried one's do.
    app.dispatch(TreeAct::SelectAgent {
        project: root.clone(),
        session: "old-3".into(),
        pane: None,
    });
    let sheet = app.render().find_layer("sheet SessionDetail").expect("its Details").clone();
    assert!(
        !sheet.all_text().contains(words::word("desktop.session.gone")),
        "{}",
        sheet.all_text()
    );

    app.dispatch(TreeAct::MoreEarlier(root.clone()));
    let (id, call) = wire.call();
    assert!(
        matches!(&call, Call::Read { after: Some(a), q: None, .. } if a == "codex/old-49"),
        "{call:?}"
    );
    wire.answer(id, earlier_page(120, 50, ""));
    pumped(&mut app, |a| a.clients[0].earlier_rows.rows.len() == 100);
    let more = tree(&app).find_layer("row earlier more").unwrap().all_text();
    assert_eq!(more, "Show 20 more 100 of 120", "fewer than a page are left");

    app.dispatch(TreeAct::MoreEarlier(root.clone()));
    let (id, _) = wire.call();
    wire.answer(id, earlier_page(120, 100, ""));
    pumped(&mut app, |a| a.clients[0].earlier_rows.rows.len() == 120);
    let all = tree(&app);
    assert!(all.find_layer("row earlier more").is_none(), "all shown");
    assert!(all.find_layer("row agent codex 119").is_some());

    // Closed and opened again, it lists one page.
    app.dispatch(TreeAct::OpenEarlier(root.clone()));
    app.dispatch(TreeAct::OpenEarlier(root));
    let again = tree(&app);
    assert_eq!(again.find_layer("row earlier more").unwrap().all_text(), "Show 50 more 50 of 120");
}

/// The search reaches every earlier session through the daemon, not only
/// the rows read: what it finds is listed and counted.
#[test]
fn a_search_finds_earlier_sessions_through_the_daemon() {
    let (_tmp, mut app, mut wire) = wired_app();
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Working)],
        changed: vec![],
        gone: vec![],
        earlier: 1_204,
        zone: 0,
    });
    pumped(&mut app, |a| a.clients[0].earlier == 1_204);
    let (id, _) = wire.call();
    wire.answer(id, earlier_page(1_204, 0, ""));
    pumped(&mut app, |a| a.clients[0].earlier_rows.rows.len() == 50);

    app.search.set_text("pane resize");
    app.pump();
    let (id, call) = wire.call();
    assert!(
        matches!(&call, Call::Read { what, after: None, q: Some(q), .. }
            if what == "sessions" && q == "pane resize"),
        "{call:?}"
    );
    app.pump();
    assert!(wire.calls().is_empty(), "asked once for the same words");
    wire.answer(id, earlier_page(70, 0, "pane resize"));
    pumped(&mut app, |a| a.clients[0].earlier_found.as_ref().is_some_and(|f| f.rows.len() == 50));
    let tree = app.render().find_layer("tree").unwrap().clone();
    assert!(tree.all_text().contains("70 match"), "{}", tree.all_text());
    assert!(tree.find_layer("row agent codex pane resize 0").is_some());
    assert_eq!(tree.find_layer("row earlier more").unwrap().all_text(), "Show 20 more 50 of 70");
    app.dispatch(TreeAct::MoreEarlier(app.clients[0].root.clone()));
    let (_, call) = wire.call();
    assert!(
        matches!(&call, Call::Read { after: Some(a), q: Some(q), .. }
            if a == "codex/old-49" && q == "pane resize"),
        "{call:?}"
    );

    // Other words: a new search, and the answer to the old one is let go.
    app.search.set_text("eval");
    app.pump();
    let (id, _) = wire.call();
    wire.answer(id, earlier_page(0, 0, "eval"));
    pumped(&mut app, |a| {
        a.clients[0].earlier_found.as_ref().is_some_and(|f| f.q == "eval" && !f.asking())
    });
    app.search.set_text("");
    app.pump();
    assert!(app.clients[0].earlier_found.is_none(), "no search, nothing found");
}

#[test]
fn the_state_is_written_once_by_whoever_takes_it() {
    let (tmp, mut app) = sample_app(false);
    app.dispatch(WindowAct::ToggleSidebar);
    let (home, state) = app.unsaved_state().expect("changed");
    assert_eq!(home, tmp.path());
    assert!(!state.settings.sidebar_open);
    assert!(app.unsaved_state().is_none(), "taken once");
}

/// An Eval's view as the daemon reads it: the record of an Eval of "fix
/// login", with a question carried from an earlier Seal, changes and a
/// requirement to look at, one the readings could not settle, and the rest
/// settled.
fn an_eval_view() -> weft_core::eval_view::EvalView {
    let record: serde_json::Value =
        serde_json::from_str(include_str!("../../../weft-core/tests/fixtures/eval-v4/record.json"))
            .unwrap();
    let mut v = weft_core::eval_view::EvalView::of_record("evl_3", &record);
    v.title = "fix login".into();
    v
}

/// The colour an item's shape is drawn in, in the list.
fn mark_colour_of(sheet: &crate::gpui::Element, key: &str) -> String {
    let row = sheet
        .find_layer(&format!("eval item {key}"))
        .or_else(|| sheet.find_layer(&format!("eval item {key} picked")))
        .expect(key);
    style(row.find_layer("mark").expect("its shape"), "color").unwrap_or_default().to_string()
}

/// Open the Eval view and answer its read with `view`.
pub(crate) fn open_view(
    app: &mut WeftDesktopApp,
    wire: &mut Wire,
    view: &weft_core::eval_view::EvalView,
) {
    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    answer_view(app, wire, view);
}

/// Answer the Eval view's newest read with `view`.
pub(crate) fn answer_view(
    app: &mut WeftDesktopApp,
    wire: &mut Wire,
    view: &weft_core::eval_view::EvalView,
) {
    let (id, call) = wire
        .calls()
        .into_iter()
        .rev()
        .find(|(_, c)| matches!(c, Call::Read { what, .. } if what == "eval"))
        .expect("read");
    assert!(matches!(call, Call::Read { ref unit, .. } if unit == "evl_3"));
    wire.answer(id, serde_json::to_value(view).unwrap());
    pumped(app, |a| !a.clients[0].owes_any(|t| matches!(t, Then::EvalView { .. })));
}

#[test]
fn the_eval_view_lists_its_items_and_shows_the_one_picked_in_plain_prose() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    let before = app.render().find_layer("sheet EvalView").unwrap().clone();
    assert!(before.find_layer("loading").is_some(), "it breathes while it is read");
    assert!(!before.all_text().contains(words::word("desktop.eval.no_record")));
    open_view(&mut app, &mut wire, &an_eval_view());

    let sheet = app.render().find_layer("sheet EvalView").unwrap().clone();
    let text = sheet.all_text();
    for said in
        ["‹ fix login", "To look at", "Settled", "3 other changes your Ask explains", "Seal…"]
    {
        assert!(text.contains(said), "{said} in {text}");
    }
    for gone in ["By file", "By step", "verdict", "aligned", "%"] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    assert!(sheet.find_layer("by file").is_none() && sheet.find_layer("by step").is_none());
    // Each kind has its colour, on its shape only.
    assert_eq!(mark_colour_of(&sheet, "c|sel_1/d2"), palette::TEAL, "asked before");
    assert_eq!(mark_colour_of(&sheet, "w|w_sess"), palette::AMBER_BORDER, "to look at");
    assert_eq!(mark_colour_of(&sheet, "r|ask_1/f0"), palette::BLUE_BORDER, "could not settle");
    assert_eq!(mark_colour_of(&sheet, "r|ask_1/f1"), palette::MUTED, "settled");
    // The question from an earlier Seal is on top, and picked first.
    let list = sheet.find_layer("eval items").unwrap();
    let first = list.all_text().find("↺").unwrap();
    assert!(list.all_text()[..first].find('■').is_none(), "{}", list.all_text());
    let picked = sheet.find_layer("eval item c|sel_1/d2 picked").expect("picked first");
    assert_eq!(style(picked, "background"), Some(palette::SELECTED_BG));
    assert!(picked.find_layer("picked bar").is_some(), "a bar on its left edge");
    let detail = sheet.find_layer("eval detail").unwrap().all_text();
    assert!(detail.contains("↺ Asked before · not sure at your Seal of 10 Nov"), "{detail}");

    // Picking another shows it: against a decision, with what the agent said.
    app.dispatch(SheetAct::EvalOpen("w|w_log".into()));
    let sheet = app.render().find_layer("sheet EvalView").unwrap().clone();
    assert!(sheet.find_layer("eval item c|sel_1/d2 picked").is_none());
    let row = sheet.find_layer("eval item w|w_log picked").expect("picked");
    let words =
        row.children.iter().find(|c| c.text.as_deref().is_some_and(|t| t.starts_with("A failed")));
    assert_eq!(words.and_then(|w| style(w, "font-weight")), Some("600"), "bold");
    let detail = sheet.find_layer("eval detail").unwrap();
    let said = detail.all_text();
    assert!(said.contains("■ To look at · against your decision of 3 Nov"), "{said}");
    assert!(said.contains("read twice, both agree"), "{said}");
    assert!(said.contains("The agent said: “Failed sign-ins"), "{said}");
    let how = detail.find_layer("how sure").unwrap().find_layer("dots").unwrap();
    assert_eq!(how.children.len(), 2, "a dot per reading");

    // What each read said opens in place.
    app.dispatch(SheetAct::EvalOpen("reads".into()));
    let reads = app.render().find_layer("reads").expect("open").all_text();
    assert!(reads.contains("First read · A failed sign-in"), "{reads}");
    app.dispatch(SheetAct::EvalBack);
    assert!(app.render().find_layer("reads").is_none());

    // Its lines, the ones a reading quoted brighter.
    app.dispatch(SheetAct::EvalOpen("lines".into()));
    let (id, call) = wire
        .calls()
        .into_iter()
        .find(|(_, c)| matches!(c, Call::Read { what, .. } if what == "window"))
        .expect("its lines");
    assert!(matches!(call, Call::Read { ref unit, .. } if unit == "evl_3/w_log"));
    // While they come, lines breathe in their place, never "not come yet".
    let coming = app.render().find_layer("eval lines").unwrap().clone();
    assert!(coming.find_layer("loading").is_some());
    assert!(!coming.all_text().contains(words::word("desktop.eval.no_lines")));
    wire.answer(id, json!({"text": "@@ -40,1 +40,2 @@\n fail() {\n+  log.warn(email)\n"}));
    pumped(&mut app, |a| !a.clients[0].owes_any(|t| matches!(t, Then::Window { .. })));
    let lines = app.render().find_layer("eval lines").unwrap().clone();
    assert!(lines.all_text().contains("■ To look at · the lines that show it"));
    assert!(lines.all_text().contains("src/auth/login.ts"), "{}", lines.all_text());
    let lit = lines.find_layer("line lit").expect("the quoted line is brighter");
    assert!(lit.all_text().contains("log.warn(email)"));
    // Read again and refused: why, where the lines would be, with Try again.
    app.dispatch(SheetAct::EvalBack);
    app.dispatch(SheetAct::EvalOpen("lines".into()));
    let (id, _) = wire
        .calls()
        .into_iter()
        .find(|(_, c)| matches!(c, Call::Read { what, .. } if what == "window"))
        .expect("its lines again");
    wire.refuse(id, "the diff is gone");
    pumped(&mut app, |a| !a.clients[0].owes_any(|t| matches!(t, Then::Window { .. })));
    let lines = app.render().find_layer("eval lines").unwrap().clone();
    let failed = lines.find_layer("failed").expect("why").all_text();
    assert!(failed.contains("This could not be read: the diff is gone"), "{failed}");
    assert!(failed.contains(words::word("wait.try_again")));
    // Back to the item, then to Details.
    app.dispatch(SheetAct::EvalBack);
    assert!(app.render().find_layer("eval detail").is_some());
    app.dispatch(SheetAct::EvalBack);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::EvalDetail { .. })));
}

/// An Eval's Details (11.3): its headline, its items each opening the Eval
/// view on itself, and what the Seal will ask.
#[test]
fn an_eval_s_details_say_its_headline_and_each_item_opens_the_view_on_itself() {
    let (_tmp, mut app, mut wire) = wired_app();
    let record: serde_json::Value =
        serde_json::from_str(include_str!("../../../weft-core/tests/fixtures/eval-v4/record.json"))
            .unwrap();
    app.clients[0].records["evl_3"] = record;
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_3".into() });
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().clone();
    let text = sheet.all_text();
    assert!(
        sheet.find_layer("headline").unwrap().all_text().contains("5 things to look at"),
        "{text}"
    );
    assert!(text.contains("read by claude"), "{text}");
    assert!(text.contains("Seal asks you 5 questions, then records your decision."), "{text}");
    for gone in ["verdict", "aligned", "met ·", "%", "By file"] {
        assert!(!text.contains(gone), "{gone} in {text}");
    }
    let row = sheet.find_layer("item w|w_log").expect("an item");
    assert_eq!(row.find_layer("mark").and_then(|m| style(m, "color")), Some(palette::AMBER_BORDER));
    app.dispatch(SheetAct::EvalItem {
        project: weft(),
        eval: "evl_3".into(),
        item: "w|w_log".into(),
    });
    answer_view(&mut app, &mut wire, &an_eval_view());
    let view = app.render().find_layer("sheet EvalView").unwrap().clone();
    assert!(view.find_layer("eval item w|w_log picked").is_some(), "opened on the item");
}

#[test]
fn a_running_eval_is_one_ring_an_arc_per_reading_its_steps_and_a_plain_stop() {
    use weft_core::eval_view::{EvalView, Sources};
    let (_tmp, mut app, mut wire) = wired_app();
    let rd1 = json!({"kind": "read", "task_id": "evl_3~rd1",
        "windows": [{"id": "w1", "answer": "unexplained", "quote": "x", "reason": "Sessions end after an hour."}],
        "requirements": []});
    let evidence = json!({"windows": [{"id": "w1", "path": "src/a.ts", "class": "code"}]});
    let brief = json!({"asks": [{"ask_id": "a", "title": "fix login"}]});
    let (planned, done) =
        (["evl_3~rd1".to_string(), "evl_3~rr1".to_string()], ["evl_3~rd1".to_string()]);
    let running = EvalView::build(Sources {
        eval_id: "evl_3",
        changes: &json!({}),
        evidence: &evidence,
        brief: &brief,
        record: None,
        outputs: &[rd1],
        planned: &planned,
        done: &done,
        checks_running: &["npm test".to_string()],
    });
    let mut judge = PaneView::new(9, "claude".into(), "claude".into());
    (judge.judge, judge.turn) = (Some("evl_3".into()), Some(Turn::Working));
    app.clients[0].panes.push(judge);
    open_view(&mut app, &mut wire, &running);

    let sheet = app.render().find_layer("sheet EvalView").unwrap().clone();
    let text = sheet.all_text();
    let ring = sheet.find_layer("progress ring").expect("one ring");
    assert!(ring.find_layer("arc 0 done").is_some(), "{ring:?}");
    let now = ring.find_layer("arc 1 now").expect("the second reading under way");
    assert_eq!(style(now, "animation"), Some("pulse"), "it breathes");
    assert!(ring.find_layer("arc 2 next").is_some());
    assert!(ring.find_layer("ring label").unwrap().all_text().contains("2nd read"));
    for said in [
        "✓ What you asked",
        "✓ Read the change",
        "Reading again where unsure",
        "Questions for your Seal, if needed",
        "claude is reading · Watch ›",
        "npm test is running",
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }
    let row = sheet.find_layer("eval item w|w1").expect("the list fills in");
    assert!(row.all_text().contains("reading again"), "{}", row.all_text());
    app.dispatch(SheetAct::WatchJudge { project: weft(), pane: 9 });
    assert_eq!(app.active_tab().map(|t| t.pane), Some(9), "the reader opens to be watched");

    open_view(&mut app, &mut wire, &running);
    let stop = app.render().find_layer("btn Stop Eval").expect("Stop Eval").clone();
    assert_eq!(stop.all_text(), "Stop Eval");
    assert_ne!(style(&stop, "color"), Some(palette::AMBER), "a plain button");
    app.dispatch(SheetAct::StopEval { project: weft(), eval: "evl_3".into() });
    let asked = wire.calls().into_iter().any(|(_, c)| {
        matches!(c, Call::Act { ref act, ref eval, .. } if act == "stop" && eval.as_deref() == Some("evl_3"))
    });
    assert!(asked, "Stop asks the daemon for its one dialog");
}

#[test]
fn an_ended_agents_pane_says_so_and_its_work_stays_listed() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "claude");
    wire.event(Event::Exited { pane });
    pumped(&mut app, |a| a.clients[0].view(pane).is_some_and(|v| !v.running));
    let dom = app.render();
    let said = dom.find_layer("agent ended").expect("the pane says it ended").all_text();
    assert_eq!(said, words::say("agent.ended", "claude"));
    assert!(dom.find_layer("terminal").is_none());
    assert!(dom.find_layer("the message row").is_none(), "nothing to send to");
}

#[test]
fn the_header_says_while_weft_is_connecting_or_reconnecting() {
    let (_tmp, mut app) = sample_app(false);
    let tag = |app: &WeftDesktopApp| app.render().find_layer("link tag").map(|t| t.all_text());
    assert_eq!(tag(&app), Some(words::word("web.link.connecting").to_string()));
    let wire = Wire::on(&mut app.clients[0]);
    assert_eq!(tag(&app), None, "connected: nothing to say");
    wire.hang_up();
    pumped(&mut app, |a| !a.clients[0].is_connected());
    assert_eq!(tag(&app), Some(words::word("web.link.reconnecting").to_string()));
}

#[test]
fn an_agent_that_starts_waiting_off_screen_is_told_once() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let first = start(&mut app, &mut wire, "claude");
    let second = start(&mut app, &mut wire, "codex");
    // codex is in front: its own wait is not told; claude's is, once.
    let turns = |a: Option<Turn>, b: Option<Turn>| Event::Agents {
        panes: vec![a, b],
        changed: Vec::new(),
        gone: Vec::new(),
        earlier: 0,
        zone: 0,
    };
    wire.event(turns(Some(Turn::Working), Some(Turn::Waiting)));
    pumped(&mut app, |a| a.clients[0].view(second).is_some_and(|v| v.turn == Some(Turn::Waiting)));
    assert!(app.take_notices().is_empty(), "the agent in front is seen");
    wire.event(turns(Some(Turn::Waiting), Some(Turn::Waiting)));
    pumped(&mut app, |a| a.clients[0].view(first).is_some_and(|v| v.turn == Some(Turn::Waiting)));
    let told = app.take_notices();
    assert_eq!(told.len(), 1);
    assert_eq!(
        told[0].title,
        words::fill("desktop.notice.waiting", &[("harness", "claude"), ("project", "weft")])
    );
    wire.event(turns(Some(Turn::Waiting), Some(Turn::Working)));
    pumped(&mut app, |a| a.clients[0].view(second).is_some_and(|v| v.turn == Some(Turn::Working)));
    assert!(app.take_notices().is_empty(), "told once while it waits");
    // A notice opens its agent.
    app.open_notice(&told[0].tag);
    assert_eq!(app.active_tab().map(|t| t.pane), Some(first));
    // Notifications off for the project: none.
    app.clients[0].notify = false;
    wire.event(turns(Some(Turn::Working), Some(Turn::Working)));
    wire.event(turns(Some(Turn::Working), Some(Turn::Waiting)));
    pumped(&mut app, |a| a.clients[0].view(second).is_some_and(|v| v.turn == Some(Turn::Waiting)));
    assert!(app.take_notices().is_empty());
}

#[test]
fn a_project_that_will_not_open_is_told_once_with_why() {
    let (_tmp, mut app, wire) = wired_app();
    wire.hang_up();
    pumped(&mut app, |a| !a.clients[0].is_connected());
    let why = "Another version of Weft is running. `weft stop` ends it and its agents.";
    app.not_opened(&weft(), why);
    app.not_opened(&weft(), why);
    let told = app.take_notices();
    assert_eq!(told.len(), 1, "told once while the reason stays the same");
    assert_eq!(
        told[0].title,
        words::fill("desktop.notice.not_opened", &[("project", "weft"), ("why", why)])
    );
    app.not_opened(&weft(), "no session");
    assert_eq!(app.take_notices().len(), 1, "a new reason is told");
    // Closed and opened again: the same reason is told again.
    app.close_project(&weft());
    app.open_project_path(weft());
    app.not_opened(&weft(), "no session");
    assert_eq!(app.take_notices().len(), 1, "told again after a close");
}

#[test]
fn a_session_that_ended_is_told_and_the_window_goes() {
    let (_tmp, mut app, _wire) = wired_app();
    app.session_ended();
    let told = app.take_notices();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].title, words::word("desktop.notice.ended"));
    assert!(app.quit_requested, "as `weft stop` asked");
}

#[test]
fn a_tab_dragged_onto_a_pane_moves_or_splits_there() {
    use crate::panes_view::DropTarget;
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    for h in ["claude", "codex", "grok"] {
        start(&mut app, &mut wire, h);
    }
    // Every tab can be dragged, and every pane's body takes a drop three ways.
    let shown = app.render();
    let tab = shown.find_layer("tab weft · claude").expect("its tab");
    assert_eq!(tab.drag.as_ref().map(|d| (d.group, d.tab)), Some((0, 0)));
    let zones: Vec<_> = ["drop here", "drop right", "drop down"]
        .iter()
        .map(|z| shown.find_layer(z).and_then(|e| e.drop))
        .collect();
    assert_eq!(
        zones,
        [
            Some(DropTarget::TabBar { group: 0, index: usize::MAX }),
            Some(DropTarget::EdgeRight { group: 0 }),
            Some(DropTarget::EdgeDown { group: 0 }),
        ]
    );

    app.dispatch(PaneAct::DropTab { group: 0, tab: 0, target: DropTarget::EdgeRight { group: 0 } });
    assert_eq!(app.state.panes.groups.len(), 2, "split, the tab on the right");
    assert_eq!(app.state.panes.groups[1].tabs.len(), 1);
    // Onto the other pane's tab: moved in beside it.
    app.dispatch(PaneAct::DropTab {
        group: 1,
        tab: 0,
        target: DropTarget::TabBar { group: 0, index: 0 },
    });
    assert_eq!(app.state.panes.groups.len(), 1, "its pane emptied and went");
    assert_eq!(app.state.panes.groups[0].tabs.len(), 3);
}

#[test]
fn ask_on_a_project_row_opens_on_its_agents() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectProject(weft()));
    app.dispatch(BarAct::Ask);
    assert!(app.active_sheet.is_none(), "no agent to ask yet");
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: words::word("why.start_an_agent").to_string() }
    );
    start(&mut app, &mut wire, "claude");
    let codex = start(&mut app, &mut wire, "codex");
    app.dispatch(TreeAct::SelectProject(weft()));
    app.dispatch(BarAct::Ask);
    // It starts on the agent in front, named, and goes where it shows.
    let Some(ActiveSheet::Ask { project, send_to: Some(to), .. }) = &app.active_sheet else {
        panic!("{:?}", app.active_sheet)
    };
    assert_eq!(*project, weft());
    assert_eq!(*to, format!("pane:{codex}"), "the agent in front");
    app.ask.set_text("add a health endpoint");
    app.dispatch(SheetAct::SubmitAsk);
    let call = wire.calls().into_iter().map(|(_, c)| c).find(|c| matches!(c, Call::Act { .. }));
    assert!(matches!(call, Some(Call::Act { pane: Some(p), .. }) if p == codex), "{call:?}");
}

/// Remote's sheet, in the state `view` says, with what it says.
fn remote_sheet(app: &mut WeftDesktopApp, view: Option<Value>) -> Element {
    app.clients[0].remote = view;
    app.dispatch(WindowAct::OpenRemote);
    app.render().find_layer("sheet Remote").expect("the Remote sheet").clone()
}

/// Until a tunnel gives its address, the sheet says what its program said,
/// under its name; with two tunnels, Change tunnel goes back to the list with
/// the current one picked, and the person switches one way and back.
#[test]
fn remote_says_why_its_tunnel_has_not_started_and_changes_tunnel() {
    let (_tmp, mut app, mut wire) = wired_app();
    let said = |e: &Element| e.all_text();
    let on = |tunnel: &str, why: Option<&str>| {
        let mut v = json!({"state": "on", "login": "octo", "tunnel": "starting",
                           "tunnel_title": tunnel, "can_change_tunnel": true, "devices": []});
        if let Some(why) = why {
            v["tunnel_why"] = json!(why);
        }
        v
    };
    let why = remote_sheet(
        &mut app,
        Some(on("Tailscale Funnel", Some("Funnel is not enabled on your tailnet."))),
    );
    assert!(said(&why).contains(words::word("desktop.remote.tunnel_starting")));
    assert!(said(&why).contains("TAILSCALE FUNNEL SAID"), "{}", said(&why));
    assert!(said(&why).contains("Funnel is not enabled on your tailnet."));
    let up = remote_sheet(&mut app, Some(on("Tailscale Funnel", None)));
    assert!(!said(&up).contains("SAID"), "nothing said, nothing shown");

    let tunnels = json!([{"id": "cloudflared", "title": "Cloudflare Tunnel"},
                         {"id": "tailscale", "title": "Tailscale Funnel"}]);
    for (from, to) in [("cloudflared", "tailscale"), ("tailscale", "cloudflared")] {
        assert!(up.find_layer("btn Change tunnel").is_some());
        app.dispatch(SheetAct::RemoteChangeTunnel);
        assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "change_tunnel"}) });
        remote_sheet(
            &mut app,
            Some(json!({"state": "choose", "tunnels": tunnels, "chosen": from})),
        );
        app.dispatch(SheetAct::RemoteUse);
        assert_eq!(
            wire.call().1,
            Call::Remote { act: json!({"act": "choose", "tunnel": from}) },
            "the current one is picked"
        );
        app.dispatch(SheetAct::RemotePick(to.into()));
        app.dispatch(SheetAct::RemoteUse);
        assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "choose", "tunnel": to}) });
    }

    let one = remote_sheet(
        &mut app,
        Some(json!({"state": "on", "login": "octo",
        "tunnel": "up", "address": "https://a.trycloudflare.com", "devices": []})),
    );
    assert!(one.find_layer("btn Change tunnel").is_none(), "one tunnel: nothing to change to");
}

#[test]
fn remote_says_every_state_weft_remote_can_be_in_and_acts_on_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    let said = |e: &Element| e.all_text();

    // Off: what it is, and Turn on.
    let off = remote_sheet(&mut app, None);
    assert!(said(&off).contains(words::word("desktop.remote.off_what")), "{}", said(&off));
    app.dispatch(SheetAct::RemoteOn);
    assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "on"}) });
    let starting = remote_sheet(&mut app, Some(json!({"state": "starting"})));
    assert!(said(&starting).contains(words::word("desktop.remote.starting")));

    // Choose a tunnel: each with its note and guide; picking is a choice,
    // Check sends it.
    let guide = "https://github.com/fab7hq/fab7/blob/main/products/weft/tools/tailscale.md";
    let tunnels = json!([{"id": "cloudflared", "title": "Cloudflare Tunnel",
                          "note": "traffic is decrypted at Cloudflare's edge", "guide": null},
                         {"id": "tailscale", "title": "Tailscale Funnel", "note": null,
                          "guide": guide}]);
    let choose = remote_sheet(&mut app, Some(json!({"state": "choose", "tunnels": tunnels})));
    assert!(said(&choose).contains("traffic is decrypted at Cloudflare's edge"));
    assert!(choose.find_layer("guide cloudflared").is_none(), "no guide, no link");
    assert_eq!(
        choose.find_layer("guide tailscale").and_then(|g| g.action.clone()),
        Some(SheetAct::OpenGuide(guide.into()).into()),
        "the guide opens in the browser"
    );
    app.dispatch(SheetAct::RemotePick("tailscale".into()));
    assert!(wire.calls().is_empty(), "a pick sends nothing");
    assert!(choose.find_layer("btn Check").is_some());
    app.dispatch(SheetAct::RemoteUse);
    assert_eq!(
        wire.call().1,
        Call::Remote { act: json!({"act": "choose", "tunnel": "tailscale"}) }
    );

    // Its program is not found: its guide, and Check again. Weft installs nothing.
    let tunnel = json!({"id": "tailscale", "title": "Tailscale Funnel", "program": "tailscale",
                        "guide": guide});
    let missing = remote_sheet(&mut app, Some(json!({"state": "not_installed", "tunnel": tunnel})));
    assert!(said(&missing).contains(words::word("desktop.remote.not_found")));
    assert!(said(&missing).contains("Tailscale Funnel needs tailscale"), "{}", said(&missing));
    assert!(missing.find_layer("guide tailscale").is_some());
    assert!(missing.find_layer("btn Run it").is_none(), "nothing to run");
    app.dispatch(SheetAct::RemoteLook);
    assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "look"}) });

    // Signing in: the code and where; Cancel stops it.
    let signing = remote_sheet(
        &mut app,
        Some(json!({"state": "signing_in", "code": "7F3K-29QD",
        "url": "github.com/login/device"})),
    );
    assert!(
        said(&signing).contains("7F3K-29QD") && said(&signing).contains("github.com/login/device")
    );
    app.dispatch(SheetAct::RemoteCancel);
    assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "cancel"}) });
    let out =
        remote_sheet(&mut app, Some(json!({"state": "signed_out", "failed": "the code expired"})));
    assert!(said(&out).contains("the code expired"));
    app.dispatch(SheetAct::RemoteSignIn);
    assert_eq!(wire.call().1, Call::Remote { act: json!({"act": "sign_in"}) });

    // On: where, the tunnel, the devices; the Start at login warning only while it is off.
    let on = json!({"state": "on", "login": "octo", "address": "https://tide.trycloudflare.com",
        "tunnel": "up", "devices": []});
    app.clients[0].start_at_login = false;
    let shown = remote_sheet(&mut app, Some(on.clone()));
    assert!(
        said(&shown).contains("octo") && said(&shown).contains("https://tide.trycloudflare.com")
    );
    assert!(shown.find_layer("start at login warning").is_some());
    app.clients[0].start_at_login = true;
    assert!(remote_sheet(&mut app, Some(on)).find_layer("start at login warning").is_none());
}

#[test]
fn a_device_asking_opens_the_allow_bar_and_it_goes_with_the_request() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.bottom_bar = BottomBarState::WhyNot { sentence: "before".into() };
    let asking = json!({"state": "on", "devices": [],
        "asking": {"device": "d_9", "name": "iPad", "login": "octo"}});
    wire.event(Event::Remote { view: asking });
    pumped(&mut app, |a| matches!(a.bottom_bar, BottomBarState::AllowDevice { .. }));
    assert_eq!(
        app.bottom_bar,
        BottomBarState::AllowDevice {
            device_id: "d_9".into(),
            device_name: "iPad".into(),
            login: Some("octo".into())
        }
    );
    // Answered elsewhere: the bar goes, and what was there comes back.
    wire.event(Event::Remote { view: json!({"state": "on", "devices": []}) });
    pumped(&mut app, |a| !matches!(a.bottom_bar, BottomBarState::AllowDevice { .. }));
    assert_eq!(app.bottom_bar, BottomBarState::WhyNot { sentence: "before".into() });
}

#[test]
fn a_failed_step_shows_its_own_last_lines_under_its_row() {
    let (_tmp, mut app) = sample_app(false);
    let said = "Error: Plugin \"rf\" not found in marketplace \"fab7\"\nRun it again.";
    app.clients[0].setup = Some(SetupView {
        rows: vec![SetupRow {
            name: "claude".into(),
            title: "Claude Code".into(),
            program: "claude".into(),
            state: SetupState::Failed {
                step: "claude plugin install rf@fab7".into(),
                said: said.into(),
            },
            version: None,
        }],
        ..Default::default()
    });
    app.dispatch(WindowAct::OpenUpdates);
    let dom = app.render();
    let out = dom.find_layer("failed step output").expect("its lines").all_text();
    assert!(out.contains("not found in marketplace") && out.contains("Run it again."), "{out}");
    assert!(dom.find_layer("btn Set up claude").is_some(), "Set up runs it again");
}

#[test]
fn updates_opens_by_itself_once_when_nothing_is_set_up_and_something_could_be() {
    let (_tmp, mut app, mut wire) = wired_app();
    let row = |state| SetupRow {
        name: "claude".into(),
        title: "Claude Code".into(),
        program: "claude".into(),
        state,
        version: None,
    };
    let view = |state| SetupView { rows: vec![row(state)], ..Default::default() };
    // Ready: nothing opens.
    wire.event(Event::Setup {
        view: serde_json::to_value(view(SetupState::Ready { version: None })).unwrap(),
    });
    pumped(&mut app, |a| a.clients[0].setup.is_some());
    assert!(app.active_sheet.is_none());
    // Nothing ready and it could be set up: Updates opens.
    wire.event(Event::Setup { view: serde_json::to_value(view(SetupState::NotSetUp)).unwrap() });
    pumped(&mut app, |a| a.active_sheet.is_some());
    assert!(matches!(app.active_sheet, Some(ActiveSheet::Updates)));
    // Closed: it does not open again by itself.
    app.dispatch(SheetAct::Close);
    wire.event(Event::Setup { view: serde_json::to_value(view(SetupState::NotSetUp)).unwrap() });
    pumped(&mut app, |a| a.clients[0].setup.is_some());
    std::thread::sleep(Duration::from_millis(50));
    app.pump();
    assert!(app.active_sheet.is_none(), "once");
}

#[test]
fn closing_a_project_asks_first_in_the_bar() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(TreeAct::CloseProject(weft()));
    let bar = app.render().find_layer("Close project bar").expect("asked first").all_text();
    assert!(bar.contains(&words::fill("desktop.close.ask", &[("project", "weft")])), "{bar}");
    assert!(app.state.open_projects.contains(&weft()), "not closed yet");
    app.dispatch(BarAct::Cancel);
    assert!(app.state.open_projects.contains(&weft()));
    app.dispatch(TreeAct::CloseProject(weft()));
    app.dispatch(BarAct::ConfirmCloseProject(weft()));
    assert!(!app.state.open_projects.contains(&weft()));
}

/// Two agents in panes: the first's hook says it waits; the second's hook
/// says it works, and the daemon read it waiting from its screen.
fn two_waiting(app: &mut WeftDesktopApp, wire: &mut Wire) -> (u32, u32) {
    app.state.panes.groups.clear();
    let (a, b) = (start(app, wire, "claude"), start(app, wire, "codex"));
    let mut hooked = Row::of("claude", "s-a", State::Running, Some(Turn::Waiting), 2);
    hooked.pane = Some(a);
    let mut read = Row::of("codex", "s-b", State::Running, Some(Turn::Working), 1);
    read.pane = Some(b);
    // The sample's sessions went: only these two are carried.
    let gone = app.clients[0].sessions.iter().map(Row::key).collect();
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Waiting), Some(Turn::Waiting)],
        changed: vec![hooked, read],
        gone,
        earlier: 0,
        zone: 0,
    });
    pumped(app, |a| a.clients[0].sessions.len() == 2);
    (a, b)
}

#[test]
fn the_square_on_a_waiting_agent_says_plainly_why() {
    let (_tmp, mut app, mut wire) = wired_app();
    two_waiting(&mut app, &mut wire);
    app.dispatch(TreeAct::WhyWaiting { project: weft(), session: "s-a".into() });
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot {
            sentence: words::fill("desktop.why.asking", &[("harness", "claude")])
        }
    );
    app.dispatch(TreeAct::WhyWaiting { project: weft(), session: "s-b".into() });
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot {
            sentence: words::fill("desktop.why.looks_asking", &[("harness", "codex")])
        }
    );
    let rows = app.render();
    assert!(rows.find_layer("why waiting s-b").is_some(), "its ■ says why");
}

#[test]
fn needs_you_goes_to_each_waiting_agent_in_turn() {
    let (_tmp, mut app, mut wire) = wired_app();
    let (a, b) = two_waiting(&mut app, &mut wire);
    assert_eq!(app.active_tab().map(|t| t.pane), Some(b), "codex in front");
    app.dispatch(WindowAct::NextNeedsYou);
    assert_eq!(app.active_tab().map(|t| t.pane), Some(a));
    app.dispatch(WindowAct::NextNeedsYou);
    assert_eq!(app.active_tab().map(|t| t.pane), Some(b));
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Working), Some(Turn::Working)],
        changed: Vec::new(),
        gone: Vec::new(),
        earlier: 0,
        zone: 0,
    });
    pumped(&mut app, |a| a.clients[0].view(b).is_some_and(|v| v.turn == Some(Turn::Working)));
    app.dispatch(WindowAct::NextNeedsYou);
    assert_eq!(
        app.bottom_bar,
        BottomBarState::WhyNot { sentence: words::word("why.nothing_needs_you").to_string() }
    );
}

#[test]
fn a_selected_project_says_in_plain_words_where_its_acts_go() {
    let (_tmp, mut app) = sample_app(false);
    app.clients[0].routing =
        weft_core::routing::Routing::opened(&json!({"acts": {"ask": "claude", "eval": "codex"}}));
    assert!(app.render().find_layer("routing line").is_none(), "only while selected");
    app.dispatch(TreeAct::SelectProject(weft()));
    let line = app.render().find_layer("routing line").expect("said").all_text();
    assert_eq!(line, "Asks go to claude; Evals go to codex.");
}

#[test]
fn updates_does_not_open_on_a_view_still_being_asked() {
    let (_tmp, mut app, mut wire) = wired_app();
    let row = |name: &str, state| SetupRow {
        name: name.into(),
        title: name.into(),
        program: name.into(),
        state,
        version: None,
    };
    // The first look: one harness still answering, one not on this machine.
    let asking = SetupView {
        rows: vec![row("claude", SetupState::Checking), row("grok", SetupState::NotFound)],
        ..Default::default()
    };
    wire.event(Event::Setup { view: serde_json::to_value(asking).unwrap() });
    pumped(&mut app, |a| a.clients[0].setup.is_some());
    assert!(app.active_sheet.is_none(), "not until each has answered");
    let answered = SetupView {
        rows: vec![
            row("claude", SetupState::Ready { version: None }),
            row("grok", SetupState::NotFound),
        ],
        ..Default::default()
    };
    wire.event(Event::Setup { view: serde_json::to_value(answered).unwrap() });
    pumped(&mut app, |a| {
        a.clients[0].setup.as_ref().is_some_and(|v| v.rows[0].state != SetupState::Checking)
    });
    assert!(app.active_sheet.is_none(), "claude is ready");
}

#[test]
fn needs_you_says_so_when_the_one_waiting_runs_elsewhere() {
    let (_tmp, mut app, _wire) = wired_app();
    // The only one waiting runs in a program Weft does not show.
    app.clients[0].sessions = vec![Row::of("codex", "s-x", State::Running, Some(Turn::Waiting), 1)];
    assert_eq!(app.tree_projects()[0].project.needs, 1, "counted");
    app.dispatch(WindowAct::NextNeedsYou);
    let said = words::fill("why.elsewhere", &[("harness", "codex")]);
    assert_eq!(app.bottom_bar, BottomBarState::WhyNot { sentence: said });
}

#[test]
fn remotes_start_at_login_button_turns_it_on_as_the_daemon_has_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.settings.start_at_login = true;
    app.clients[0].start_at_login = false;
    let _ = remote_sheet(&mut app, Some(json!({"state": "on", "login": "octo", "devices": []})));
    app.dispatch(WindowAct::ToggleStartAtLogin);
    let (_, call) = wire.call();
    assert!(matches!(call, Call::StartAtLogin { on: true, .. }), "{call:?}");
}

#[test]
fn a_dialog_that_stops_an_eval_says_stop_eval_in_amber() {
    let (_tmp, mut app, mut wire) = wired_app();
    wire.event(Event::Pending {
        id: "p1".into(),
        pane: u32::MAX,
        what: words::word("dialog.stop_eval.title").into(),
        why: "The judges of Build account erasure stop, and the Eval is voided.".into(),
        payload: Vec::new(),
        sends: None,
        go: Some(weft_proto::Go { word: "dialog.stop_eval.go".into(), warn: true }),
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    let dom = app.render();
    let go = dom.find_layer("btn Stop Eval").expect("its own button");
    assert_eq!(go.all_text(), "Stop Eval");
    assert!(go.styles.iter().any(|(k, v)| k == "color" && v == palette::AMBER), "amber: {go:?}");
    assert!(dom.find_layer("btn Send").is_none(), "no Send on a dialog that types nothing");
    // It types into no agent, so it names none, and it is amber as its yes is.
    assert!(
        dom.find_layer("send to choice").is_none(),
        "no SEND TO on a dialog that types nothing"
    );
    let bar = dom.find_layer("Cancel / Send bar").unwrap();
    assert!(
        bar.styles.iter().any(|(k, v)| k == "border-top" && v.contains(palette::AMBER)),
        "{bar:?}"
    );
}

/// A dialog that types nothing names its work in its head, beside its
/// title, and says the rest in one line.
#[test]
fn a_dialog_that_types_nothing_names_its_work_in_its_head() {
    let (_tmp, mut app, mut wire) = wired_app();
    let why = "health endpoint · 2 Asks\nclaude checks the change against what you asked.\n\
               Nothing is typed into the agent's pane.";
    wire.event(Event::Pending {
        id: "p1".into(),
        pane: u32::MAX,
        what: words::word("dialog.eval.title").into(),
        why: why.into(),
        payload: Vec::new(),
        sends: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    let dom = app.render();
    let head = dom.find_layer("dialog head").expect("its head").all_text();
    assert_eq!(head, "Eval this work? health endpoint · 2 Asks");
    let bar = dom.find_layer("Cancel / Send bar").unwrap();
    let lines: Vec<&Element> = bar.children.iter().filter(|c| c.text.is_some()).collect();
    assert_eq!(lines.len(), 1, "the rest in one line: {lines:?}");
    assert_eq!(
        lines[0].text.as_deref(),
        Some(
            "claude checks the change against what you asked. Nothing is typed into the agent's pane."
        )
    );
}

/// The Seal's setup warning takes the acts' left, as the Ask's does; what
/// the Seal records moves to its own line above them.
#[test]
fn a_seal_with_a_setup_warning_has_it_beside_its_acts() {
    let (_tmp, mut app, mut wire) = wired_app();
    super::seal_tests::no_questions(&mut app, &mut wire);
    let records = "Nothing is typed.";
    let acts = app.render().find_layer("seal acts").expect("its acts").clone();
    assert!(acts.all_text().contains(records), "no warning: the note beside the acts");
    assert!(acts.find_layer("setup warning").is_none());

    app.act_warning = Some("codex is not set up for RingFrame.".into());
    let dom = app.render();
    let acts = dom.find_layer("seal acts").expect("its acts");
    let row: Vec<_> = acts.children.iter().map(|c| c.layer_name.clone()).collect();
    assert_eq!(row.first().cloned().flatten().as_deref(), Some("setup warning"), "{row:?}");
    assert!(acts.find_layer("btn Seal").is_some());
    assert!(!acts.all_text().contains(records), "{}", acts.all_text());
    let bar = dom.find_layer("Seal disposition bar").unwrap().all_text();
    assert!(bar.contains(records), "{bar}");
}

/// The exact prompt reads from its first line, each line wrapped in the box,
/// the route's command marked: one run of text, not words laid side by side.
#[test]
fn the_exact_prompt_reads_from_its_first_line_and_wraps() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    let typed =
        "/goal Read the build number\n\nRules:\n- KISS: keep it plain and short so anyone reads it";
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "Ask Claude Code?".into(),
        why: String::new(),
        payload: typed.as_bytes().to_vec(),
        sends: None,
        go: None,
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    let dom = app.render();
    let payload = dom.find_layer("payload").expect("the prompt");
    assert!(
        payload.children.iter().all(|c| c.tag != "span"),
        "no spans laid in a row: {payload:?}"
    );
    let text = payload.children.iter().find(|c| c.tag == "rich").expect("one run of text");
    assert_eq!(text.text.as_deref(), Some(typed));
    assert_eq!(text.runs.first().map(|(r, _)| r.clone()), Some(0..5), "/goal marked");
}

#[test]
fn a_device_asking_shows_over_a_staged_send() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "/rf:ask".into(),
        why: String::new(),
        payload: b"x".to_vec(),
        sends: None,
        go: None,
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    app.bottom_bar = BottomBarState::AllowDevice {
        device_id: "d_9".into(),
        device_name: "iPad".into(),
        login: None,
    };
    assert!(app.render().find_layer("Allow a device bar").is_some());
}

#[test]
fn remote_says_nothing_of_its_own_before_the_daemon_has_spoken() {
    let (_tmp, mut app) = sample_app(false);
    app.dispatch(WindowAct::OpenRemote);
    let sheet = app.render().find_layer("sheet Remote").unwrap().all_text();
    assert!(sheet.contains(words::word("web.link.connecting")), "{sheet}");
    assert!(!sheet.contains(words::word("desktop.remote.off")), "{sheet}");
    assert!(!sheet.contains(words::word("desktop.remote.turn_on")), "{sheet}");
}

#[test]
fn send_to_is_a_list_of_its_agents_then_a_new_one_of_each_harness() {
    let (_tmp, mut app, mut wire) = wired_app();
    let (_, codex) = two_waiting(&mut app, &mut wire);
    app.dispatch(TreeAct::SelectProject(weft()));
    app.dispatch(BarAct::Ask);
    // Closed: the one it goes to, and ▾.
    let choice = app.render().find_layer("send to choice").expect("closed").all_text();
    assert_eq!(choice, "weft · codex ▾");
    app.dispatch(SheetAct::ToggleSendTo);
    let list = app.render().find_layer("send to list").expect("open").clone();
    let text = list.all_text();
    assert!(text.contains("weft · claude") && text.contains("weft · codex ✓"), "{text}");
    assert!(text.contains(&words::word("start.title").to_uppercase()), "{text}");
    assert!(list.find_layer("send to start:grok").is_some(), "{text}");
    // Open, it scrolls with what is between the words and the acts: the
    // acts stay in view.
    let dom = app.render();
    let sheet = dom.find_layer("sheet Ask").unwrap();
    let body = sheet.find_layer("ask body").expect("what scrolls");
    assert!(body.styles.iter().any(|(k, v)| k == "overflow-y" && v == "auto"), "{body:?}");
    assert!(body.find_layer("send to list").is_some());
    assert!(body.find_layer("btn Ask").is_none() && sheet.find_layer("btn Ask").is_some());
    // Picking is a choice: it closes the list, and sends nothing.
    app.dispatch(SheetAct::AskTo("start:grok".into()));
    // Only the new harness's sheet is read.
    let acts = wire.calls().into_iter().filter(|(_, c)| !matches!(c, Call::Read { .. })).count();
    assert_eq!(acts, 0);
    assert_eq!(app.render().find_layer("send to choice").unwrap().all_text(), "grok ▾");
    app.ask.set_text("add a health endpoint");
    app.dispatch(SheetAct::SubmitAsk);
    let (_, call) = wire.call();
    assert!(
        matches!(&call, Call::Act { pane: None, start: Some(h), .. } if h == "grok"),
        "{call:?}"
    );
    let _ = codex;
}

#[test]
fn a_staged_send_goes_where_staged_or_where_sent_to() {
    let (_tmp, mut app, mut wire) = wired_app();
    let (claude, codex) = two_waiting(&mut app, &mut wire);
    let stage = |wire: &mut Wire, id: &str| {
        wire.event(Event::Pending {
            id: id.into(),
            pane: codex,
            what: "/rf:ask".into(),
            why: String::new(),
            payload: b"x".to_vec(),
            sends: None,
            go: None,
        })
    };
    stage(&mut wire, "p1");
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    assert_eq!(app.render().find_layer("send to choice").unwrap().all_text(), "weft · codex ▾");
    // Into another agent.
    app.dispatch(BarAct::SendTo { pending: "p1".into(), to: format!("pane:{claude}") });
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let (_, call) = wire.call();
    assert!(
        matches!(call, Call::Resolve { to: Some(p), start: None, .. } if p == claude),
        "{call:?}"
    );
    // Into a new one.
    stage(&mut wire, "p2");
    pumped(&mut app, |a| a.clients[0].waiting.iter().any(|s| s.id == "p2"));
    app.dispatch(BarAct::SendTo { pending: "p2".into(), to: "start:grok".into() });
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p2".into(), answer: Answer::Yes });
    let (_, call) = wire.call();
    assert!(
        matches!(&call, Call::Resolve { to: None, start: Some(h), .. } if h == "grok"),
        "{call:?}"
    );
}

/// ☰ › Turbo turns the machine's turbo in `config.toml`, through the daemon;
/// the switch shows what the daemon then says, not what the window kept.
#[test]
fn the_turbo_switch_sets_the_machine_s_turbo_through_the_daemon() {
    let (_tmp, mut app, mut wire) = wired_app();
    assert!(!app.turbo());
    app.dispatch(WindowAct::ToggleTurbo);
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: vec!["turbo".into()],
            value: Some(json!(true))
        }
    );
    assert!(!app.turbo(), "not until the daemon says so");
    wire.event(Event::Config { config: json!({"machine": {"turbo": true}, "project": null}) });
    pumped(&mut app, |a| a.turbo());
}

/// ☰ › Settings… and its shortcut open Settings on General; each switch is
/// the machine's, set through the daemon, and shows what the daemon says.
#[test]
fn settings_opens_on_general_and_its_switches_set_the_machine_s_settings() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config = Some(json!({"machine": {"turbo": false}, "project": null}));
    app.dispatch(WindowAct::ToggleMenu);
    assert!(app.render().find_layer("menu Settings").is_some(), "in ☰");
    app.dispatch(WindowAct::OpenSettings);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::Settings { tab: SettingsTab::General })));
    let dom = app.render();
    let sheet = dom.find_layer("sheet Settings").expect("the Settings sheet");
    assert!(sheet.find_layer("settings tab General").is_some());
    let badge = |row: &str| {
        sheet
            .find_layer(row)
            .unwrap_or_else(|| panic!("{row}"))
            .find_layer("setting badge")
            .unwrap()
            .all_text()
    };
    assert_eq!(badge("setting Turbo"), "OFF");
    assert_eq!(badge("setting Start at login"), "OFF");
    assert_eq!(badge("setting Notifications"), "ON", "on until turned off");
    assert!(sheet.find_layer("setting Suggestions").is_none(), "no prompt suggestions");

    app.dispatch(SheetAct::Toggle(Setting::Notify));
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: vec!["notify".into()],
            value: Some(json!(false))
        }
    );
    wire.event(Event::Config { config: json!({"machine": {"notify": false}}) });
    pumped(&mut app, |a| {
        a.clients[0].config.as_ref().is_some_and(|c| c["machine"]["notify"] == false)
    });
    let dom = app.render();
    let sheet = dom.find_layer("sheet Settings").unwrap();
    let said =
        |row: &str| sheet.find_layer(row).unwrap().find_layer("setting badge").unwrap().all_text();
    assert_eq!(said("setting Notifications"), "OFF");
}

/// Workflow: who runs each step and Eval's judges, every project's; a
/// choice lists "The agent you are in" and each harness; a role's model is
/// typed and kept on Enter.
#[test]
fn workflow_sets_every_project_s_routing_and_debate() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config = Some(json!({"machine": {"routing": {"eval": "claude"},
        "eval": {"debate": {"harness": "codex", "read": {"effort": "low"}}}}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::Workflow));
    app.dispatch(SheetAct::ShowJudges);
    let dom = app.render();
    let sheet = dom.find_layer("sheet Settings").unwrap();
    let value = |row: &str| {
        sheet
            .find_layer(row)
            .unwrap_or_else(|| panic!("{row}"))
            .find_layer("setting value")
            .unwrap()
            .all_text()
    };
    assert_eq!(
        value("setting Ask"),
        format!("{} ▾", words::word("desktop.settings.agent_you_are_in"))
    );
    assert_eq!(value("setting Eval"), "Claude Code ▾");
    assert_eq!(value("setting Judges run in"), "Codex ▾");
    assert_eq!(value("setting Read effort"), "low ▾");

    let key = |k: &[&str]| k.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    app.dispatch(SheetAct::OpenChoice(key(&["routing", "seal"])));
    let dom = app.render();
    let list = dom.find_layer("choices routing.seal").expect("the choices");
    for option in [
        "choice The agent you are in",
        "choice Antigravity",
        "choice Claude Code",
        "choice Codex",
        "choice Grok Build",
    ] {
        assert!(list.find_layer(option).is_some(), "{option}");
    }
    app.dispatch(SheetAct::Choose {
        project: None,
        key: key(&["routing", "seal"]),
        value: Some(json!("codex")),
    });
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: key(&["routing", "seal"]),
            value: Some(json!("codex"))
        }
    );
    assert!(app.render().find_layer("choices routing.seal").is_none(), "closed once chosen");
    app.dispatch(SheetAct::Choose { project: None, key: key(&["routing", "eval"]), value: None });
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet { scope: "machine".into(), key: key(&["routing", "eval"]), value: None }
    );

    app.dispatch(SheetAct::EditSetting {
        project: None,
        key: key(&["eval", "debate", "question", "model"]),
    });
    assert_eq!(app.focus, Focus::Setting);
    app.setting_field.paste("gpt-6-luna");
    app.dispatch(SheetAct::CommitSetting);
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: key(&["eval", "debate", "question", "model"]),
            value: Some(json!("gpt-6-luna"))
        }
    );
    assert_eq!(app.focus, Focus::Terminal);
}

/// Eval's readers: how hard they look is one choice for both, what an
/// effort changes is the harness's, and each judge, once opened, shows its
/// default effort by its value.
#[test]
fn eval_s_judges_look_as_hard_as_chosen_and_show_their_defaults() {
    let (_tmp, mut app, mut wire) = wired_app();
    let efforts = |e: &str| json!({"read": {"effort": e}, "question": {"effort": e}});
    let mut debate = efforts("low");
    debate["harness"] = json!("codex");
    app.clients[0].config = Some(json!({"machine": {"eval": {"debate": debate}}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::Workflow));
    let dom = app.render();
    let hard = dom.find_layer("setting How hard").expect("how hard");
    assert_eq!(hard.find_layer("how hard on").unwrap().all_text(), "Quick");
    assert!(hard.all_text().contains("In Codex, this sets each judge's reasoning"));
    assert!(dom.find_layer("setting Read effort").is_none(), "each reader is closed until opened");

    app.dispatch(SheetAct::HowHard("thorough".into()));
    for role in ["read", "question"] {
        let (_, call) = wire.call();
        let key = ["eval", "debate", role, "effort"].map(String::from).to_vec();
        assert_eq!(
            call,
            Call::ConfigSet { scope: "machine".into(), key, value: Some(json!("high")) }
        );
    }
    app.dispatch(SheetAct::HowHard("balanced".into()));
    let (_, call) = wire.call();
    assert!(matches!(call, Call::ConfigSet { value: None, .. }), "Balanced takes them out");

    app.clients[0].config =
        Some(json!({"machine": {"eval": {"debate": {"harness": "codex"}}}, "project": null}));
    app.dispatch(SheetAct::ShowJudges);
    let dom = app.render();
    let value = |row: &str| {
        dom.find_layer(row)
            .unwrap_or_else(|| panic!("{row}"))
            .find_layer("setting value")
            .unwrap()
            .all_text()
    };
    assert_eq!(
        dom.find_layer("setting How hard").unwrap().find_layer("how hard on").unwrap().all_text(),
        "Balanced"
    );
    assert_eq!(value("setting Questions effort"), "medium · default ▾");
    assert_eq!(
        value("setting Read model"),
        words::word("desktop.settings.own_model"),
        "typed in Codex"
    );
}

/// A project's own settings, from its row's ⋯: only what differs from every
/// project's, each beside every project's value, with Reset; and more added.
#[test]
fn a_project_s_own_settings_list_what_differs_and_reset_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    let base = "practices/software-development";
    app.clients[0].config = Some(json!({
        "machine": {"routing": {"eval": "claude"}},
        "project": {"routing": {"eval": "codex"},
                    "ringframe": {"deltas": {base: {"entries": [{"id": "practice.kiss", "tier": "situational"}]}}}}}));
    app.dispatch(TreeAct::ToggleProjectMenu(weft()));
    assert!(app.render().find_layer("project menu Settings").is_some());
    app.dispatch(TreeAct::OpenProjectSettings(weft()));
    assert!(
        matches!(&app.active_sheet, Some(ActiveSheet::ProjectSettings { project }) if *project == weft())
    );
    // Its rules as this project has them, and every project's beside them.
    let rules = |tier: &str| {
        json!({"domains": [], "catalogs": [{"catalog": base, "render": {"core_cap": 5}, "entries": [
        {"id": "practice.kiss", "label": "KISS", "tier": tier, "source": "weft-project"},
        {"id": "practice.yagni", "label": "YAGNI", "tier": "core", "source": "shipped"}]}]})
    };
    for _ in 0..2 {
        let (id, call) = wire.call();
        let Call::Read { unit, .. } = call else { panic!("a read: {call:?}") };
        wire.answer(id, rules(if unit == "project" { "situational" } else { "core" }));
    }
    pumped(&mut app, |a| a.clients[0].read.len() >= 2);
    let dom = app.render();
    let sheet = dom.find_layer("sheet Project settings").expect("the project's settings");
    let row = sheet.find_layer("project setting routing.eval").expect("Eval differs here");
    assert_eq!(row.find_layer("setting value").unwrap().all_text(), "Codex ▾");
    assert!(
        row.all_text().contains("Claude Code"),
        "every project's value beside it: {}",
        row.all_text()
    );
    assert!(sheet.find_layer("project setting routing.ask").is_none(), "only what differs");
    let kiss =
        sheet.find_layer("project setting ringframe.practice.kiss").expect("KISS differs here");
    assert_eq!(kiss.find_layer("setting value").unwrap().all_text(), "situational, on ▾");
    assert!(kiss.all_text().contains("core, on"), "every project's beside it: {}", kiss.all_text());
    assert!(sheet.find_layer("project setting ringframe.practice.yagni").is_none());

    let key = vec!["routing".to_string(), "eval".to_string()];
    app.dispatch(SheetAct::Choose { project: Some(weft()), key: key.clone(), value: None });
    let (_, call) = wire.call();
    assert_eq!(call, Call::ConfigSet { scope: "project".into(), key, value: None });
    let kiss_key: Vec<String> =
        ["ringframe", "deltas", base, "entries", "practice.kiss"].map(String::from).to_vec();
    app.dispatch(SheetAct::Choose { project: Some(weft()), key: kiss_key, value: None });
    let (_, call) = wire.call();
    let entries: Vec<String> = ["ringframe", "deltas", base, "entries"].map(String::from).to_vec();
    assert_eq!(call, Call::ConfigSet { scope: "project".into(), key: entries, value: None });

    app.dispatch(SheetAct::AddProjectSetting);
    let dom = app.render();
    let list = dom.find_layer("choices add").expect("what can be set here");
    assert!(list.find_layer("choice Ask").is_some() && list.find_layer("choice Seal").is_some());
    assert!(list.find_layer("choice RingFrame · YAGNI").is_some(), "a rule not set here");
    assert!(
        list.find_layer(&format!("choice {}", words::word("desktop.settings.core_cap"))).is_some()
    );
    assert!(list.find_layer("choice Turbo").is_none(), "Turbo is every project's only");
}

/// Many rules: the first five, then how many more; asked, every one.
#[test]
fn ringframe_folds_its_rules_after_five() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::RingFrame));
    let (id, _) = wire.call();
    let entries: Vec<Value> = (0..8)
        .map(|i| json!({"id": format!("r{i}"), "text": "A rule.", "source": "shipped"}))
        .collect();
    wire.answer(
        id,
        json!({"domains": [], "catalogs": [
        {"catalog": "practices/software-development", "entries": entries}]}),
    );
    pumped(&mut app, |a| {
        a.clients[0].read.contains_key(&crate::daemon::Then::Rules { project: false })
    });
    let dom = app.render();
    assert!(dom.find_layer("rule r4").is_some() && dom.find_layer("rule r5").is_none());
    assert_eq!(dom.find_layer("more rules").unwrap().all_text(), "+ 3 more");
    app.dispatch(SheetAct::AllRules("practices/software-development".into()));
    let dom = app.render();
    assert!(dom.find_layer("rule r7").is_some() && dom.find_layer("more rules").is_none());
}

/// Each practice turned on lists its rules below the base practice's, with
/// the same controls, each change written to that practice's catalog; one
/// turned off lists none.
#[test]
fn ringframe_lists_the_rules_of_each_practice_turned_on() {
    let (_tmp, mut app, mut wire) = wired_app();
    let trading = "practices/autonomous-trading";
    app.clients[0].config = Some(json!({"machine": {"ringframe": {"deltas": {
        trading: {"domain": "autonomous-trading"}}}}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::RingFrame));
    let (id, _) = wire.call();
    let many: Vec<Value> = (0..8)
        .map(|i| json!({"id": format!("trading.r{i}"), "label": format!("R{i}"), "source": "shipped"}))
        .collect();
    wire.answer(
        id,
        json!({
        "domains": [{"domain": "software-development", "base": true},
                    {"domain": "autonomous-trading", "base": false},
                    {"domain": "machine-learning", "base": false}],
        "catalogs": [
            {"catalog": "practices/software-development", "entries": [
                {"id": "practice.kiss", "label": "KISS", "source": "shipped"},
                {"id": "own.1", "label": "Mine", "source": "weft"}]},
            {"catalog": trading, "entries": many},
            {"catalog": "practices/machine-learning", "entries": [
                {"id": "ml.baseline", "label": "Baseline", "source": "shipped"}]}]}),
    );
    pumped(&mut app, |a| {
        a.clients[0].read.contains_key(&crate::daemon::Then::Rules { project: false })
    });
    let dom = app.render();
    let head = dom.find_layer(&format!("rules head {trading}")).expect("its rules' head");
    assert!(head.all_text().contains("AUTONOMOUS TRADING · RULES"), "{}", head.all_text());
    assert!(dom.find_layer("rules practices/machine-learning").is_none(), "turned off");
    let listed = dom.find_layer(&format!("rules {trading}")).expect("its rules");
    assert!(listed.find_layer("rule trading.r4").is_some());
    assert!(listed.find_layer("rule trading.r5").is_none(), "folded after five");
    let base = dom.find_layer("rules practices/software-development").unwrap();
    assert!(base.find_layer("more rules").is_none(), "the base's own fold");

    app.dispatch(SheetAct::AllRules(trading.into()));
    let dom = app.render();
    let listed = dom.find_layer(&format!("rules {trading}")).unwrap();
    assert!(
        listed.find_layer("rule trading.r7").is_some() && listed.find_layer("more rules").is_none()
    );

    // Its switch writes its own catalog.
    let switch = listed.find_layer("rule trading.r0").unwrap().find_layer("rule switch").unwrap();
    app.dispatch(switch.action.clone().expect("a switch"));
    let call = loop {
        match wire.call() {
            (_, Call::Read { .. }) => continue,
            (_, call) => break call,
        }
    };
    let k = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: k(&["ringframe", "deltas", trading, "entries"]),
            value: Some(json!([{"id": "trading.r0", "enabled": false}])),
        }
    );
    // A rule of the person's own goes into that practice, its id one no
    // practice holds, typed under its rules only.
    let add = head.find_layer("add rule").unwrap().action.clone().expect("+ Add a rule");
    app.dispatch(add);
    let editing = app.setting_editing.clone().map(|(_, k)| k);
    assert_eq!(editing, Some(k(&["ringframe", "deltas", trading, "entries", "own.2", "text"])));
    let dom = app.render();
    let listed = dom.find_layer(&format!("rules {trading}")).unwrap();
    assert!(listed.find_layer("setting value").is_some(), "typed here");
    let base = dom.find_layer("rules practices/software-development").unwrap();
    assert!(base.find_layer("setting value").is_none(), "not under the base's");
}

/// RingFrame would not say its rules: the sheet says why, asks nothing more
/// while it stays open, and asks again when Settings opens again.
#[test]
fn a_refused_settings_read_says_why_and_is_asked_again_on_opening() {
    let (_tmp, mut app, mut wire) = wired_app();
    let rules = crate::daemon::Then::Rules { project: false };
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::RingFrame));
    let (id, _) = wire.call();
    wire.refuse(id, "config.absent: no configuration");
    pumped(&mut app, |a| a.clients[0].refused.contains_key(&rules));
    assert!(app.render().all_text().contains("RingFrame did not answer: config.absent"));
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Read { what, .. } if what == "preview"));
    wire.refuse(id, "config.absent: no configuration");
    pumped(&mut app, |a| a.clients[0].refused.len() == 2);

    wire.event(Event::Turbo { on: true });
    pumped(&mut app, |a| a.clients[0].turbo);
    assert!(!app.clients[0].owes(&rules), "not asked again while the sheet stays open");

    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::RingFrame));
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Read { what, .. } if what == "rules"));
}

/// RingFrame: the practices, the cap, each rule with where it comes from, what
/// judges must not do, and a preview; each change written as RingFrame reads
/// it, a rule as the list that holds it.
#[test]
fn ringframe_shows_the_rules_and_writes_each_change_as_ringframe_reads_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    let base = "practices/software-development";
    app.clients[0].config = Some(json!({"machine": {"ringframe": {"deltas": {base: {
        "entries": [{"id": "practice.kiss", "enabled": false}]}}}}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::RingFrame));
    let (id, call) = wire.call();
    assert_eq!(
        call,
        Call::Read {
            what: "rules".into(),
            unit: "machine".into(),
            pane: None,
            lines: None,
            before: None,
            after: None,
            q: None,
        }
    );
    wire.answer(id, json!({
        "domains": [{"domain": "software-development", "base": true}, {"domain": "autonomous-trading", "base": false, "description": "Trading"}],
        "catalogs": [{"catalog": base, "render": {"core_cap": 5}, "entries": [
            {"id": "practice.kiss", "label": "KISS", "text": "Keep it simple.", "tier": "core", "enabled": false, "source": "weft", "applies_to": ["implement"]},
            {"id": "practice.yagni", "label": "YAGNI", "text": "Only what is needed.", "tier": "core", "source": "shipped"}]}],
        "judges": {"deny": ["Push to any remote."], "from": "default"}}));
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Read { ref what, ref unit, .. }
        if what == "preview" && unit == "antigravity|native_plan|implement"));
    wire.answer(id, json!({"text": "Rules:\n- KISS: Keep it simple.\n", "kept": 5, "left_out": 2}));
    pumped(&mut app, |a| a.clients[0].read.len() == 2);
    let dom = app.render();
    assert_eq!(
        dom.find_layer("preview kept").unwrap().all_text(),
        "5 core rules kept · 2 left out by the cap"
    );
    assert!(dom.find_layer("preview text").is_none(), "the text waits for Show");
    let own = dom.find_layer("deny 0").expect("RingFrame's own list, listed");
    assert!(own.all_text().contains(words::word("desktop.settings.shipped")));
    let kiss = dom.find_layer("rule practice.kiss").expect("KISS");
    assert!(
        kiss.all_text().contains(words::word("desktop.settings.changed")),
        "{}",
        kiss.all_text()
    );
    assert_eq!(kiss.find_layer("setting badge").unwrap().all_text(), "OFF");
    assert!(dom.find_layer("rule practice.yagni").unwrap().find_layer("edit").is_some());

    let k = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let set = |w: &mut Wire, key: Vec<String>, value: Option<Value>| {
        let (_, call) = w.call();
        assert_eq!(call, Call::ConfigSet { scope: "machine".into(), key, value });
    };
    app.dispatch(SheetAct::Choose {
        project: None,
        key: k(&["ringframe", "deltas", base, "entries", "practice.kiss", "enabled"]),
        value: Some(json!(true)),
    });
    set(
        &mut wire,
        k(&["ringframe", "deltas", base, "entries"]),
        Some(json!([{"id": "practice.kiss", "enabled": true}])),
    );
    app.dispatch(SheetAct::Choose {
        project: None,
        key: k(&["ringframe", "deltas", base, "entries", "practice.kiss"]),
        value: None,
    });
    set(&mut wire, k(&["ringframe", "deltas", base, "entries"]), None);
    app.dispatch(SheetAct::Choose {
        project: None,
        key: k(&["ringframe", "deltas", "practices/autonomous-trading"]),
        value: Some(json!({"domain": "autonomous-trading"})),
    });
    set(
        &mut wire,
        k(&["ringframe", "deltas", "practices/autonomous-trading"]),
        Some(json!({"domain": "autonomous-trading"})),
    );

    app.dispatch(SheetAct::EditSetting {
        project: None,
        key: k(&["ringframe", "deltas", base, "entries", "own.1", "text"]),
    });
    app.setting_field.paste("Say it plainly.");
    app.dispatch(SheetAct::CommitSetting);
    set(
        &mut wire,
        k(&["ringframe", "deltas", base, "entries"]),
        Some(json!([
        {"id": "practice.kiss", "enabled": false},
        {"id": "own.1", "text": "Say it plainly.", "applies_to": ["implement"], "tier": "situational"}])),
    );

    app.dispatch(SheetAct::EditSetting {
        project: None,
        key: k(&["ringframe", "eval", "deny", "+"]),
    });
    app.setting_field.paste("Delete files.");
    app.dispatch(SheetAct::CommitSetting);
    set(
        &mut wire,
        k(&["ringframe", "eval", "deny"]),
        Some(json!(["Push to any remote.", "Delete files."])),
    );

    app.dispatch(SheetAct::ShowPreview);
    assert!(app.render().find_layer("preview text").unwrap().all_text().contains("KISS"));
}

/// Harnesses: inspect and tune, never add. A harness shows how RingFrame
/// sends to it and its route rules; a rule's status is switched, written as
/// the list RingFrame reads.
#[test]
fn harnesses_show_how_ringframe_sends_and_switch_a_route_rule_s_status() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config = Some(json!({"machine": {}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::Harnesses));
    let mut asked = Vec::new();
    while asked.len() < 2 {
        let (id, call) = wire.call();
        match call {
            Call::Read { what, unit, .. } if what == "profile" => {
                assert_eq!(unit, "antigravity", "the first harness");
                wire.answer(id, json!({"capabilities": [
                    {"id": "native_plan", "delivery_mode": "native_dispatch",
                     "selection": "A plan or analysis before implementation.", "purpose": "Plans first"},
                    {"id": "native_goal", "delivery_mode": "human_handoff", "max_prompt_chars": 4000,
                     "purpose": "Works on its own until it marks the goal done"}],
                    "paste_fold_chars": 512}));
                asked.push("profile");
            }
            Call::Read { what, .. } if what == "rules" => {
                wire.answer(id, json!({"domains": [], "catalogs": [{"catalog": "antigravity", "entries": [
                    {"id": "agy.plan_as_files", "label": "Plan As Files", "text": "Draft the plan.", "status": "qualified", "source": "shipped"}]}]}));
                asked.push("rules");
            }
            _ => {}
        }
    }
    pumped(&mut app, |a| a.clients[0].read.len() >= 2);
    let dom = app.render();
    for layer in
        ["harness Antigravity", "harness Claude Code", "harness Codex", "harness Grok Build"]
    {
        assert!(dom.find_layer(layer).is_some(), "{layer}");
    }
    let goal = dom.find_layer("sends native_goal").expect("Goal");
    assert!(
        goal.all_text().contains(words::word("desktop.harness.you_send")),
        "{}",
        goal.all_text()
    );
    assert!(goal.all_text().contains("4000"), "{}", goal.all_text());
    assert!(
        goal.all_text().contains("Works on its own until it marks the goal done"),
        "its purpose, and its limit"
    );
    let plan = dom.find_layer("sends native_plan").unwrap().all_text();
    assert!(
        plan.contains("Plans first") && !plan.contains("before implementation"),
        "for people, not the model: {plan}"
    );
    assert!(dom.find_layer("sends folds").unwrap().all_text().contains("512"));
    let rule = dom.find_layer("route rule agy.plan_as_files").expect("its route rule");
    assert!(rule.find_layer("status desktop.harness.candidate").is_some());

    app.dispatch(SheetAct::Choose {
        project: None,
        key: ["ringframe", "deltas", "antigravity", "entries", "agy.plan_as_files", "status"]
            .map(String::from)
            .to_vec(),
        value: Some(json!("candidate")),
    });
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: ["ringframe", "deltas", "antigravity", "entries"].map(String::from).to_vec(),
            value: Some(json!([{"id": "agy.plan_as_files", "status": "candidate"}])),
        }
    );
    app.dispatch(SheetAct::ShowHarness("codex".into()));
    loop {
        let (_, call) = wire.call();
        if let Call::Read { what, unit, .. } = call {
            assert_eq!((what.as_str(), unit.as_str()), ("profile", "codex"));
            break;
        }
    }
}

#[test]
fn a_shown_conversation_reads_what_its_file_added_each_second_one_read_at_a_time() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "claude");
    let key = (weft(), pane);
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, _) = wire.call();
    wire.answer(id, json!({"conversation": {"items": [], "after": "1:10"}}));
    pumped(&mut app, |a| a.conversations.contains_key(&key));

    // The person's message reaches the file after the agent moved: with
    // nothing else asking, the shown conversation reads what was added.
    let later = std::time::Instant::now() + Duration::from_secs(2);
    app.follow_conversations(later);
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Read { ref after, .. } if after.as_deref() == Some("1:10")));
    app.follow_conversations(later + Duration::from_secs(5));
    assert!(wire.calls().is_empty(), "one read at a time");
    let said = vec![item("10.0", Kind::Person, None, Some("also add a test"))];
    wire.answer(id, json!({"conversation": {"items": said, "after": "1:40"}}));
    pumped(&mut app, |a| a.conversations[&key].len() == 1);
    app.follow_conversations(later + Duration::from_millis(5500));
    assert!(wire.calls().is_empty(), "not again within the second");
    app.follow_conversations(later + Duration::from_secs(6));
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Read { ref after, .. } if after.as_deref() == Some("1:40")));

    // In Terminal, nothing is read.
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Terminal });
    wire.calls();
    app.follow_conversations(later + Duration::from_secs(20));
    assert!(wire.calls().is_empty(), "a conversation not shown is not read");
}

#[test]
fn a_terminal_scrolled_back_shows_the_way_to_its_end_and_one_press_goes_there() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    let lines: String = (0..200).map(|n| format!("line {n}\r\n")).collect();
    app.pane_view_mut(&weft(), pane).unwrap().feed(lines.as_bytes());
    assert!(app.render().find_layer("scroll to end").is_none(), "at its end, none");
    app.pane_view_mut(&weft(), pane).unwrap().scroll(10);
    let button =
        app.render().find_layer("scroll to end").cloned().expect("scrolled back, the button");
    assert_eq!(button.action, Some(PaneAct::ToEnd { project: weft(), pane }.into()));
    app.dispatch(PaneAct::ToEnd { project: weft(), pane });
    assert_eq!(app.pane_view_mut(&weft(), pane).unwrap().scroll_offset(), 0);
    assert!(app.render().find_layer("scroll to end").is_none(), "at its end again, none");
}
