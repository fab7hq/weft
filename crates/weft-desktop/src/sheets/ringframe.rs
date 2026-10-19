//! Settings › RingFrame: the practices a prompt draws on, the core rules per
//! phase, every rule of the base practice with where it comes from, what Eval
//! judges must not do, and a preview of the Rules block a route gets. Every
//! project's; a project's own are in its settings.

use serde_json::{Value, json};
use weft_core::words::{fill, word};

use super::SheetProps;
use super::menu::badge;
use crate::action::{Focus, SheetAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, input_el, palette};
use crate::ui::{note, row_list, section_label, sheet_body};

const BASE: &str = "practices/software-development";
const TASKS: [&str; 4] = ["implement", "plan", "review", "diagnose"];
const ROUTES: [&str; 3] = ["native_plan", "native_goal", "native_direct"];

fn key(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

/// A domain's id as words: `autonomous-trading` is "Autonomous trading".
fn domain_name(id: &str) -> String {
    let words = id.replace('-', " ");
    let mut chars = words.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

fn tag(text: &str, on: bool) -> Element {
    div()
        .font_mono()
        .font_size(10)
        .pad(2, 6)
        .border(if on { palette::BLUE_BORDER } else { palette::LINE })
        .color(if on { palette::BLUE_BORDER } else { palette::DIM })
        .text(text.to_string())
}

fn link(layer: &str, act: SheetAct, text: &str) -> Element {
    div()
        .layer(layer)
        .on_click(act)
        .font_size(12)
        .color(palette::BLUE_BORDER)
        .text(text.to_string())
}

/// A field for one setting, typed in place while it is being edited.
fn field(props: &SheetProps, k: Vec<String>, shown: String) -> Element {
    let editing = props.setting_editing.is_some_and(|(p, e)| p.is_none() && *e == k);
    let boxed = div()
        .layer("setting value")
        .flex_1()
        .pad(5, 8)
        .bg(palette::SURFACE_ALT)
        .border(palette::LINE)
        .font_size(12);
    if editing {
        boxed.child(input_el(Focus::Setting).text(props.setting_text.to_string()).font_size(12))
    } else {
        boxed
            .on_click(SheetAct::EditSetting { project: None, key: k })
            .color(palette::MUTED)
            .text(shown)
    }
}

/// Choices under a control, when open: each picks one.
fn choices(props: &SheetProps, dotted: &str, options: Vec<(String, SheetAct)>) -> Option<Element> {
    (props.choice == Some(dotted)).then(|| {
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
        list
    })
}

/// What a Settings read shows until RingFrame answers: that it is asked,
/// or why RingFrame would not say.
pub(super) fn unanswered(refused: Option<&String>) -> String {
    match refused {
        Some(why) => fill("desktop.settings.refused", &[("why", why)]),
        None => word("desktop.settings.asking").to_string(),
    }
}

pub(super) fn render(props: &SheetProps) -> Element {
    let then = Then::Rules { project: false };
    let rules = props.clients.iter().find_map(|c| c.read.get(&then));
    let Some(rules) = rules else {
        let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
        return sheet_body().child(note(unanswered(refused), palette::MUTED));
    };
    let machine = props
        .clients
        .iter()
        .find_map(|c| c.config.as_ref())
        .map_or(Value::Null, |c| c["machine"].clone());
    let layer = &machine["ringframe"];
    let catalogs = rules["catalogs"].as_array().cloned().unwrap_or_default();
    let base = catalogs.iter().find(|c| c["catalog"] == BASE).cloned().unwrap_or(Value::Null);

    // Practices: the base always; each other, turned on here by naming it.
    let mut practices = vec![
        div()
            .layer("practice software-development")
            .self_stretch()
            .flex_row()
            .gap(12)
            .pad(8, 14)
            .items_center()
            .child(
                div()
                    .w(200)
                    .font_weight(600)
                    .font_size(13)
                    .text(domain_name("software-development")),
            )
            .child(
                div()
                    .flex_1()
                    .font_size(12)
                    .color(palette::MUTED)
                    .text(word("desktop.settings.always")),
            )
            .child(badge(
                "setting badge",
                true,
                (palette::SELECTED_BG, palette::BLUE_BORDER, palette::BLUE_BORDER),
            )),
    ];
    for d in rules["domains"].as_array().into_iter().flatten().filter(|d| d["base"] != true) {
        let id = d["domain"].as_str().unwrap_or_default();
        let catalog = format!("practices/{id}");
        let on = !layer["deltas"][catalog.as_str()].is_null();
        let value = (!on).then(|| json!({"domain": id}));
        practices.push(
            div()
                .layer(format!("practice {id}"))
                .on_click(SheetAct::Choose {
                    project: None,
                    key: key(&["ringframe", "deltas", &catalog]),
                    value,
                })
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(8, 14)
                .items_center()
                .child(div().w(200).font_weight(600).font_size(13).text(domain_name(id)))
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::MUTED)
                        .text(d["description"].as_str().unwrap_or_default().to_string()),
                )
                .child(badge(
                    "setting badge",
                    on,
                    (palette::SELECTED_BG, palette::BLUE_BORDER, palette::BLUE_BORDER),
                )),
        );
    }
    // The core rules per phase: the base practice's cap.
    let cap_key = key(&["ringframe", "deltas", BASE, "render", "core_cap"]);
    let cap = base["render"]["core_cap"].as_u64().unwrap_or(5);
    let dotted = cap_key.join(".");
    let options: Vec<(String, SheetAct)> = (1..=9)
        .map(|n| {
            (
                n.to_string(),
                SheetAct::Choose { project: None, key: cap_key.clone(), value: Some(json!(n)) },
            )
        })
        .collect();
    practices.push(
        div()
            .layer("setting Core rules per phase")
            .self_stretch()
            .flex_col()
            .child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(12)
                    .pad(8, 14)
                    .items_center()
                    .child(
                        div()
                            .w(200)
                            .font_weight(600)
                            .font_size(13)
                            .text(word("desktop.settings.core_cap")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .font_size(12)
                            .color(palette::MUTED)
                            .text(word("desktop.settings.core_cap_what")),
                    )
                    .child(
                        div()
                            .layer("setting value")
                            .on_click(SheetAct::OpenChoice(cap_key.clone()))
                            .pad(4, 10)
                            .border(palette::LINE)
                            .font_size(13)
                            .text(format!("{cap} ▾")),
                    ),
            )
            .children(choices(props, &dotted, options)),
    );

    // The base practice's rules.
    let mut rows = Vec::new();
    for e in base["entries"].as_array().into_iter().flatten() {
        rows.push(rule(props, e));
    }
    let own = base["entries"].as_array().map_or(0, |e| {
        e.iter().filter(|x| x["id"].as_str().is_some_and(|i| i.starts_with("own."))).count()
    });
    let add = link(
        "add rule",
        SheetAct::EditSetting {
            project: None,
            key: key(&[
                "ringframe",
                "deltas",
                BASE,
                "entries",
                &format!("own.{}", own + 1),
                "text",
            ]),
        },
        word("desktop.settings.add_rule"),
    );

    // What Eval judges must not do: the machine's list, else RingFrame's own.
    let deny = layer["eval"]["deny"].as_array().cloned();
    let mut judges = Vec::new();
    for (i, rule) in deny.iter().flatten().enumerate() {
        judges.push(
            div()
                .layer(format!("deny {i}"))
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(8, 14)
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .font_size(13)
                        .text(rule.as_str().unwrap_or_default().to_string()),
                )
                .child(link(
                    "remove",
                    SheetAct::Choose {
                        project: None,
                        key: key(&["ringframe", "eval", "deny", &i.to_string()]),
                        value: None,
                    },
                    word("desktop.settings.remove"),
                )),
        );
    }
    let deny_add = key(&["ringframe", "eval", "deny", "+"]);
    let adding = props.setting_editing.is_some_and(|(p, k)| p.is_none() && *k == deny_add);
    judges.push(if adding {
        div().self_stretch().pad(8, 14).child(field(props, deny_add, String::new()))
    } else {
        div().self_stretch().pad(8, 14).child(link(
            "add deny",
            SheetAct::EditSetting { project: None, key: deny_add },
            word("desktop.settings.add"),
        ))
    });

    let mut body = sheet_body()
        .child(section_label(word("desktop.settings.practices")))
        .child(row_list(practices))
        .child(
            div()
                .self_stretch()
                .flex_row()
                .justify_between()
                .child(section_label(fill(
                    "desktop.settings.rules",
                    &[("practice", &domain_name("software-development"))],
                )))
                .child(add),
        );
    let editing_new = props.setting_editing.is_some_and(|(p, k)| {
        p.is_none() && k.get(4).is_some_and(|id| id.starts_with("own.")) && !rows_have(&base, &k[4])
    });
    if editing_new && let Some((_, k)) = props.setting_editing {
        rows.push(div().self_stretch().pad(8, 14).child(field(props, k.clone(), String::new())));
    }
    body = body.child(row_list(rows)).child(section_label(word("desktop.settings.judges")));
    if deny.is_none() {
        body = body.child(note(word("desktop.settings.judges_own"), palette::MUTED));
    }
    body.child(row_list(judges)).child(preview(props))
}

