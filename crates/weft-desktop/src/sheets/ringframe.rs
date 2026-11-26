//! Settings › Rules: what the person works on, and each practice's rules,
//! switched on and off; with Advanced settings, the core rules per phase,
//! each rule's tier and a preview of the Rules block a route gets. Also
//! Eval's "While reading, Eval never". Every project's; a project's own are
//! in its settings.

use serde_json::{Value, json};
use weft_core::words::{fill, word};

use super::SheetProps;
use super::settings::{group, line_row, link, toggle};
use crate::action::{Focus, SheetAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, input_el, palette};

pub const BASE: &str = "practices/software-development";
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
/// breathing, or why RingFrame would not say, with Try again.
pub(super) fn unanswered(refused: Option<&String>, then: Then) -> Element {
    match refused {
        Some(why) => {
            let again = SheetAct::TryAgain { project: Default::default(), then };
            crate::waiting::failed(
                fill("desktop.settings.refused", &[("why", why)]),
                Some(again.into()),
            )
        }
        None => crate::waiting::busy_line(word("desktop.settings.asking"), palette::MUTED),
    }
}

/// How many rules show before "N more".
const FIRST_RULES: usize = 5;

/// RingFrame's rules, every project's, once the daemon has said them.
fn rules_read<'a>(props: &'a SheetProps) -> Option<&'a Value> {
    props.clients.iter().find_map(|c| c.read.get(&Then::Rules { project: false }))
}

/// What shows until RingFrame's rules are said.
fn rules_waiting(props: &SheetProps) -> Element {
    let then = Then::Rules { project: false };
    let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
    unanswered(refused, then.clone())
}

/// The machine's RingFrame layer, as the daemon last said it.
fn layer_of(props: &SheetProps) -> Value {
    props
        .clients
        .iter()
        .find_map(|c| c.config.as_ref())
        .map_or(Value::Null, |c| c["machine"]["ringframe"].clone())
}

/// One row: its name, what it is, and its controls; a line under it.
fn line(layer: impl Into<String>, label: &str, what: &str) -> Element {
    line_row(layer, label, (!what.is_empty()).then_some(what))
}

