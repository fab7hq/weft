use std::io::Write as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use weft_core::conversation::{Item, Kind};
use weft_core::harness::{Harness, Harnesses};
use weft_core::ledger::{Check, Sent, Unit, Verdict};
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
        conversation: None,
    };
    Harnesses(vec![
        mk("antigravity", "Antigravity", "agy"),
        mk("claude", "Claude Code", "claude"),
        mk("codex", "Codex", "codex"),
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
    let check = Check {
        eval_id: "evl_3".into(),
        verdict: Verdict::Matches,
        agreement: 0.67,
        judged_by: None,
    };
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
    for i in 0..14 {
        c.sessions.push(Row::of("claude", &format!("old_{i}"), State::Ended, None, 800 - i));
    }

    // health endpoint judged by Eval 3; build number sealed by Seal 4 on Eval 2.
    let record = weft_core::work::record(&[
        json!({"type": "eval.opened", "id": "evl_2", "time": "2026-10-08T11:06:00Z",
               "data": {"basis": {"asks": ["ask_4"]}}}),
        json!({"type": "eval.completed", "id": "evl_2", "time": "2026-10-08T11:07:00Z",
               "data": {"basis": {"asks": ["ask_4"]}, "verdict": "aligned", "confidence": 1.0}}),
        json!({"type": "seal.created", "id": "sel_4", "time": "2026-10-08T11:08:00Z",
               "actor": {"kind": "human"}, "links": [{"rel": "seals", "id": "evl_2"}],
               "data": {"disposition": "accepted"}}),
        json!({"type": "eval.opened", "id": "evl_3", "time": "2026-10-08T11:11:00Z",
               "data": {"basis": {"asks": ["ask_3"]}}}),
        json!({"type": "eval.completed", "id": "evl_3", "time": "2026-10-08T11:12:00Z",
               "data": {"basis": {"asks": ["ask_3"]}, "verdict": "aligned", "confidence": 0.67}}),
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
            after: None
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
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::Act {
            act: "ask".into(),
            unit: None,
            pane: Some(pane),
            text: Some("refresh token 60s before expiry".into()),
            eval: None,
            start: None
        }
    );
    // Nothing waits on the person until the daemon stages it.
    assert!(app.render().find_layer("Cancel / Send bar").is_none());
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "/rf:ask".into(),
        why: "ask".into(),
        payload: Vec::new(),
        sends: None,
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    let bar = app.render();
    assert!(bar.find_layer("btn Send").is_some());
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

    // A refusal is said in the bar.
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    let (id, call) = wire.call();
    assert!(matches!(call, Call::Act { ref act, .. } if act == "eval"));
    wire.refuse(id, "no agent can run it");
    pumped(&mut app, |a| {
        a.bottom_bar == BottomBarState::WhyNot { sentence: "no agent can run it".into() }
    });
}

/// WORK is a row per unit of work with one status; a unit opens as its
/// workflow, and its Eval and Seal name it.
#[test]
fn a_unit_of_work_opens_as_its_workflow_and_its_acts_name_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    let tree = app.render().find_layer("tree").unwrap().all_text();
    for said in ["asked", "asking", "aligned · 0.67", "sealed · accepted"] {
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

    // Seal decides on the unit's Eval; Eval names the unit.
    app.dispatch(SheetAct::SealWork { project: weft(), unit: "ask_3".into() });
    assert!(matches!(
        &app.bottom_bar,
        BottomBarState::SealDisposition { eval, .. } if eval == "evl_3"
    ));
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

/// What the app keeps per agent, its conversation, draft and suggestion, and
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
    };
    app.clients[0].waiting = vec![staged("pending_1")];
    let elsewhere = PathBuf::from("/work/closed");
    for key in [(weft(), 1), (weft(), 7), (elsewhere.clone(), 1)] {
        app.conversations.insert(key.clone(), Vec::new());
        app.messages.insert(key.clone(), TextField::new(Shape::Line));
        app.suggestions.insert(key, (String::new(), String::new()));
    }
    app.staged_to.insert("pending_1".into(), "2".into());
    app.staged_to.insert("pending_gone".into(), "2".into());
    app.drop_gone_tabs();
    let kept = [(weft(), 1)];
    assert_eq!(app.conversations.keys().cloned().collect::<Vec<_>>(), kept);
    assert_eq!(app.messages.keys().cloned().collect::<Vec<_>>(), kept);
    assert_eq!(app.suggestions.keys().cloned().collect::<Vec<_>>(), kept);
    assert_eq!(app.staged_to.keys().collect::<Vec<_>>(), ["pending_1"]);
    app.close_project(&weft());
    assert!(app.conversations.is_empty() && app.messages.is_empty());
    assert!(app.suggestions.is_empty() && app.staged_to.is_empty());
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
    let before = app.render().find_layer("sheet WorkDetail").unwrap().all_text();
    assert!(!before.contains("the prompt as typed"), "nothing typed shown before it is read");
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
    assert!(sheet.contains(words::word("desktop.seal.unchecked")), "{sheet}");
    assert!(!sheet.contains(words::word("web.seal.holds")));
    wire.answer(id, json!({"seal_id": "sel_4", "codes": ["receipt_hash_mismatch"]}));
    pumped(&mut app, |a| !a.clients[0].read.is_empty());
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().all_text();
    assert!(sheet.contains(words::word("web.seal.broken")), "{sheet}");
    assert!(!sheet.contains(words::word("web.seal.holds")));
}

#[test]
fn an_eval_shows_no_verdict_or_agreement_the_record_does_not_give() {
    let (_tmp, mut app) = sample_app(false);
    let e = app.clients[0].work.record.evals.iter_mut().find(|e| e.id == "evl_3").unwrap();
    e.verdict = None;
    e.agreement = None;
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_3".into() });
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().all_text();
    assert!(!sheet.contains("aligned"), "{sheet}");
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
    };
    app.clients[0].setup = Some(SetupView {
        weft: "0.4.3".into(),
        weft_latest: Some("0.4.4".into()),
        weft_behind: true,
        configuration: "0.4.4".into(),
        latest: Some("0.4.4".into()),
        sense: "fab7-sense-v1".into(),
        sense_latest: Some("fab7-sense-v2".into()),
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
    assert!(upd.all_text().contains("0.4.3 → 0.4.4"));
    assert!(upd.find_layer("btn Find app… codex").is_some());
    assert!(upd.find_layer("btn Update all").is_some());
    for (name, state) in [("grok", SetupState::CannotInstall), ("agy", SetupState::Unknown)] {
        let r = upd.find_layer(&format!("update row {name}")).unwrap().all_text();
        assert!(r.starts_with(state.mark()), "{r}");
        assert!(r.contains(&state.words()), "{r}");
        assert!(!r.contains("Ready"), "{r}");
    }
    for forbidden in ["fab7-sense-v1", "sha256:", "/Users/", ".onnx", "weft-sense"] {
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

#[test]
fn the_tree_rows_are_one_line_with_the_status_shape() {
    let (_tmp, app) = sample_app(true);
    let dom = app.render();
    let tree = dom.find_layer("tree").expect("tree");
    assert_eq!(style(tree, "width"), Some("340px"));
    for (layer, shape) in [
        ("row agent antigravity fix login", "■"),
        ("row agent codex", "●"),
        ("row agent claude health endpoint", "●"),
        ("row agent claude readme fix", "□"),
    ] {
        let row = tree.find_layer(layer).unwrap_or_else(|| panic!("{layer}"));
        assert_eq!(style(row, "height"), Some("36px"), "{layer}");
        assert_eq!(row.children[0].all_text(), shape, "{layer}");
        let text = row.all_text();
        for word in ["working", "needs you", "ended", "No Ask yet"] {
            assert!(!text.contains(word), "{layer} says {word:?}: {text}");
        }
    }
    assert!(tree.find_layer("row agent codex").unwrap().all_text().contains('⚡'));
    assert!(!tree.find_layer("row agent claude readme fix").unwrap().all_text().contains('⚡'));
    assert_eq!(tree.find_layer("section AGENTS").unwrap().all_text(), "AGENTS");
    assert_eq!(tree.find_layer("section WORK").unwrap().all_text(), "WORK");
}

/// What the desktop asked the daemon to suggest since.
fn suggests(wire: &mut Wire) -> Vec<(u64, String)> {
    wire.calls()
        .into_iter()
        .filter_map(|(id, c)| match c {
            Call::Suggest { draft, .. } => Some((id, draft)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_agent_in_front_is_suggested_the_rest_of_a_settled_draft_and_a_late_answer_is_dropped() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    start(&mut app, &mut wire, "claude");
    start(&mut app, &mut wire, "codex");
    let t0 = Instant::now();
    app.message_field().type_text("run the");
    app.suggest_tick(t0);
    app.suggest_tick(t0 + Duration::from_millis(50));
    assert!(suggests(&mut wire).is_empty(), "not before the draft settles");
    app.suggest_tick(t0 + Duration::from_millis(120));
    app.suggest_tick(t0 + Duration::from_millis(300));
    let asked = suggests(&mut wire);
    assert_eq!(
        asked.iter().map(|a| a.1.as_str()).collect::<Vec<_>>(),
        ["run the"],
        "once per draft"
    );
    wire.answer(asked[0].0, json!({"text": " login tests", "score": 0.8}));
    pumped(&mut app, |a| a.ghost().is_some());
    assert_eq!(app.ghost(), Some(" login tests"));
    assert!(app.render().find_layer("take suggestion").is_some());
    // Only the agent in front shows one.
    app.dispatch(PaneAct::SelectTab { group: 0, tab: 0 });
    assert_eq!(app.ghost(), None);
    app.dispatch(PaneAct::SelectTab { group: 0, tab: 1 });
    assert_eq!(app.ghost(), Some(" login tests"));
    // Tab takes it; an answer for a draft no longer there is dropped.
    app.take_suggestion();
    assert_eq!(app.message_field().text(), "run the login tests");
    app.message_field().type_text(" now");
    app.suggest_tick(t0 + Duration::from_millis(400));
    app.suggest_tick(t0 + Duration::from_millis(520));
    let asked = suggests(&mut wire);
    app.message_field().type_text("!");
    wire.answer(asked[0].0, json!({"text": " please", "score": 0.9}));
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    assert_eq!(app.ghost(), None, "an answer to a draft no longer there");
    // A malformed answer shows nothing.
    app.message_field().set_text("ok");
    app.suggest_tick(t0 + Duration::from_millis(1000));
    app.suggest_tick(t0 + Duration::from_millis(1120));
    let asked = suggests(&mut wire);
    wire.answer(asked[0].0, json!({"text": 7}));
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    assert_eq!(app.ghost(), None);
}

#[test]
fn an_empty_field_asks_for_the_next_prompt_once_each_turn_has_ended() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "claude");
    let t0 = Instant::now();
    let mut settle = |app: &mut WeftDesktopApp, ms: u64| {
        app.suggest_tick(t0 + Duration::from_millis(ms));
        app.suggest_tick(t0 + Duration::from_millis(ms + 150));
        suggests(&mut wire)
    };
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Working);
    assert!(settle(&mut app, 0).is_empty(), "nothing to follow while it works");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::TurnEnded);
    let asked = settle(&mut app, 300);
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].1, "");
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
    app.clients[0].units[0].check = Some(Check {
        eval_id: "evl_9".into(),
        verdict: Verdict::Matches,
        agreement: 1.0,
        judged_by: None,
    });
    assert_eq!(outlined(&app), ["btn Seal"]);
    app.clients[0].sessions[2].turn = Some(Turn::Working);
    assert!(outlined(&app).is_empty());
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
        wire.calls().into_iter().filter(|(_, c)| matches!(c, Call::Input { .. })).collect();
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
    wire.event(Event::Agents { panes: vec![Some(Turn::Working)], sessions: Vec::new(), zone: 0 });
    pumped(&mut app, |a| a.clients[0].owes(&Then::Conversation { pane }));
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Read { ref what, .. } if what == "conversation"));
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
    assert!(text.contains(words::word("desktop.seal.unchecked")));
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

