//! Drawing. Spec: `plans/weft/spec/interface.md` (v2).
//!
//! The rule this module enforces: a field that cannot be traced to a ledger
//! event may not be rendered. One short function per surface, named after the
//! screen it draws.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tui_term::widget::PseudoTerminal;

use crate::app::{Act, App, Modal, Reading};
use crate::keys::Focus;
use crate::layout::{self, Layout};
use crate::ledger::Unit;
use crate::theme::{Theme, pair};

/// The `[W] WORK` cell the tab row keeps when the list is hidden.
const WORK_TAB: &str = " [W] WORK   ";
/// How wide the `WORK` header is when the list has the whole width.
const WORK_HEADER: u16 = 34;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let geo = geometry(app, area);
    let th = app.theme;

    let (title, needs_you) = title_bar(app, geo.title.width);
    frame.render_widget(title, geo.title);
    app.note_needs_you(needs_you.map(|(x, w)| (geo.title.x + x, w)));
    if geo.tabs.height > 0 {
        let (line, spans) = tab_row(app, &geo);
        app.note_tabs(spans);
        frame.render_widget(Paragraph::new(line), geo.tabs);
    }
    frame.render_widget(rule(geo.top_rule.width, geo.divider, geo.top_junction, th), geo.top_rule);

    if app.pane_count() == 0 {
        app.note_rows(Vec::new());
        app.note_list_area(None);
        frame.render_widget(first_run(app, geo.content), geo.content);
    } else {
        app.note_list_area(geo.list);
        if let Some(list) = geo.list {
            let (para, rows) = work_list(app, list);
            app.note_rows(rows);
            frame.render_widget(para, list);
        } else {
            app.note_rows(Vec::new());
        }
        if let Some(divider) = geo.divider_area {
            frame.render_widget(Paragraph::new(vlines(divider.height, th)), divider);
        }
        if let Some(right) = geo.right {
            match app.drawer().is_some() {
                true => frame.render_widget(drawer(app, right), right),
                false => agent(frame, app, right),
            }
        }
    }

    if let (Some(rule_area), Some(panel_area)) = (geo.panel_rule, geo.panel) {
        frame.render_widget(rule(rule_area.width, None, '─', th), rule_area);
        frame.render_widget(panel(app, panel_area), panel_area);
    }

    frame.render_widget(
        rule(geo.bottom_rule.width, geo.body_divider, '┴', th),
        geo.bottom_rule,
    );
    let (bar, action_spans) = action_bar(app, geo.actions.width);
    app.note_actions(
        geo.actions.y,
        action_spans.into_iter().map(|(x, w, a)| (geo.actions.x + x, w, a)).collect(),
    );
    frame.render_widget(bar, geo.actions);
    frame.render_widget(hint(app), geo.hint);

    if let Some(modal) = app.modal.clone() {
        if let Some((title, lines, choices)) = centred(app, &modal) {
            overlay(frame, app, &title, &lines, &choices, area);
        }
    }
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
                .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(wanted)])
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

    if app.pane_count() == 0 {
        return geo;
    }

    match layout::for_width(area.width) {
        Layout::Split { list, .. } if app.show_work() => {
            let cols = RLayout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(list),
                    Constraint::Length(1),
                    Constraint::Min(1),
                ])
                .split(content);
            geo.list = Some(cols[0]);
            geo.divider_area = Some(cols[1]);
            geo.right = Some(cols[2]);
            geo.divider = Some(list);
            geo.body_divider = Some(list);
            geo.top_junction = '┼';
        }
        Layout::Split { .. } => {
            // Hidden, the agent has the whole width and the list keeps a tab.
            geo.right = Some(content);
            geo.divider = Some(WORK_TAB.chars().count() as u16);
            geo.top_junction = '┴';
        }
        Layout::Single { .. } => {
            if app.show_work() && app.focus == Focus::Weft && app.drawer().is_none() {
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

// --- the bars ----------------------------------------------------------------

fn title_bar(app: &App, width: u16) -> (Paragraph<'static>, Option<(u16, u16)>) {
    let th = app.theme;
    let state = match (app.pane_count(), app.focus) {
        (0, _) => String::new(),
        (_, Focus::Weft) => "   in Weft".into(),
        (_, Focus::Agent) => "   in the agent".into(),
    };
    let left = vec![
        Span::styled(" ▚▞ ", Style::default().fg(th.accent())),
        Span::styled(format!("WEFT   {}{}", app.project, state), th.title()),
    ];
    let right = match (app.pane_count(), app.focus) {
        (0, _) => vec![Span::styled("NO AGENT RUNNING ", th.label())],
        (_, Focus::Agent) => vec![Span::styled(
            format!("[{}] TO COME BACK ", app.toggle.label().to_uppercase()),
            th.label(),
        )],
        (_, Focus::Weft) => {
            let mut spans = vec![Span::styled(
                pair("open", &app.open_count().to_string()),
                th.label(),
            )];
            if app.needs_you() > 0 {
                spans.push(Span::raw("   "));
                spans.push(Span::styled(
                    pair("needs you", &app.needs_you().to_string()),
                    th.needs_you(),
                ));
            }
            spans.push(Span::raw(" "));
            spans
        }
    };
    // The count is a place to click, so where it was drawn has to be known.
    let counted = app.focus == Focus::Weft && app.needs_you() > 0;
    let tail: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let last = right.last().map(|s| s.content.chars().count()).unwrap_or(0);
    let span = counted.then(|| {
        let end = width as usize - last;
        let start = (width as usize).saturating_sub(tail);
        (start as u16, (end - start) as u16)
    });
    (Paragraph::new(spread(left, right, width)), span)
}

/// Agents are tabs over the pane. The focused surface's header is the accent.
fn tab_row(app: &App, geo: &Geo) -> (Line<'static>, Vec<(u16, u16, Option<usize>)>) {
    let th = app.theme;
    let in_weft = app.focus == Focus::Weft;
    let mut spans: Vec<Span> = Vec::new();
    let mut spans_at: Vec<(u16, u16, Option<usize>)> = Vec::new();
    let mut col = 0u16;
    let push = |spans: &mut Vec<Span<'static>>, col: &mut u16, text: String, style: Style| {
        *col += text.chars().count() as u16;
        spans.push(Span::styled(text, style));
    };

    let header_style = if in_weft { Style::default().fg(th.accent()) } else { th.label() };
    match (app.show_work(), geo.divider) {
        (false, Some(_)) => {
            push(&mut spans, &mut col, WORK_TAB.to_string(), header_style);
            push(&mut spans, &mut col, "│ ".into(), th.rule());
        }
        (true, Some(at)) => {
            push(&mut spans, &mut col, padded(" WORK", at as usize), header_style);
            push(&mut spans, &mut col, "│ ".into(), th.rule());
        }
        _ => push(&mut spans, &mut col, padded(" WORK", WORK_HEADER as usize), header_style),
    }

    // Reading a drawer replaces the tabs with what is being read.
    if let Some(d) = app.drawer() {
        let width = geo.tabs.width.saturating_sub(col);
        let back = "[←] BACK ";
        let title = clip(&d.title, width.saturating_sub(back.chars().count() as u16 + 1) as usize);
        push(&mut spans, &mut col, padded(&title, (width as usize).saturating_sub(back.chars().count())), th.title());
        push(&mut spans, &mut col, back.into(), th.label());
        return (Line::from(spans), spans_at);
    }

    for i in 0..app.pane_count() {
        let active = i == app.pane_focus;
        let waiting = app.waiting(i).is_some();
        let harness = app.harness_at(i).unwrap_or("agent").to_string();
        let ready = app.readiness(&harness).is_ready();
        let text = format!(
            "{}{} {}{}{}",
            if active { "▸ " } else { "" },
            i + 1,
            harness,
            if waiting { " ●" } else { "" },
            if ready { "" } else { " ⚠" }
        );
        let style = match (active, in_weft, waiting) {
            // In the agent the active tab carries the accent; in Weft the
            // `WORK` header does, so the tab steps back.
            (true, false, _) => Style::default().fg(th.accent()).add_modifier(Modifier::BOLD),
            (true, true, _) => th.selected(),
            (_, _, true) => th.needs_you(),
            _ => th.label(),
        };
        let start = col;
        push(&mut spans, &mut col, text, style);
        spans_at.push((start, col - start, Some(i)));
        push(&mut spans, &mut col, "    ".into(), th.label());
    }
    let start = col;
    push(&mut spans, &mut col, "+".into(), th.label());
    spans_at.push((start, 1, None));
    (Line::from(spans), spans_at)
}

/// Every action shows its key. What cannot be done now is drawn muted and
/// stays where it was, so the shape of the bar never jumps.
fn action_bar(app: &App, width: u16) -> (Paragraph<'static>, Vec<(u16, u16, Act)>) {
    let th = app.theme;
    let plain = |text: &str| (Paragraph::new(Line::styled(text.to_string(), th.label())), Vec::new());
    if app.focus == Focus::Agent {
        // While you are in the agent, Weft has no keys to offer.
        return (Paragraph::new(""), Vec::new());
    }
    match &app.modal {
        Some(Modal::Confirm(_)) => return plain("  [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Quit) => return plain("  [Enter] CONFIRM   [←] CANCEL"),
        Some(Modal::StartAgent { .. }) => return plain("  [Enter] START   [←] CANCEL"),
        Some(Modal::SetUp { gap, .. }) => {
            // Nothing to offer for a missing CLI: it is not Weft's to install.
            return match gap {
                crate::readiness::Gap::Cli => plain("  [←] BACK"),
                _ => plain("  [Enter] DO IT   [←] NOT NOW"),
            };
        }
        Some(Modal::Ask { .. }) => return (Paragraph::new(""), Vec::new()),
        Some(Modal::Help) | Some(Modal::Note(_)) => return plain("  [Enter] CLOSE"),
        None => {}
    }
    if app.pane_count() == 0 {
        return plain("  [Enter] START   [H]ELP   [X] QUIT");
    }
    if app.waiting_here() {
        return plain("  [Enter] ANSWER IT   [E]XPLAIN WHY IT SAYS THAT");
    }
    if let Some(d) = app.drawer() {
        let sibling = match d.kind {
            Reading::Judges => ("[P] WORDING", Act::Wording),
            Reading::Wording => ("[J] JUDGES", Act::Judges),
        };
        let mut at = Vec::new();
        let mut col = 2u16;
        let mut left = vec![Span::raw("  ")];
        for (label, act) in [sibling, ("[F]IX THIS", Act::Fix), ("[D]ECIDE", Act::Decide)] {
            let span = key(app, label, act);
            at.push((col, span.content.chars().count() as u16, act));
            col += span.content.chars().count() as u16 + 3;
            left.push(span);
            left.push(Span::raw("   "));
        }
        left.pop();
        return (
            Paragraph::new(spread(left, vec![Span::styled("[←] BACK ", th.label())], width)),
            at,
        );
    }
    let mut spans = vec![Span::raw("  ")];
    let mut at = Vec::new();
    let mut col = 2u16;
    for (i, (label, act)) in [
        ("[A]SK", Act::Ask),
        ("[S]END", Act::Send),
        ("[C]HECK", Act::Check),
        ("[D]ECIDE", Act::Decide),
        ("[N]EW AGENT", Act::NewAgent),
        ("[W]ORK", Act::Work),
        ("[H]ELP", Act::Help),
        ("[X] QUIT", Act::Quit),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            spans.push(Span::raw("   "));
            col += 3;
        }
        let span = key(app, label, act);
        at.push((col, span.content.chars().count() as u16, act));
        col += span.content.chars().count() as u16;
        spans.push(span);
    }
    (Paragraph::new(Line::from(spans)), at)
}

/// One action on the bar, dimmed when it cannot be used.
fn key(app: &App, label: &str, act: Act) -> Span<'static> {
    let th = app.theme;
    let style = match app.unavailable(act) {
        Some(_) => Style::default().fg(th.line()),
        None => th.label(),
    };
    Span::styled(label.to_string(), style)
}

