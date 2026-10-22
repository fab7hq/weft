//! Settings: the machine's, for every project, on tabs. General holds Turbo,
//! Start at login, Notifications and Suggestions; Workflow who runs each step
//! and Eval's judges. A project's own settings use the same rows, for what
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
const TABS: [(SettingsTab, &str, &str); 4] = [
    (SettingsTab::General, "General", "desktop.settings.tab.general"),
    (SettingsTab::Workflow, "Workflow", "desktop.settings.tab.workflow"),
    (SettingsTab::RingFrame, "RingFrame", "desktop.settings.tab.ringframe"),
    (SettingsTab::Harnesses, "Harnesses", "desktop.settings.tab.harnesses"),
];

/// What a setting takes.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    /// A harness, or the agent you are in.
    Act,
    /// A harness, or where Eval runs.
    DebateHarness,
    /// An effort, or the judge's default.
    Effort,
    /// A model, picked or typed as the judges' harness takes one; empty is the
    /// session's.
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
            name: word("desktop.settings.runs_in").to_string(),
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
        SettingsTab::Harnesses => super::harnesses::render(props),
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
    let mut judges = vec![row_of(&all[3]), how_hard(props)];
    judges.push(
        div()
            .layer("setting Each judge")
            .on_click(SheetAct::ShowJudges)
            .self_stretch()
            .pad(10, 16)
            .font_size(13)
            .color(palette::BLUE_BORDER)
            .text(format!(
                "{} {}",
                if props.judges_shown { "⌄" } else { "›" },
                word("desktop.settings.each_judge")
            )),
    );
    // Each judge, once opened: its model and its effort, on one row.
    for (i, (_, name, key)) in ROLES.iter().enumerate().filter(|_| props.judges_shown) {
        let (model, effort) = (&all[4 + 2 * i], &all[5 + 2 * i]);
        let what = word(&format!("{key}_what")).to_string();
        judges.push(
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
                        // Under "Each judge", a step in.
                        .child(div().w(4))
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
                .children(choices(props, None, model))
                .children(choices(props, None, effort)),
        );
    }
    sheet_body()
        .child(section_label(word("desktop.settings.acts")))
        .child(row_list(acts))
        .child(section_label(word("desktop.settings.debate")))
        .child(row_list(judges))
        .child(
            div()
                .self_stretch()
                .pad(10, 0)
                .font_size(12)
                .color(palette::DIM)
                .text(word("desktop.settings.workflow_foot").to_string()),
        )
}

/// How hard the judges look, one choice for all three, and what an effort
/// changes in the harness they run in.
fn how_hard(props: &SheetProps) -> Element {
    let on = how_hard_now(props);
    let mut seg = div().flex_row().border(palette::LINE);
    let choices = [
        ("quick", "desktop.settings.quick"),
        ("balanced", "desktop.settings.balanced"),
        ("thorough", "desktop.settings.thorough"),
    ];
    for (how, key) in choices {
        let chosen = on == how;
        let mut part = div()
            .layer(if chosen { "how hard on".to_string() } else { format!("how hard {how}") })
            .pad(6, 14)
            .font_size(13)
            .border_right(palette::LINE)
            .color(if chosen { palette::BLUE_BORDER } else { palette::MUTED })
            .text(word(key).to_string());
        if chosen {
            part = part.bg(palette::SURFACE_ALT).font_weight(600);
        } else {
            part = part.on_click(SheetAct::HowHard(how.to_string()));
        }
        seg = seg.child(part);
    }
    if on == "custom" {
        seg = seg.child(
            div()
                .layer("how hard on")
                .pad(6, 14)
                .font_size(13)
                .bg(palette::SURFACE_ALT)
                .color(palette::BLUE_BORDER)
                .text(word("desktop.settings.custom").to_string()),
        );
    }
    let (title, judge) =
        judges_harness(props).map_or((String::new(), None), |h| (h.title.clone(), h.eval.as_ref()));
    let reach = match judge.map(|j| j.effort) {
        Some(weft_core::harness::EffortReaches::Model) => "desktop.settings.effort_reaches_model",
        Some(weft_core::harness::EffortReaches::Lookups) => {
            "desktop.settings.effort_reaches_lookups"
        }
        None => "desktop.settings.effort_reaches_unknown",
    };
    div()
        .layer("setting How hard")
        .self_stretch()
        .flex_col()
        .gap(6)
        .pad(10, 16)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .child(label_col(word("desktop.settings.how_hard"), None))
                .child(seg),
        )
        .child(
            div()
                .font_size(12)
                .color(palette::MUTED)
                .text(fill(reach, &[("harness", title.as_str())])),
        )
}

