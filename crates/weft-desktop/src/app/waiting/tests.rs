//! Every wait says so: while an answer is owed the window shows the
//! working or loading state with its words, and a failure keeps its reason
//! until the person acts.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use weft_core::onboarding::{Row as SetupRow, State as SetupState, View as SetupView};
use weft_core::turns::Turn;
use weft_proto::{Call, Event};

use super::tests::{Wire, eval_staged, pumped, start, wired_app};
use super::*;
use crate::action::{Answer, BarAct, PaneAct, SheetAct, TreeAct};
use crate::gpui::Element;

pub(super) fn weft() -> PathBuf {
    PathBuf::from("/work/weft")
}

/// The bar while something works, with what it says.
pub(super) fn working_bar(app: &WeftDesktopApp) -> Option<String> {
    app.render().find_layer("working bar").map(Element::all_text)
}

/// The next call the desktop writes that `pick` picks; those before it are
/// passed over.
pub(super) fn call_of(wire: &mut Wire, pick: impl Fn(&Call) -> bool) -> (u64, Call) {
    loop {
        let (id, call) = wire.call();
        if pick(&call) {
            return (id, call);
        }
    }
}

/// An Eval opening that another window or device began, on `work`.
fn opening_elsewhere(work: &str, end: Option<weft_proto::JobEnd>) -> weft_proto::Job {
    weft_proto::Job {
        id: format!("job_{work}"),
        kind: "eval_open".into(),
        on: weft_proto::JobOn { work: Some(work.into()), ..Default::default() },
        call: None,
        steps: Vec::new(),
        share: None,
        age_ms: 1_000,
        end,
    }
}

pub(super) fn act_call(wire: &mut Wire, act: &str) -> u64 {
    call_of(wire, |c| matches!(c, Call::Act { act: a, .. } if a == act)).0
}

pub(super) fn resolve_call(wire: &mut Wire) -> u64 {
    call_of(wire, |c| matches!(c, Call::Resolve { yes: true, .. })).0
}

/// A failure is said in the bar, outlasts the why-not's few seconds, and
/// goes once the person acts.
pub(super) fn fails_until_acted(app: &mut WeftDesktopApp, said: &str) {
    pumped(app, |a| a.failed.is_some());
    assert_eq!(app.failed.as_deref(), Some(said));
    assert!(working_bar(app).is_none(), "nothing works any more");
    assert!(!app.expire_why_not(Instant::now() + Duration::from_secs(60)));
    let shown = app.render().find_layer("failed sentence").expect("said in the bar").all_text();
    assert_eq!(shown, said);
    app.dispatch(TreeAct::SelectProject(weft()));
    assert!(app.failed.is_none(), "the person acted");
}

/// The Eval dialog's yes: "Opening the Eval…" with what it is about, its
/// line and note, and the work's row says it too; once its readers start,
/// its view opens by itself.
#[test]
fn an_eval_says_it_is_opening_until_its_readers_start_then_its_view_opens() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(SheetAct::Close);
    app.dispatch(BarAct::Eval);
    // Its dialog still being made: said at once.
    let preparing = working_bar(&app).expect("the bar works while the dialog is made");
    assert!(preparing.contains(words::word("wait.preparing")), "{preparing}");
    eval_staged(&mut app, &mut wire, "p1", None);
    assert!(working_bar(&app).is_none(), "the dialog takes the bar");

    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let id = resolve_call(&mut wire);
    wire.event(Event::Resolved { id: "p1".into(), yes: true });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());
    let bar = working_bar(&app).expect("the dialog stays as its working bar");
    for said in [
        words::word("wait.opening_eval"),
        "fix login · 1 Ask · read by antigravity",
        words::word("wait.opening_eval_line"),
    ] {
        assert!(bar.contains(said), "{said} in {bar}");
    }
    let row = app.render().find_layer("row unit fix login").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_opening_eval")), "{row}");
    // The time so far, once it has gone on a while.
    let since = Instant::now() - Duration::from_secs(75);
    let w = crate::waiting::Working { since, ..app.working().unwrap() };
    let drawn = crate::waiting::working_bar(&w, Instant::now());
    assert!(drawn.find_layer("moving bar").is_some());
    assert_eq!(drawn.find_layer("working time").unwrap().all_text(), "1:15");

    // Opened, its readers start: the view opens by itself.
    wire.answer(id, json!({}));
    let mut units = app.clients[0].units.clone();
    units[0].check = Some(weft_core::ledger::Check {
        eval_id: "evl_9".into(),
        report: Default::default(),
        judged_by: None,
    });
    let record = weft_core::work::record(&[
        json!({"type": "eval.opened", "id": "evl_9", "time": "2026-10-08T11:30:00Z",
               "data": {"basis": {"asks": ["ask_1"]}}}),
    ]);
    let work = weft_core::work::work(&units, &record);
    wire.event(Event::Units { units, records: json!({}), work });
    pumped(&mut app, |a| matches!(a.active_sheet, Some(ActiveSheet::EvalView { .. })));
    assert!(matches!(
        &app.active_sheet,
        Some(ActiveSheet::EvalView { eval, .. }) if eval == "evl_9"
    ));
    assert!(working_bar(&app).is_none());
    let row = app.render().find_layer("row unit fix login").unwrap().all_text();
    assert!(!row.contains(words::word("wait.row_opening_eval")), "{row}");
}

