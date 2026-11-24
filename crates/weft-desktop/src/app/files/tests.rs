//! The files an agent named: which become chips, the file each opens in its
//! agent's pane, the page it loads, and what the page may ask.

use std::path::PathBuf;

use serde_json::json;
use weft_core::conversation::{Item, Kind};
use weft_core::files::{FileKind, Ref};
use weft_core::state::PaneViewMode;

use super::WeftDesktopApp;
use crate::action::{FileAct, PaneAct};
use crate::app::tests::{pumped, start, wired_app};
use crate::gpui::Element;

fn weft() -> PathBuf {
    PathBuf::from("/work/weft")
}

fn a_ref(path: &str, kind: FileKind) -> Ref {
    Ref { path: path.into(), kind, size: 4_710, modified: 1_795_312_120_486 }
}

fn said(id: &str, kind: Kind, text: Option<&str>, refs: Vec<Ref>) -> Item {
    Item {
        id: id.into(),
        kind,
        at: None,
        text: text.map(str::to_string),
        tool: None,
        arg: None,
        call: None,
        failed: None,
        refs,
        mentions: Vec::new(),
    }
}

/// The file names on the chips under one element, in order.
fn chip_names(el: &Element) -> Vec<String> {
    fn walk(el: &Element, out: &mut Vec<String>) {
        if el.layer_name.as_deref() == Some("file") {
            out.push(el.aria_label.clone().unwrap_or_default());
        }
        for c in &el.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(el, &mut out);
    out
}

/// Only an agent's message shows chips: the files it named that are on the
/// machine. A person's message and the steps show none.
#[test]
fn an_agents_message_shows_a_chip_for_each_file_it_named() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "codex");
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, _) = wire.call();
    let mut wrote = said("2.0", Kind::Step, None, vec![a_ref("out/revenue.png", FileKind::Image)]);
    wrote.tool = Some("Write".into());
    let items = vec![
        said(
            "0.0",
            Kind::Person,
            Some("see `~/q2.xlsx`"),
            vec![a_ref("/home/q2.xlsx", FileKind::Sheet)],
        ),
        said(
            "1.0",
            Kind::Agent,
            Some("Wrote `out/report.md` from `scripts/q3.py`."),
            vec![
                a_ref("out/report.md", FileKind::Markdown),
                a_ref("scripts/q3.py", FileKind::Text),
            ],
        ),
        wrote,
        said("6.0", Kind::Agent, Some("Done."), Vec::new()),
    ];
    wire.answer(id, json!({"conversation": {"items": items}}));
    pumped(&mut app, |a| a.conversations.contains_key(&(weft(), pane)));

    let dom = app.render();
    drawable(&dom);
    let you = dom.find_layer("msg you").expect("the person's message");
    assert!(chip_names(you).is_empty(), "a person's message shows no chips");
    let reply = dom.find_layer("msg codex").expect("the agent's first message");
    assert_eq!(chip_names(reply), ["out/report.md", "scripts/q3.py"]);
    assert!(dom.find_layer("tool steps folded").is_some_and(|s| chip_names(s).is_empty()));
    let done = dom.find_layer("msg codex 2").expect("the last message");
    assert!(chip_names(done).is_empty(), "no file, no chips");
    // A chip shows the file's name, not its folder.
    let chip = reply.find_layer("file").unwrap();
    assert!(chip.all_text().starts_with("▤ report.md"), "{}", chip.all_text());
    assert!(!chip.all_text().contains("out/"));
}

/// Every style a chip uses is one the window draws, and a box has a
/// control's corners.
fn drawable(el: &Element) {
    for (k, v) in &el.styles {
        assert!(crate::window::STYLES.contains(&k.as_str()), "{:?} uses {k}: {v}", el.layer_name);
    }
    if el.styles.iter().any(|(k, v)| k == "border" && v.starts_with("1px")) {
        assert!(el.styles.iter().any(|(k, _)| k == "border-radius"), "{:?}", el.layer_name);
    }
    el.children.iter().for_each(drawable);
}

fn open(app: &mut WeftDesktopApp, file: Ref) {
    app.dispatch(FileAct::Open { project: weft(), pane: 0, file });
}

/// On macOS a chip opens its file in its agent's pane, the page drawing what
/// it can: one file for each agent, the next one taking its place and the
/// same one again read again. Elsewhere every file opens in the machine's
/// own app.
#[test]
fn a_chip_opens_its_file_in_its_agents_pane() {
    let (tmp, mut app, _wire) = wired_app();
    let kinds =
        [FileKind::Markdown, FileKind::Pdf, FileKind::Html, FileKind::Video, FileKind::Other];
    for (n, kind) in kinds.into_iter().enumerate() {
        open(&mut app, a_ref(&format!("out/{n}"), kind));
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(app.files.outside.len(), kinds.len());
        assert!(app.files.viewers.is_empty(), "no viewer");
        return;
    }
    assert!(app.files.outside.is_empty(), "none in the machine's own app");
    let [v] = &app.files.viewers[..] else { panic!("one file for the agent") };
    assert_eq!((v.file.path.as_str(), v.abs.clone()), ("out/4", weft().join("out/4")));
    let first = v.id;
    open(&mut app, a_ref("out/4", FileKind::Other));
    let again = app.files.of(&weft(), 0).unwrap().id;
    assert_ne!(again, first, "the same file again is read again");
    assert!(app.files.get(first).is_none());

    // Another agent's pane keeps its own.
    app.dispatch(FileAct::Open {
        project: weft(),
        pane: 1,
        file: a_ref("out/notes.md", FileKind::Markdown),
    });
    assert_eq!(app.files.viewers.len(), 2);
    assert_eq!(app.files.of(&weft(), 0).unwrap().id, again);

    // The file it may serve is where it is on this machine.
    let file = tmp.path().join("report.md");
    std::fs::write(&file, "# Q3").unwrap();
    open(&mut app, a_ref(&file.to_string_lossy(), FileKind::Markdown));
    assert_eq!(app.files.of(&weft(), 0).unwrap().abs, file.canonicalize().unwrap());
}

