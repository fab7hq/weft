//! What the screen shows, read the way a person reads it.

use super::*;
use crate::app::tests::{app, press, unit};
use crate::ledger::{Check, Sent, Verdict};
use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// The whole screen against the one it should be, kept in
/// `snapshots/`. A project is named after its scratch directory, which
/// differs on every run, so it reads as `[project]`.
fn expect_screen(name: &str, drawn: &str) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"weft-app-[a-z]+-\d+-\d+ *", "[project] ");
    settings.bind(|| insta::assert_snapshot!(name, drawn));
}

/// What the frame actually drew, one line per row.
fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every column is measured in cells, so a row carrying `⚡` ends where
/// every other row does and its badge stays on the right.
#[test]
fn a_turbo_rows_badge_is_right_aligned() {
    let mut a = app();
    crate::app::tests::agent_says(&mut a, "codex", "fixture", "waiting");
    let row = a.rows().iter().find(|r| matches!(r, crate::app::Row::Harness { .. })).cloned();
    let row = row.expect("the harness row");
    let plain = sidebar_row(&a, &row, false, 40).width();
    a.session_mut().panes[0].turbo = true;
    let turbo = sidebar_row(&a, &row, false, 40);
    assert!(turbo.spans.iter().any(|s| s.content.contains('⚡')), "{turbo:?}");
    assert_eq!(turbo.width(), plain, "the badge ends where it did");
}

#[test]
fn a_follow_up_says_what_it_follows() {
    let mut a = app();
    a.set_units(vec![unit(Sent::TakenByAgent)]);
    press(&mut a, KeyCode::Char('f'));
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("FOLLOW UP  health endpoint · ask_1"), "{drawn}");
    press(&mut a, KeyCode::Esc);
    press(&mut a, KeyCode::Char('a'));
    assert!(!screen(&mut a, 120, 32).contains("FOLLOW UP"), "a plain Ask follows nothing");
}

/// An Ask that has been judged, with the record the judges wrote on disk.
fn judged() -> App {
    judged_with(false)
}

/// `judged`, with an Eval of both Asks already gathered on the record, for a
/// test that presses `[E]VAL`.
fn judged_with(gathered: bool) -> App {
    let mut a = app();
    let eval_id = "evl_1";
    let dir = a.root().join(".fab7/rf/evals").join(eval_id);
    std::fs::create_dir_all(&dir).expect("evals dir");
    std::fs::write(
        dir.join("record.json"),
        serde_json::json!({
            "eval_id": eval_id,
            "verdict": "drifted",
            "confidence": 0.67,
            "judgements": [
                {"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5.6-luna"}},
                {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5.6-luna"}},
                {"judge": {"angle": "adversary", "host": "codex", "model": "gpt-5.6-luna"}}
            ],
            "items": [
                {"id": "i1", "status": "active", "text": "returns the real build number",
                 "majority": "no", "agreement": 1.0,
                 "votes": [{"angle": "drift", "vote": "no",
                            "reason": "it still reads a literal \"dev\" at line 41"}]},
                {"id": "i2", "status": "active", "text": "the endpoint responds at /health",
                 "majority": "yes", "agreement": 1.0, "votes": []}
            ],
            "drift": {"commission": [{"path": "README.md"}]}
        })
        .to_string(),
    )
    .expect("record");

    let mut u = unit(Sent::Arrived { exact: true });
    u.check = Some(Check {
        eval_id: eval_id.into(),
        verdict: Verdict::DoesntMatch,
        agreement: 0.67,
        judged_by: Some("codex".into()),
    });
    let mut ready = unit(Sent::ReadyToSend);
    ready.ask_id = "ask_2".into();
    ready.title = "readme fix".into();
    // The daemon is the authority on what work exists, so an act can only
    // reach a unit that is really on the ledger. These two go there first,
    // with the same ids; the richer board below is what is drawn.
    crate::app::tests::record_units(&a, &["ask_1", "ask_2"]);
    if gathered {
        crate::app::tests::gather_on_record(&a, &["ask_1", "ask_2"]);
    }
    a.refresh_for_test();
    a.set_units(vec![u, ready]);
    // The daemon reads a record for a unit it knows; this board is
    // fabricated, so the record is handed over the same way it would be.
    let text =
        std::fs::read_to_string(a.root().join(".fab7/rf/evals/evl_1/record.json")).expect("record");
    let record: serde_json::Value = serde_json::from_str(&text).expect("json");
    let parsed = weft_core::record::Record::parse(&record).expect("a record");
    a.set_records(serde_json::json!({"evl_1": parsed}));
    a
}