/// Rules: what the person works on, and each practice's rules, switched on
/// and off; a practice opens its rules in place. Advanced settings adds the
/// core rules per phase, each rule's tier and Edit, and the preview.
pub(super) fn render(props: &SheetProps) -> Element {
    let Some(rules) = rules_read(props) else {
        return div().self_stretch().flex_col().pad(12, 0).child(rules_waiting(props));
    };
    let layer = layer_of(props);
    let catalogs = rules["catalogs"].as_array().cloned().unwrap_or_default();
    let base = catalogs.iter().find(|c| c["catalog"] == BASE).cloned().unwrap_or(Value::Null);
    let mut body = div()
        .self_stretch()
        .flex_col()
        .child(
            div().font_size(12).color(palette::MUTED).pad(4, 0).text(word("settings.rules_what")),
        )
        .child(group(word("settings.rules.work_on"), false, None));
    // What the person works on: the base always; each other, turned on here by naming it.
    let domains = rules["domains"].as_array().cloned().unwrap_or_default();
    for d in &domains {
        let id = d["domain"].as_str().unwrap_or_default();
        let what = d["description"].as_str().unwrap_or_default();
        if d["base"] == true {
            body = body.child(
                line(format!("practice {id}"), &domain_name(id), what).child(
                    div()
                        .font_mono()
                        .font_size(10)
                        .color(palette::DIM)
                        .text(word("settings.rules.always_on")),
                ),
            );
            continue;
        }
        let catalog = format!("practices/{id}");
        let on = !layer["deltas"][catalog.as_str()].is_null();
        let value = (!on).then(|| json!({"domain": id}));
        body = body.child(
            line(format!("practice {id}"), &domain_name(id), what)
                .on_click(SheetAct::Choose {
                    project: None,
                    key: key(&["ringframe", "deltas", &catalog]),
                    value,
                })
                .child(toggle(on, false)),
        );
    }
    if props.advanced {
        // The core rules per phase: the base practice's cap.
        let cap_key = key(&["ringframe", "deltas", BASE, "render", "core_cap"]);
        let cap = base["render"]["core_cap"].as_u64().unwrap_or(5);
        let dotted = cap_key.join(".");
        let options: Vec<(String, SheetAct)> = (1..=9)
            .map(|n| {
                let act =
                    SheetAct::Choose { project: None, key: cap_key.clone(), value: Some(json!(n)) };
                (n.to_string(), act)
            })
            .collect();
        body = body.child(group(word("settings.rules.cap"), true, None)).child(
            div()
                .layer("setting Core rules per phase")
                .self_stretch()
                .flex_col()
                .child(
                    line(
                        "core cap",
                        word("desktop.settings.core_cap"),
                        word("desktop.settings.core_cap_what"),
                    )
                    .child(
                        div()
                            .layer("setting value")
                            .on_click(SheetAct::OpenChoice(cap_key.clone()))
                            .w(70)
                            .pad(6, 10)
                            .bg(palette::SURFACE_ALT)
                            .border(palette::LINE)
                            .font_size(13)
                            .text(format!("{cap} ▾")),
                    ),
                )
                .children(choices(props, &dotted, options)),
        );
    }
    // The rules of the base practice, then of each practice turned on here.
    let on = domains.iter().filter_map(|d| {
        let id = d["domain"].as_str()?;
        let catalog = format!("practices/{id}");
        (d["base"] != true && !layer["deltas"][catalog.as_str()].is_null()).then_some(catalog)
    });
    let groups: Vec<String> = std::iter::once(BASE.to_string()).chain(on).collect();
    // A rule of the person's own goes into the practice open, else the base.
    let into = props.practice_shown.filter(|c| groups.iter().any(|g| g == c)).unwrap_or(BASE);
    let own = next_own(&catalogs);
    let new_key = key(&["ringframe", "deltas", into, "entries", &own, "text"]);
    body = body.child(
        group(
            word("settings.rules.the_rules"),
            false,
            Some(link(
                "add rule",
                SheetAct::EditSetting { project: None, key: new_key },
                word("settings.rules.add"),
            )),
        )
        .layer("rules head"),
    );
    // The new rule being typed: an own id no practice holds yet.
    let typing = props.setting_editing.and_then(|(p, k)| {
        let new = p.is_none()
            && k.get(4).is_some_and(|id| {
                id.starts_with("own.") && !catalogs.iter().any(|c| rows_have(c, id))
            });
        new.then(|| k.clone())
    });
    if let Some(k) = typing {
        body =
            body.child(div().layer("new rule").self_stretch().flex_row().pad(8, 0).child(field(
                props,
                k,
                String::new(),
            )));
    }
    for catalog in &groups {
        let listed = catalogs.iter().find(|c| c["catalog"] == catalog.as_str());
        let name = domain_name(catalog.trim_start_matches("practices/"));
        body = body.children(practice(props, catalog, listed, &name));
    }
    if props.advanced {
        body = body.child(div().self_stretch().pad(10, 0).child(preview(props)));
    }
    body
}

/// The id a new rule of the person's own takes: the first `own.N` no
/// practice holds, so two practices never share one.
fn next_own(catalogs: &[Value]) -> String {
    let taken = |id: &str| catalogs.iter().any(|c| rows_have(c, id));
    (1..).map(|n| format!("own.{n}")).find(|id| !taken(id)).unwrap_or_default()
}

/// One practice: its name and how many of its rules are on, which opens it;
/// open, its first few rules, then how many more.
fn practice(props: &SheetProps, catalog: &str, listed: Option<&Value>, name: &str) -> Vec<Element> {
    let empty = Value::Null;
    let listed = listed.unwrap_or(&empty);
    let entries = listed["entries"].as_array().map_or(&[][..], Vec::as_slice);
    let open = props.practice_shown == Some(catalog);
    let n_on = entries.iter().filter(|e| e["enabled"] != false).count();
    let said =
        fill("settings.rules.on", &[("on", &n_on.to_string()), ("n", &entries.len().to_string())]);
    let head = div()
        .layer(format!("rules head {catalog}"))
        .on_click(SheetAct::ShowPractice(catalog.to_string()))
        .self_stretch()
        .flex_row()
        .gap(8)
        .pad(9, 8)
        .items_center()
        .border_bottom(palette::LINE)
        .bg(if open { palette::SELECTED_BG } else { "transparent" })
        .child(div().font_mono().font_size(11).color(palette::DIM).text(if open {
            "⌄"
        } else {
            "›"
        }))
        .child(
            div()
                .flex_1()
                .font_size(14)
                .font_weight(if open { 600 } else { 400 })
                .text(name.to_string()),
        )
        .child(div().font_mono().font_size(10).color(palette::DIM).text(said));
    if !open {
        return vec![head];
    }
    let folded = !props.all_rules.contains(catalog) && entries.len() > FIRST_RULES + 1;
    let shown = if folded { FIRST_RULES } else { entries.len() };
    let mut rows = div()
        .layer(format!("rules {catalog}"))
        .self_stretch()
        .flex_col()
        .style("padding-left", "22px");
    for e in entries.iter().take(shown) {
        rows = rows.child(rule(props, catalog, e));
    }
    if folded {
        let n = (entries.len() - FIRST_RULES).to_string();
        rows = rows.child(
            div()
                .layer("more rules")
                .on_click(SheetAct::AllRules(catalog.to_string()))
                .self_stretch()
                .pad(9, 0)
                .border_bottom(palette::LINE)
                .font_size(13)
                .color(palette::MUTED)
                .text(fill("settings.rules.more", &[("n", &n)])),
        );
    }
    vec![head, rows]
}