fn hint(app: &App) -> Paragraph<'static> {
    let th = app.theme;
    if let Some(said) = app.hint_text() {
        return Paragraph::new(Line::styled(format!(" {said}"), th.needs_you()));
    }
    let text = match (&app.modal, app.focus) {
        (Some(Modal::Quit), _) => {
            " Nothing you asked for is lost either way. It is all written down.".into()
        }
        (Some(Modal::Confirm(_)), _) => {
            " Weft never types into an agent without asking you first.".into()
        }
        (Some(Modal::Ask { .. }), _) => {
            " The agent will ask you which approach to take, in its own pane.".into()
        }
        (Some(Modal::StartAgent { .. }), _) => " It runs here, with your own settings.".into(),
        (Some(Modal::SetUp { gap, .. }), _) => match gap {
            crate::readiness::Gap::Cli => " Run it in a terminal, then start an agent again.".into(),
            _ => " Weft never changes an agent without asking you first.".into(),
        },
        (Some(_), _) => " [Enter] closes this.".to_string(),
        (None, Focus::Agent) => {
            let mut s = format!(
                " Every key goes to the agent, Esc included. [{}] comes back.",
                app.toggle.label()
            );
            if app.needs_you() > 0 {
                s.push_str(&format!(" {} needs you.", app.needs_you()));
            }
            s
        }
        (None, Focus::Weft) if app.pane_count() == 0 => " Nothing is running yet.".into(),
        (None, Focus::Weft) if app.waiting_here() => format!(
            " {} needs your answer (from the screen). Weft never answers for you.",
            app.harness_at(app.pane_focus).unwrap_or("the agent")
        ),
        (None, Focus::Weft) if app.drawer().is_some() => {
            " [↑↓] or the wheel to scroll · [←] back to the agent".into()
        }
        (None, Focus::Weft) if app.not_ready().is_some() => {
            let (name, state) = app.not_ready().expect("not ready");
            let mut said = state.say(&name).unwrap_or_default();
            said.push_str(if state.can_be_set_up() {
                " [R]EADY UP sets it up."
            } else {
                " Your agent still runs here."
            });
            format!(" {said}")
        }
        (None, Focus::Weft) if !app.show_work() => {
            " [W] brings the list back · [Space] still jumps to what needs you · [Ctrl+]] agent"
                .into()
        }
        (None, Focus::Weft) => {
            " [↑↓] pick · [Enter] open · [Space] next needs-you · [←] back · [Ctrl+]] agent".into()
        }
    };
    Paragraph::new(Line::styled(text, th.label()))
}

