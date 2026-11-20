//! Every wait shows its progress: the machine's jobs, taken from its
//! `progress` events and from a project's opening, show their steps, their
//! count and how far along they are, on the wait each belongs to.

use std::time::{Duration, Instant};

use serde_json::json;
use weft_core::onboarding::{Row as SetupRow, State as SetupState, View as SetupView};
use weft_core::turns::Turn;
use weft_proto::{Call, Event, Job};

use super::tests::{Wire, answer_view, pumped, start, wired_app};
use super::waiting_tests::{act_call, call_of, fails_until_acted, resolve_call, weft, working_bar};
use super::*;
use crate::action::{Answer, BarAct, SheetAct, TreeAct};
use crate::gpui::Element;

/// A job as the daemon sends it.
fn job(v: serde_json::Value) -> Job {
    serde_json::from_value(v).expect("a job")
}

fn step(key: &str, state: &str) -> serde_json::Value {
    json!({"key": key, "state": state})
}

/// The Eval dialog's yes, waiting on the machine: the call it made.
fn eval_said_yes(app: &mut WeftDesktopApp, wire: &mut Wire) -> u64 {
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    super::tests::eval_staged(app, wire, "p1", None);
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::Yes });
    let id = resolve_call(wire);
    wire.event(Event::Resolved { id: "p1".into(), yes: true });
    pumped(app, |a| a.clients[0].waiting.is_empty());
    id
}

/// An Eval opening: reading the change, 38 of 103 files, 9 new.
fn opening(call: Option<u64>, work: &str) -> Job {
    let mut reading = step("step.reading_change", "now");
    reading["ms"] = json!(18_000);
    reading["done"] = json!(38);
    reading["total"] = json!(103);
    reading["unit"] = json!("files");
    reading["args"] = json!({"new": "9"});
    let mut starting = step("step.starting_readers", "next");
    starting["args"] = json!({"harness": "antigravity"});
    job(json!({
        "id": "job_1", "kind": "eval_open", "on": {"work": work}, "call": call,
        "steps": [reading, step("step.dividing", "next"),
                  step("step.preparing_readers", "next"), starting],
        "share": 0.2, "age_ms": 18_000,
    }))
}

/// A yes's job, by the call that began it: the bar lists its steps, the one
/// under way with its count, and fills by the share done; the line it said
/// before its steps came goes.
#[test]
fn a_yes_shows_the_steps_of_the_job_its_call_began() {
    let (_tmp, mut app, mut wire) = wired_app();
    let id = eval_said_yes(&mut app, &mut wire);
    // Before its job comes: as before, the line and no steps.
    let bar = app.render().find_layer("working bar").unwrap().clone();
    assert!(bar.find_layer("steps").is_none());
    assert!(bar.all_text().contains(words::word("wait.opening_eval_line")));

    // Another call's job is not this wait's.
    let other = job(json!({"id": "job_9", "kind": "send", "on": {"pane": 4}, "call": id + 99,
                           "steps": [step("step.typing_prompt", "now")], "age_ms": 500}));
    wire.event(Event::Progress { job: other });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    assert!(app.working().unwrap().progress.is_none(), "not this wait's job");

    // Its own, by its call, though it names another work.
    wire.event(Event::Progress { job: opening(Some(id), "ask_9") });
    pumped(&mut app, |a| a.working().is_some_and(|w| w.progress.is_some()));
    assert_eq!(app.working().unwrap().progress.unwrap().job.id, "job_1");
    let bar = app.render().find_layer("working bar").unwrap().clone();
    let text = bar.all_text();
    for said in [words::word("wait.opening_eval"), "fix login · 1 Ask · read by antigravity"] {
        assert!(text.contains(said), "{said} in {text}");
    }
    assert!(!text.contains(words::word("wait.opening_eval_line")), "its steps say it now");
    let now = bar.find_layer("step now").expect("the step under way").all_text();
    assert!(now.contains("Reading the change"), "{now}");
    assert!(now.contains("38 of 103 files"), "{now}");
    assert!(now.contains("· 9 of them new"), "{now}");
    assert!(now.contains("0:18"), "its own time: {now}");
    let next: Vec<String> = bar
        .find_layer("steps")
        .unwrap()
        .children
        .iter()
        .filter(|c| c.layer_name.as_deref() == Some("step next"))
        .map(Element::all_text)
        .collect();
    assert_eq!(next.len(), 3, "{next:?}");
    assert!(next[2].contains("Starting antigravity to read it"), "{next:?}");
    let filled = bar.find_layer("filled").expect("the bar fills by the share done");
    assert!(filled.attrs.contains(&("data-share".into(), "0.20".into())), "{:?}", filled.attrs);
    assert!(bar.find_layer("moving bar").is_none());

    // It moves on: the step read is ticked, with its time.
    let mut moved = opening(Some(id), "ask_9");
    moved.steps[0].state = weft_proto::StepState::Done;
    moved.steps[1].state = weft_proto::StepState::Now;
    wire.event(Event::Progress { job: moved });
    pumped(&mut app, |a| {
        a.render().find_layer("working bar").and_then(|b| b.find_layer("step done")).is_some()
    });
    let bar = app.render().find_layer("working bar").unwrap().clone();
    let done = bar.find_layer("step done").unwrap().all_text();
    assert!(done.contains("✓") && done.contains("Reading the change") && done.contains("0:18"));
    assert!(!done.contains("38 of 103"), "a count shows only while it counts: {done}");
    assert!(bar.find_layer("step now").unwrap().all_text().contains("Dividing it among"));
}

