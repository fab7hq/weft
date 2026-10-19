//! Settings: the machine's, for every project, on tabs. General holds Turbo,
//! Start at login, Notifications and Suggestions; Workflow who does each act
//! and the Eval debate. A project's own settings use the same rows, for what
//! differs from every project's. Each is set through the daemon and shows
//! what it last said.

use std::path::Path;

use serde_json::{Value, json};
use weft_core::words::{fill, word};

use super::SheetProps;
use super::menu::badge;
use crate::action::{Action, Focus, Setting, SettingsTab, SheetAct, WindowAct};
use crate::gpui::{Element, div, input_el, palette};
use crate::ui::{close_button, row_list, section_label, sheet, sheet_body, sheet_foot, sheet_head};

/// The tabs, in their order.
const TABS: [(SettingsTab, &str, &str); 3] = [
    (SettingsTab::General, "General", "desktop.settings.tab.general"),
    (SettingsTab::Workflow, "Workflow", "desktop.settings.tab.workflow"),
    (SettingsTab::RingFrame, "RingFrame", "desktop.settings.tab.ringframe"),
];

/// What a setting takes.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// A harness, or the agent you are in.
    Act,
    /// A harness, or where Eval runs.
    DebateHarness,
    /// An effort, or the harness's own.
    Effort,
    /// A model, typed; empty is the harness's own.
    Model,
}

/// One setting a project can have of its own, as Workflow shows it.
pub(super) struct Entry {
    pub key: &'static [&'static str],
    /// Its name in a row's layer, and in "+ Change a setting here".
    pub name: String,
    pub label: String,
    pub what: Option<String>,
    pub kind: Kind,
}

const ROLES: [(&str, &str, &str); 3] = [
    ("map", "Map", "desktop.settings.map"),
    ("reduce", "Reduce", "desktop.settings.reduce"),
    ("confirm", "Confirm", "desktop.settings.confirm"),
];

/// Every setting Workflow holds, in its order.
pub(super) fn entries() -> Vec<Entry> {
    let act = |key, name: &str, label: &str, what: &str| Entry {
        key,
        name: name.to_string(),
        label: label.to_string(),
        what: Some(word(what).to_string()),
        kind: Kind::Act,
    };
    let mut out = vec![
        act(&["routing", "ask"][..], "Ask", "Ask", "desktop.settings.ask_what"),
        act(&["routing", "eval"][..], "Eval", "Eval", "desktop.settings.eval_what"),
        act(&["routing", "seal"][..], "Seal", "Seal", "desktop.settings.seal_what"),
        Entry {
            key: &["eval", "debate", "harness"],
            name: "Runs in".to_string(),
            label: word("desktop.settings.runs_in").to_string(),
            what: Some(word("desktop.settings.runs_in_what").to_string()),
            kind: Kind::DebateHarness,
        },
    ];
    for (role, name, key) in ROLES {
        let role_key: &'static [&'static str] = match role {
            "map" => &["eval", "debate", "map", "model"],
            "reduce" => &["eval", "debate", "reduce", "model"],
            _ => &["eval", "debate", "confirm", "model"],
        };
        let effort_key: &'static [&'static str] = match role {
            "map" => &["eval", "debate", "map", "effort"],
            "reduce" => &["eval", "debate", "reduce", "effort"],
            _ => &["eval", "debate", "confirm", "effort"],
        };
        let shown = word(key);
        out.push(Entry {
            key: role_key,
            name: format!("{name} model"),
            label: fill("desktop.settings.model", &[("role", shown)]),
            what: None,
            kind: Kind::Model,
        });
        out.push(Entry {
            key: effort_key,
            name: format!("{name} effort"),
            label: fill("desktop.settings.effort", &[("role", shown)]),
            what: None,
            kind: Kind::Effort,
        });
    }
    out
}

pub(super) fn render(props: &SheetProps, tab: SettingsTab) -> Element {
    let body = match tab {
        SettingsTab::General => general(props),
        SettingsTab::Workflow => workflow(props),
        SettingsTab::RingFrame => super::ringframe::render(props),
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
        .child(label_col(label, Some(what)))
        .child(badge)
}

fn label_col(label: &str, what: Option<&str>) -> Element {
    let mut col = div()
        .flex_1()
        .flex_col()
        .gap(2)
        .child(div().font_weight(600).font_size(14).text(label.to_string()));
    if let Some(what) = what {
        col = col.child(div().font_size(12).color(palette::MUTED).text(what.to_string()));
    }
    col
}

fn workflow(props: &SheetProps) -> Element {
    let all = entries();
    let row_of = |e: &Entry| {
        div()
            .layer(format!("setting {}", e.name))
            .self_stretch()
            .flex_col()
            .child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(12)
                    .pad(10, 16)
                    .items_center()
                    .child(label_col(&e.label, e.what.as_deref()))
                    .child(control(props, None, e)),
            )
            .children(choices(props, None, e))
    };
    let acts: Vec<Element> = all.iter().take(3).map(row_of).collect();
    let mut debate = vec![row_of(&all[3])];
    // Each role: its model and its effort, on one row.
    for (i, (_, name, key)) in ROLES.iter().enumerate() {
        let (model, effort) = (&all[4 + 2 * i], &all[5 + 2 * i]);
        let what = word(&format!("{key}_what")).to_string();
        debate.push(
            div()
                .layer(format!("setting {name}"))
                .self_stretch()
                .flex_col()
                .child(
                    div()
                        .self_stretch()
                        .flex_row()
                        .gap(12)
                        .pad(10, 16)
                        .items_center()
                        .child(label_col(word(key), Some(&what)))
                        .child(
                            div()
                                .layer(format!("setting {}", model.name))
                                .child(control(props, None, model)),
                        )
                        .child(
                            div()
                                .layer(format!("setting {}", effort.name))
                                .child(control(props, None, effort)),
                        ),
                )
                .children(choices(props, None, effort)),
        );
    }
    sheet_body()
        .child(section_label(word("desktop.settings.acts")))
        .child(row_list(acts))
        .child(section_label(word("desktop.settings.debate")))
        .child(row_list(debate))
}

