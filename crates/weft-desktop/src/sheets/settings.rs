//! Settings: the machine's, for every project, in sections listed beside the
//! one shown. General holds the machine's name, Start Weft at login, Notify
//! me, how long finished sessions are kept, and Turbo; Eval how carefully it
//! reads and who reads; Rules and Agent apps their own files. Advanced
//! settings shows each section's expert controls in place. A project's own
//! settings use the same controls, for what differs from every project's.
//! Each is set through the daemon and shows what it last said.

use std::path::Path;

use serde_json::{Value, json};
use weft_core::words::{fill, word};

use super::SheetProps;
use crate::action::{Action, Focus, Setting, SettingsTab, SheetAct, WindowAct};
use crate::gpui::{Element, div, input_el, palette};
use crate::ui::{close_button, section_label, sheet, sheet_foot};

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

const ROLES: [(&str, &str, &str); 2] = [
    ("read", "Read", "desktop.settings.read"),
    ("question", "Questions", "desktop.settings.question"),
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
            "read" => &["eval", "debate", "read", "model"],
            _ => &["eval", "debate", "question", "model"],
        };
        let effort_key: &'static [&'static str] = match role {
            "read" => &["eval", "debate", "read", "effort"],
            _ => &["eval", "debate", "question", "effort"],
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
        SettingsTab::Eval => eval(props),
        SettingsTab::Rules => super::ringframe::render(props),
        SettingsTab::Apps => super::harnesses::render(props),
    };
    let title = SECTIONS.iter().find(|(t, ..)| *t == tab).map_or("", |(_, _, key)| word(key));
    // Advanced settings on: the badge beside the title, and a blue line under it.
    let head = div()
        .layer("settings head")
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(12, 20)
        .items_center()
        .border_bottom(if props.advanced { palette::BLUE_BORDER } else { palette::LINE })
        .child(div().font_weight(600).font_size(16).text(word("web.settings.title")))
        .children(props.advanced.then(advanced_mark));
    let shown = div()
        .layer("sheet body")
        .flex_1()
        .self_stretch()
        .flex_col()
        .style("overflow-y", "auto")
        .style("min-width", "0px")
        .pad(18, 24)
        .child(
            div()
                .layer(format!("settings section {title}"))
                .self_stretch()
                .flex_col()
                .style("max-width", "640px")
                .child(div().font_weight(600).font_size(16).text(title.to_string()))
                .child(body),
        );
    sheet(
        "sheet Settings",
        head,
        div()
            .self_stretch()
            .flex_1()
            .flex_row()
            .style("min-height", "0px")
            .child(nav(props, tab))
            .child(shown),
        sheet_foot(Vec::new(), vec![close_button()]),
    )
}

/// The sections, in their order: each by its name in a layer, and its word.
const SECTIONS: [(SettingsTab, &str, &str); 4] = [
    (SettingsTab::General, "General", "settings.general"),
    (SettingsTab::Eval, "Eval", "settings.eval"),
    (SettingsTab::Rules, "Rules", "settings.rules"),
    (SettingsTab::Apps, "Agent apps", "settings.apps"),
];

/// The badge Settings carries while Advanced settings is on.
fn advanced_mark() -> Element {
    div()
        .layer("advanced mark")
        .pad(2, 7)
        .bg(palette::BLUE_BORDER)
        .color(palette::CANVAS)
        .font_mono()
        .font_size(10)
        .font_weight(700)
        .text(word("settings.advanced_mark"))
}

