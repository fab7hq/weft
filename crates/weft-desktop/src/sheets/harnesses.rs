//! Settings › Harnesses: inspect and tune, never add. The harnesses this Weft
//! ships, each with how it stands; one shown with the version it was verified
//! with, where its program is, how RingFrame sends to it, and its route rules
//! (on or off, qualified or candidate, wording, Reset).

use serde_json::{Value, json};
use weft_core::onboarding::State;
use weft_core::words::{fill, word};

use super::SheetProps;
use super::menu::badge;
use crate::action::{Focus, SheetAct, WindowAct};
use crate::daemon::Then;
use crate::gpui::{Element, div, input_el, palette};
use crate::ui::{Size, Tone, button, row_list, section_label, sheet_body};

fn key(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn link(layer: &str, act: impl Into<crate::action::Action>, text: &str) -> Element {
    div()
        .layer(layer)
        .on_click(act)
        .font_size(12)
        .color(palette::BLUE_BORDER)
        .text(text.to_string())
}

pub(super) fn render(props: &SheetProps) -> Element {
    let rows = props
        .clients
        .iter()
        .find_map(|c| c.setup.as_ref())
        .map(|v| v.rows.clone())
        .unwrap_or_default();
    let state_of = |name: &str| rows.iter().find(|r| r.name == name).map(|r| r.state.clone());
    let shown = props.harness;
    let mut list = div().layer("harness list").w(240).flex_col().border_right(palette::LINE);
    for h in props.harnesses.iter() {
        let state = state_of(&h.name);
        let on = h.name == shown;
        list = list.child(
            div()
                .layer(format!("harness {}", h.title))
                .on_click(SheetAct::ShowHarness(h.name.clone()))
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(10, 14)
                .items_center()
                .border_bottom(palette::LINE)
                .bg(if on { palette::SELECTED_BG } else { "transparent" })
                .child(
                    div()
                        .w(14)
                        .font_mono()
                        .font_weight(700)
                        .font_size(12)
                        .color(crate::ui::mark_colour(state.as_ref().map_or("?", State::mark)))
                        .text(state.as_ref().map_or("?", State::mark).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .font_size(13)
                        .font_weight(if on { 600 } else { 400 })
                        .text(h.title.clone()),
                )
                .child(
                    div()
                        .font_mono()
                        .font_size(11)
                        .color(palette::DIM)
                        .text(state.as_ref().map(State::words).unwrap_or_default()),
                ),
        );
    }
    list = list.child(
        div().pad(12, 14).font_size(11).color(palette::DIM).text(word("desktop.harness.supported")),
    );

    let detail = props.harnesses.iter().find(|h| h.name == shown).map(|h| {
        let state = state_of(&h.name);
        let mut head = div()
            .self_stretch()
            .flex_row()
            .gap(10)
            .items_baseline()
            .child(div().font_weight(600).font_size(16).text(h.title.clone()));
        if let Some(c) = &h.conversation {
            head = head.child(
                div()
                    .font_mono()
                    .font_size(12)
                    .color(palette::MUTED)
                    .text(fill("desktop.harness.verified", &[("version", &c.verified)])),
            );
        }
        let found = !matches!(state, Some(State::NotFound));
        let machine = row_list(vec![
            div()
                .layer("harness program")
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(9, 14)
                .items_center()
                .child(
                    div()
                        .w(170)
                        .font_weight(600)
                        .font_size(13)
                        .text(word("desktop.harness.program")),
                )
                .child(div().flex_1().font_size(12).color(palette::MUTED).text(word(if found {
                    "desktop.harness.found"
                } else {
                    "desktop.harness.not_found"
                })))
                .child(button(
                    "harness find",
                    word("desktop.updates.locate"),
                    SheetAct::Locate(h.name.clone()),
                    Tone::Plain,
                    Size::Row,
                )),
            div()
                .layer("harness setup")
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(9, 14)
                .items_center()
                .child(
                    div().w(170).font_weight(600).font_size(13).text(word("desktop.harness.setup")),
                )
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::MUTED)
                        .text(state.as_ref().map(State::words).unwrap_or_default()),
                )
                .child(link(
                    "harness updates",
                    WindowAct::OpenUpdates,
                    word("desktop.harness.in_updates"),
                )),
        ]);
        sheet_body()
            .child(head)
            .child(machine)
            .child(section_label(word("desktop.harness.sends")))
            .child(sends(props, &h.name))
            .child(section_label(fill("desktop.harness.route_rules", &[("harness", &h.title)])))
            .child(route_rules(props, &h.name))
    });
    div().layer("harnesses").self_stretch().flex_1().flex_row().child(list).children(detail)
}