fn rows_have(base: &Value, id: &str) -> bool {
    base["entries"].as_array().is_some_and(|e| e.iter().any(|x| x["id"] == id))
}

/// One rule: its name and wording, where it comes from, its tier and whether
/// it is on; Edit opens its wording, tasks and concerns, Reset takes the
/// change out.
fn rule(props: &SheetProps, e: &Value) -> Element {
    let id = e["id"].as_str().unwrap_or_default();
    let base = |field: &str| key(&["ringframe", "deltas", BASE, "entries", id, field]);
    let changed = e["source"] != "shipped";
    let on = e["enabled"] != false;
    let tier = e["tier"].as_str().unwrap_or("core");
    let label = e["label"].as_str().unwrap_or(id).to_string();
    let tier_key = base("tier");
    let dotted = tier_key.join(".");
    let tiers: Vec<(String, SheetAct)> = ["core", "situational"]
        .iter()
        .map(|t| {
            (
                t.to_string(),
                SheetAct::Choose { project: None, key: tier_key.clone(), value: Some(json!(t)) },
            )
        })
        .collect();
    let editing = props
        .setting_editing
        .is_some_and(|(p, k)| p.is_none() && k.get(4).map(String::as_str) == Some(id));
    let mut row = div()
        .layer(format!("rule {id}"))
        .self_stretch()
        .flex_col()
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(7, 14)
                .items_center()
                .bg(if changed { palette::SELECTED_BG } else { "transparent" })
                .child(div().w(170).font_weight(600).font_size(13).text(label))
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::MUTED)
                        .style("white-space", "nowrap")
                        .style("overflow", "hidden")
                        .text(e["text"].as_str().unwrap_or_default().to_string()),
                )
                .child(tag(
                    word(if changed {
                        "desktop.settings.changed"
                    } else {
                        "desktop.settings.shipped"
                    }),
                    changed,
                ))
                .child(
                    div()
                        .layer("rule tier")
                        .on_click(SheetAct::OpenChoice(tier_key.clone()))
                        .child(tag(tier, false)),
                )
                .child(
                    div()
                        .layer("rule switch")
                        .on_click(SheetAct::Choose {
                            project: None,
                            key: base("enabled"),
                            value: Some(json!(!on)),
                        })
                        .child(badge(
                            "setting badge",
                            on,
                            (palette::SELECTED_BG, palette::BLUE_BORDER, palette::BLUE_BORDER),
                        )),
                )
                .child(if changed {
                    link(
                        "reset",
                        SheetAct::Choose {
                            project: None,
                            key: key(&["ringframe", "deltas", BASE, "entries", id]),
                            value: None,
                        },
                        word("act.reset"),
                    )
                } else {
                    link(
                        "edit",
                        SheetAct::EditSetting { project: None, key: base("text") },
                        word("desktop.settings.edit"),
                    )
                }),
        )
        .children(choices(props, &dotted, tiers));
    if editing {
        for (field_name, word_key) in [
            ("text", "desktop.settings.wording"),
            ("applies_to", "desktop.settings.applies_to"),
            ("concerns", "desktop.settings.concerns"),
        ] {
            let shown = match &e[field_name] {
                Value::Array(items) => {
                    items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                }
                v => v.as_str().unwrap_or_default().to_string(),
            };
            row = row.child(
                div()
                    .self_stretch()
                    .flex_row()
                    .gap(10)
                    .pad(4, 28)
                    .items_center()
                    .child(div().w(130).font_size(12).color(palette::MUTED).text(word(word_key)))
                    .child(field(props, base(field_name), shown)),
            );
        }
    }
    row
}