/// A setting as the daemon last said it: every project's, or `project`'s own.
pub(super) fn now(props: &SheetProps, project: Option<&Path>, key: &[&str]) -> Value {
    let config = match project {
        Some(root) => props.client(root).and_then(|c| c.config.as_ref()),
        None => props.clients.iter().find_map(|c| c.config.as_ref()),
    };
    let mut v =
        config.map_or(&Value::Null, |c| &c[if project.is_some() { "project" } else { "machine" }]);
    for k in key {
        v = &v[*k];
    }
    v.clone()
}

/// What a value of `kind` reads as: a harness by its title.
pub(super) fn shown(props: &SheetProps, kind: Kind, v: &Value) -> String {
    match (kind, v.as_str()) {
        (Kind::Act, None) => word("desktop.settings.agent_you_are_in").to_string(),
        (Kind::DebateHarness, None) => word("desktop.settings.where_eval_runs").to_string(),
        (Kind::Effort, None) => word("desktop.settings.own_effort").to_string(),
        (Kind::Model, None) => word("desktop.settings.own_model").to_string(),
        (Kind::Act | Kind::DebateHarness, Some(h)) => {
            props.harnesses.iter().find(|x| x.name == h).map_or(h.to_string(), |x| x.title.clone())
        }
        (_, Some(s)) => s.to_string(),
    }
}

/// A setting's control: what it says now; a choice opens its list, a model
/// its field.
pub(super) fn control(props: &SheetProps, project: Option<&Path>, e: &Entry) -> Element {
    let key: Vec<String> = e.key.iter().map(|k| k.to_string()).collect();
    let v = now(props, project, e.key);
    let editing = props.setting_editing.is_some_and(|(p, k)| p.as_deref() == project && *k == key);
    let boxed = div()
        .layer("setting value")
        .w(if e.kind == Kind::Effort { 110 } else { 190 })
        .pad(6, 10)
        .bg(palette::SURFACE_ALT)
        .border(if project.is_some() { palette::BLUE_BORDER } else { palette::LINE })
        .font_size(13);
    if e.kind == Kind::Model {
        if editing {
            return boxed.child(
                input_el(Focus::Setting).text(props.setting_text.to_string()).font_size(13),
            );
        }
        let colour = if v.is_null() { palette::DIM } else { palette::TEXT };
        let project = project.map(Path::to_path_buf);
        return boxed
            .on_click(SheetAct::EditSetting { project, key })
            .color(colour)
            .text(shown(props, e.kind, &v));
    }
    let colour = if project.is_some() { palette::BLUE_BORDER } else { palette::TEXT };
    boxed
        .on_click(SheetAct::OpenChoice(key))
        .color(colour)
        .text(format!("{} ▾", shown(props, e.kind, &v)))
}

/// A setting's choices, when they are open: each sets it.
pub(super) fn choices(props: &SheetProps, project: Option<&Path>, e: &Entry) -> Option<Element> {
    let dotted = e.key.join(".");
    if props.choice != Some(dotted.as_str()) || e.kind == Kind::Model {
        return None;
    }
    let none = shown(props, e.kind, &Value::Null);
    let mut options: Vec<(String, Option<Value>)> = vec![(none, None)];
    match e.kind {
        Kind::Act | Kind::DebateHarness => {
            options.extend(props.harnesses.iter().map(|h| (h.title.clone(), Some(json!(h.name)))))
        }
        Kind::Effort => options.extend(
            ["low", "medium", "high", "xhigh"].iter().map(|x| (x.to_string(), Some(json!(x)))),
        ),
        Kind::Model => {}
    }
    let key: Vec<String> = e.key.iter().map(|k| k.to_string()).collect();
    let mut list = div()
        .layer(format!("choices {dotted}"))
        .self_stretch()
        .flex_col()
        .bg(palette::SURFACE_ALT)
        .border_top(palette::LINE);
    for (title, value) in options {
        list = list.child(
            div()
                .layer(format!("choice {title}"))
                .on_click(SheetAct::Choose {
                    project: project.map(Path::to_path_buf),
                    key: key.clone(),
                    value,
                })
                .self_stretch()
                .pad(8, 32)
                .font_size(13)
                .text(title),
        );
    }
    Some(list)
}
