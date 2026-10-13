//! What the screen shows, read the way a person reads it.

use super::*;
use crate::app::tests::{app, ctrl, press, unit};
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
    // A live daemon's sessions carry the time the test ran; the Atlas's
    // samples hold their clock still and keep theirs.
    if !name.starts_with("atlas_") {
        settings.add_filter(r"(ready|working|turn ended|needs you|ended) \d\d:\d\d", "$1 --:--");
    }
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

#[test]
fn a_follow_up_says_what_it_follows() {
    let mut a = app();
    a.set_units(vec![unit(Sent::TakenByAgent)]);
    crate::app::tests::on_work(&mut a);
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
    crate::app::tests::asked(&mut a);
    a.settle();
    let mut more: Vec<crate::ledger::Unit> = Vec::new();
    for i in 0..4 {
        let mut u = crate::app::tests::unit(crate::ledger::Sent::ReadyToSend);
        u.ask_id = format!("ask_{i}");
        u.title = format!("unit {i}");
        more.push(u);
    }
    a.set_units(more);
    // Two projects, six works, one line each, none elided.
    let asks = a.rows().iter().filter(|r| matches!(r, crate::app::Row::Ask { .. })).count();
    assert_eq!(asks, 6, "{:?}", a.rows());
    expect_screen("six_units_in_two_projects", &screen(&mut a, 80, 24));
}

#[test]
fn agents_are_tabs_over_the_pane_numbered_and_marked() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    let tabs = drawn.lines().nth(1).expect("a tab row");
    assert!(!tabs.contains("WORK"), "the row is the tabs, nothing else: {tabs}");
    assert!(
        tabs.contains("▸ 1 weft-app-co… · codex"),
        "the active agent is marked, its project shortened: {tabs}"
    );
    assert!(tabs.trim_end().ends_with('+'), "a new agent is one click away: {tabs}");
}

#[test]
fn a_row_is_one_line_saying_what_and_where_it_stands() {
    let mut a = judged();
    let drawn = screen(&mut a, 80, 24);
    let row = drawn.lines().find(|l| l.contains("health endpoint")).expect("the row");
    // Its harness is on the row; under an Eval its round says its
    // state, and the verdict is the round's, said once.
    assert!(row.contains("codex") && !row.contains("EVALED") && !row.contains('●'), "{row}");
    assert!(drawn.contains("Eval 1 · drifted · 0.67"), "{drawn}");
    assert!(!drawn.contains("DOESN'T MATCH"), "{drawn}");
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
    assert!(a.select(|r| matches!(r, crate::app::Row::Eval { .. })));
    press(&mut a, KeyCode::Enter);
    let read = a.detail().expect("the view").lines.join("\n");
    assert!(read.contains("JUDGED BY   codex · gpt-5.6-luna"), "{read}");
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

/// A work's Details block the screen and hold its Ask alone: no Eval
/// or Seal section, however far it has gone.
#[test]
fn a_works_details_block_and_hold_its_ask_alone() {
    let mut a = judged();
    let health =
        |r: &crate::app::Row| matches!(r, crate::app::Row::Ask { ask, .. } if ask.unit == "ask_1");
    assert!(a.select(health));
    press(&mut a, KeyCode::Enter);
    let d = a.detail().expect("the view");
    assert_eq!(d.title, "health endpoint  ›  Eval 1 drifted  ›  not sealed");
    assert!(d.lines.iter().any(|l| l.starts_with("ASK [")));
    assert!(!d.lines.iter().any(|l| l.starts_with("EVAL") || l.starts_with("SEAL")));
    let drawn = screen(&mut a, 120, 32);
    assert!(!drawn.lines().nth(5).unwrap_or("").contains('│'), "nothing beside it:\n{drawn}");
}

#[test]
fn a_reading_too_wide_for_the_view_folds_rather_than_being_cut() {
    let mut a = judged();
    let said = "CHANGED WITH NO JUDGE ABLE TO TIE IT TO WHAT YOU ASKED";
    a.open_view_read(crate::app::View::Work("ask_1".into()), Some(said), None, None);
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
    assert!(a.select(|r| matches!(r, crate::app::Row::Eval { .. })));
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
    assert!(lines[22].contains("[←] CANCEL   [Enter] SEND"), "the bar: {}", lines[22]);
    assert!(lines[23].contains("never types into an agent without asking"), "{}", lines[23]);
}

#[test]
fn side_by_side_puts_the_list_left_and_never_squeezes_the_pane() {
    let mut a = judged();
    expect_screen("side_by_side", &screen(&mut a, 120, 32));
    let Layout::Split { list, pane } = layout::for_width(120) else { panic!() };
    assert_eq!((list, pane), (44, 75), "as the Atlas draws it");
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
    crate::app::tests::on_work(&mut a);
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
    expect_screen("the_eval_view_change", &drawn);
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
    crate::app::tests::on_work(&mut a);
    press(&mut a, KeyCode::Char('v'));
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("judging · map 1 of 1 · reduce 1 of 2 · confirm —"), "{drawn}");
    assert!(drawn.contains("met, so far"), "{drawn}");
    assert!(drawn.contains("judging · 1 change"), "{drawn}");
    expect_screen("the_eval_view_judging", &drawn);
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
        weft: "0.1.2 → 0.1.3".into(),
        weft_latest: Some("0.1.3".into()),
        weft_behind: true,
        weft_waiting: false,
        latest: Some("v0.1.3".into()),
        plugin: Some("0.1.3".into()),
        configuration: "0.1.3 is the latest".into(),
        configuration_behind: false,
        pending_release: false,
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
    assert!(help.contains("into · out of the agent"), "{help}");
    assert!(help.contains("T  turbo mode"), "{help}");
    let at = |key: &str| {
        help.lines().position(|l| l.contains(key)).unwrap_or_else(|| panic!("{key} in {help}"))
    };
    assert_eq!(at("U  update") + 1, at("H  help"), "update sits right above help:\n{help}");
    // The Weft menu: the same order, and no turbo, whose switch is on the title bar.
    let menu: Vec<char> = crate::app::WEFT_MENU.iter().map(|(k, ..)| *k).collect();
    assert_eq!(menu, ['O', 'N', 'B', 'R', 'L', 'U', 'H', 'X']);
    assert_eq!(crate::app::WEFT_MENU[3].1, "Remote");
    assert_eq!(crate::app::WEFT_MENU[4].1, "Start at login");
    assert_eq!(crate::app::WEFT_MENU[5].1, "Update");
}

/// remote.md §12: `W` → `R` shows the remote view `weft remote` publishes,
/// in each of its states.
#[test]
fn the_remote_view_in_each_state() {
    let mut a = app();
    // With Start at login on, the view is T8.13/T8.14's (T13.3 is its off).
    a.session_mut().start_at_login = true;
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('r'));
    assert_eq!(a.modal, Some(crate::app::Modal::Remote));
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
            as i64;
    let on = |devices: serde_json::Value| {
        serde_json::json!({"state": "on", "login": "0xnairb",
            "address": "https://seating-texas-air-guns.trycloudflare.com", "tunnel": "up",
            "note": "traffic is decrypted at Cloudflare's edge", "devices": devices})
    };
    let states = [
        ("the_remote_view_off", serde_json::Value::Null),
        (
            "the_remote_view_choose",
            serde_json::json!({"state": "choose",
                "tunnels": [{"id": "cloudflared", "title": "Cloudflare Tunnel"}]}),
        ),
        (
            "the_remote_view_not_installed",
            serde_json::json!({"state": "not_installed",
                "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"},
                "offer": {"how": "brew", "argv": "brew install cloudflared"}}),
        ),
        (
            "the_remote_view_download",
            serde_json::json!({"state": "not_installed",
                "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"},
                "offer": {"how": "download", "from": "github.com/cloudflare/cloudflared"}}),
        ),
        (
            "the_remote_view_install_failed",
            serde_json::json!({"state": "not_installed",
                "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"},
                "offer": {"how": "download", "from": "github.com/cloudflare/cloudflared"},
                "failed": "cloudflared-linux-arm64 does not match GitHub's checksum; nothing installed."}),
        ),
        (
            "the_remote_view_installing",
            serde_json::json!({"state": "installing", "line": "==> Pouring cloudflared--2026.9.3.arm64_sequoia.bottle.tar.gz"}),
        ),
        (
            "the_remote_view_signing_in",
            serde_json::json!({"state": "signing_in", "code": "WXYZ-1234", "url": "github.com/login/device"}),
        ),
        ("the_remote_view_on", on(serde_json::json!([]))),
        (
            "the_remote_view_devices",
            on(serde_json::json!([
                {"id": "d_1", "name": "iPhone", "login": "0xnairb", "last_seen": now - 5 * 60_000},
                {"id": "d_2", "name": "iPad", "login": "0xnairb", "last_seen": now - 3 * 3_600_000},
            ])),
        ),
    ];
    for (name, view) in states {
        a.session_mut().remote = (!view.is_null()).then_some(view);
        expect_screen(name, &screen(&mut a, 120, 32));
    }
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("Traffic is decrypted at Cloudflare's edge."), "{drawn}");
    // "Run this?", before Weft installs anything.
    a.session_mut().remote = Some(serde_json::json!({"state": "not_installed",
        "tunnel": {"id": "cloudflared", "title": "Cloudflare Tunnel", "program": "cloudflared"},
        "offer": {"how": "brew", "argv": "brew install cloudflared"}}));
    a.remote_run = true;
    expect_screen("the_remote_view_run_this", &screen(&mut a, 120, 32));
    a.remote_run = false;
}