/// The Rules block a route gets with these settings, once asked for.
fn preview(props: &SheetProps) -> Element {
    let unit = props.preview.to_string();
    let mut parts = props.preview.split('|');
    let mut part = || parts.next().unwrap_or_default().to_string();
    let (host, route, task) = (part(), part(), part());
    let pick = |i: usize, now: &str, all: Vec<String>| {
        let dotted = format!("preview.{i}");
        div()
            .flex_col()
            .child(
                div()
                    .layer(format!("preview pick {i}"))
                    .on_click(SheetAct::OpenChoice(vec!["preview".into(), i.to_string()]))
                    .pad(4, 8)
                    .border(palette::LINE)
                    .font_size(12)
                    .text(format!("{now} ▾")),
            )
            .children(choices(
                props,
                &dotted,
                all.into_iter().map(|v| (v.clone(), SheetAct::PreviewPick(i, v))).collect(),
            ))
    };
    let hosts: Vec<String> = props.harnesses.iter().map(|h| h.name.clone()).collect();
    let shown =
        props.clients.iter().find_map(|c| c.read.get(&Then::Preview { unit: unit.clone() }));
    let mut el =
        div().layer("preview").self_stretch().flex_col().border(palette::BLUE_BORDER).child(
            div()
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(8, 14)
                .items_start()
                .child(div().font_weight(600).font_size(13).text(word("desktop.settings.preview")))
                .child(pick(0, &host, hosts))
                .child(pick(1, &route, ROUTES.map(String::from).to_vec()))
                .child(pick(2, &task, TASKS.map(String::from).to_vec()))
                .child(div().flex_1())
                .child(link("show preview", SheetAct::ShowPreview, word("desktop.settings.show"))),
        );
    if let Some(text) = shown.and_then(|v| v["text"].as_str()) {
        el = el.child(
            div()
                .layer("preview text")
                .self_stretch()
                .pad(8, 14)
                .font_mono()
                .font_size(11)
                .color(palette::MUTED)
                .style("white-space", "pre-wrap")
                .text(text.to_string()),
        );
    } else if let Some(why) =
        props.clients.iter().find_map(|c| c.refused.get(&Then::Preview { unit: unit.clone() }))
    {
        el = el.child(
            div()
                .layer("preview text")
                .self_stretch()
                .pad(8, 14)
                .font_size(12)
                .color(palette::MUTED)
                .text(unanswered(Some(why))),
        );
    }
    el
}
