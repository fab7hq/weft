//! Drawing. Spec: `plans/weft/spec/interface.md` (v2).
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

use crate::app::{Act, App, Modal, Reading};
use crate::keys::Focus;
use crate::layout::{self, Layout};
use crate::ledger::Unit;
use crate::theme::{Theme, pair};

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
        let (para, rows) = first_run(app, geo.content);
        app.note_rows(rows);
        app.note_list_area(None);
        frame.render_widget(para, geo.content);
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
            // Hidden, the agent has the whole width and nothing is left over.
            // [W] brings it back, and the bar says so.
            geo.right = Some(content);
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
    // Where you are is shown by what is lit, not by a word: WEFT in the accent
    // means the keys are Weft's, and the agent's own name in the accent means
    // they are the agent's.
    let here = app.focus == Focus::Weft || app.pane_count() == 0;
    let left = vec![
        Span::styled(" ▚▞ ", Style::default().fg(th.accent())),
        Span::styled(
            "WEFT".to_string(),
            if here {
                Style::default().fg(th.accent()).add_modifier(Modifier::BOLD)
            } else {
                th.label()
            },
        ),
        Span::styled(format!("   {}", app.project), th.title()),
    ];
    let right = match (app.pane_count(), app.focus) {
        (0, _) => vec![Span::styled("NO AGENT RUNNING ", th.label())],
        // The counts keep their place whichever surface has the keys: they
        // are facts about the work, not about where you are.
        (_, _) => {
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

    // No header word: the list is directly below, and where the keys go is
    // shown by what is lit in the title bar.
    match (app.show_work(), geo.divider) {
        (_, Some(at)) => {
            push(&mut spans, &mut col, " ".repeat(at as usize), th.label());
            push(&mut spans, &mut col, "│ ".into(), th.rule());
        }
        _ => push(&mut spans, &mut col, " ".into(), th.label()),
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
        Some(Modal::Ask { .. }) => return plain("  [Enter] SEND   [←] CANCEL"),
        // No cancel: the agent has already gone, so the only question left is
        // what to put in its place.
        Some(Modal::Ended { .. }) => return plain("  [↑↓] pick   [Enter] DO IT   [←] CLOSE THE PANE"),
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
    // Active in the terminal's own foreground, inactive in grey — the way a
    // menu reads. It was the other way round, and everything looked off.
    let style = match app.unavailable(act) {
        Some(_) => th.label(),
        None => Style::default().fg(th.primary()),
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
        (Some(Modal::Ended { .. }), _) => {
            " Weft keeps no session of its own. The harness owns it; the record names it.".into()
        }
        (Some(Modal::SetUp { gap, .. }), _) => match gap {
            crate::readiness::Gap::Cli => " Run it in a terminal, then start an agent again.".into(),
            _ => " Weft never changes an agent without asking you first.".into(),
        },
        (Some(_), _) => " [Enter] closes this.".to_string(),
        (None, Focus::Agent) => {
            format!(" {} to switch between Weft and harness.", app.toggle.label())
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
            " [W] brings the list back · [Space] still jumps to what needs you".into()
        }
        (None, Focus::Weft) => {
            " [↑↓] pick · [Enter] open · [Space] next needs-you · [←] back".into()
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
    let record = app.record(&check.eval_id);
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
    if let Some(check) = &unit.check
        && let Some(record) = app.record(&check.eval_id) {
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
    lines.push(Line::styled(provenance(unit), th.label()));
    // `[O]PEN AGENT` comes first: for a row whose agent is gone it is the only
    // thing that leads anywhere, and that is the common case on reopening.
    lines.extend(keys_for_row(
        app,
        &[
            ("[O]PEN AGENT", Act::OpenAgent),
            ("[P] WORDING", Act::Wording),
            ("[J] JUDGES", Act::Judges),
            ("[F]IX THIS", Act::Fix),
            ("[D]ECIDE", Act::Decide),
        ],
        indent,
        width,
    ));
    lines
}

/// The row's own keys, wrapped rather than cut. Beside a pane the list is
/// narrow, and a key that runs off the edge is a key nobody knows is there.
fn keys_for_row(
    app: &App,
    offered: &[(&str, Act)],
    indent: usize,
    width: usize,
) -> Vec<Line<'static>> {
    const GAP: &str = "   ";
    let mut lines = Vec::new();
    let mut spans = vec![Span::raw(" ".repeat(indent))];
    let mut at = indent;
    for (label, act) in offered {
        let wants = label.chars().count() + if at > indent { GAP.len() } else { 0 };
        if at > indent && at + wants > width {
            lines.push(Line::from(std::mem::take(&mut spans)));
            spans.push(Span::raw(" ".repeat(indent)));
            at = indent;
        } else if at > indent {
            spans.push(Span::raw(GAP));
            at += GAP.len();
        }
        spans.push(key(app, label, *act));
        at += label.chars().count();
    }
    lines.push(Line::from(spans));
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
fn first_run(app: &App, area: Rect) -> (Paragraph<'static>, Vec<(u16, u16, usize)>) {
    let th = app.theme;
    let found = app.starts().to_vec();
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
    lines.push(Line::styled("   ┌──────────────────────────────────────────────────┐".to_string(), th.rule()));
    lines.push(Line::from(vec![
        Span::styled("   │  ".to_string(), th.rule()),
        Span::styled(padded("START AN AGENT", 48), th.title()),
        Span::styled("│".to_string(), th.rule()),
    ]));
    lines.push(Line::styled("   │                                                  │".to_string(), th.rule()));
    if found.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("   │  ".to_string(), th.rule()),
            Span::styled(padded("no coding agent found on PATH", 48), th.label()),
            Span::styled("│".to_string(), th.rule()),
        ]));
    }
    let mut choices = Vec::new();
    for (i, agent) in found.iter().enumerate() {
        let picked = i == app.modal_choice;
        choices.push((area.y + lines.len() as u16, 1, i));
        lines.push(Line::from(vec![
            Span::styled("   │  ".to_string(), th.rule()),
            Span::styled(
                padded(&clip(&format!(" {} {}", if picked { "▸" } else { " " }, agent.label), 48), 48),
                if picked { th.selected() } else { th.label() },
            ),
            Span::styled("│".to_string(), th.rule()),
        ]));
    }
    lines.push(Line::styled("   │                                                  │".to_string(), th.rule()));
    // The footer says what the picked choice would actually do: for a session
    // being picked up, which one, by what was last asked in it.
    let footer = match found.get(app.modal_choice).and_then(|c| c.session.as_ref()) {
        Some(s) => format!("{} · {}", crate::sessions::clock(&s.at), s.last),
        None => "it runs here, with your own settings".to_string(),
    };
    lines.push(Line::from(vec![
        Span::styled("   │  ".to_string(), th.rule()),
        Span::styled(padded(&clip(&footer, 48), 48), th.label()),
        Span::styled("│".to_string(), th.rule()),
    ]));
    lines.push(Line::styled("   └──────────────────────────────────────────────────┘".to_string(), th.rule()));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "   Click anything. Arrows and Enter work too. To select text in a pane, hold Shift."
            .to_string(),
        th.label(),
    ));
    (Paragraph::new(lines), choices)
}