#[test]
fn the_title_bar_says_what_is_open_and_never_which_directory() {
    let mut a = judged();
    for width in [60, 80, 120, 200] {
        let drawn = screen(&mut a, width, 24);
        let title = drawn.lines().next().expect("a title bar");
        assert!(title.contains("WEFT"), "{title}");
        assert!(!title.contains("in Weft"), "where you are is lit, not named: {title}");
        // With more than one project a name is wrong the moment focus
        // moves, and the sidebar names them where they can be acted on.
        let name = a.project_name().to_string();
        assert!(!title.contains(&name), "the title named a directory at {width}: {title}");
    }
    let drawn = screen(&mut a, 80, 24);
    let title = drawn.lines().next().expect("a title bar");
    assert!(title.contains("OPEN  1"), "one agent is open: {title}");
    // NEEDS YOU is an agent asking for input: here, none is.
    assert!(!title.contains("NEEDS YOU"), "{title}");
    crate::app::tests::agent_says(&mut a, "codex", "fixture", "waiting");
    let drawn = screen(&mut a, 80, 24);
    let title = drawn.lines().next().expect("a title bar");
    assert!(title.contains("NEEDS YOU  1"), "{title}");
}

#[test]
fn six_units_in_two_projects_fit_on_one_screen() {
    let mut a = judged();
    let (other, _) = crate::app::tests::test_session("second");
    a.open_project(&other.to_string_lossy());
    a.add("codex", "/bin/cat").expect("an agent in the second project");
    a.settle();
    let mut more: Vec<crate::ledger::Unit> = Vec::new();
    for i in 0..4 {
        let mut u = crate::app::tests::unit(crate::ledger::Sent::ReadyToSend);
        u.ask_id = format!("ask_{i}");
        u.title = format!("unit {i}");
        more.push(u);
    }
    a.set_units(more);
    // Two project rows, two harness rows, six units: ten lines in a body
    // with room for twenty, and none of them elided. The old row was four
    // to five lines per unit, which did not fit at two units.
    assert_eq!(a.rows().len(), 10, "{:?}", a.rows());
    expect_screen("six_units_in_two_projects", &screen(&mut a, 80, 24));
}

#[test]
fn agents_are_tabs_over_the_pane_numbered_and_marked() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    let tabs = drawn.lines().nth(1).expect("a tab row");
    assert!(!tabs.contains("WORK"), "the row is the tabs, nothing else: {tabs}");
    assert!(tabs.contains("▸ 1 codex"), "the active agent is marked: {tabs}");
    assert!(tabs.trim_end().ends_with('+'), "a new agent is one click away: {tabs}");
}

#[test]
fn a_row_is_one_line_saying_what_and_where_it_stands() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    let row = drawn.lines().find(|l| l.contains("health endpoint")).expect("the row");
    // Whose it is is the level above, not repeated on every child.
    assert!(drawn.lines().any(|l| l.contains("▾ codex")), "{drawn}");
    // The furthest act, named as the bar names it, and never an alarm.
    // What the judges said is a section of the detail view, not four
    // more lines here.
    assert!(row.contains("EVALED") && !row.contains('●'), "{row}");
    assert!(!drawn.contains("DOESN'T MATCH"), "the verdict is not on the row:\n{drawn}");
}

#[test]
fn an_ask_row_carries_no_dot_even_when_its_prompt_is_ready() {
    // The dot is an agent asking for input, on its harness's row.
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    assert!(drawn.contains("ASKED") && !drawn.contains("● ASKED"), "{drawn}");
}

