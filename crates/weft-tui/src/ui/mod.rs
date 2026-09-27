//! Drawing.
//!
//! The rule this module enforces: a field that cannot be traced to a ledger
//! event may not be rendered. One short function per surface, named after the
//! screen it draws.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use tui_term::widget::PseudoTerminal;

use crate::app::{Act, App, Modal, Target};
use crate::keys::Focus;
use crate::layout::{self, Layout};
use crate::ledger::Unit;
use crate::theme::{Theme, pair};

mod agent;
mod bar;
mod list;
mod panel;
#[cfg(test)]
mod tests;
mod title;

use agent::*;
use bar::*;
use list::*;
use panel::*;
use title::*;

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hold_rows(true);
    draw_frame(frame, app);
    app.hold_rows(false);
}

fn draw_frame(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let geo = geometry(app, area);
    let th = app.theme;
    app.hits.clear();

    let (title, turbo) = title_bar(app, geo.title.width);
    if let Some((from, to)) = turbo {
        let at = Rect { x: geo.title.x + from, width: to - from, ..geo.title };
        app.hits.push((at, Target::Turbo));
    }
    frame.render_widget(title, geo.title);
    if geo.tabs.height > 0 {
        let (line, tabs) = tab_row(app, &geo);
        for (from, to, pane) in tabs {
            let at = Rect { x: from, width: to - from, ..geo.tabs };
            app.hits.push((at, Target::Tab(pane)));
        }
        frame.render_widget(Paragraph::new(line), geo.tabs);
    }
    frame.render_widget(rule(geo.top_rule.width, geo.divider, geo.top_junction, th), geo.top_rule);

    if app.detail().is_some() {
        // It blocks. Nothing behind it is reachable while it is open, which
        // is what keeps `[P]ROCEED` and the acts on the bar from ever being
        // live at the same time.
        let para = detail(app, geo.content);
        frame.render_widget(para, geo.content);
    } else {
        if let Some(list) = geo.list {
            let drawn = work_list(app, list, geo.right.is_some());
            frame.render_widget(drawn, list);
        }
        if let Some(divider) = geo.divider_area {
            frame.render_widget(Paragraph::new(vlines(divider.height, th)), divider);
        }
        if let Some(right) = geo.right {
            agent(frame, app, right);
        }
    }

    if let (Some(rule_area), Some(panel_area)) = (geo.panel_rule, geo.panel) {
        frame.render_widget(rule(rule_area.width, None, '─', th), rule_area);
        frame.render_widget(panel(app, panel_area), panel_area);
    }

    frame.render_widget(rule(geo.bottom_rule.width, geo.body_divider, '┴', th), geo.bottom_rule);
    frame.render_widget(action_bar(app, geo.actions.width), geo.actions);
    frame.render_widget(hint(app), geo.hint);
}

// --- where everything goes ---------------------------------------------------

struct Geo {
    title: Rect,
    tabs: Rect,
    top_rule: Rect,
    content: Rect,
    list: Option<Rect>,
    divider_area: Option<Rect>,
    right: Option<Rect>,
    panel_rule: Option<Rect>,
    panel: Option<Rect>,
    bottom_rule: Rect,
    actions: Rect,
    hint: Rect,
    /// Column of the vertical divider on the tab row, if there is one.
    divider: Option<u16>,
    /// Column of the divider through the body, if it runs that far.
    body_divider: Option<u16>,
    top_junction: char,
}