/// How RingFrame sends to it, as its profile says.
fn sends(props: &SheetProps, host: &str) -> Element {
    let then = Then::Profile { host: host.to_string() };
    let profile = props.clients.iter().find_map(|c| c.read.get(&then));
    let Some(p) = profile else {
        let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
        return div()
            .font_size(12)
            .color(palette::MUTED)
            .text(super::ringframe::unanswered(refused));
    };
    let mut rows = Vec::new();
    for cap in p["capabilities"].as_array().into_iter().flatten() {
        let id = cap["id"].as_str().unwrap_or_default();
        let who = if cap["delivery_mode"] == "human_handoff" {
            "desktop.harness.you_send"
        } else {
            "desktop.harness.weft_sends"
        };
        let mut said = cap["selection"].as_str().unwrap_or_default().to_string();
        if let Some(n) = cap["max_prompt_chars"].as_u64() {
            said = fill("desktop.harness.limit", &[("n", &n.to_string())]);
        }
        let name = weft_core::words::WORDS
            .iter()
            .find(|(k, _)| *k == format!("desktop.harness.{id}"))
            .map_or(id, |(_, w)| *w);
        rows.push(
            div()
                .layer(format!("sends {id}"))
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(8, 14)
                .items_center()
                .child(div().w(170).font_weight(600).font_size(13).text(name.to_string()))
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::MUTED)
                        .style("white-space", "nowrap")
                        .style("overflow", "hidden")
                        .text(said),
                )
                .child(div().font_mono().font_size(10).color(palette::DIM).text(word(who))),
        );
    }
    if let Some(n) = p["paste_fold_chars"].as_u64() {
        rows.push(
            div()
                .layer("sends folds")
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(8, 14)
                .child(
                    div().w(170).font_weight(600).font_size(13).text(word("desktop.harness.folds")),
                )
                .child(
                    div()
                        .flex_1()
                        .font_size(12)
                        .color(palette::MUTED)
                        .text(fill("desktop.harness.folds_what", &[("n", &n.to_string())])),
                ),
        );
    }
    row_list(rows)
}

/// Its route rules, from RingFrame's catalog for it: each on or off,
/// qualified or candidate, its wording, and Reset once changed.
fn route_rules(props: &SheetProps, host: &str) -> Element {
    let then = Then::Rules { project: false };
    let Some(rules) = props.clients.iter().find_map(|c| c.read.get(&then)) else {
        let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
        return div()
            .font_size(12)
            .color(palette::MUTED)
            .text(super::ringframe::unanswered(refused));
    };
    let catalog = Some(rules)
        .and_then(|r| r["catalogs"].as_array())
        .and_then(|cs| cs.iter().find(|c| c["catalog"] == host))
        .cloned()
        .unwrap_or(Value::Null);
    let mut rows = Vec::new();
    for e in catalog["entries"].as_array().into_iter().flatten() {
        let id = e["id"].as_str().unwrap_or_default();
        let field = |f: &str| key(&["ringframe", "deltas", host, "entries", id, f]);
        let changed = e["source"] != "shipped";
        let on = e["enabled"] != false;
        let qualified = e["status"] == "qualified";
        let status = |q: bool, w: &str| {
            let mine = q == qualified;
            div()
                .layer(format!("status {w}"))
                .on_click(SheetAct::Choose {
                    project: None,
                    key: field("status"),
                    value: Some(json!(if q { "qualified" } else { "candidate" })),
                })
                .pad(2, 7)
                .font_mono()
                .font_size(10)
                .color(if mine { palette::TEAL } else { palette::DIM })
                .bg(if mine { palette::SURFACE_ALT } else { "transparent" })
                .text(word(w))
        };
        let editing =
            props.setting_editing.is_some_and(|(p, k)| p.is_none() && *k == field("text"));
        let text: Element = if editing {
            div()
                .flex_1()
                .child(input_el(Focus::Setting).text(props.setting_text.to_string()).font_size(12))
        } else {
            div()
                .flex_1()
                .font_size(12)
                .color(palette::MUTED)
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .text(e["text"].as_str().unwrap_or_default().to_string())
        };
        rows.push(
            div()
                .layer(format!("route rule {id}"))
                .self_stretch()
                .flex_row()
                .gap(10)
                .pad(8, 14)
                .items_center()
                .bg(if changed { palette::SELECTED_BG } else { "transparent" })
                .child(
                    div()
                        .w(150)
                        .font_weight(600)
                        .font_size(13)
                        .text(e["label"].as_str().unwrap_or(id).to_string()),
                )
                .child(text)
                .child(
                    div()
                        .flex_row()
                        .border(palette::LINE)
                        .child(status(true, "desktop.harness.qualified"))
                        .child(status(false, "desktop.harness.candidate")),
                )
                .child(
                    div()
                        .layer("rule switch")
                        .on_click(SheetAct::Choose {
                            project: None,
                            key: field("enabled"),
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
                            key: key(&["ringframe", "deltas", host, "entries", id]),
                            value: None,
                        },
                        word("act.reset"),
                    )
                } else {
                    link(
                        "edit",
                        SheetAct::EditSetting { project: None, key: field("text") },
                        word("desktop.settings.edit"),
                    )
                }),
        );
    }
    row_list(rows)
}