// --- the work list -----------------------------------------------------------

/// One row per unit of work: what you asked for, and where it stands. The
/// selected row expands in place.
fn work_list(app: &App, area: Rect) -> (Paragraph<'static>, Vec<(u16, u16, usize)>) {
    let th = app.theme;
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    let mut rows: Vec<(u16, u16, usize)> = Vec::new();

    if app.units().is_empty() {
        lines.push(Line::raw(""));
        if let Some((name, state)) = app.not_ready() {
            lines.push(Line::styled(
                format!(" {}", state.say(&name).unwrap_or_default()),
                th.label(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                " Your agent runs here either way. Nothing is written down until it is set up."
                    .to_string(),
                th.label(),
            ));
            return (Paragraph::new(lines), rows);
        }
        let (said, next) = if app.record_available() {
            ("Nothing asked for yet.", "[A]SK for something and Weft writes it down.")
        } else {
            // RingFrame owns the record. Without it there is nothing to read,
            // and saying so is better than an empty list that looks settled.
            (
                "No ringframe on PATH, so there is no record to read.",
                "The agents still run here. Install ringframe to see the work.",
            )
        };
        lines.push(Line::styled(format!(" {said}"), th.label()));
        lines.push(Line::raw(""));
        lines.push(Line::styled(format!(" {next}"), th.label()));
        return (Paragraph::new(lines), rows);
    }

    let offset = app.list_offset().min(app.units().len().saturating_sub(1));
    if offset > 0 {
        lines.push(Line::styled(format!("   ↑ {offset} above"), th.label()));
    }
    for (i, unit) in app.units().iter().enumerate().skip(offset) {
        let start = area.y + lines.len() as u16;
        if lines.len() + 2 > area.height as usize {
            let more = app.units().len() - i;
            lines.push(Line::styled(format!("   ↓ {more} more"), th.label()));
            break;
        }
        let picked = i == app.selected;
        let marker = if picked { " ▸ " } else { "   " };
        // The harness sits just past the title field rather than at the far
        // edge, so a wide list does not strand it across the screen.
        let title_width = width
            .saturating_sub(marker.len() + unit.harness.chars().count() + 2)
            .min(36);
        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), Style::default().fg(th.accent())),
            Span::styled(
                padded(&clip(&unit.title, title_width), title_width),
                if picked { th.selected() } else { Style::default().fg(th.primary()) },
            ),
            Span::styled(format!("{} ", unit.harness), th.label()),
        ]));
        for line in status_lines(app, unit, width) {
            lines.push(line);
        }
        if app.expanded() == Some(i) {
            for line in expanded_rows(app, unit, width) {
                lines.push(line);
            }
        }
        lines.push(Line::raw(""));
        let height = area.y + lines.len() as u16 - start;
        rows.push((start, height, i));
    }
    (Paragraph::new(lines), rows)
}

