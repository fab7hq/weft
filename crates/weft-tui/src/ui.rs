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

use crate::app::{Act, App, Modal};
use crate::keys::Focus;
use crate::layout::{self, Layout};
use crate::ledger::Unit;
use crate::theme::{Theme, pair};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let geo = geometry(app, area);
    let th = app.theme;

    frame.render_widget(title_bar(app, geo.title.width), geo.title);
    if geo.tabs.height > 0 {
        frame.render_widget(Paragraph::new(tab_row(app, &geo)), geo.tabs);
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

// --- the bars ----------------------------------------------------------------

fn title_bar(app: &App, width: u16) -> Paragraph<'static> {
    let th = app.theme;
    // Where you are is shown by what is lit, not by a word: WEFT in the accent
    // means the keys are Weft's, and the agent's own name in the accent means
    // they are the agent's.
    let here = app.focus == Focus::Weft || app.pane_count() == 0;
    let left = vec![
        Span::raw(" "),
        Span::styled("╸", Style::default().fg(th.accent())),
        Span::styled("┃", Style::default().fg(th.warp())),
        Span::styled("╺ ", Style::default().fg(th.accent())),
        Span::styled(
            "WEFT".to_string(),
            if here {
                Style::default().fg(th.accent()).add_modifier(Modifier::BOLD)
            } else {
                th.label()
            },
        ),
        // No path: with more than one project the name is wrong the moment
        // focus moves, and the sidebar names them where they can be acted on.
    ];
    let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
    let mut right = Vec::new();
    if app.sync_view().is_some_and(|v| v.needs_anything()) {
        right.push(Span::styled("↑ [U]PDATE   ", lit));
    }
    right.extend(match (app.pane_count(), app.focus) {
        (0, _) => vec![Span::styled("NO AGENT RUNNING ", th.label())],
        // The counts keep their place whichever surface has the keys: they
        // are facts about the work, not about where you are.
        (_, _) => {
            let mut spans =
                vec![Span::styled(pair("open", &app.open_count().to_string()), th.label())];
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
    });
    Paragraph::new(spread(left, right, width))
}

/// Agents are tabs over the pane. The focused surface's header is the accent.
fn tab_row(app: &App, geo: &Geo) -> Line<'static> {
    let th = app.theme;
    let in_weft = app.focus == Focus::Weft;
    let mut spans: Vec<Span> = Vec::new();
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

    // The detail view replaces the tabs with the unit it is about.
    if let Some(d) = app.detail() {
        let width = geo.tabs.width.saturating_sub(col);
        let back = format!("{} ", d.harness);
        let title = clip(&d.title, width.saturating_sub(back.chars().count() as u16 + 1) as usize);
        push(
            &mut spans,
            &mut col,
            padded(&title, (width as usize).saturating_sub(back.chars().count())),
            th.title(),
        );
        push(&mut spans, &mut col, back, th.label());
        return Line::from(spans);
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
        push(&mut spans, &mut col, text, style);
        push(&mut spans, &mut col, "    ".into(), th.label());
    }
    push(&mut spans, &mut col, "+".into(), th.label());
    Line::from(spans)
}

/// A refusal from the daemon, in words that say what to do about it.
fn refused(code: &str) -> String {
    match code {
        "PaneBlocked" => "Weft did not type it: the agent looks like it is waiting for you.".into(),
        "ModeNotEntered" => {
            "Weft did not type it: the agent never showed the command as on.".into()
        }
        "NoProcess" => "Weft did not type it: that agent is not running.".into(),
        "InjectionInFlight" => "Weft did not type it: it is still typing the last one.".into(),
        other => format!("Weft did not type it: {other}"),
    }
}

