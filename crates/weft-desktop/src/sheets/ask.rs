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
