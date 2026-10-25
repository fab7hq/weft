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
    /// The staged send's SEND TO.
    pub staged_to: Option<&'a crate::sheets::send_to::SendTo>,
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
        return render_staged_bar(client, staged, props.staged_to);
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
        BottomBarState::SealDisposition { project, eval, selected, reason } => {
            let client = props.clients.iter().find(|c| &c.root == project);
            render_seal_disposition_bar(client, project, eval, selected, reason.as_deref())
        }
        BottomBarState::GitNoteConfirm { project } => render_git_note_bar(project),
        BottomBarState::Undo { commits, picked, .. } => render_undo_bar(commits, *picked),
        BottomBarState::WhyNot { sentence } => render_tree_bar(props, Some(sentence.as_str())),
        BottomBarState::None => render_tree_bar(props, None),
    }
}

fn title(text: impl Into<String>) -> Element {
    div().font_weight(600).font_size(15).text(text)
}

fn render_staged_bar(
    client: &ProjectClient,
    staged: &Staged,
    send_to: Option<&crate::sheets::send_to::SendTo>,
) -> Element {
    let pane = client.view(staged.pane);
    let resolve = |answer| BarAct::Resolve {
        project: client.root.clone(),
        pending: staged.id.clone(),
        answer,
    };
    let cancel =
        button("btn Cancel", word("act.cancel"), resolve(Answer::No), Tone::Plain, Size::Sheet);

    // It keeps SEND TO: where it was staged, another agent, or a new one.
    let choice = send_to.map(|v| {
        let pick = |to: String| BarAct::SendTo { pending: staged.id.clone(), to }.into();
        div()
            .flex_row()
            .gap(10)
            .items_center()
            .child(
                div()
                    .font_mono()
                    .font_weight(600)
                    .font_size(11)
                    .color(palette::MUTED)
                    .text(word("ask.send_to")),
            )
            .child(crate::sheets::send_to::render(v, pick))
    });
    // The agent has not said it is ready: Weft did not type it, and why. A
    // dialog that types nothing waits on no agent.
    let not_ready = client.last_refusal.clone().or_else(|| {
        turns::may_type(pane.and_then(|p| p.turn))
            .err()
            .filter(|_| !staged.payload.is_empty())
            .map(|why| fill("not_typed.why", &[("why", why)]))
    });
    if let Some(reason) = not_ready {
        return asking_bar("Cancel / Send bar", palette::BLUE_BORDER)
            .child(title(word("not_ready.title")))
            .child(note(reason, palette::MUTED))
            .child(note(word("not_ready.why"), palette::MUTED))
            .children(choice)
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
    // Its title, why, and the exact bytes Send types, when it types any, the
    // route's command marked.
    let typed = String::from_utf8_lossy(&staged.payload).into_owned();
    let command = typed.split_once(' ').map(|(c, _)| c).filter(|c| c.starts_with(['/', '$']));
    let (command, rest) = match command {
        Some(c) => (Some(format!("{c} ")), typed[c.len() + 1..].to_string()),
        None => (None, typed.clone()),
    };
    let into = (!typed.is_empty()).then(|| {
        let name = client.root.file_name().map(|n| n.to_string_lossy().into_owned());
        let target = match (name, pane) {
            (Some(n), Some(p)) => format!("{n} · {}", p.harness),
            (_, Some(p)) => p.harness.clone(),
            _ => String::new(),
        };
        div()
            .font_size(13)
            .color(palette::MUTED)
            .text(fill("dialog.ask.into", &[("target", &target)]))
    });
    let payload = (!typed.is_empty()).then(|| {
        div()
            .layer("payload")
            .self_stretch()
            .pad(10, 12)
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_mono()
            .font_size(12)
            .style("white-space", "pre-wrap")
            .style("max-height", "240px")
            .style("overflow-y", "auto")
            .color(palette::TEXT)
            .children(command.map(|c| span(c).color(palette::BLUE_BORDER)))
            .child(span(rest))
    });
    asking_bar("Cancel / Send bar", palette::BLUE_BORDER)
        .child(
            div().flex_row().gap(8).items_center().child(title(staged.what.clone())).children(into),
        )
        .children(staged.why.lines().map(|l| note(l.to_string(), palette::MUTED)))
        .children(payload)
        .children(choice)
        .child(bar_acts(vec![cancel, yes_button(staged.go.as_ref(), resolve(Answer::Yes))]))
}

/// A dialog's yes: Send, or the button it names, amber when it stops
/// something.
fn yes_button(go: Option<&weft_proto::Go>, yes: BarAct) -> Element {
    let (said, tone) = match go {
        Some(g) => (word(&g.word), if g.warn { Tone::Warn } else { Tone::Primary }),
        None => (word("act.send"), Tone::Primary),
    };
    button(format!("btn {said}"), said, yes, tone, Size::Sheet)
}

/// The Seal's one dialog (17.4, 17.5): the work it seals and the verdict it
/// rests on, the decision, and for anything but a yes an optional reason.
/// Turning a project's Git note on (17.19): what a note holds, and Turn on.
fn render_git_note_bar(project: &std::path::Path) -> Element {
    let name = project.file_name().unwrap_or_default().to_string_lossy().into_owned();
    asking_bar("Git note bar", palette::BLUE_BORDER)
        .child(title(word("dialog.export.title")))
        .child(note(fill("dialog.export.why", &[("project", &name)]), palette::MUTED))
        .child(
            div()
                .layer("sample")
                .self_stretch()
                .pad(10, 12)
                .bg(palette::SURFACE)
                .border(palette::LINE)
                .font_mono()
                .font_size(12)
                .style("white-space", "pre-wrap")
                .text(word("dialog.export.sample")),
        )
        .child(bar_acts(vec![
            button("btn Cancel", word("act.cancel"), BarAct::Cancel, Tone::Quiet, Size::Bar),
            button(
                "btn Turn on",
                word("dialog.export.go"),
                BarAct::TurnOnGitNote,
                Tone::Primary,
                Size::Bar,
            ),
        ]))
}

/// Undo's one dialog: the commits from where the work started up to now,
/// newest first, in a list that scrolls and keeps the one picked in view.
fn render_undo_bar(commits: &[weft_core::after_seal::Commit], picked: usize) -> Element {
    let row = |(at, c): (usize, &weft_core::after_seal::Commit)| {
        let on = at == picked;
        div()
            .layer(format!("commit {}", c.short))
            .on_click(BarAct::PickCommit(at))
            .flex_row()
            .items_center()
            .gap(12)
            .shrink_0()
            .h(34)
            .pad(0, 14)
            .bg(if on { palette::SELECTED_BG } else { palette::SURFACE })
            .border_bottom(palette::LINE)
            .child(
                div()
                    .w(12)
                    .h(12)
                    .shrink_0()
                    .style("border-radius", "6px")
                    .bg(if on { palette::BLUE_BORDER } else { palette::SURFACE })
                    .border(if on { palette::BLUE_BORDER } else { palette::DIM }),
            )
            .child(
                div()
                    .w(62)
                    .shrink_0()
                    .font_mono()
                    .font_size(12)
                    .color(palette::DIM)
                    .text(c.short.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .style("min-width", "0px")
                    .style("white-space", "nowrap")
                    .style("overflow", "hidden")
                    .style("text-overflow", "ellipsis")
                    .font_size(13)
                    .font_weight(if on { 600 } else { 400 })
                    .color(if on { palette::TEXT } else { palette::MUTED })
                    .text(c.subject.clone()),
            )
            .children(c.start.then(|| {
                div()
                    .font_mono()
                    .font_size(11)
                    .color(palette::BLUE_BORDER)
                    .text(word("dialog.undo.started"))
            }))
            .child(div().font_mono().font_size(11).color(palette::DIM).text(c.when.clone()))
    };
    let count = match commits.len() {
        1 => word("dialog.undo.count_one").to_string(),
        n => fill("dialog.undo.count", &[("n", &n.to_string())]),
    };
    let label = div()
        .flex_col()
        .gap(4)
        .w(96)
        .shrink_0()
        .child(
            div()
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .color(palette::DIM)
                .text(word("dialog.undo.put_back")),
        )
        .child(div().font_size(12).color(palette::DIM).text(count));
    let tree = div()
        .layer("commits")
        .flex_col()
        .flex_1()
        .style("min-width", "0px")
        .style("max-height", "156px")
        .style("overflow-y", "auto")
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .shows(picked)
        .children(commits.iter().enumerate().map(row));
    asking_bar("Undo bar", palette::AMBER_BORDER)
        .child(title(word("dialog.undo.title")))
        .child(div().flex_row().gap(10).items_start().child(label).child(tree))
        .child(
            div()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1())
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    BarAct::Cancel,
                    Tone::Quiet,
                    Size::Bar,
                ))
                .child(button("btn Undo", word("offer.undo"), BarAct::Undo, Tone::Warn, Size::Bar)),
        )
}

