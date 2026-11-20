//! Files an Eval should skip: the Eval dialog's findings and the line each
//! writes, a project's settings with Check again, and what an opened Eval
//! counted and did not read, in its Details.

use serde_json::{Value, json};
use weft_core::skip::NotRead;
use weft_proto::{Call, Event};

use super::tests::{Wire, pumped, sample_app, wired_app};
use super::waiting_tests::{act_call, call_of, weft};
use super::*;
use crate::action::{Answer, BarAct, SheetAct, TreeAct};
use crate::gpui::Element;

const EVIDENCE: &str = "research/papers/to-err-is-agentic/evidence/";

fn output(added: bool) -> Value {
    let mut f = json!({"kind": "untracked_bulk", "path": "output/", "files": 329,
                       "fix": {"file": ".gitignore", "line": "/output/"}});
    if added {
        f["added"] = json!(true);
    }
    f
}

fn evidence() -> Value {
    json!({"kind": "data_bulk", "path": EVIDENCE, "files": 13_708,
           "kinds_of_files": [["json", 13_708]],
           "fix": {"file": ".gitattributes",
                   "line": "research/papers/*/evidence/** linguist-generated=true"}})
}

fn homepage() -> Value {
    json!({"kind": "sub_repo", "path": "homepage/", "files": 12, "commits": 3})
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

/// Text that never runs past its box: one line, cut with an ellipsis.
fn cut(el: &Element) -> bool {
    style(el, "white-space") == Some("nowrap")
        && style(el, "text-overflow") == Some("ellipsis")
        && style(el, "min-width") == Some("0")
}

/// Every style is one the window draws, and every corner one of the two.
fn drawn(screen: &Element) {
    let radii = [format!("{}px", palette::RADIUS_CONTROL), format!("{}px", palette::RADIUS_PANEL)];
    for el in every(screen) {
        for (k, v) in &el.styles {
            assert!(crate::window::STYLES.contains(&k.as_str()), "{:?} uses {k}", el.layer_name);
            assert!(k != "border-radius" || radii.contains(v), "{:?} rounds by {v}", el.layer_name);
        }
    }
}

/// The Eval dialog of `ask_1`, staged as `id` with `reads`.
fn dialog(app: &mut WeftDesktopApp, wire: &mut Wire, id: &str, reads: Value) {
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    let call = act_call(wire, "eval");
    said_again(wire, id, reads);
    wire.answer(call, json!({ "pending": id }));
    pumped(app, |a| a.clients[0].waiting.iter().any(|w| w.id == id));
}

/// The daemon says the dialog `id` again, with `reads`.
fn said_again(wire: &mut Wire, id: &str, reads: Value) {
    wire.event(Event::Pending {
        id: id.into(),
        pane: u32::MAX,
        what: words::word("dialog.eval.title").into(),
        why: "weft 0.7 updates · 3 Asks\nantigravity reads the whole change.".into(),
        payload: serde_json::to_vec(&json!({ "reads": reads })).unwrap(),
        sends: None,
        go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
    });
}

fn bar(app: &WeftDesktopApp) -> Element {
    app.render().find_layer("Cancel / Send bar").expect("the dialog").clone()
}

/// Under Reads, what to check before it reads: each finding with its exact
/// line and one button; a nested repository a muted line with none. Its
/// line written, the daemon says the dialog again under the same id: it
/// shows once, the row in teal, and Reads counts fewer files.
#[test]
fn the_eval_dialog_lists_its_findings_and_a_line_written_turns_its_row_teal() {
    let (_tmp, mut app, mut wire) = wired_app();
    let reads = |files: u64, findings: Vec<Value>, before: Option<u64>| {
        let mut r = json!({"since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 52,
                           "files": files, "skipped": {}, "large": false, "findings": findings});
        if let Some(b) = before {
            r["files_before"] = json!(b);
        }
        r
    };
    dialog(
        &mut app,
        &mut wire,
        "p1",
        reads(14_181, vec![output(false), evidence(), homepage()], None),
    );
    let b = bar(&app);
    drawn(&b);
    let check = b.find_layer("check before").expect("Check before it reads").clone();
    let head = check.all_text();
    assert!(head.starts_with(words::word("skip.check_before")), "{head}");
    assert_eq!(check.find_layer("findings left").unwrap().all_text(), "2 of 2 left");
    let row = check.find_layer("finding output/").unwrap();
    assert_eq!(
        row.find_layer("finding said").unwrap().all_text(),
        "output/ — 329 files Git does not track"
    );
    assert_eq!(row.find_layer("line").unwrap().all_text(), "/output/");
    assert!(row.find_layer("btn Add to .gitignore").is_some());
    let row = check.find_layer(&format!("finding {EVIDENCE}")).unwrap();
    assert!(row.all_text().contains("13,708 data files (json)"), "{}", row.all_text());
    assert!(row.all_text().contains("research/papers/*/evidence/** linguist-generated=true"));
    assert!(row.find_layer("btn Mark as generated").is_some());
    let nested = check.find_layer("finding homepage/").unwrap();
    assert_eq!(
        nested.all_text(),
        "○ homepage/ is its own repository: 12 changed files there are not part of this Eval"
    );
    assert_eq!(style(nested.find_layer("mark").unwrap(), "color"), Some(palette::DIM));
    assert!(every(nested).iter().all(|e| e.action.is_none()), "no button");
    assert_eq!(b.find_layer("reads counts").unwrap().all_text(), "52 commits · 14,181 files");

    // Its button writes its line; while it does, the row breathes in its place.
    let add =
        SheetAct::SkipAdd { project: weft(), file: ".gitignore".into(), line: "/output/".into() };
    assert!(b.find_action(&add.clone().into()).is_some());
    app.dispatch(add.clone());
    let (id, call) = call_of(&mut wire, |c| matches!(c, Call::SkipAdd { .. }));
    assert_eq!(call, Call::SkipAdd { file: ".gitignore".into(), line: "/output/".into() });
    app.dispatch(add);
    let row = bar(&app).find_layer("finding output/").unwrap().clone();
    assert!(
        row.find_layer("writing").is_some() && row.find_layer("btn Add to .gitignore").is_none()
    );

    // The daemon says the dialog again, under the same id, and answers.
    let after = vec![output(true), evidence(), homepage()];
    said_again(&mut wire, "p1", reads(13_852, after, Some(14_181)));
    wire.answer(id, json!({"findings": [evidence()]}));
    pumped(&mut app, |a| a.clients[0].skip.len() == 1);
    assert!(wire.calls().iter().all(|(_, c)| !matches!(c, Call::SkipAdd { .. })), "written once");
    assert_eq!(app.clients[0].waiting.len(), 1, "the dialog said again replaces itself");
    let b = bar(&app);
    let dialogs = every(&app.render())
        .iter()
        .filter(|e| e.layer_name.as_deref() == Some("check before"))
        .count();
    assert_eq!(dialogs, 1);
    let added = b.find_layer("finding output/ added").expect("its row, written");
    assert_eq!(
        added.find_layer("finding said").unwrap().all_text(),
        "Added to .gitignore · output/ — 329 files are no longer read"
    );
    let mark = added.find_layer("mark").unwrap();
    assert_eq!((mark.all_text().as_str(), style(mark, "color")), ("✓", Some(palette::TEAL)));
    assert!(every(added).iter().all(|e| e.action.is_none()), "no button once written");
    let said = added.find_layer("finding said").unwrap();
    assert!(said.runs.iter().any(|(r, run)| r.start == 0 && run.color == Some(palette::TEAL)));
    assert_eq!(b.find_layer("findings left").unwrap().all_text(), "1 of 2 left");
    let counts = b.find_layer("reads counts").unwrap();
    assert_eq!(counts.all_text(), "52 commits · 14,181 13,852 files");
    let struck = &counts.runs[0];
    assert!(struck.1.strike && &counts.text.as_ref().unwrap()[struck.0.clone()] == "14,181");

    // A line refused says why, under the findings.
    let mark_it = SheetAct::SkipAdd {
        project: weft(),
        file: ".gitattributes".into(),
        line: "research/papers/*/evidence/** linguist-generated=true".into(),
    };
    app.dispatch(mark_it);
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::SkipAdd { .. }));
    wire.refuse_as(id, "refused", ".gitattributes is not writable");
    pumped(&mut app, |a| a.clients[0].skip_refused.is_some());
    let refused = bar(&app).find_layer("skip refused").expect("why").all_text();
    assert_eq!(refused, ".gitattributes is not writable");
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::No });
}