fn rows_have(catalog: &Value, id: &str) -> bool {
    catalog["entries"].as_array().is_some_and(|e| e.iter().any(|x| x["id"] == id))
}

/// One rule: its name and wording, which opens its wording to change; Reset
/// once changed; on or off. Advanced settings adds its tier and Edit, and
/// opens its tasks and concerns too.
pub(super) fn rule(props: &SheetProps, catalog: &str, e: &Value) -> Element {
    let id = e["id"].as_str().unwrap_or_default();
    let entry = |field: &str| key(&["ringframe", "deltas", catalog, "entries", id, field]);
    let changed = e["source"] != "shipped";
    let on = e["enabled"] != false;
    let tier = e["tier"].as_str().unwrap_or("core");
    let label = e["label"].as_str().unwrap_or(id).to_string();
    let host = !catalog.starts_with("practices/");
    let tier_key = entry("tier");
    let dotted = tier_key.join(".");
    let tiers: Vec<(String, SheetAct)> = ["core", "situational"]
        .iter()
        .map(|t| {
            let act =
                SheetAct::Choose { project: None, key: tier_key.clone(), value: Some(json!(t)) };
            (t.to_string(), act)
        })
        .collect();
    let editing = props.setting_editing.is_some_and(|(p, k)| {
        p.is_none() && k.get(2).is_some_and(|c| c == catalog) && k.get(4).is_some_and(|i| i == id)
    });
    let name = div()
        .layer("rule name")
        .on_click(SheetAct::EditSetting { project: None, key: entry("text") })
        .flex_1()
        .flex_col()
        .gap(2)
        .style("min-width", "0px")
        .child(div().font_size(14).text(label))
        .child(
            div()
                .font_size(12)
                .color(palette::MUTED)
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .style("text-overflow", "ellipsis")
                .text(e["text"].as_str().unwrap_or_default().to_string()),
        );
    let mut top = div()
        .self_stretch()
        .flex_row()
        .gap(10)
        .pad(9, 0)
        .items_center()
        .border_bottom(palette::LINE)
        .child(name);
    if changed {
        top = top.child(tag(word("desktop.settings.changed"), true));
    }
    if props.advanced && host {
        top = top.child(status(e, &entry("status")));
    } else if props.advanced {
        top = top.child(
            div()
                .layer("rule tier")
                .on_click(SheetAct::OpenChoice(tier_key.clone()))
                .child(tag(tier, false)),
        );
    }
    if changed {
        top = top.child(link(
            "reset",
            SheetAct::Choose {
                project: None,
                key: key(&["ringframe", "deltas", catalog, "entries", id]),
                value: None,
            },
            word("act.reset"),
        ));
    } else if props.advanced {
        top = top.child(link(
            "edit",
            SheetAct::EditSetting { project: None, key: entry("text") },
            word("desktop.settings.edit"),
        ));
    }
    top = top.child(
        div()
            .layer("rule switch")
            .on_click(SheetAct::Choose {
                project: None,
                key: entry("enabled"),
                value: Some(json!(!on)),
            })
            .child(toggle(on, false)),
    );
    let mut row = div()
        .layer(format!("rule {id}"))
        .self_stretch()
        .flex_col()
        .child(top)
        .children(choices(props, &dotted, tiers));
    if editing {
        let fields: &[(&str, &str)] = if props.advanced && !host {
            &[
                ("text", "desktop.settings.wording"),
                ("applies_to", "desktop.settings.applies_to"),
                ("concerns", "desktop.settings.concerns"),
            ]
        } else {
            &[("text", "desktop.settings.wording")]
        };
        for (field_name, word_key) in fields {
            let shown = match &e[*field_name] {
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
                    .pad(4, 0)
                    .items_center()
                    .child(div().w(130).font_size(12).color(palette::MUTED).text(word(word_key)))
                    .child(field(props, entry(field_name), shown)),
            );
        }
    }
    row
}