// --- the bottom-anchored panels ----------------------------------------------

/// The confirmation and the quit question are anchored to the bottom so the
/// row they are about stays in view above them.
/// Every interruption is a panel in the same place: anchored to the bottom,
/// where the quit question appears. Weft has no centred box — one covers
/// exactly what the person was reading, which is what v1 did wrong.
///
/// This is the sizing pass. `panel` draws the same content.
fn panel_lines(app: &App) -> Option<Vec<String>> {
    let modal = app.modal.as_ref()?;
    let mut lines = vec![panel_title(app, modal)];
    lines.extend(panel_body(app, modal));
    lines.extend(panel_choices(app, modal).into_iter().map(|c| format!("  {c}")));
    Some(lines)
}

fn panel_title(_app: &App, modal: &Modal) -> String {
    match modal {
        Modal::Quit => "QUIT WEFT?".into(),
        Modal::Help => "KEYS".into(),
        Modal::Note(_) => "WEFT".into(),
        Modal::Ask { .. } => "WHAT DO YOU WANT DONE?".into(),
        Modal::Confirm(p) => p.what.to_uppercase(),
        Modal::StartAgent { .. } => "START AN AGENT".into(),
        Modal::SetUp { harness, gap, .. } => match gap {
            crate::readiness::Gap::Cli => "RINGFRAME IS NOT INSTALLED".into(),
            _ => format!("SET {} UP FOR RINGFRAME", harness.to_uppercase()),
        },
        Modal::Ended { harness, .. } => format!("{} HAS ENDED", harness.to_uppercase()),
    }
    .to_string()
}

