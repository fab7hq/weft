//! Bottom `TreeBar` (`Ask`, `Eval`, `Seal`, `Keys` `D3.11`, why-not sentences
//! `D10.1`/`D10.2`) and `PendingBar` (`D4.5`–`D4.14` including `D4.11`
//! `Cancel / Send anyway`, `D5.x` Seal dispositions, `D8.3` `Allow a device bar`,
//! and `D9.9` `Quit Weft bar`).

use weft_core::turns::Turn;
use weft_core::words;

use crate::app::BottomBarState;
use crate::daemon::{ProjectClient, Staged};
use crate::gpui::{Element, div, palette};
use crate::tree_view::TreeSelection;

pub struct BottomBarProps<'a> {
    pub machine: &'a str,
    pub state: &'a BottomBarState,
    pub clients: &'a [ProjectClient],
    pub selection: Option<&'a TreeSelection>,
}

/// Render the bottom bar (`PendingBar` when active, or `TreeBar` otherwise).
pub fn render_bottom_bar(props: &BottomBarProps) -> Element {
    // Check if any project client has a staged pending item (`D4.5`–`D4.11`)
    if let Some((client, staged)) =
        props.clients.iter().find_map(|c| c.waiting.last().map(|s| (c, s)))
    {
        return render_staged_bar(client, staged);
    }

    match props.state {
        BottomBarState::QuitConfirm => render_quit_bar(),
        BottomBarState::AllowDevice { device_id, device_name } => {
            render_allow_device_bar(props.machine, device_id, device_name)
        }
        BottomBarState::RemoveDeviceConfirm { device_id, device_name } => {
            render_remove_device_bar(device_id, device_name)
        }
        BottomBarState::SealDisposition { project, eval, selected } => {
            render_seal_disposition_bar(project, eval, selected)
        }
        BottomBarState::WhyNot { sentence } => render_tree_bar(props, Some(sentence.as_str())),
        BottomBarState::None => render_tree_bar(props, None),
    }
}

fn render_staged_bar(client: &ProjectClient, staged: &Staged) -> Element {
    let pane_view = client.view(staged.pane);
    let harness = pane_view.map(|p| p.harness.as_str()).unwrap_or("agent");
    let turn = pane_view.and_then(|p| p.turn);

    // D4.11: When the agent has not said it is ready (Working / Waiting / TurnEnded / refusal),
    // show `The agent has not said it is ready` and `btn Send anyway`.
    let not_ready_reason = if let Some(refusal) = &client.last_refusal {
        Some(refusal.clone())
    } else {
        match turn {
            Some(Turn::Working) => {
                Some(words::fill("not_typed.why", &[("why", words::word("not_ready.working"))]))
            }
            Some(Turn::Waiting) => {
                Some(words::fill("not_typed.why", &[("why", words::word("not_ready.asking"))]))
            }
            None | Some(Turn::Ended) => {
                Some(words::fill("not_typed.why", &[("why", words::word("not_ready.unknown"))]))
            }
            Some(Turn::Ready | Turn::TurnEnded) => None,
        }
    };

    if let Some(reason) = not_ready_reason {
        return div()
            .layer("Cancel / Send bar")
            .self_stretch()
            .flex_col()
            .gap(8)
            .pad(14, 20)
            .items_start()
            .bg(palette::SURFACE_ALT)
            .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
            .child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(8)
                    .items_center()
                    .child(
                        div().font_weight(600).font_size(15).text(words::word("not_ready.title")),
                    )
                    .child(div().flex_1()),
            )
            .child(div().self_stretch().font_size(13).color(palette::MUTED).text(reason))
            .child(
                div()
                    .self_stretch()
                    .font_size(13)
                    .color(palette::MUTED)
                    .text(words::word("not_ready.why")),
            )
            .child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .items_center()
                    .child(div().flex_1())
                    .child(
                        div()
                            .layer("btn Cancel")
                            .on_click(format!("resolve_staged:{}:{}:no", client.name, staged.id))
                            .pad(9, 18)
                            .bg(palette::SURFACE)
                            .rounded(10)
                            .border(palette::LINE)
                            .font_weight(600)
                            .font_size(14)
                            .text(words::word("act.cancel")),
                    )
                    .child(
                        div()
                            .layer("btn Send anyway")
                            .on_click(format!("resolve_staged:{}:{}:force", client.name, staged.id))
                            .pad(9, 18)
                            .bg(palette::BLUE)
                            .rounded(10)
                            .font_weight(600)
                            .font_size(14)
                            .color("#ffffff")
                            .text(words::word("act.send_anyway")),
                    ),
            );
    }

    let summary = words::fill("bar.will_type", &[("command", &staged.what), ("harness", harness)]);

    div()
        .layer("Cancel / Send bar")
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .child(div().font_weight(600).font_size(15).text(staged.what.clone()))
                .child(div().flex_1()),
        )
        .child(div().self_stretch().font_size(13).color(palette::MUTED).text(summary))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Cancel")
                        .on_click(format!("resolve_staged:{}:{}:no", client.name, staged.id))
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.cancel")),
                )
                .child(
                    div()
                        .layer("btn Send")
                        .on_click(format!("resolve_staged:{}:{}:yes", client.name, staged.id))
                        .pad(9, 18)
                        .bg(palette::BLUE)
                        .rounded(10)
                        .font_weight(600)
                        .font_size(14)
                        .color("#ffffff")
                        .text(words::word("act.send")),
                ),
        )
}

