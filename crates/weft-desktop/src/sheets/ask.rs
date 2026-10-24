//! The Ask, docked under the panes: what to ask, and where it goes; or a
//! follow-up on a work.

use weft_core::words::word;

use super::{SheetProps, send_to};
use crate::action::{Focus, SheetAct};
use crate::gpui::{Element, div, palette, textarea_el};
use crate::ui::{Size, Tone, button};

pub(super) fn render(props: &SheetProps, follow_up_unit: Option<&str>) -> Element {
    let act = if follow_up_unit.is_some() { word("act.follow_up") } else { word("act.ask") };
    let title = if follow_up_unit.is_some() { word("act.follow_up") } else { word("ask.title") };

    let targets = props.send_to.map(|v| send_to::render(v, |key| SheetAct::AskTo(key).into()));
    let eyebrow = follow_up_unit.and_then(|unit| eyebrow(props, unit)).map(|said| {
        div()
            .layer("eyebrow")
            .font_mono()
            .font_weight(600)
            .font_size(11)
            .color(palette::DIM)
            .text(said)
    });

    div()
        .layer("sheet Ask")
        .self_stretch()
        .shrink_0()
        .flex_col()
        .gap(12)
        .pad(16, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .children(eyebrow)
        .child(div().font_weight(600).font_size(15).text(title))
        .child(
            textarea_el(Focus::Ask)
                .layer("ask input")
                .self_stretch()
                .style("min-height", "78px")
                .pad(10, 12)
                .bg(palette::SURFACE)
                .border(palette::BLUE_BORDER)
                .font_size(14)
                .style("line-height", "20px")
                .color(palette::TEXT)
                .placeholder(word("ask.title"))
                .text(props.ask_draft.to_string()),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .child(
                    div()
                        .font_mono()
                        .font_weight(600)
                        .font_size(11)
                        .color(palette::MUTED)
                        .text(word("ask.send_to")),
                )
                .children(targets)
                .child(div().flex_1())
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    SheetAct::Close,
                    Tone::Quiet,
                    Size::Sheet,
                ))
                .child(button("btn Ask", act, SheetAct::SubmitAsk, Tone::Primary, Size::Sheet)),
        )
}

/// A follow-up's eyebrow: `FOLLOW UP · <the work's title> · <the id it carries>`.
fn eyebrow(props: &SheetProps, unit: &str) -> Option<String> {
    let crate::app::ActiveSheet::Ask { project, .. } = props.sheet else { return None };
    let client = props.clients.iter().find(|c| &c.root == project)?;
    let work = client.work.of_ask(unit)?;
    let title = client.units.iter().find(|u| u.ask_id == work.root).map(|u| u.title.as_str())?;
    let said = format!("{} · {title} · {}", word("act.follow_up"), work.follows);
    Some(said.to_uppercase())
}
