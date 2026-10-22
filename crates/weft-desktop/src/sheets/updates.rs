//! Updates: Weft, RingFrame and Sense, and every harness Weft has a file for,
//! each with its mark and what it needs, as the daemon's setup view says.

use weft_core::onboarding::{Next, State as SetupState, View as SetupView};
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::SheetAct;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, note, row_list, section_label, sheet, sheet_body,
    sheet_foot, sheet_head,
};

/// How many things the view says are behind: Weft, RingFrame, Sense and
/// each harness's plugin.
pub fn behind(view: &SetupView) -> usize {
    [view.weft_behind, view.configuration_behind, view.sense_behind].iter().filter(|b| **b).count()
        + view.rows.iter().filter(|r| matches!(r.state, SetupState::Behind { .. })).count()
}

fn mark_colour(mark: &str) -> &'static str {
    match mark {
        "✓" => palette::TEAL,
        "↑" | "↻" => palette::BLUE_BORDER,
        "✗" | "!" => palette::RED,
        _ => palette::AMBER,
    }
}

/// One row: its mark, its name, what it says, and its act.
fn row(layer: String, mark: &str, name: &str, said: String, act: Option<Element>) -> Element {
    list_row()
        .layer(layer)
        .gap(12)
        .pad(10, 16)
        .child(
            div()
                .font_weight(700)
                .font_size(14)
                .color(mark_colour(mark))
                .w(16)
                .style("text-align", "center")
                .text(mark),
        )
        .child(div().w(140).font_weight(600).font_size(14).text(name))
        .child(div().flex_1().font_size(13).color(palette::MUTED).text(said))
        .children(act)
}

/// What a mark of Weft's own rows says.
fn status(mark: &str) -> &'static str {
    word(match mark {
        "↑" => "desktop.updates.behind",
        "↻" => "desktop.updates.waiting",
        "?" => "desktop.updates.unknown",
        _ => "desktop.updates.current",
    })
}

fn with_version(version: &str, said: &str) -> String {
    if version.is_empty() { said.to_string() } else { format!("{version} · {said}") }
}

pub(super) fn render(props: &SheetProps) -> Element {
    let Some(v) = props.clients.iter().find_map(|c| c.setup.clone()) else {
        return sheet(
            "sheet Updates",
            sheet_head(word("desktop.updates.title"), None),
            sheet_body().child(note(word("desktop.updates.asking"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };
    let head_said = match behind(&v) {
        0 if v.up_to_date() => word("desktop.updates.all_current").to_string(),
        0 => String::new(),
        1 => word("desktop.updates.one").to_string(),
        n => fill("desktop.updates.many", &[("n", &n.to_string())]),
    };

    let update = |name: &str, act: SheetAct| {
        button(
            format!("btn Update {name}"),
            word("desktop.updates.update"),
            act,
            Tone::Plain,
            Size::Row,
        )
    };
    let mut weft_said = with_version(&v.weft, status(v.weft_mark()));
    if let (true, Some(latest)) = (v.weft_behind, &v.weft_latest) {
        weft_said = format!("{} → {latest} · {}", v.weft, status("↑"));
    }
    let ours = row_list(vec![
        row(
            "update row Weft".into(),
            v.weft_mark(),
            "Weft",
            weft_said,
            // Weft is updated the way it was installed, never from here.
            None,
        ),
        row(
            "update row RingFrame".into(),
            v.configuration_mark(),
            "RingFrame",
            with_version(&v.configuration, status(v.configuration_mark())),
            v.configuration_behind
                .then(|| update("RingFrame", SheetAct::SetupRun("ringframe".into()))),
        ),
        row(
            "update row Sense".into(),
            v.sense_mark(),
            "Sense",
            status(v.sense_mark()).to_string(),
            v.sense_behind.then(|| update("Sense", SheetAct::SetupRun("sense".into()))),
        ),
    ]);

    let harnesses = v
        .rows
        .iter()
        .flat_map(|r| {
            let act = r.state.next().map(|next| {
                let (label, act) = match next {
                    Next::Locate => ("desktop.updates.locate", SheetAct::Locate(r.name.clone())),
                    Next::SetUp => ("desktop.updates.set_up", SheetAct::SetupRun(r.name.clone())),
                    Next::Update => ("desktop.updates.update", SheetAct::SetupRun(r.name.clone())),
                };
                button(
                    format!("btn {} {}", word(label), r.name),
                    word(label),
                    act,
                    Tone::Plain,
                    Size::Row,
                )
            });
            let row = row(
                format!("update row {}", r.title),
                r.state.mark(),
                &r.title,
                r.state.words(),
                act,
            );
            // A step that failed shows what it printed last, as it printed it.
            let said = match &r.state {
                SetupState::Failed { said, .. } if !said.is_empty() => Some(
                    list_row().style("padding", "0 16px 12px 44px").child(
                        div()
                            .layer("failed step output")
                            .flex_1()
                            .pad(10, 14)
                            .bg(palette::TERMINAL_BG)
                            .style("border-left", format!("2px solid {}", palette::RED))
                            .font_mono()
                            .font_size(12)
                            .style("white-space", "pre")
                            .text(said.clone()),
                    ),
                ),
                _ => None,
            };
            std::iter::once(row).chain(said)
        })
        .collect::<Vec<_>>();

    let mut body = sheet_body().child(section_label("Weft")).child(ours);
    body = body.child(section_label(word("tree.agents")));
    body = if harnesses.is_empty() {
        body.child(note(word("desktop.updates.no_harness"), palette::MUTED))
    } else {
        body.child(note(word("desktop.updates.sign_in"), palette::MUTED)).child(row_list(harnesses))
    };
    if let Some(said) = &v.note {
        body = body.child(note(said.clone(), palette::AMBER));
    }

    let mut right = vec![close_button()];
    if v.proceeds() {
        right.push(button(
            "btn Update all",
            word("desktop.updates.all"),
            SheetAct::SetupAll,
            Tone::Primary,
            Size::Sheet,
        ));
    }
    let aside = (!head_said.is_empty()).then_some((head_said, palette::BLUE_BORDER));
    sheet(
        "sheet Updates",
        sheet_head(word("desktop.updates.title"), aside),
        body,
        sheet_foot(Vec::new(), right),
    )
}