/// The status, and — when the work has been checked — who agreed and who
/// judged. Every verdict names the host that produced it.
fn status_lines(app: &App, unit: &Unit, width: usize) -> Vec<Line<'static>> {
    let th = app.theme;
    let style = if unit.needs_you() { th.needs_you() } else { th.label() };
    let head = format!("   {}{}", unit.marker(), unit.status().to_uppercase());
    let Some(check) = &unit.check else {
        return vec![Line::styled(head, style)];
    };
    let record = crate::record::Record::read(&app.root, &check.eval_id);
    let agreed = match &record {
        Some(r) => r.agreed(check.agreement),
        None => format!("agreement {:.2}", check.agreement),
    };
    let judged = match check.judged_by.clone().or_else(|| {
        record.as_ref().map(|r| r.judged_by().join(" and ")).filter(|s| !s.is_empty())
    }) {
        Some(h) => format!(" · judged by {h}"),
        None => String::new(),
    };
    let tail = format!("{agreed}{judged}");
    if head.chars().count() + 3 + tail.chars().count() <= width {
        return vec![Line::from(vec![
            Span::styled(head, style),
            Span::styled(format!(" · {tail}"), th.label()),
        ])];
    }
    // Narrow: the agreement and the host wrap rather than being cut, because
    // every verdict names the host that produced it.
    let mut lines = vec![Line::styled(head, style)];
    for l in wrap(&tail, width.saturating_sub(3)) {
        lines.push(Line::styled(format!("   {l}"), th.label()));
    }
    lines
}

/// What the expanded row adds: the judges' votes, what changed with no judge
/// able to explain it, where the fields came from, and the row's own actions.
fn expanded_rows(app: &App, unit: &Unit, width: usize) -> Vec<Line<'static>> {
    let th = app.theme;
    let mut lines = Vec::new();
    let indent = 5usize;
    if let Some(check) = &unit.check {
        if let Some(record) = crate::record::Record::read(&app.root, &check.eval_id) {
            // One column width for the whole block, so the votes line up.
            let agreed_width = record
                .items
                .iter()
                .map(|i| record.agreed_short_on(i).chars().count())
                .max()
                .unwrap_or(0);
            let room = width.saturating_sub(indent + 12 + agreed_width);
            for item in &record.items {
                let vote = item.plain_majority();
                let agreed = record.agreed_short_on(item);
                lines.push(Line::styled(
                    format!(
                        "{:indent$}{} {} {:<8} {}",
                        "",
                        mark_for(vote),
                        padded(&clip(&item.text, room), room),
                        vote,
                        agreed,
                        indent = indent
                    ),
                    th.label(),
                ));
            }
            if !record.unexplained.is_empty() {
                lines.push(Line::styled(
                    format!(
                        "{:indent$}{} changed and no judge could tie it to what you asked",
                        "",
                        record.unexplained.join(", "),
                        indent = indent
                    ),
                    th.label(),
                ));
            }
        }
    }
    lines.push(Line::styled(provenance(unit), th.label()));
    lines.push(Line::from(vec![
        Span::raw(" ".repeat(indent)),
        key(app, "[P] WORDING", Act::Wording),
        Span::raw("   "),
        key(app, "[J] JUDGES", Act::Judges),
        Span::raw("   "),
        key(app, "[F]IX THIS", Act::Fix),
        Span::raw("   "),
        key(app, "[D]ECIDE", Act::Decide),
    ]));
    lines
}

/// Where the row's fields came from, in the terms the record uses.
fn provenance(unit: &Unit) -> String {
    let mut parts = vec![format!("asked {}", clock(&unit.asked_at)), unit.sent_phrase().to_string()];
    if let Some(c) = &unit.check {
        parts.push(format!("recorded as {}, {:.2}", c.verdict.recorded(), c.agreement));
    }
    if let Some(d) = &unit.sealed {
        parts.push(format!("sealed {d}"));
    }
    format!("     {}", parts.join(" · "))
}

/// The time of day out of a ledger timestamp, which is what a row has room
/// for. Anything that is not an ISO instant is shown as it was recorded.
fn clock(recorded: &str) -> String {
    match (recorded.find('T'), recorded.chars().count()) {
        (Some(t), n) if n >= t + 6 => recorded.chars().skip(t + 1).take(5).collect(),
        _ => recorded.to_string(),
    }
}

fn mark_for(vote: &str) -> &'static str {
    match vote {
        "yes" => "✓",
        "no" => "✗",
        _ => "?",
    }
}

// --- the pane, the drawer, and first run --------------------------------------

fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    app.note_pane_area(area);
    let focus = app.pane_focus;
    app.with_pane_screen(focus, area.height, area.width, |screen| {
        frame.render_widget(PseudoTerminal::new(screen).block(Block::default()), area);
    });
}

/// A reading surface beside the list. It replaces the pane, not the screen.
fn drawer(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let Some(d) = app.drawer() else { return Paragraph::new("") };
    let room = area.height as usize;
    let mut lines: Vec<Line> = d
        .lines
        .iter()
        .skip(d.offset)
        .take(room)
        .map(|l| Line::styled(format!(" {}", clip(l, area.width.saturating_sub(1) as usize)), th.label()))
        .collect();
    let more = d.lines.len().saturating_sub(d.offset + lines.len());
    if more > 0 && !lines.is_empty() {
        lines.pop();
        lines.push(Line::styled(format!(" … {more} more lines · [↓]"), th.label()));
    }
    Paragraph::new(lines)
}

