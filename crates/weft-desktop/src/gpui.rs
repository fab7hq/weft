//! The declarative element tree the views build and the window draws, and
//! the guideline's palette.

use std::path::Path;

use crate::action::{Action, Focus};

/// The design guideline's tokens (`tokens.css`, from Toile's `weft`).
pub mod palette {
    pub const CANVAS: &str = "#090d16";
    pub const SURFACE: &str = "#101726";
    pub const SURFACE_ALT: &str = "#0b111d";
    pub const TERMINAL_BG: &str = "#05080f";
    pub const SELECTED_BG: &str = "#16233a";
    pub const TITLEBAR_BG: &str = "#151b28";
    pub const LINE: &str = "#253550";
    pub const LINE_HOVER: &str = "#3a4a66";
    pub const TEXT: &str = "#f1f3f7";
    pub const MUTED: &str = "#a2adbd";
    pub const DIM: &str = "#6e7b8f";
    pub const AMBER: &str = "#edbb5b";
    pub const AMBER_BORDER: &str = "#d29922";
    pub const AMBER_BG: &str = "#2a2011";
    pub const TEAL: &str = "#39c5bb";
    pub const TEAL_BG: &str = "#0c2628";
    pub const BLUE: &str = "#388bfd";
    pub const BLUE_BORDER: &str = "#58a6ff";
    pub const RED: &str = "#f85149";
    pub const RED_BG: &str = "#2d1216";
    /// Corners: a boxed control's, and a menu's, popover's or sheet's.
    pub const RADIUS_CONTROL: u32 = 4;
    pub const RADIUS_PANEL: u32 = 6;
}

/// Declarative GPUI element node.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub tag: &'static str,
    pub layer_name: Option<String>,
    pub id: Option<String>,
    pub role: Option<String>,
    pub aria_label: Option<String>,
    pub aria_selected: Option<bool>,
    pub placeholder: Option<String>,
    pub styles: Vec<(String, String)>,
    pub attrs: Vec<(String, String)>,
    pub text: Option<String>,
    pub action: Option<Action>,
    /// A text field: which one.
    pub field: Option<Focus>,
    /// A tab that can be dragged.
    pub drag: Option<crate::panes_view::TabDrag>,
    /// Where a tab dropped here goes.
    pub drop: Option<crate::panes_view::DropTarget>,
    /// Shows that a tab dragged over it would drop here.
    pub drop_glow: bool,
    /// Rich text: how each range of `text` is styled.
    pub runs: Vec<(std::ops::Range<usize>, Run)>,
    pub children: Vec<Element>,
}

/// How a range of rich text is styled.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
    pub strike: bool,
    pub underline: bool,
    pub color: Option<&'static str>,
    pub background: Option<&'static str>,
}

pub fn div() -> Element {
    Element {
        tag: "div",
        layer_name: None,
        id: None,
        role: None,
        aria_label: None,
        aria_selected: None,
        placeholder: None,
        styles: Vec::new(),
        attrs: Vec::new(),
        text: None,
        action: None,
        field: None,
        drag: None,
        drop: None,
        drop_glow: false,
        runs: Vec::new(),
        children: Vec::new(),
    }
}

/// Text whose ranges are styled apart, wrapping as one line of words.
pub fn rich(text: impl Into<String>, runs: Vec<(std::ops::Range<usize>, Run)>) -> Element {
    let mut el = div();
    el.tag = "rich";
    el.text = Some(text.into());
    el.runs = runs;
    el
}

/// A bar of `colour` along one side of its parent, over its own lines: a div
/// draws its border in one colour, so a selection's bar is a child of its own.
/// The parent is `position: relative`.
pub fn edge_bar(side: &str, width: u32, colour: &str) -> Element {
    let bar = div().style("position", "absolute").bg(colour);
    match side {
        "top" => bar.style("top", "0").style("left", "0").style("right", "0").h(width),
        _ => bar.style("top", "0").style("left", "0").style("bottom", "0").w(width),
    }
}

pub fn span(t: impl Into<String>) -> Element {
    let mut el = div();
    el.tag = "span";
    el.text = Some(t.into());
    el
}