#[test]
fn an_eval_that_could_not_open_says_why_in_amber_until_the_person_acts() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    eval_staged(&mut app, &mut wire, "p1", None);
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let id = resolve_call(&mut wire);
    wire.event(Event::Resolved { id: "p1".into(), yes: true });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());
    assert!(working_bar(&app).is_some());
    wire.refuse(id, "the change could not be read (git exited 128).");
    fails_until_acted(
        &mut app,
        "RingFrame could not open the Eval: the change could not be read (git exited 128).",
    );

    // The daemon's own refusal says itself, with no RingFrame before it.
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    eval_staged(&mut app, &mut wire, "p2", None);
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p2".into(), answer: Answer::Yes });
    let id = resolve_call(&mut wire);
    wire.event(Event::Resolved { id: "p2".into(), yes: true });
    wire.refuse_as(id, "opening", "This work's Eval is already opening");
    fails_until_acted(&mut app, "This work's Eval is already opening");
}

/// An Eval another window or the phone opens shows here too, from the
/// daemon's word, and goes when the daemon says it is open.
#[test]
fn an_eval_opened_elsewhere_shows_opening_here_too() {
    let (_tmp, mut app, mut wire) = wired_app();
    wire.event(Event::Progress { job: opening_elsewhere("ask_3", None) });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    let bar = working_bar(&app).expect("opening, though not asked here");
    assert!(bar.contains(words::word("wait.opening_eval")), "{bar}");
    assert!(bar.contains("health endpoint · 1 Ask · read by codex"), "{bar}");
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_opening_eval")), "{row}");
    let done = weft_proto::JobEnd { ok: true, why: None };
    wire.event(Event::Progress { job: opening_elsewhere("ask_3", Some(done)) });
    pumped(&mut app, |a| a.clients[0].jobs.is_empty());
    assert!(working_bar(&app).is_none());
}

/// Seal: "Sealing…" with the work and the decision, and its row says so,
/// until the answer comes; a refusal stays.
#[test]
fn a_seal_says_it_is_sealing_until_answered() {
    let (_tmp, mut app, mut wire) = wired_app();
    super::seal_tests::no_questions(&mut app, &mut wire);
    app.dispatch(BarAct::ConfirmSeal {
        project: weft(),
        eval: "evl_3".into(),
        disposition: "accepted".into(),
        reason: None,
    });
    let id = act_call(&mut wire, "seal");
    let bar = working_bar(&app).expect("the bar works");
    assert!(bar.contains(words::word("wait.sealing")), "{bar}");
    assert!(bar.contains("health endpoint · accepted"), "{bar}");
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_sealing")), "{row}");
    wire.refuse(id, "RingFrame would not seal it.");
    fails_until_acted(&mut app, "RingFrame would not seal it.");
}