/// Sealing: its steps with their times, the files read counted.
#[test]
fn a_seal_lists_its_steps_with_their_times_and_its_count() {
    let (_tmp, mut app, mut wire) = wired_app();
    super::seal_tests::no_questions(&mut app, &mut wire);
    app.dispatch(BarAct::ConfirmSeal {
        project: weft(),
        eval: "evl_3".into(),
        disposition: "accepted".into(),
        reason: None,
    });
    let id = act_call(&mut wire, "seal");
    let mut checking = step("step.checking_work", "done");
    checking["ms"] = json!(2_000);
    let mut reading = step("step.reading_files_now", "now");
    reading["ms"] = json!(7_000);
    reading["done"] = json!(8_412);
    reading["total"] = json!(15_902);
    reading["unit"] = json!("files");
    wire.event(Event::Progress {
        job: job(json!({
            "id": "job_2", "kind": "seal", "on": {"work": "ask_3"}, "call": id,
            "steps": [checking, reading, step("step.writing_decision", "next"),
                      step("step.noting_git", "next")],
            "share": 0.53, "age_ms": 9_000,
        })),
    });
    pumped(&mut app, |a| a.working().is_some_and(|w| w.progress.is_some()));
    let bar = app.render().find_layer("working bar").unwrap().clone();
    let text = bar.all_text();
    assert!(text.contains(words::word("wait.sealing")), "{text}");
    assert!(text.contains("health endpoint · accepted"), "{text}");
    let done = bar.find_layer("step done").unwrap().all_text();
    assert!(done.contains("Checking the work and its Eval") && done.contains("0:02"), "{done}");
    let now = bar.find_layer("step now").unwrap().all_text();
    assert!(now.contains("8,412 of 15,902 files"), "{now}");
    assert!(bar.find_layer("step next").unwrap().all_text().contains("Writing your decision"));
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_sealing")), "{row}");
}

/// A job on a work, from anywhere, says so on the work's row until it ends.
#[test]
fn a_jobs_work_says_so_on_its_row() {
    let (_tmp, mut app, mut wire) = wired_app();
    let sealing = |end: Option<serde_json::Value>| {
        job(json!({"id": "job_5", "kind": "seal", "on": {"work": "ask_3"},
                   "steps": [step("step.checking_work", "now")], "age_ms": 600, "end": end}))
    };
    wire.event(Event::Progress { job: sealing(None) });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_sealing")), "{row}");
    let other = app.render().find_layer("row unit fix login").unwrap().all_text();
    assert!(!other.contains(words::word("wait.row_sealing")), "{other}");
    wire.event(Event::Progress { job: sealing(Some(json!({"ok": true}))) });
    pumped(&mut app, |a| a.clients[0].jobs.is_empty());
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(!row.contains(words::word("wait.row_sealing")), "{row}");
}

