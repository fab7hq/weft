//! The person's decisions, as a project's settings list them (card 9.15):
//! each answer given at a Seal in its answer's colour, its day and the files
//! it is about; one whose file has changed since says so and carries
//! "changed since". Nothing here is pressable.

use weft_core::decisions::{Decision, Decisions};

use crate::daemon::{ProjectClient, Then};
use crate::gpui::{Element, Run, div, palette, rich};
use crate::seal_dialog::tone;

/// The section, under the project's settings; none while nothing is decided
/// or the list is still being read.
pub fn settings(client: Option<&ProjectClient>) -> Option<Element> {
    let read = client?.read.get(&Then::Decisions)?;
    let d: Decisions = serde_json::from_value(read.clone()).ok()?;
    if d.list.is_empty() {
        return None;
    }
    let head = div()
        .layer("decisions head")
        .self_stretch()
        .flex_row()
        .justify_between()
        .items_center()
        .style("padding", "26px 0 8px 0")
        .border_bottom(palette::LINE)
        .child(
            div()
                .font_mono()
                .font_weight(600)
                .font_size(11)
                .color(palette::DIM)
                .text(d.title.clone()),
        )
        .child(div().layer("decisions count").font_size(13).color(palette::MUTED).text(d.count));
    let rows = d.list.iter().map(|one| row(one, &d.changed));
    Some(div().layer("your decisions").self_stretch().flex_col().child(head).children(rows))
}

/// One decision: its dot, its behaviour, and under it the answer, the day,
/// and its files, or what has changed since.
fn row(d: &Decision, changed: &str) -> Element {
    let (colour, _, _) = tone(&d.tone);
    let lead = format!("{} · {} · ", d.answer_words, d.day);
    let (rest, files) = match d.stale_line.as_deref().filter(|_| d.stale) {
        Some(line) => (line.to_string(), stale_files(line)),
        None => (d.files.join(", "), d.files.clone()),
    };
    let mut runs = vec![(0..d.answer_words.len(), Run { color: Some(colour), ..Run::default() })];
    runs.extend(
        crate::ui::mono_runs(&rest, &files, None)
            .into_iter()
            .map(|(r, run)| (r.start + lead.len()..r.end + lead.len(), run)),
    );
    let sub = rich(format!("{lead}{rest}"), runs)
        .layer("decision said")
        .font_size(12)
        .color(palette::MUTED);
    let tag = d.stale.then(|| {
        div()
            .layer("decision changed")
            .shrink_0()
            .flex_row()
            .items_center()
            .h(22)
            .pad(0, 8)
            .border(palette::AMBER_BORDER)
            .style("white-space", "nowrap")
            .font_size(12)
            .color(palette::AMBER)
            .text(changed.to_string())
    });
    let dot = div()
        .layer("decision dot")
        .w(8)
        .h(8)
        .bg(colour)
        .style("border-radius", format!("{}px", palette::RADIUS_CONTROL));
    div()
        .layer(if d.stale { "a decision gone stale" } else { "a decision that stands" })
        .self_stretch()
        .flex_row()
        .items_start()
        .gap(12)
        .style("padding", "11px 0")
        .border_bottom(palette::LINE)
        .font_size(14)
        .child(div().style("padding-top", "6px").shrink_0().child(dot))
        .child(
            div()
                .flex_1()
                .style("min-width", "0")
                .flex_col()
                .gap(3)
                .child(
                    div()
                        .layer("decision behaviour")
                        .color(palette::TEXT)
                        .text(d.behaviour.clone()),
                )
                .child(sub),
        )
        .children(tag)
}

/// The files a stale line names, as its words put them before "has changed
/// since".
fn stale_files(line: &str) -> Vec<String> {
    ["decisions.stale", "decisions.stale_n"]
        .iter()
        .find_map(|key| {
            let (pre, post) = weft_core::words::word(key).split_once("{files}")?;
            line.strip_prefix(pre)?.strip_suffix(post).map(str::to_string)
        })
        .map(|files| files.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}
