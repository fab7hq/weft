//! The Ask, docked under the panes: what to ask, and the running agent it
//! goes to; or a follow-up on a work.

use std::path::Path;

use weft_core::sessions::State as SessionState;
use weft_core::words::word;

use super::SheetProps;
use crate::action::{Focus, SheetAct};
use crate::gpui::{Element, div, palette, textarea_el};
use crate::ui::{Size, Tone, button};

pub(super) fn render(
    props: &SheetProps,
    project: &Path,
    target_session: Option<&str>,
    follow_up_unit: Option<&str>,
) -> Element {
    let act = if follow_up_unit.is_some() { word("act.follow_up") } else { word("act.ask") };
    let title = if follow_up_unit.is_some() { word("act.follow_up") } else { word("ask.title") };

    let mut targets = div().flex_row().gap(6).style("flex-wrap", "wrap");
    if let Some(listed) = props.listed(project) {
        let rows = &listed.project.agents.rows;
        for a in rows.iter().filter(|a| a.state == SessionState::Running) {
            let picked = target_session == Some(a.session.as_str())
                || (target_session.is_none()
                    && rows.first().map(|f| f.session.as_str()) == Some(&a.session));
            targets =
                targets.child(
                    div()
                        .layer(format!("send target {}", a.harness))
                        .on_click(SheetAct::AskTarget { session: a.session.clone() })
                        .flex_row()
                        .gap(8)
                        .pad(8, 12)
                        .items_center()
                        .bg(if picked { palette::SELECTED_BG } else { palette::SURFACE })
                        .border(if picked { palette::BLUE_BORDER } else { palette::LINE })
                        .child(div().font_size(14).font_weight(600).text(a.harness.clone()))
                        .child(div().font_size(12).color(palette::MUTED).text(
                            a.title.clone().unwrap_or_else(|| word("tree.no_ask").to_string()),
                        )),
                );
        }
    }

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
                .child(targets)
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
