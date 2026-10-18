//! SEND TO: where an Ask or a staged send goes. A list in two parts: the
//! project's running agents, then START AN AGENT, a new agent of each
//! harness. Closed, the one picked and ▾; open, the list, the picked one ✓.
//! Picking is a choice: Ask and Send do it.

use weft_core::words::word;

use crate::action::{Action, SheetAct};
use crate::gpui::{Element, div, palette, span};

/// What a new agent of `harness` is picked as.
pub fn start_key(harness: &str) -> String {
    format!("start:{harness}")
}

/// The list as drawn: each running agent by its key (its session) and name,
/// then each harness a new agent can be of.
#[derive(Debug, Clone, Default)]
pub struct SendTo {
    pub agents: Vec<(String, String)>,
    pub starts: Vec<String>,
    pub picked: String,
    pub open: bool,
}

impl SendTo {
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
            .child(span(view.label(&view.picked)))
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
            .child(div().flex_1().text(label));
        if on {
            row = row.bg(palette::SELECTED_BG).color(palette::BLUE_BORDER).child(span("✓"));
        }
        row
    };
    let mut list = div()
        .layer("send to list")
        .w(220)
        .flex_col()
        .pad(4, 0)
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
