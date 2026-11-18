//! The Seal's dialog when its Eval left questions: one question at a time
//! (17.4), each answered before the next, then a review of every answer with
//! the decision and its reason (17.5), and only there Seal. It takes the
//! window's body. Picking an answer sends nothing; Seal sends it all, once.

use std::path::Path;

use weft_core::seal_questions::{Answering, Question, answer_tone, answer_words, decision_tone};
use weft_core::words::{fill, word};

use crate::action::{BarAct, Focus};
use crate::app::SealAsking;
use crate::daemon::ProjectClient;
use crate::gpui::{Element, div, input_el, palette, span};

pub struct SealProps<'a> {
    pub client: Option<&'a ProjectClient>,
    pub project: &'a Path,
    pub eval: &'a str,
    /// The decision picked, and its reason.
    pub picked: (&'a str, Option<&'a str>),
    pub asking: &'a SealAsking,
    /// What is typed in the own-words field.
    pub words: &'a str,
    /// What Weft reads of the setup of the harness the Seal goes to.
    pub warning: Option<&'a str>,
}

/// A tone's colours: its fill, its text on a tinted ground, and that ground.
pub fn tone(t: &str) -> (&'static str, &'static str, &'static str) {
    match t {
        "teal" => (palette::TEAL, palette::TEAL, palette::TEAL_BG),
        "blue" => (palette::BLUE_BORDER, palette::BLUE_BORDER, palette::SELECTED_BG),
        "amber" => (palette::AMBER_BORDER, palette::AMBER, palette::AMBER_BG),
        "red" => (palette::RED, palette::RED, palette::RED_BG),
        _ => (palette::MUTED, palette::MUTED, palette::SURFACE),
    }
}

/// Next before its question has an answer: the primary button at 45 %, as
/// the card dims it, over the dialog's ground.
const NEXT_WAITS_BG: &str = "#1f4882";
const NEXT_WAITS_LINE: &str = "#2e5483";

/// The column the question stands in, and the review's.
const QUESTION_WIDTH: u32 = 600;
const REVIEW_WIDTH: u32 = 720;

/// An answer's or a decision's dot, in its colour: the guideline's ●.
fn dot(colour: &str, size: u32) -> Element {
    span("●").shrink_0().font_size(size).color(colour)
}

fn gap(px: u32) -> Element {
    div().h(px).shrink_0()
}

pub fn render(props: &SealProps) -> Element {
    let a = &props.asking.answering;
    let col = match a.shown() {
        Some(q) => question(props, a, q),
        None => review(props, a),
    };
    div()
        .layer("Seal dialog")
        .self_stretch()
        .flex_1()
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .style("overflow-y", "auto")
        .child(div().self_stretch().flex_row().justify_center().pad(0, 16).child(col))
}

/// `Seal fix login`, what it counts, Cancel, and the thin line of progress.
fn head(props: &SealProps, counted: String, done: u32, width: u32) -> Vec<Element> {
    let client = props.client;
    let title = client
        .and_then(|c| {
            let unit = c.work.of_eval(props.eval)?;
            c.units.iter().find(|u| u.ask_id == unit.root).map(|u| u.title.clone())
        })
        .unwrap_or_default();
    vec![
        div()
            .layer("seal head")
            .self_stretch()
            .flex_row()
            .items_center()
            .font_size(14)
            .color(palette::MUTED)
            .child(span(fill("seal.q.title", &[("title", &title)])))
            .child(div().flex_1())
            .child(span(counted))
            .child(div().w(24).shrink_0())
            .child(span(word("act.cancel")).layer("btn Cancel").on_click(BarAct::Cancel)),
        gap(12),
        div()
            .layer("progress")
            .w(width)
            .h(2)
            .bg(palette::LINE)
            .child(div().w(done).h(2).bg(palette::BLUE_BORDER)),
    ]
}