/// Each other yes that waits keeps the bar with its own words until its
/// answer comes.
#[test]
fn every_yes_that_waits_says_what_it_does_until_answered() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].view_mut(pane).unwrap().turn = Some(Turn::Ready);
    app.clients[0].sessions[2].pane = Some(pane);

    // Send.
    wire.event(Event::Pending {
        id: "p1".into(),
        pane,
        what: "Ask Claude?".into(),
        why: String::new(),
        payload: b"Refresh the token.".to_vec(),
        sends: None,
        go: None,
    });
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty());
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let id = resolve_call(&mut wire);
    wire.event(Event::Resolved { id: "p1".into(), yes: true });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());
    let bar = working_bar(&app).expect("sending");
    assert!(bar.contains(words::word("wait.sending")), "{bar}");
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.working().is_none());

    // Ask.
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    app.ask.set_text("refresh token 60s before expiry");
    app.dispatch(SheetAct::SubmitAsk);
    let id = act_call(&mut wire, "ask");
    assert!(working_bar(&app).unwrap().contains(words::word("wait.asking")));
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.working().is_none());

    // Stop Eval: its dialog made ready, then its yes.
    app.dispatch(SheetAct::StopEval { project: weft(), eval: "evl_3".into() });
    let id = act_call(&mut wire, "stop");
    assert!(working_bar(&app).unwrap().contains(words::word("wait.preparing")));
    wire.event(Event::Pending {
        id: "p2".into(),
        pane: u32::MAX,
        what: words::word("dialog.stop_eval.title").into(),
        why: "health endpoint\nThe judges stop.".into(),
        payload: Vec::new(),
        sends: None,
        go: Some(weft_proto::Go { word: "dialog.stop_eval.go".into(), warn: true }),
    });
    wire.answer(id, json!({}));
    pumped(&mut app, |a| !a.clients[0].waiting.is_empty() && a.working().is_none());
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p2".into(), answer: Answer::Yes });
    let id = resolve_call(&mut wire);
    wire.event(Event::Resolved { id: "p2".into(), yes: true });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());
    let bar = working_bar(&app).expect("stopping");
    assert!(bar.contains(words::word("wait.stopping_eval")), "{bar}");
    assert!(bar.contains("health endpoint"), "{bar}");
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.working().is_none());

    // Proceed.
    app.dispatch(SheetAct::Proceed { project: weft(), unit: "ask_2".into() });
    let id = act_call(&mut wire, "proceed");
    assert!(working_bar(&app).unwrap().contains(words::word("wait.preparing")));
    wire.refuse(id, "nothing to proceed with");
    fails_until_acted(&mut app, "nothing to proceed with");

    // Start an agent, and resume one.
    app.dispatch(TreeAct::StartAgent { project: weft(), harness: "codex".into() });
    let id = call_of(&mut wire, |c| matches!(c, Call::StartAgent { .. })).0;
    let starting = words::fill("wait.starting", &[("harness", "codex")]);
    assert!(working_bar(&app).unwrap().contains(&starting));
    wire.refuse(id, "codex would not start");
    fails_until_acted(&mut app, "codex would not start");
    app.clients[0].sessions[3].resumable = true;
    app.dispatch(SheetAct::Resume {
        project: weft(),
        harness: "claude".into(),
        session: "s4".into(),
    });
    let id = call_of(&mut wire, |c| matches!(c, Call::StartAgent { .. })).0;
    assert!(working_bar(&app).unwrap().contains(words::word("wait.resuming")));
    wire.answer(id, json!({"pane": 40}));
    pumped(&mut app, |a| a.working().is_none());

    // A setting saved.
    app.dispatch(crate::action::WindowAct::ToggleTurbo);
    let id = call_of(&mut wire, |c| matches!(c, Call::ConfigSet { .. })).0;
    assert!(working_bar(&app).unwrap().contains(words::word("wait.saving")));
    wire.refuse(id, "RingFrame refused the rule");
    fails_until_acted(&mut app, "RingFrame refused the rule");
}

