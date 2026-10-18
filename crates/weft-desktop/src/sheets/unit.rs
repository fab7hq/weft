//! A unit of work's Details: its workflow in order, each step opening its
//! own Details, and the unit's acts.

use std::path::Path;

use weft_core::offers::Step;
use weft_core::words::{fill, word};
use weft_core::work::Status;

use super::SheetProps;
use crate::action::{SheetAct, TreeAct};
use crate::gpui::{Element, div, palette};
use crate::tree_view::status_colour;
use crate::ui::{
    Size, Tone, button, close_button, note, row_list, section_label, sheet, sheet_body, sheet_foot,
    sheet_head,
};

pub(super) fn render(props: &SheetProps, project: &Path, unit: &str) -> Element {
    let client = props.client(project);
    let row = props.listed(project).and_then(|l| l.work_row(unit));
    let detail = client.and_then(|c| {
        let u = c.work.of_ask(unit)?;
        let judges =
            |e: &str| c.records.get(e).and_then(|v| serde_json::from_value(v.clone()).ok());
        let why = row.map_or((None, None), |r| (r.eval.clone(), r.seal.clone()));
        Some(weft_core::offers::unit_detail(u, &c.units, &c.work.record, &judges, why, c.zone))
    });
    let Some(d) = detail else {
        return sheet(
            "sheet Unit",
            sheet_head(unit, None),
            sheet_body().child(note(word("desktop.work.gone"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };
    let colour = status_colour(&d.status);
    let head = div()
        .self_stretch()
        .flex_col()
        .gap(4)
        .style("padding", "16px 20px 12px 20px")
        .border_bottom(palette::LINE)
        .child(
            div()
                .flex_row()
                .gap(10)
                .items_baseline()
                .child(div().flex_1().font_weight(600).font_size(18).text(d.title.clone()))
                .child(
                    div()
                        .font_mono()
                        .font_size(12)
                        .color(colour)
                        .text(weft_core::tree::status_word(&d.status)),
                ),
        )
        .child(div().font_size(12).color(palette::MUTED).text(d.aside.clone()));

    let to = |t: TreeAct| t;
    let at = project.to_path_buf();
    let mut n = 0;
    let steps: Vec<Element> = d
        .steps
        .iter()
        .map(|s| {
            n += 1;
            let (label, text, aside, open): (String, Element, Option<String>, Option<Element>) =
                match s {
                    Step::Ask { unit, title, follow_up, aside } => (
                        word(if *follow_up { "act.follow_up" } else { "act.ask" }).into(),
                        div().font_size(14).text(title.clone()),
                        Some(aside.clone()),
                        Some(chevron(to(TreeAct::SelectAsk {
                            project: at.clone(),
                            unit: unit.clone(),
                        }))),
                    ),
                    Step::Eval { eval, n, verdict, agreement, aside } => {
                        let said = match verdict {
                            Some(v) => div()
                                .flex_row()
                                .gap(4)
                                .child(
                                    div()
                                        .font_size(14)
                                        .color(verdict_colour(v))
                                        .text(word(&format!("verdict.{v}")).to_string()),
                                )
                                .child(div().font_size(14).text(match agreement {
                                    Some(a) => format!(
                                        "· {}",
                                        fill("unit.confidence", &[("agreement", a)])
                                    ),
                                    None => String::new(),
                                })),
                            None => div()
                                .font_size(14)
                                .color(palette::BLUE_BORDER)
                                .text(word("tree.judging")),
                        };
                        (
                            fill("tree.eval", &[("n", &n.to_string())]),
                            said,
                            Some(aside.clone()),
                            Some(
                                div()
                                    .on_click(SheetAct::EvalView {
                                        project: at.clone(),
                                        eval: eval.clone(),
                                    })
                                    .shrink_0()
                                    .font_size(13)
                                    .color(palette::BLUE_BORDER)
                                    .text(format!("{} ›", word("act.eval_view"))),
                            ),
                        )
                    }
                    Step::Seal { seal, decision, aside, .. } => (
                        word("act.seal").into(),
                        div().font_size(14).text(decision.clone()),
                        Some(aside.clone()),
                        Some(chevron(to(TreeAct::SelectSeal {
                            project: at.clone(),
                            seal: seal.clone(),
                        }))),
                    ),
                    Step::NotSealed => (
                        word("act.seal").into(),
                        div().font_size(14).color(palette::DIM).text(word("unit.not_sealed")),
                        None,
                        None,
                    ),
                };
            let mut col = div()
                .flex_1()
                .style("min-width", "0")
                .flex_col()
                .gap(3)
                .child(section_label(label))
                .child(text);
            if let Some(a) = aside {
                col = col.child(div().font_size(12).color(palette::MUTED).text(a));
            }
            let mut step = div()
                .layer(format!("step {n}"))
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(12, 16)
                .items_start()
                .child(
                    div()
                        .w(18)
                        .shrink_0()
                        .font_mono()
                        .font_size(12)
                        .color(palette::DIM)
                        .text(n.to_string()),
                )
                .child(col);
            if let Some(open) = open {
                step = step.child(open);
            }
            step
        })
        .collect();
    let body = sheet_body()
        .gap(4)
        .style("padding", "12px 20px")
        .child(section_label(word("unit.workflow")))
        .child(row_list(steps));

    // The act its status puts forward is the sheet's primary one.
    let primary = match d.status {
        Status::Asking | Status::Asked | Status::Judging => "eval",
        Status::Judged { .. } => "seal",
        Status::Sealed { .. } => "follow_up",
    };
    let tone = |act: &str| if act == primary { Tone::Primary } else { Tone::Plain };
    let ids = || (project.to_path_buf(), d.unit.clone());
    let left = vec![
        button(
            "btn Follow up",
            word("act.follow_up"),
            SheetAct::FollowUp { project: ids().0, unit: ids().1 },
            tone("follow_up"),
            Size::Sheet,
        ),
        button(
            "btn Eval",
            word("act.eval"),
            SheetAct::EvalWork { project: ids().0, unit: ids().1 },
            tone("eval"),
            Size::Sheet,
        ),
        button(
            "btn Seal",
            word("act.seal"),
            SheetAct::SealWork { project: ids().0, unit: ids().1 },
            tone("seal"),
            Size::Sheet,
        ),
    ];
    sheet("sheet Unit", head, body, sheet_foot(left, vec![close_button()]))
}

fn chevron(action: TreeAct) -> Element {
    div().on_click(action).shrink_0().font_size(12).color(palette::DIM).text("›")
}

fn verdict_colour(v: &str) -> &'static str {
    if v == "aligned" { palette::TEAL } else { palette::AMBER }
}