/// The choice the three judges' efforts make: `quick`, `balanced`,
/// `thorough`, or `custom`.
fn how_hard_now(props: &SheetProps) -> &'static str {
    let set: Vec<Value> = ROLES
        .iter()
        .map(|(role, _, _)| now(props, None, &["eval", "debate", role, "effort"]))
        .collect();
    let all = |v: Value| set.iter().all(|x| *x == v);
    if all(Value::Null) {
        "balanced"
    } else if all(json!("low")) {
        "quick"
    } else if all(json!("high")) {
        "thorough"
    } else {
        "custom"
    }
}

/// The harness Eval's judges run in: the debate's, else Eval's.
fn judges_harness<'a>(props: &'a SheetProps) -> Option<&'a weft_core::harness::Harness> {
    let set = now(props, None, &["eval", "debate", "harness"]);
    let set = if set.is_null() { now(props, None, &["routing", "eval"]) } else { set };
    let name = set.as_str()?;
    props.harnesses.iter().find(|h| h.name == name)
}

/// What a judge in that harness takes, from its harness file.
fn judge<'a>(props: &'a SheetProps) -> Option<&'a weft_core::harness::Eval> {
    judges_harness(props)?.eval.as_ref()
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

/// The judge a setting's key is about: `map`, `reduce` or `confirm`.
fn key_role<'a>(key: &[&'a str]) -> &'a str {
    key.get(2).copied().unwrap_or_default()
}

/// A judge's default effort, from its harness's `[eval]`.
fn default_of<'a>(judge: &'a weft_core::harness::Eval, role: &str) -> Option<&'a str> {
    let d = &judge.default;
    match role {
        "map" => Some(&d.map),
        "reduce" => Some(&d.reduce),
        "confirm" => Some(&d.confirm),
        _ => None,
    }
    .map(String::as_str)
}

/// What a value of `kind` (at `key`) reads as: a harness by its title, and
/// nothing set by what applies then.
pub(super) fn shown(props: &SheetProps, kind: Kind, key: &[&str], v: &Value) -> String {
    match (kind, v.as_str()) {
        (Kind::Act, None) => word("desktop.settings.agent_you_are_in").to_string(),
        (Kind::DebateHarness, None) => word("desktop.settings.where_eval_runs").to_string(),
        (Kind::Effort, None) => {
            // The judge's default, by its value, where the harness says it.
            let role = key_role(key);
            match judge(props).and_then(|j| default_of(j, role)) {
                Some(effort) => fill("desktop.settings.effort_default", &[("effort", effort)]),
                None => word("desktop.settings.own_effort").to_string(),
            }
        }
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
        .w(if e.kind == Kind::Effort { 160 } else { 190 })
        .pad(6, 10)
        .bg(palette::SURFACE_ALT)
        .border(if project.is_some() { palette::BLUE_BORDER } else { palette::LINE })
        .font_size(13);
    let judge = judge(props);
    let typed = judge.is_none_or(|j| j.model_typed);
    if e.kind == Kind::Model && !typed && judge.is_some_and(|j| j.models.is_empty()) {
        // A judge here always runs on the session's model.
        return boxed.color(palette::DIM).text(shown(props, e.kind, e.key, &v));
    }
    if e.kind == Kind::Model && typed {
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
            .text(shown(props, e.kind, e.key, &v));
    }
    let colour = if project.is_some() { palette::BLUE_BORDER } else { palette::TEXT };
    boxed
        .on_click(SheetAct::OpenChoice(key))
        .color(colour)
        .text(format!("{} ▾", shown(props, e.kind, e.key, &v)))
}

/// A setting's choices, when they are open: each sets it.
pub(super) fn choices(props: &SheetProps, project: Option<&Path>, e: &Entry) -> Option<Element> {
    let dotted = e.key.join(".");
    if props.choice != Some(dotted.as_str()) {
        return None;
    }
    let judge = judge(props);
    let none = shown(props, e.kind, e.key, &Value::Null);
    let mut options: Vec<(String, Option<Value>)> = vec![(none, None)];
    match e.kind {
        Kind::Act | Kind::DebateHarness => {
            options.extend(props.harnesses.iter().map(|h| (h.title.clone(), Some(json!(h.name)))))
        }
        Kind::Effort => {
            let common = ["low", "medium", "high", "xhigh"].map(String::from);
            let efforts = judge.map_or(&common[..], |j| &j.efforts[..]);
            options.extend(efforts.iter().map(|x| (x.clone(), Some(json!(x)))))
        }
        Kind::Model => {
            let models = judge.map(|j| &j.models[..]).unwrap_or_default();
            options.extend(models.iter().map(|x| (x.clone(), Some(json!(x)))))
        }
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