/// The sections listed beside the one shown, and Advanced settings' switch at
/// their foot.
fn nav(props: &SheetProps, shown: SettingsTab) -> Element {
    let needing = super::harnesses::needing(props);
    let mut list = div()
        .layer("settings sections")
        .w(200)
        .shrink_0()
        .flex_col()
        .pad(12, 0)
        .border_right(palette::LINE);
    for (tab, name, key) in SECTIONS {
        let on = tab == shown;
        let mut item = div()
            .layer(format!("settings tab {name}"))
            .on_click(SheetAct::SettingsTab(tab))
            .self_stretch()
            .flex_row()
            .items_center()
            .pad(8, 16)
            .font_size(13)
            .font_weight(if on { 600 } else { 400 })
            .color(if on { palette::TEXT } else { palette::MUTED })
            .bg(if on { palette::SELECTED_BG } else { "transparent" })
            .style(
                "border-left",
                format!("2px solid {}", if on { palette::BLUE_BORDER } else { "transparent" }),
            )
            .child(div().flex_1().text(word(key)));
        if tab == SettingsTab::Apps && needing > 0 {
            item = item.child(
                div()
                    .layer("apps needing")
                    .font_mono()
                    .font_size(11)
                    .color(palette::AMBER)
                    .text(format!("■ {needing}")),
            );
        }
        list = list.child(item);
    }
    list.child(div().flex_1()).child(
        div()
            .layer("setting Advanced settings")
            .on_click(SheetAct::Advanced)
            .self_stretch()
            .flex_row()
            .items_center()
            .gap(10)
            .pad(12, 16)
            .border_top(palette::LINE)
            .child(
                div().flex_1().font_size(13).color(palette::MUTED).text(word("settings.advanced")),
            )
            .child(toggle(props.advanced, false)),
    )
}

/// A switch, as the cards draw it: a track, its knob at the right when on.
/// Turbo's is amber.
pub(super) fn toggle(on: bool, amber: bool) -> Element {
    let (track, line) = match (on, amber) {
        (false, _) => ("#1d2a40", palette::LINE),
        (true, false) => (palette::BLUE, palette::BLUE_BORDER),
        (true, true) => ("#b07b16", palette::AMBER_BORDER),
    };
    let knob =
        div().layer(if on { "switch on" } else { "switch off" }).w(14).h(14).bg(palette::TEXT);
    let mut el = div()
        .layer("setting switch")
        .shrink_0()
        .w(34)
        .h(20)
        .pad(2, 2)
        .flex_row()
        .items_center()
        .bg(track)
        .border(line)
        .child(knob);
    if on {
        el = el.justify_end();
    }
    el
}

/// A section's group of rows: its name in the eyebrow's capitals, marked when
/// only Advanced settings shows it, and its one act on the right.
pub(super) fn group(name: impl Into<String>, more: bool, act: Option<Element>) -> Element {
    let mut label = div().flex_1().flex_row().items_center().gap(8).child(section_label(name));
    if more {
        label = label.child(
            div()
                .pad(1, 5)
                .border(palette::BLUE_BORDER)
                .font_mono()
                .font_size(9)
                .color(palette::BLUE_BORDER)
                .text(word("settings.advanced_mark")),
        );
    }
    div()
        .self_stretch()
        .flex_row()
        .items_center()
        .style("padding", "16px 0 6px")
        .child(label)
        .children(act)
}

/// One row: its name and what it does, then its controls; a line under it.
pub(super) fn line_row(layer: impl Into<String>, label: &str, what: Option<&str>) -> Element {
    div()
        .layer(layer)
        .self_stretch()
        .flex_row()
        .gap(12)
        .pad(10, 0)
        .items_center()
        .border_bottom(palette::LINE)
        .child(label_col(label, what))
}

/// A link that acts: `+ Add`, Reset, Edit, Remove.
pub(super) fn link(layer: &str, act: impl Into<Action>, text: &str) -> Element {
    div()
        .layer(layer)
        .on_click(act)
        .font_size(12)
        .color(palette::BLUE_BORDER)
        .text(text.to_string())
}

/// General: the machine's name, Start Weft at login, Notify me, how long
/// finished sessions stay, and Turbo.
fn general(props: &SheetProps) -> Element {
    let config = props.clients.iter().find_map(|c| c.config.as_ref());
    let machine = |key: &str| config.and_then(|c| c["machine"][key].as_bool());
    let start_at_login = props.clients.iter().any(|c| c.start_at_login);
    let login_what = fill("settings.login_what", &[("machine", props.machine)]);
    let switched = |name: &str, act: Action, label: &str, what: &str, on: bool, amber: bool| {
        line_row(format!("setting {name}"), label, Some(what))
            .on_click(act)
            .child(toggle(on, amber))
    };
    div()
        .self_stretch()
        .flex_col()
        .child(group(props.machine, false, None))
        .child(name_row(props))
        .child(switched(
            "Start at login",
            WindowAct::ToggleStartAtLogin.into(),
            word("settings.login"),
            &login_what,
            start_at_login,
            false,
        ))
        .child(switched(
            "Notifications",
            SheetAct::Toggle(Setting::Notify).into(),
            word("settings.notify"),
            word("settings.notify_what"),
            machine("notify") != Some(false),
            false,
        ))
        .child(keep_row(props))
        .child(group(word("settings.agents"), false, None))
        .child(switched(
            "Turbo",
            WindowAct::ToggleTurbo.into(),
            word("desktop.menu.turbo"),
            word("settings.turbo_what"),
            machine("turbo") == Some(true),
            true,
        ))
}