fn geometry(app: &App, area: Rect) -> Geo {
    let tab_height = if app.pane_count() > 0 { 1 } else { 0 };
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(tab_height),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    let body = rows[3];

    // A bottom-anchored panel keeps the row it is about in view above it.
    let (content, panel_rule, panel) = match panel_lines(app) {
        Some(lines) => {
            let wanted = (lines.len() as u16 + 1).min(body.height.saturating_sub(3));
            let split = RLayout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Length(wanted),
                ])
                .split(body);
            (split[0], Some(split[1]), Some(split[2]))
        }
        None => (body, None, None),
    };

    let mut geo = Geo {
        title: rows[0],
        tabs: rows[1],
        top_rule: rows[2],
        content,
        list: None,
        divider_area: None,
        right: None,
        panel_rule,
        panel,
        bottom_rule: rows[4],
        actions: rows[5],
        hint: rows[6],
        divider: None,
        body_divider: None,
        top_junction: '─',
    };

    // The detail view blocks, so it takes the whole body: no split, no
    // divider, and no junction in the rule above it.
    if app.detail().is_some() {
        return geo;
    }

    match layout::for_width(area.width) {
        Layout::Split { list, .. } if app.show_work() => {
            let cols = RLayout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(list), Constraint::Length(1), Constraint::Min(1)])
                .split(content);
            geo.list = Some(cols[0]);
            geo.divider_area = Some(cols[1]);
            geo.right = Some(cols[2]);
            geo.divider = Some(list);
            geo.body_divider = Some(list);
            geo.top_junction = '┼';
        }
        Layout::Split { .. } => {
            // Hidden, the agent has the whole width and nothing is left over.
            // [W] brings it back, and the bar says so.
            geo.right = Some(content);
        }
        Layout::Single { .. } => {
            if app.show_work() && app.focus == Focus::Weft && app.detail().is_none() {
                geo.list = Some(content);
            } else {
                geo.right = Some(content);
            }
        }
    }
    geo
}

fn rule(width: u16, at: Option<u16>, junction: char, th: Theme) -> Paragraph<'static> {
    let mut line: String = "─".repeat(width as usize);
    if let Some(col) = at {
        let col = col as usize;
        if col < width as usize {
            line = line
                .chars()
                .enumerate()
                .map(|(i, c)| if i == col { junction } else { c })
                .collect();
        }
    }
    Paragraph::new(Line::styled(line, th.rule()))
}

fn vlines(height: u16, th: Theme) -> Vec<Line<'static>> {
    (0..height).map(|_| Line::styled("│", th.rule())).collect()
}

// --- small things -------------------------------------------------------------

fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    // Display width, not characters: `⚡` takes two cells.
    let used: usize = left.iter().chain(right.iter()).map(Span::width).sum();
    let gap = (width as usize).saturating_sub(used).max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// How many cells a text takes on screen: `⚡` takes two.
fn cells(s: &str) -> usize {
    Span::raw(s).width()
}

fn padded(s: &str, n: usize) -> String {
    let len = cells(s);
    if len >= n { s.to_string() } else { format!("{s}{}", " ".repeat(n - len)) }
}

/// At most `n` cells of it, ending in `…` when it had to be cut.
fn clip(s: &str, n: usize) -> String {
    if cells(s) <= n {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let mut next = out.clone();
        next.push(c);
        if cells(&next) + 1 > n {
            break;
        }
        out = next;
    }
    if n > 0 {
        out.push('…');
    }
    out
}

/// Break a line to a width without touching what it contains. Prefers the
/// last space, falls back to a hard break, and keeps every character.
fn fold(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let chars: Vec<char> = para.chars().collect();
        let mut at = 0usize;
        while chars.len() - at > width {
            let window = &chars[at..at + width];
            let split = window.iter().rposition(|c| *c == ' ').map(|i| i + 1).unwrap_or(width);
            out.push(chars[at..at + split].iter().collect());
            at += split;
        }
        out.push(chars[at..].iter().collect());
    }
    out
}

/// What the list and the panel say, as text, for tests that read the screen
/// the way a person would.
#[cfg(test)]
pub(crate) fn sidebar_text(app: &App) -> String {
    let flat = |l: Line<'static>| l.spans.iter().map(|s| s.content.to_string()).collect::<String>();
    app.rows().iter().map(|r| flat(sidebar_row(app, r, false, 80))).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
pub(crate) fn panel_text(app: &App) -> Vec<String> {
    panel_lines(app).unwrap_or_default()
}