/// Undo reads its commits, then its yes says "Undoing…"; with no commit
/// where the work started, it says so rather than nothing.
#[test]
fn undo_says_it_is_preparing_then_undoing() {
    use weft_core::after_seal::Offer;
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::Offer { project: weft(), unit: "ask_4".into(), offer: Offer::Undo });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "commits"));
    let bar = working_bar(&app).expect("its commits are read");
    assert!(bar.contains(words::word("wait.preparing")) && bar.contains("build number"), "{bar}");
    let commit = json!({"id": "4f2affff", "short": "4f2a9c1", "subject": "Tidy", "when": "11:02", "start": true});
    wire.answer(id, json!({"commits": [commit]}));
    pumped(&mut app, |a| matches!(a.bottom_bar, BottomBarState::Undo { .. }));
    app.dispatch(BarAct::Undo);
    let id = act_call(&mut wire, "undo");
    assert!(working_bar(&app).unwrap().contains(words::word("wait.undoing")));
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.working().is_none());

    app.dispatch(SheetAct::Offer { project: weft(), unit: "ask_4".into(), offer: Offer::Undo });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "commits"));
    wire.answer(id, json!({"commits": []}));
    fails_until_acted(&mut app, words::word("wait.no_start"));

    // A follow-up's words that could not be read say why.
    app.dispatch(SheetAct::Offer { project: weft(), unit: "ask_4".into(), offer: Offer::FollowUp });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "remedy"));
    assert!(working_bar(&app).unwrap().contains(words::word("wait.preparing")));
    wire.refuse(id, "the Eval left nothing");
    fails_until_acted(&mut app, "the Eval left nothing");
}

/// A message the daemon would not type is said, not dropped.
#[test]
fn a_message_refused_is_said() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.message_field().set_text("hello");
    app.dispatch(PaneAct::SendMessage);
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Say { .. }));
    wire.refuse(id, &format!("agent {pane} is not running"));
    fails_until_acted(&mut app, &format!("agent {pane} is not running"));
    // Typed, and refused by the agent: said too.
    app.message_field().set_text("again");
    app.dispatch(PaneAct::SendMessage);
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Say { .. }));
    wire.answer(id, json!({}));
    wire.event(Event::Injected {
        pane,
        refusal: Some("Weft did not type it: the agent is working.".into()),
        unrecorded: None,
    });
    fails_until_acted(&mut app, "Weft did not type it: the agent is working.");
}

/// A Details read: lines that breathe while it comes, never "none yet";
/// failed, its reason where it would be, with Try again.
#[test]
fn a_details_read_breathes_while_it_comes_and_says_why_it_failed() {
    let (_tmp, mut app, mut wire) = wired_app();
    // The Seal's receipt.
    app.clients[0].work.record.seals = vec![weft_core::work::Seal {
        id: "sel_4".into(),
        n: 4,
        at: String::new(),
        disposition: "accepted".into(),
        by: "you".into(),
        eval: Some("evl_2".into()),
        reason: None,
    }];
    app.dispatch(TreeAct::SelectSeal { project: weft(), seal: "sel_4".into() });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "seal"));
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().clone();
    let checking = sheet.find_layer("receipt checking").expect("checking").all_text();
    assert_eq!(checking, words::word("wait.checking_receipt"));
    wire.refuse(id, "seals/sel_4.json is missing");
    pumped(&mut app, |a| !a.clients[0].refused.is_empty());
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().clone();
    let failed = sheet.find_layer("failed").expect("why, where it would be").all_text();
    let said =
        words::fill("wait.could_not_check_receipt", &[("why", "seals/sel_4.json is missing")]);
    assert!(failed.contains(&said), "{failed}");
    assert!(failed.contains(words::word("wait.try_again")));
    // It stays while the person does other things, until Try again.
    app.dispatch(TreeAct::SelectProject(weft()));
    assert!(app.render().find_layer("failed").is_some());
    app.dispatch(SheetAct::TryAgain {
        project: weft(),
        then: crate::daemon::Then::Seal { seal: "sel_4".into() },
    });
    call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "seal"));
    let sheet = app.render().find_layer("sheet SealDetail").unwrap().clone();
    assert!(sheet.find_layer("failed").is_none(), "asked again");
    assert!(sheet.find_layer("receipt checking").is_some());
}

