//! A project's own settings, from its row's ⋯: only what differs from every
//! project's, each beside every project's value, with Reset; "+ Change a
//! setting here" adds one. Turbo is every project's only, so never here.

use std::path::Path;

use serde_json::Value;
use weft_core::words::{fill, word};

use super::SheetProps;
use super::settings::{Entry, choices, control, entries, now, shown};
use crate::action::SheetAct;
use crate::gpui::{Element, div, palette};
use crate::ui::{close_button, note, row_list, sheet, sheet_body, sheet_foot, sheet_head};

pub(super) fn render(props: &SheetProps, project: &Path) -> Element {
    let name = props.listed(project).map_or_else(
        || project.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        |l| l.project.name.clone(),
    );
    let all = entries();
    let adding = props.choice == Some("add");
    // What this project sets, and the one whose choices were opened to add it.
    let shown_here: Vec<&Entry> = all
        .iter()
        .filter(|e| {
            !now(props, Some(project), e.key).is_null()
                || props.choice == Some(e.key.join(".").as_str())
        })
        .collect();
    let mut rows: Vec<Element> = shown_here.iter().map(|e| row(props, project, e)).collect();
    rows.extend(ringframe_rows(props, project));
    let mut body = sheet_body();
    if rows.is_empty() {
        body =
            body.child(note(fill("desktop.project.none", &[("project", &name)]), palette::MUTED));
    } else {
        body = body.child(row_list(rows));
    }
    body = body.child(
        div()
            .layer("project add")
            .on_click(SheetAct::AddProjectSetting)
            .font_size(13)
            .color(palette::BLUE_BORDER)
            .text(word("desktop.project.add")),
    );
    if adding {
        let mut list = div().layer("choices add").self_stretch().flex_col().border(palette::LINE);
        let addable = all
            .iter()
            .filter(|e| now(props, Some(project), e.key).is_null())
            .map(|e| {
                (e.name.clone(), e.label.clone(), e.key.iter().map(|k| k.to_string()).collect())
            })
            .chain(ringframe_addable(props, project).into_iter().map(|(l, k)| (l.clone(), l, k)));
        for (name, label, key) in addable {
            list = list.child(
                div()
                    .layer(format!("choice {name}"))
                    .on_click(SheetAct::OpenChoice(key))
                    .self_stretch()
                    .pad(8, 14)
                    .font_size(13)
                    .text(label),
            );
        }
        body = body.child(list);
    }
    let key = crate::keys::hint_here(crate::keys::Shortcut::Settings);
    body = body.child(note(fill("desktop.project.foot", &[("key", &key)]), palette::DIM));
    sheet(
        "sheet Project settings",
        sheet_head(
            fill("desktop.project.title", &[("project", &name)]),
            Some((word("desktop.project.differs").to_string(), palette::MUTED)),
        ),
        body,
        sheet_foot(Vec::new(), vec![close_button()]),
    )
}

/// One setting of the project's: every project's value beside it, and Reset.
fn row(props: &SheetProps, project: &Path, e: &Entry) -> Element {
    let every = shown(props, e.kind, &now(props, None, e.key));
    let key: Vec<String> = e.key.iter().map(|k| k.to_string()).collect();
    div()
        .layer(format!("project setting {}", e.key.join(".")))
        .self_stretch()
        .flex_col()
        .child(
            div()
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
                        .child(div().font_weight(600).font_size(14).text(e.label.clone()))
                        .child(
                            div()
                                .font_size(12)
                                .color(palette::MUTED)
                                .text(fill("desktop.project.every", &[("value", &every)])),
                        ),
                )
                .child(control(props, Some(project), e))
                .child(
                    div()
                        .layer("reset")
                        .on_click(SheetAct::Choose {
                            project: Some(project.to_path_buf()),
                            key,
                            value: None::<Value>,
                        })
                        .font_size(12)
                        .color(palette::BLUE_BORDER)
                        .text(word("act.reset")),
                ),
        )
        .children(choices(props, Some(project), e))
}

