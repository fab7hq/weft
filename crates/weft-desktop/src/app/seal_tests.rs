//! The Seal's questions on the desktop: read once when the Seal opens, one
//! at a time, each answered before the next, a review of every answer, and
//! only there Seal, which sends the answers with the decision.

use std::path::PathBuf;

use serde_json::{Value, json};
use weft_core::ask::{Draft, SealAnswer};
use weft_proto::Call;

use super::tests::{Wire, pumped, wired_app};
use super::*;
use crate::action::{BarAct, Focus, SheetAct};
use crate::gpui::{Element, palette};

fn weft() -> PathBuf {
    PathBuf::from("/work/weft")
}

/// What evl_3 left for its Seal: one asked before, one in the person's words.
fn two_questions() -> Value {
    json!({"questions": [
        {"id": "q1", "ask": "did_you_want",
         "behaviour": "The sign-in page remembers the last email address typed on this device.",
         "why": "You weren't sure last time, so it's asked again.",
         "asked_before": {"decision": "sel_1/d1", "seal": "sel_1", "date": "2026-11-10"}},
        {"id": "q2", "ask": "what_did_you_mean",
         "behaviour": "What you meant by “show an error”."}
    ], "eval": "evl_3"})
}

/// Open the Seal on health endpoint's work and answer its read of the
/// questions with `questions`.
fn open_seal(app: &mut WeftDesktopApp, wire: &mut Wire, questions: Value) {
    app.dispatch(SheetAct::SealWork { project: weft(), unit: "ask_3".into() });
    // What was asked before it, the test's own, is passed over.
    let id = loop {
        match wire.call() {
            (id, Call::Read { what, unit, .. }) if what == "questions" => {
                assert_eq!(unit, "evl_3", "the Seal reads its Eval's questions");
                break id;
            }
            _ => continue,
        }
    };
    wire.answer(id, questions);
    pumped(app, |a| {
        !matches!(
            a.bottom_bar,
            BottomBarState::SealDisposition { questions: SealQuestions::Reading, .. }
        )
    });
}

/// The Seal opened on work whose Eval asked nothing: today's dialog.
pub(crate) fn no_questions(app: &mut WeftDesktopApp, wire: &mut Wire) {
    open_seal(app, wire, json!({"questions": [], "eval": "evl_3"}));
}

fn seals(wire: &mut Wire) -> Vec<(Option<String>, Option<Draft>)> {
    wire.calls()
        .into_iter()
        .filter_map(|(_, c)| match c {
            Call::Act { act, text, draft, .. } if act == "seal" => Some((text, draft)),
            _ => None,
        })
        .collect()
}

fn dialog(app: &WeftDesktopApp) -> Element {
    app.render().find_layer("Seal dialog").expect("the Seal's dialog").clone()
}

fn every(el: &Element) -> Vec<&Element> {
    let mut out = vec![el];
    for c in &el.children {
        out.extend(every(c));
    }
    out
}

fn colour(el: &Element) -> Option<&str> {
    el.styles.iter().find(|(k, _)| k == "color").map(|(_, v)| v.as_str())
}

fn background(el: &Element) -> Option<&str> {
    el.styles.iter().find(|(k, _)| k == "background").map(|(_, v)| v.as_str())
}

fn confirm(disposition: &str, reason: Option<&str>) -> BarAct {
    BarAct::ConfirmSeal {
        project: weft(),
        eval: "evl_3".into(),
        disposition: disposition.into(),
        reason: reason.map(str::to_string),
    }
}

#[test]
fn the_seal_reads_its_evals_questions_once_and_waits_for_them() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::SealWork { project: weft(), unit: "ask_3".into() });
    let (id, call) = wire.call();
    assert!(
        matches!(&call, Call::Read { what, unit, .. } if what == "questions" && unit == "evl_3")
    );
    // While they are read, nothing can be sealed.
    let dom = app.render();
    assert!(dom.find_layer("btn Seal").is_none(), "no Seal while the questions are read");
    assert!(dom.all_text().contains("Reading what to ask you"), "{}", dom.all_text());
    app.dispatch(confirm("accepted", None));
    assert!(seals(&mut wire).is_empty(), "nothing is sealed while the questions are read");

    wire.answer(id, two_questions());
    pumped(&mut app, |a| a.seal_asking().is_some());
    let text = dialog(&app).all_text();
    for said in [
        "Seal health endpoint",
        "Question 1 of 2",
        "Asked before · not sure on 10 Nov",
        "The sign-in page remembers the last email address typed on this device.",
        "You weren't sure last time, so it's asked again.",
        "Did you want this?",
        "I wanted this",
        "Not wanted",
        "Not sure",
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }
    // It takes the window's body: no tree beside it, no bar under it.
    let dom = app.render();
    assert!(dom.find_layer("tree").is_none());
    assert!(dom.find_layer("Seal disposition bar").is_none());
    // Drawn again, it is not read again.
    app.render();
    app.dispatch(BarAct::SealAnswer("wanted".into()));
    let reads = wire.calls().into_iter().filter(|(_, c)| matches!(c, Call::Read { .. })).count();
    assert_eq!(reads, 0, "the questions are read once");
}