/// One of Weft's own pictures (`asset`), at the width and height its styles give.
pub fn img_el(name: &str) -> Element {
    let mut el = div();
    el.tag = "img";
    el.attrs.push(("src".into(), name.into()));
    el
}

/// The bytes of one of Weft's own pictures, an SVG: its banner, the woven mark
/// and its name, drawn for a dark background.
pub fn asset(name: &str) -> Option<&'static [u8]> {
    match name {
        "weft-banner" => Some(include_bytes!("../../../assets/weft-banner-dark.svg")),
        _ => None,
    }
}

/// One of the desktop's text fields: a line, or an area that grows with its lines.
pub fn input_el(field: Focus) -> Element {
    let mut el = div();
    el.tag = "input";
    el.field = Some(field);
    el
}

pub fn textarea_el(field: Focus) -> Element {
    let mut el = div();
    el.tag = "textarea";
    el.field = Some(field);
    el
}

/// A pane's terminal, which the window paints from its own cells.
pub fn terminal_el(project: &Path, pane: u32) -> Element {
    let mut el = div();
    el.tag = "terminal";
    el.attrs.push(("data-project".into(), project.to_string_lossy().into_owned()));
    el.attrs.push(("data-pane".into(), pane.to_string()));
    el
}

impl Element {
    pub fn layer(mut self, name: impl Into<String>) -> Self {
        self.layer_name = Some(name.into());
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    pub fn aria_selected(mut self, selected: bool) -> Self {
        self.aria_selected = Some(selected);
        self
    }

    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = Some(p.into());
        self
    }

    pub fn attr(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.attrs.push((k.into(), v.into()));
        self
    }

    pub fn style(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        let k = k.into();
        let v = v.into();
        if let Some(existing) = self.styles.iter_mut().find(|(ek, _)| *ek == k) {
            existing.1 = v;
        } else {
            self.styles.push((k, v));
        }
        self
    }

    pub fn flex_row(self) -> Self {
        self.style("display", "flex").style("flex-direction", "row")
    }

    pub fn flex_col(self) -> Self {
        self.style("display", "flex").style("flex-direction", "column")
    }

    pub fn items_center(self) -> Self {
        self.style("align-items", "center")
    }

    pub fn items_start(self) -> Self {
        self.style("align-items", "flex-start")
    }

    pub fn items_baseline(self) -> Self {
        self.style("align-items", "baseline")
    }

    pub fn justify_between(self) -> Self {
        self.style("justify-content", "space-between")
    }

    pub fn justify_center(self) -> Self {
        self.style("justify-content", "center")
    }

    pub fn justify_end(self) -> Self {
        self.style("justify-content", "flex-end")
    }

    pub fn self_stretch(self) -> Self {
        self.style("align-self", "stretch")
    }

    pub fn flex_1(self) -> Self {
        self.style("flex", "1 1 0").style("min-width", "0").style("min-height", "0")
    }

    pub fn shrink_0(self) -> Self {
        self.style("flex-shrink", "0")
    }

    pub fn gap(self, px: u32) -> Self {
        self.style("gap", format!("{px}px"))
    }

    pub fn w(self, px: u32) -> Self {
        self.style("width", format!("{px}px"))
    }

    pub fn h(self, px: u32) -> Self {
        self.style("height", format!("{px}px"))
    }

    pub fn pad(self, v: u32, h: u32) -> Self {
        self.style("padding", format!("{v}px {h}px"))
    }

    pub fn bg(self, color: &str) -> Self {
        self.style("background", color)
    }

    pub fn color(self, color: &str) -> Self {
        self.style("color", color)
    }

    pub fn font_size(self, px: u32) -> Self {
        self.style("font-size", format!("{px}px"))
    }

    pub fn font_weight(self, w: u32) -> Self {
        self.style("font-weight", w.to_string())
    }

    pub fn font_mono(self) -> Self {
        self.style("font-family", "'JetBrains Mono'")
    }

    /// A box around the element: a control, so with the control's corners.
    pub fn border(self, color: &str) -> Self {
        self.style("border", format!("1px solid {color}"))
            .style("border-radius", format!("{}px", palette::RADIUS_CONTROL))
    }

    /// A panel's corners: a menu, a popover or a sheet over the page.
    pub fn panel_corners(self) -> Self {
        self.style("border-radius", format!("{}px", palette::RADIUS_PANEL))
    }