/// Every action shows its key. What cannot be done now is drawn muted and
/// stays where it was, so the shape of the bar never jumps.
fn action_bar(app: &App, width: u16) -> Paragraph<'static> {
    let th = app.theme;
    let plain = |text: &str| Paragraph::new(Line::styled(text.to_string(), th.label()));
    if app.focus == Focus::Agent {
        // One key is Weft's in the agent: the way back.
        return plain(&format!("  {}  BACK TO WEFT", app.toggle.label()));
    }
    match &app.modal {
        Some(Modal::Confirm(_)) => return plain("  [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Quit) => return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Weft) => return plain("  [↑↓] PICK   [Enter] DO IT   [ESC] CLOSE"),
        Some(Modal::CloseProject { .. }) => {
            return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL");
        }
        Some(Modal::OpenProject { .. }) => return plain("  [Enter] OPEN   [←] CANCEL"),
        Some(Modal::SendAnyway { .. }) => {
            return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL");
        }
        Some(Modal::StartAgent { .. }) => return plain("  [↑↓] PICK   [Enter] START   [←] CANCEL"),
        Some(Modal::RingFrame) => {
            return match app.sync_view() {
                Some(v) if v.needs_anything() && !v.running => plain("  [P]ROCEED   [ESC] CLOSE"),
                _ => plain("  [ESC] CLOSE"),
            };
        }
        Some(Modal::Ask { .. }) => return plain("  [Enter] SEND   [←] CANCEL"),
        Some(Modal::PickUp { .. }) => return plain("  [↑↓] PICK   [Enter] DO IT   [←] CANCEL"),
        Some(Modal::Help) | Some(Modal::Note(_)) => return plain("  [ESC] CLOSE"),
        None => {}
    }
    if app.waiting_here() {
        return plain("  [Enter] ANSWER IT   [Y] WHY WEFT THINKS SO");
    }
    if app.detail().is_some() {
        // Three keys, the same three on every state of the view. `[P]ROCEED`
        // is dim when there is nothing to carry forward, and the state line
        // at the top of the view is what says why.
        let mut left = vec![Span::raw("  ")];
        for (label, act) in [("[P]ROCEED", Act::Proceed), ("[F] FOLLOW UP", Act::FollowUp)] {
            left.push(key(app, label, act));
            left.push(Span::raw("   "));
        }
        left.push(Span::styled("[↑↓] SCROLL", th.label()));
        return Paragraph::new(spread(left, vec![Span::styled("[ESC] CLOSE ", th.label())], width));
    }

    // RingFrame's three acts, in the order they happen. Weft's own operations
    // are in the `[W]` menu, which is not an act on the record.
    let mut spans = vec![Span::raw("  ")];
    for (i, (label, act)) in
        [("[A]SK", Act::Ask), ("[E]VAL", Act::Eval), ("[S]EAL", Act::Seal)].into_iter().enumerate()
    {
        if i > 0 {
            spans.push(Span::raw("   "));
        }
        spans.push(key(app, label, act));
    }
    let menu = key(app, "[W]EFT ⌄", Act::WeftMenu);
    Paragraph::new(spread(spans, vec![menu, Span::raw(" ")], width))
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
    // What is sitting unsent in their agent. The daemon has always reported
    // this and nothing drew it, so a refused injection was silence.
    if let Some(refusal) = app.last_refusal() {
        return Paragraph::new(Line::styled(format!(" {}", refused(&refusal)), th.needs_you()));
    }
    if let Some(v) = app.newer.get().filter(|_| app.modal.is_none() && !app.waiting_here()) {
        let lit = Style::default().fg(th.accent()).add_modifier(Modifier::BOLD);
        return Paragraph::new(Line::styled(format!(" Weft {v} is out - run weft update"), lit));
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
        (Some(Modal::PickUp { .. }), _) => " Your work is in the record either way.".into(),
        (Some(Modal::RingFrame), _) => {
            " Weft never changes an agent without asking you first.".into()
        }
        (Some(_), _) => String::new(),
        (None, Focus::Agent) => " Every other key goes to the agent, Esc included.".into(),
        (None, Focus::Weft) if app.waiting_here() => format!(
            " {} needs your answer (from the screen). Weft never answers for you.",
            app.harness_at(app.pane_focus).unwrap_or("the agent")
        ),
        (None, Focus::Weft) if app.detail().is_some() => String::new(),
        (None, Focus::Weft) if app.not_ready().is_some() => {
            let (name, state) = app.not_ready().expect("not ready");
            let mut said = state.say(&name).unwrap_or_default();
            said.push_str(if state.can_be_set_up() {
                " [U]PDATE sets it up."
            } else {
                " Your agent still runs here."
            });
            format!(" {said}")
        }
        (None, Focus::Weft) if !app.show_work() => {
            " The list is hidden. [B] brings it back.".into()
        }
        (None, Focus::Weft) => " [H]ELP lists every key.".into(),
    };
    Paragraph::new(Line::styled(text, th.label()))
}

// --- the work list -----------------------------------------------------------

