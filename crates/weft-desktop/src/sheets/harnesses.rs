//! Settings › Agent apps: inspect and tune, never add. Each harness this Weft
//! supports and how it stands, in plain words, with what to do when it needs
//! something. With Advanced settings on, one opens in place: its version and
//! the one Weft was verified with, where its program is, how Weft asks it,
//! and its own rules.

use serde_json::Value;
use weft_core::onboarding::State;
use weft_core::words::{fill, word};

use super::SheetProps;
use super::settings::{group, line_row, link};
use crate::action::SheetAct;
use crate::daemon::Then;
use crate::gpui::{Element, div, palette};

/// How an agent app stands, in the person's words: its mark, the chip's
/// colours, its word, what to do when it needs something, and whether it
/// needs the person.
struct Standing {
    mark: &'static str,
    tone: (&'static str, &'static str, &'static str),
    word: String,
    what: Option<&'static str>,
    needs: bool,
}

const OK: (&str, &str, &str) = (palette::TEAL, "#1f6f69", palette::TEAL_BG);
const WARN: (&str, &str, &str) = (palette::AMBER, palette::AMBER_BORDER, palette::AMBER_BG);
const NO: (&str, &str, &str) = (palette::MUTED, palette::LINE, "transparent");

fn standing(state: Option<&State>) -> Standing {
    let s = |mark, tone, w: &str, what, needs| Standing {
        mark,
        tone,
        word: word(w).to_string(),
        what,
        needs,
    };
    match state {
        Some(State::Ready { .. }) => s("✓", OK, "settings.apps.ready", None, false),
        Some(State::Behind { .. }) => {
            s("■", WARN, "settings.apps.update", Some("settings.apps.update_what"), true)
        }
        Some(State::NotFound) => {
            s("□", NO, "settings.apps.missing", Some("settings.apps.missing_what"), false)
        }
        Some(State::NotSetUp | State::CannotInstall | State::Failed { .. }) => {
            s("■", WARN, "settings.apps.setup", Some("settings.apps.setup_what"), true)
        }
        Some(other) => Standing {
            mark: other.mark(),
            tone: NO,
            word: other.label().to_string(),
            what: None,
            needs: false,
        },
        None => Standing { mark: "?", tone: NO, word: String::new(), what: None, needs: false },
    }
}

fn state_of(props: &SheetProps, name: &str) -> Option<State> {
    props.setup?.rows.iter().find(|r| r.name == name).map(|r| r.state.clone())
}

/// How many agent apps need the person, as the list of sections says.
pub(super) fn needing(props: &SheetProps) -> usize {
    props.harnesses.iter().filter(|h| standing(state_of(props, &h.name).as_ref()).needs).count()
}

pub(super) fn render(props: &SheetProps) -> Element {
    let machine = [("machine", props.machine)];
    let mut body = div()
        .layer("harnesses")
        .self_stretch()
        .flex_col()
        .child(
            div()
                .font_size(12)
                .color(palette::MUTED)
                .pad(4, 0)
                .text(fill("settings.apps_what", &machine)),
        )
        .child(group(fill("settings.apps.on", &machine), false, None));
    for h in props.harnesses.iter() {
        let state = state_of(props, &h.name);
        let st = standing(state.as_ref());
        let open = props.advanced && props.harness == h.name;
        let version = props
            .setup
            .and_then(|v| v.rows.iter().find(|r| r.name == h.name))
            .and_then(|r| r.version.clone());
        let verified = h
            .conversation
            .as_ref()
            .map(|c| fill("desktop.harness.verified", &[("version", &c.verified)]));
        let said: Vec<String> = version.into_iter().chain(verified).collect();
        let mut title = div()
            .flex_row()
            .gap(8)
            .items_baseline()
            .child(div().font_size(14).text(h.title.clone()));
        if props.advanced && !said.is_empty() {
            title = title.child(
                div().font_mono().font_size(11).color(palette::MUTED).text(said.join(" · ")),
            );
        }
        let mut label = div().flex_1().flex_col().gap(2).style("min-width", "0px").child(title);
        if let Some(what) = st.what {
            label =
                label.child(div().font_size(12).color(palette::MUTED).text(fill(what, &machine)));
        }
        let (text, line, bg) = st.tone;
        let mut row = div()
            .layer(format!("harness {}", h.title))
            .self_stretch()
            .flex_row()
            .gap(10)
            .pad(10, 8)
            .items_center()
            .border_bottom(palette::LINE)
            .bg(if open { palette::SELECTED_BG } else { "transparent" })
            .child(div().w(14).font_mono().font_size(12).color(text).text(st.mark))
            .child(label)
            .children((!st.word.is_empty()).then(|| {
                div()
                    .layer("harness standing")
                    .pad(3, 8)
                    .font_size(12)
                    .color(text)
                    .border(line)
                    .bg(bg)
                    .text(st.word.clone())
            }));
        if props.advanced {
            row = row.on_click(SheetAct::ShowHarness(h.name.clone())).child(
                div().font_mono().font_size(11).color(palette::DIM).text(if open {
                    "⌄"
                } else {
                    "›"
                }),
            );
        }
        body = body.child(row);
        if open {
            body = body.child(detail(props, &h.name, state.as_ref()));
        }
    }
    body
}

/// An agent app opened: where its program is, how Weft asks it, and its own
/// rules.
fn detail(props: &SheetProps, host: &str, state: Option<&State>) -> Element {
    let found = !matches!(state, Some(State::NotFound));
    div()
        .layer("harness detail")
        .self_stretch()
        .flex_col()
        .style("padding-left", "22px")
        .child(
            line_row(
                "harness program",
                word("desktop.harness.program"),
                Some(word(if found {
                    "desktop.harness.found"
                } else {
                    "desktop.harness.not_found"
                })),
            )
            .child(link(
                "harness find",
                SheetAct::Locate(host.to_string()),
                word("desktop.updates.locate"),
            )),
        )
        .child(group(word("settings.apps.asks"), true, None))
        .child(asks(props, host))
        .child(group(word("settings.apps.rules"), true, None))
        .child(its_rules(props, host))
}

/// How Weft asks it, as its profile says: each mode, what it does in a line
/// for people, and its limit when it has one.
fn asks(props: &SheetProps, host: &str) -> Element {
    let then = Then::Profile { host: host.to_string() };
    let profile = props.clients.iter().find_map(|c| c.read.get(&then));
    let Some(p) = profile else {
        let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
        return super::ringframe::unanswered(refused, then.clone());
    };
    let mut rows = div().self_stretch().flex_col();
    for cap in p["capabilities"].as_array().into_iter().flatten() {
        let id = cap["id"].as_str().unwrap_or_default();
        let limit = cap["max_prompt_chars"]
            .as_u64()
            .map(|n| fill("desktop.harness.limit", &[("n", &n.to_string())]));
        let said = [cap["purpose"].as_str().map(str::to_string), limit]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let name = weft_core::words::WORDS
            .iter()
            .find(|(k, _)| *k == format!("desktop.harness.{id}"))
            .map_or(id, |(_, w)| *w);
        rows = rows.child(
            div()
                .layer(format!("sends {id}"))
                .self_stretch()
                .flex_row()
                .gap(12)
                .pad(9, 0)
                .items_center()
                .border_bottom(palette::LINE)
                .child(div().w(110).font_size(14).text(name.to_string()))
                .child(div().flex_1().font_size(12).color(palette::MUTED).text(said)),
        );
    }
    rows
}

/// Its own rules, from RingFrame's catalog for it, each as Rules shows one.
fn its_rules(props: &SheetProps, host: &str) -> Element {
    let then = Then::Rules { project: false };
    let Some(rules) = props.clients.iter().find_map(|c| c.read.get(&then)) else {
        let refused = props.clients.iter().find_map(|c| c.refused.get(&then));
        return super::ringframe::unanswered(refused, then.clone());
    };
    let catalog = rules["catalogs"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["catalog"] == host))
        .cloned()
        .unwrap_or(Value::Null);
    let mut rows = div().self_stretch().flex_col();
    for e in catalog["entries"].as_array().into_iter().flatten() {
        rows = rows.child(super::ringframe::rule(props, host, e));
    }
    rows
}