#[test]
fn the_eval_view_and_its_lines_breathe_while_read_and_say_why_they_failed() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "eval"));
    let sheet = app.render().find_layer("sheet EvalView").unwrap().clone();
    assert!(sheet.find_layer("loading").is_some());
    let text = sheet.all_text();
    assert!(!text.contains(words::word("desktop.eval.not_judged")), "{text}");
    assert!(!text.contains(words::word("desktop.eval.no_record")), "{text}");
    wire.refuse(id, "no record of evl_3");
    pumped(&mut app, |a| !a.clients[0].refused.is_empty());
    let failed = app.render().find_layer("failed").expect("why").all_text();
    assert!(failed.contains("This could not be read: no record of evl_3"), "{failed}");
    app.dispatch(SheetAct::TryAgain {
        project: weft(),
        then: crate::daemon::Then::EvalView { eval: "evl_3".into() },
    });
    call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "eval"));
    assert!(app.render().find_layer("loading").is_some(), "read again");
}

#[test]
fn what_was_asked_and_the_evals_record_say_why_they_could_not_be_read() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectAsk { project: weft(), unit: "ask_1".into() });
    let (source, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "source"));
    let (wording, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "wording"));
    wire.refuse(source, "the Ask's record is gone");
    wire.answer(wording, json!({"text": "Fix login."}));
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    let sheet = app.render().find_layer("sheet WorkDetail").unwrap().clone();
    let failed = sheet.find_layer("failed").expect("why").all_text();
    assert!(failed.contains("the Ask's record is gone"), "{failed}");
    assert!(sheet.all_text().contains("Fix login."));

    // An Eval's record the board does not hold, read through the daemon.
    app.clients[0].records = json!({});
    app.dispatch(TreeAct::SelectEval { project: weft(), eval: "evl_3".into() });
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "judges"));
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().clone();
    assert!(sheet.find_layer("loading").is_some(), "its findings breathe");
    wire.refuse(id, "no record");
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    let sheet = app.render().find_layer("sheet EvalDetail").unwrap().clone();
    assert!(sheet.find_layer("failed").unwrap().all_text().contains("no record"));
}

/// A conversation's first page breathes while it comes; failed, it says why
/// with Try again; an older page too.
#[test]
fn a_conversation_breathes_while_read_and_says_why_it_failed() {
    let (_tmp, mut app, mut wire) = wired_app();
    // Each harness's file says how its conversation is read.
    let read = weft_core::harness::fixture::harnesses().iter().find_map(|h| h.conversation.clone());
    assert!(read.is_some());
    for h in app.clients[0].harnesses.0.iter_mut() {
        h.conversation = read.clone();
    }
    let pane = start(&mut app, &mut wire, "claude");
    let group = app.state.panes.focused_group;
    let tab = app.state.panes.groups[group].active_tab;
    app.dispatch(PaneAct::SetView { group, tab, view: PaneViewMode::Conversation });
    let (id, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "conversation"));
    let shown = app.render().find_layer("pane conversation").unwrap().clone();
    assert!(shown.find_layer("loading").is_some(), "{shown:?}");
    wire.refuse(id, "the transcript is not there");
    pumped(&mut app, |a| !a.clients[0].refused.is_empty());
    let shown = app.render().find_layer("pane conversation").unwrap().clone();
    assert!(shown.find_layer("failed").unwrap().all_text().contains("the transcript is not there"));
    app.dispatch(SheetAct::TryAgain {
        project: weft(),
        then: crate::daemon::Then::Conversation { pane },
    });
    let (id, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "conversation"));
    wire.answer(id, json!({"conversation": {"items": [], "before": "c1", "after": "c9"}}));
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    let shown = app.render().find_layer("pane conversation").unwrap().clone();
    assert!(shown.find_layer("failed").is_none() && shown.find_layer("loading").is_none());
    // The page before.
    app.dispatch(PaneAct::Older { project: weft(), pane });
    let (id, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "conversation"));
    assert!(app.render().find_layer("pane conversation").unwrap().find_layer("loading").is_some());
    wire.refuse(id, "older page gone");
    pumped(&mut app, |a| !a.clients[0].refused.is_empty());
    let shown = app.render().find_layer("pane conversation").unwrap().clone();
    assert!(shown.find_layer("failed").unwrap().all_text().contains("older page gone"));
}

