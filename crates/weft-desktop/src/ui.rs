//! The pieces every screen is built from: buttons, a sheet's frame, a
//! section's label, a list of rows, a bar along the window's foot, and the
//! colour of a mark.

use weft_core::words;

use crate::action::{Action, SheetAct};
use crate::gpui::{Element, div, palette};

/// What a button is for, which says how it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The act the screen is for.
    Primary,
    Plain,
    /// Leaving without acting.
    Quiet,
    /// An act that stops or removes something.
    Warn,
}

/// How large a button is: in a sheet's foot, in the 56 px bar, or in a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Sheet,
    Bar,
    Row,
}

pub fn button(
    layer: impl Into<String>,
    label: impl Into<String>,
    action: impl Into<Action>,
    tone: Tone,
    size: Size,
) -> Element {
    let (v, h, font) = match size {
        Size::Sheet => (9, 18, 14),
        Size::Bar => (7, 16, 13),
        Size::Row => (5, 12, 12),
    };
    let (bg, line, text) = match tone {
        Tone::Primary => (palette::BLUE, palette::BLUE_BORDER, palette::CANVAS),
        Tone::Plain => (palette::SURFACE, palette::LINE, palette::TEXT),
        Tone::Quiet => (palette::SURFACE, palette::LINE, palette::MUTED),
        Tone::Warn => (palette::SURFACE, palette::AMBER, palette::AMBER),
    };
    div()
        .layer(layer)
        .on_click(action)
        .shrink_0()
        .pad(v, h)
        .bg(bg)
        .border(line)
        .font_weight(600)
        .font_size(font)
        .color(text)
        .text(label)
}

/// A sheet's Close.
pub fn close_button() -> Element {
    button("btn Close", words::word("act.close"), SheetAct::Close, Tone::Plain, Size::Sheet)
}

/// A sheet: its head, its body, which scrolls, and its foot of acts.
pub fn sheet(layer: &str, head: Element, body: Element, foot: Element) -> Element {
    div()
        .layer(layer)
        .flex_1()
        .self_stretch()
        .flex_col()
        .items_start()
        .child(head)
        .child(body)
        .child(foot)
}

/// A sheet's head: its title, and what it says beside it.
pub fn sheet_head(title: impl Into<String>, aside: Option<(String, &str)>) -> Element {
    let mut head = div()
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(12, 20)
        .items_baseline()
        .border_bottom(palette::LINE)
        .child(div().flex_1().font_weight(600).font_size(16).text(title));
    if let Some((text, colour)) = aside {
        head = head.child(div().font_size(13).color(colour).text(text));
    }
    head
}

/// A sheet's body: a column that scrolls.
pub fn sheet_body() -> Element {
    div()
        .self_stretch()
        .flex_1()
        .flex_col()
        .style("overflow-y", "auto")
        .gap(12)
        .pad(18, 20)
        .items_start()
}

/// A sheet's foot: its acts, with room between those on the left and the right.
pub fn sheet_foot(left: Vec<Element>, right: Vec<Element>) -> Element {
    div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(10, 20)
        .items_center()
        .border_top(palette::LINE)
        .children(left)
        .child(div().flex_1())
        .children(right)
}

/// A section's name, in the eyebrow's capitals.
pub fn section_label(text: impl Into<String>) -> Element {
    div()
        .font_mono()
        .font_weight(600)
        .font_size(11)
        .color(palette::DIM)
        .text(text.into().to_uppercase())
}

/// A plain line of the sheet's prose, which wraps.
pub fn note(text: impl Into<String>, colour: &str) -> Element {
    div().self_stretch().font_size(13).color(colour).text(text)
}

/// Rows in a box, a line between each.
pub fn row_list(rows: Vec<Element>) -> Element {
    let n = rows.len();
    let mut list = div().self_stretch().flex_col().bg(palette::SURFACE).border(palette::LINE);
    for (i, row) in rows.into_iter().enumerate() {
        list = list.child(if i + 1 < n { row.border_bottom(palette::LINE) } else { row });
    }
    list
}

/// One row of a list.
pub fn list_row() -> Element {
    div().self_stretch().flex_row().gap(10).pad(10, 14).items_center()
}

/// The colour of a mark: ✓ teal, ✗ red, anything else amber.
pub fn mark_colour(mark: &str) -> &'static str {
    match mark {
        "✓" => palette::TEAL,
        "✗" => palette::RED,
        _ => palette::AMBER,
    }
}

/// A verdict's colour.
pub fn verdict_colour(verdict: &str) -> &'static str {
    if verdict == "aligned" { palette::TEAL } else { palette::AMBER }
}

/// A bar along the window's foot that asks something: its line on top in
/// `accent`.
pub fn asking_bar(layer: &str, accent: &str) -> Element {
    div()
        .layer(layer)
        .self_stretch()
        .flex_col()
        .gap(8)
        .pad(14, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {accent}"))
}

/// The acts of an asking bar, at its right.
pub fn bar_acts(acts: Vec<Element>) -> Element {
    div().self_stretch().flex_row().gap(10).items_center().child(div().flex_1()).children(acts)
}