#[test]
fn every_verdict_names_the_host_that_produced_it() {
    let mut a = judged();
    press(&mut a, KeyCode::Enter);
    let read = a.detail().expect("the view").lines.join("\n");
    assert!(read.contains("judged by codex"), "{read}");
    // Agreement, never a score.
    assert!(read.contains("agreed"), "{read}");
    assert!(!read.contains("confidence"), "{read}");
}

#[test]
fn nothing_on_the_board_claims_the_work_is_done_or_that_a_prompt_is_better() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24).to_lowercase();
    for forbidden in ["done", "better", "improved", "quality score", "correct"] {
        assert!(!drawn.contains(forbidden), "{forbidden} must not appear: {drawn}");
    }
}

#[test]
fn the_action_bar_shows_a_key_for_everything_it_offers() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    for key in ["[A]SK", "[E]VAL", "[S]EAL", "[W]EFT"] {
        assert!(drawn.contains(key), "{key} missing from the bar: {drawn}");
    }
    // The verb and the reading are not on it: proceeding happens in the
    // detail view, and `[Enter]` is how that view is reached.
    assert!(!drawn.contains("[P]ROCEED"), "{drawn}");
    assert!(!drawn.contains("[D]ETAIL"), "{drawn}");
}

#[test]
fn the_bar_keeps_its_shape_whether_or_not_an_action_can_be_used() {
    // Unavailable actions are drawn muted and stay put, so the bar never
    // jumps under the pointer.
    let mut nothing = app();
    let mut something = judged();
    let bar = |a: &mut App| screen(a, 80, 24).lines().nth(22).map(str::to_string).expect("a bar");
    assert_eq!(bar(&mut nothing), bar(&mut something));
}

#[test]
fn in_the_agent_weft_offers_no_keys_at_all() {
    // One key is Weft's, on the bar, and said nowhere else; the counts
    // keep their place.
    let mut a = judged();
    a.focus = Focus::Agent;
    expect_screen("in_the_agent", &screen(&mut a, 80, 24));
}

#[test]
fn hiding_the_work_list_gives_the_agent_the_whole_width() {
    // The list is away, nothing is left behind, and the way back is said.
    let mut a = judged();
    press(&mut a, KeyCode::Char('b'));
    expect_screen("the_list_hidden", &screen(&mut a, 120, 32));
}

#[test]
fn the_detail_view_blocks_and_holds_all_three_acts() {
    // One unit and one decision. Nothing behind it is reachable, which is
    // what keeps the bar's acts and `[P]ROCEED` from both being live.
    // Judged, it has nothing to send: [F] FOLLOW UP is the one way on.
    let mut a = judged();
    press(&mut a, KeyCode::Enter);
    let lines = &a.detail().expect("the view").lines;
    for section in ["ASK", "EVAL", "SEAL"] {
        assert!(lines.iter().any(|l| l.starts_with(section)), "{section} is missing from the view");
    }
    expect_screen("the_detail_view", &screen(&mut a, 120, 32));
}

#[test]
fn a_reading_too_wide_for_the_view_folds_rather_than_being_cut() {
    let mut a = judged();
    press(&mut a, KeyCode::Enter);
    // Narrow enough that the longest reason cannot sit on one line. This
    // is where the exact wording is read, so nothing may be lost off the
    // right-hand edge.
    let drawn = screen(&mut a, 46, 70);
    // The longest line in the fixture is 56 characters in a 45-column
    // view. Every word of it is on the screen, across two lines.
    let flat: String = drawn.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    assert!(
        flat.contains("CHANGED WITH NO JUDGE ABLE TO TIE IT TO WHAT YOU ASKED"),
        "the line lost something in the fold:\n{drawn}"
    );
    for line in drawn.lines() {
        assert!(
            !line.trim_end().ends_with('…') || line.contains("more lines"),
            "a reading was cut instead of folded:\n{line}"
        );
    }
}