/// The question shown: where it comes from, the behaviour, why, what it
/// asks, and its answers; Back and Next.
fn question(props: &SealProps, a: &Answering, q: &Question) -> Element {
    let n = a.questions.len() as u32;
    let done = QUESTION_WIDTH * (a.at as u32 + 1) / n.max(1);
    let given = a.given[a.at].as_ref();
    let typed = q.answers().contains(&"words") && !props.words.trim().is_empty();
    // Typed words are the answer; Not sure only while nothing is typed.
    let picked = |answer: &str| !typed && given.is_some_and(|g| g.answer == answer);
    let ready = typed || given.is_some_and(|g| g.answer != "words");

    let answer = |answer: &str| -> Element {
        let (fill, _, _) = tone(answer_tone(answer));
        if answer == "words" {
            return div()
                .layer("answer words")
                .self_stretch()
                .flex_row()
                .items_center()
                .gap(12)
                .h(48)
                .pad(0, 18)
                .bg(palette::SURFACE)
                .border(if typed { fill } else { palette::LINE })
                .child(dot(fill, 10))
                .child(
                    input_el(Focus::SealWords)
                        .flex_1()
                        .font_size(14)
                        .placeholder(answer_words("words"))
                        .text(props.words.to_string()),
                );
        }
        let on = picked(answer);
        div()
            .layer(format!("answer {answer}"))
            .on_click(BarAct::SealAnswer(answer.to_string()))
            .aria_selected(on)
            .self_stretch()
            .flex_row()
            .items_center()
            .gap(12)
            .h(48)
            .pad(0, 18)
            .bg(if on { fill } else { palette::SURFACE })
            .border(if on { fill } else { palette::LINE })
            .font_size(14)
            .font_weight(if on { 600 } else { 400 })
            .color(if on { palette::CANVAS } else { palette::TEXT })
            .child(dot(if on { palette::CANVAS } else { fill }, 10))
            .child(span(answer_words(answer)))
            .children(on.then(|| div().flex_1()))
            .children(on.then(|| span("✓")))
    };

    let next = div()
        .layer("btn Next")
        .shrink_0()
        .pad(10, 24)
        .font_weight(600)
        .font_size(14)
        .color(palette::CANVAS);
    let next = if ready {
        next.on_click(BarAct::SealNext).bg(palette::BLUE).border(palette::BLUE_BORDER)
    } else {
        next.bg(NEXT_WAITS_BG).border(NEXT_WAITS_LINE)
    };
    let back = span(word("seal.q.back")).layer("btn Back").font_size(14).color(if a.at == 0 {
        palette::DIM
    } else {
        palette::MUTED
    });
    let back = if a.at == 0 { back } else { back.on_click(BarAct::SealBack) };

    let mut col = div().layer("seal question").w(QUESTION_WIDTH).flex_col().pad(56, 0);
    col = col.children(head(props, a.counter(), done, QUESTION_WIDTH)).child(gap(48));
    if let Some(line) = q.before_line() {
        col = col
            .child(div().layer("asked before").font_size(14).color(palette::DIM).text(line))
            .child(gap(8));
    }
    col = col.child(
        div()
            .layer("behaviour")
            .font_size(20)
            .font_weight(600)
            .color(palette::TEXT)
            .text(q.behaviour.clone()),
    );
    if let Some(why) = q.why.as_deref().filter(|w| !w.is_empty()) {
        col = col
            .child(gap(8))
            .child(div().layer("why").font_size(14).color(palette::MUTED).text(why.to_string()));
    }
    col.child(gap(32))
        .child(
            div()
                .layer("question label")
                .font_size(14)
                .font_weight(600)
                .color(palette::TEXT)
                .text(q.label()),
        )
        .child(gap(12))
        .child(
            div()
                .layer("answers")
                .self_stretch()
                .flex_col()
                .gap(8)
                .children(q.answers().iter().map(|a| answer(a))),
        )
        .child(gap(40))
        .child(
            div()
                .layer("seal question acts")
                .self_stretch()
                .flex_row()
                .items_center()
                .child(back)
                .child(div().flex_1())
                .child(next),
        )
}

