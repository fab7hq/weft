//! The 48 px top bar and the macOS window titlebar.

use weft_core::words;

use crate::gpui::{Element, div, palette, span};

#[derive(Debug, Clone)]
pub struct HeaderProps {
    pub machine: String,
    pub sidebar_open: bool,
    pub needs_you: usize,
    pub menu_open: bool,
}

/// Render the native macOS window titlebar (`Weft — <machine>`).
pub fn render_titlebar(machine: &str) -> Element {
    div()
        .layer("titlebar")
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(9, 14)
        .items_center()
        .bg(palette::TITLEBAR_BG)
        .border_bottom(palette::LINE)
        .child(div().w(12).h(12).shrink_0().bg("#ff5f57"))
        .child(div().w(12).h(12).shrink_0().bg("#febc2e"))
        .child(div().w(12).h(12).shrink_0().bg("#28c840"))
        .child(
            div()
                .flex_1()
                .flex_row()
                .justify_center()
                .items_center()
                .child(div().font_size(12).color(palette::DIM).text(format!("Weft — {machine}"))),
        )
        .child(div().w(52).h(4).shrink_0())
}

/// Render the top bar: the list's toggle, the woven mark, the machine, what
/// needs you, and `☰`. Turbo shows on each agent's title, Updates in `☰`.
pub fn render_header(props: &HeaderProps) -> Element {
    let toggle_label = if props.sidebar_open {
        words::word("web.list.hide")
    } else {
        words::word("web.list.show")
    };
    let icon_color = if props.sidebar_open { palette::BLUE_BORDER } else { palette::DIM };
    let toggle_icon = div()
        .w(14)
        .h(11)
        .border(icon_color)
        .flex_row()
        .items_center()
        .style("overflow", "hidden")
        .child(
            div()
                .w(4)
                .style("height", "100%")
                .bg(if props.sidebar_open { palette::BLUE_BORDER } else { "transparent" })
                .border_right(icon_color),
        )
        .child(div().flex_1().style("height", "100%"));

    let toggle = div()
        .layer("the list's toggle")
        .role("button")
        .aria_label(toggle_label)
        .on_click("toggle_sidebar")
        .flex_row()
        .pad(6, 8)
        .items_center()
        .bg(if props.sidebar_open { palette::SELECTED_BG } else { palette::SURFACE })
        .border(if props.sidebar_open { palette::BLUE_BORDER } else { palette::LINE })
        .child(toggle_icon);

    let brand = div()
        .flex_row()
        .gap(8)
        .items_center()
        .child(
            div()
                .flex_row()
                .font_mono()
                .font_weight(700)
                .font_size(12)
                .style("line-height", "18px")
                .child(span("╸").color(palette::TEAL))
                .child(span("┃").color(palette::BLUE_BORDER))
                .child(span("╺").color(palette::TEAL)),
        )
        .child(
            div()
                .font_weight(700)
                .font_size(15)
                .style("letter-spacing", "0.04em")
                .color(palette::TEAL)
                .text("WEFT"),
        );

    let mut bar = div()
        .layer("header")
        .self_stretch()
        .h(48)
        .flex_row()
        .gap(12)
        .pad(0, 16)
        .items_center()
        .bg(palette::CANVAS)
        .border_bottom(palette::LINE)
        .child(toggle)
        .child(brand)
        .child(div().font_weight(600).font_size(15).text(props.machine.clone()))
        .child(div().flex_1().flex_row().items_center());

    if props.needs_you > 0 {
        bar = bar.child(
            div()
                .layer("needs you tag")
                .flex_row()
                .gap(6)
                .pad(4, 10)
                .items_center()
                .bg(palette::AMBER_BG)
                .border(palette::AMBER_BORDER)
                .font_mono()
                .font_size(12)
                .color(palette::AMBER)
                .child(span("■"))
                .child(span(format!("NEEDS YOU {}", props.needs_you))),
        );
    }

    bar.child(
        div()
            .layer("menu button")
            .role("button")
            .aria_label("Menu")
            .on_click("toggle_menu")
            .pad(4, 8)
            .font_weight(600)
            .font_size(18)
            .text("☰"),
    )
}