    pub fn border_bottom(self, color: &str) -> Self {
        self.style("border-bottom", format!("1px solid {color}"))
    }

    pub fn border_top(self, color: &str) -> Self {
        self.style("border-top", format!("1px solid {color}"))
    }

    pub fn border_right(self, color: &str) -> Self {
        self.style("border-right", format!("1px solid {color}"))
    }

    pub fn text(mut self, t: impl Into<String>) -> Self {
        self.text = Some(t.into());
        self
    }

    pub fn drag(mut self, tab: crate::panes_view::TabDrag) -> Self {
        self.drag = Some(tab);
        self
    }

    pub fn drop_on(mut self, target: crate::panes_view::DropTarget) -> Self {
        self.drop = Some(target);
        self
    }

    pub fn drop_glow(mut self) -> Self {
        self.drop_glow = true;
        self
    }

    pub fn on_click(mut self, action: impl Into<Action>) -> Self {
        self.action = Some(action.into());
        self
    }

    pub fn child(mut self, child: Element) -> Self {
        self.children.push(child);
        self
    }

    pub fn children(mut self, iter: impl IntoIterator<Item = Element>) -> Self {
        self.children.extend(iter);
        self
    }

    /// Find the first descendant or self matching `layer-name`.
    pub fn find_layer(&self, name: &str) -> Option<&Element> {
        if self.layer_name.as_deref() == Some(name) {
            return Some(self);
        }
        for c in &self.children {
            if let Some(found) = c.find_layer(name) {
                return Some(found);
            }
        }
        None
    }

    /// Find the first descendant or self with `on_click` action `act`.
    pub fn find_action(&self, act: &Action) -> Option<&Element> {
        if self.action.as_ref() == Some(act) {
            return Some(self);
        }
        for c in &self.children {
            if let Some(found) = c.find_action(act) {
                return Some(found);
            }
        }
        None
    }

    /// Collect all visible text in pre-order traversal.
    pub fn all_text(&self) -> String {
        let mut out = String::new();
        self.collect_text(&mut out);
        out
    }

    fn collect_text(&self, out: &mut String) {
        if let Some(t) = &self.text {
            if !out.is_empty() && !out.ends_with(' ') && !out.ends_with('\n') {
                out.push(' ');
            }
            out.push_str(t);
        }
        for c in &self.children {
            c.collect_text(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::palette::*;

    /// The value `tokens.css` gives a custom property.
    fn token(name: &str) -> String {
        let css = include_str!("../tokens.css");
        let at =
            css.find(&format!("{name}:")).unwrap_or_else(|| panic!("{name} not in tokens.css"));
        let value = &css[at + name.len() + 1..];
        value[..value.find(';').unwrap()].trim().to_string()
    }

    #[test]
    fn the_palette_is_the_guideline_tokens() {
        for (ours, name) in [
            (CANVAS, "--fab7-canvas"),
            (SURFACE, "--fab7-surface"),
            (SURFACE_ALT, "--fab7-panel"),
            (TERMINAL_BG, "--fab7-void"),
            (SELECTED_BG, "--fab7-selected"),
            (TITLEBAR_BG, "--fab7-titlebar"),
            (LINE, "--fab7-line"),
            (LINE_HOVER, "--fab7-line-hover"),
            (TEXT, "--fab7-text"),
            (MUTED, "--fab7-muted"),
            (DIM, "--fab7-faint"),
            (AMBER, "--fab7-amber-text"),
            (AMBER_BORDER, "--fab7-amber"),
            (AMBER_BG, "--fab7-amber-bg"),
            (TEAL, "--fab7-teal"),
            (TEAL_BG, "--fab7-teal-bg"),
            (BLUE, "--fab7-button"),
            (BLUE_BORDER, "--fab7-accent"),
            (RED, "--fab7-danger"),
            (RED_BG, "--fab7-danger-bg"),
        ] {
            assert_eq!(ours, token(name), "{name}");
        }
        assert_eq!(format!("{RADIUS_CONTROL}px"), token("--weft-radius-control"));
        assert_eq!(format!("{RADIUS_PANEL}px"), token("--weft-radius-panel"));
    }
}