/// remote.md §8: the question a device asking to be allowed puts in front.
#[test]
fn the_allow_question() {
    let mut a = app();
    a.modal = Some(crate::app::Modal::Allow {
        device: "d_1".into(),
        name: "iPhone".into(),
        login: "0xnairb".into(),
    });
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("Allow \"iPhone\", signed in as 0xnairb?"), "{drawn}");
    expect_screen("the_allow_question", &drawn);
}

/// A word both clients show is spelled once, in `weft_core::words`: no string
/// in the drawing code holds one, as written or as the bar shouts it.
#[test]
fn the_shared_words_are_spelled_only_in_words() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") || path.ends_with("tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for literal in text.split('"').skip(1).step_by(2) {
            // `web.*` is the web app's own wording, not the TUI's.
            for (key, word) in
                weft_core::words::WORDS.iter().filter(|(k, _)| !k.starts_with("web."))
            {
                // A sentence is found inside a longer string, a word or two
                // only as a string of its own.
                let spelled = word.split(['{', '}']).step_by(2).any(|part| {
                    let part = part.trim();
                    let held =
                        |w: &str| if part.len() > 16 { literal.contains(w) } else { literal == w };
                    !part.is_empty() && (held(part) || held(&part.to_uppercase()))
                });
                if spelled {
                    found.push(format!("{}: {key} in \"{literal}\"", path.display()));
                }
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}

// --- the Atlas: each changed screen, set up with its card's sample ---------

/// The daemon tells new panes' (empty) states in `agents` events; take every
/// one first, or one lands after the states a test sets and clears them.
/// Each event replaces the session list, so a marker there shows one came;
/// done once none has come for 300 ms.
fn settle_agents(a: &mut App) {
    use std::time::{Duration, Instant};
    let mark =
        || vec![weft_core::sessions::Row::of("-", "-", weft_core::sessions::State::Quiet, None, 0)];
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut heard: Option<Instant> = None;
    a.session_mut().sessions = mark();
    while Instant::now() < deadline
        && heard.is_none_or(|t| t.elapsed() < Duration::from_millis(300))
    {
        a.pump();
        if a.session_mut().sessions != mark() {
            a.session_mut().sessions = mark();
            heard = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    a.session_mut().sessions = Vec::new();
}

/// The Atlas's sample, as its TUI cards show it: project weft, with claude
/// (ready) and codex (its turn ended) running; fix login of claude, ASKED;
/// health endpoint of codex, EVALED.
fn atlas() -> App {
    use weft_core::sessions::{Row, State};
    use weft_core::turns::Turn;
    let (root, session) =
        crate::app::tests::test_session_at("weft-atlas", "weft", crate::app::tests::set_up(|w| w));
    let mut a = App::with_session(root, crate::keys::Toggle, session);
    a.add("claude", "/bin/cat").expect("claude");
    a.add("codex", "/bin/cat").expect("codex");
    a.settle();
    settle_agents(&mut a);
    for h in ["claude", "codex"] {
        a.set_readiness(h, weft_core::readiness::Readiness::Ready);
    }
    let mut fix = unit(Sent::TakenByAgent);
    (fix.ask_id, fix.title, fix.harness) = ("ask_1".into(), "fix login".into(), "claude".into());
    fix.session_ref = Some("s_claude".into());
    let mut health = unit(Sent::Arrived { exact: true });
    (health.ask_id, health.session_ref) = ("ask_3".into(), Some("s_codex".into()));
    health.check = Some(Check {
        eval_id: "evl_2".into(),
        verdict: Verdict::DoesntMatch,
        agreement: 0.67,
        judged_by: Some("codex".into()),
    });
    a.set_units(vec![fix, health]);
    a.set_clock(AT_11_35, 0);
    a.session_mut().sessions = vec![
        bound(&a, 0, "s_claude", Some(Turn::Ready), 6, Some("fix login")),
        bound(&a, 1, "s_codex", Some(Turn::TurnEnded), 43, Some("health endpoint")),
    ];
    let _ = (Row::of, State::Running);
    a.session_mut().set_turns(vec![Some(Turn::Ready), Some(Turn::TurnEnded)]);
    // claude at its prompt, as the cards draw it.
    let _ = screen(&mut a, 120, 32);
    a.session_mut().panes[0].feed(" ❯\x1b[?25l".as_bytes());
    a
}

/// 2026-10-08 11:35 UTC, a Thursday: the cards' "now".
const AT_11_35: i64 = 1_791_459_300_000;

/// A session running in pane `pane`, its last change `ago` minutes before
/// 11:35, as weftd's index would hold it.
fn bound(
    a: &App,
    pane: usize,
    id: &str,
    turn: Option<weft_core::turns::Turn>,
    ago: i64,
    title: Option<&str>,
) -> weft_core::sessions::Row {
    let p = &a.session().panes[pane];
    let at = AT_11_35 - ago * 60_000;
    let mut r =
        weft_core::sessions::Row::of(&p.harness, id, weft_core::sessions::State::Running, turn, at);
    r.pane = Some(p.id);
    r.title = title.map(str::to_string);
    r.asks = usize::from(title.is_some());
    r
}

/// T1.9: a newer Weft that is out and not installed says what to do about it.
#[test]
fn atlas_t1_9_a_newer_weft_is_out() {
    let mut a = atlas();
    a.session_mut().setup = Some(weft_core::onboarding::View {
        weft: "0.2.0 → 0.2.1".into(),
        weft_latest: Some("0.2.1".into()),
        weft_behind: true,
        ..weft_core::onboarding::View::default()
    });
    expect_screen("atlas_t1_9", &screen(&mut a, 120, 32));
}

/// T13.1: an update installed while agents run waits for them, and says so
/// in place of `↑ [U]PDATE`, with how many run.
#[test]
fn atlas_t13_1_an_update_waiting() {
    let mut a = atlas();
    a.session_mut().update = Some("0.2.1".into());
    let drawn = screen(&mut a, 120, 32);
    let title = drawn.lines().next().unwrap_or_default();
    assert!(title.contains("↻ UPDATE WAITING   [T]URBO OFF"), "{title}");
    assert!(!drawn.contains("[U]PDATE"), "{drawn}");
    let hint = "Weft 0.2.1 is installed - it starts when no agent is running (2 running)";
    assert_eq!(drawn.lines().last().map(str::trim), Some(hint), "{drawn}");
    expect_screen("atlas_t13_1", &drawn);
    // It started: nothing waits, and the title is as before.
    a.session_mut().update = None;
    assert!(!screen(&mut a, 120, 32).contains("UPDATE WAITING"));
}

/// T13.2: Weft's own "Start at login" in the Weft menu, after Remote, with
/// its state; while it is picked the line under the keys says what it
/// does.
#[test]
fn atlas_t13_2_start_at_login_in_the_weft_menu() {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('w'));
    for _ in 0..4 {
        press(&mut a, KeyCode::Down);
    }
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("▸ [L]  Start at login  OFF"), "{drawn}");
    let says =
        "Start Weft when you log in, and again if it stops. Weft never turns this on by itself.";
    assert_eq!(drawn.lines().last().map(str::trim), Some(says), "{drawn}");
    expect_screen("atlas_t13_2", &drawn);
    a.session_mut().start_at_login = true;
    assert!(screen(&mut a, 120, 32).contains("▸ [L]  Start at login  ON"));
    // Another item picked: the line is the menu's own again.
    press(&mut a, KeyCode::Up);
    assert!(!screen(&mut a, 120, 32).contains(says));
}

/// T13.3: Remote on with Start at login off says, in one line, that Remote
/// stops at a restart, and offers `[L]`; with the option on neither is shown.
#[test]
fn atlas_t13_3_remote_on_with_start_at_login_off() {
    let mut a = atlas();
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
            as i64;
    a.session_mut().remote = Some(serde_json::json!({"state": "on", "login": "0xnairb",
    "address": "https://tall-river-1234.trycloudflare.com", "tunnel": "up",
    "note": "traffic is decrypted at Cloudflare's edge", "devices": [
        {"id": "d_1", "name": "iPhone", "login": "0xnairb", "last_seen": now - 10_000},
        {"id": "d_2", "name": "Browser", "login": "0xnairb", "last_seen": now - 3 * 3_600_000},
    ]}));
    press(&mut a, KeyCode::Char('w'));
    press(&mut a, KeyCode::Char('r'));
    let drawn = screen(&mut a, 120, 32);
    let line =
        "Start at login is off: Remote stops when this machine restarts, until you run weft.";
    let at = |what: &str| drawn.lines().position(|l| l.contains(what));
    assert_eq!(at(line), at("Traffic is decrypted").map(|i| i + 1), "{drawn}");
    assert!(
        drawn.contains("[↑↓] PICK   [D] REMOVE   [L] START AT LOGIN   [O] TURN OFF   [ESC] CLOSE"),
        "{drawn}"
    );
    expect_screen("atlas_t13_3", &drawn);
    a.session_mut().start_at_login = true;
    let on = screen(&mut a, 120, 32);
    assert!(!on.contains(line) && !on.contains("[L] START AT LOGIN"), "{on}");
}

/// The Atlas's sample with its second project: docs, with claude running and
/// readme fix asked of it.
fn atlas_with_docs() -> App {
    let mut a = atlas();
    let docs = a.root().with_file_name("docs");
    std::fs::create_dir_all(&docs).expect("docs");
    a.open_project(&docs.to_string_lossy());
    a.add("claude", "/bin/cat").expect("claude in docs");
    a.settle();
    let mut readme = unit(Sent::TakenByAgent);
    (readme.ask_id, readme.title, readme.harness) =
        ("ask_4".into(), "readme fix".into(), "claude".into());
    a.set_units(vec![readme]);
    // Back to weft, where the card has the keys.
    a.focus = Focus::Weft;
    // A frame first: the arrows keep the selection in the rows it drew.
    let _ = screen(&mut a, 120, 32);
    a.selected = 1;
    press(&mut a, KeyCode::Up);
    assert_eq!(a.project_name(), "weft");
    a
}

/// Every open project's running agents are tabs, named project · harness,
/// in the tree's order. A digit, Tab and a click reach an agent in another
/// project, and what is typed then goes to that agent.
#[test]
fn tabs_hold_every_open_projects_agents() {
    let mut a = atlas_with_docs();
    let tabs = |a: &mut App| screen(a, 120, 32).lines().nth(1).expect("a tab row").to_string();
    let row = tabs(&mut a);
    for want in ["1 weft · claude", "2 weft · codex", "3 docs · claude"] {
        assert!(row.contains(want), "{want}: {row}");
    }
    press(&mut a, KeyCode::Char('3'));
    assert_eq!((a.project_name(), a.focus, a.pane_selected), ("docs", Focus::Weft, true));
    assert_eq!(a.harness_at(a.pane_focus), Some("claude"));
    assert!(tabs(&mut a).contains("▸ 3 docs · claude"), "{}", tabs(&mut a));
    // Tab from the last agent wraps to the first, back in weft.
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Tab);
    assert_eq!((a.project_name(), a.harness_at(a.pane_focus)), ("weft", Some("claude")));
}