fn render_seal_disposition_bar(
    client: Option<&ProjectClient>,
    project: &std::path::Path,
    eval: &str,
    selected: &str,
    picked_reason: Option<&str>,
) -> Element {
    let chip = |layer: String, said: String, picked: bool, act: BarAct| {
        div()
            .layer(layer)
            .on_click(act)
            .pad(6, 12)
            .bg(if picked { palette::SELECTED_BG } else { palette::SURFACE })
            .border(if picked { palette::BLUE_BORDER } else { palette::LINE })
            .font_weight(if picked { 600 } else { 400 })
            .font_size(13)
            .color(if picked { palette::BLUE_BORDER } else { palette::MUTED })
            .text(said)
    };
    let disposition = |d: &str| {
        let act = BarAct::PickDisposition {
            project: project.to_path_buf(),
            eval: eval.to_string(),
            disposition: d.to_string(),
        };
        chip(format!("disposition {d}"), weft_core::tree::decision(d), selected == d, act)
    };
    let reason = |r: &str| {
        let picked = picked_reason == Some(r);
        let act = BarAct::PickReason {
            project: project.to_path_buf(),
            eval: eval.to_string(),
            disposition: selected.to_string(),
            reason: (!picked).then(|| r.to_string()),
        };
        chip(format!("reason {r}"), word(&format!("seal.r.{r}")).to_string(), picked, act)
    };
    let row = |label: &str| {
        div().flex_row().gap(10).items_center().font_size(13).child(
            div()
                .w(64)
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .color(palette::DIM)
                .text(word(label)),
        )
    };
    // The work, by its first Ask's title and its Asks, and the Eval's verdict.
    let unit = client.and_then(|c| c.work.of_eval(eval));
    let title_of = |root: &str| {
        client.and_then(|c| c.units.iter().find(|u| u.ask_id == root)).map(|u| u.title.clone())
    };
    let work = unit.map(|u| {
        let t = title_of(&u.root).unwrap_or_default();
        match u.asks.len() {
            1 => fill("dialog.eval.work_one", &[("title", &t)]),
            n => fill("dialog.eval.work", &[("title", &t), ("n", &n.to_string())]),
        }
    });
    let judged = client.and_then(|c| c.work.record.evals.iter().find(|e| e.id == eval));
    let verdict = judged.and_then(|e| {
        let v = e.verdict?;
        let agreement = e.agreement.map(|a| format!(" {a:.2}")).unwrap_or_default();
        Some(format!(
            "{}{agreement} · {}",
            word(&format!("verdict.{}", v.recorded())),
            fill("tree.eval", &[("n", &e.n.to_string())])
        ))
    });
    let reasons = weft_core::tree::reasons(selected);
    let said = fill(
        if picked_reason.is_some() { "dialog.seal.records_why" } else { "dialog.seal.records" },
        &[
            ("disposition", &weft_core::tree::decision(selected)),
            ("work", work.as_deref().unwrap_or(eval)),
        ],
    );
    asking_bar("Seal disposition bar", palette::BLUE_BORDER)
        .child(
            div()
                .flex_row()
                .gap(8)
                .items_center()
                .child(title(word("dialog.seal.title")))
                .children(work.map(|w| div().font_size(13).color(palette::MUTED).text(w))),
        )
        .children(verdict.map(|v| row("dialog.seal.verdict").child(span(v))))
        .child(row("dialog.seal.decision").children(weft_core::tree::DISPOSITIONS.map(disposition)))
        .children((!reasons.is_empty()).then(|| {
            row("dialog.seal.why")
                .children(reasons.iter().map(|r| reason(r)))
                .child(span(word("dialog.seal.optional")).font_size(12).color(palette::DIM))
        }))
        .child(
            div()
                .flex_row()
                .gap(10)
                .items_center()
                .child(div().flex_1().font_size(13).color(palette::MUTED).text(said))
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    BarAct::Cancel,
                    Tone::Quiet,
                    Size::Bar,
                ))
                .child(button(
                    "btn Seal",
                    word("act.seal"),
                    BarAct::ConfirmSeal {
                        project: project.to_path_buf(),
                        eval: eval.to_string(),
                        disposition: selected.to_string(),
                        reason: picked_reason.map(str::to_string),
                    },
                    Tone::Primary,
                    Size::Bar,
                )),
        )
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
        Some(TreeSelection::Work { project, unit }) => props
            .clients
            .iter()
            .find(|c| &c.root == project)
            .and_then(|c| c.units.iter().find(|u| &u.ask_id == unit))
            .map(|u| u.title.as_str()),
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