/// A setting's value in its box: a field while it is typed, else what it
/// says, which opens it.
fn boxed_field(props: &SheetProps, key: Vec<String>, said: String, width: u32) -> Element {
    let editing = props.setting_editing.is_some_and(|(p, k)| p.is_none() && *k == key);
    let field = div()
        .layer("setting value")
        .w(width)
        .pad(6, 10)
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .font_size(13)
        .style("min-width", "0px");
    if editing {
        field.child(input_el(Focus::Setting).text(props.setting_text.to_string()).font_size(13))
    } else {
        field
            .on_click(SheetAct::EditSetting { project: None, key })
            .color(palette::TEXT)
            .style("white-space", "nowrap")
            .style("overflow", "hidden")
            .style("text-overflow", "ellipsis")
            .text(said)
    }
}

/// The machine's name: what Weft and the person's devices call it, typed
/// in its field and kept on Enter. Of any length, it ends in an ellipsis.
fn name_row(props: &SheetProps) -> Element {
    let field = boxed_field(props, vec!["name".to_string()], props.machine.to_string(), 220);
    line_row("setting Name", word("desktop.settings.name"), Some(word("settings.name_what")))
        .child(field)
}

/// Keep finished sessions' choices: a number of days and its word; 0 keeps
/// them all.
const KEEP: [(u64, &str); 4] = [
    (3, "settings.keep.3"),
    (7, "settings.keep.7"),
    (30, "settings.keep.30"),
    (0, "settings.keep.forever"),
];

/// Keep finished sessions: 3 days, a week, a month, Custom or Keep forever.
/// Custom shows a field of days beside the choice, kept on Enter.
fn keep_row(props: &SheetProps) -> Element {
    let key = vec!["sessions".to_string(), "keep_days".to_string()];
    let days = props
        .clients
        .iter()
        .find_map(|c| c.config.as_ref()?["effective"]["keep_days"].as_u64())
        .unwrap_or(weft_core::config::KEEP_DAYS);
    let preset = KEEP.iter().find(|(d, _)| *d == days).filter(|_| !props.keep_custom);
    let said = preset.map_or(word("settings.keep.custom"), |(_, w)| word(w));
    let dotted = key.join(".");
    let pick = div()
        .layer("setting value")
        .on_click(SheetAct::OpenChoice(key.clone()))
        .w(150)
        .pad(6, 10)
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .font_size(13)
        .text(format!("{said} ▾"));
    let mut row = line_row(
        "setting Keep finished sessions",
        word("settings.keep"),
        Some(word("settings.keep_what")),
    )
    .child(pick);
    if preset.is_none() {
        row = row
            .child(boxed_field(props, key.clone(), days.to_string(), 70).layer("setting days"))
            .child(div().font_size(13).color(palette::DIM).text(word("settings.days")));
    }
    let list = (props.choice == Some(dotted.as_str())).then(|| {
        let mut list = div()
            .layer(format!("choices {dotted}"))
            .self_stretch()
            .flex_col()
            .bg(palette::SURFACE_ALT)
            .border_bottom(palette::LINE);
        let option = |title: &str, act: SheetAct| {
            div()
                .layer(format!("choice {title}"))
                .on_click(act)
                .self_stretch()
                .pad(8, 16)
                .font_size(13)
                .text(title.to_string())
        };
        for (d, w) in &KEEP[..3] {
            let act = SheetAct::Choose { project: None, key: key.clone(), value: Some(json!(d)) };
            list = list.child(option(word(w), act));
        }
        list = list.child(option(word("settings.keep.custom"), SheetAct::KeepCustom));
        let forever = SheetAct::Choose { project: None, key: key.clone(), value: Some(json!(0)) };
        list.child(option(word("settings.keep.forever"), forever))
    });
    div().self_stretch().flex_col().child(row).children(list)
}