/// An open that failed, begun elsewhere, says why in amber until the
/// person acts; pressing that work's Eval again forgets it.
#[test]
fn a_failed_job_says_why_until_the_person_acts() {
    let (_tmp, mut app, mut wire) = wired_app();
    let mut failed = opening(None, "ask_1");
    failed.end = Some(weft_proto::JobEnd { ok: false, why: Some("git exited 128".into()) });
    wire.event(Event::Progress { job: failed });
    fails_until_acted(&mut app, "RingFrame could not open the Eval: git exited 128");
    assert!(app.clients[0].job_failed(|j| j.kind == "eval_open").is_some(), "kept");
    app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
    app.dispatch(BarAct::Eval);
    assert!(app.clients[0].jobs.is_empty(), "Eval pressed again");
}

/// The jobs a project's opening carries restore what runs and what failed.
#[test]
fn the_jobs_a_project_opens_with_restore_its_waits() {
    let (_tmp, mut app, _wire) = wired_app();
    let mut failed = job(json!({"id": "job_7", "kind": "seal", "on": {"work": "ask_1"},
                                "steps": [], "age_ms": 4_000}));
    failed.end = Some(weft_proto::JobEnd { ok: false, why: Some("the record moved".into()) });
    let c = &app.clients[0];
    let open = json!({"units": c.units, "work": c.work, "jobs": [opening(None, "ask_3"), failed]});
    app.clients[0].take_opened(&open);
    app.pump();
    assert_eq!(app.failed.as_deref(), Some("the record moved"));
    let w = app.working().expect("the open still runs");
    assert!(w.opening && w.since <= Instant::now() - Duration::from_secs(18), "timed by its age");
    let bar = working_bar(&app).unwrap();
    assert!(bar.contains("38 of 103 files"), "{bar}");
    assert!(bar.contains("health endpoint · 1 Ask · read by"), "{bar}");
    let row = app.render().find_layer("row unit health endpoint").unwrap().all_text();
    assert!(row.contains(words::word("wait.row_opening_eval")), "{row}");
    assert!(app.tick(Instant::now() + Duration::from_secs(2)), "redrawn each second while it runs");
}

/// The readers: each step with its time, how many reads are in, and
/// how long the reader has read.
#[test]
fn the_eval_view_shows_its_readers_steps_counted() {
    use weft_core::eval_view::{EvalView, Sources};
    let (_tmp, mut app, mut wire) = wired_app();
    let brief = json!({"asks": [{"ask_id": "a", "title": "fix login"}]});
    let planned = ["evl_3~rd1".to_string()];
    let running = EvalView::build(Sources {
        eval_id: "evl_3",
        changes: &json!({}),
        evidence: &json!({}),
        brief: &brief,
        record: None,
        outputs: &[],
        planned: &planned,
        done: &[],
        checks_running: &[],
    });
    let mut judge = PaneView::new(9, "claude".into(), "claude".into());
    (judge.judge, judge.turn) = (Some("evl_3".into()), Some(Turn::Working));
    app.clients[0].panes.push(judge);
    let mut asked = step("eval_view.step.asked", "done");
    asked["ms"] = json!(6_000);
    let mut reading = step("eval_view.step.reading", "now");
    reading["ms"] = json!(271_000);
    reading["done"] = json!(3);
    reading["total"] = json!(7);
    reading["unit"] = json!("reads");
    wire.event(Event::Progress {
        job: job(json!({
            "id": "job_3", "kind": "readers", "on": {"eval": "evl_3", "work": "ask_1"},
            "steps": [asked, reading, step("eval_view.step.again_next", "next"),
                      step("eval_view.step.questions_next", "next")],
            "age_ms": 252_000,
        })),
    });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    app.dispatch(SheetAct::EvalView { project: weft(), eval: "evl_3".into() });
    answer_view(&mut app, &mut wire, &running);
    let progress = app.render().find_layer("eval progress").expect("it runs").clone();
    let done = progress.find_layer("step done").unwrap().all_text();
    assert!(done.contains("What you asked") && done.contains("0:06"), "{done}");
    let now = progress.find_layer("step now").unwrap().all_text();
    assert!(now.contains("Reading the change, all of it at once"), "{now}");
    assert!(now.contains("4:31") && now.contains("3 of 7 read"), "{now}");
    let next = progress.find_layer("step next").unwrap().all_text();
    assert!(next.contains("Reading again where unsure, if needed"), "{next}");
    let judge = progress.find_layer("judge 9").unwrap().all_text();
    assert!(judge.starts_with("claude is reading · 4:12"), "{judge}");
}