/// One row per unit of work: what you asked for, and where it stands. The
/// selected row expands in place.
fn work_list(app: &mut App, area: Rect, beside_agent: bool) -> Paragraph<'static> {
    let th = app.theme;
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    // What the arrows have to keep the selection inside, now that there is no
    // wheel. One line goes to the "N above" marker when the list is scrolled.
    app.note_list_rows(area.height.saturating_sub(1) as usize);

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
            return Paragraph::new(lines);
        }
        let (said, next) = if app.record_available() && app.pane_count() == 0 {
            // When the agent area is beside the list, it says how to start one.
            (
                "Nothing asked for yet.",
                if beside_agent { "" } else { "[N] starts an agent to ask." },
            )
        } else if app.record_available() {
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
        if !next.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!(" {next}"), th.label()));
        }
        return Paragraph::new(lines);
    }

    let all = app.rows();
    let offset = app.list_offset().min(all.len().saturating_sub(1));
    if offset > 0 {
        lines.push(Line::styled(format!("   ↑ {offset} above"), th.label()));
    }
    let room = area.height as usize;
    for (i, row) in all.iter().enumerate().skip(offset) {
        // The last line says what is below, when anything is.
        if lines.len() + 1 >= room && i + 1 < all.len() {
            lines.push(Line::styled(format!("   ↓ {} more", all.len() - i), th.label()));
            break;
        }
        lines.push(sidebar_row(app, row, i == app.selected, width));
    }
    Paragraph::new(lines)
}

/// One row of the sidebar. `▾` and `▸` mark the levels that hold something;
/// an action is a leaf and carries no marker. `⌫` is the one gesture that
/// removes anything, so it is the one thing with a glyph of its own.
fn sidebar_row(app: &App, row: &crate::app::Row, picked: bool, width: usize) -> Line<'static> {
    use crate::app::Row;
    let th = app.theme;
    let pick = |s: String| {
        if picked { Span::styled(s, th.selected()) } else { Span::styled(s, th.label()) }
    };
    match row {
        Row::Project { name, folded, open, waiting, .. } => {
            let mut tail = if *open == 1 { "1 open".to_string() } else { format!("{open} open") };
            if *waiting > 0 {
                tail.push_str(&format!(" · {waiting} ●"));
            }
            let head = format!(" {} {name}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(tail.chars().count() + 4);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(format!("{tail}  "), th.label()),
                Span::styled("⌫".to_string(), th.label()),
            ])
        }
        Row::Harness { name, folded, waiting, running, .. } => {
            let tail = if *waiting > 0 { format!("{waiting} ●  ") } else { "   ".into() };
            let gone = if *running { "" } else { " · not running" };
            let head = format!("   {} {name}{gone}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(tail.chars().count() + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(tail, th.needs_you()),
            ])
        }
        Row::Action { project, unit } => {
            // The row's own project, not the focused one: a unit in the
            // background was being drawn from whichever board was in front.
            let Some(unit) = app.units_of(*project).get(*unit) else { return Line::raw("") };
            let (word, waiting) = row_state(unit);
            let state = format!("{}{word}  ", if waiting { "● " } else { "" });
            let head = format!("      {}", unit.title);
            let room = width.saturating_sub(state.chars().count() + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(state, if waiting { th.needs_you() } else { th.label() }),
            ])
        }
    }
}

/// Where a unit stands, in one word: the furthest act that has happened, in
/// the past tense, and whether it is waiting on the person.
///
/// The same three acts the detail view names, so the sidebar and the view
/// teach one vocabulary between them rather than two.
fn row_state(unit: &Unit) -> (String, bool) {
    if unit.cancelled {
        return ("CANCELLED".into(), false);
    }
    let word = if unit.sealed.is_some() {
        "SEALED"
    } else if unit.check.is_some() {
        "EVALED"
    } else {
        "ASKED"
    };
    (word.into(), unit.needs_you())
}

// --- the pane, the drawer, and first run --------------------------------------

fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.pane_count() == 0 {
        frame.render_widget(no_agent(app, area), area);
        return;
    }
    let focus = app.pane_focus;
    app.with_pane_screen(focus, area.height, area.width, |screen| {
        frame.render_widget(PseudoTerminal::new(screen).block(Block::default()), area);
    });
}

/// One unit's whole story, blocking. Lines fold rather than being cut: this
/// is where the exact wording is read, and a prompt missing its right-hand
/// end is not the exact wording.
fn detail(app: &mut App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let room = area.height as usize;
    let width = (area.width.saturating_sub(1) as usize).max(1);
    // Folded before it is scrolled, so one press of `↓` moves one drawn line
    // rather than a whole paragraph of them.
    let (folded, offset) = {
        let Some(d) = app.detail() else { return Paragraph::new("") };
        (d.lines.iter().flat_map(|l| fold(l, width)).collect::<Vec<String>>(), d.offset)
    };
    app.note_detail_reach(folded.len().saturating_sub(room));

    let mut lines: Vec<Line> = folded
        .iter()
        .skip(offset)
        .take(room)
        .map(|l| {
            // A section heading is the one thing that carries weight here.
            let heading = ["ASK ", "EVAL ", "SEAL ", "● "].iter().any(|h| l.starts_with(h));
            let style = if heading { th.title() } else { th.label() };
            Line::styled(format!(" {l}"), style)
        })
        .collect();
    let more = folded.len().saturating_sub(offset + lines.len());
    if more > 0 && !lines.is_empty() {
        lines.pop();
        lines.push(Line::styled(format!(" … {more} more lines · [↓]"), th.label()));
    }
    Paragraph::new(lines)
}