/// One switch: its name and what it does, and its badge; the row turns it.
pub(super) fn switch(
    name: &str,
    act: impl Into<Action>,
    label: &str,
    what: &str,
    badge: Element,
) -> Element {
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
        .style("min-width", "0px")
        .child(div().font_size(14).text(label.to_string()));
    if let Some(what) = what {
        col = col.child(div().font_size(12).color(palette::MUTED).text(what.to_string()));
    }
    col
}

/// Eval: how carefully it reads, which agent app reads, and what it never
/// does; Advanced settings adds each reader and who runs each step.
fn eval(props: &SheetProps) -> Element {
    let all = entries();
    let row_of = |e: &Entry, named: Option<(&str, &str)>| {
        let (label, what) = named.map_or((e.label.clone(), e.what.clone()), |(l, w)| {
            (word(l).into(), Some(word(w).into()))
        });
        div()
            .self_stretch()
            .flex_col()
            .child(
                line_row(format!("setting {}", e.name), &label, what.as_deref())
                    .child(control(props, None, e)),
            )
            .children(choices(props, None, e))
    };
    let mut body = div()
        .self_stretch()
        .flex_col()
        .child(group(word("settings.eval.care"), false, None))
        .child(care(props))
        .child(group(word("settings.eval.who"), false, None))
        .child(row_of(&all[3], Some(("settings.eval.read_by", "settings.eval.read_by_what"))));
    if props.advanced {
        body = body.child(group(word("settings.eval.each"), true, None));
        // Each reader on one row: its model and its effort.
        for (i, (_, name, key)) in ROLES.iter().enumerate() {
            let (model, effort) = (&all[4 + 2 * i], &all[5 + 2 * i]);
            let what = word(&format!("{key}_what")).to_string();
            body = body.child(
                div()
                    .layer(format!("setting {name}"))
                    .self_stretch()
                    .flex_col()
                    .child(
                        line_row(format!("reader {name}"), word(key), Some(&what))
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
        body = body
            .child(group(word("settings.eval.steps"), true, None))
            .child(row_of(&all[0], None))
            .child(row_of(&all[1], Some(("act.eval", "settings.eval.eval_what"))))
            .child(row_of(&all[2], None));
    }
    body.child(super::ringframe::eval_never(props))
}

/// How carefully Eval reads: Quick, Balanced or Thorough for every reader,
/// or Custom once a reader is set on its own.
fn care(props: &SheetProps) -> Element {
    let on = how_hard_now(props);
    let mut cards = div().layer("setting How hard").self_stretch().flex_row().gap(10);
    let options = [
        ("quick", "desktop.settings.quick", "settings.eval.quick_what"),
        ("balanced", "desktop.settings.balanced", "settings.eval.balanced_what"),
        ("thorough", "desktop.settings.thorough", "settings.eval.thorough_what"),
        ("custom", "desktop.settings.custom", "settings.eval.custom_what"),
    ];
    for (how, name, what) in options.into_iter().filter(|(h, ..)| *h != "custom" || on == "custom")
    {
        let chosen = on == how;
        let mut card = div()
            .layer(if chosen { "how hard on".to_string() } else { format!("how hard {how}") })
            .flex_1()
            .flex_col()
            .gap(3)
            .pad(10, 12)
            .bg(if chosen { palette::SELECTED_BG } else { palette::SURFACE })
            .border(if chosen { palette::BLUE_BORDER } else { palette::LINE })
            .child(
                div()
                    .font_size(14)
                    .font_weight(600)
                    .color(if chosen { palette::BLUE_BORDER } else { palette::TEXT })
                    .text(word(name)),
            )
            .child(div().font_size(11).color(palette::MUTED).text(word(what)));
        if !chosen && how != "custom" {
            card = card.on_click(SheetAct::HowHard(how.to_string()));
        }
        cards = cards.child(card);
    }
    cards
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

/// The reader a setting's key is about: `read` or `question`.
fn key_role<'a>(key: &[&'a str]) -> &'a str {
    key.get(2).copied().unwrap_or_default()
}

/// A judge's default effort, from its harness's `[eval]`.
fn default_of<'a>(judge: &'a weft_core::harness::Eval, role: &str) -> Option<&'a str> {
    let d = &judge.default;
    match role {
        "read" => Some(&d.read),
        "question" => Some(&d.question),
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