/// Searching earlier sessions says it searches, never "nothing matches",
/// until the daemon answers; a failed read says why with Try again.
#[test]
fn a_search_says_it_searches_until_answered() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.search.set_text("zebra");
    app.pump();
    let (id, _) =
        call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "sessions"));
    let tree = app.render().find_layer("tree").unwrap().clone();
    assert!(tree.find_layer("search searching").is_some());
    assert!(tree.find_layer("search empty").is_none(), "not nothing yet");
    wire.answer(id, json!({"rows": [], "total": 0, "next": null}));
    pumped(&mut app, |a| a.clients[0].earlier_found.as_ref().is_some_and(|f| !f.asking()));
    let tree = app.render().find_layer("tree").unwrap().clone();
    assert!(tree.find_layer("search empty").is_some());

    // Every earlier session's first page, refused: why, and Try again.
    app.search.set_text("");
    app.dispatch(TreeAct::OpenEarlier(weft()));
    app.clients[0].earlier_rows = Default::default();
    app.clients[0].earlier_rows.total = 14;
    app.clients[0].more_earlier(false);
    let _ = app.clients[0].earlier_rows.restart();
    app.clients[0].earlier_again(false);
    app.clients[0].earlier_rows.failed = Some("the index is busy".into());
    let tree = app.render().find_layer("tree").unwrap().clone();
    let failed = tree.find_layer("failed").expect("why").all_text();
    assert!(failed.contains("the index is busy"), "{failed}");
    assert!(tree.find_action(&TreeAct::EarlierAgain(weft()).into()).is_some());
}

/// Settings' RingFrame reads breathe while asked; refused, they say why
/// with Try again.
#[test]
fn settings_reads_breathe_and_say_why_they_were_refused() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(crate::action::WindowAct::OpenSettings);
    app.dispatch(SheetAct::SettingsTab(crate::action::SettingsTab::RingFrame));
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "rules"));
    let busy = app.render().find_layer("busy").expect("asked").all_text();
    assert_eq!(busy, words::word("desktop.settings.asking"));
    wire.refuse(id, "RingFrame is not installed");
    pumped(&mut app, |a| !a.clients[0].refused.is_empty());
    let failed = app.render().find_layer("failed").expect("why").all_text();
    assert!(failed.contains("RingFrame is not installed"), "{failed}");
    app.dispatch(SheetAct::TryAgain {
        project: weft(),
        then: crate::daemon::Then::Rules { project: false },
    });
    call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "rules"));
}

/// The Ask's sheet, before its first read comes, says it is asked rather
/// than showing nothing.
#[test]
fn the_ask_sheet_says_it_is_read_before_its_first_answer() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    app.clients[0].sessions[2].pane = Some(pane);
    app.selection = Some(TreeSelection::Agent { project: weft(), session: "s3".into() });
    app.dispatch(BarAct::Ask);
    call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "ask"));
    let sheet = app.render().find_layer("sheet Ask").unwrap().clone();
    assert!(sheet.find_layer("ask reading").is_some(), "{}", sheet.all_text());
}

