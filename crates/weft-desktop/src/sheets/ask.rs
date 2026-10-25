//! The Ask, docked under the panes: what to ask, and where it goes; or a
//! follow-up on a work.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use weft_core::ask::{Draft, route_label};
use weft_core::words::{fill, word};

use super::{SheetProps, send_to};
use crate::action::{Focus, SheetAct};
use crate::gpui::{Element, div, palette, span, textarea_el};
use crate::ui::{Size, Tone, button};

/// How many practices show before "+ N more".
const SHOWN: usize = 3;

/// What the person picked of the open Ask's sheet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AskChoices {
    pub route: Option<String>,
    /// The plan picked: none here is the one offered; `Some(None)`, no plan.
    pub plan: Option<Option<String>>,
    /// Practices taken out, by id.
    pub removed: BTreeSet<String>,
    /// Every practice shown, past the first few.
    pub all: bool,
    /// The practices taken out are shown, to put one back.
    pub adding: bool,
    /// The ROUTE or PLAN list that is open.
    pub open: Option<String>,
}

impl AskChoices {
    /// What Ask sends: the route and its task, the plan, and every practice
    /// not taken out.
    pub fn draft(&self, sheet: &Value) -> Draft {
        let text = |key: &str| sheet[key].as_str().map(str::to_string);
        let plan = match &self.plan {
            Some(picked) => picked.clone(),
            None => sheet["plan"]["path"].as_str().map(str::to_string),
        };
        let rules = practices(sheet)
            .iter()
            .filter_map(|r| r["id"].as_str())
            .filter(|id| !self.removed.contains(*id))
            .map(str::to_string)
            .collect();
        Draft {
            route: text("route"),
            task: text("task"),
            plan,
            rules: Some(rules),
            ..Default::default()
        }
    }
}

/// What the sheet is asked for: the harness, the words and the route picked.
pub fn query(harness: &str, words: &str, choices: &AskChoices) -> String {
    json!({"harness": harness, "words": words, "route": choices.route}).to_string()
}

fn practices(sheet: &Value) -> &[Value] {
    sheet["practices"].as_array().map_or(&[], Vec::as_slice)
}

fn label(text: impl Into<String>) -> Element {
    div().font_mono().font_weight(600).font_size(10).color(palette::DIM).text(text)
}

/// A ROUTE or PLAN pick: closed, what is picked and ▾.
fn pick(which: &str, shown: String) -> Element {
    div()
        .layer(format!("pick {which}"))
        .on_click(SheetAct::AskOpen(which.to_string()))
        .flex_row()
        .gap(8)
        .pad(8, 12)
        .items_center()
        .bg(palette::SURFACE)
        .border(palette::LINE)
        .font_size(14)
        .child(span(shown))
        .child(span("▾").font_size(11).color(palette::MUTED))
}

/// A rule's line: its label, its words cut to one line, and what is at its end.
fn rule_line(rule: &Value, end: Element) -> Element {
    let text = |key: &str| rule[key].as_str().unwrap_or_default().to_string();
    div()
        .layer(format!("rule {}", text("id")))
        .flex_row()
        .gap(10)
        .pad(7, 12)
        .items_center()
        .style("border-bottom", format!("1px solid {}", palette::LINE))
        .font_size(13)
        .child(div().w(120).font_weight(600).text(text("label")))
        .child(
            div()
                .flex_1()
                .color(palette::MUTED)
                .style("white-space", "nowrap")
                .style("overflow", "hidden")
                .style("text-overflow", "ellipsis")
                .text(text("text")),
        )
        .child(end)
}

fn rules_box(heading: Element, lines: Vec<Element>) -> Element {
    div()
        .self_stretch()
        .flex_row()
        .gap(12)
        .items_start()
        .child(div().w(64).child(heading))
        .child(div().flex_1().flex_col().bg(palette::SURFACE).border(palette::LINE).children(lines))
}