/// Every open project's agents as tabs.
#[test]
fn atlas_t14_1_every_projects_agents_as_tabs() {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('1'));
    // As the card draws it: back in the list, no pane selected.
    press(&mut a, KeyCode::Left);
    assert!(!a.pane_selected);
    expect_screen("atlas_t14_1", &screen(&mut a, 120, 32));
}

/// The layout bar's keys, pressed from the list, then closed.
fn layout(a: &mut App, keys: &[KeyCode]) {
    a.focus = Focus::Weft;
    press(a, KeyCode::Char('\\'));
    for &k in keys {
        press(a, k);
    }
    press(a, KeyCode::Esc);
}

/// The Atlas's sample with docs open, its claude moved to a pane on the
/// right; the keys in the list.
fn two_panes() -> App {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('3'));
    layout(&mut a, &[KeyCode::Right]);
    // The left pane showing its first tab, as the card draws it.
    for n in ['1', '3'] {
        a.focus = Focus::Weft;
        press(&mut a, KeyCode::Char(n));
    }
    a.focus = Focus::Weft;
    a
}

fn names(a: &App) -> Vec<String> {
    a.tabs().iter().map(|t| format!("{} · {}", t.project_name, t.harness)).collect()
}

/// The tabs count pane by pane: an agent moved to a pane of its own is
/// numbered after the first pane's, and its digit still reaches it there.
/// Tab goes on through every pane.
#[test]
fn tabs_are_numbered_pane_by_pane() {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('1'));
    let moved = (0, a.session().panes[a.pane_focus].id);
    layout(&mut a, &[KeyCode::Right]);
    assert_eq!(names(&a), ["weft · codex", "docs · claude", "weft · claude"]);
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('1'));
    assert_eq!(a.panes().active().id, a.panes().panes()[0].id);
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('3'));
    assert_eq!((a.project_name(), a.harness_at(a.pane_focus)), ("weft", Some("claude")));
    assert_eq!(a.panes().active().showing(), Some(moved));
    assert_eq!(a.panes().active().id, a.panes().panes()[1].id, "in the pane that holds it");
    // Tab from the last wraps to the first, in the first pane.
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Tab);
    assert_eq!(a.panes().active().id, a.panes().panes()[0].id);
}

/// `Tab` and `1`–`9` select an agent, in any pane, and leave Weft's keys
/// Weft's; `Enter` enters it; `Ctrl+]` steps back out to it, selected; `←`
/// goes back to the list; `Ctrl+]` from the list goes straight in.
#[test]
fn tab_and_digits_select_an_agent_and_enter_enters_it() {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('3'));
    layout(&mut a, &[KeyCode::Right]);
    a.pane_selected = false;
    press(&mut a, KeyCode::Char('1'));
    assert_eq!((a.focus, a.pane_selected), (Focus::Weft, true));
    assert_eq!((a.project_name(), a.harness_at(a.pane_focus)), ("weft", Some("claude")));
    press(&mut a, KeyCode::Tab);
    assert_eq!((a.focus, a.harness_at(a.pane_focus)), (Focus::Weft, Some("codex")));
    press(&mut a, KeyCode::Tab);
    assert_eq!(
        (a.focus, a.project_name()),
        (Focus::Weft, "docs"),
        "into the other pane, still selected"
    );
    assert_eq!(a.panes().active().id, a.panes().panes()[1].id);
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.focus, Focus::Agent);
    ctrl(&mut a, ']');
    assert_eq!((a.focus, a.pane_selected, a.project_name()), (Focus::Weft, true, "docs"));
    press(&mut a, KeyCode::Left);
    assert_eq!((a.focus, a.pane_selected), (Focus::Weft, false));
    assert!(a.show_work(), "back in the list");
    ctrl(&mut a, ']');
    assert_eq!(
        (a.focus, a.project_name()),
        (Focus::Agent, "docs"),
        "straight into the pane with the keys"
    );
}

/// `\` opens the layout bar: `→` and `↓` split the selected pane, `Tab`
/// selects the next pane and moves nothing, `M` moves the shown tab into the
/// next pane (one leaving its pane empty closes it, which the hint says
/// first), `J` joins, and every other key is the bar's until Esc. `|` and `[`
/// do nothing.
#[test]
fn the_layout_bar_splits_selects_moves_and_joins() {
    let mut a = atlas_with_docs();
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('|'));
    assert_eq!(a.panes().count(), 1, "| is not a key any more");
    press(&mut a, KeyCode::Char('3'));
    press(&mut a, KeyCode::Char('\\'));
    assert!(a.layout_bar);
    press(&mut a, KeyCode::Right);
    assert_eq!(a.panes().count(), 2);
    assert_eq!((a.project_name(), a.harness_at(a.pane_focus)), ("docs", Some("claude")));
    assert!(!a.show_work(), "the list hides to make room");
    press(&mut a, KeyCode::Down);
    assert_eq!(a.hint_text(), Some(weft_core::words::word("why.split_needs_two")));
    press(&mut a, KeyCode::Char('a'));
    assert!(a.modal.is_none() && a.layout_bar);
    // Tab selects the other pane; nothing moves.
    press(&mut a, KeyCode::Tab);
    assert_eq!(a.panes().count(), 2);
    assert_eq!(a.panes().active().id, a.panes().panes()[0].id);
    assert_eq!(names(&a), ["weft · claude", "weft · codex", "docs · claude"]);
    // M moves its shown tab into the next pane.
    press(&mut a, KeyCode::Char('m'));
    assert_eq!(names(&a), ["weft · claude", "docs · claude", "weft · codex"]);
    press(&mut a, KeyCode::Char('['));
    assert_eq!(names(&a), ["weft · claude", "docs · claude", "weft · codex"], "[ moves nothing");
    // Weft's claude is alone in its pane now: the hint says M closes it.
    press(&mut a, KeyCode::Tab);
    assert_eq!(a.panes().active().tabs.len(), 1);
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.lines().last().is_some_and(|l| l.contains("closes")), "{drawn}");
    press(&mut a, KeyCode::Char('m'));
    assert_eq!(a.panes().count(), 1);
    press(&mut a, KeyCode::Down);
    assert_eq!(a.panes().count(), 2);
    press(&mut a, KeyCode::Char('j'));
    assert_eq!(a.panes().count(), 1);
    press(&mut a, KeyCode::Esc);
    assert!(!a.layout_bar);
}