/// Screen 1: nothing is running yet, and the one thing to do about it.
fn first_run(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let found = App::available_agents();
    let mut lines: Vec<Line> = vec![Line::raw(""), Line::raw("")];
    for text in [
        "Weft keeps track of what you asked your coding agents",
        "for, what came back, and who checked it.",
        "",
        "You work in the agent as usual. Weft writes it down.",
        "",
    ] {
        lines.push(Line::styled(format!("   {text}"), th.label()));
    }
    lines.push(Line::styled("   ┌────────────────────────────────────────────┐".to_string(), th.rule()));
    lines.push(Line::from(vec![
        Span::styled("   │  ".to_string(), th.rule()),
        Span::styled(padded("START AN AGENT", 42), th.title()),
        Span::styled("│".to_string(), th.rule()),
    ]));
    lines.push(Line::styled("   │                                            │".to_string(), th.rule()));
    if found.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("   │  ".to_string(), th.rule()),
            Span::styled(padded("no coding agent found on PATH", 42), th.label()),
            Span::styled("│".to_string(), th.rule()),
        ]));
    }
    for (i, agent) in found.iter().enumerate() {
        let picked = i == app.modal_choice;
        lines.push(Line::from(vec![
            Span::styled("   │  ".to_string(), th.rule()),
            Span::styled(
                padded(&format!(" {} {agent}", if picked { "▸" } else { " " }), 42),
                if picked { th.selected() } else { th.label() },
            ),
            Span::styled("│".to_string(), th.rule()),
        ]));
    }
    lines.push(Line::styled("   │                                            │".to_string(), th.rule()));
    lines.push(Line::from(vec![
        Span::styled("   │  ".to_string(), th.rule()),
        Span::styled(padded("it runs here, with your own settings", 42), th.label()),
        Span::styled("│".to_string(), th.rule()),
    ]));
    lines.push(Line::styled("   └────────────────────────────────────────────┘".to_string(), th.rule()));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "   Click anything. Arrows and Enter work too. To select text in a pane, hold Shift."
            .to_string(),
        th.label(),
    ));
    let _ = area;
    Paragraph::new(lines)
}

// --- the bottom-anchored panels ----------------------------------------------

/// The confirmation and the quit question are anchored to the bottom so the
/// row they are about stays in view above them.
fn panel_lines(app: &App) -> Option<Vec<String>> {
    match app.modal.as_ref()? {
        Modal::Confirm(p) => {
            let mut lines = vec![p.what.to_uppercase()];
            lines.extend(p.why.iter().cloned());
            lines.push(String::new());
            lines.extend(String::from_utf8_lossy(&p.payload).lines().map(str::to_string));
            Some(lines)
        }
        Modal::SetUp { harness, commands, gap } => {
            let mut lines = match gap {
                crate::readiness::Gap::Cli => vec![
                    "RINGFRAME IS NOT INSTALLED".to_string(),
                    "Weft reads the record RingFrame writes. Without it there is no record.".into(),
                    String::new(),
                    "  curl -LsSf https://fab7.dev/rf/install.sh | sh".into(),
                    String::new(),
                    "Weft will not install it for you: it is a tool on your machine, not a".into(),
                    "change to one agent. Your agents still run here in the meantime.".into(),
                ],
                _ => {
                    let mut l = vec![
                        format!("SET {} UP FOR RINGFRAME", harness.to_uppercase()),
                        format!(
                            "Weft will run these two commands. They change {harness}, not this project."
                        ),
                        String::new(),
                    ];
                    l.extend(commands.iter().map(|c| format!("  {c}")));
                    l.push(String::new());
                    l.push("The ringframe CLI is already installed.".into());
                    l
                }
            };
            lines.retain(|l| !l.is_empty() || true);
            Some(lines)
        }
        Modal::Quit => Some(vec![
            "QUIT WEFT?".into(),
            "Your agents are running in the background. They can keep going".into(),
            "without Weft open.".into(),
            String::new(),
            "Quit, leave the agents running".into(),
            "Quit and stop the agents".into(),
            "Cancel".into(),
        ]),
        _ => None,
    }
}

fn panel(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let width = area.width.saturating_sub(2) as usize;
    match app.modal.as_ref() {
        Some(Modal::Quit) => {
            let mut lines = vec![
                Line::styled(" QUIT WEFT?".to_string(), th.title()),
                Line::styled(
                    " Your agents are running in the background. They can keep going".to_string(),
                    th.label(),
                ),
                Line::styled(" without Weft open.".to_string(), th.label()),
                Line::raw(""),
            ];
            for (i, choice) in [
                "Quit, leave the agents running",
                "Quit and stop the agents",
                "Cancel",
            ]
            .into_iter()
            .enumerate()
            {
                let picked = i == app.modal_choice;
                lines.push(Line::styled(
                    format!("   {} {choice}", if picked { "▸" } else { " " }),
                    if picked { th.selected() } else { th.label() },
                ));
            }
            Paragraph::new(lines)
        }
        Some(Modal::SetUp { .. }) => {
            let lines = panel_lines(app).unwrap_or_default();
            let mut drawn: Vec<Line> = Vec::new();
            for (i, l) in lines.iter().enumerate() {
                drawn.push(Line::styled(
                    format!(" {l}"),
                    if i == 0 { th.title() } else { th.label() },
                ));
            }
            Paragraph::new(drawn)
        }
        Some(Modal::Confirm(p)) => {
            let mut lines = vec![Line::styled(format!(" {}", p.what.to_uppercase()), th.title())];
            for why in &p.why {
                lines.push(Line::styled(format!(" {why}"), th.label()));
            }
            lines.push(Line::raw(""));
            let text = String::from_utf8_lossy(&p.payload);
            let body: Vec<String> = text.lines().flat_map(|l| wrap(l, width)).collect();
            let room = (area.height as usize).saturating_sub(lines.len());
            for (i, l) in body.iter().take(room).enumerate() {
                let more = body.len() > room && i + 1 == room;
                lines.push(Line::styled(
                    if more { format!(" {l}  ▼") } else { format!(" {l}") },
                    Style::default().fg(th.primary()),
                ));
            }
            Paragraph::new(lines)
        }
        _ => Paragraph::new(""),
    }
}

// --- the one remaining overlay -----------------------------------------------