fn render_seal_disposition_bar(project: &str, eval: &str, selected: &str) -> Element {
    let chip = |disp: &str, label: &str| {
        let is_sel = selected == disp;
        div()
            .layer(format!("disposition {disp}"))
            .on_click(format!("select_seal_disposition:{project}:{eval}:{disp}"))
            .pad(7, 14)
            .bg(if is_sel { palette::SELECTED_BG } else { palette::SURFACE })
            .rounded(8)
            .border(if is_sel { palette::BLUE_BORDER } else { palette::LINE })
            .font_weight(600)
            .font_size(13)
            .color(if is_sel { palette::BLUE_BORDER } else { palette::TEXT })
            .text(label)
    };

    div()
        .layer("Seal disposition bar")
        .self_stretch()
        .flex_col()
        .gap(10)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .child(div().font_weight(600).font_size(15).text("Seal this Eval round"))
                .child(div().flex_1()),
        )
        .child(
            div()
                .flex_row()
                .gap(8)
                .items_center()
                .child(chip("accepted", "accepted"))
                .child(chip("deferred", "parked"))
                .child(chip("rejected", "rejected"))
                .child(chip("abandoned", "dropped")),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Cancel")
                        .on_click("cancel_bottom_bar")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.cancel")),
                )
                .child(
                    div()
                        .layer("btn Seal")
                        .on_click(format!("confirm_seal_disposition:{project}:{eval}:{selected}"))
                        .pad(9, 18)
                        .bg(palette::BLUE)
                        .rounded(10)
                        .font_weight(600)
                        .font_size(14)
                        .color("#ffffff")
                        .text(words::word("act.seal")),
                ),
        )
}

fn render_quit_bar() -> Element {
    div()
        .layer("Quit Weft bar")
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .child(div().font_weight(600).font_size(15).text("Quit Weft?"))
                .child(div().flex_1()),
        )
        .child(
            div()
                .self_stretch()
                .font_size(13)
                .color(palette::MUTED)
                .text("Your agents are still running. You can leave them running in the background, or stop them now."),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Cancel")
                        .on_click("cancel_bottom_bar")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.cancel")),
                )
                .child(
                    div()
                        .layer("btn Stop agents and quit")
                        .on_click("quit_stop_agents")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::AMBER)
                        .text("Stop agents and quit"),
                )
                .child(
                    div()
                        .layer("btn Leave running and quit")
                        .on_click("quit_leave_running")
                        .pad(9, 18)
                        .bg(palette::BLUE)
                        .rounded(10)
                        .font_weight(600)
                        .font_size(14)
                        .color("#ffffff")
                        .text("Leave running and quit"),
                ),
        )
}