/// Three warps, which are the record, and the one thread Weft carries across
/// them, going over and under. The mask colours each cell: `b` warp, `m` the
/// other threads, `t` the carried thread, `x` the word.
const LOGO: [(&str, &str); 7] = [
    ("  ┃   ╹   ┃", "  b   b   b"),
    ("━╸┃╺━━━━━╸┃╺━    ╻   ╻  ┏━━╸  ┏━━╸  ╺┳╸", "mmbmmmmmmmbmm    x   x  xxxx  xxxx  xxx"),
    ("  ╹   ╻   ╹      ┃   ┃  ┃     ┃      ┃", "  b   b   b      x   x  x     x      x"),
    ("━━━━━╸┃╺━━━━━•   ┃ ╻ ┃  ┣━━   ┣━━•   ┃", "ttttttbttttttt   x x x  xtt   xttt   x"),
    ("  ╻   ╹   ╻      ┃ ┃ ┃  ┃     ┃      ┃", "  b   b   b      x x x  x     x      x"),
    ("━╸┃╺━━━━━╸┃╺━    ┗━┻━┛  ┗━━╸  ╹      ╹", "mmbmmmmmmmbmm    xxxxx  xxxx  x      x"),
    ("  ┃   ╻   ┃", "  b   b   b"),
];

fn logo(th: Theme, indent: &str) -> Vec<Line<'static>> {
    LOGO.iter()
        .map(|(text, mask)| {
            let mut spans = vec![Span::raw(indent.to_string())];
            let mut run = String::new();
            let mut kind = ' ';
            for (ch, k) in text.chars().zip(mask.chars()) {
                if k != kind && !run.is_empty() {
                    spans.push(logo_span(th, kind, std::mem::take(&mut run)));
                }
                kind = k;
                run.push(ch);
            }
            if !run.is_empty() {
                spans.push(logo_span(th, kind, run));
            }
            Line::from(spans)
        })
        .collect()
}

fn logo_span(th: Theme, kind: char, text: String) -> Span<'static> {
    match kind {
        'b' => Span::styled(text, Style::default().fg(th.warp())),
        'm' => Span::styled(text, th.label()),
        't' => Span::styled(text, Style::default().fg(th.accent())),
        'x' => Span::styled(text, th.title()),
        _ => Span::raw(text),
    }
}

/// No agent is running: the mark, and the two ways in.
fn no_agent(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let mut lines = vec![Line::raw("")];
    if LOGO.len() + 5 <= area.height as usize {
        lines.extend(logo(th, "   "));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled("   [N] starts an agent.".to_string(), th.label()));
    if !app.units().is_empty() {
        lines.push(Line::styled(
            "   [Enter] on your work picks up the session it was asked in.".to_string(),
            th.label(),
        ));
    }
    Paragraph::new(lines)
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
        Modal::Weft => "WEFT".into(),
        Modal::CloseProject { .. } => "CLOSE THIS PROJECT?".into(),
        Modal::OpenProject { .. } => "OPEN A PROJECT".into(),
        Modal::SendAnyway { .. } => "THE AGENT LOOKS LIKE IT IS WAITING".into(),
        Modal::Help => "KEYS".into(),
        Modal::Note(_) => "WEFT".into(),
        Modal::Ask { .. } => "WHAT DO YOU WANT DONE?".into(),
        Modal::Confirm(p) => p.what.to_uppercase(),
        Modal::StartAgent { .. } => "START AN AGENT".into(),
        Modal::RingFrame => "SYNC RINGFRAME".into(),
        Modal::PickUp { harness, .. } => format!("{} IS NOT RUNNING", harness.to_uppercase()),
    }
    .to_string()
}

fn panel_body(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        // The menu is its choices; there is nothing to say above them.
        Modal::Weft => Vec::new(),
        Modal::SendAnyway { evidence, .. } => vec![
            "Weft read this on the screen and took it for a question meant".into(),
            "for you:".into(),
            String::new(),
            format!("  {evidence}"),
            String::new(),
            "Typing now would answer it. If that line is left over from".into(),
            "something already dealt with, say so and Weft will type.".into(),
            String::new(),
        ],
        Modal::OpenProject { text } => {
            vec![format!("{text}█"), String::new(), "The path of a repository to open.".into()]
        }
        Modal::CloseProject { name } => vec![
            name.clone(),
            String::new(),
            "The agents keep running and the record is untouched.".into(),
            "Weft stops showing it here.".into(),
            String::new(),
        ],
        Modal::Quit => vec![
            "Your agents are running in the background. They can keep going".into(),
            "without Weft open.".into(),
            String::new(),
        ],
        Modal::Help => help_lines(app).into_iter().chain(routing_lines(app)).collect(),
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
        Modal::PickUp { .. } => Vec::new(),
        Modal::RingFrame => ringframe_body(app.sync_view()),
    }
}