/// What an answer is shown as: its words, or the person's own in quotes.
fn said(answer: &str, words: Option<&str>) -> String {
    match words {
        Some(w) if answer == "words" => format!("“{w}”"),
        _ => answer_words(answer),
    }
}

/// Every answer, each with Change; then the decision and its reason, and Seal.
fn review(props: &SealProps, a: &Answering) -> Element {
    let n = a.questions.len();
    let rows = a.questions.iter().zip(&a.given).enumerate().map(|(i, (q, g))| {
        let (answer, words) =
            g.as_ref().map_or(("not_sure", None), |g| (g.answer.as_str(), g.words.as_deref()));
        let (_, text, ground) = tone(answer_tone(answer));
        let pill = div()
            .layer(format!("answer pill {i}"))
            .flex_row()
            .items_center()
            .gap(6)
            .pad(2, 10)
            .bg(ground)
            .border(text)
            .font_size(14)
            .font_weight(600)
            .color(text)
            .child(dot(text, 8))
            .child(span(said(answer, words)));
        div()
            .layer(format!("review {i}"))
            .self_stretch()
            .flex_row()
            .items_start()
            .gap(24)
            .pad(12, 0)
            .border_bottom(palette::LINE)
            .child(
                div()
                    .flex_1()
                    .flex_col()
                    .style("min-width", "0px")
                    .child(div().font_size(14).color(palette::TEXT).text(q.behaviour.clone()))
                    .children(
                        q.before_line().map(|l| div().font_size(14).color(palette::DIM).text(l)),
                    ),
            )
            .child(
                div()
                    .w(if words.is_some() { 340 } else { 240 })
                    .shrink_0()
                    .flex_row()
                    .justify_end()
                    .items_start()
                    .gap(12)
                    .child(pill)
                    .child(
                        span(word("seal.q.change"))
                            .layer(format!("btn Change {i}"))
                            .on_click(BarAct::SealChange(i))
                            .font_size(14)
                            .color(palette::MUTED)
                            .pad(2, 0),
                    ),
            )
    });

    let (selected, reason) = props.picked;
    let label =
        |key: &str| div().w(72).shrink_0().font_size(14).color(palette::MUTED).text(word(key));
    let reasons = reason_chips(props.project, props.eval, selected, reason);
    let records = word("seal.q.records").to_string();
    let seal = div()
        .layer("btn Seal")
        .shrink_0()
        .pad(10, 28)
        .bg(palette::BLUE)
        .border(palette::BLUE_BORDER)
        .font_weight(600)
        .font_size(14)
        .color(palette::CANVAS)
        .text(word("act.seal"));
    // Seal is there only once every question has an answer.
    let seal = a.may_seal().then(|| {
        seal.on_click(BarAct::ConfirmSeal {
            project: props.project.to_path_buf(),
            eval: props.eval.to_string(),
            disposition: selected.to_string(),
            reason: reason.map(str::to_string),
        })
    });

    div()
        .layer("seal review")
        .w(REVIEW_WIDTH)
        .flex_col()
        .pad(36, 0)
        .children(head(
            props,
            fill("seal.q.all_answered", &[("n", &n.to_string())]),
            REVIEW_WIDTH,
            REVIEW_WIDTH,
        ))
        .child(gap(24))
        .child(
            div()
                .font_size(20)
                .font_weight(600)
                .color(palette::TEXT)
                .text(word("seal.q.your_answers")),
        )
        .child(gap(8))
        .child(
            div()
                .layer("every answer")
                .self_stretch()
                .flex_col()
                .border_top(palette::LINE)
                .children(rows),
        )
        .child(gap(24))
        .child(
            div()
                .layer("decision")
                .flex_row()
                .items_center()
                .gap(16)
                .child(label("seal.q.decision"))
                .child(decision_chips(props.project, props.eval, selected)),
        )
        .children(reasons.map(|chips| {
            div()
                .layer("reason, optional")
                .flex_row()
                .items_center()
                .gap(16)
                .pad(10, 0)
                .child(label("seal.q.why"))
                .child(chips)
                .child(span(word("dialog.seal.optional")).font_size(14).color(palette::DIM))
        }))
        .children(
            props
                .warning
                .is_some()
                .then(|| div().pad(10, 0).font_size(14).color(palette::DIM).text(records.clone())),
        )
        .child(gap(28))
        .child(
            div()
                .layer("seal acts")
                .self_stretch()
                .flex_row()
                .items_center()
                .gap(16)
                .child(
                    span(word("seal.q.back"))
                        .layer("btn Back")
                        .on_click(BarAct::SealBack)
                        .font_size(14)
                        .color(palette::MUTED),
                )
                .child(div().flex_1())
                .child(match props.warning {
                    Some(w) => crate::bottom_bar::warned(w),
                    None => span(records).font_size(14).color(palette::DIM),
                })
                .children(seal),
        )
}