/// Updates: "checking…" while the release is asked, never "could not
/// check"; an update of RingFrame says "Updating…" with its time, its
/// button and Update all rest, and the step that waits says what for.
#[test]
fn updates_say_checking_and_updating_and_rest_the_others() {
    let (_tmp, mut app, mut wire) = wired_app();
    let row = |name: &str, state| SetupRow {
        name: name.into(),
        title: name.into(),
        program: name.into(),
        state,
        version: None,
    };
    let pending = SetupView {
        weft: "0.8.0".into(),
        configuration: "local".into(),
        sense: "0.1.3".into(),
        pending_release: true,
        rows: vec![row("grok", SetupState::Checking)],
        ..Default::default()
    };
    app.clients[0].setup = Some(pending);
    app.active_sheet = Some(ActiveSheet::Updates);
    let sheet = app.render().find_layer("sheet Updates").unwrap().clone();
    for name in ["Weft", "RingFrame", "Sense"] {
        let r = sheet.find_layer(&format!("update row {name}")).unwrap().all_text();
        assert!(r.contains(words::word("wait.checking")), "{r}");
        assert!(!r.contains(words::word("desktop.updates.unknown")), "{r}");
    }
    let grok = sheet.find_layer("update row grok").unwrap().all_text();
    assert!(grok.contains(words::word("wait.checking")), "{grok}");

    let behind = SetupView {
        weft: "0.8.0".into(),
        weft_latest: Some("0.8.0".into()),
        configuration: "local → 0.8.0".into(),
        configuration_behind: true,
        latest: Some("0.8.0".into()),
        sense: "0.1.3 → 0.1.4".into(),
        sense_latest: Some("0.1.4".into()),
        sense_behind: true,
        rows: vec![row("claude", SetupState::Running("claude plugin install rf@fab7".into()))],
        ..Default::default()
    };
    wire.event(Event::Setup { view: serde_json::to_value(&behind).unwrap() });
    pumped(&mut app, |a| a.setup_view().is_some_and(|v| v.configuration_behind));
    app.dispatch(SheetAct::SetupRun("ringframe".into()));
    let (id, _) =
        call_of(&mut wire, |c| matches!(c, Call::Setup { harness, .. } if harness == "ringframe"));
    let sheet = app.render().find_layer("sheet Updates").unwrap().clone();
    let rf = sheet.find_layer("update row RingFrame").unwrap();
    assert!(rf.all_text().contains(words::word("wait.updating")), "{}", rf.all_text());
    assert!(rf.find_layer("working time").is_some() && rf.find_layer("moving bar").is_some());
    assert!(rf.find_layer("btn Update RingFrame").is_none(), "no button while it runs");
    let sense = sheet.find_layer("update row Sense").unwrap();
    let waits = words::fill("wait.waits_for", &[("name", "RingFrame")]);
    assert!(sense.all_text().contains(&waits), "{}", sense.all_text());
    assert!(sense.find_layer("btn Update Sense").unwrap().action.is_none(), "it rests");
    assert!(sheet.find_layer("btn Update all").unwrap().action.is_none(), "Update all waits");
    let claude = sheet.find_layer("update row claude").unwrap();
    assert!(claude.find_layer("breathing dot").is_some());
    assert!(claude.find_layer("working time").is_some(), "{}", claude.all_text());
    // Pressed again while it runs, nothing more is asked.
    app.dispatch(SheetAct::SetupRun("sense".into()));
    assert!(wire.calls().iter().all(|(_, c)| !matches!(c, Call::Setup { .. })));

    // What is installed, said while it runs, does not end it: the answer does.
    let done = SetupView { configuration: "0.8.0".into(), configuration_behind: false, ..behind };
    wire.event(Event::Setup { view: serde_json::to_value(&done).unwrap() });
    pumped(&mut app, |a| a.setup_view().is_some_and(|v| !v.configuration_behind));
    assert!(app.updating().is_some(), "still owed");
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.updating().is_none());
    let sheet = app.render().find_layer("sheet Updates").unwrap().clone();
    let sense = sheet.find_layer("update row Sense").unwrap();
    assert!(sense.find_layer("btn Update Sense").unwrap().action.is_some());
}

/// The header's "Connecting" breathes.
#[test]
fn connecting_breathes() {
    let (_tmp, app) = super::tests::sample_app(false);
    let tag = app.render().find_layer("link tag").expect("connecting").clone();
    assert!(tag.find_layer("breathing dot").is_some());
    assert_eq!(tag.all_text(), words::word("web.link.connecting"));
}