#[test]
fn the_machine_is_named_by_its_host_name_and_never_made_up() {
    assert_eq!(machine_name_of("studio.local"), "studio");
    assert_eq!(machine_name_of("build-box"), "build-box");
    assert_eq!(machine_name_of(""), words::word("desktop.this_machine"));
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

/// An Eval's view as the daemon reads it: one part with a step not met, its
/// change, a finding and a check.
fn an_eval_view() -> weft_core::eval_view::EvalView {
    use weft_core::eval_view::{Change, EvalView, Group, Reading, State, Step};
    EvalView {
        eval_id: "evl_3".into(),
        closed: true,
        verdict: Some("drifted".into()),
        confidence: Some(0.75),
        commission: Some(0.02),
        omission: Some(0.5),
        findings: vec!["the error is logged and the person sees nothing".into()],
        groups: vec![Group {
            key: "p1".into(),
            title: "Phase 1".into(),
            steps: vec![Step {
                id: "7".into(),
                label: "7".into(),
                text: "A failed send is told.".into(),
                done_when: "a send that fails shows the reason".into(),
                state: State::NotMet,
                changes: vec!["w1".into()],
                readings: vec![Reading {
                    role: "map".into(),
                    vote: "no".into(),
                    reason: "the error is swallowed".into(),
                    missing: String::new(),
                    counted: true,
                    cites: Vec::new(),
                }],
                findings: Vec::new(),
            }],
        }],
        changes: vec![Change {
            window: "w1".into(),
            path: "src/send.rs".into(),
            result: "required".into(),
            serves: vec!["7".into()],
            why: String::new(),
            quotes: vec!["log::debug!".into()],
            lines: Some("L88–L97".into()),
        }],
        checks: vec!["cargo test passed".into()],
        ..Default::default()
    }
}

#[test]
fn the_eval_view_is_read_through_the_daemon_and_opened_level_by_level() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    let (id, call) = wire
        .calls()
        .into_iter()
        .find(|(_, c)| matches!(c, Call::Read { what, .. } if what == "eval"))
        .expect("read");
    assert!(matches!(call, Call::Read { ref unit, .. } if unit == "evl_3"));
    let before = app.render().find_layer("sheet EvalView").unwrap().all_text();
    assert!(before.contains(words::word("desktop.eval.no_record")), "{before}");
    wire.answer(id, serde_json::to_value(an_eval_view()).unwrap());
    pumped(&mut app, |a| !a.clients[0].owes_any(|t| matches!(t, Then::EvalView { .. })));

    let top = app.render().find_layer("sheet EvalView").unwrap().clone();
    let text = top.all_text();
    assert!(text.contains("drifted") && text.contains("confidence 0.75"), "{text}");
    assert!(top.find_layer("by step").is_some() && top.find_layer("by file").is_some());
    assert!(text.contains(words::word("eval_view.findings")), "{text}");
    assert!(top.find_layer("eval row p1").is_some(), "{text}");

    app.dispatch(SheetAct::EvalOpen("g:p1".into()));
    assert!(app.render().find_layer("eval row 7").is_some());
    app.dispatch(SheetAct::EvalOpen("s:7".into()));
    let step = app.render().all_text();
    assert!(step.contains("A failed send is told.") && step.contains("Done when"), "{step}");
    // Its head says what its row said; a change is named by its lines.
    assert!(step.contains("1 change"), "{step}");
    assert!(step.contains("src/send.rs L88–L97") && !step.contains("w1"), "{step}");
    app.dispatch(SheetAct::EvalOpen("c:w1".into()));
    let (id, call) = wire
        .calls()
        .into_iter()
        .find(|(_, c)| matches!(c, Call::Read { what, .. } if what == "window"))
        .expect("its lines");
    assert!(matches!(call, Call::Read { ref unit, .. } if unit == "evl_3/w1"));
    wire.answer(
        id,
        json!({"text": "@@ -88,2 +88,3 @@\n fn send() {\n+    log::debug!(\"send failed\");\n }"}),
    );
    pumped(&mut app, |a| !a.clients[0].owes_any(|t| matches!(t, Then::Window { .. })));
    let lines = app.render().find_layer("sheet EvalView").unwrap().clone();
    assert!(lines.all_text().contains("src/send.rs L88–L97"), "{}", lines.all_text());
    let lit = lines.find_layer("line lit").expect("the quoted line is lit");
    assert!(lit.all_text().contains("log::debug!"));
    // The crumb goes back one level at a time, then to Details.
    app.dispatch(SheetAct::EvalBack);
    assert!(app.render().all_text().contains("A failed send is told."));
    app.dispatch(SheetAct::EvalBack);
    app.dispatch(SheetAct::EvalBack);
    app.dispatch(SheetAct::ToggleEvalViewMode);
    assert!(app.render().find_layer("eval row src/send.rs").is_some(), "by file");
    app.dispatch(SheetAct::EvalBack);
    assert!(matches!(app.active_sheet, Some(ActiveSheet::EvalDetail { .. })));
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
        sessions: Vec::new(),
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
    let (_, call) = wire.call();
    assert!(matches!(call, Call::Act { pane: Some(p), .. } if p == codex), "{call:?}");
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
    wire.event(Event::Agents {
        panes: vec![Some(Turn::Waiting), Some(Turn::Waiting)],
        sessions: vec![hooked, read],
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
        sessions: Vec::new(),
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
    // Picking is a choice: it closes the list, and sends nothing.
    app.dispatch(SheetAct::AskTo("start:grok".into()));
    assert!(wire.calls().is_empty());
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
    app.clients[0].config =
        Some(json!({"machine": {"turbo": false, "suggest": false}, "project": null}));
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
    assert_eq!(badge("setting Suggestions"), "OFF");

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
    app.dispatch(SheetAct::Toggle(Setting::Suggest));
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: vec!["suggest".into()],
            value: Some(json!(true))
        }
    );
    wire.event(Event::Config { config: json!({"machine": {"suggest": true, "notify": false}}) });
    pumped(&mut app, |a| {
        a.clients[0].config.as_ref().is_some_and(|c| c["machine"]["notify"] == false)
    });
    let dom = app.render();
    let sheet = dom.find_layer("sheet Settings").unwrap();
    let said =
        |row: &str| sheet.find_layer(row).unwrap().find_layer("setting badge").unwrap().all_text();
    assert_eq!(said("setting Notifications"), "OFF");
    assert_eq!(said("setting Suggestions"), "ON");
}

