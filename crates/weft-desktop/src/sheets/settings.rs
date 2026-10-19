//! Settings: the machine's, for every project, on tabs. General holds Turbo,
//! Start at login, Notifications and Suggestions; each switch is set through
//! the daemon and shows what it last said.

use weft_core::words::word;

use super::SheetProps;
use super::menu::badge;
use crate::action::{Action, Setting, SettingsTab, SheetAct, WindowAct};
use crate::gpui::{Element, div, palette};
use crate::ui::{close_button, row_list, section_label, sheet, sheet_body, sheet_foot, sheet_head};

/// The tabs, in their order.
const TABS: [(SettingsTab, &str, &str); 1] =
    [(SettingsTab::General, "General", "desktop.settings.tab.general")];

pub(super) fn render(props: &SheetProps, tab: SettingsTab) -> Element {
    let body = match tab {
        SettingsTab::General => general(props),
    };
    sheet(
        "sheet Settings",
        sheet_head(word("desktop.settings.title"), None),
        div().self_stretch().flex_1().flex_col().child(tabs(tab)).child(body),
        sheet_foot(Vec::new(), vec![close_button()]),
    )
}

fn tabs(shown: SettingsTab) -> Element {
    let mut row = div()
        .layer("settings tabs")
        .self_stretch()
        .flex_row()
        .gap(4)
        .pad(0, 20)
        .bg(palette::SURFACE_ALT)
        .border_bottom(palette::LINE);
    for (tab, name, key) in TABS {
        let on = tab == shown;
        row = row.child(
            div()
                .layer(format!("settings tab {name}"))
                .on_click(SheetAct::SettingsTab(tab))
                .pad(9, 14)
                .font_size(13)
                .font_weight(if on { 600 } else { 400 })
                .color(if on { palette::TEXT } else { palette::MUTED })
                .style(
                    "border-bottom",
                    format!("2px solid {}", if on { palette::BLUE_BORDER } else { "transparent" }),
                )
                .text(word(key)),
        );
    }
    row
}

fn general(props: &SheetProps) -> Element {
    let config = props.clients.iter().find_map(|c| c.config.as_ref());
    let machine = |key: &str| config.and_then(|c| c["machine"][key].as_bool());
    let start_at_login = props.clients.iter().any(|c| c.start_at_login);
    let turbo_accent = (palette::AMBER_BG, palette::AMBER_BORDER, palette::AMBER);
    let blue = (palette::SELECTED_BG, palette::BLUE_BORDER, palette::BLUE_BORDER);
    sheet_body().child(section_label(word("desktop.settings.every_project"))).child(row_list(vec![
        switch(
            "Turbo",
            WindowAct::ToggleTurbo,
            word("desktop.menu.turbo"),
            word("desktop.settings.turbo_what"),
            badge("setting badge", machine("turbo") == Some(true), turbo_accent),
        ),
        switch(
            "Start at login",
            WindowAct::ToggleStartAtLogin,
            word("desktop.menu.login"),
            word("desktop.menu.login_what"),
            badge("setting badge", start_at_login, blue),
        ),
        switch(
            "Notifications",
            SheetAct::Toggle(Setting::Notify),
            word("desktop.settings.notify"),
            word("desktop.settings.notify_what"),
            badge("setting badge", machine("notify") != Some(false), blue),
        ),
        switch(
            "Suggestions",
            SheetAct::Toggle(Setting::Suggest),
            word("desktop.settings.suggest"),
            word("desktop.settings.suggest_what"),
            badge("setting badge", machine("suggest") != Some(false), blue),
        ),
    ]))
}

/// One switch: its name and what it does, and its badge; the row turns it.
fn switch(name: &str, act: impl Into<Action>, label: &str, what: &str, badge: Element) -> Element {
    div()
        .layer(format!("setting {name}"))
        .on_click(act)
        .self_stretch()
        .flex_row()
        .gap(12)
        .pad(11, 16)
        .items_center()
        .child(
            div()
                .flex_1()
                .flex_col()
                .gap(2)
                .child(div().font_weight(600).font_size(14).text(label))
                .child(div().font_size(12).color(palette::MUTED).text(what)),
        )
        .child(badge)
}