#[test]
fn scrolling_past_the_last_line_stops_there_instead_of_emptying_the_panel() {
    let mut a = judged();
    press(&mut a, KeyCode::Enter);
    // A short panel, so there is somewhere to scroll to.
    let _ = screen(&mut a, 120, 14);
    for _ in 0..200 {
        press(&mut a, KeyCode::Down);
    }
    let drawn = screen(&mut a, 120, 14);
    assert!(
        drawn.contains("A check is a judgement"),
        "the end of the reading stays in view:\n{drawn}"
    );
}

#[test]
fn the_confirmation_is_anchored_to_the_bottom_with_the_row_still_in_view() {
    let mut a = judged_with(true);
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Char('e'));
    let drawn = screen(&mut a, 80, 24);
    let lines: Vec<&str> = drawn.lines().collect();
    let row = lines
        .iter()
        .position(|l| l.contains("readme fix"))
        .unwrap_or_else(|| panic!("the row:\n{drawn}"));
    let panel = lines
        .iter()
        .position(|l| l.contains("EVAL THIS WORK"))
        .unwrap_or_else(|| panic!("the panel:\n{drawn}"));
    assert!(row < panel, "the row it is about stays above it:\n{drawn}");
    assert!(!lines[22].contains("[Enter]"), "{}", lines[22]);
    assert!(lines[22].contains("[←] CANCEL"), "{}", lines[22]);
    assert!(lines[23].contains("never types into an agent without asking"), "{}", lines[23]);
}

#[test]
fn side_by_side_puts_the_list_left_and_never_squeezes_the_pane() {
    let mut a = judged();
    expect_screen("side_by_side", &screen(&mut a, 120, 32));
    let Layout::Split { list, pane } = layout::for_width(120) else { panic!() };
    assert_eq!((list, pane), (39, 80), "the spec's own screen 4");
}

#[test]
fn every_cell_of_the_logo_has_a_colour_and_every_colour_a_cell() {
    for (text, mask) in LOGO {
        assert_eq!(text.chars().count(), mask.chars().count(), "{text}");
        for (c, k) in text.chars().zip(mask.chars()) {
            assert_eq!(c == ' ', k == ' ', "{text}");
        }
    }
}

#[test]
fn a_list_longer_than_the_screen_says_what_is_above_and_below() {
    let mut a = app();
    let many: Vec<_> = (0..20)
        .map(|i| {
            let mut u = unit(Sent::TakenByAgent);
            u.ask_id = format!("ask_{i}");
            u.title = format!("unit {i}");
            u
        })
        .collect();
    a.set_units(many);
    expect_screen("a_long_list", &screen(&mut a, 80, 24));
    // Down to the last row, which the arrows have to scroll to now that
    // there is no wheel.
    while a.selected + 1 < a.rows().len() {
        press(&mut a, KeyCode::Down);
    }
    expect_screen("a_long_list_scrolled", &screen(&mut a, 80, 24));
}

#[test]
fn space_goes_to_an_agent_asking_and_never_to_an_ask() {
    let mut a = judged();
    press(&mut a, KeyCode::Char(' '));
    assert!(a.detail().is_none(), "an Ask is not NEEDS YOU");
    assert_eq!(a.hint_text(), Some("Nothing needs your input."));
}

#[test]
fn a_harness_that_is_not_set_up_is_marked_and_says_so() {
    let mut a = judged();
    a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin));
    let drawn = screen(&mut a, 80, 24);
    let tabs = drawn.lines().nth(1).expect("a tab row");
    assert!(tabs.contains("codex ⚠"), "the tab carries it: {tabs}");
    assert!(drawn.contains("codex is not set up for RingFrame"), "{drawn}");
    assert!(drawn.contains("[U]PDATE sets it up"), "{drawn}");
}

#[test]
fn the_bar_keeps_every_entry_when_a_harness_is_not_set_up() {
    // Weft's own operations moved into the [W] menu, so the bar fits at
    // 80 columns with room to spare. Every entry still has to be on
    // screen and readable.
    let mut a = judged();
    a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin));
    let drawn = screen(&mut a, 80, 24);
    for key in ["[A]SK", "[E]VAL", "[S]EAL", "[W]EFT"] {
        assert!(drawn.contains(key), "{key} fell off the bar: {drawn}");
    }
}

