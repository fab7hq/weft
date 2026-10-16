//! Toile `atlas` `52px` top bar and macOS window titlebar (`D2.6`, `D2.9`, `D9.9`, `D15.7`).

use weft_core::words;

use crate::gpui::{Element, div, palette};

#[derive(Debug, Clone)]
pub struct HeaderProps {
    pub machine: String,
    pub sidebar_open: bool,
    pub turbo: bool,
    pub updates_available: usize,
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
        .child(div().w(12).h(12).shrink_0().rounded(6).bg("#ff5f57"))
        .child(div().w(12).h(12).shrink_0().rounded(6).bg("#febc2e"))
        .child(div().w(12).h(12).shrink_0().rounded(6).bg("#28c840"))
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

/// Render the Toile `atlas` `52px` top bar (`the list's toggle`, `╸┃╺ WEFT`,
/// machine name, `⚡ turbo` pill, `↑ n updates` pill, `NEEDS YOU n` pill, `☰`).
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
        .rounded(2)
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
        .rounded(8)
        .bg(if props.sidebar_open { palette::SELECTED_BG } else { palette::SURFACE })
        .border(if props.sidebar_open { palette::BLUE_BORDER } else { palette::LINE })
        .child(toggle_icon);

    let brand = div()
        .flex_row()
        .gap(8)
        .items_center()
        .child(
            div()
                .font_mono()
                .font_weight(700)
                .font_size(12)
                .style("line-height", "18px")
                .color(palette::TEAL)
                .text("╸┃╺"),
        )
        .child(div().font_weight(600).font_size(15).color(palette::TEAL).text("WEFT"));

    let mut bar = div()
        .layer("header")
        .self_stretch()
        .h(52)
        .flex_row()
        .gap(12)
        .pad(8, 20)
        .items_center()
        .border_bottom(palette::LINE)
        .child(toggle)
        .child(brand)
        .child(div().font_weight(600).font_size(15).text(props.machine.clone()))
        .child(div().flex_1().flex_row().items_center());

    if props.turbo {
        bar = bar.child(
            div()
                .layer("turbo pill")
                .on_click("toggle_turbo")
                .flex_row()
                .gap(5)
                .pad(3, 10)
                .items_center()
                .bg("#3a2c0c")
                .rounded(20)
                .child(
                    div()
                        .font_size(12)
                        .color(palette::AMBER)
                        .text(format!("⚡ {}", words::word("web.turbo"))),
                ),
        );
    }

    if props.updates_available > 0 {
        let label = if props.updates_available == 1 {
            "↑ 1 update".to_string()
        } else {
            format!("↑ {} updates", props.updates_available)
        };
        bar = bar.child(
            div()
                .layer("updates pill")
                .on_click("open_updates")
                .flex_row()
                .gap(5)
                .pad(3, 10)
                .items_center()
                .bg(palette::SELECTED_BG)
                .rounded(20)
                .border(palette::BLUE_BORDER)
                .child(div().font_size(12).color(palette::BLUE_BORDER).text(label)),
        );
    }

    if props.needs_you > 0 {
        bar = bar.child(
            div()
                .layer("needs you pill")
                .flex_row()
                .pad(3, 10)
                .items_center()
                .bg("#3a2c0c")
                .rounded(20)
                .child(
                    div()
                        .font_size(12)
                        .color(palette::AMBER)
                        .text(format!("NEEDS YOU {}", props.needs_you)),
                ),
        );
    }

    bar.child(
        div()
            .layer("menu button")
            .role("button")
            .aria_label("Menu")
            .on_click("toggle_menu")
            .font_weight(600)
            .font_size(19)
            .text("☰"),
    )
}
