//! The Ask, docked under the panes: what to ask, and where it goes; or a
//! follow-up on a work.

use std::collections::BTreeMap;

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
    /// Practices the person put in (true) or took out (false), by id.
    pub picked: BTreeMap<String, bool>,
    /// Every practice shown: the kept ones past the first few, and the rest.
    pub all: bool,
    /// The person picks the route now, rather than wait while the words are read.
    pub picking: bool,
    /// The ROUTE or PLAN list that is open.
    pub open: Option<String>,
}

impl AskChoices {
    /// What Ask sends: the route and its task, the plan, and every practice
    /// kept.
    pub fn draft(&self, sheet: &Value) -> Draft {
        let text = |key: &str| sheet[key].as_str().map(str::to_string);
        let plan = match &self.plan {
            Some(picked) => picked.clone(),
            None => sheet["plan"]["path"].as_str().map(str::to_string),
        };
        let rules = practices(sheet)
            .iter()
            .filter_map(|r| r["id"].as_str())
            .filter(|id| is_in(sheet, &self.picked, id))
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

/// Whether a practice is kept: the person's pick, else whether it was
/// proposed for the words; with none proposed, every one is.
pub fn is_in(sheet: &Value, picked: &BTreeMap<String, bool>, id: &str) -> bool {
    picked.get(id).copied().unwrap_or_else(|| match sheet["proposed"].as_array() {
        Some(proposed) => proposed.iter().any(|p| p == id),
        None => true,
    })
}

/// The practices kept, and those not.
fn split_practices<'a>(sheet: &'a Value, c: &AskChoices) -> (Vec<&'a Value>, Vec<&'a Value>) {
    practices(sheet)
        .iter()
        .partition(|r| is_in(sheet, &c.picked, r["id"].as_str().unwrap_or_default()))
}

/// Whether the sheet shows the words being read (17.23): they are read at
/// all, there are words, the sheet shown is for others, and the person has
/// not picked a route or chosen to.
pub fn sensing(sheet: Option<&Value>, sheet_for: Option<&str>, text: &str, c: &AskChoices) -> bool {
    let senses = sheet.is_some_and(|s| s["senses"] == true);
    let picks = c.picking || c.route.is_some();
    senses && !picks && !text.trim().is_empty() && sheet_for != Some(text)
}

/// The words being read (17.23): one panel where the route, its rules and
/// the practices will be.
fn sensing_panel() -> Element {
    // The window breathes `pulse` alone, so the dots breathe together.
    let dot = || {
        div()
            .w(8)
            .h(8)
            .style("border-radius", "4px")
            .bg(palette::BLUE_BORDER)
            .style("animation", "pulse")
    };
    div()
        .layer("sensing")
        .self_stretch()
        .h(150)
        .flex_col()
        .gap(10)
        .items_center()
        .justify_center()
        .bg(palette::SURFACE)
        .border(palette::BLUE_BORDER)
        .child(div().flex_row().gap(6).child(dot()).child(dot()).child(dot()))
        .child(div().font_weight(600).font_size(15).text(word("ask.sensing")))
        .child(div().font_size(13).color(palette::MUTED).text(word("ask.sensing_line")))
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
    let route_pick = match route {
        "" => pick("route", word("ask.pick_route").into())
            .style("border-color", palette::AMBER)
            .color(palette::AMBER),
        r => pick("route", route_label(r)),
    };
    let picks = vec![
        label(word("ask.route")),
        route_pick,
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
    let route_rules = match route {
        "" => vec![
            div().pad(7, 12).font_size(13).color(palette::DIM).text(word("ask.route_rules_wait")),
        ],
        _ => sheet["route_rules"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|r| rule_line(r, span(named.clone()).color(palette::DIM)))
            .collect(),
    };
    rows.push(rules_box(label(word("ask.route_rules")), route_rules).layer("route rules"));
    let (kept, not_kept) = split_practices(sheet, c);
    let shown = if c.all { kept.len() } else { kept.len().min(SHOWN) };
    let out = |r: &Value| {
        let id = r["id"].as_str().unwrap_or_default().to_string();
        span("×").layer(format!("out {id}")).on_click(SheetAct::AskRuleOut(id)).color(palette::DIM)
    };
    let mut lines: Vec<Element> = kept[..shown].iter().map(|r| rule_line(r, out(r))).collect();
    let more = practices(sheet).len() - shown;
    if !c.all && more > 0 {
        let more = fill("ask.more", &[("n", &more.to_string())]);
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
    if c.all {
        for r in not_kept {
            let id = r["id"].as_str().unwrap_or_default().to_string();
            lines.push(rule_line(
                r,
                span("+")
                    .layer(format!("in {id}"))
                    .on_click(SheetAct::AskRuleBack(id))
                    .color(palette::BLUE_BORDER),
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
    let c = props.ask_choices;
    let reading = sensing(props.ask_sheet, props.ask_sheet_for, props.ask_draft, c);
    let (picks, rows) = match props.ask_sheet {
        Some(_) if reading => (Vec::new(), vec![sensing_panel()]),
        Some(sheet) => draft_rows(sheet, c, &harness),
        None => (Vec::new(), Vec::new()),
    };
    let no_route = !reading && props.ask_sheet.is_some_and(|s| s["route"].is_null());
    let link = |layer: &str, act: SheetAct, key: &str| {
        div().layer(layer).on_click(act).font_size(13).color(palette::BLUE_BORDER).text(word(key))
    };
    let add_rule = props.ask_sheet.filter(|_| !reading && !c.all).and_then(|sheet| {
        let (_, not_kept) = split_practices(sheet, c);
        (!not_kept.is_empty()).then(|| link("add rule", SheetAct::AskAddRule, "ask.add_rule"))
    });
    let pick_yourself =
        reading.then(|| link("pick yourself", SheetAct::AskPickRoute, "ask.pick_yourself"));
    let hint = no_route.then(|| {
        div().layer("hint").font_size(13).color(palette::AMBER).text(word("ask.pick_to_ask"))
    });
    let ask_button = button("btn Ask", act, SheetAct::SubmitAsk, Tone::Primary, Size::Sheet);
    let ask_button =
        if no_route || reading { ask_button.style("opacity", "0.4") } else { ask_button };
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
                .children(pick_yourself)
                .child(div().flex_1())
                .children(hint)
                .child(button(
                    "btn Cancel",
                    word("act.cancel"),
                    SheetAct::Close,
                    Tone::Quiet,
                    Size::Sheet,
                ))
                .child(ask_button),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A sheet of 14 practices, of which the first `n` were proposed.
    fn sheet(n: Option<usize>) -> Value {
        let ids: Vec<String> = (0..14).map(|i| format!("r{i}")).collect();
        let practices: Vec<Value> = ids.iter().map(|id| json!({"id": id, "label": id})).collect();
        let proposed = n.map(|n| ids[..n].to_vec());
        json!({"route": "native_plan", "task": "plan", "practices": practices, "proposed": proposed})
    }

    #[test]
    fn the_proposed_practices_are_kept_with_0_1_or_12_of_14() {
        for n in [0, 1, 12] {
            let s = sheet(Some(n));
            let c = AskChoices::default();
            let (kept, not_kept) = split_practices(&s, &c);
            assert_eq!((kept.len(), not_kept.len()), (n, 14 - n));
            let sent = c.draft(&s).rules.unwrap();
            assert_eq!(sent, (0..n).map(|i| format!("r{i}")).collect::<Vec<_>>());
        }
        let none = sheet(None);
        let (kept, _) = split_practices(&none, &AskChoices::default());
        assert_eq!(kept.len(), 14, "nothing proposed: every practice is kept");
    }

    #[test]
    fn the_person_puts_in_and_takes_out_whatever_was_proposed() {
        let s = sheet(Some(1));
        let mut c = AskChoices::default();
        c.picked.insert("r0".into(), false);
        c.picked.insert("r7".into(), true);
        assert_eq!(c.draft(&s).rules.unwrap(), ["r7"]);
    }

    #[test]
    fn the_words_show_as_read_while_the_sheet_is_for_others() {
        let s = json!({"senses": true});
        let c = AskChoices::default();
        assert!(sensing(Some(&s), Some("old"), "new", &c));
        assert!(!sensing(Some(&s), Some("new"), "new", &c));
        assert!(!sensing(Some(&s), Some("old"), "  ", &c));
        assert!(!sensing(Some(&json!({"senses": false})), Some("old"), "new", &c));
        let picking = AskChoices { picking: true, ..Default::default() };
        assert!(!sensing(Some(&s), Some("old"), "new", &picking));
        let routed = AskChoices { route: Some("native_goal".into()), ..Default::default() };
        assert!(!sensing(Some(&s), Some("old"), "new", &routed));
    }
}