fn setup_row(name: &str, title: &str, state: SetupState) -> SetupRow {
    SetupRow { name: name.into(), title: title.into(), program: name.into(), state, version: None }
}

/// Updates: RingFrame's steps on one line, Sense's MB counted, a
/// harness's step of three; a failed update says why until run again.
#[test]
fn updates_show_each_updates_steps_and_count() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.clients[0].setup = Some(SetupView {
        weft: "0.8.0".into(),
        weft_latest: Some("0.8.0".into()),
        configuration: "local → 0.8.0".into(),
        configuration_behind: true,
        latest: Some("0.8.0".into()),
        sense: "0.1.3 → 0.1.4".into(),
        sense_latest: Some("0.1.4".into()),
        sense_behind: true,
        rows: vec![setup_row(
            "claude",
            "Claude Code",
            SetupState::Running("claude plugin install rf@fab7".into()),
        )],
        ..Default::default()
    });
    app.active_sheet = Some(ActiveSheet::Updates);
    let mut found = step("step.finding_release", "done");
    found["ms"] = json!(3_000);
    wire.event(Event::Progress {
        job: job(json!({"id": "job_r", "kind": "update", "on": {"harness": "ringframe"},
                        "steps": [found, step("step.downloading", "now"),
                                  step("step.installing", "next")],
                        "share": 0.45, "age_ms": 12_000})),
    });
    let mut downloading = step("step.downloading", "now");
    downloading["ms"] = json!(21_000);
    downloading["done"] = json!(41_000_000);
    downloading["total"] = json!(94_000_000);
    downloading["unit"] = json!("bytes");
    wire.event(Event::Progress {
        job: job(json!({"id": "job_s", "kind": "update", "on": {"harness": "sense"},
                        "steps": [downloading], "share": 0.44, "age_ms": 21_000})),
    });
    let mut running = step("step.running", "now");
    running["ms"] = json!(5_000);
    running["args"] = json!({"command": "claude plugin install rf@fab7"});
    wire.event(Event::Progress {
        job: job(json!({"id": "job_c", "kind": "update", "on": {"harness": "claude"},
                        "steps": [step("step.finding_release", "done"), running,
                                  step("step.installing", "next")],
                        "age_ms": 9_000})),
    });
    pumped(&mut app, |a| a.clients[0].jobs.len() == 3);
    let sheet = app.render().find_layer("sheet Updates").unwrap().clone();
    let rf = sheet.find_layer("update row RingFrame").unwrap();
    assert!(rf.all_text().contains(words::word("wait.updating")), "{}", rf.all_text());
    assert!(rf.find_layer("btn Update RingFrame").is_none(), "no button while it runs");
    assert!(rf.find_layer("filled").is_some());
    let steps = rf.find_layer("steps").unwrap();
    assert!(steps.find_layer("step done").unwrap().all_text().contains("Finding the newest"));
    assert!(steps.find_layer("step now").unwrap().all_text().contains("Downloading"));
    assert!(steps.find_layer("step next").unwrap().all_text().contains("Installing"));
    assert_eq!(rf.find_layer("working time").unwrap().all_text(), "0:12");
    let sense = sheet.find_layer("update row Sense").unwrap();
    let said = sense.all_text();
    assert!(said.contains("Downloading") && said.contains("41 of 94 MB"), "{said}");
    assert_eq!(sense.find_layer("working time").unwrap().all_text(), "0:21", "its step's own");
    let claude = sheet.find_layer("update row Claude Code").unwrap().all_text();
    assert!(claude.contains("step 2 of 3"), "{claude}");
    assert!(claude.contains("running: claude plugin install rf@fab7"), "{claude}");
    assert!(claude.contains("0:05"), "{claude}");
    assert!(app.updating().is_some(), "an update runs, as the machine says");

    // RingFrame's update failed: why, under its row, until it runs again.
    let failed = job(json!({"id": "job_r", "kind": "update", "on": {"harness": "ringframe"},
                            "steps": [], "age_ms": 30_000,
                            "end": {"ok": false, "why": "the release could not be read"}}));
    for id in ["job_s", "job_c"] {
        wire.event(Event::Progress {
            job: job(json!({"id": id, "kind": "update", "steps": [], "age_ms": 1,
                            "end": {"ok": true}})),
        });
    }
    wire.event(Event::Progress { job: failed });
    pumped(&mut app, |a| a.clients[0].jobs.len() == 1 && a.updating().is_none());
    assert!(app.failed.is_none(), "said in its row, not the bar");
    let sheet = app.render().find_layer("sheet Updates").unwrap().clone();
    let why = sheet.find_layer("job failed").expect("why").all_text();
    assert!(why.contains("the release could not be read"), "{why}");
    app.dispatch(SheetAct::SetupRun("ringframe".into()));
    call_of(&mut wire, |c| matches!(c, Call::Setup { .. }));
    assert!(app.clients[0].jobs.is_empty(), "run again");
}