#[test]
fn help_says_what_this_project_routes() {
    let mut a = judged();
    let text = "[routing]\neval = \"claude-code\"\nseal = \"codex\"\n";
    a.set_routing(
        weft_core::config::read(text, &weft_core::harness::fixture::harnesses()).routing(a.root()),
    );
    press(&mut a, KeyCode::Char('h'));
    // Tall enough for the whole of help; at 24 rows it scrolls to this.
    let drawn = screen(&mut a, 80, 30);
    assert!(drawn.contains("This project routes"), "{drawn}");
    assert!(drawn.contains("eval → claude-code"), "{drawn}");
    assert!(drawn.contains("seal → codex"), "{drawn}");
}

#[test]
fn help_says_nothing_about_routing_when_there_is_none() {
    let mut a = judged();
    press(&mut a, KeyCode::Char('h'));
    let drawn = screen(&mut a, 80, 24);
    assert!(!drawn.contains("This project routes"), "{drawn}");
}

#[test]
fn the_ask_box_shows_what_was_typed_spaces_and_all() {
    // Typed spaces vanished on screen and came back in the confirmation,
    // which made the confirmation look like it had changed the wording.
    let mut a = judged();
    press(&mut a, KeyCode::Char('a'));
    for c in "fix    the     build".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("fix    the     build"), "{drawn}");
}

#[test]
fn folding_a_line_keeps_every_character() {
    // Both the ask box and the confirmation fold rather than wrap: one
    // shows what was typed, the other promises the exact wording.
    for text in [
        "a  b",
        "one two three four five",
        "   leading",
        "trailing   ",
        "/plan Fix   the   thing",
        "averylongwordwithnospacesatall",
    ] {
        let folded = fold(text, 8).join("");
        assert_eq!(folded, text.replace('\n', ""), "{text:?} came back as {folded:?}");
    }
}

#[test]
fn what_can_be_used_is_lit_and_what_cannot_is_grey() {
    let a = judged();
    let available = key(&a, "[A]SK", Act::Ask);
    // Judged, so there is nothing to send.
    let unavailable = key(&a, "[P]ROCEED", Act::Proceed);
    assert_eq!(available.style.fg, Some(a.theme.primary()), "active reads as text");
    assert_eq!(unavailable.style.fg, Some(a.theme.muted()), "inactive reads as grey");
}

#[test]
fn the_ask_box_cancels_on_the_key_it_shows() {
    let mut a = judged();
    press(&mut a, KeyCode::Char('a'));
    for c in "half an intent".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("[←] CANCEL"), "{drawn}");
    press(&mut a, KeyCode::Left);
    assert!(a.modal.is_none(), "[←] cancels whatever has been typed");
}

#[test]
fn a_pane_waiting_for_an_answer_offers_to_take_you_there_and_nothing_else() {
    let mut a = judged();
    a.session_mut().set_turns(vec![Some(weft_core::turns::Turn::Waiting)]);
    let tabs = screen(&mut a, 80, 24);
    assert!(
        tabs.lines().nth(1).is_some_and(|l| l.contains('●')),
        "the tab carries the dot: {tabs}"
    );

    press(&mut a, KeyCode::Char(' '));
    let drawn = screen(&mut a, 80, 24);
    assert!(!drawn.contains("[Enter]"), "Enter is the default, said once in [H]ELP: {drawn}");
    assert!(drawn.contains("[Y] HOW WEFT KNOWS"), "{drawn}");
    assert!(drawn.contains("is asking you something"), "{drawn}");
    assert!(drawn.contains("Weft never answers for you"), "{drawn}");
}