/// A harness's rule: qualified or candidate, either picked.
fn status(e: &Value, field: &[String]) -> Element {
    let qualified = e["status"] == "qualified";
    let part = |q: bool, w: &str| {
        let mine = q == qualified;
        div()
            .layer(format!("status {w}"))
            .on_click(SheetAct::Choose {
                project: None,
                key: field.to_vec(),
                value: Some(json!(if q { "qualified" } else { "candidate" })),
            })
            .pad(3, 8)
            .font_size(12)
            .font_weight(if mine { 600 } else { 400 })
            .color(if mine { palette::TEAL } else { palette::MUTED })
            .bg(if mine { palette::TEAL_BG } else { "transparent" })
            .text(word(w))
    };
    div()
        .flex_row()
        .border(palette::LINE)
        .child(part(true, "desktop.harness.qualified"))
        .child(part(false, "desktop.harness.candidate"))
}

/// While reading, Eval never: the machine's list, else RingFrame's own, each
/// with Remove; `+ Add` types one.
pub(super) fn eval_never(props: &SheetProps) -> Element {
    let deny_add = key(&["ringframe", "eval", "deny", "+"]);
    let add = link(
        "add deny",
        SheetAct::EditSetting { project: None, key: deny_add.clone() },
        word("desktop.settings.add"),
    );
    let mut el = div().layer("eval never").self_stretch().flex_col().child(group(
        word("settings.eval.never"),
        false,
        Some(add),
    ));
    let Some(rules) = rules_read(props) else {
        return el.child(rules_waiting(props));
    };
    let layer = layer_of(props);
    let deny = layer["eval"]["deny"].as_array().cloned();
    let shipped = deny.is_none();
    let listed = deny.or_else(|| rules["judges"]["deny"].as_array().cloned());
    if props.setting_editing.is_some_and(|(p, k)| p.is_none() && *k == deny_add) {
        el = el.child(div().self_stretch().flex_row().pad(8, 0).child(field(
            props,
            deny_add,
            String::new(),
        )));
    }
    for (i, rule) in listed.iter().flatten().enumerate() {
        el = el.child(
            line(format!("deny {i}"), rule.as_str().unwrap_or_default(), "")
                .children(shipped.then(|| {
                    div()
                        .font_mono()
                        .font_size(10)
                        .color(palette::DIM)
                        .text(word("settings.eval.built_in"))
                }))
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
    el
}

/// The Rules block a route gets with these settings, once asked for.
fn preview(props: &SheetProps) -> Element {
    let unit = props.preview.to_string();
    let mut parts = props.preview.split('|');
    let mut part = || parts.next().unwrap_or_default().to_string();
    let (host, route, task) = (part(), part(), part());
    // A harness by its title, a route by its name, a task as RingFrame names it.
    let said = |i: usize, v: &str| -> String {
        match i {
            0 => props.harnesses.iter().find(|h| h.name == v).map_or(v, |h| &h.title).to_string(),
            1 => weft_core::words::WORDS
                .iter()
                .find(|(k, _)| *k == format!("desktop.harness.{v}"))
                .map_or(v, |(_, w)| *w)
                .to_string(),
            _ => v.to_string(),
        }
    };
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
                    .text(format!("{} ▾", said(i, now))),
            )
            .children(choices(
                props,
                &dotted,
                all.into_iter().map(|v| (said(i, &v), SheetAct::PreviewPick(i, v))).collect(),
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
                .children(shown.map(|v| {
                    let n = |k: &str| v[k].as_u64().unwrap_or(0).to_string();
                    div().layer("preview kept").font_size(12).color(palette::MUTED).text(fill(
                        "desktop.settings.preview_kept",
                        &[("kept", &n("kept")), ("left_out", &n("left_out"))],
                    ))
                }))
                .child(link("show preview", SheetAct::ShowPreview, word("desktop.settings.show"))),
        );
    if let Some(text) = shown.and_then(|v| v["text"].as_str()).filter(|_| props.preview_shown) {
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
                .text(fill("desktop.settings.refused", &[("why", why)])),
        );
    }
    el
}
