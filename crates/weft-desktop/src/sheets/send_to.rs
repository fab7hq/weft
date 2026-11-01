//! SEND TO: where an Ask or a staged send goes. A list in two parts: the
//! project's running agents, then START AN AGENT, a new agent of each
//! harness. Closed, the one picked and ▾; open, the list, the picked one ✓.
//! Picking is a choice: Ask and Send do it.

use weft_core::words::word;

use crate::action::{Action, SheetAct};
use crate::gpui::{Element, div, palette, span};

/// What a running agent is picked as: its pane.
pub fn pane_key(pane: u32) -> String {
    format!("pane:{pane}")
}

/// The pane a key picks, if it picks one.
pub fn pane_in(key: &str) -> Option<u32> {
    key.strip_prefix("pane:")?.parse().ok()
}

/// What a new agent of `harness` is picked as.
pub fn start_key(harness: &str) -> String {
    format!("start:{harness}")
}

/// The list as drawn: each running agent by its key (its pane) and name,
/// then each harness a new agent can be of.
#[derive(Debug, Clone, Default)]
pub struct SendTo {
    pub agents: Vec<(String, String)>,
    pub starts: Vec<String>,
    pub picked: String,
    pub open: bool,
}

impl SendTo {
    /// The harness of the agent picked: a new one's, or a running one's as
    /// its label names it (`project · harness`).
    pub fn picked_harness(&self) -> String {
        let label = self.label(&self.picked);
        label.rsplit(" · ").next().unwrap_or_default().to_string()
    }

    fn label(&self, key: &str) -> String {
        match key.strip_prefix("start:") {
            Some(h) => h.to_string(),
            None => self
                .agents
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, l)| l.clone())
                .unwrap_or_default(),
        }
    }
}

pub fn render(view: &SendTo, pick: impl Fn(String) -> Action) -> Element {
    if !view.open {
        return div()
            .layer("send to choice")
            .on_click(SheetAct::ToggleSendTo)
            .flex_row()
            .gap(10)
            .pad(8, 12)
            .items_center()
            .bg(palette::SURFACE)
            .border(palette::LINE)
            .font_size(15)
            .style("max-width", "320px")
            .child(cut(span(view.label(&view.picked))))
            .child(span("▾").font_size(12).color(palette::MUTED));
    }
    let item = |key: &str, label: String| {
        let on = key == view.picked;
        let mut row = div()
            .layer(format!("send to {key}"))
            .on_click(pick(key.to_string()))
            .flex_row()
            .gap(8)
            .pad(0, 12)
            .h(37)
            .items_center()
            .shrink_0()
            .child(cut(div().flex_1().text(label)));
        if on {
            row = row.bg(palette::SELECTED_BG).color(palette::BLUE_BORDER).child(span("✓"));
        }
        row
    };
    // Any number of agents: past six rows or so it scrolls, the one picked
    // in view.
    let started = view.starts.iter().position(|h| start_key(h) == view.picked);
    let picked = view
        .agents
        .iter()
        .position(|(k, _)| *k == view.picked)
        .or(started.map(|at| view.agents.len() + 1 + at))
        .unwrap_or_default();
    let mut list = div()
        .layer("send to list")
        .w(220)
        .flex_col()
        .pad(4, 0)
        .style("max-height", "240px")
        .style("overflow-y", "auto")
        .shows(picked)
        .bg(palette::SURFACE)
        .border(palette::BLUE_BORDER)
        .panel_corners()
        .font_size(15);
    for (key, label) in &view.agents {
        list = list.child(item(key, label.clone()));
    }
    if !view.starts.is_empty() {
        list = list.child(
            div()
                .shrink_0()
                .style("padding", "8px 12px 4px 12px")
                .border_top(palette::LINE)
                .font_mono()
                .font_weight(600)
                .font_size(10)
                .color(palette::DIM)
                .text(word("start.title").to_uppercase()),
        );
        for h in &view.starts {
            list = list.child(item(&start_key(h), h.clone()));
        }
    }
    list
}

/// A name of any length on one line, cut with … where it does not fit.
fn cut(name: Element) -> Element {
    name.style("min-width", "0px")
        .style("white-space", "nowrap")
        .style("overflow", "hidden")
        .style("text-overflow", "ellipsis")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_list_of_any_length_scrolls_and_keeps_the_picked_one_in_view() {
        let agents = (0..40)
            .map(|i| (pane_key(i), format!("a project with a very long name {i} · harness")))
            .collect();
        let starts = vec!["claude".into(), "codex".into()];
        let view = SendTo { agents, starts, picked: start_key("codex"), open: true };
        let list = render(&view, |key| SheetAct::AskTo(key).into());
        let style = |k: &str| list.styles.iter().find(|(s, _)| s == k).map(|(_, v)| v.clone());
        assert_eq!(style("overflow-y").as_deref(), Some("auto"));
        assert!(style("max-height").is_some(), "a bounded height");
        // 40 agents, START AN AGENT, claude, then codex.
        assert_eq!(list.shows, Some(42));
        assert_eq!(list.children[42].layer_name.as_deref(), Some("send to start:codex"));
    }
}