fn render_allow_device_bar(machine: &str, device_id: &str, device_name: &str) -> Element {
    div()
        .layer("Allow a device bar")
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .child(
                    div()
                        .font_weight(600)
                        .font_size(15)
                        .text(format!("Allow \"{device_name}\" to connect to {machine}?")),
                )
                .child(div().flex_1()),
        )
        .child(
            div()
                .self_stretch()
                .font_size(13)
                .color(palette::MUTED)
                .text("Signed in as 0xnairb. Only allow a device you are holding."),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Refuse")
                        .on_click(format!("remote_allow_device:{device_id}:no"))
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text("Refuse"),
                )
                .child(
                    div()
                        .layer("btn Allow")
                        .on_click(format!("remote_allow_device:{device_id}:yes"))
                        .pad(9, 18)
                        .bg(palette::BLUE)
                        .rounded(10)
                        .font_weight(600)
                        .font_size(14)
                        .color("#ffffff")
                        .text("Allow"),
                ),
        )
}

fn render_remove_device_bar(device_id: &str, device_name: &str) -> Element {
    div()
        .layer("Remove device bar")
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::AMBER))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(8)
                .items_center()
                .child(
                    div().font_weight(600).font_size(15).text(format!("Remove \"{device_name}\"?")),
                )
                .child(div().flex_1()),
        )
        .child(
            div()
                .self_stretch()
                .font_size(13)
                .color(palette::MUTED)
                .text(words::word("web.machines.remove_ask")),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(
                    div()
                        .layer("btn Cancel")
                        .on_click("cancel_bottom_bar")
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::LINE)
                        .font_weight(600)
                        .font_size(14)
                        .text(words::word("act.cancel")),
                )
                .child(
                    div()
                        .layer("btn Confirm Remove")
                        .on_click(format!("remote_remove_device:{device_id}"))
                        .pad(9, 18)
                        .bg(palette::SURFACE)
                        .rounded(10)
                        .border(palette::AMBER)
                        .font_weight(600)
                        .font_size(14)
                        .color(palette::AMBER)
                        .text(words::word("web.machines.remove")),
                ),
        )
}

fn render_tree_bar(props: &BottomBarProps, why_not: Option<&str>) -> Element {
    let btn = |layer: &str, label: &str, action: &str| {
        div()
            .layer(layer)
            .on_click(action)
            .flex_row()
            .gap(6)
            .pad(9, 18)
            .items_center()
            .justify_center()
            .bg(palette::SURFACE)
            .rounded(10)
            .border(palette::LINE)
            .child(div().font_weight(600).font_size(14).text(label))
    };

    let selected_label = match props.selection {
        Some(TreeSelection::Agent { session, .. }) => {
            words::fill("web.bar.selected", &[("what", session)])
        }
        Some(TreeSelection::OpenGroup { .. }) => {
            words::fill("web.bar.selected", &[("what", words::word("web.bar.open_round"))])
        }
        Some(TreeSelection::Ask { unit, .. }) => words::fill("web.bar.selected", &[("what", unit)]),
        Some(TreeSelection::EvalGroup { eval, .. }) => {
            words::fill("web.bar.selected", &[("what", eval)])
        }
        Some(TreeSelection::Seal { seal, .. }) => {
            words::fill("web.bar.selected", &[("what", seal)])
        }
        _ => String::new(),
    };

    let mut row = div()
        .layer("tree bar")
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(10, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(btn("btn Ask", words::word("act.ask"), "bar_act_ask"))
        .child(btn("btn Eval", words::word("act.eval"), "bar_act_eval"))
        .child(btn("btn Seal", words::word("act.seal"), "bar_act_seal"));

    if let Some(sentence) = why_not {
        row = row.child(
            div().layer("why not sentence").font_size(13).color(palette::AMBER).text(sentence),
        );
    }

    row = row.child(div().flex_1());

    if !selected_label.is_empty() {
        row = row.child(div().font_size(12).color(palette::DIM).text(selected_label));
    }

    row
}