/// Ask, Help, Note and the agent picker are interruptions by nature, so they
/// stay centred boxes. Everything else in v2 is drawn in place.
fn centred(app: &App, modal: &Modal) -> Option<(String, Vec<String>, Vec<String>)> {
    match modal {
        Modal::Confirm(_) | Modal::Quit | Modal::SetUp { .. } => None,
        Modal::Help => Some((
            " KEYS ".into(),
            vec![
                "[↑↓]     pick a row          [Enter]  expand it".into(),
                "[←]      back, everywhere    [Space]  next thing that needs you".into(),
                "[Tab]    next agent          [1]-[9]  that agent".into(),
                "[A]SK · [S]END · [C]HECK · [D]ECIDE".into(),
                "[P] the wording · [J] the judges · [F]IX THIS".into(),
                "[N]EW AGENT · [W]ORK shows or hides the list · [X] QUIT".into(),
                String::new(),
                format!("[{}] goes into the agent, and comes back.", app.toggle.label()),
                "In the agent every other key goes straight through, Esc included.".into(),
                "To select text in a pane, hold Shift.".into(),
            ],
            vec!["[Enter] CLOSE".into()],
        )),
        Modal::Note(text) => Some((
            " WEFT ".into(),
            text.lines().map(str::to_string).collect(),
            vec!["[Enter] CLOSE".into()],
        )),
        Modal::Ask { text, target } => {
            let mut lines = if text.is_empty() { Vec::new() } else { wrap(text, 58) };
            lines.push(format!("{text_cursor}_", text_cursor = ""));
            lines.push(String::new());
            let tabs: Vec<String> = (0..app.pane_count())
                .map(|i| {
                    format!(
                        "{}{} {}",
                        if i == *target { "▸ " } else { "" },
                        i + 1,
                        app.harness_at(i).unwrap_or("agent")
                    )
                })
                .collect();
            lines.push(format!("SEND TO     {}", tabs.join("    ")));
            Some((
                " WHAT DO YOU WANT DONE? ".into(),
                lines,
                vec!["[Enter] SEND            [←] CANCEL".into()],
            ))
        }
        Modal::StartAgent { .. } => Some((
            " START AN AGENT ".into(),
            vec!["It runs here, with your own settings.".into(), String::new()],
            App::available_agents().iter().map(|a| a.to_string()).collect(),
        )),
    }
}

fn overlay(
    frame: &mut Frame,
    app: &App,
    title: &str,
    lines: &[String],
    choices: &[String],
    area: Rect,
) {
    let th = app.theme;
    let height = (lines.len() + choices.len() + 4).min(area.height as usize) as u16;
    let width = 74u16.min(area.width.saturating_sub(4));
    let box_area = centre(area, width, height);
    frame.render_widget(Clear, box_area);

    let mut body: Vec<Line> = vec![Line::raw("")];
    for l in lines {
        body.push(Line::styled(format!("  {l}"), Style::default().fg(th.primary())));
    }
    if !choices.is_empty() {
        body.push(Line::raw(""));
    }
    let picker = matches!(app.modal, Some(Modal::StartAgent { .. }));
    for (i, c) in choices.iter().enumerate() {
        let picked = picker && i == app.modal_choice;
        body.push(Line::styled(
            if picker { format!("   {} {c}", if picked { "▸" } else { " " }) } else { format!("   {c}") },
            if picked { th.selected() } else { th.label() },
        ));
    }
    frame.render_widget(
        Paragraph::new(body).block(
            Block::bordered().border_style(th.rule()).title(Span::styled(title.to_string(), th.title())),
        ),
        box_area,
    );
}

// --- small things -------------------------------------------------------------

/// Left spans, right spans, and the gap between them.
fn spread(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: u16) -> Line<'static> {
    let used: usize = left.iter().chain(right.iter()).map(|s| s.content.chars().count()).sum();
    let gap = (width as usize).saturating_sub(used).max(1);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