/// A dialog that found nothing to skip says nothing of it; any number of
/// findings scroll in their box, and a long path or line ends in an
/// ellipsis rather than running past it.
#[test]
fn the_eval_dialog_survives_no_findings_and_many_and_long_ones() {
    let (_tmp, mut app, mut wire) = wired_app();
    let r = json!({"since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 1, "files": 2,
                   "skipped": {}, "large": false, "findings": []});
    dialog(&mut app, &mut wire, "p1", r);
    assert!(bar(&app).find_layer("check before").is_none());
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::No });
    wire.event(Event::Resolved { id: "p1".into(), yes: false });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());

    let long = format!("{}/", "a-folder-with-a-long-name".repeat(12));
    let many: Vec<Value> = (0..60)
        .map(|i| {
            json!({"kind": "large_text", "path": format!("{long}{i}.csv"), "files": 1,
                   "bytes": 48_000_000,
                   "fix": {"file": ".gitattributes",
                           "line": format!("{long}{i}.csv linguist-generated=true")}})
        })
        .collect();
    let r = json!({"since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 1, "files": 2,
                   "skipped": {}, "large": false, "findings": many});
    dialog(&mut app, &mut wire, "p2", r);
    let b = bar(&app);
    drawn(&b);
    let list = b.find_layer("findings").unwrap();
    assert_eq!(list.children.len(), 60);
    assert_eq!(style(list, "overflow-y"), Some("auto"));
    assert!(style(list, "max-height").is_some());
    let row = &list.children[0];
    assert!(row.all_text().contains("48 MB of text"), "{}", row.all_text());
    assert!(cut(row.find_layer("finding said").unwrap()));
    assert!(cut(row.find_layer("line").unwrap()));
    assert_eq!(b.find_layer("findings left").unwrap().all_text(), "60 of 60 left");
}