/// A Seal's receipt check: its files counted and a small bar.
#[test]
fn a_receipt_check_counts_its_files() {
    let (_tmp, mut app, mut wire) = wired_app();
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
    call_of(&mut wire, |c| matches!(c, Call::Read { what, .. } if what == "seal"));
    let mut reading = step("step.reading_files_now", "now");
    reading["done"] = json!(8_412);
    reading["total"] = json!(15_902);
    reading["unit"] = json!("files");
    wire.event(Event::Progress {
        job: job(json!({"id": "job_k", "kind": "seal_check", "on": {"seal": "sel_4"},
                        "steps": [reading], "share": 0.53, "age_ms": 2_000})),
    });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    let line = app.render().find_layer("receipt checking").expect("checking").clone();
    let text = line.all_text();
    assert!(text.contains(words::word("wait.checking_receipt")), "{text}");
    assert!(text.contains("8,412 of 15,902 files"), "{text}");
    assert!(line.find_layer("receipt progress").is_some());
}

/// Remote turning on: its steps with their times, and the note of
/// the one that can take a while.
#[test]
fn remote_turning_on_lists_its_steps() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(crate::action::WindowAct::OpenRemote);
    app.dispatch(SheetAct::RemoteOn);
    call_of(&mut wire, |c| matches!(c, Call::Remote { .. }));
    let done = |key: &str, ms: u64| json!({"key": key, "state": "done", "ms": ms});
    let mut waiting = step("step.waiting_address", "now");
    waiting["ms"] = json!(41_000);
    wire.event(Event::Progress {
        job: job(json!({"id": "job_m", "kind": "remote",
                        "steps": [done("step.starting_remote", 2_000),
                                  done("step.signing_in", 31_000),
                                  done("step.starting_tunnel", 4_000), waiting],
                        "share": 0.75, "age_ms": 78_000})),
    });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    let sheet = app.render().find_layer("sheet Remote").unwrap().clone();
    let on = sheet.find_layer("remote turning on").expect("turning on").clone();
    let text = on.all_text();
    assert!(text.contains(words::word("wait.turning_on_remote")), "{text}");
    assert!(text.contains("Starting Weft's remote 0:02"), "{text}");
    let now = on.find_layer("step now").unwrap().all_text();
    assert!(now.contains("Waiting until the address answers"), "{now}");
    assert!(now.contains(words::word("step.waiting_address_note")), "{now}");
    assert_eq!(on.find_layer("working time").unwrap().all_text(), "1:18");
    assert!(sheet.find_layer("remote asked").is_none(), "its steps say it");

    let failed = job(json!({"id": "job_m", "kind": "remote", "steps": [], "age_ms": 80_000,
                            "end": {"ok": false, "why": "the tunnel did not answer"}}));
    wire.event(Event::Progress { job: failed });
    pumped(&mut app, |a| a.clients[0].job_failed(|_| true).is_some());
    let why = app.render().find_layer("remote failed").expect("why").all_text();
    assert!(why.contains("the tunnel did not answer"), "{why}");
}

/// A send's job breathes on its agent's tab.
#[test]
fn a_send_breathes_on_its_agents_tab() {
    let (_tmp, mut app, mut wire) = wired_app();
    let pane = start(&mut app, &mut wire, "claude");
    let tab = |app: &WeftDesktopApp| {
        app.render().find_layer("tab strip").unwrap().find_layer("tab working").is_some()
    };
    assert!(!tab(&app));
    wire.event(Event::Progress {
        job: job(json!({"id": "job_t", "kind": "send", "on": {"pane": pane},
                        "steps": [step("step.typing_prompt", "now")], "age_ms": 500})),
    });
    pumped(&mut app, |a| !a.clients[0].jobs.is_empty());
    assert!(tab(&app));
}