fn padded(s: &str, n: usize) -> String {
    let len = s.chars().count();
    if len >= n {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(n - len))
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else if n == 0 {
        String::new()
    } else {
        s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if line.chars().count() + word.chars().count() + 1 > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn centre(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height: height.min(area.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, unit, press};
    use crate::ledger::{Check, Sent, Verdict};
    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// What the frame actually drew, one line per row. The wireframes in
    /// `spec/interface.md` are the reference; these read the same way.
    fn screen(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| draw(frame, app)).expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }
    }

    /// An Ask that has been judged, with the record the judges wrote on disk.
    fn judged() -> App {
        let mut a = app();
        let eval_id = "evl_1";
        let dir = a.root.join(".fab7/rf/evals").join(eval_id);
        std::fs::create_dir_all(&dir).expect("evals dir");
        std::fs::write(
            dir.join("record.json"),
            serde_json::json!({
                "eval_id": eval_id,
                "verdict": "drifted",
                "confidence": 0.67,
                "judgements": [
                    {"judge": {"angle": "coverage", "host": "codex", "model": "gpt-5.6-luna"}},
                    {"judge": {"angle": "drift", "host": "codex", "model": "gpt-5.6-luna"}},
                    {"judge": {"angle": "adversary", "host": "codex", "model": "gpt-5.6-luna"}}
                ],
                "items": [
                    {"id": "i1", "status": "active", "text": "returns the real build number",
                     "majority": "no", "agreement": 1.0,
                     "votes": [{"angle": "drift", "vote": "no",
                                "reason": "it still reads a literal \"dev\" at line 41"}]},
                    {"id": "i2", "status": "active", "text": "the endpoint responds at /health",
                     "majority": "yes", "agreement": 1.0, "votes": []}
                ],
                "drift": {"commission": [{"path": "README.md"}]}
            })
            .to_string(),
        )
        .expect("record");

        let mut u = unit(Sent::Arrived { exact: true });
        u.check = Some(Check {
            eval_id: eval_id.into(),
            verdict: Verdict::DoesntMatch,
            agreement: 0.67,
            judged_by: Some("codex".into()),
        });
        let mut ready = unit(Sent::ReadyToSend);
        ready.ask_id = "ask_2".into();
        ready.title = "readme fix".into();
        a.set_units(vec![u, ready]);
        a
    }

    #[test]
    fn the_title_bar_names_the_project_where_you_are_and_what_is_open() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let title = drawn.lines().next().expect("a title bar");
        assert!(title.contains("WEFT"), "{title}");
        assert!(title.contains("in Weft"), "{title}");
        assert!(title.contains("OPEN  2"), "{title}");
        assert!(title.contains("NEEDS YOU  2"), "{title}");
    }

    #[test]
    fn agents_are_tabs_over_the_pane_numbered_and_marked() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(tabs.contains("WORK"), "{tabs}");
        assert!(tabs.contains("▸ 1 codex"), "the active agent is marked: {tabs}");
        assert!(tabs.trim_end().ends_with('+'), "a new agent is one click away: {tabs}");
    }

    #[test]
    fn a_row_shows_what_you_asked_for_the_agent_and_where_it_stands() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("health endpoint"), "{drawn}");
        assert!(drawn.contains("codex"), "{drawn}");
        assert!(drawn.contains("DOESN'T MATCH WHAT YOU ASKED"), "{drawn}");
    }

    #[test]
    fn a_handoff_that_is_ready_carries_the_dot_the_vocabulary_gives_it() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("● READY TO SEND"), "{drawn}");
    }

    #[test]
    fn every_verdict_names_the_host_that_produced_it() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("2 of 3 judges agreed"), "agreement, never a score: {drawn}");
        assert!(drawn.contains("judged by codex"), "{drawn}");
    }

    #[test]
    fn nothing_on_the_board_claims_the_work_is_done_or_that_a_prompt_is_better() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24).to_lowercase();
        for forbidden in ["done", "better", "improved", "quality score", "correct"] {
            assert!(!drawn.contains(forbidden), "{forbidden} must not appear: {drawn}");
        }
    }

    #[test]
    fn an_expanded_row_shows_the_votes_the_provenance_and_its_own_keys() {
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("returns the real build number"), "{drawn}");
        assert!(drawn.contains("README.md changed and no judge could tie it"), "{drawn}");
        assert!(drawn.contains("asked 14:02"), "{drawn}");
        assert!(drawn.contains("recorded as drifted, 0.67"), "the recorded term travels too: {drawn}");
        for line in drawn.lines() {
            assert!(line.chars().count() <= 80, "nothing overflows the screen: {line}");
        }
        for key in ["[P] WORDING", "[J] JUDGES", "[F]IX THIS", "[D]ECIDE"] {
            assert!(drawn.contains(key), "{key} missing from the expanded row: {drawn}");
        }
    }

    #[test]
    fn the_action_bar_shows_a_key_for_everything_it_offers() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        for key in [
            "[A]SK", "[S]END", "[C]HECK", "[D]ECIDE", "[N]EW AGENT", "[W]ORK", "[H]ELP", "[X] QUIT",
        ] {
            assert!(drawn.contains(key), "{key} missing from the bar: {drawn}");
        }
    }

    #[test]
    fn the_bar_keeps_its_shape_whether_or_not_an_action_can_be_used() {
        // Unavailable actions are drawn muted and stay put, so the bar never
        // jumps under the pointer.
        let mut nothing = app();
        let mut something = judged();
        let bar = |a: &mut App| {
            screen(a, 80, 24).lines().nth(22).map(str::to_string).expect("a bar")
        };
        assert_eq!(bar(&mut nothing), bar(&mut something));
    }

    #[test]
    fn in_the_agent_weft_offers_no_keys_at_all() {
        let mut a = judged();
        a.focus = Focus::Agent;
        let drawn = screen(&mut a, 80, 24);
        let lines: Vec<&str> = drawn.lines().collect();
        assert_eq!(lines[22].trim(), "", "the action bar is empty in the agent");
        assert!(lines[23].contains("Esc included"), "{}", lines[22]);
        assert!(lines[0].contains("TO COME BACK"), "{}", lines[0]);
    }

    #[test]
    fn hiding_the_work_list_leaves_a_tab_to_bring_it_back() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('w'));
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("[W] WORK"), "{drawn}");
        assert!(!drawn.contains("health endpoint"), "the list is away: {drawn}");
        assert!(drawn.contains("[W] brings the list back"), "{drawn}");
    }

    #[test]
    fn the_drawer_replaces_the_pane_and_keeps_the_list_beside_it() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('j'));
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("THE JUDGES · health endpoint"), "{drawn}");
        assert!(drawn.contains("[←] BACK"), "{drawn}");
        assert!(drawn.contains("readme fix"), "the list stays: {drawn}");
        assert!(drawn.contains("A check is a judgement, not a guarantee."), "{drawn}");
        assert!(drawn.contains("none of them did it"), "{drawn}");
    }

    #[test]
    fn the_wording_and_the_judges_are_siblings_not_a_stack() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('j'));
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("[P] WORDING"), "the bar offers the other one: {drawn}");
        press(&mut a, KeyCode::Left);
        assert!(a.drawer().is_none(), "one [←] closes it, with nothing underneath");
    }

    #[test]
    fn the_confirmation_is_anchored_to_the_bottom_with_the_row_still_in_view() {
        let mut a = judged();
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Char('c'));
        let drawn = screen(&mut a, 80, 24);
        let lines: Vec<&str> = drawn.lines().collect();
        let row = lines.iter().position(|l| l.contains("readme fix")).expect("the row");
        let panel = lines.iter().position(|l| l.contains("CHECK THIS WORK")).expect("the panel");
        assert!(row < panel, "the row it is about stays above it:\n{drawn}");
        assert!(lines[22].contains("[Enter] DO IT"), "{}", lines[22]);
        assert!(lines[22].contains("[←] CANCEL"), "{}", lines[22]);
        assert!(lines[23].contains("never types into an agent without asking"), "{}", lines[23]);
    }

    #[test]
    fn side_by_side_puts_the_list_left_and_never_squeezes_the_pane() {
        let mut a = judged();
        let drawn = screen(&mut a, 120, 32);
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(tabs.contains('│'), "a divider between the two surfaces: {tabs}");
        let Layout::Split { list, pane } = layout::for_width(120) else { panic!() };
        assert_eq!((list, pane), (39, 80), "the spec's own screen 4");
    }

    #[test]
    fn first_run_offers_the_one_thing_there_is_to_do() {
        let (root, session) = crate::app::tests::test_session("first-run");
        let mut a = App::with_session(root, crate::keys::Toggle, session);
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("NO AGENT RUNNING"), "{drawn}");
        assert!(drawn.contains("START AN AGENT"), "{drawn}");
        assert!(drawn.contains("Weft writes it down"), "{drawn}");
        assert!(drawn.contains("[Enter] START"), "{drawn}");
        assert!(drawn.contains("Nothing is running yet."), "{drawn}");
    }

    #[test]
    fn a_list_longer_than_the_screen_says_what_is_above_and_below() {
        let mut a = app();
        let many: Vec<_> = (0..20)
            .map(|i| {
                let mut u = unit(Sent::TakenByAgent);
                u.ask_id = format!("ask_{i}");
                u.title = format!("unit {i}");
                u
            })
            .collect();
        a.set_units(many);
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("↓"), "there is more below: {drawn}");
        for _ in 0..4 {
            a.on_mouse(wheel(MouseEventKind::ScrollDown, 10, 6)).expect("wheel");
        }
        let scrolled = screen(&mut a, 80, 24);
        assert!(scrolled.contains("above"), "and above, once scrolled: {scrolled}");
        assert!(!scrolled.contains("unit 0"), "the first row scrolled away: {scrolled}");
    }

    #[test]
    fn clicking_an_action_word_does_what_its_key_does() {
        let mut a = judged();
        screen(&mut a, 80, 24);
        let bar = screen(&mut a, 80, 24);
        let row = bar.lines().position(|l| l.contains("[H]ELP")).expect("the bar") as u16;
        let column = bar.lines().nth(row as usize).unwrap().find("[H]ELP").unwrap() as u16;
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
        .expect("click");
        assert!(matches!(a.modal, Some(crate::app::Modal::Help)), "{:?}", a.modal);
    }

    #[test]
    fn clicking_the_needs_you_count_jumps_to_what_needs_you() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let title = drawn.lines().next().unwrap();
        let column = title.find("NEEDS YOU").expect("the count") as u16;
        assert_eq!(a.selected, 0);
        a.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
        .expect("click");
        assert_eq!(a.selected, 1, "it moved to the next thing that needs you");
    }

    #[test]
    fn a_harness_that_is_not_set_up_is_marked_and_says_so() {
        let mut a = judged();
        a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin));
        let drawn = screen(&mut a, 80, 24);
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(tabs.contains("codex ⚠"), "the tab carries it: {tabs}");
        assert!(drawn.contains("codex is not set up for RingFrame"), "{drawn}");
        assert!(drawn.contains("[R]EADY UP"), "and the key is shown where it is offered: {drawn}");
        assert!(drawn.contains("[R]EADY UP sets it up"), "{drawn}");
    }

    #[test]
    fn the_bar_keeps_its_eight_entries_when_a_harness_is_not_set_up() {
        // The bar is exactly full at 80 columns, so [R]EADY UP lives in the
        // hint. Every entry still has to be on screen and readable.
        let mut a = judged();
        a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin));
        let drawn = screen(&mut a, 80, 24);
        for key in ["[A]SK", "[S]END", "[C]HECK", "[D]ECIDE", "[N]EW AGENT", "[W]ORK", "[H]ELP", "[X] QUIT"] {
            assert!(drawn.contains(key), "{key} fell off the bar: {drawn}");
        }
    }

    #[test]
    fn setting_a_harness_up_shows_the_commands_before_running_them() {
        let mut a = judged();
        a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin));
        press(&mut a, KeyCode::Char('r'));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("SET CODEX UP FOR RINGFRAME"), "{drawn}");
        assert!(drawn.contains("codex plugin marketplace add fab7hq/fab7"), "{drawn}");
        assert!(drawn.contains("codex plugin add rf@fab7"), "{drawn}");
        assert!(drawn.contains("[Enter] DO IT"), "{drawn}");
        assert!(drawn.contains("[←] NOT NOW"), "{drawn}");
        assert!(drawn.contains("not this project"), "it says what it changes: {drawn}");
    }

    #[test]
    fn a_missing_cli_is_said_rather_than_offered() {
        let mut a = judged();
        a.set_readiness("codex", crate::readiness::Readiness::Missing(crate::readiness::Gap::Cli));
        press(&mut a, KeyCode::Char('r'));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("RINGFRAME IS NOT INSTALLED"), "{drawn}");
        assert!(drawn.contains("install.sh"), "it carries the command: {drawn}");
        assert!(!drawn.contains("[Enter] DO IT"), "and offers to run nothing: {drawn}");
        assert!(drawn.contains("[←] BACK"), "{drawn}");
    }

    #[test]
    fn the_ask_box_cancels_on_the_key_it_shows() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('a'));
        for c in "half an intent".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("[←] CANCEL"), "{drawn}");
        press(&mut a, KeyCode::Left);
        assert!(a.modal.is_none(), "[←] cancels whatever has been typed");
    }

    #[test]
    fn a_pane_waiting_for_an_answer_offers_to_take_you_there_and_nothing_else() {
        let mut a = judged();
        a.input(0, b"Allow command?\r\n").expect("type");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            a.pump();
            if a.waiting(0).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let tabs = screen(&mut a, 80, 24);
        assert!(tabs.lines().nth(1).is_some_and(|l| l.contains('●')), "the tab carries the dot: {tabs}");

        press(&mut a, KeyCode::Char(' '));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("[Enter] ANSWER IT"), "{drawn}");
        assert!(drawn.contains("[E]XPLAIN WHY IT SAYS THAT"), "{drawn}");
        assert!(drawn.contains("(from the screen)"), "the inference is labelled: {drawn}");
        assert!(drawn.contains("Weft never answers for you"), "{drawn}");
    }

    #[test]
    fn an_unavailable_action_puts_one_sentence_in_the_hint_and_no_dialog() {
        let mut a = app();
        press(&mut a, KeyCode::Char('s'));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("Nothing has been asked for yet."), "{drawn}");
        assert!(!drawn.contains("┌"), "no box was opened:\n{drawn}");
    }
}
