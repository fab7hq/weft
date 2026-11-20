//! Updates: Weft, RingFrame and Sense, and every harness Weft has a file for,
//! each with its mark and what it needs, as the daemon's setup view says.

use std::time::Instant;

use weft_core::onboarding::{Next, State as SetupState, View as SetupView};
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::SheetAct;
use crate::gpui::{Element, div, palette};
use crate::ui::{
    Size, Tone, button, close_button, list_row, note, row_list, section_label, sheet, sheet_body,
    sheet_foot, sheet_head,
};
use crate::waiting::Progress;

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

/// A row that is working: the dot where its mark was, what it says, then
/// its words (and the time so far), and, while it updates, the moving bar.
fn working_row(
    layer: String,
    name: &str,
    said: String,
    doing: Option<Element>,
    (since, bar): (Option<Instant>, bool),
) -> Element {
    let now = Instant::now();
    let head = list_row()
        .gap(12)
        .pad(0, 0)
        .child(div().w(16).flex_row().justify_center().child(crate::waiting::dot()))
        .child(div().w(140).font_weight(600).font_size(14).text(name))
        .child(div().flex_1().font_size(13).color(palette::MUTED).text(said))
        .children(doing)
        .children(since.map(|since| {
            div()
                .layer("working time")
                .font_mono()
                .font_size(12)
                .color(palette::DIM)
                .text(crate::waiting::elapsed(since, now))
        }));
    div()
        .layer(layer)
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(10, 16)
        .child(head)
        .children(bar.then(crate::waiting::moving_bar))
}

/// An update the machine says how it goes: the dot, "Updating…" and its
/// time, the bar filled by the share done and its steps on one line; or,
/// while a step counts, that step's words, its count and its own time.
fn job_row(layer: String, name: &str, version: &str, p: &Progress) -> Element {
    let now = Instant::now();
    let counted = p.now_step().and_then(|s| Some((s, crate::waiting::count(s)?)));
    let (said, time, counted) = match counted {
        Some((s, c)) => (
            crate::waiting::step_said(s),
            p.step_time(s, now).unwrap_or_else(|| now.saturating_duration_since(p.since)),
            Some(c),
        ),
        None => (word("wait.updating").to_string(), now.saturating_duration_since(p.since), None),
    };
    let head = list_row()
        .gap(12)
        .pad(0, 0)
        .child(div().w(16).flex_row().justify_center().child(crate::waiting::dot()))
        .child(div().w(140).font_weight(600).font_size(14).text(name))
        .child(div().flex_1().font_size(13).color(palette::MUTED).text(version))
        .child(div().layer("updating").font_size(13).color(palette::BLUE_BORDER).text(said))
        .children(counted.clone().map(|c| {
            div().layer("step count").font_mono().font_size(12).color(palette::MUTED).text(c)
        }))
        .child(
            div()
                .layer("working time")
                .font_mono()
                .font_size(12)
                .color(palette::DIM)
                .text(crate::waiting::clock(time)),
        );
    let inline = (counted.is_none() && !p.job.steps.is_empty())
        .then(|| crate::waiting::steps_inline(p).style("padding-left", "28px"));
    div()
        .layer(layer)
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(10, 16)
        .child(head)
        .child(crate::waiting::share_bar(p.job.share))
        .children(inline)
}

/// A harness's setup the machine says how it goes: which step of how many,
/// what it runs, and that step's own time.
fn setup_row(layer: String, name: &str, p: &Progress) -> Element {
    let now = Instant::now();
    let step = p.now_step();
    let time = step
        .and_then(|s| p.step_time(s, now))
        .unwrap_or_else(|| now.saturating_duration_since(p.since));
    let place = p.place().map(|n| div().layer("step place").color(palette::MUTED).text(n));
    let doing =
        step.map(|s| div().font_mono().color(palette::TEXT).text(crate::waiting::step_said(s)));
    let dot = (place.is_some() && doing.is_some()).then(|| div().color(palette::DIM).text("·"));
    list_row()
        .layer(layer)
        .gap(12)
        .pad(10, 16)
        .child(div().w(16).flex_row().justify_center().child(crate::waiting::dot()))
        .child(div().w(140).font_weight(600).font_size(14).text(name))
        .child(
            div()
                .flex_1()
                .flex_row()
                .gap(6)
                .items_center()
                .font_size(12)
                .style("min-width", "0px")
                .children(place)
                .children(dot)
                .children(doing),
        )
        .child(
            div()
                .layer("working time")
                .font_mono()
                .font_size(12)
                .color(palette::DIM)
                .text(crate::waiting::clock(time)),
        )
}

/// Why an update failed, under its row, until the person runs it again.
fn job_failed(p: &Progress) -> Option<Element> {
    let why = p.failure()?;
    Some(
        list_row()
            .layer("job failed")
            .style("padding", "0 16px 12px 44px")
            .child(crate::waiting::failed(why, None)),
    )
}

/// A button that rests while another update runs: dim, and not pressed.
fn resting(button: Element) -> Element {
    Element { action: None, ..button }.style("opacity", "0.45")
}

