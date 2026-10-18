//! The bar along the window's foot: Ask, Eval and Seal with why one cannot
//! be done; or what waits on the person: a staged prompt, a Seal's
//! disposition, a device asking to connect, a device to remove, quitting.

use weft_core::turns;
use weft_core::words::{fill, word};

use crate::action::{Answer, BarAct, SheetAct};
use crate::app::BottomBarState;
use crate::daemon::{ProjectClient, Staged};
use crate::gpui::{Element, div, palette, span};
use crate::tree_view::TreeSelection;
use crate::ui::{Size, Tone, asking_bar, bar_acts, button, note};

pub struct BottomBarProps<'a> {
    pub machine: &'a str,
    pub state: &'a BottomBarState,
    pub clients: &'a [ProjectClient],
    pub selection: Option<&'a TreeSelection>,
    /// The act put forward once the agent in front's turn has ended: outlined, never pressed.
    pub suggested: Option<weft_core::board::Act>,
}

/// The bar: what waits on the person, or the tree's acts.
pub fn render_bottom_bar(props: &BottomBarProps) -> Element {
    // A device asking to be allowed is answered first, over anything.
    if let BottomBarState::AllowDevice { device_id, device_name, login } = props.state {
        return render_allow_device_bar(props.machine, device_id, device_name, login.as_deref());
    }
    if let Some((client, staged)) =
        props.clients.iter().find_map(|c| c.waiting.last().map(|s| (c, s)))
    {
        return render_staged_bar(client, staged);
    }
    match props.state {
        BottomBarState::QuitConfirm => render_quit_bar(),
        BottomBarState::AllowDevice { device_id, device_name, login } => {
            render_allow_device_bar(props.machine, device_id, device_name, login.as_deref())
        }
        BottomBarState::RemoveDeviceConfirm { device_id, device_name } => {
            render_remove_device_bar(device_id, device_name)
        }
        BottomBarState::CloseProjectConfirm { project } => render_close_project_bar(project),
        BottomBarState::SealDisposition { project, eval, selected } => {
            render_seal_disposition_bar(project, eval, selected)
        }
        BottomBarState::WhyNot { sentence } => render_tree_bar(props, Some(sentence.as_str())),
        BottomBarState::None => render_tree_bar(props, None),
    }
}

fn title(text: impl Into<String>) -> Element {
    div().font_weight(600).font_size(15).text(text)
}

fn render_staged_bar(client: &ProjectClient, staged: &Staged) -> Element {
    let pane = client.view(staged.pane);
    let resolve = |answer| BarAct::Resolve {
        project: client.root.clone(),
        pending: staged.id.clone(),
        answer,
    };
    let cancel =
        button("btn Cancel", word("act.cancel"), resolve(Answer::No), Tone::Plain, Size::Sheet);

    // The agent has not said it is ready: Weft did not type it, and why.
    let not_ready = client.last_refusal.clone().or_else(|| {
        turns::may_type(pane.and_then(|p| p.turn))
            .err()
            .map(|why| fill("not_typed.why", &[("why", why)]))
    });
    if let Some(reason) = not_ready {
        return asking_bar("Cancel / Send bar", palette::BLUE_BORDER)
            .child(title(word("not_ready.title")))
            .child(note(reason, palette::MUTED))
            .child(note(word("not_ready.why"), palette::MUTED))
            .child(bar_acts(vec![
                cancel,
                button(
                    "btn Send anyway",
                    word("act.send_anyway"),
                    resolve(Answer::Anyway),
                    Tone::Primary,
                    Size::Sheet,
                ),
            ]));
    }
    let summary = match pane {
        Some(p) => fill("bar.will_type", &[("command", &staged.what), ("harness", &p.harness)]),
        None => fill("desktop.bar.will_type", &[("command", &staged.what)]),
    };
    asking_bar("Cancel / Send bar", palette::BLUE_BORDER)
        .child(title(staged.what.clone()))
        .child(note(summary, palette::MUTED))
        .child(bar_acts(vec![
            cancel,
            button("btn Send", word("act.send"), resolve(Answer::Yes), Tone::Primary, Size::Sheet),
        ]))
}

fn render_seal_disposition_bar(project: &std::path::Path, eval: &str, selected: &str) -> Element {
    let chip = |disposition: &str| {
        let picked = selected == disposition;
        div()
            .layer(format!("disposition {disposition}"))
            .on_click(BarAct::PickDisposition {
                project: project.to_path_buf(),
                eval: eval.to_string(),
                disposition: disposition.to_string(),
            })
            .pad(6, 12)
            .bg(if picked { palette::SELECTED_BG } else { palette::SURFACE })
            .border(if picked { palette::BLUE_BORDER } else { palette::LINE })
            .font_weight(if picked { 600 } else { 400 })
            .font_size(12)
            .color(if picked { palette::BLUE_BORDER } else { palette::MUTED })
            .text(weft_core::tree::decision(disposition))
    };
    div()
        .layer("Seal disposition bar")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(12, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::AMBER_BORDER))
        .child(span("■").font_mono().color(palette::AMBER))
        .child(div().font_weight(600).font_size(14).text(word("desktop.bar.seal_round")))
        .child(div().flex_1())
        .children(["accepted", "deferred", "rejected", "abandoned"].map(chip))
        .child(div().w(12))
        .child(button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar))
        .child(button(
            "btn Seal",
            word("act.seal"),
            BarAct::ConfirmSeal {
                project: project.to_path_buf(),
                eval: eval.to_string(),
                disposition: selected.to_string(),
            },
            Tone::Primary,
            Size::Bar,
        ))
}