/// A project's settings list the files an Eval should skip, each with its
/// line and button, never a nested repository; Check again looks again,
/// and with none left they say every file to skip is already ignored.
#[test]
fn a_projects_settings_list_the_files_to_skip_and_check_again() {
    let (_tmp, mut app, mut wire) = wired_app();
    let media = json!({"kind": "committed_media", "path": "marketing/weft/assets/final/",
                       "files": 24, "fix": {"file": ".gitattributes",
                       "line": "marketing/weft/assets/final/** linguist-generated=true"}});
    let built = json!({"kind": "committed_build", "path": "web/dist/", "files": 7,
                       "fix": {"file": ".gitattributes", "line": "web/dist/** linguist-generated=true"}});
    let dump = json!({"kind": "large_text", "path": "data/dump.sql", "files": 1,
                      "bytes": 48_000_000,
                      "fix": {"file": ".gitattributes", "line": "data/dump.sql linguist-generated=true"}});
    let findings = vec![output(false), evidence(), media, built, dump, homepage()];
    wire.event(Event::Skip { findings: serde_json::from_value(json!(findings)).unwrap() });
    pumped(&mut app, |a| !a.clients[0].skip.is_empty());
    app.dispatch(TreeAct::OpenProjectSettings(weft()));
    let sheet =
        |app: &WeftDesktopApp| app.render().find_layer("sheet Project settings").unwrap().clone();
    let s = sheet(&app);
    let section = s.find_layer("files to skip").expect("the section");
    drawn(section);
    let head = section.find_layer("skip head").unwrap().all_text();
    assert_eq!(head, "FILES AN EVAL SHOULD SKIP Check again");
    let said: Vec<String> = every(section)
        .iter()
        .filter(|e| e.layer_name.as_deref() == Some("finding said"))
        .map(|e| e.all_text())
        .collect();
    assert_eq!(
        said,
        [
            "output/ — 329 files Git does not track".to_string(),
            format!("{EVIDENCE} — 13,708 data files (json)"),
            "marketing/weft/assets/final/ — 24 images and videos".into(),
            "web/dist/ — 7 files a build wrote".into(),
            "data/dump.sql — 48 MB of text".into(),
        ],
        "never a nested repository"
    );
    let buttons = every(section)
        .iter()
        .filter_map(|e| e.layer_name.clone())
        .filter(|l| l.starts_with("btn "))
        .collect::<Vec<_>>();
    assert_eq!(buttons.len(), 5);
    assert_eq!(buttons[0], "btn Add to .gitignore");
    assert!(buttons[1..].iter().all(|b| b == "btn Mark as generated"), "{buttons:?}");
    assert!(section.find_layer("skip none").is_none());

    // Its button writes its line.
    app.dispatch(SheetAct::SkipAdd {
        project: weft(),
        file: ".gitignore".into(),
        line: "/output/".into(),
    });
    let (id, call) = call_of(&mut wire, |c| matches!(c, Call::SkipAdd { .. }));
    assert_eq!(call, Call::SkipAdd { file: ".gitignore".into(), line: "/output/".into() });
    let rest: Vec<Value> = vec![evidence()];
    wire.answer(id, json!({"findings": rest}));
    pumped(&mut app, |a| a.clients[0].skip.len() == 1);
    assert!(sheet(&app).find_layer("finding output/").is_none(), "gone once written");

    // Check again: it breathes until the check answers, then none is left.
    let again = sheet(&app).find_layer("check again").unwrap().action.clone().unwrap();
    assert_eq!(again, SheetAct::SkipCheck(weft()).into());
    app.dispatch(again);
    let (id, call) = call_of(&mut wire, |c| matches!(c, Call::SkipCheck | Call::SkipAdd { .. }));
    assert_eq!(call, Call::SkipCheck);
    assert!(sheet(&app).find_layer("files to skip").unwrap().find_layer("breathing dot").is_some());
    wire.answer(id, json!({"findings": []}));
    pumped(&mut app, |a| a.clients[0].skip.is_empty());
    let s = sheet(&app);
    let section = s.find_layer("files to skip").unwrap();
    assert!(section.find_layer("breathing dot").is_none());
    let none = section.find_layer("skip none").expect("the empty state").all_text();
    assert_eq!(none, format!("✓ {}", words::word("skip.none")));
    assert!(every(section).iter().all(|e| e.layer_name.as_deref() != Some("finding said")));
    drawn(section);
}