/// A Remote act is said at once, until `weft remote` says where it is.
#[test]
fn a_remote_act_is_said_until_its_view_moves() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(crate::action::WindowAct::OpenRemote);
    app.dispatch(SheetAct::RemoteOn);
    call_of(&mut wire, |c| matches!(c, Call::Remote { .. }));
    assert!(app.render().find_layer("remote asked").is_some());
    wire.event(Event::Remote { view: json!({"state": "starting"}) });
    pumped(&mut app, |a| a.remote_asked.is_none());
    assert!(app.render().find_layer("remote asked").is_none());
}

/// Another reader picked in the Eval dialog: the dialog shown stays, and
/// its Read by breathes until the new one comes.
#[test]
fn a_new_reader_picked_breathes_until_its_dialog_comes() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    eval_staged(&mut app, &mut wire, "p1", None);
    let row = app.render().find_layer("read by").unwrap().clone();
    assert!(row.find_layer("breathing dot").is_none());
    app.dispatch(BarAct::ReadBy("codex".into()));
    let row = app.render().find_layer("read by").unwrap().clone();
    assert!(row.find_layer("breathing dot").is_some(), "made again for codex");
    eval_staged(&mut app, &mut wire, "p2", Some("codex"));
    wire.event(Event::Resolved { id: "p1".into(), yes: false });
    pumped(&mut app, |a| a.clients[0].waiting.len() == 1);
    let row = app.render().find_layer("read by").unwrap().clone();
    assert!(row.find_layer("breathing dot").is_none());
}

/// A project's own settings: RingFrame's rows breathe while read, and say
/// why when RingFrame would not.
#[test]
fn a_projects_ringframe_rows_breathe_and_say_why_they_were_refused() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(TreeAct::OpenProjectSettings(weft()));
    let reads: Vec<u64> = (0..2)
        .map(|_| call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "rules")).0)
        .collect();
    assert!(app.render().find_layer("loading").is_some());
    let none = words::fill("desktop.project.none", &[("project", "weft")]);
    assert!(!app.render().all_text().contains(&none), "nothing said to differ while it reads");
    for id in reads {
        wire.refuse(id, "RingFrame is not installed");
    }
    pumped(&mut app, |a| !a.clients[0].owes_any(|_| true));
    let failed = app.render().find_layer("failed").expect("why").all_text();
    assert!(failed.contains("RingFrame is not installed"), "{failed}");
}

/// Said why not while something works: under its working bar, not lost.
#[test]
fn a_why_not_while_something_works_is_said_under_its_bar() {
    let (_tmp, mut app, mut wire) = wired_app();
    wire.event(Event::Progress { job: opening_elsewhere("ask_3", None) });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    app.selection = None;
    app.dispatch(BarAct::Seal);
    let bar = app.render().find_layer("working bar").unwrap().clone();
    assert!(bar.find_layer("failed sentence").is_some(), "{}", bar.all_text());
}

/// A program the person found is checked on its row until the daemon says
/// what it found.
#[test]
fn a_program_found_is_checked_on_its_row() {
    let (_tmp, mut app, mut wire) = wired_app();
    let row = |state| SetupRow {
        name: "codex".into(),
        title: "Codex".into(),
        program: "codex".into(),
        state,
        version: None,
    };
    let view = |state| SetupView { rows: vec![row(state)], ..Default::default() };
    app.clients[0].setup = Some(view(SetupState::NotFound));
    app.active_sheet = Some(ActiveSheet::Updates);
    app.locate("codex", std::path::Path::new("/opt/bin/codex"));
    let (id, _) = call_of(&mut wire, |c| matches!(c, Call::Setup { act, .. } if act == "locate"));
    let r = app.render().find_layer("update row Codex").unwrap().clone();
    assert!(r.find_layer("busy").is_some(), "{}", r.all_text());
    let ready = view(SetupState::Ready { version: None });
    wire.event(Event::Setup { view: serde_json::to_value(&ready).unwrap() });
    wire.answer(id, json!({}));
    pumped(&mut app, |a| a.updating().is_none());
    assert!(app.render().find_layer("update row Codex").unwrap().find_layer("busy").is_none());
}
