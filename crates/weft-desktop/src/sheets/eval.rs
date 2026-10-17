//! An Eval's Details, and the Eval view of its record.

use std::path::Path;

use weft_core::record::Record;
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, mark_colour, note, row_list, section_label, sheet,
    sheet_body, sheet_foot, sheet_head, verdict_colour,
};

/// The Eval's record: from the board's records, or as read through the
/// daemon. None until one of them has it.
fn judges(props: &SheetProps, project: &Path, eval: &str) -> Option<Record> {
    let c = props.client(project)?;
    let held = c.records.get(eval).map(|v| v.get("record").unwrap_or(v));
    let read = c.read.get(&Then::Judges { eval: eval.to_string() });
    held.into_iter().chain(read).find_map(Record::parse)
}

/// Marked lines in a box: ✓, ✗ or ? and what they say.
fn marked(lines: &[weft_core::offers::Marked], mono: bool) -> Element {
    row_list(
        lines
            .iter()
            .map(|m| {
                let text =
                    div().flex_1().font_size(if mono { 12 } else { 13 }).text(m.text.clone());
                let mut r = list_row()
                    .child(
                        div()
                            .font_weight(700)
                            .font_size(13)
                            .color(mark_colour(m.mark))
                            .text(m.mark),
                    )
                    .child(if mono { text.font_mono() } else { text });
                if let Some(res) = &m.result {
                    r = r.child(div().font_size(12).color(palette::MUTED).text(res.clone()));
                }
                r
            })
            .collect(),
    )
}

/// Asks in a box, each opening its Details.
pub(super) fn ask_rows(project: &Path, asks: &[weft_core::tree::Ask]) -> Element {
    row_list(
        asks.iter()
            .map(|a| {
                list_row()
                    .on_click(TreeAct::SelectAsk {
                        project: project.to_path_buf(),
                        unit: a.unit.clone(),
                    })
                    .child(div().flex_1().font_size(13).text(a.title.clone()))
                    .child(div().font_size(12).color(palette::MUTED).text(a.harness.clone()))
                    .child(div().font_size(12).color(palette::DIM).text("›"))
            })
            .collect(),
    )
}

pub(super) fn render_detail(props: &SheetProps, project: &Path, eval_id: &str) -> Element {
    let client = props.client(project);
    let group =
        props.listed(project).and_then(|l| l.project.work.evals.iter().find(|g| g.eval == eval_id));
    let record = judges(props, project, eval_id);
    let detail = client.and_then(|c| {
        let ev = c.rounds.record.evals.iter().find(|e| e.id == eval_id)?;
        Some(weft_core::offers::eval_detail(
            ev,
            record.as_ref(),
            &c.units,
            &c.rounds.record,
            group.and_then(|g| g.seal.clone()),
        ))
    });
    let n = detail.as_ref().map(|d| d.n).or_else(|| group.map(|g| g.n));
    let title = match n {
        Some(n) => fill("tree.eval", &[("n", &n.to_string())]),
        None => eval_id.to_string(),
    };
    let judging = detail.as_ref().map(|d| d.judging).or_else(|| group.map(|g| g.judging));
    let verdict = detail.as_ref().and_then(|d| d.verdict).or_else(|| group.and_then(|g| g.verdict));
    let agreement = detail
        .as_ref()
        .and_then(|d| d.agreement.clone())
        .or_else(|| group.and_then(|g| g.agreement.clone()));
    let aside = match (judging, verdict) {
        (Some(true), _) => Some((word("tree.judging").to_string(), palette::AMBER)),
        (_, Some(v)) => Some((
            agreement.map_or_else(|| v.to_string(), |a| format!("{v} {a}")),
            verdict_colour(v),
        )),
        _ => None,
    };

    let mut body = sheet_body();
    match &detail {
        None => body = body.child(note(word("desktop.eval.gone"), palette::MUTED)),
        Some(d) => {
            if let Some(by) = &d.judged_by {
                let when = d.at.and_then(|t| client.map(|c| weft_core::offers::stamp(t, c.zone)));
                let said = match when {
                    Some(ts) => format!("{} {by} · {ts}", word("web.eval.judged_by")),
                    None => format!("{} {by}", word("web.eval.judged_by")),
                };
                body = body.child(note(said, palette::MUTED));
            }
            body = body.child(match &d.sealed {
                Some(sl) => div()
                    .on_click(TreeAct::SelectSeal {
                        project: project.to_path_buf(),
                        seal: sl.seal.clone(),
                    })
                    .font_size(13)
                    .color(palette::TEAL)
                    .text(format!("{} ›", fill("web.eval.sealed_in", &[("n", &sl.n.to_string())]))),
                None => note(
                    fill("web.eval.seal_what", &[("n", &d.asks.len().to_string())]),
                    palette::MUTED,
                ),
            });
            if !d.findings.is_empty() {
                body = body
                    .child(section_label(word("web.eval.findings")))
                    .child(marked(&d.findings, false));
            }
            if !d.checks.is_empty() {
                body = body
                    .child(section_label(word("eval_view.checks")))
                    .child(marked(&d.checks, true));
            }
            if !d.asks.is_empty() {
                body = body
                    .child(section_label(word("web.eval.asks")))
                    .child(ask_rows(project, &d.asks));
            }
        }
    }

    let ids = || (project.to_path_buf(), eval_id.to_string());
    let left = vec![
        button(
            "btn Seal",
            word("act.seal"),
            SheetAct::Seal { project: ids().0, eval: ids().1 },
            Tone::Primary,
            Size::Sheet,
        ),
        button(
            "btn Eval view",
            word("act.eval_view"),
            SheetAct::EvalView { project: ids().0, eval: ids().1 },
            Tone::Plain,
            Size::Sheet,
        ),
    ];
    sheet(
        "sheet EvalDetail",
        sheet_head(title, aside),
        body,
        sheet_foot(left, vec![close_button()]),
    )
}