#[test]
fn an_unavailable_action_puts_one_sentence_in_the_hint_and_no_dialog() {
    let mut a = app();
    press(&mut a, KeyCode::Char('p'));
    let drawn = screen(&mut a, 80, 24);
    assert!(drawn.contains("Nothing has been asked for yet."), "{drawn}");
    assert!(!drawn.contains("┌"), "no box was opened:\n{drawn}");
}

/// Work judged by an Eval of today's kind (`/2`): RingFrame's files for it on
/// disk, as the daemon reads them for the Eval view.
fn judged_v2(closed: bool) -> App {
    let mut a = app();
    let eval_id = "evl_2";
    let rf = a.root().join(".fab7/rf");
    let dir = rf.join("evals").join(eval_id);
    std::fs::create_dir_all(&dir).expect("evals dir");
    let s7 = "../plans/app/plan.md#Phase 1/7";
    let s8 = "../plans/app/plan.md#Phase 1/8";
    let write = |name: &str, v: serde_json::Value| {
        std::fs::write(dir.join(name), v.to_string()).expect(name);
    };
    write(
        "changes.json",
        serde_json::json!({"requirements": [
        {"id": s7, "kind": "local", "part": "../plans/app/plan.md#Phase 1", "ask": "ask_1",
         "text": "**A failed send is told.** The person sees why.", "done_when": "A test with a refusing CLI."},
        {"id": s8, "kind": "local", "part": "../plans/app/plan.md#Phase 1", "ask": "ask_1",
         "text": "Keep the log short.", "done_when": "The log is one line."}],
        "findings": [{"kind": "unclaimed_rewrite", "requirements": [s7],
                      "detail": "commit 94d7989 (\"Follow-up work\") rewrites lines of 758156f"}]}),
    );
    write(
        "evidence.json",
        serde_json::json!({"windows": [
        {"id": "w_b", "path": "src/server.rs", "class": "code"},
        {"id": "w_a", "path": "src/log.rs", "class": "code"},
        {"id": "w_t", "path": "src/telemetry.rs", "class": "code"}]}),
    );
    write(
        "brief.json",
        serde_json::json!({"asks": [{"ask_id": "ask_1", "title": "health endpoint"}]}),
    );
    write(
        "windows.json",
        serde_json::json!({"windows": {
        "w_b": "--- src/server.rs\n@@ -1 +1,2 @@\n+let mut unrecorded = None;\n+let _ = submit(ask);\n",
        "w_a": "--- src/log.rs\n@@ -1 +1 @@\n+log(one_line);\n",
        "w_t": "--- src/telemetry.rs\n@@ -0,0 +1 @@\n+post(\"https://collect.example.com\");\n"}}),
    );
    if closed {
        write(
            "record.json",
            serde_json::json!({
            "schema": "ringframe.eval-record/2", "eval_id": eval_id, "verdict": "drifted", "confidence": 0.75,
            "drift": {"commission": 0.02, "omission": 0.5}, "judges": [],
            "items": [
                {"id": s7, "result": "not_met", "votes": [
                    {"role": "reduce", "vote": "met", "counted": true, "reason": "the person is told",
                     "citations": [{"window": "w_b", "quote": "let mut unrecorded = None;"}]},
                    {"role": "confirm", "vote": "not_met", "counted": true, "reason": "the follow-up drops the refusal again",
                     "missing": "unrecorded is never set", "citations": []}]},
                {"id": s8, "result": "met", "votes": [
                    {"role": "reduce", "vote": "met", "counted": true, "reason": "one line",
                     "citations": [{"window": "w_a", "quote": "log(one_line);"}]}]}],
            "windows": [
                {"id": "w_b", "path": "src/server.rs", "result": "required", "readings": {"map": {"serves": [s7]}}},
                {"id": "w_a", "path": "src/log.rs", "result": "required", "readings": {"map": {"serves": [s8]}}},
                {"id": "w_t", "path": "src/telemetry.rs", "result": "unexplained",
                 "readings": {"map": {"unexplained": "posts the project path outside", "quote": "collect.example.com"}}}],
            "findings": [{"kind": "unclaimed_rewrite", "requirements": [s7],
                          "detail": "commit 94d7989 (\"Follow-up work\") rewrites lines of 758156f"}],
            "checks": [{"command": "cargo test", "outcome": "succeeded", "seconds": 12}],
            "limitations": []}),
        );
    } else {
        // Running: the map is in, one reduce is out.
        let tasks = rf.join(format!("tmp/eval-{eval_id}/tasks"));
        std::fs::create_dir_all(&tasks).expect("tasks");
        for t in ["m1", "r1", "r2"] {
            std::fs::write(tasks.join(format!("{eval_id}~{t}.json")), "{}").expect("task");
        }
        std::fs::create_dir_all(dir.join("tasks")).expect("outputs");
        let map = serde_json::json!({"kind": "map", "judge": {"role": "map"}, "windows": [
            {"window": "w_b", "serves": [s7]}, {"window": "w_a", "serves": [s8]},
            {"window": "w_t", "unexplained": "posts the project path outside", "quote": "collect.example.com"}]});
        let reduce = serde_json::json!({"kind": "reduce", "judge": {"role": "reduce"}, "requirements": [
            {"id": s7, "vote": "met", "reason": "the person is told", "citations": []}]});
        let mut ledger = String::new();
        for (t, out) in [("m1", map), ("r1", reduce)] {
            let path = format!("evals/{eval_id}/tasks/{eval_id}.{t}.1.json");
            std::fs::write(rf.join(&path), out.to_string()).expect("output");
            ledger.push_str(
                &serde_json::json!({"type": "eval.task", "data": {
                "eval_id": eval_id, "task_id": format!("{eval_id}~{t}"), "outcome": "accepted",
                "artifact": {"path": path}}})
                .to_string(),
            );
            ledger.push('\n');
        }
        std::fs::write(rf.join("eval-tasks.fixture"), ledger).expect("tasks");
    }
    let mut u = unit(Sent::Arrived { exact: true });
    if closed {
        u.check = Some(Check {
            eval_id: eval_id.into(),
            verdict: Verdict::DoesntMatch,
            agreement: 0.75,
            judged_by: None,
        });
    } else {
        u.gathered =
            Some(crate::ledger::Gathered { eval_id: eval_id.into(), by: "fixture".into() });
    }
    crate::app::tests::record_units(&a, &["ask_1"]);
    // The Eval's task events go on the ledger after the Asks it judges.
    if let Ok(tasks) = std::fs::read_to_string(rf.join("eval-tasks.fixture")) {
        let mut ledger = std::fs::read_to_string(rf.join("ledger.jsonl")).unwrap_or_default();
        ledger.push_str(&tasks);
        std::fs::write(rf.join("ledger.jsonl"), ledger).expect("ledger");
    }
    a.refresh_for_test();
    a.set_units(vec![u]);
    a
}