/// Three sections side by side, one key to a line.
fn help_lines(app: &App) -> Vec<String> {
    let toggle = app.toggle.label();
    let navigation: Vec<(&str, &str)> = vec![
        ("NAVIGATION", ""),
        ("↑ ↓", "move"),
        ("→ ←", "unfold · fold, back"),
        ("Enter", "open"),
        ("Space", "what needs you"),
        ("Tab", "next agent"),
        ("1-9", "that agent"),
        (toggle, "agent, and back"),
        ("⌫", "close the project"),
    ];
    let ringframe = [("RINGFRAME", ""), ("A", "ask"), ("E", "eval"), ("S", "seal")];
    let weft = [
        ("WEFT", ""),
        ("P", "proceed the action"),
        ("F", "follow up next action"),
        ("U", "sync RingFrame"),
        ("O", "open a project"),
        ("N", "new agent"),
        ("B", "sidebar"),
        ("W", "the Weft menu"),
        ("H", "help"),
        ("X", "quit"),
    ];
    let cell = |col: &[(&str, &str)], i: usize, key_w: usize, w: usize| match col.get(i) {
        Some((head, "")) => format!("{head:<w$}"),
        Some((key, what)) => format!("{key:<key_w$}{what:<0$}", w - key_w),
        None => " ".repeat(w),
    };
    let rows = navigation.len().max(weft.len());
    let mut lines: Vec<String> = (0..rows)
        .map(|i| {
            let line = format!(
                "{}  {}  {}",
                cell(&navigation, i, 9, 28),
                cell(&ringframe, i, 3, 13),
                cell(&weft, i, 3, 24)
            );
            line.trim_end().to_string()
        })
        .collect();
    lines.push(String::new());
    lines.push("In an agent every key is the agent's, Esc included. Weft takes no mouse.".into());
    lines
}

fn ringframe_body(view: Option<&weft_core::sync::View>) -> Vec<String> {
    use weft_core::sync::Mark;
    let Some(v) = view else { return vec!["Checking the latest release…".into()] };
    let mut l = vec![match (&v.latest, &v.plugin) {
        (Some(t), Some(p)) => format!("Latest release {t} · rf {p}"),
        (Some(t), None) => format!("Latest release {t}"),
        _ => "Weft could not reach the latest release.".into(),
    }];
    l.push(String::new());
    l.extend(v.rows.iter().map(|(name, now)| format!("  {name:<15} {now}")));
    l.push(String::new());
    if v.steps.is_empty() {
        l.push("Everything is up to date.".into());
        l.push("Open agents keep their skills until you restart them.".into());
        return l;
    }
    let failed = v.steps.iter().any(|s| s.mark == Mark::Failed);
    l.push(match (v.running, failed) {
        (true, _) => "Running:".into(),
        (_, true) => "Stopped. [P]ROCEED runs it again from the failed command:".into(),
        _ => "[P]ROCEED will run, in order:".into(),
    });
    for s in &v.steps {
        let mark = match s.mark {
            Mark::Waiting => '·',
            Mark::Running => '▸',
            Mark::Done => '✓',
            Mark::Failed => '✗',
        };
        l.push(format!("  {mark} {}", s.line()));
        l.extend(s.said.lines().map(|said| format!("      {said}")));
    }
    l.push(String::new());
    l.push("Your personal and project rules are not touched.".into());
    l
}