/// Each pane is drawn at its place with its agent told that size; what is
/// typed reaches only the pane entered.
#[test]
fn each_pane_gets_its_size_and_the_keys_reach_only_the_pane_with_them() {
    let mut a = two_panes();
    let _ = screen(&mut a, 120, 32);
    let (left, right) = (a.pane_size_in(0, 0), a.pane_size_in(1, 0));
    assert_eq!((left, right), ((25, 58), (25, 57)));
    press(&mut a, KeyCode::Enter);
    assert_eq!((a.project_name(), a.focus), ("docs", Focus::Agent));
    press(&mut a, KeyCode::Char('q'));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && !a.pane_text(0).unwrap_or_default().contains('q')
    {
        let _ = a.turn(std::time::Duration::from_millis(50));
    }
    assert!(a.pane_text(0).unwrap_or_default().contains('q'), "docs' claude got it");
    assert!(!a.pane_text_in(0, 0).unwrap_or_default().contains('q'), "weft's did not");
    ctrl(&mut a, ']');
    assert_eq!((a.focus, a.pane_selected), (Focus::Weft, true));
}

/// A window too small for the layout shows the pane with the keys alone,
/// and the layout comes back when it fits again.
#[test]
fn a_window_too_small_for_the_layout_shows_the_pane_with_the_keys_alone() {
    let mut a = two_panes();
    let narrow = screen(&mut a, 70, 32);
    assert_eq!(narrow.lines().nth(2).map(|l| l.matches('┌').count()), Some(1), "{narrow}");
    assert!(narrow.contains("docs · claude") && !narrow.contains("weft · codex"), "{narrow}");
    assert_eq!(a.panes().count(), 2, "the layout is kept");
    let wide = screen(&mut a, 120, 32);
    assert_eq!(wide.lines().nth(2).map(|l| l.matches('┌').count()), Some(2), "{wide}");
}

/// Two panes side by side, each with its own tabs, the right one selected.
#[test]
fn atlas_t15_1_two_panes_side_by_side() {
    let mut a = two_panes();
    assert!(a.pane_selected);
    expect_screen("atlas_t15_1", &screen(&mut a, 120, 32));
}

/// The right pane entered: every key its agent's.
#[test]
fn atlas_t15_6_entered() {
    let mut a = two_panes();
    press(&mut a, KeyCode::Enter);
    assert_eq!(a.focus, Focus::Agent);
    expect_screen("atlas_t15_6", &screen(&mut a, 120, 32));
}

/// The layout bar, the left pane's tab with the keys.
#[test]
fn atlas_t15_2_the_layout_bar() {
    let mut a = two_panes();
    press(&mut a, KeyCode::Char('1'));
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('\\'));
    expect_screen("atlas_t15_2", &screen(&mut a, 120, 32));
}

/// One pane below the other.
#[test]
fn atlas_t15_3_one_below_the_other() {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('3'));
    layout(&mut a, &[KeyCode::Down]);
    for n in ['1', '3'] {
        a.focus = Focus::Weft;
        press(&mut a, KeyCode::Char(n));
    }
    a.focus = Focus::Weft;
    expect_screen("atlas_t15_3", &screen(&mut a, 120, 32));
}

/// Show the nth tab of that name by its number, from the list.
fn go(a: &mut App, name: &str, nth: usize) {
    let n = names(a).iter().enumerate().filter(|(_, n)| *n == name).nth(nth).map(|(i, _)| i);
    let n = n.unwrap_or_else(|| panic!("{name} #{nth}: {:?}", names(a)));
    a.focus = Focus::Weft;
    press(a, KeyCode::Char(char::from_digit(n as u32 + 1, 10).expect("a digit")));
}

/// Four panes, the most: a fifth split says so and nothing moves.
#[test]
fn atlas_t15_4_four_panes_the_most() {
    let mut a = atlas_with_docs();
    a.open_project(&a.root().with_file_name("docs").to_string_lossy());
    a.add("codex", "/bin/cat").expect("codex in docs");
    a.add("claude", "/bin/cat").expect("a second claude in docs");
    a.settle();
    let _ = screen(&mut a, 120, 32);
    // As the card lays them out: one each above, the right one docs' codex;
    // weft's codex below on the left, docs' two claudes on the right.
    go(&mut a, "docs · codex", 0);
    layout(&mut a, &[KeyCode::Right]);
    go(&mut a, "weft · codex", 0);
    layout(&mut a, &[KeyCode::Down]);
    for _ in 0..2 {
        go(&mut a, "docs · claude", 0);
        layout(&mut a, &[KeyCode::Char('m')]);
    }
    go(&mut a, "docs · codex", 0);
    layout(&mut a, &[KeyCode::Down]);
    go(&mut a, "docs · claude", 1);
    layout(&mut a, &[KeyCode::Char('m'), KeyCode::Char('m')]);
    let shown: Vec<_> = a.panes().panes().iter().map(|p| p.tabs.len()).collect();
    assert_eq!(shown, [1, 1, 1, 2], "{:?}", names(&a));
    go(&mut a, "docs · claude", 1);
    let before = names(&a);
    a.focus = Focus::Weft;
    press(&mut a, KeyCode::Char('\\'));
    press(&mut a, KeyCode::Right);
    assert_eq!(a.hint_text(), Some(weft_core::words::word("why.four_panes")));
    assert_eq!((a.panes().count(), names(&a)), (4, before), "nothing moved");
    expect_screen("atlas_t15_4", &screen(&mut a, 120, 32));
}

fn mouse(a: &mut App, kind: crossterm::event::MouseEventKind, (column, row): (u16, u16)) {
    a.on_mouse(crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
}

/// Where a tab is drawn, by its name: its first cell.
fn tab_at(a: &mut App, name: &str) -> (u16, u16) {
    let _ = screen(a, 120, 32);
    let tabs = a.tabs();
    let t = tabs.iter().find(|t| format!("{} · {}", t.project_name, t.harness) == name);
    let t = t.unwrap_or_else(|| panic!("{name}: {:?}", names(a)));
    let r = a.hits.iter().find(|(_, h)| *h == crate::app::Target::Tab(t.project, t.pane));
    let r = r.expect("drawn").0;
    (r.x, r.y)
}

/// Press a tab, drag it, and let it go.
fn drag(a: &mut App, name: &str, to: (u16, u16)) {
    use crossterm::event::{MouseButton, MouseEventKind};
    let from = tab_at(a, name);
    mouse(a, MouseEventKind::Down(MouseButton::Left), from);
    mouse(a, MouseEventKind::Drag(MouseButton::Left), to);
    mouse(a, MouseEventKind::Up(MouseButton::Left), to);
}

/// A tab dragged with the mouse: along its bar it is reordered; onto a
/// pane's outer fifth it splits that pane that way; onto a pane's middle
/// or another bar it moves there; the last tab out of a pane closes it. A
/// press let go where it was shows the tab.
#[test]
fn a_dragged_tab_reorders_moves_splits_and_joins() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut a = atlas_with_docs();
    let first = tab_at(&mut a, "weft · claude");
    drag(&mut a, "weft · codex", first);
    assert_eq!(names(&a), ["weft · codex", "weft · claude", "docs · claude"]);
    // The pane's right fifth, beside the list: a pane on the right.
    drag(&mut a, "docs · claude", (115, 15));
    assert_eq!(a.panes().count(), 2);
    assert_eq!(a.panes().panes()[1].tabs.len(), 1);
    assert_eq!(a.project_name(), "docs", "the tab keeps the keys");
    // The right pane's middle.
    drag(&mut a, "weft · claude", (90, 15));
    assert_eq!(names(&a), ["weft · codex", "docs · claude", "weft · claude"]);
    // Onto the left pane's bar, after its tab: the right pane keeps one.
    drag(&mut a, "docs · claude", (40, 1));
    assert_eq!(names(&a), ["weft · codex", "docs · claude", "weft · claude"]);
    assert_eq!(a.panes().panes()[0].tabs.len(), 2);
    // The last tab out of a pane: it closes.
    drag(&mut a, "weft · claude", (30, 15));
    assert_eq!(a.panes().count(), 1);
    // Pressed and let go where it was: shown, and nothing moves.
    let at = tab_at(&mut a, "docs · claude");
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), at);
    mouse(&mut a, MouseEventKind::Up(MouseButton::Left), at);
    assert_eq!((a.project_name(), a.focus, a.pane_selected), ("docs", Focus::Weft, true));
    assert_eq!(names(&a), ["weft · codex", "docs · claude", "weft · claude"]);
}