fn render_quit_bar() -> Element {
    div()
        .layer("Quit Weft bar")
        .self_stretch()
        .flex_row()
        .gap(12)
        .pad(12, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .child(
            div()
                .flex_1()
                .flex_col()
                .gap(2)
                .child(div().font_weight(600).font_size(14).text(word("desktop.quit.title")))
                .child(div().font_size(12).color(palette::MUTED).text(word("desktop.quit.what"))),
        )
        .child(button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar))
        .child(button(
            "btn Stop agents and quit",
            word("desktop.quit.stop"),
            BarAct::StopAgentsAndQuit,
            Tone::Warn,
            Size::Bar,
        ))
        .child(button(
            "btn Leave running and quit",
            word("desktop.quit.leave"),
            BarAct::LeaveRunningAndQuit,
            Tone::Primary,
            Size::Bar,
        ))
}

fn render_allow_device_bar(
    machine: &str,
    device_id: &str,
    device_name: &str,
    login: Option<&str>,
) -> Element {
    let answer = |yes| BarAct::AllowDevice { device: device_id.to_string(), yes };
    let mut bar = asking_bar("Allow a device bar", palette::BLUE_BORDER)
        .child(title(fill("desktop.allow.ask", &[("device", device_name), ("machine", machine)])));
    if let Some(login) = login {
        bar = bar.child(note(fill("web.add.signed_in", &[("login", login)]), palette::MUTED));
    }
    bar.child(note(word("desktop.allow.holding"), palette::MUTED)).child(bar_acts(vec![
        button("btn Refuse", word("desktop.allow.refuse"), answer(false), Tone::Plain, Size::Sheet),
        button("btn Allow", word("desktop.allow.allow"), answer(true), Tone::Primary, Size::Sheet),
    ]))
}

fn render_close_project_bar(project: &std::path::Path) -> Element {
    let name = ProjectClient::name_of(project);
    asking_bar("Close project bar", palette::BLUE_BORDER)
        .child(title(fill("desktop.close.ask", &[("project", &name)])))
        .child(note(word("desktop.close.what"), palette::MUTED))
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Plain, Size::Sheet),
            button(
                "btn Close it",
                word("desktop.close.yes"),
                BarAct::ConfirmCloseProject(project.to_path_buf()),
                Tone::Primary,
                Size::Sheet,
            ),
        ]))
}

fn render_remove_device_bar(device_id: &str, device_name: &str) -> Element {
    asking_bar("Remove device bar", palette::AMBER)
        .child(title(fill("desktop.remote.remove_sure", &[("device", device_name)])))
        .child(note(word("desktop.remote.remove_what"), palette::MUTED))
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Plain, Size::Sheet),
            button(
                "btn Confirm Remove",
                word("web.machines.remove"),
                SheetAct::RemoveDevice(device_id.to_string()),
                Tone::Warn,
                Size::Sheet,
            ),
        ]))
}

fn render_tree_bar(props: &BottomBarProps, why_not: Option<&str>) -> Element {
    use weft_core::board::Act;
    // Eval put forward in teal, Seal in amber, as the guideline's outlined buttons.
    let btn = |layer: &str, label: &str, action: BarAct, act: Act| {
        let accent = match act {
            Act::Eval if props.suggested == Some(Act::Eval) => Some(palette::TEAL),
            Act::Seal if props.suggested == Some(Act::Seal) => Some(palette::AMBER),
            _ => None,
        };
        div()
            .layer(layer)
            .on_click(action)
            .flex_row()
            .gap(6)
            .pad(7, 18)
            .items_center()
            .justify_center()
            .bg(palette::SURFACE)
            .border(accent.unwrap_or(palette::LINE))
            .child(
                div()
                    .font_weight(600)
                    .font_size(13)
                    .color(accent.unwrap_or(palette::TEXT))
                    .text(label),
            )
    };
    let selected = match props.selection {
        Some(TreeSelection::Agent { session, .. }) => Some(session.as_str()),
        Some(TreeSelection::OpenGroup { .. }) => Some(word("web.bar.open_round")),
        Some(TreeSelection::Ask { unit, .. }) => Some(unit.as_str()),
        Some(TreeSelection::EvalGroup { eval, .. }) => Some(eval.as_str()),
        Some(TreeSelection::Seal { seal, .. }) => Some(seal.as_str()),
        _ => None,
    };
    let mut row = div()
        .layer("tree bar")
        .self_stretch()
        // Room from the window's rounded bottom corners.
        .h(56)
        .shrink_0()
        .flex_row()
        .gap(10)
        .pad(0, 20)
        .items_center()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE)
        .child(btn("btn Ask", word("act.ask"), BarAct::Ask, Act::Ask))
        .child(btn("btn Eval", word("act.eval"), BarAct::Eval, Act::Eval))
        .child(btn("btn Seal", word("act.seal"), BarAct::Seal, Act::Seal));
    if let Some(sentence) = why_not {
        row = row.child(
            div().layer("why not sentence").font_size(13).color(palette::AMBER).text(sentence),
        );
    }
    row = row.child(div().flex_1());
    if let Some(what) = selected {
        row = row.child(
            div()
                .font_size(12)
                .color(palette::DIM)
                .text(fill("web.bar.selected", &[("what", what)])),
        );
    }
    row
}