/// The choices a panel offers, if it is the kind that picks one.
fn panel_choices(app: &App, modal: &Modal) -> Vec<String> {
    match modal {
        Modal::Weft => crate::app::WEFT_MENU
            .iter()
            .map(|(key, label, _)| format!("[{key}]  {label}"))
            .collect(),
        Modal::CloseProject { .. } => vec!["Close it".into(), "Cancel".into()],
        Modal::SendAnyway { .. } => {
            vec!["Type it anyway".into(), "Cancel — I will answer the agent".into()]
        }
        Modal::OpenProject { .. } => Vec::new(),
        Modal::Quit => vec![
            "Quit, leave the agents running".into(),
            "Quit and stop the agents".into(),
            "Cancel".into(),
        ],
        Modal::StartAgent { .. } => app.fresh_starts().iter().map(|c| c.label.clone()).collect(),
        Modal::PickUp { harness, session, .. } => crate::app::pick_up_choices(session.as_ref())
            .into_iter()
            .map(|c| match (c, session) {
                (crate::app::PickUp::Resume, Some(s)) => clip(
                    &format!(
                        "Resume the session this was asked in · {} · {}",
                        crate::sessions::clock(&s.at),
                        s.last
                    ),
                    72,
                ),
                _ => format!("Start a fresh {harness}"),
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
    if len >= n { s.to_string() } else { format!("{s}{}", " ".repeat(n - len)) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, press, unit};
    use crate::ledger::{Check, Sent, Verdict};
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// What the frame actually drew, one line per row.
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

    /// An Ask that has been judged, with the record the judges wrote on disk.
    fn judged() -> App {
        let mut a = app();
        let eval_id = "evl_1";
        let dir = a.root().join(".fab7/rf/evals").join(eval_id);
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
        let text = std::fs::read_to_string(a.root().join(".fab7/rf/evals/evl_1/record.json"))
            .expect("record");
        let record: serde_json::Value = serde_json::from_str(&text).expect("json");
        let parsed = weft_core::record::Record::parse(&record).expect("a record");
        a.set_records(serde_json::json!({"evl_1": parsed}));
        a
    }

    #[test]
    fn the_title_bar_says_what_is_open_and_never_which_directory() {
        let mut a = judged();
        for width in [60, 80, 120, 200] {
            let drawn = screen(&mut a, width, 24);
            let title = drawn.lines().next().expect("a title bar");
            assert!(title.contains("WEFT"), "{title}");
            assert!(!title.contains("in Weft"), "where you are is lit, not named: {title}");
            // With more than one project a name is wrong the moment focus
            // moves, and the sidebar names them where they can be acted on.
            let name = a.project_name().to_string();
            assert!(!title.contains(&name), "the title named a directory at {width}: {title}");
        }
        let drawn = screen(&mut a, 80, 24);
        let title = drawn.lines().next().expect("a title bar");
        assert!(title.contains("OPEN  2"), "{title}");
        assert!(title.contains("NEEDS YOU  2"), "{title}");
    }

    #[test]
    fn six_units_in_two_projects_fit_on_one_screen() {
        let mut a = judged();
        let (other, _) = crate::app::tests::test_session("second");
        a.open_project(&other.to_string_lossy());
        a.add("codex", "/bin/cat").expect("an agent in the second project");
        let mut more: Vec<crate::ledger::Unit> = Vec::new();
        for i in 0..4 {
            let mut u = crate::app::tests::unit(crate::ledger::Sent::ReadyToSend);
            u.ask_id = format!("ask_{i}");
            u.title = format!("unit {i}");
            more.push(u);
        }
        a.set_units(more);
        let drawn = screen(&mut a, 80, 24);
        // Two project rows, two harness rows, six units: ten lines in a body
        // with room for twenty, and none of them elided. The old row was four
        // to five lines per unit, which did not fit at two units.
        assert_eq!(a.rows().len(), 10, "{:?}", a.rows());
        assert!(!drawn.contains("more"), "something was elided:\n{drawn}");
        assert!(!drawn.contains("above"), "something scrolled away:\n{drawn}");
        for row in a.rows() {
            if let crate::app::Row::Action { project, unit } = row {
                let title = a.units_of(project)[unit].title.clone();
                assert!(drawn.contains(&title), "{title} is missing:\n{drawn}");
            }
        }
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
    fn a_row_is_one_line_saying_what_and_where_it_stands() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        let row = drawn.lines().find(|l| l.contains("health endpoint")).expect("the row");
        // Whose it is is the level above, not repeated on every child.
        assert!(drawn.lines().any(|l| l.contains("▾ codex")), "{drawn}");
        // The act that is waiting, named as the bar names it. What the judges
        // said is a section of the detail view, not four more lines here.
        assert!(row.contains("● EVALED"), "{row}");
        assert!(!drawn.contains("DOESN'T MATCH"), "the verdict is not on the row:\n{drawn}");
    }

    #[test]
    fn a_handoff_that_is_ready_carries_the_dot_the_vocabulary_gives_it() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("● ASKED"), "{drawn}");
    }

    #[test]
    fn every_verdict_names_the_host_that_produced_it() {
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        let read = a.detail().expect("the view").lines.join("\n");
        assert!(read.contains("judged by codex"), "{read}");
        // Agreement, never a score.
        assert!(read.contains("agreed"), "{read}");
        assert!(!read.contains("confidence"), "{read}");
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
    fn the_action_bar_shows_a_key_for_everything_it_offers() {
        let mut a = judged();
        let drawn = screen(&mut a, 80, 24);
        for key in ["[A]SK", "[E]VAL", "[S]EAL", "[W]EFT"] {
            assert!(drawn.contains(key), "{key} missing from the bar: {drawn}");
        }
        // The verb and the reading are not on it: proceeding happens in the
        // detail view, and `[Enter]` is how that view is reached.
        assert!(!drawn.contains("[P]ROCEED"), "{drawn}");
        assert!(!drawn.contains("[D]ETAIL"), "{drawn}");
    }

    #[test]
    fn the_bar_keeps_its_shape_whether_or_not_an_action_can_be_used() {
        // Unavailable actions are drawn muted and stay put, so the bar never
        // jumps under the pointer.
        let mut nothing = app();
        let mut something = judged();
        let bar =
            |a: &mut App| screen(a, 80, 24).lines().nth(22).map(str::to_string).expect("a bar");
        assert_eq!(bar(&mut nothing), bar(&mut something));
    }

    #[test]
    fn in_the_agent_weft_offers_no_keys_at_all() {
        let mut a = judged();
        a.focus = Focus::Agent;
        let drawn = screen(&mut a, 80, 24);
        let lines: Vec<&str> = drawn.lines().collect();
        assert_eq!(lines[22].trim(), "Ctrl+]  BACK TO WEFT", "the one key Weft keeps");
        assert!(
            !lines[23].contains("Ctrl"),
            "and the line below does not repeat it: {}",
            lines[23]
        );
        assert!(!lines[0].contains("Ctrl"), "the title bar stops repeating it: {}", lines[0]);
        assert!(lines[0].contains("NEEDS YOU"), "the counts keep their place: {}", lines[0]);
    }

    #[test]
    fn hiding_the_work_list_gives_the_agent_the_whole_width() {
        let mut a = judged();
        press(&mut a, KeyCode::Char('b'));
        let drawn = screen(&mut a, 120, 32);
        assert!(!drawn.contains("health endpoint"), "the list is away: {drawn}");
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(!tabs.contains('│'), "and nothing is left behind: {tabs}");
        assert!(drawn.contains("[B] brings it back"), "the way back is said: {drawn}");
    }

    #[test]
    fn the_detail_view_blocks_and_holds_all_three_acts() {
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        let drawn = screen(&mut a, 120, 32);
        assert!(drawn.contains("health endpoint"), "{drawn}");
        // One unit and one decision. Nothing behind it is reachable, which is
        // what keeps the bar's acts and `[P]ROCEED` from both being live.
        assert!(!drawn.contains("readme fix"), "the list is not behind it:\n{drawn}");
        let lines = &a.detail().expect("the view").lines;
        for section in ["ASK", "EVAL", "SEAL"] {
            assert!(
                lines.iter().any(|l| l.starts_with(section)),
                "{section} is missing from the view"
            );
        }
        assert!(drawn.contains("ASK"), "and the top of it is on the screen:\n{drawn}");
        assert!(drawn.contains("A check is a judgement, not a guarantee."), "{drawn}");
        assert!(drawn.contains("[P]ROCEED"), "{drawn}");
        assert!(drawn.contains("[ESC] CLOSE"), "{drawn}");
    }

    #[test]
    fn a_reading_too_wide_for_the_view_folds_rather_than_being_cut() {
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        // Narrow enough that the longest reason cannot sit on one line. This
        // is where the exact wording is read, so nothing may be lost off the
        // right-hand edge.
        let drawn = screen(&mut a, 46, 70);
        // The longest line in the fixture is 56 characters in a 45-column
        // view. Every word of it is on the screen, across two lines.
        let flat: String = drawn.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        assert!(
            flat.contains("CHANGED WITH NO JUDGE ABLE TO TIE IT TO WHAT YOU ASKED"),
            "the line lost something in the fold:\n{drawn}"
        );
        for line in drawn.lines() {
            assert!(
                !line.trim_end().ends_with('…') || line.contains("more lines"),
                "a reading was cut instead of folded:\n{line}"
            );
        }
    }

    #[test]
    fn scrolling_past_the_last_line_stops_there_instead_of_emptying_the_panel() {
        let mut a = judged();
        press(&mut a, KeyCode::Enter);
        // A short panel, so there is somewhere to scroll to.
        let _ = screen(&mut a, 120, 14);
        for _ in 0..200 {
            press(&mut a, KeyCode::Down);
        }
        let drawn = screen(&mut a, 120, 14);
        assert!(
            drawn.contains("A check is a judgement"),
            "the end of the reading stays in view:\n{drawn}"
        );
    }

    #[test]
    fn the_confirmation_is_anchored_to_the_bottom_with_the_row_still_in_view() {
        let mut a = judged();
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Char('e'));
        let drawn = screen(&mut a, 80, 24);
        let lines: Vec<&str> = drawn.lines().collect();
        let row = lines.iter().position(|l| l.contains("readme fix")).expect("the row");
        let panel = lines
            .iter()
            .position(|l| l.contains("EVAL THIS WORK"))
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
    fn every_cell_of_the_logo_has_a_colour_and_every_colour_a_cell() {
        for (text, mask) in LOGO {
            assert_eq!(text.chars().count(), mask.chars().count(), "{text}");
            for (c, k) in text.chars().zip(mask.chars()) {
                assert_eq!(c == ' ', k == ' ', "{text}");
            }
        }
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
        // Down to the last row, which the arrows have to scroll to now that
        // there is no wheel.
        while a.selected + 1 < a.rows().len() {
            press(&mut a, KeyCode::Down);
        }
        let scrolled = screen(&mut a, 80, 24);
        assert!(scrolled.contains("above"), "and above, once scrolled: {scrolled}");
        assert!(!scrolled.contains("unit 0"), "the first row scrolled away: {scrolled}");
    }

    #[test]
    fn space_goes_to_the_ask_that_needs_you_and_opens_it() {
        let mut a = judged();
        press(&mut a, KeyCode::Char(' '));
        assert_eq!(
            a.selected_unit().map(|u| u.title.clone()),
            Some("readme fix".into()),
            "it moved past the selection to the next thing that needs you"
        );
        assert!(a.detail().is_some(), "and opened it");
    }

    #[test]
    fn a_harness_that_is_not_set_up_is_marked_and_says_so() {
        let mut a = judged();
        a.set_readiness(
            "codex",
            crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin),
        );
        let drawn = screen(&mut a, 80, 24);
        let tabs = drawn.lines().nth(1).expect("a tab row");
        assert!(tabs.contains("codex ⚠"), "the tab carries it: {tabs}");
        assert!(drawn.contains("codex is not set up for RingFrame"), "{drawn}");
        assert!(drawn.contains("[U]PDATE sets it up"), "{drawn}");
    }

    #[test]
    fn the_bar_keeps_every_entry_when_a_harness_is_not_set_up() {
        // Weft's own operations moved into the [W] menu, so the bar fits at
        // 80 columns with room to spare. Every entry still has to be on
        // screen and readable.
        let mut a = judged();
        a.set_readiness(
            "codex",
            crate::readiness::Readiness::Missing(crate::readiness::Gap::Plugin),
        );
        let drawn = screen(&mut a, 80, 24);
        for key in ["[A]SK", "[E]VAL", "[S]EAL", "[W]EFT"] {
            assert!(drawn.contains(key), "{key} fell off the bar: {drawn}");
        }
    }

    #[test]
    fn help_says_what_this_project_routes() {
        let mut a = judged();
        let text = serde_json::json!({
            a.root().to_string_lossy().into_owned(): {"eval": "claude-code", "seal": "codex"}
        })
        .to_string();
        a.set_routing(crate::routing::read(&text, a.root()));
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
        for text in [
            "a  b",
            "one two three four five",
            "   leading",
            "trailing   ",
            "/plan Fix   the   thing",
            "averylongwordwithnospacesatall",
        ] {
            let folded = fold(text, 8).join("");
            assert_eq!(folded, text.replace('\n', ""), "{text:?} came back as {folded:?}");
        }
    }

    #[test]
    fn what_can_be_used_is_lit_and_what_cannot_is_grey() {
        let a = judged();
        let available = key(&a, "[A]SK", Act::Ask);
        // Already set up, so there is nothing to ready.
        let unavailable = key(&a, "[U]PDATE", Act::ReadyUp);
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
        assert!(
            tabs.lines().nth(1).is_some_and(|l| l.contains('●')),
            "the tab carries the dot: {tabs}"
        );

        press(&mut a, KeyCode::Char(' '));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("[Enter] ANSWER IT"), "{drawn}");
        assert!(drawn.contains("[Y] WHY WEFT THINKS SO"), "{drawn}");
        assert!(drawn.contains("(from the screen)"), "the inference is labelled: {drawn}");
        assert!(drawn.contains("Weft never answers for you"), "{drawn}");
    }

    #[test]
    fn an_unavailable_action_puts_one_sentence_in_the_hint_and_no_dialog() {
        let mut a = app();
        press(&mut a, KeyCode::Char('p'));
        let drawn = screen(&mut a, 80, 24);
        assert!(drawn.contains("Nothing has been asked for yet."), "{drawn}");
        assert!(!drawn.contains("┌"), "no box was opened:\n{drawn}");
    }
}