/// What a row's act waits for while another update runs.
fn waits_for(name: &str) -> Element {
    div()
        .layer("waits for")
        .font_size(12)
        .color(palette::DIM)
        .text(fill("wait.waits_for", &[("name", name)]))
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
    let Some(v) = props.setup.cloned() else {
        return sheet(
            "sheet Updates",
            sheet_head(word("desktop.updates.title"), None),
            sheet_body()
                .child(crate::waiting::busy_line(word("desktop.updates.asking"), palette::MUTED)),
            sheet_foot(Vec::new(), vec![close_button()]),
        );
    };
    let head_said = match behind(&v) {
        0 if v.up_to_date() => word("desktop.updates.all_current").to_string(),
        0 => String::new(),
        1 => word("desktop.updates.one").to_string(),
        n => fill("desktop.updates.many", &[("n", &n.to_string())]),
    };

    // An update running: its row says so, and every other act rests and
    // says what it waits for.
    let running = props.updating.as_ref().map(|(name, since)| (name.as_str(), *since));
    let title_of = |name: &str| match name {
        "ringframe" => "RingFrame".to_string(),
        "sense" => "Sense".to_string(),
        other => v.row(other).map_or_else(|| other.to_string(), |r| r.title.clone()),
    };
    // Update all past RingFrame and Sense waits on the harness it runs.
    let waits = running.and_then(|(name, _)| match name {
        "all" => v
            .rows
            .iter()
            .find(|r| matches!(r.state, SetupState::Running(_)))
            .map(|r| r.title.clone()),
        name => Some(title_of(name)),
    });
    let act_of = |b: Element| -> Element {
        match (running, &waits) {
            (None, _) => b,
            (Some(_), None) => resting(b),
            (Some(_), Some(name)) => {
                div().flex_row().gap(10).items_center().child(resting(b)).child(waits_for(name))
            }
        }
    };
    let update = |name: &str, act: SheetAct| {
        act_of(button(
            format!("btn Update {name}"),
            word("desktop.updates.update"),
            act,
            Tone::Plain,
            Size::Row,
        ))
    };
    let checking = || crate::waiting::busy_line(word("wait.checking"), palette::MUTED);
    let updating = || {
        div()
            .layer("updating")
            .font_size(13)
            .color(palette::BLUE_BORDER)
            .text(word("wait.updating"))
    };
    // Our own rows: checking while the release is asked, updating while it
    // updates, else where each stands.
    let ours_row = |target: &str, name: &str, version: &str, mark: &str, act: Option<Element>| {
        let layer = format!("update row {name}");
        let job = props.machine_job("update", Some(target));
        if let Some(p) = job.filter(|p| p.running()) {
            return job_row(layer, name, version, p);
        }
        let row = match running.filter(|(n, _)| *n == target) {
            Some((_, since)) => {
                working_row(layer, name, version.to_string(), Some(updating()), (Some(since), true))
            }
            None if v.pending_release => {
                working_row(layer, name, version.to_string(), Some(checking()), (None, false))
            }
            None => row(layer, mark, name, with_version(version, status(mark)), act),
        };
        match job.and_then(job_failed) {
            Some(failed) => div().self_stretch().flex_col().child(row).child(failed),
            None => row,
        }
    };
    // The daemon's line already says where Weft stands: "0.6.0 → 0.6.3" when behind.
    let ours = row_list(vec![
        // Weft is updated the way it was installed, never from here.
        ours_row("weft", "Weft", &v.weft, v.weft_mark(), None),
        ours_row(
            "ringframe",
            "RingFrame",
            &v.configuration,
            v.configuration_mark(),
            v.configuration_behind
                .then(|| update("RingFrame", SheetAct::SetupRun("ringframe".into()))),
        ),
        ours_row(
            "sense",
            "Sense",
            &v.sense,
            v.sense_mark(),
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
                let b = button(
                    format!("btn {} {}", word(label), r.name),
                    word(label),
                    act,
                    Tone::Plain,
                    Size::Row,
                );
                if matches!(next, Next::Locate) { b } else { act_of(b) }
            });
            let layer = format!("update row {}", r.title);
            let job = props.machine_job("update", Some(&r.name));
            let row = match &r.state {
                // Its setup, as the machine says it goes.
                _ if job.is_some_and(Progress::running) => {
                    setup_row(layer, &r.title, job.expect("running"))
                }
                // A program the person found, being checked.
                _ if running.is_some_and(|(n, _)| n == r.name) => {
                    working_row(layer, &r.title, String::new(), Some(checking()), (None, false))
                }
                // Running a step: it breathes, and says how long.
                // Still asked: "checking…", never a state it has not said.
                SetupState::Checking => {
                    row(layer, r.state.mark(), &r.title, word("wait.checking").to_string(), act)
                }
                SetupState::Running(_) => {
                    let since = props.running_since.get(&r.name).copied();
                    working_row(layer, &r.title, r.state.words(), None, (since, false))
                }
                _ => row(layer, r.state.mark(), &r.title, r.state.words(), act),
            };
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
                _ => job.and_then(job_failed),
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
        let all = button(
            "btn Update all",
            word("desktop.updates.all"),
            SheetAct::SetupAll,
            Tone::Primary,
            Size::Sheet,
        );
        // Update all waits while an update runs.
        right.push(if running.is_some() { resting(all) } else { all });
    }
    let aside = (!head_said.is_empty()).then_some((head_said, palette::BLUE_BORDER));
    sheet(
        "sheet Updates",
        sheet_head(word("desktop.updates.title"), aside),
        body,
        sheet_foot(Vec::new(), right),
    )
}