pub(super) fn render_view(
    props: &SheetProps,
    project: &Path,
    eval_id: &str,
    by_file: bool,
) -> Element {
    let record = judges(props, project, eval_id);
    let mut list = div().self_stretch().flex_1().flex_col().style("overflow-y", "auto");
    match &record {
        None => {
            list = list.child(
                div().pad(11, 20).child(note(word("desktop.eval.no_record"), palette::MUTED)),
            )
        }
        Some(r) => {
            let heading = |key: &str, n: usize| {
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(11, 20)
                    .items_center()
                    .border_bottom(palette::LINE)
                    .child(div().flex_1().font_size(15).text(word(key)))
                    .child(div().font_size(12).color(palette::MUTED).text(n.to_string()))
            };
            let line =
                || div().self_stretch().flex_row().gap(10).pad(10, 20).border_bottom(palette::LINE);
            list = list.child(heading("eval_view.checks", r.checks.len()));
            for c in &r.checks {
                let ok = matches!(c.outcome.as_str(), "succeeded" | "passed");
                let mark = if ok { "✓" } else { "✗" };
                list = list.child(
                    line()
                        .items_center()
                        .child(
                            div()
                                .font_weight(700)
                                .font_size(13)
                                .color(mark_colour(mark))
                                .text(mark),
                        )
                        .child(div().flex_1().font_mono().font_size(12).text(c.command.clone()))
                        .child(div().font_size(12).color(palette::MUTED).text(c.outcome.clone())),
                );
            }
            list = list.child(heading("eval_view.limitations", r.items.len()));
            for item in &r.items {
                let mark = weft_core::offers::mark_for(item.plain_majority());
                list = list.child(
                    line()
                        .items_start()
                        .child(
                            div()
                                .font_weight(700)
                                .font_size(13)
                                .color(mark_colour(mark))
                                .text(mark),
                        )
                        .child(div().flex_1().font_size(13).text(item.text.clone())),
                );
            }
        }
    }

    let head = div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(12, 20)
        .items_center()
        .border_bottom(palette::LINE)
        .child(
            div()
                .on_click(TreeAct::SelectEval {
                    project: project.to_path_buf(),
                    eval: eval_id.to_string(),
                })
                .font_size(13)
                .color(palette::BLUE_BORDER)
                .text(format!("‹ {}", word("web.detail.title"))),
        )
        .child(div().flex_1().font_weight(600).font_size(16).text(word("act.eval_view")))
        .child(
            div()
                .layer("toggle eval view mode")
                .on_click(SheetAct::ToggleEvalViewMode)
                .pad(4, 10)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_size(12)
                .text(word(if by_file { "eval_view.by_file" } else { "eval_view.by_step" })),
        );
    sheet(
        "sheet EvalView",
        head,
        list,
        sheet_foot(vec![note(word("web.eval.click"), palette::DIM).flex_1()], vec![close_button()]),
    )
}