/// RingFrame as this project changes it: the core rules per phase, each rule,
/// what judges must not do; each beside every project's, with Reset. A rule or
/// the cap whose choices were opened to add it is shown too.
pub(super) fn ringframe_rows(props: &SheetProps, project: &Path) -> Vec<Element> {
    use crate::daemon::Then;
    use serde_json::json;
    const BASE: &str = "practices/software-development";
    let read =
        |mine: bool| props.client(project).and_then(|c| c.read.get(&Then::Rules { project: mine }));
    let base_of = |v: Option<&Value>| {
        v.and_then(|r| r["catalogs"].as_array())
            .and_then(|cs| cs.iter().find(|c| c["catalog"] == BASE))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let (here, every) = (base_of(read(true)), base_of(read(false)));
    let mine = now(props, Some(project), &["ringframe"]);
    let key = |parts: &[&str]| parts.iter().map(|p| p.to_string()).collect::<Vec<_>>();
    let mut rows = Vec::new();

    let cap_key = key(&["ringframe", "deltas", BASE, "render", "core_cap"]);
    let cap_dotted = cap_key.join(".");
    if !mine["deltas"][BASE]["render"]["core_cap"].is_null()
        || props.choice == Some(cap_dotted.as_str())
    {
        let options = (1..=9)
            .map(|n| {
                (
                    n.to_string(),
                    SheetAct::Choose {
                        project: Some(project.to_path_buf()),
                        key: cap_key.clone(),
                        value: Some(json!(n)),
                    },
                )
            })
            .collect();
        rows.push(ring_row(
            props,
            "ringframe.core_cap",
            word("desktop.settings.core_cap").to_string(),
            every["render"]["core_cap"].as_u64().unwrap_or(5).to_string(),
            here["render"]["core_cap"].as_u64().unwrap_or(5).to_string(),
            (cap_key, project),
            options,
        ));
    }
    let set_here: Vec<String> = mine["deltas"][BASE]["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str().map(str::to_string))
        .collect();
    for e in here["entries"].as_array().into_iter().flatten() {
        let id = e["id"].as_str().unwrap_or_default();
        let entry_key = key(&["ringframe", "deltas", BASE, "entries", id]);
        if !set_here.iter().any(|s| s == id) && props.choice != Some(entry_key.join(".").as_str()) {
            continue;
        }
        let said = |e: &Value| {
            let on = if e["enabled"] == false { "off" } else { "on" };
            format!("{}, {on}", e["tier"].as_str().unwrap_or("core"))
        };
        let theirs = every["entries"]
            .as_array()
            .and_then(|es| es.iter().find(|x| x["id"] == id))
            .map_or_else(|| said(e), said);
        let options =
            [("core", true), ("core", false), ("situational", true), ("situational", false)]
                .iter()
                .map(|(tier, on)| {
                    let title = format!("{tier}, {}", if *on { "on" } else { "off" });
                    let value = Some(json!({"tier": tier, "enabled": on}));
                    (
                        title,
                        SheetAct::Choose {
                            project: Some(project.to_path_buf()),
                            key: entry_key.clone(),
                            value,
                        },
                    )
                })
                .collect();
        let label = format!("RingFrame · {}", e["label"].as_str().unwrap_or(id));
        rows.push(ring_row(
            props,
            &format!("ringframe.{id}"),
            label,
            theirs,
            said(e),
            (entry_key, project),
            options,
        ));
    }
    if let Some(deny) = mine["eval"]["deny"].as_array() {
        let n = deny.len().to_string();
        rows.push(ring_row(
            props,
            "ringframe.deny",
            word("desktop.settings.judges").to_string(),
            word("desktop.settings.judges_own").to_string(),
            n,
            (key(&["ringframe", "eval", "deny"]), project),
            Vec::new(),
        ));
    }
    rows
}

/// What can be added here from RingFrame: the cap, and each rule not set.
pub(super) fn ringframe_addable(props: &SheetProps, project: &Path) -> Vec<(String, Vec<String>)> {
    use crate::daemon::Then;
    const BASE: &str = "practices/software-development";
    let mine = now(props, Some(project), &["ringframe"]);
    let mut out = Vec::new();
    if mine["deltas"][BASE]["render"]["core_cap"].is_null() {
        out.push((
            word("desktop.settings.core_cap").to_string(),
            ["ringframe", "deltas", BASE, "render", "core_cap"].map(String::from).to_vec(),
        ));
    }
    let rules = props.client(project).and_then(|c| c.read.get(&Then::Rules { project: true }));
    let set: Vec<&str> = mine["deltas"][BASE]["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    for c in rules
        .and_then(|r| r["catalogs"].as_array())
        .into_iter()
        .flatten()
        .filter(|c| c["catalog"] == BASE)
    {
        for e in c["entries"].as_array().into_iter().flatten() {
            let id = e["id"].as_str().unwrap_or_default();
            if !set.contains(&id) {
                let label = format!("RingFrame · {}", e["label"].as_str().unwrap_or(id));
                out.push((
                    label,
                    ["ringframe", "deltas", BASE, "entries", id].map(String::from).to_vec(),
                ));
            }
        }
    }
    out
}

/// A RingFrame row of the project's: its value, every project's beside it,
/// its choices when open, and Reset.
fn ring_row(
    props: &SheetProps,
    name: &str,
    label: String,
    every: String,
    value: String,
    (key, project): (Vec<String>, &Path),
    options: Vec<(String, SheetAct)>,
) -> Element {
    let dotted = key.join(".");
    let open = props.choice == Some(dotted.as_str());
    let mut el = div().layer(format!("project setting {name}")).self_stretch().flex_col().child(
        div()
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
                    .child(
                        div()
                            .font_size(12)
                            .color(palette::MUTED)
                            .text(fill("desktop.project.every", &[("value", &every)])),
                    ),
            )
            .child(
                div()
                    .layer("setting value")
                    .on_click(SheetAct::OpenChoice(key.clone()))
                    .w(190)
                    .pad(6, 10)
                    .border(palette::BLUE_BORDER)
                    .font_size(13)
                    .color(palette::BLUE_BORDER)
                    .text(if options.is_empty() { value } else { format!("{value} ▾") }),
            )
            .child(
                div()
                    .layer("reset")
                    .on_click(SheetAct::Choose {
                        project: Some(project.to_path_buf()),
                        key,
                        value: None,
                    })
                    .font_size(12)
                    .color(palette::BLUE_BORDER)
                    .text(word("act.reset")),
            ),
    );
    if open && !options.is_empty() {
        let mut list = div()
            .layer(format!("choices {dotted}"))
            .self_stretch()
            .flex_col()
            .bg(palette::SURFACE_ALT)
            .border_top(palette::LINE);
        for (title, act) in options {
            list = list.child(
                div()
                    .layer(format!("choice {title}"))
                    .on_click(act)
                    .self_stretch()
                    .pad(8, 32)
                    .font_size(13)
                    .text(title),
            );
        }
        el = el.child(list);
    }
    el
}