/// The decision, each in its own colour: the one picked fills with it, the
/// others are outlined with a dot of theirs.
pub fn decision_chips(project: &Path, eval: &str, selected: &str) -> Element {
    let chip = |(i, d): (usize, &&str)| {
        let on = selected == *d;
        let (fill, _, _) = tone(decision_tone(d));
        let chip = div()
            .layer(format!("disposition {d}"))
            .on_click(BarAct::PickDisposition {
                project: project.to_path_buf(),
                eval: eval.to_string(),
                disposition: d.to_string(),
            })
            .aria_selected(on)
            .flex_row()
            .items_center()
            .gap(8)
            .h(34)
            .pad(0, 14)
            .bg(if on { fill } else { palette::SURFACE })
            .font_size(14)
            .font_weight(if on { 600 } else { 400 })
            .color(if on { palette::CANVAS } else { palette::MUTED })
            .child(dot(if on { palette::CANVAS } else { fill }, 8))
            .child(span(weft_core::tree::decision(d)));
        if i == 0 { chip } else { chip.border_left(palette::LINE) }
    };
    div()
        .layer("decisions")
        .flex_row()
        .border(palette::LINE)
        .style("overflow", "hidden")
        .children(weft_core::tree::DISPOSITIONS.iter().enumerate().map(chip))
}

/// The optional reasons of a decision that is not a yes: the one picked in
/// its decision's colour, the others plain.
pub fn reason_chips(
    project: &Path,
    eval: &str,
    selected: &str,
    picked: Option<&str>,
) -> Option<Element> {
    let reasons = weft_core::tree::reasons(selected);
    if reasons.is_empty() {
        return None;
    }
    let (_, text, ground) = tone(decision_tone(selected));
    let chip = |(i, r): (usize, &&str)| {
        let on = picked == Some(*r);
        let chip = div()
            .layer(format!("reason {r}"))
            .on_click(BarAct::PickReason {
                project: project.to_path_buf(),
                eval: eval.to_string(),
                disposition: selected.to_string(),
                reason: (!on).then(|| r.to_string()),
            })
            .aria_selected(on)
            .h(34)
            .flex_row()
            .items_center()
            .pad(0, 14)
            .bg(if on { ground } else { palette::SURFACE })
            .font_size(14)
            .font_weight(if on { 600 } else { 400 })
            .color(if on { text } else { palette::MUTED })
            .text(word(&format!("seal.r.{r}")).to_string());
        if i == 0 { chip } else { chip.border_left(palette::LINE) }
    };
    Some(
        div()
            .layer("reasons")
            .flex_row()
            .border(palette::LINE)
            .style("overflow", "hidden")
            .children(reasons.iter().enumerate().map(chip)),
    )
}