/// The draft's ROUTE and PLAN picks, their open list, and the route's rules
/// and the practices (17.1): route rules are shown and always sent; × takes
/// a practice out, and + rule shows those taken out, to put one back.
fn draft_rows(sheet: &Value, c: &AskChoices, harness: &str) -> (Vec<Element>, Vec<Element>) {
    let route = sheet["route"].as_str().unwrap_or_default();
    let plan = match &c.plan {
        Some(picked) => picked.clone(),
        None => sheet["plan"]["path"].as_str().map(str::to_string),
    };
    let picks = vec![
        label(word("ask.route")),
        pick("route", route_label(route)),
        label(word("ask.plan")),
        pick("plan", plan.clone().unwrap_or_else(|| word("ask.no_plan").to_string())),
    ];
    let option = |label: String, on: bool, act: SheetAct| {
        let row = div().on_click(act).pad(6, 12).font_size(13).text(label);
        if on { row.bg(palette::SELECTED_BG).color(palette::BLUE_BORDER) } else { row }
    };
    let list: Option<Vec<Element>> = match c.open.as_deref() {
        Some("route") => Some(
            sheet["routes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|r| option(route_label(r), r == route, SheetAct::AskRoute(r.to_string())))
                .collect(),
        ),
        Some("plan") => Some(
            std::iter::once(option(
                word("ask.no_plan").into(),
                plan.is_none(),
                SheetAct::AskPlan(None),
            ))
            .chain(
                sheet["plans"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["path"].as_str())
                    .map(|p| {
                        option(
                            p.to_string(),
                            plan.as_deref() == Some(p),
                            SheetAct::AskPlan(Some(p.to_string())),
                        )
                    }),
            )
            .collect(),
        ),
        _ => None,
    };
    let mut rows: Vec<Element> = Vec::new();
    if let Some(list) = list {
        rows.push(
            div()
                .layer("pick list")
                .self_stretch()
                .flex_col()
                .bg(palette::SURFACE)
                .border(palette::BLUE_BORDER)
                .children(list),
        );
    }
    let named = format!("{harness} · {} · {}", route_label(route), word("ask.always"));
    let route_rules = sheet["route_rules"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| rule_line(r, span(named.clone()).color(palette::DIM)))
        .collect();
    rows.push(rules_box(label(word("ask.route_rules")), route_rules).layer("route rules"));
    let kept: Vec<&Value> = practices(sheet)
        .iter()
        .filter(|r| !c.removed.contains(r["id"].as_str().unwrap_or_default()))
        .collect();
    let shown = if c.all { kept.len() } else { kept.len().min(SHOWN) };
    let out = |r: &Value| {
        let id = r["id"].as_str().unwrap_or_default().to_string();
        span("×").layer(format!("out {id}")).on_click(SheetAct::AskRuleOut(id)).color(palette::DIM)
    };
    let mut lines: Vec<Element> = kept[..shown].iter().map(|r| rule_line(r, out(r))).collect();
    if shown < kept.len() {
        let more = fill("ask.more", &[("n", &(kept.len() - shown).to_string())]);
        lines.push(
            div()
                .layer("more")
                .on_click(SheetAct::AskMore)
                .pad(7, 12)
                .font_size(13)
                .color(palette::BLUE_BORDER)
                .text(more),
        );
    }
    if c.adding {
        for r in practices(sheet)
            .iter()
            .filter(|r| c.removed.contains(r["id"].as_str().unwrap_or_default()))
        {
            let id = r["id"].as_str().unwrap_or_default().to_string();
            lines.push(rule_line(
                r,
                span("+").on_click(SheetAct::AskRuleBack(id)).color(palette::BLUE_BORDER),
            ));
        }
    }
    let count = fill("ask.practices", &[("n", &kept.len().to_string())]);
    rows.push(rules_box(label(count), lines).layer("rules"));
    (picks, rows)
}

pub(super) fn render(props: &SheetProps, follow_up_unit: Option<&str>) -> Element {
    let act = if follow_up_unit.is_some() { word("act.follow_up") } else { word("act.ask") };
    let title = if follow_up_unit.is_some() { word("act.follow_up") } else { word("ask.title") };

    let targets = props.send_to.map(|v| send_to::render(v, |key| SheetAct::AskTo(key).into()));
    let harness = props.send_to.map(|v| v.picked_harness()).unwrap_or_default();
    let (picks, rows) = match props.ask_sheet {
        Some(sheet) => draft_rows(sheet, props.ask_choices, &harness),
        None => (Vec::new(), Vec::new()),
    };
    let add_rule = (!props.ask_choices.removed.is_empty()).then(|| {
        div()
            .layer("add rule")
            .on_click(SheetAct::AskAddRule)
            .font_size(13)
            .color(palette::BLUE_BORDER)
            .text(word("ask.add_rule"))
    });
    let eyebrow = follow_up_unit.and_then(|unit| eyebrow(props, unit)).map(|said| {
        div()
            .layer("eyebrow")
            .font_mono()
            .font_weight(600)
            .font_size(11)
            .color(palette::DIM)
            .text(said)
    });

    div()
        .layer("sheet Ask")
        .self_stretch()
        .shrink_0()
        .flex_col()
        .gap(12)
        .pad(16, 20)
        .items_start()
        .bg(palette::SURFACE_ALT)
        .style("border-top", format!("2px solid {}", palette::BLUE_BORDER))
        .children(eyebrow)
        .child(div().font_weight(600).font_size(15).text(title))
        .child(
            textarea_el(Focus::Ask)
                .layer("ask input")
                .self_stretch()
                .style("min-height", "78px")
                .pad(10, 12)
                .bg(palette::SURFACE)
                .border(palette::BLUE_BORDER)
                .font_size(14)
                .style("line-height", "20px")
                .color(palette::TEXT)
                .placeholder(word("ask.title"))
                .text(props.ask_draft.to_string()),
        )
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .child(
                    div()
                        .font_mono()
                        .font_weight(600)
                        .font_size(11)
                        .color(palette::MUTED)
                        .text(word("ask.send_to")),
                )
                .children(targets)
                .children(picks),
        )
        .children(rows)
        .child(
            div()
                .self_stretch()
                .flex_row()
                .gap(12)
                .items_center()
                .children(add_rule)
                .child(div().flex_1())
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    SheetAct::Close,
                    Tone::Quiet,
                    Size::Sheet,
                ))
                .child(button("btn Ask", act, SheetAct::SubmitAsk, Tone::Primary, Size::Sheet)),
        )
}

/// A follow-up's eyebrow: `FOLLOW UP · <the work's title> · <the id it carries>`.
fn eyebrow(props: &SheetProps, unit: &str) -> Option<String> {
    let crate::app::ActiveSheet::Ask { project, .. } = props.sheet else { return None };
    let client = props.clients.iter().find(|c| &c.root == project)?;
    let work = client.work.of_ask(unit)?;
    let title = client.units.iter().find(|u| u.ask_id == work.root).map(|u| u.title.as_str())?;
    let said = format!("{} · {title} · {}", word("act.follow_up"), work.follows);
    Some(said.to_uppercase())
}
