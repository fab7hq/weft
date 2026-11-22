//! The files an agent pointed at: which become chips, the viewer each
//! opens, the page it loads, and what the page may ask.

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

/// Each message shows the files it named; a run of steps shows the files
/// its steps wrote, each once, in the order they were first written.
#[test]
fn messages_and_steps_show_a_chip_for_each_file() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.state.panes.groups.clear();
    let pane = start(&mut app, &mut wire, "codex");
    app.dispatch(PaneAct::SetView { group: 0, tab: 0, view: PaneViewMode::Conversation });
    let (id, _) = wire.call();
    let wrote = |id: &str, path: &str, kind| {
        let mut s = said(id, Kind::Step, None, vec![a_ref(path, kind)]);
        s.tool = Some("Write".into());
        s
    };
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
        wrote("2.0", "out/revenue.png", FileKind::Image),
        wrote("3.0", "out/report.md", FileKind::Markdown),
        said("3.1", Kind::Step, None, Vec::new()),
        wrote("4.0", "out/revenue.png", FileKind::Image),
        wrote("5.0", "out/report.html", FileKind::Html),
        said("6.0", Kind::Agent, Some("Done."), Vec::new()),
    ];
    wire.answer(id, json!({"conversation": {"items": items}}));
    pumped(&mut app, |a| a.conversations.contains_key(&(weft(), pane)));

    let dom = app.render();
    drawable(&dom);
    let you = dom.find_layer("msg you").expect("the person's message");
    assert_eq!(chip_names(you), ["/home/q2.xlsx"]);
    let reply = dom.find_layer("msg codex").expect("the agent's first message");
    assert_eq!(chip_names(reply), ["out/report.md", "scripts/q3.py"]);
    let steps = dom.find_layer("tool steps with files").expect("the steps and their files");
    assert!(steps.find_layer("tool steps folded").is_some(), "the steps as before");
    assert_eq!(chip_names(steps), ["out/revenue.png", "out/report.md", "out/report.html"]);
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
    app.dispatch(FileAct::Open { project: weft(), pane: 0, harness: "codex".into(), file });
}

/// On macOS every kind opens in the viewer, a window of its own, the page
/// drawing what it can; the same file again brings its window in front.
/// Elsewhere every file opens in the machine's own app.
#[test]
fn a_chip_opens_its_file_in_the_viewer() {
    let (tmp, mut app, _wire) = wired_app();
    let kinds =
        [FileKind::Markdown, FileKind::Pdf, FileKind::Html, FileKind::Video, FileKind::Other];
    for (n, kind) in kinds.into_iter().enumerate() {
        open(&mut app, a_ref(&format!("out/{n}"), kind));
    }
    if !cfg!(target_os = "macos") {
        assert_eq!(app.files.outside.len(), kinds.len());
        assert!(app.files.viewers.is_empty() && app.files.to_open.is_empty(), "no viewer");
        return;
    }
    assert!(app.files.outside.is_empty(), "none in the machine's own app");
    let ids: Vec<u64> = app.files.viewers.iter().map(|v| v.id).collect();
    assert_eq!(app.files.to_open, ids);
    open(&mut app, a_ref("out/0", FileKind::Markdown));
    assert_eq!(app.files.viewers.len(), kinds.len(), "its window again, not a second one");
    assert_eq!(app.files.to_open.last(), Some(&ids[0]));

    // The file the window may serve is where it is on this machine.
    let file = tmp.path().join("report.md");
    std::fs::write(&file, "# Q3").unwrap();
    open(&mut app, a_ref(&file.to_string_lossy(), FileKind::Markdown));
    let v = app.files.viewers.last().unwrap();
    assert_eq!(v.abs, file.canonicalize().unwrap());
    assert_eq!(app.files.get(ids[1]).unwrap().abs, weft().join("out/1"), "gone: as named");
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
    assert_eq!(v.title(), "Q3 report & notes.md — weft");
    assert_eq!(
        v.page(),
        "weft://app/viewer.html#u=weft%3A%2F%2Ffile%2F0&path=out%2FQ3+report+%26+notes.md\
         &kind=markdown&size=4710&modified=1795312120486&agent=codex&project=weft"
    );
}

/// Reveal shows the viewer's file in the machine's file manager; Close
/// ends its viewer.
#[test]
fn the_viewer_reveals_its_file_or_closes() {
    let (_tmp, mut app, _wire) = wired_app();
    if !cfg!(target_os = "macos") {
        return;
    }
    open(&mut app, a_ref("out/report.md", FileKind::Markdown));
    let id = *app.files.to_open.last().unwrap();
    app.dispatch(FileAct::Reveal(id));
    assert_eq!(app.files.reveal, [weft().join("out/report.md")]);
    app.dispatch(FileAct::Close(id));
    assert!(app.files.get(id).is_none() && app.files.viewers.is_empty());
    app.dispatch(FileAct::Reveal(id));
    assert_eq!(app.files.reveal.len(), 1, "closed: nothing to reveal");
}