#[test]
fn each_answer_is_picked_without_sending_and_next_waits_for_one() {
    let (_tmp, mut app, mut wire) = wired_app();
    open_seal(&mut app, &mut wire, two_questions());

    // Next is not offered, and refused, until there is an answer.
    let next = dialog(&app).find_layer("btn Next").unwrap().clone();
    assert!(next.action.is_none(), "Next waits for an answer");
    app.dispatch(BarAct::SealNext);
    assert!(dialog(&app).all_text().contains("Question 1 of 2"));

    app.dispatch(BarAct::SealAnswer("not_wanted".into()));
    assert!(wire.calls().is_empty(), "an answer picked sends nothing");
    let d = dialog(&app);
    let picked = d.find_layer("answer not_wanted").unwrap();
    assert_eq!(picked.aria_selected, Some(true));
    assert_eq!(background(picked), Some(palette::AMBER_BORDER), "Not wanted fills with amber");
    assert!(picked.all_text().contains('✓'));
    assert_eq!(d.find_layer("answer wanted").unwrap().aria_selected, Some(false));
    assert!(d.find_layer("btn Next").unwrap().action.is_some());

    // The own-words question: its field takes the keyboard, and Next waits for words or Not sure.
    app.dispatch(BarAct::SealNext);
    let text = dialog(&app).all_text();
    assert!(text.contains("Question 2 of 2") && text.contains("What did you mean?"), "{text}");
    assert_eq!(app.focus, Focus::SealWords);
    assert!(dialog(&app).find_layer("btn Next").unwrap().action.is_none());
    app.dispatch(BarAct::SealNext);
    assert!(dialog(&app).all_text().contains("Question 2 of 2"), "no answer, no Next");
    app.dispatch(BarAct::SealAnswer("not_sure".into()));
    assert!(dialog(&app).find_layer("btn Next").unwrap().action.is_some(), "Not sure counts");
    app.seal_words.paste("A short message under the field.");
    let d = dialog(&app);
    assert_eq!(
        d.find_layer("answer not_sure").unwrap().aria_selected,
        Some(false),
        "words typed are the answer"
    );
    // Back keeps what was answered.
    app.dispatch(BarAct::SealBack);
    assert_eq!(dialog(&app).find_layer("answer not_wanted").unwrap().aria_selected, Some(true));
    assert_ne!(app.focus, Focus::SealWords);
    assert!(wire.calls().is_empty(), "still nothing sent");
}