fn panel_body(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        Modal::Quit => vec![
            "Your agents are running in the background. They can keep going".into(),
            "without Weft open.".into(),
            String::new(),
        ],
        Modal::Help => vec![
            "[↑↓] pick a row        [Enter] expand it        [←] back, everywhere".into(),
            "[Space] next thing that needs you               [Tab] next agent".into(),
            "[A]SK · [S]END · [C]HECK · [D]ECIDE · [R]EADY UP".into(),
            "[O]PEN AGENT reopens the session a row was asked in".into(),
            "[P] the wording · [J] the judges · [F]IX THIS".into(),
            "[N]EW AGENT · [W]ORK hides the list · [X] QUIT".into(),
            String::new(),
            format!("{} to switch between Weft and harness.", app.toggle.label()),
            "In the agent every other key goes through, Esc included.".into(),
            "To select text in a pane, hold Shift.".into(),
        ]
        .into_iter()
        .chain(routing_lines(app))
        .collect(),
        Modal::Note(text) => text.lines().map(str::to_string).collect(),
        Modal::Ask { text, target } => {
            // Folded, not wrapped: `wrap` rejoins words with single spaces, so
            // a run of spaces vanished on screen and then reappeared in the
            // confirmation. An input shows what was typed.
            let mut lines = if text.is_empty() { Vec::new() } else { fold(text, 72) };
            match lines.last_mut() {
                Some(last) => last.push('_'),
                None => lines.push("_".into()),
            }
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
            lines
        }
        Modal::Confirm(p) => {
            let mut lines = p.why.clone();
            lines.push(String::new());
            lines.extend(String::from_utf8_lossy(&p.payload).lines().map(str::to_string));
            lines
        }
        Modal::StartAgent { .. } => {
            vec!["It runs here, with your own settings.".into(), String::new()]
        }
        Modal::Ended { harness, session, .. } => {
            let mut lines = vec!["Nothing is running in this pane any more.".to_string()];
            match session {
                // The id comes from the receipt RingFrame's hook wrote, so
                // what is named here is a session that really happened.
                Some(s) => {
                    lines.push(String::new());
                    lines.push(format!(
                        "{harness} last wrote to session {} at {}:",
                        short_id(&s.id),
                        crate::sessions::clock(&s.at)
                    ));
                    lines.push(format!("  {}", clip(&s.last, 68)));
                }
                None => {
                    lines.push(String::new());
                    lines.push(format!(
                        "RingFrame has no record of a session for {harness} here, so"
                    ));
                    lines.push("there is none Weft can name to open again.".into());
                }
            }
            lines.push(String::new());
            lines
        }
        Modal::SetUp { harness, commands, gap } => match gap {
            crate::readiness::Gap::Cli => vec![
                "Weft reads the record RingFrame writes. Without it there is no record.".into(),
                String::new(),
                "  curl -LsSf https://fab7.dev/rf/install.sh | sh".into(),
                String::new(),
                "Weft will not install it for you: it is a tool on your machine, not a".into(),
                "change to one agent. Your agents still run here in the meantime.".into(),
            ],
            _ => {
                let mut l = vec![
                    format!("Weft will run these two commands. They change {harness}, not this project."),
                    String::new(),
                ];
                l.extend(commands.iter().map(|c| format!("  {c}")));
                l.push(String::new());
                l.push("The ringframe CLI is already installed.".into());
                l
            }
        },
    }
}