/// A tab being dragged: where it would land is shaded, and it follows the
/// pointer.
#[test]
fn atlas_t15_5_a_tab_dragged() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let mut a = two_panes();
    let from = tab_at(&mut a, "weft · codex");
    mouse(&mut a, MouseEventKind::Down(MouseButton::Left), from);
    mouse(&mut a, MouseEventKind::Drag(MouseButton::Left), (90, 27));
    let drawn = screen(&mut a, 120, 32);
    assert_eq!(
        a.drop_at((90, 27)),
        Some(crate::panes::Drop::Edge {
            pane: a.panes().panes()[1].id,
            side: crate::panes::Side::Down
        })
    );
    expect_screen("atlas_t15_5", &drawn);
}

/// The Atlas's sample with docs open, its claude bound to a session of
/// that id, and the layout kept in a file.
fn kept(file: &std::path::Path, docs: &str) -> App {
    let mut a = atlas_with_docs();
    press(&mut a, KeyCode::Char('3'));
    let row = bound(&a, 0, docs, Some(weft_core::turns::Turn::Ready), 1, None);
    a.session_mut().sessions = vec![row];
    a.keep_layout_in(file.to_path_buf());
    let _ = screen(&mut a, 120, 32);
    a
}

/// The layout is kept by session id: a window started again lays out the
/// agents still running as they were, and an ended session's place goes.
#[test]
fn the_layout_is_kept_and_comes_back_for_what_still_runs() {
    let file = std::env::temp_dir().join(format!("weft-layout-{}.toml", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let mut a = kept(&file, "s_docs");
    layout(&mut a, &[KeyCode::Right]);
    let text = std::fs::read_to_string(&file).expect("kept");
    for id in ["s_claude", "s_codex", "s_docs"] {
        assert!(text.contains(id), "{text}");
    }
    // Started again, the same sessions running: the same panes.
    let again = kept(&file, "s_docs");
    assert_eq!(again.panes().count(), 2);
    assert_eq!(names(&again), names(&a));
    assert_eq!(again.panes().panes()[1].tabs.len(), 1);
    // docs' claude is another session now: its place goes, and it opens
    // in the pane with the keys.
    let other = kept(&file, "s_other");
    assert_eq!(other.panes().count(), 1);
    assert_eq!(names(&other), ["weft · claude", "weft · codex", "docs · claude"]);
    let _ = std::fs::remove_file(&file);
}

/// T9.7: N starts an agent; each set-up harness starts fresh.
#[test]
fn atlas_t9_7_start_an_agent() {
    let mut a = atlas_with_docs();
    let fresh = |h: &str| crate::app::Start {
        harness: h.into(),
        label: format!("{h} · start fresh"),
        spec: "/bin/cat".into(),
    };
    a.set_starts(vec![fresh("claude"), fresh("codex")]);
    a.modal = Some(Modal::StartAgent { choice: 0 });
    expect_screen("atlas_t9_7", &screen(&mut a, 120, 32));
}

/// A project RingFrame refuses to set up, as `ringframe init` says why.
fn not_set_up(scratch: &str, folder: &str, code: &str) -> App {
    let said = format!(r#"{{"error": "{code}", "detail": "RingFrame's words"}}"#);
    let world = crate::app::tests::set_up(|w| w.answers("ringframe", &["init"], 2, &said));
    let (root, session) = crate::app::tests::test_session_at(scratch, folder, world);
    let mut a = App::with_session(root, crate::keys::Toggle, session);
    a.settle();
    a
}

/// T2.4: RingFrame is not set up here yet; the checklist says what it needs.
#[test]
fn atlas_t2_4_ringframe_not_set_up_here() {
    let mut a = not_set_up("weft-atlas", "weft", "workspace.no_git");
    expect_screen("atlas_t2_4", &screen(&mut a, 120, 32));
}

/// Each line is RingFrame's answer, and the checklist goes as soon as the
/// project is set up: Weft asks again while it is not.
#[test]
fn the_checklist_follows_ringframe_until_the_project_is_set_up() {
    let mut a = not_set_up("weft-app-unset", "", "workspace.no_commit");
    let drawn = screen(&mut a, 120, 32);
    for line in [
        "RingFrame is not set up here yet.",
        "✓ ringframe is installed",
        "✓ a Git repository",
        "✗ a first commit   commit once",
        "Weft sets it up as soon as these hold.",
    ] {
        assert!(drawn.contains(line), "{line}: {drawn}");
    }
    // RingFrame set it up: the workspace is there.
    std::fs::create_dir_all(a.root().join(".fab7/rf")).expect("the workspace");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while a.needs().is_some() && std::time::Instant::now() < deadline {
        let _ = a.turn(std::time::Duration::from_millis(100));
    }
    let drawn = screen(&mut a, 120, 32);
    assert!(!drawn.contains("RingFrame is not set up here yet."), "{drawn}");
    assert!(drawn.contains("Nothing asked for yet."), "{drawn}");
}

/// Under the agents already running, the checklist takes the place of what
/// to ask; with no ringframe, its first line is the one to fix.
#[test]
fn the_checklist_sits_under_the_agents_and_names_a_missing_ringframe() {
    let mut a = app();
    a.session_mut().needs = Some(weft_core::workspace::Needs::refused(None));
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("✗ ringframe is installed"), "{drawn}");
    assert!(drawn.contains("run Weft's installer again"), "{drawn}");
    assert!(drawn.contains("? a Git repository"), "{drawn}");
    assert!(!drawn.contains("[A]SK for something"), "{drawn}");
}

/// What claude shows in the Atlas's cards, typed into its pane's emulator.
const CLAUDE_AT_WORK: &str = " ● Read(src/login.rs)\r\n ● Update(src/login.rs)\r\n   └  Updated src/login.rs with 12 additions and 3 removals\r\n ● Running cargo test…\r\n   test login::rejects_expired ... ok\r\n   test login::refreshes_token ... ok\r\n\r\n ✻ Thinking… (esc to interrupt)\r\n\r\n ❯\x1b[?25l";

/// The Atlas's sample while claude works: fix login working, telemetry
/// still being asked, and claude's screen in its pane.
fn atlas_at_work() -> App {
    use weft_core::sessions::{Row, State};
    use weft_core::turns::Turn;
    let mut a = atlas();
    let mut units = a.units().to_vec();
    let mut telemetry = unit(Sent::NotSent);
    (telemetry.ask_id, telemetry.title, telemetry.harness) =
        ("ask_2".into(), "telemetry".into(), "claude".into());
    telemetry.requested_only = true;
    let mut readme = units[1].clone();
    (readme.ask_id, readme.title, readme.sealed) =
        ("ask_4".into(), "readme fix".into(), Some("accepted".into()));
    readme.seal_id = Some("sel_4".into());
    units.insert(1, telemetry);
    units.push(readme);
    a.set_units(units);
    a.session_mut().sessions = vec![
        bound(&a, 0, "s_claude", Some(Turn::Working), 4, Some("fix login")),
        bound(&a, 1, "s_codex", Some(Turn::TurnEnded), 43, Some("health endpoint")),
    ];
    let _ = (Row::of, State::Running);
    a.session_mut().set_turns(vec![Some(Turn::Working), Some(Turn::TurnEnded)]);
    let _ = screen(&mut a, 120, 32);
    a.session_mut().panes[0].feed(format!("\x1b[2J\x1b[H{CLAUDE_AT_WORK}").as_bytes());
    a
}

/// T3.1: Weft has the keys; the pane's frame is grey.
#[test]
fn atlas_t3_1_the_agent_inactive() {
    let mut a = atlas_at_work();
    a.focus = Focus::Weft;
    expect_screen("atlas_t3_1", &screen(&mut a, 120, 32));
}

/// T3.3: the agent has the keys; its frame is blue.
#[test]
fn atlas_t3_3_the_agent_active() {
    let mut a = atlas_at_work();
    a.focus = Focus::Agent;
    expect_screen("atlas_t3_3", &screen(&mut a, 120, 32));
}

/// T3.5: scrolled back, the pane reads its history inside the same blue
/// frame, and nothing on screen says READING or LIVE.
#[test]
fn atlas_t3_5_the_agent_scrolled_back() {
    let mut a = atlas_at_work();
    let earlier = " ● Running cargo test…\r\n   test login::rejects_expired ... ok\r\n   test login::refreshes_token ... ok\r\n   test result: ok. 163 passed; 0 failed\r\n ● Update(src/login.rs)\r\n   └  Updated src/login.rs\r\n …\r\n";
    let p = &mut a.session_mut().panes[0];
    p.feed(format!("\x1b[2J\x1b[H{earlier}{}{CLAUDE_AT_WORK}", "\r\n".repeat(17)).as_bytes());
    for _ in 0..40 {
        let top = p.with_screen(|s| s.contents().lines().next().unwrap_or_default().to_string());
        if top == " ● Running cargo test…" {
            break;
        }
        p.scroll(1);
    }
    a.focus = Focus::Agent;
    let drawn = screen(&mut a, 120, 32);
    assert!(!drawn.contains("READING") && !drawn.contains("LIVE"), "{drawn}");
    expect_screen("atlas_t3_5", &drawn);
}

/// Focus moves the frame's state: light while Weft has the keys, heavy
/// while the agent has them.
#[test]
fn focus_moves_the_frames_state() {
    let mut a = atlas_at_work();
    a.focus = Focus::Weft;
    let weft = screen(&mut a, 120, 32);
    assert!(weft.contains('┌') && !weft.contains('┏'), "{weft}");
    // Enter on its harness gives the agent the keys; a tab's digit or Tab
    // selects it, and Enter then enters it.
    for keys in [
        &[KeyCode::Enter][..],
        &[KeyCode::Char('2'), KeyCode::Enter],
        &[KeyCode::Tab, KeyCode::Enter],
    ] {
        a.focus = Focus::Weft;
        a.pane_selected = false;
        assert!(a.select(|r| matches!(r, crate::app::Row::Agent { .. })));
        for &key in keys {
            press(&mut a, key);
        }
        assert_eq!(a.focus, Focus::Agent, "{keys:?}");
    }
    let agent = screen(&mut a, 120, 32);
    assert!(agent.contains('┏') && !agent.contains('┌'), "{agent}");
}

/// T4.1: Ask, empty, with SEND TO a list of the agents.
#[test]
fn atlas_t4_1_ask_empty() {
    let mut a = atlas();
    press(&mut a, KeyCode::Char('a'));
    expect_screen("atlas_t4_1", &screen(&mut a, 120, 32));
}

/// A pending as the daemon stages it, on the pane at `pane`.
fn staged(pane: usize, asking: weft_core::offers::Asking, payload: &str) -> crate::app::Pending {
    crate::app::Pending {
        staged: "pnd_1".into(),
        pane,
        payload: payload.as_bytes().to_vec(),
        what: asking.what,
        why: asking.why,
    }
}

/// T4.5: the confirmation is a bar, CANCEL beside SEND.
#[test]
fn atlas_t4_5_the_bar_ready_to_ask() {
    let mut a = atlas();
    let asking = weft_core::offers::asking("/rf:ask", "claude");
    let pending =
        staged(0, asking, "Read the build number from the environment and show it on /health");
    a.modal = Some(Modal::Bar(Bar::Send(pending)));
    expect_screen("atlas_t4_5", &screen(&mut a, 120, 32));
}

/// T4.11: the agent has not said it is ready: CANCEL beside SEND ANYWAY.
#[test]
fn atlas_t4_11_the_bar_send_anyway() {
    let mut a = atlas();
    let asking = weft_core::offers::asking("/rf:ask", "claude");
    let pending = staged(0, asking, "Read the build number");
    let why = weft_core::words::word(weft_core::turns::WORKING);
    a.modal = Some(Modal::Bar(Bar::SendAnyway { pending, why }));
    expect_screen("atlas_t4_11", &screen(&mut a, 120, 32));
}

// --- Section 11: agents and rounds -----------------------------------------

/// A session of the 11.x sample, ended or past, `day` days before the 8th.
fn past(
    harness: &str,
    id: &str,
    title: Option<&str>,
    asks: usize,
    day: i64,
    hm: (i64, i64),
) -> weft_core::sessions::Row {
    let at = AT_11_35 - (11 * 60 + 35) * 60_000 - day * 86_400_000 + (hm.0 * 60 + hm.1) * 60_000;
    let mut r =
        weft_core::sessions::Row::of(harness, id, weft_core::sessions::State::Ended, None, at);
    r.title = title.map(str::to_string);
    r.asks = asks;
    r
}

/// The 11.x cards' sample: weft with three agents running (claude waiting,
/// codex working, codex whose turn ended), fifteen ended, Open since Eval 3,
/// Eval 3 not sealed, and four Seals over thirty-one Asks.
fn atlas11() -> App {
    use weft_core::rounds::{Eval, Named, Record, Round, Rounds, Seal, Sealed};
    use weft_core::turns::Turn;
    let (root, session) = crate::app::tests::test_session_at(
        "weft-atlas11",
        "weft",
        crate::app::tests::set_up(|w| w),
    );
    let mut a = App::with_session(root, crate::keys::Toggle, session);
    a.add("claude", "/bin/cat").expect("claude");
    a.add("codex", "/bin/cat").expect("codex");
    a.add("codex", "/bin/cat").expect("codex again");
    a.settle();
    settle_agents(&mut a);
    for h in ["claude", "codex"] {
        a.set_readiness(h, weft_core::readiness::Readiness::Ready);
    }
    let check = Check {
        eval_id: "evl_3".into(),
        verdict: Verdict::Matches,
        agreement: 0.67,
        judged_by: Some("codex".into()),
    };
    let work = |id: &str, title: &str, harness: &str, at: &str, sent: Sent| {
        let mut u = unit(sent);
        (u.ask_id, u.title, u.harness, u.asked_at) =
            (id.into(), title.into(), harness.into(), at.into());
        u
    };
    let mut fix =
        work("ask_1", "fix login", "claude", "2026-10-08T11:29:00Z", Sent::Arrived { exact: true });
    (fix.session_ref, fix.arrived_at) = (Some("k1".into()), Some("2026-10-08T11:29:00Z".into()));
    let mut telemetry = work("ask_2", "telemetry", "claude", "2026-10-08T11:20:00Z", Sent::NotSent);
    telemetry.requested_only = true;
    let mut health = work(
        "ask_3",
        "health endpoint",
        "codex",
        "2026-10-08T10:40:00Z",
        Sent::Arrived { exact: true },
    );
    (health.check, health.session_ref) = (Some(check.clone()), Some("c2".into()));
    let mut build = work(
        "ask_4",
        "build number",
        "codex",
        "2026-10-08T10:30:00Z",
        Sent::Arrived { exact: true },
    );
    build.check = Some(check);
    let mut readme = work(
        "ask_5",
        "readme fix",
        "claude",
        "2026-10-07T09:00:00Z",
        Sent::Arrived { exact: true },
    );
    (readme.sealed, readme.seal_id, readme.session_ref) =
        (Some("accepted".into()), Some("sel_4".into()), Some(ENDED_CLAUDE.into()));
    readme.check = Some(Check {
        eval_id: "evl_2".into(),
        verdict: Verdict::Matches,
        agreement: 1.0,
        judged_by: Some("codex".into()),
    });
    let mut docs =
        work("ask_6", "docs links", "codex", "2026-10-07T08:00:00Z", Sent::Arrived { exact: true });
    (docs.sealed, docs.seal_id, docs.check) =
        (Some("accepted".into()), Some("sel_4".into()), readme.check.clone());
    a.set_units(vec![fix, telemetry, health, build, readme, docs]);

    let evals = (1..=3)
        .map(|n| Eval {
            id: format!("evl_{n}"),
            n,
            opened_at: String::new(),
            asks: match n {
                3 => vec!["ask_3".into(), "ask_4".into()],
                2 => vec!["ask_5".into(), "ask_6".into()],
                _ => Vec::new(),
            },
            verdict: Some(Verdict::Matches),
            agreement: Some(if n == 3 { 0.67 } else { 1.0 }),
            judged_by: Some("codex".into()),
            completed_at: Some("2026-10-08T11:05:00Z".into()),
            voided: false,
        })
        .collect();
    let seals = (1..=4)
        .map(|n| Seal {
            id: format!("sel_{n}"),
            n,
            at: "2026-10-07T18:40:00Z".into(),
            disposition: "accepted".into(),
            by: "you".into(),
            eval: (n >= 3).then(|| format!("evl_{}", n - 2)),
        })
        .collect();
    let sealed = |n: usize, asks: Vec<String>| Sealed {
        seal: format!("sel_{n}"),
        n,
        decision: "accepted".into(),
        at: String::new(),
        by: "you".into(),
        eval: (n >= 3).then(|| Named { id: format!("evl_{}", n - 2), n: n - 2 }),
        asks,
    };
    let many = |k: usize| (0..k).map(|i| format!("old_{i}")).collect::<Vec<_>>();
    a.session_mut().rounds = Rounds {
        judging: Vec::new(),
        open: vec!["ask_1".into(), "ask_2".into()],
        since: Some(3),
        evals: vec![Round {
            eval: "evl_3".into(),
            n: 3,
            judging: false,
            verdict: Some(Verdict::Matches),
            agreement: Some(0.67),
            asks: vec!["ask_3".into(), "ask_4".into()],
            sealed: None,
        }],
        seals: vec![
            sealed(4, vec!["ask_5".into(), "ask_6".into()]),
            sealed(3, many(9)),
            sealed(2, many(11)),
            sealed(1, many(9)),
        ],
        record: Record { evals, seals },
    };
    // The record as the daemon reads it and sends it.
    let judges = weft_core::record::Record::parse(&serde_json::json!({
        "eval_id": "evl_3", "verdict": "aligned", "confidence": 0.67,
        "judgements": [
            {"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5.6-terra"}},
            {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5.6-terra"}}],
        "items": [
            {"text": "/health returns the build number and the uptime", "majority": "yes", "agreement": 1.0},
            {"text": "the build number comes from the git tag, not a literal \"dev\"", "majority": "yes", "agreement": 1.0}],
        "checks": [{"command": "cargo test", "outcome": "succeeded", "seconds": 12}]
    }))
    .expect("a record");
    a.set_records(serde_json::json!({"evl_3": judges}));
    a.set_clock(AT_11_35, 0);
    let mut sessions = vec![
        bound(&a, 1, "c1", Some(Turn::Working), 4, None),
        bound(&a, 0, "k1", Some(Turn::Waiting), 6, Some("fix login")),
        bound(&a, 2, "c2", Some(Turn::TurnEnded), 43, Some("health endpoint")),
    ];
    let mut ended = past("claude", ENDED_CLAUDE, Some("readme fix"), 1, 0, (9, 12));
    ended.started = ended.last - 14 * 60_000;
    sessions.push(ended);
    sessions.push(past("codex", "a0c7e1d2", Some("lint pass"), 2, 0, (8, 30)));
    sessions.push(past("claude", "f3b8d9e0", None, 0, 0, (8, 2)));
    for (h, id, title, asks, day, hm) in [
        ("codex", "1a9e40c3", Some("flaky test"), 3, 0, (7, 40)),
        ("claude", "77d0b2e1", None, 0, 0, (7, 15)),
        ("claude", "c41f9a07", Some("token refresh"), 2, 1, (18, 22)),
        ("codex", "5be2d8f4", Some("cli --help text"), 1, 1, (16, 5)),
        ("codex", "09ad3e6b", Some("readme badge"), 1, 1, (14, 40)),
        ("claude", "e7c2105a", Some("login form errors"), 4, 1, (10, 13)),
        ("claude", "3f8b6c21", Some("pane resize glitch"), 2, 2, (17, 48)),
        ("codex", "a6d4e9f0", Some("config.toml loader"), 3, 2, (15, 2)),
        ("codex", "2c90b7e5", None, 0, 2, (9, 31)),
        ("claude", "8e15f3ad", Some("notify on waiting"), 1, 3, (19, 20)),
        ("codex", "b47a0c62", Some("tunnel picker"), 2, 3, (14, 55)),
        ("claude", "61dd8e93", Some("project picker typing"), 1, 3, (10, 7)),
    ] {
        sessions.push(past(h, id, title, asks, day, hm));
    }
    a.session_mut().sessions = sessions;
    a.session_mut().set_turns(vec![
        Some(Turn::Waiting),
        Some(Turn::Working),
        Some(Turn::TurnEnded),
    ]);
    let _ = screen(&mut a, 120, 32);
    a.session_mut().panes[0].feed(CLAUDE_ASKS.as_bytes());
    let _ = screen(&mut a, 120, 32);
    assert!(
        a.select(|r| matches!(r, crate::app::Row::Agent { agent, .. } if agent.session == "k1"))
    );
    a
}

/// The ended claude session of the 11.x sample: `s 883b9d12`.
const ENDED_CLAUDE: &str = "883b9d12-4c1e-4f0a-9d2e-7a1b3c5d6e7f";

/// What claude shows in T11.1, asking to make an edit.
const CLAUDE_ASKS: &str = "\x1b[2J\x1b[H   ● Write(src/login.rs)\r\n   Do you want to make this edit to login.rs?\r\n   ❯ 1. Yes\r\n     2. Yes, and don't ask again this session\r\n     3. No, and tell Claude what to do differently (esc)\x1b[?25l";

/// T11.1: AGENTS and WORK.
#[test]
fn atlas_t11_1_agents_and_rounds() {
    let mut a = atlas11();
    expect_screen("atlas_t11_1", &screen(&mut a, 120, 32));
}

/// T11.2: a work's Details, its Ask only.
#[test]
fn atlas_t11_2_a_work() {
    let mut a = atlas11();
    a.open_view_read(
        crate::app::View::Work("ask_1".into()),
        Some("Refresh the token before it expires, not after"),
        Some("In src/login.rs refresh the session token 60 s before expiry, and retry once on a 401\nbefore signing the person out …"),
        None,
    );
    expect_screen("atlas_t11_2", &screen(&mut a, 120, 32));
}

/// T11.3: an Eval, once.
#[test]
fn atlas_t11_3_an_eval() {
    let mut a = atlas11();
    a.open_view_read(crate::app::View::Eval("evl_3".into()), None, None, None);
    expect_screen("atlas_t11_3", &screen(&mut a, 120, 32));
}

/// T11.4: a Seal, once.
#[test]
fn atlas_t11_4_a_seal() {
    let mut a = atlas11();
    let held = serde_json::json!({"seal_id": "rcp_7f3a2c", "codes": [], "subject_matches": true});
    a.open_view_read(crate::app::View::Seal("sel_4".into()), None, None, Some(held));
    expect_screen("atlas_t11_4", &screen(&mut a, 120, 32));
}

/// T11.5: a session that has ended.
#[test]
fn atlas_t11_5_a_session() {
    let mut a = atlas11();
    a.open_view_read(crate::app::View::Session(ENDED_CLAUDE.into()), None, None, None);
    expect_screen("atlas_t11_5", &screen(&mut a, 120, 32));
}

/// T11.9: a running session no pane of Weft's holds, of a harness whose
/// resume joins it: [R]ESUME THIS joins it.
#[test]
fn atlas_t11_9_a_running_session_it_can_join() {
    let mut a = atlas11();
    let mut joined = past("codex", "01a12dd3-e1a3-7031-85a5-11ae2080309f", None, 0, 0, (11, 2));
    joined.state = weft_core::sessions::State::Running;
    joined.turn = Some(weft_core::turns::Turn::Working);
    joined.last = joined.started + 18 * 60_000;
    a.session_mut().sessions.push(joined);
    a.open_view_read(
        crate::app::View::Session("01a12dd3-e1a3-7031-85a5-11ae2080309f".into()),
        None,
        None,
        None,
    );
    expect_screen("atlas_t11_9", &screen(&mut a, 120, 32));
}

/// Enter on a session that runs with no pane of Weft's (Weft restarted, the
/// harness's own server still holding it) opens its Details, where Resume
/// joins it, when its harness's resume joins; otherwise it says another
/// program runs it.
#[test]
fn enter_on_a_running_session_no_pane_holds_opens_its_details_to_join_it() {
    use crate::app::{Row, View};
    let mut a = atlas11();
    for (harness, id) in [
        ("codex", "01a12dd3-e1a3-7031-85a5-11ae2080309f"),
        ("claude", "c1a12dd3-e1a3-7031-85a5-11ae2080309f"),
    ] {
        let mut running = past(harness, id, None, 0, 0, (11, 2));
        running.state = weft_core::sessions::State::Running;
        running.turn = Some(weft_core::turns::Turn::Working);
        // Running sessions come first, as the daemon orders them.
        a.session_mut().sessions.insert(0, running);
    }
    let on = |id: &'static str| move |r: &Row| matches!(r, Row::Agent { agent, .. } if agent.session == id);
    assert!(a.select(on("01a12dd3-e1a3-7031-85a5-11ae2080309f")), "codex's row in the tree");
    press(&mut a, KeyCode::Enter);
    assert!(
        matches!(a.detail().map(|d| &d.view), Some(View::Session(id)) if id == "01a12dd3-e1a3-7031-85a5-11ae2080309f"),
        "its Details, where Resume joins it"
    );
    press(&mut a, KeyCode::Esc);
    assert!(a.select(on("c1a12dd3-e1a3-7031-85a5-11ae2080309f")), "claude's row in the tree");
    press(&mut a, KeyCode::Enter);
    assert!(a.detail().is_none());
    assert_eq!(
        a.hint_text(),
        Some(weft_core::words::fill("why.elsewhere", &[("harness", "claude")]).as_str())
    );
}

/// T11.7: Enter on `▸ n earlier sessions` lists them under it, in the tree.
#[test]
fn atlas_t11_7_earlier_sessions_expanded() {
    use crate::app::Row;
    let mut a = atlas11();
    assert!(a.select(|r| matches!(r, Row::Earlier { open: false, .. })));
    press(&mut a, KeyCode::Enter);
    assert!(a.detail().is_none(), "no screen of its own: the tree opens in place");
    assert!(matches!(a.rows()[a.selected], Row::Earlier { open: true, .. }));
    expect_screen("atlas_t11_7", &screen(&mut a, 120, 32));
    // Enter again folds it, and the sessions go.
    press(&mut a, KeyCode::Enter);
    assert!(matches!(a.rows()[a.selected], Row::Earlier { open: false, .. }));
    assert!(a.rows().iter().all(|r| !matches!(r, Row::Agent { earlier: true, .. })));
}

/// T11.1's keys: Enter on each kind of row goes where the card says.
#[test]
fn enter_on_each_row_goes_where_t11_1_says() {
    use crate::app::{Row, View};
    let mut a = atlas11();
    let view = |a: &App| a.detail().map(|d| d.view.clone());
    // A running agent: its pane.
    press(&mut a, KeyCode::Enter);
    assert_eq!((a.focus, a.pane_focus), (Focus::Agent, 0), "k1 runs in the first pane");
    a.focus = Focus::Weft;
    type Pick<'a> = &'a dyn Fn(&Row) -> bool;
    let cases: [(Pick, View); 3] = [
        (
            &|r| matches!(r, Row::Agent { agent, .. } if agent.session == ENDED_CLAUDE),
            View::Session(ENDED_CLAUDE.into()),
        ),
        (&|r| matches!(r, Row::Ask { ask, .. } if ask.unit == "ask_1"), View::Work("ask_1".into())),
        (&|r| matches!(r, Row::Eval { .. }), View::Eval("evl_3".into())),
    ];
    for (pick, want) in cases {
        assert!(a.select(pick), "{want:?}");
        press(&mut a, KeyCode::Enter);
        assert_eq!(view(&a), Some(want));
        press(&mut a, KeyCode::Left);
        assert_eq!(view(&a), None, "← goes back to the tree");
    }
    // Sealed unfolds into its Seals, and a Seal opens.
    assert!(a.select(|r| matches!(r, Row::Sealed { .. })));
    press(&mut a, KeyCode::Enter);
    let seals = a.rows().iter().filter(|r| matches!(r, Row::Seal { .. })).count();
    assert_eq!(seals, 4);
    assert!(a.select(|r| matches!(r, Row::Seal { seal, .. } if seal.n == 4)));
    press(&mut a, KeyCode::Enter);
    assert_eq!(view(&a), Some(View::Seal("sel_4".into())));
    // A session that runs with no pane of Weft's: codex's resume joins it,
    // so Enter opens its Details, where Resume joins it.
    a.close_all_details();
    a.session_mut().sessions[0].pane = None;
    assert!(a.select(|r| matches!(r, Row::Agent { agent, .. } if agent.session == "c1")));
    press(&mut a, KeyCode::Enter);
    assert_eq!(view(&a), Some(View::Session("c1".into())));
    assert_eq!(a.focus, Focus::Weft);
}

/// T11.1: Ask on an ended agent says how to Ask it (D12.4); Seal on Eval 3
/// asks before typing; Seal in a sealed Eval's Details says so (D12.6).
#[test]
fn acts_on_the_sample_say_why_in_the_cards_words() {
    use crate::app::{Row, View};
    let mut a = atlas11();
    assert!(a.select(|r| matches!(r, Row::Agent { agent, .. } if agent.session == ENDED_CLAUDE)));
    press(&mut a, KeyCode::Char('a'));
    assert_eq!(a.hint_text(), Some("claude has ended: resume this session to Ask it."));
    a.open_view_read(View::Eval("evl_2".into()), None, None, None);
    press(&mut a, KeyCode::Char('s'));
    assert_eq!(a.hint_text(), Some("Eval 2 is sealed already."));
    assert!(a.modal.is_none(), "the act stays, and says why");
}

/// T11.5: R resumes the id shown at once, as the web's Resume does: it asks
/// the daemon for that id, with no confirm first.
#[test]
fn r_resumes_the_id_shown_at_once() {
    let mut a = atlas11();
    a.open_view_read(crate::app::View::Session(ENDED_CLAUDE.into()), None, None, None);
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("[R]ESUME THIS starts claude again on session s 883b9d12."), "{drawn}");
    assert!(!drawn.contains("asks first"), "{drawn}");
    press(&mut a, KeyCode::Char('r'));
    // This sample's daemon has never heard of the session, so it refuses
    // that id, by name: nothing else was asked for, and nothing asked first.
    let Some(Modal::Note(said)) = a.modal.clone() else { panic!("{:?}", a.modal) };
    assert!(said.contains(ENDED_CLAUDE), "{said}");
}

/// T11.2: Enter on a running session goes to its pane.
#[test]
fn enter_on_a_running_sessions_line_goes_to_its_pane() {
    let mut a = atlas11();
    a.open_view_read(crate::app::View::Work("ask_1".into()), None, None, None);
    press(&mut a, KeyCode::Enter);
    assert_eq!((a.focus, a.pane_focus), (Focus::Agent, 0));
    assert!(a.detail().is_none(), "the Details close as the keys go to the agent");
}

/// T11.8: `/` narrows the whole tree as it is typed: agents, earlier sessions
/// and works, by project.
#[test]
fn search_narrows_the_tree_across_agents_earlier_sessions_and_works() {
    use crate::app::Row;
    let mut a = atlas11();
    let all = a.rows().len();
    press(&mut a, KeyCode::Char('/'));
    assert_eq!(a.tree_search(), Some(""));
    for c in "token".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    let rows = a.rows();
    let sessions: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            Row::Agent { agent, .. } => agent.title.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(sessions, ["token refresh"], "an earlier session matches without being opened");
    assert!(
        rows.iter().any(|r| matches!(r, Row::Project { matches: Some(1), .. })),
        "the project says how many: {rows:?}"
    );
    assert!(a.selected_row_is_agent(), "the selection lands on the first match");
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("1 match") && drawn.contains("/ token"), "{drawn}");
    // Backspacing widens it again, and a harness matches too.
    for _ in 0..5 {
        press(&mut a, KeyCode::Backspace);
    }
    for c in "codex".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    assert!(a.rows().iter().filter(|r| matches!(r, Row::Agent { .. })).count() > 1);
    // Nothing matches: it says so, and Esc brings the tree back.
    for c in "zzz".chars() {
        press(&mut a, KeyCode::Char(c));
    }
    assert!(a.rows().is_empty());
    assert!(screen(&mut a, 120, 32).contains("Nothing matches codexzzz."));
    press(&mut a, KeyCode::Esc);
    assert_eq!(a.tree_search(), None);
    assert_eq!(a.rows().len(), all, "the tree is as it was");
}

/// With two projects open, the one with nothing in it says so under its own
/// heading; it never takes the list from the other. Quitting the only agent of
/// a project with no work leaves it that way.
#[test]
fn a_project_with_nothing_in_it_leaves_the_other_projects_listed() {
    let mut a = judged();
    let (other, _) = crate::app::tests::test_session("second");
    a.open_project(&other.to_string_lossy());
    a.add("codex", "/bin/cat").expect("an agent in the second project");
    a.settle();
    a.focus = Focus::Agent;
    a.input(0, &[4]).expect("Ctrl+D ends /bin/cat");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && a.pane_count() > 0 {
        a.pump();
        a.notice_an_agent_that_ended();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(a.pane_count(), 0, "the agent quit");
    let drawn = screen(&mut a, 120, 30);
    assert!(drawn.contains("readme fix"), "the first project's work is still listed: {drawn}");
    assert!(drawn.contains("Nothing asked for yet."), "the empty one says so: {drawn}");
    assert_eq!(drawn.matches("open  ⌫").count(), 2, "both projects are listed: {drawn}");
}

/// T7.9: with both releases reached and nothing behind, the view says so.
#[test]
fn the_update_view_says_when_nothing_is_newer() {
    use weft_core::onboarding::{Row, State, View};
    let mut a = atlas();
    let row = |name: &str, title: &str| Row {
        name: name.into(),
        title: title.into(),
        program: name.into(),
        state: State::Ready { version: Some("0.2.0".into()) },
    };
    a.session_mut().setup = Some(View {
        weft: "0.2.1 is the latest".into(),
        weft_latest: Some("0.2.1".into()),
        latest: Some("v0.2.0".into()),
        plugin: Some("0.2.0".into()),
        configuration: "0.2.0 is the latest".into(),
        rows: vec![row("claude-code", "Claude Code"), row("codex", "Codex")],
        ..View::default()
    });
    a.modal = Some(crate::app::Modal::RingFrame);
    let drawn = screen(&mut a, 120, 32);
    assert!(drawn.contains("You are using the latest version."), "{drawn}");
    assert!(drawn.contains("✓  Weft") && drawn.contains("✓  RingFrame"), "{drawn}");
    assert!(!drawn.contains("[U]PDATE"), "nothing to update lights nothing: {drawn}");
}