#[test]
fn the_review_shows_every_answer_in_its_colour_and_seal_sends_them_all() {
    let (_tmp, mut app, mut wire) = wired_app();
    open_seal(&mut app, &mut wire, two_questions());
    app.dispatch(BarAct::SealAnswer("not_wanted".into()));
    app.dispatch(BarAct::SealNext);
    app.seal_words.paste("A short message under the field, in red.");
    // Seal is not there before the review, and refused if asked.
    assert!(dialog(&app).find_layer("btn Seal").is_none());
    app.dispatch(confirm("accepted", None));
    assert!(seals(&mut wire).is_empty(), "no Seal before every question is answered");
    app.dispatch(BarAct::SealNext);

    let d = dialog(&app);
    let text = d.all_text();
    for said in [
        "Your answers",
        "All 2 answered",
        "Asked before · not sure on 10 Nov",
        "Records your decision and these answers. Nothing is typed.",
        "Decision",
    ] {
        assert!(text.contains(said), "{said} in {text}");
    }
    let pill = d.find_layer("answer pill 0").unwrap();
    assert!(pill.all_text().contains("Not wanted"));
    assert_eq!(colour(pill), Some(palette::AMBER), "not wanted is said in amber");
    let pill = d.find_layer("answer pill 1").unwrap();
    assert!(pill.all_text().contains("“A short message under the field, in red.”"));
    assert_eq!(colour(pill), Some(palette::BLUE_BORDER), "own words in blue");
    for el in every(d.find_layer("every answer").unwrap()) {
        for (k, v) in &el.styles {
            assert!(!v.contains(palette::RED), "no answer is red: {k} {:?}", el.layer_name);
        }
    }

    // Change goes back to that question, its answer kept, and Next comes back.
    app.dispatch(BarAct::SealChange(0));
    let d = dialog(&app);
    assert!(d.all_text().contains("Question 1 of 2"));
    assert_eq!(d.find_layer("answer not_wanted").unwrap().aria_selected, Some(true));
    app.dispatch(BarAct::SealAnswer("wanted".into()));
    app.dispatch(BarAct::SealNext);
    assert_eq!(app.seal_words.text(), "A short message under the field, in red.");
    app.dispatch(BarAct::SealNext);
    let pill = dialog(&app).find_layer("answer pill 0").unwrap().clone();
    assert!(pill.all_text().contains("I wanted this"));
    assert_eq!(colour(&pill), Some(palette::TEAL));

    // The decision in its colour; a reject's reason takes it.
    let (project, eval) = (weft(), "evl_3".to_string());
    app.dispatch(BarAct::PickDisposition {
        project: project.clone(),
        eval: eval.clone(),
        disposition: "rejected".into(),
    });
    app.dispatch(BarAct::PickReason {
        project,
        eval,
        disposition: "rejected".into(),
        reason: Some("not_what_i_asked".into()),
    });
    let d = dialog(&app);
    assert!(d.find_layer("answer pill 0").is_some(), "a decision keeps the answers");
    assert_eq!(background(d.find_layer("disposition rejected").unwrap()), Some(palette::RED));
    assert_eq!(colour(d.find_layer("reason not_what_i_asked").unwrap()), Some(palette::RED));
    assert_eq!(colour(d.find_layer("reason wrong_approach").unwrap()), Some(palette::MUTED));
    let seal = d.find_layer("btn Seal").expect("Seal, on the review").clone();
    assert!(wire.calls().is_empty(), "nothing sent before Seal");

    app.dispatch(seal.action.clone().expect("Seal acts"));
    let sent = seals(&mut wire);
    assert_eq!(sent.len(), 1, "one Seal");
    let (text, draft) = sent[0].clone();
    assert_eq!(text.as_deref(), Some("rejected"));
    let draft = draft.expect("a draft");
    assert_eq!(draft.reason.as_deref(), Some("not_what_i_asked"));
    let answers = draft.answers.expect("the answers go with the Seal");
    assert_eq!(answers.len(), 2);
    assert_eq!(
        (answers[0].question.as_str(), answers[0].answer.as_str(), answers[0].words.as_deref()),
        ("q1", "wanted", None)
    );
    assert_eq!(
        answers[1],
        SealAnswer {
            question: "q2".into(),
            answer: "words".into(),
            words: Some("A short message under the field, in red.".into()),
            ms: answers[1].ms,
        }
    );
    assert!(answers.iter().all(|a| a.ms.is_some()), "each answer's time: {answers:?}");
    assert_eq!(app.bottom_bar, BottomBarState::None);
    assert_ne!(app.focus, Focus::SealWords);
}

#[test]
fn a_seal_whose_eval_asked_nothing_goes_as_it_always_has() {
    let (_tmp, mut app, mut wire) = wired_app();
    no_questions(&mut app, &mut wire);
    let dom = app.render();
    assert!(dom.find_layer("Seal dialog").is_none());
    let bar = dom.find_layer("Seal disposition bar").expect("today's dialog").all_text();
    for said in ["Seal this work?", "DECISION", "Nothing is typed."] {
        assert!(bar.contains(said), "{said} in {bar}");
    }
    assert!(!bar.contains("VERDICT"), "no verdict line: {bar}");
    // Its decisions take their colours too.
    assert_eq!(background(dom.find_layer("disposition accepted").unwrap()), Some(palette::TEAL));
    app.dispatch(confirm("accepted", None));
    let sent = seals(&mut wire);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.as_ref().and_then(|d| d.answers.clone()), None, "no answers to send");
}

#[test]
fn a_question_read_refused_leaves_the_seal_as_it_always_was() {
    let (_tmp, mut app, mut wire) = wired_app();
    app.dispatch(SheetAct::SealWork { project: weft(), unit: "ask_3".into() });
    let (id, _) = wire.call();
    wire.refuse(id, "RingFrame would not say");
    pumped(&mut app, |a| {
        matches!(
            a.bottom_bar,
            BottomBarState::SealDisposition { questions: SealQuestions::Unasked, .. }
        )
    });
    assert!(app.render().find_layer("btn Seal").is_some());
}

#[test]
fn the_seals_questions_use_only_what_the_window_draws() {
    let (_tmp, mut app, mut wire) = wired_app();
    open_seal(&mut app, &mut wire, two_questions());
    let mut screens = vec![app.render()];
    app.dispatch(BarAct::SealAnswer("not_sure".into()));
    screens.push(app.render());
    app.dispatch(BarAct::SealNext);
    app.dispatch(BarAct::SealAnswer("not_sure".into()));
    app.dispatch(BarAct::SealNext);
    screens.push(app.render());
    let radii = [format!("{}px", palette::RADIUS_CONTROL), format!("{}px", palette::RADIUS_PANEL)];
    for screen in &screens {
        for el in every(screen) {
            for (k, v) in &el.styles {
                assert!(
                    crate::window::STYLES.contains(&k.as_str()),
                    "{:?} uses {k}",
                    el.layer_name
                );
                assert!(
                    k != "border-radius" || radii.contains(v),
                    "{:?} rounds by {v}",
                    el.layer_name
                );
            }
        }
    }
}