/// The page it loads names the file's address and what is known of it, as
/// the page's `URLSearchParams` reads them back.
#[test]
fn the_viewer_loads_its_page_with_the_file() {
    let (_tmp, mut app, _wire) = wired_app();
    if !cfg!(target_os = "macos") {
        return;
    }
    open(&mut app, a_ref("out/Q3 report & notes.md", FileKind::Markdown));
    let v = app.files.viewers.last().unwrap();
    assert_eq!(
        v.page(),
        "weft://app/viewer.html#u=weft%3A%2F%2Ffile%2F0&path=out%2FQ3+report+%26+notes.md\
         &kind=markdown&size=4710&modified=1795312120486"
    );
}

/// Reveal shows the viewer's file in the machine's file manager; Close
/// gives the pane back to the conversation.
#[test]
fn the_viewer_reveals_its_file_or_closes() {
    let (_tmp, mut app, _wire) = wired_app();
    if !cfg!(target_os = "macos") {
        return;
    }
    open(&mut app, a_ref("out/report.md", FileKind::Markdown));
    let id = app.files.of(&weft(), 0).unwrap().id;
    app.dispatch(FileAct::Reveal(id));
    assert_eq!(app.files.reveal, [weft().join("out/report.md")]);
    app.dispatch(FileAct::Close(id));
    assert!(app.files.get(id).is_none() && app.files.viewers.is_empty());
    app.dispatch(FileAct::Reveal(id));
    assert_eq!(app.files.reveal.len(), 1, "closed: nothing to reveal");
}

/// The file opens beside the conversation in its agent's pane, its chip
/// marked, the divider between them; a narrow pane gives the file its
/// whole width, and Close gives it back to the conversation.
#[test]
fn the_file_opens_beside_its_conversation() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "codex");
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, _) = wire.call();
    let items = vec![said(
        "1.0",
        Kind::Agent,
        Some("Wrote `out/report.md` from `scripts/q3.py`."),
        vec![a_ref("out/report.md", FileKind::Markdown), a_ref("scripts/q3.py", FileKind::Text)],
    )];
    wire.answer(id, json!({"conversation": {"items": items}}));
    pumped(&mut app, |a| a.conversations.contains_key(&(weft(), pane)));
    assert!(app.render().find_layer("pane with file").is_none(), "no file open yet");

    app.dispatch(FileAct::Open {
        project: weft(),
        pane,
        file: a_ref("out/report.md", FileKind::Markdown),
    });
    let viewer = app.files.of(&weft(), pane).unwrap().id;
    let dom = app.render();
    drawable(&dom);
    let beside = dom.find_layer("conversation beside").expect("the conversation beside it");
    assert!(beside.find_layer("pane conversation").is_some());
    let open = beside.find_layer("file open").expect("its chip marked");
    assert_eq!(open.aria_label.as_deref(), Some("out/report.md"));
    assert_eq!(chip_names(beside), ["scripts/q3.py"], "the other chip as it was");
    assert!(dom.find_layer("divider").is_some());
    let page = dom.find_layer("file page").unwrap();
    assert_eq!(page.tag, "viewer");
    assert!(page.attrs.contains(&("data-id".into(), viewer.to_string())));

    // Too narrow for both: the file alone.
    app.set_window_bounds(0.0, 0.0, 960.0, 640.0);
    let dom = app.render();
    assert!(dom.find_layer("conversation beside").is_none());
    assert!(dom.find_layer("file page").is_some());

    app.dispatch(FileAct::Close(viewer));
    let dom = app.render();
    assert!(dom.find_layer("pane with file").is_none());
    assert!(dom.find_layer("pane conversation").is_some(), "the conversation back");
}

/// The divider leaves the conversation its width, as long as each side
/// keeps its least.
#[test]
fn the_divider_keeps_each_side_its_least() {
    use crate::files::{CONVERSATION_MIN, FILE_MIN, Files};
    let mut files = Files::default();
    assert_eq!(files.beside(940.0), Some(420.0));
    files.width = 100.0;
    assert_eq!(files.beside(940.0), Some(CONVERSATION_MIN));
    files.width = 900.0;
    assert_eq!(files.beside(940.0), Some(940.0 - FILE_MIN));
    assert_eq!(files.beside(CONVERSATION_MIN + FILE_MIN - 1.0), None, "too narrow: the file alone");
}