/// The choices a panel offers, if it is the kind that picks one.
fn panel_choices(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        Modal::Quit => vec![
            "Quit, leave the agents running".into(),
            "Quit and stop the agents".into(),
            "Cancel".into(),
        ],
        Modal::StartAgent { .. } => app.starts().iter().map(|c| c.label.clone()).collect(),
        Modal::Ended { harness, session, .. } => crate::app::ended_choices(session.as_ref())
            .into_iter()
            .map(|c| match c {
                crate::app::Ended::Resume => "Pick it up where it left off".to_string(),
                crate::app::Ended::Fresh => format!("Start a fresh {harness}"),
                crate::app::Ended::Close => "Close this pane".to_string(),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn panel(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let Some(modal) = app.modal.as_ref() else { return Paragraph::new("") };
    let mut drawn = vec![Line::styled(format!(" {}", panel_title(app, modal)), th.title())];

    let room = (area.height as usize).saturating_sub(1);
    let body = panel_body(app, modal);
    let body: Vec<String> = body
        .iter()
        .flat_map(|l| {
            if l.chars().count() > area.width.saturating_sub(2) as usize {
                fold(l, area.width.saturating_sub(2) as usize)
            } else {
                vec![l.clone()]
            }
        })
        .collect();
    let choices = panel_choices(app, modal);
    let for_body = room.saturating_sub(choices.len());
    for (i, l) in body.iter().take(for_body).enumerate() {
        let more = body.len() > for_body && i + 1 == for_body;
        drawn.push(Line::styled(
            if more { format!(" {l}  ▼") } else { format!(" {l}") },
            th.label(),
        ));
    }
    for (i, choice) in choices.iter().enumerate() {
        let picked = i == app.modal_choice;
        drawn.push(Line::styled(
            format!("   {} {choice}", if picked { "▸" } else { " " }),
            if picked { th.selected() } else { th.label() },
        ));
    }
    Paragraph::new(drawn)
}

// --- small things -------------------------------------------------------------

/// Left spans, right spans, and the gap between them.
/// What this project routes, shown where a person looks for how it is set up.
/// Nothing at all when it routes nothing, which is most projects.
fn routing_lines(app: &App) -> Vec<String> {
    let routed = app.routing().each();
    if routed.is_empty() {
        return Vec::new();
    }
    let said: Vec<String> = routed.iter().map(|(act, h)| format!("{act} → {h}")).collect();
    vec![String::new(), format!("This project routes  {}", said.join("   "))]
}

/// An id is too long to read and too long to fit. Its head is enough to tell
/// two sessions apart, and the whole of it is in the record.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

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
        // The daemon is the authority on what work exists, so an act can only
        // reach a unit that is really on the ledger. These two go there first,
        // with the same ids; the richer board below is what is drawn.
        crate::app::tests::record_units(&a, &["ask_1", "ask_2"]);
        a.refresh_for_test();
        a.set_units(vec![u, ready]);
        // The daemon reads a record for a unit it knows; this board is
        // fabricated, so the record is handed over the same way it would be.
        let text = std::fs::read_to_string(a.root.join(".fab7/rf/evals/evl_1/record.json"))
            .expect("record");
        let record: serde_json::Value = serde_json::from_str(&text).expect("json");
        let parsed = weft_core::record::Record::parse(&record).expect("a record");
        a.set_records(serde_json::json!({"evl_1": parsed}));
        a
    }

    #[test]
    fn the_title_bar_names_the_project_where_you_are_and_what_is_open() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let title = drawn.lines().next().expect("a title bar");
        assert!(title.contains("WEFT"), "{title}");
        assert!(!title.contains("in Weft"), "where you are is lit, not named: {title}");
        assert!(title.contains("OPEN  2"), "{title}");
        assert!(title.contains("NEEDS YOU  2"), "{title}");
    }

    #[test]
    fn agents_are_tabs_over_the_pane_numbered_and_marked() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(!tabs.contains("WORK"), "the row is the tabs, nothing else: {tabs}");
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
        assert!(lines[23].trim().starts_with("Ctrl+]"), "{}", lines[23]);
        assert!(!lines[0].contains("Ctrl"), "the title bar stops repeating it: {}", lines[0]);
        assert!(lines[0].contains("NEEDS YOU"), "the counts keep their place: {}", lines[0]);
        assert!(
            lines[23].contains("Ctrl+] to switch between Weft and harness"),
            "said once, in full: {}",
            lines[23]
        );
    }

    #[test]
    fn hiding_the_work_list_gives_the_agent_the_whole_width() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('w'));
        let drawn = screen(&mut a, 120, 32);
        assert!(!drawn.contains("health endpoint"), "the list is away: {drawn}");
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(!tabs.contains('│'), "and nothing is left behind: {tabs}");
        assert!(drawn.contains("[W] brings the list back"), "the way back is said: {drawn}");
        assert!(drawn.contains("[W]ORK"), "and shown on the bar: {drawn}");
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
        let panel = lines
            .iter()
            .position(|l| l.contains("CHECK THIS WORK"))
            .unwrap_or_else(|| panic!("the panel:\n{drawn}"));
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
    fn reopening_a_workspace_with_history_offers_to_pick_it_up() {
        let (root, session) = crate::app::tests::test_session("reopen");
        let dir = root.join(".fab7/rf/sessions/codex/01a0bdb6");
        std::fs::create_dir_all(&dir).expect("dir");
        let line = serde_json::json!({
            "session_id": "01a0bdb6", "time": "2026-09-20T07:27:14.769Z",
            "prompt": "$rf:ask research crypto trading", "cwd": root.to_string_lossy(),
        });
        std::fs::write(dir.join("prompts.jsonl"), format!("{line}\n")).expect("receipt");

        let mut a = App::with_session(root, crate::keys::Toggle, session);
        a.refresh_for_test();
        let picked = a.starts().iter().position(|c| c.session.is_some()).expect("one to pick up");
        a.modal_choice = picked;
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("codex · pick up where you left off"), "{drawn}");
        assert!(drawn.contains("codex · start fresh"), "{drawn}");
        // The footer says which session, by what was last asked in it.
        assert!(drawn.contains("07:27"), "{drawn}");
        assert!(drawn.contains("research crypto trading"), "{drawn}");
    }

    #[test]
    fn a_rows_keys_wrap_rather_than_run_off_a_narrow_list() {
        // Beside a pane the list is narrow, and a key that runs off the edge
        // is a key nobody knows is there.
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("[O]PEN AGENT"), "{drawn}");
        assert!(drawn.contains("[D]ECIDE"), "the last key is still on screen: {drawn}");
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
    fn an_ended_agent_names_the_session_it_could_be_opened_again_as() {
        let mut a = judged();
        a.modal = Some(Modal::Ended {
            pane: 0,
            harness: "codex".into(),
            session: Some(crate::sessions::Recorded {
                id: "01a0bdb6-1d1f-79c2-84b0-8b03496d7db0".into(),
                at: "2026-09-20T07:27:14.769Z".into(),
                last: "$rf:ask research crypto trading".into(),
            }),
        });
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("CODEX HAS ENDED"), "{drawn}");
        assert!(drawn.contains("01a0bdb6"), "names the recorded session: {drawn}");
        assert!(drawn.contains("07:27"), "and when it was last used: {drawn}");
        assert!(drawn.contains("research crypto trading"), "and what it was: {drawn}");
        assert!(drawn.contains("Pick it up where it left off"), "{drawn}");
        assert!(drawn.contains("Start a fresh codex"), "{drawn}");
        assert!(drawn.contains("Close this pane"), "{drawn}");
    }

    #[test]
    fn an_ended_agent_with_no_record_offers_no_session_to_resume() {
        // Weft keeps no session of its own, so with nothing in the record
        // there is nothing it can name — and it says that rather than
        // offering a resume that would have to guess at an id.
        let mut a = judged();
        a.modal = Some(Modal::Ended { pane: 0, harness: "codex".into(), session: None });
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("CODEX HAS ENDED"), "{drawn}");
        assert!(drawn.contains("no record of a session"), "{drawn}");
        assert!(!drawn.contains("Pick it up where it left off"), "{drawn}");
        assert!(drawn.contains("Start a fresh codex"), "{drawn}");
        assert!(drawn.contains("Close this pane"), "{drawn}");
    }

    #[test]
    fn help_says_what_this_project_routes() {
        let mut a = judged();
        let text = serde_json::json!({
            a.root.to_string_lossy().into_owned(): {"eval": "claude-code", "seal": "codex"}
        })
        .to_string();
        a.set_routing(crate::routing::read(&text, &a.root));
        press(&mut a, KeyCode::Char('h'));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("This project routes"), "{drawn}");
        assert!(drawn.contains("eval → claude-code"), "{drawn}");
        assert!(drawn.contains("seal → codex"), "{drawn}");
    }

    #[test]
    fn help_says_nothing_about_routing_when_there_is_none() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('h'));
        let drawn = screen(&mut a, 80, 24);
        assert!(!drawn.contains("This project routes"), "{drawn}");
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
    fn the_ask_box_shows_what_was_typed_spaces_and_all() {
        // Typed spaces vanished on screen and came back in the confirmation,
        // which made the confirmation look like it had changed the wording.
        let mut a = judged();
        press(&mut a, KeyCode::Char('a'));
        for c in "fix    the     build".chars() {
            press(&mut a, KeyCode::Char(c));
        }
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("fix    the     build"), "{drawn}");
    }

    #[test]
    fn folding_a_line_keeps_every_character() {
        // Both the ask box and the confirmation fold rather than wrap: one
        // shows what was typed, the other promises the exact wording.
        for text in ["a  b", "one two three four five", "   leading", "trailing   ",
                     "/plan Fix   the   thing", "averylongwordwithnospacesatall"] {
            let folded = fold(text, 8).join("");
            assert_eq!(folded, text.replace('\n', ""), "{text:?} came back as {folded:?}");
        }
    }

    #[test]
    fn what_can_be_used_is_lit_and_what_cannot_is_grey() {
        let a = judged();
        let available = key(&a, "[H]ELP", Act::Help);
        let unavailable = key(&a, "[S]END", Act::Send);
        assert_eq!(available.style.fg, Some(a.theme.primary()), "active reads as text");
        assert_eq!(unavailable.style.fg, Some(a.theme.muted()), "inactive reads as grey");
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