#[test]
fn the_eval_view_reads_requirements_first_then_a_step_its_change_and_by_file() {
    let mut a = judged_v2(true);
    press(&mut a, KeyCode::Char('v'));
    assert!(a.eval_screen().is_some(), "[V] opens the Eval view");
    expect_screen("the_eval_view", &screen(&mut a, 120, 32));
    // The first pickable row is the unexplained change; the steps follow.
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Enter);
    expect_screen("the_eval_view_step", &screen(&mut a, 120, 32));
    press(&mut a, KeyCode::Down);
    press(&mut a, KeyCode::Char('d'));
    let drawn = screen(&mut a, 120, 32);
    assert!(
        drawn.contains("CHANGE src/server.rs  w_b")
            && drawn.contains("+let mut unrecorded = None;"),
        "{drawn}"
    );
    press(&mut a, KeyCode::Esc);
    press(&mut a, KeyCode::Esc);
    press(&mut a, KeyCode::Tab);
    expect_screen("the_eval_view_by_file", &screen(&mut a, 120, 32));
    press(&mut a, KeyCode::Esc);
    assert!(a.eval_screen().is_none(), "Esc from the list closes it");
}

#[test]
fn a_running_eval_shows_what_is_in_as_so_far() {
    let mut a = judged_v2(false);
    press(&mut a, KeyCode::Char('v'));
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("judging · map 1 of 1 · reduce 1 of 2 · confirm —"), "{drawn}");
    assert!(drawn.contains("met, so far"), "{drawn}");
    assert!(drawn.contains("judging · 1 change"), "{drawn}");
}

