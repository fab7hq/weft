//! The `☰` menu: open a project, Turbo, Start at login, Remote, Updates and
//! Quit.

use serde_json::Value;
use weft_core::words::{self, fill, word};

use crate::action::WindowAct;
use crate::gpui::{Element, div, palette};
use crate::keys::Shortcut;

pub struct MenuProps<'a> {
    pub turbo: bool,
    pub start_at_login: bool,
    /// `weft remote`'s view, when it has published one.
    pub remote: Option<&'a Value>,
    /// A daemon has answered: until one has, Remote's state is not known.
    pub heard: bool,
    pub updates: usize,
}

/// An `ON`/`OFF` badge, in `accent` when on.
pub(crate) fn badge(layer: &str, on: bool, accent: (&str, &str, &str)) -> Element {
    let (bg, line, text) =
        if on { accent } else { (palette::SURFACE, palette::LINE, palette::MUTED) };
    div()
        .layer(layer)
        .pad(3, 9)
        .bg(bg)
        .border(line)
        .font_mono()
        .font_size(11)
        .font_weight(600)
        .color(text)
        .shrink_0()
        .text(word(if on { "desktop.on" } else { "desktop.off" }))
}

/// One item: what it is, what it says about itself, and the act.
fn item(layer: &str, act: WindowAct, label: &str) -> Element {
    div()
        .layer(layer)
        .border_bottom(palette::LINE)
        .on_click(act)
        .flex_row()
        .pad(10, 14)
        .items_center()
        .justify_between()
        .gap(12)
        .child(div().font_size(13).text(label))
}

/// A switch: its name and what it does, and its badge.
fn switch(layer: &str, act: WindowAct, label: &str, what: &str, badge: Element) -> Element {
    div()
        .layer(layer)
        .border_bottom(palette::LINE)
        .on_click(act)
        .flex_row()
        .pad(10, 14)
        .items_center()
        .justify_between()
        .gap(12)
        .child(
            div()
                .flex_1()
                .flex_col()
                .gap(2)
                .child(div().font_size(13).text(label))
                .child(div().font_size(11).color(palette::MUTED).text(what)),
        )
        .child(badge)
}

fn aside(text: impl Into<String>, colour: &str) -> Element {
    div().font_mono().font_size(11).color(colour).text(text)
}

/// What Remote says it is: on, with its devices, or off. Only `weft remote`
/// says it is on.
fn remote_status(remote: Option<&Value>, heard: bool) -> (String, &'static str) {
    if !heard {
        return (word("web.link.connecting").to_string(), palette::MUTED);
    }
    let on = remote.is_some_and(|v| v["state"] == "on");
    if !on {
        return (word("desktop.remote.off").to_string(), palette::MUTED);
    }
    let n = remote.and_then(|v| v["devices"].as_array()).map_or(0, Vec::len);
    let said = match n {
        0 => word("desktop.remote.on").to_string(),
        1 => fill("desktop.remote.on_one", &[]),
        n => fill("desktop.remote.on_many", &[("n", &n.to_string())]),
    };
    (said, palette::TEAL)
}

/// The `☰` menu.
pub fn render_menu_popover(props: &MenuProps) -> Element {
    let (remote, remote_colour) = remote_status(props.remote, props.heard);
    let updates = if props.updates > 0 {
        aside(fill("desktop.updates.n", &[("n", &props.updates.to_string())]), palette::BLUE_BORDER)
    } else {
        aside(word("desktop.updates.none"), palette::MUTED)
    };
    div()
        .layer("menu popover")
        .style("position", "absolute")
        .style("top", "8px")
        .style("right", "16px")
        .w(310)
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .panel_corners()
        .child(
            item("menu Open project", WindowAct::OpenProject, word("desktop.menu.open"))
                .child(aside(crate::keys::hint_here(Shortcut::OpenProject), palette::DIM)),
        )
        .child(switch(
            "menu Turbo",
            WindowAct::ToggleTurbo,
            word("desktop.menu.turbo"),
            word("desktop.menu.turbo_what"),
            badge(
                "turbo toggle badge",
                props.turbo,
                (palette::AMBER_BG, palette::AMBER_BORDER, palette::AMBER),
            ),
        ))
        .child(switch(
            "menu Start at login",
            WindowAct::ToggleStartAtLogin,
            word("desktop.menu.login"),
            word("desktop.menu.login_what"),
            badge(
                "login toggle badge",
                props.start_at_login,
                (palette::SELECTED_BG, palette::BLUE_BORDER, palette::BLUE_BORDER),
            ),
        ))
        .child(
            item("menu Settings", WindowAct::OpenSettings, word("desktop.menu.settings"))
                .child(aside(crate::keys::hint_here(Shortcut::Settings), palette::DIM)),
        )
        .child(
            item("menu Remote", WindowAct::OpenRemote, words::word("desktop.menu.remote"))
                .child(aside(remote, remote_colour)),
        )
        .child(
            item("menu Updates", WindowAct::OpenUpdates, word("desktop.menu.updates"))
                .child(updates),
        )
        .child(
            div()
                .layer("menu Quit Weft")
                .on_click(WindowAct::RequestQuit)
                .flex_row()
                .pad(10, 14)
                .items_center()
                .justify_between()
                .bg(palette::SELECTED_BG)
                .child(
                    div().font_size(13).color(palette::BLUE_BORDER).text(word("desktop.menu.quit")),
                )
                .child(aside(crate::keys::hint_here(Shortcut::Quit), palette::BLUE_BORDER)),
        )
}