/// The Eval dialog says what it will read; a large change says so in amber,
/// and its yes reads "Eval anyway".
#[test]
fn the_eval_dialog_says_what_it_reads_and_warns_of_a_large_change() {
    let (_tmp, mut app, mut wire) = wired_app();
    let staged = |app: &mut WeftDesktopApp, wire: &mut Wire, id: &str, reads: serde_json::Value| {
        app.dispatch(TreeAct::SelectWork { project: weft(), unit: "ask_1".into() });
        app.dispatch(BarAct::Eval);
        let call = act_call(wire, "eval");
        wire.event(Event::Pending {
            id: id.into(),
            pane: u32::MAX,
            what: words::word("dialog.eval.title").into(),
            why: "fix login · 1 Ask\ncodex reads the whole change.".into(),
            payload: serde_json::to_vec(&json!({ "reads": reads })).unwrap(),
            sends: None,
            go: Some(weft_proto::Go { word: "act.eval".into(), warn: false }),
        });
        wire.answer(call, json!({"pending": id}));
        let id = id.to_string();
        pumped(app, |a| a.clients[0].waiting.iter().any(|w| w.id == id));
        app.render().find_layer("Cancel / Send bar").unwrap().clone()
    };
    let small = json!({"since": "ask", "when": "2026-11-08T09:12:00Z", "commits": 3, "files": 14,
                       "skipped": {"generated": 1, "vendored": 0, "binary": 1, "lock": 0},
                       "large": false});
    let bar = staged(&mut app, &mut wire, "p1", small);
    let reads = bar.find_layer("reads").expect("Reads").all_text();
    assert!(reads.starts_with(words::word("reads.label")), "{reads}");
    assert!(reads.contains("3 commits · 14 files"), "{reads}");
    assert!(reads.contains("since its first Ask, Nov 8"), "{reads}");
    assert!(reads.contains("· 2 counted, not read: 1 generated, 1 binary"), "{reads}");
    assert!(bar.find_layer("reads large").is_none());
    assert!(bar.find_layer("payload").is_none(), "it types nothing");
    assert!(bar.find_layer("btn Eval").is_some());
    app.dispatch(BarAct::Resolve { project: weft(), pending: "p1".into(), answer: Answer::No });
    wire.event(Event::Resolved { id: "p1".into(), yes: false });
    pumped(&mut app, |a| a.clients[0].waiting.is_empty());

    let large = json!({"since": "seal", "when": "2026-11-08T09:12:00Z", "commits": 52,
                       "files": 13_852, "skipped": {},
                       "folder": "research/papers/to-err-is-agentic/evidence/",
                       "folder_files": 13_708, "large": true});
    let bar = staged(&mut app, &mut wire, "p2", large);
    let reads = bar.find_layer("reads").unwrap().all_text();
    assert!(reads.contains("52 commits · 13,852 files"), "{reads}");
    assert!(reads.contains("since its last Seal, Nov 8"), "{reads}");
    assert!(!reads.contains("counted, not read"), "{reads}");
    let note = bar.find_layer("reads large").expect("the amber note").all_text();
    assert!(note.contains(words::word("reads.large_title")), "{note}");
    assert!(note.contains("research/papers/to-err-is-agentic/evidence/ (13,708 files)"), "{note}");
    assert!(note.contains(words::word("reads.large_fix")), "{note}");
    assert!(bar.find_layer("btn Eval anyway").is_some(), "its yes");
    // Its folder and the file to mark are set in mono, as paths are.
    let large = bar.find_layer("reads large").unwrap();
    let mono: Vec<String> = large
        .children
        .iter()
        .filter_map(|c| {
            let t = c.text.as_ref()?;
            let (r, _) = c.runs.iter().find(|(_, run)| run.mono)?;
            Some(t[r.clone()].to_string())
        })
        .collect();
    assert_eq!(mono, ["research/papers/to-err-is-agentic/evidence/", ".gitattributes"]);
}