#[test]
fn the_ringframe_view_lists_every_harness_by_name_with_what_it_needs() {
    use weft_core::onboarding::{Row, State, View};
    let key = |a: &mut App, code: KeyCode| {
        a.on_key(crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE))
            .expect("key")
    };
    let mut a = app();
    // The daemon's own look first, so it cannot land on top of the fixture.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while a.setup_view().is_none() && std::time::Instant::now() < deadline {
        a.pump();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let row = |name: &str, title: &str, state: State| Row {
        name: name.into(),
        title: title.into(),
        program: name.into(),
        state,
    };
    a.session_mut().setup = Some(View {
        latest: Some("v0.1.3".into()),
        plugin: Some("0.1.3".into()),
        configuration: "up to date".into(),
        configuration_behind: false,
        rows: vec![
            row("antigravity", "Antigravity", State::Failed {
                step: "agy plugin install https://github.com/fab7hq/fab7/tree/main/products/ringframe/plugins/antigravity".into(),
                said: "Error: not signed in".into(),
            }),
            row("claude-code", "Claude Code", State::Behind { have: "0.1.2".into(), latest: "0.1.3".into() }),
            row("codex", "Codex", State::NotFound),
            row("zed", "Zed Agent", State::Ready { version: Some("0.1.3".into()) }),
        ],
        note: None,
    });
    a.modal = Some(crate::app::Modal::RingFrame);
    a.modal_choice = 0;
    let drawn = screen(&mut a, 120, 32);
    expect_screen("the_ringframe_view", &drawn);
    assert!(drawn.contains("Error: not signed in"), "a failed step says what it printed");
    let bar = |a: &mut App| {
        let drawn = screen(a, 120, 32);
        drawn.lines().rev().find(|l| l.contains("[ESC] CLOSE")).expect("the bar").to_string()
    };
    // `Enter` does the picked row, as everywhere, and the bar does not say so;
    // `[P]ROCEED ALL` is there while something is behind.
    assert!(
        !bar(&mut a).contains("[Enter]") && bar(&mut a).contains("[P]ROCEED ALL"),
        "{}",
        bar(&mut a)
    );
    // `Enter` on a harness that is not found asks where it is.
    a.modal_choice = 2;
    key(&mut a, KeyCode::Enter);
    assert_eq!(
        a.modal,
        Some(crate::app::Modal::Locate { harness: "codex".into(), text: String::new() })
    );
    for c in "/opt/codex".chars() {
        key(&mut a, KeyCode::Char(c));
    }
    expect_screen("the_ringframe_view_where_is_it", &screen(&mut a, 120, 32));
    key(&mut a, KeyCode::Esc);
    assert_eq!(a.modal, Some(crate::app::Modal::RingFrame), "back to the view");
}

#[test]
fn help_says_what_enter_does_once_and_update_sits_above_help() {
    let a = app();
    let help = help_lines(&a).join("\n");
    assert!(help.contains("open/proceed"), "{help}");
    assert!(help.contains("switch active pane"), "{help}");
    assert!(help.contains("T  turbo mode"), "{help}");
    let at = |key: &str| {
        help.lines().position(|l| l.contains(key)).unwrap_or_else(|| panic!("{key} in {help}"))
    };
    assert_eq!(at("U  update") + 1, at("H  help"), "update sits right above help:\n{help}");
    // The Weft menu: the same order, and no turbo, whose switch is on the title bar.
    let menu: Vec<char> = crate::app::WEFT_MENU.iter().map(|(k, ..)| *k).collect();
    assert_eq!(menu, ['O', 'N', 'B', 'U', 'H', 'X']);
    assert_eq!(crate::app::WEFT_MENU[3].1, "Update");
}