/// An Eval's Details list what its readers counted and did not read, by
/// kind and folder, and how many files that is; one that read every file
/// says nothing of it.
#[test]
fn an_evals_details_say_what_it_did_not_read_by_kind_and_folder() {
    let (_tmp, mut app) = sample_app(false);
    let group = |kind: &str, folder: &str, files: usize| NotRead {
        kind: kind.into(),
        folder: folder.into(),
        files,
    };
    let e = app.clients[0].work.record.evals.iter_mut().find(|e| e.id == "evl_3").unwrap();
    e.not_read = vec![
        group("generated", EVIDENCE, 13_708),
        group("binary", "marketing/weft/assets/final/", 15),
        group("binary", "assets/", 2),
        group("binary", "logo.png", 1),
        group("binary", "icon.png", 1),
        group("lock", "weft/Cargo.lock", 1),
        group("lock", "weft-app/package-lock.json", 1),
        group("vendored", "third_party/", 4),
        group("vendored", "vendor/", 2),
    ];
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_3".into() });
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().clone();
    drawn(&sheet);
    let section = sheet.find_layer("not read").expect("Not read");
    assert_eq!(
        section.find_layer("not read head").unwrap().all_text(),
        "Not read · 13,735 files counted"
    );
    let rows: Vec<String> = section.children[1..].iter().map(Element::all_text).collect();
    assert_eq!(
        rows,
        [
            format!("13,708 generated {EVIDENCE}"),
            "6 vendored third_party/ and 1 other folder".into(),
            "19 binary marketing/weft/assets/final/ and 3 other folders".into(),
            "2 lock weft/Cargo.lock · weft-app/package-lock.json".into(),
        ]
    );
    assert!(!section.all_text().contains("ignored"), "what is ignored is not counted");
    let path = every(section.children.last().unwrap())
        .into_iter()
        .find(|e| e.text.as_deref() == Some("weft/Cargo.lock · weft-app/package-lock.json"))
        .unwrap();
    assert!(cut(path), "a long path ends in an ellipsis");

    // One file.
    let e = app.clients[0].work.record.evals.iter_mut().find(|e| e.id == "evl_3").unwrap();
    e.not_read = vec![group("lock", "Cargo.lock", 1)];
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().clone();
    let head = sheet.find_layer("not read head").unwrap().all_text();
    assert_eq!(
        head,
        format!("{} · {}", words::word("not_read.title"), words::word("not_read.counted_one"))
    );
    // Every file read: nothing said.
    let e = app.clients[0].work.record.evals.iter_mut().find(|e| e.id == "evl_3").unwrap();
    e.not_read.clear();
    assert!(app.render().find_layer("not read").is_none());
}