/// Workflow: who does each act and the Eval debate, every project's; a
/// choice lists "The agent you are in" and each harness; a role's model is
/// typed and kept on Enter.
#[test]
fn workflow_sets_every_project_s_routing_and_debate() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config = Some(json!({"machine": {"routing": {"eval": "claude"},
        "eval": {"debate": {"harness": "codex", "map": {"effort": "low"}}}}, "project": null}));
    app.dispatch(WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(SettingsTab::Workflow));
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
    assert_eq!(value("setting Runs in"), "Codex ▾");
    assert_eq!(value("setting Map effort"), "low ▾");

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
        key: key(&["eval", "debate", "reduce", "model"]),
    });
    assert_eq!(app.focus, Focus::Setting);
    app.setting_field.paste("gpt-6-luna");
    app.dispatch(SheetAct::CommitSetting);
    let (_, call) = wire.call();
    assert_eq!(
        call,
        Call::ConfigSet {
            scope: "machine".into(),
            key: key(&["eval", "debate", "reduce", "model"]),
            value: Some(json!("gpt-6-luna"))
        }
    );
    assert_eq!(app.focus, Focus::Terminal);
}

/// A project's own settings, from its row's ⋯: only what differs from every
/// project's, each beside every project's value, with Reset; and more added.
#[test]
fn a_project_s_own_settings_list_what_differs_and_reset_it() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].config = Some(json!({
        "machine": {"routing": {"eval": "claude"}},
        "project": {"routing": {"eval": "codex"}}}));
    app.dispatch(TreeAct::ToggleProjectMenu(weft()));
    let dom = app.render();
    assert!(dom.find_layer("project menu Settings").is_some());
    app.dispatch(TreeAct::OpenProjectSettings(weft()));
    assert!(
        matches!(&app.active_sheet, Some(ActiveSheet::ProjectSettings { project }) if *project == weft())
    );
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

    let key = vec!["routing".to_string(), "eval".to_string()];
    app.dispatch(SheetAct::Choose { project: Some(weft()), key: key.clone(), value: None });
    let (_, call) = wire.call();
    assert_eq!(call, Call::ConfigSet { scope: "project".into(), key, value: None });

    app.dispatch(SheetAct::AddProjectSetting);
    let dom = app.render();
    let list = dom.find_layer("choices add").expect("what can be set here");
    assert!(list.find_layer("choice Ask").is_some() && list.find_layer("choice Seal").is_some());
    assert!(list.find_layer("choice Turbo").is_none(), "Turbo is every project's only");
}
