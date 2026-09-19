//! Drawing. Spec: `plans/weft/spec/interface.md`.
//!
//! The rule this module enforces: a field that cannot be traced to a ledger
//! event may not be rendered.

use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use crate::app::{App, Modal};
use crate::keys::Focus;
use crate::layout::{self, Layout};
use crate::ledger::Sent;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // The agents strip only exists once there is more than one agent to
    // confuse: with one, the title bar already names it.
    let strip = if app.panes.len() > 1 { 1 } else { 0 };
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(strip),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(title_bar(app), rows[0]);
    if strip == 1 {
        frame.render_widget(agents_strip(app), rows[1]);
    }
    frame.render_widget(rule(area.width), rows[2]);
    body(frame, app, rows[3]);
    frame.render_widget(rule(area.width), rows[4]);
    frame.render_widget(action_bar(app), rows[5]);
    frame.render_widget(hint(app), rows[6]);

    if let Some(modal) = app.modal.clone() {
        overlay(frame, app, &modal, area);
    }
}

fn title_bar(app: &App) -> Paragraph<'static> {
    let state = match app.focus {
        Focus::Weft => "in Weft",
        Focus::Agent => "in the agent",
    };
    let who = match app.panes.get(app.pane_focus) {
        Some(p) if app.panes.len() == 1 => format!(" · {}", p.harness),
        _ => String::new(),
    };
    let left = format!(" weft · {}{} · {}", app.project, who, state);
    let right = match app.focus {
        Focus::Weft if app.panes.is_empty() => "no agent running ".to_string(),
        Focus::Agent if app.waiting(app.pane_focus).is_some() => {
            "● needs your answer (from the screen) ".to_string()
        }
        Focus::Agent => format!("{} to come back ", app.toggle.label()),
        Focus::Weft => {
            let open = app.units.iter().filter(|u| u.sealed.is_none() && !u.cancelled).count();
            let needs = app.needs_you();
            if needs > 0 {
                format!("{open} open · {needs} needs you ")
            } else {
                format!("{open} open ")
            }
        }
    };
    Paragraph::new(Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::raw(right),
    ]))
}

/// Which agents are running, which one has focus, and how to change it.
fn agents_strip(app: &App) -> Paragraph<'static> {
    let mut spans = vec![Span::raw(" agents  ")];
    for (i, pane) in app.panes.iter().enumerate() {
        let active = i == app.pane_focus;
        let waiting = app.waiting(i).is_some();
        let mark = if active { "▸" } else { " " };
        let flag = if waiting { " ●" } else { "" };
        let label = format!("{mark}{} {}{flag}   ", i + 1, pane.harness);
        spans.push(if active {
            Span::styled(label, Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
        } else {
            Span::raw(label)
        });
    }
    spans.push(Span::raw("  Tab or 1-9 to switch"));
    Paragraph::new(Line::from(spans))
}

fn rule(width: u16) -> Paragraph<'static> {
    Paragraph::new("─".repeat(width as usize))
}

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    // With no agent running there is nothing to sit beside, so the first-run
    // message gets the whole width instead of being clipped to the rail.
    if app.panes.is_empty() {
        frame.render_widget(list(app, area.width), area);
        return;
    }
    match layout::for_width(area.width) {
        Layout::Split { rail, pane } => {
            let cols = RLayout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(rail),
                    Constraint::Length(1),
                    Constraint::Length(pane),
                ])
                .split(area);
            frame.render_widget(list(app, rail), cols[0]);
            frame.render_widget(Paragraph::new(vlines(area.height)), cols[1]);
            agent(frame, app, cols[2]);
        }
        Layout::Single { width } => match app.focus {
            Focus::Weft => frame.render_widget(list(app, width), area),
            Focus::Agent => agent(frame, app, area),
        },
    }
}

fn vlines(height: u16) -> String {
    std::iter::repeat_n("│", height as usize).collect::<Vec<_>>().join("\n")
}

/// One entry per unit of work. Narrow gets two lines; wide gets a table.
fn list(app: &App, width: u16) -> Paragraph<'static> {
    let mut lines: Vec<Line> = vec![Line::raw("")];

    if app.units.is_empty() {
        if app.panes.is_empty() {
            lines.push(Line::raw("   Nothing is running yet."));
            lines.push(Line::raw(""));
            lines.push(Line::raw("   Press N to start an agent."));
        } else {
            lines.push(Line::raw("   Nothing asked for yet."));
            lines.push(Line::raw(""));
            lines.push(Line::raw("   Press A to ask for something."));
        }
        return Paragraph::new(lines);
    }

    let wide = width >= 60;
    for (i, unit) in app.units.iter().enumerate() {
        let picked = i == app.selected;
        let marker = if picked {
            "▸"
        } else if unit.needs_you() {
            "●"
        } else {
            " "
        };
        let style = if picked {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        if wide {
            lines.push(Line::styled(
                format!(
                    " {marker} {:<24} {:<11} {}",
                    clip(&unit.title, 24),
                    clip(&unit.harness, 11),
                    unit.status()
                ),
                style,
            ));
            if let Some(c) = &unit.check {
                lines.push(Line::raw(format!("{:>39}{}", "", agreement_phrase(app, c))));
            }
        } else {
            let w = width.saturating_sub(3) as usize;
            lines.push(Line::styled(format!("{marker} {}", clip(&unit.title, w)), style));
            lines.push(Line::raw(format!("  {}", clip(&unit.harness, w))));
            lines.push(Line::raw(format!("  {}", clip(&unit.status(), w))));
        }
        lines.push(Line::raw(""));
    }
    Paragraph::new(lines)
}

/// Judge agreement, phrased as agreement. Never correctness, never a score.
fn agreement_phrase(app: &App, check: &crate::ledger::Check) -> String {
    match crate::record::Record::read(&app.root, &check.eval_id) {
        Some(r) => r.agreed(check.agreement),
        None => format!("agreement {:.2}", check.agreement),
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).chain(['…']).collect()
    }
}

fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    app.note_pane_area(area);
    let focus = app.pane_focus;
    if app.panes.is_empty() {
        // Weft starts no agent on its own. The panel stays empty until asked.
        frame.render_widget(Paragraph::new(""), area);
        return;
    }
    let Some(pane) = app.panes.get_mut(focus) else {
        frame.render_widget(Paragraph::new(""), area);
        return;
    };
    let _ = pane.fit(area.height, area.width);
    pane.pty.with_screen(|screen| {
        frame.render_widget(PseudoTerminal::new(screen).block(Block::default()), area);
    });
}

fn waiting_line(app: &App) -> Option<String> {
    app.waiting(app.pane_focus)
        .map(|_| "  ● this agent needs your answer (from the screen)".to_string())
}

fn action_bar(app: &App) -> Paragraph<'static> {
    // While the person is in the agent, Weft has no keys to offer.
    if app.focus == Focus::Agent {
        return Paragraph::new("");
    }
    let mut left = String::from("  [N]ew agent   [H]elp   [X] Quit");
    if !app.panes.is_empty() {
        left = String::from("  [A]sk   [N]ew agent   [C]heck   [D]ecide   [H]elp   [X] Quit");
        if app.selected_unit().is_some_and(|u| u.sent == Sent::ReadyToSend) {
            left = "  [A]sk   [S]end it   [C]heck   [D]ecide   [H]elp   [X] Quit".into();
        }
    }
    let right = if app.panes.is_empty() {
        String::new()
    } else {
        format!("{} to the agent ", app.toggle.label())
    };
    Paragraph::new(Line::from(vec![Span::raw(left), Span::raw("   "), Span::raw(right)]))
}

fn hint(app: &App) -> Paragraph<'static> {
    let text = match (&app.modal, app.focus) {
        (Some(Modal::Quit), _) => " Nothing you asked for is lost either way. It is all written down.".into(),
        (Some(Modal::Confirm(_)), _) => " Weft never types into an agent without asking you first.".into(),
        (Some(Modal::Ask { .. }), _) => " The agent will ask you which approach to take, in its own pane.".into(),
        (_, Focus::Agent) => format!(
            " Every key goes to the agent, Esc included. {} comes back to Weft.",
            app.toggle.label()
        ),
        (_, Focus::Weft) if app.panes.is_empty() => {
            " Press N to start an agent in this project.".to_string()
        }
        (_, Focus::Weft) => match waiting_line(app) {
            Some(_) => {
                " Weft never answers an agent's question for you. Go into the pane and choose."
                    .to_string()
            }
            None => " ↑↓ pick · Enter open · click anything".to_string(),
        },
    };
    Paragraph::new(text)
}

fn overlay(frame: &mut Frame, app: &App, modal: &Modal, area: Rect) {
    // Four rows go to the border, the blank line and the buttons.
    let room = area.height.saturating_sub(6) as usize;
    let (title, lines, buttons) = content(app, modal, room);
    let height = (lines.len() + buttons.len() + 4).min(area.height as usize) as u16;
    let width = 64u16.min(area.width.saturating_sub(4));
    let box_area = centred(area, width, height);
    frame.render_widget(Clear, box_area);

    let mut body: Vec<Line> = vec![Line::raw("")];
    for l in lines {
        body.push(Line::raw(format!("  {l}")));
    }
    if !buttons.is_empty() {
        body.push(Line::raw(""));
    }
    for (i, b) in buttons.iter().enumerate() {
        let style = if i == app.modal_choice {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        body.push(Line::styled(format!("   {b}"), style));
    }
    frame.render_widget(
        Paragraph::new(body).block(Block::bordered().title(title)),
        box_area,
    );
}

fn content(app: &App, modal: &Modal, room: usize) -> (String, Vec<String>, Vec<String>) {
    match modal {
        Modal::Quit => (
            " Quit Weft? ".into(),
            vec![
                "Your agents are running in the background. They can".into(),
                "keep going without Weft open.".into(),
            ],
            vec![
                "[ Quit, leave the agents running ]".into(),
                "[ Quit and stop the agents ]".into(),
                "[ Cancel ]".into(),
            ],
        ),
        Modal::Help => (
            " Keys ".into(),
            vec![
                "↑ ↓    pick an item".into(),
                "Enter  open it".into(),
                "A      ask for something".into(),
                "C      check the work · D  decide · X  quit".into(),
                format!("{}  switch between Weft and the agent", app.toggle.label()),
                "".into(),
                "In the agent every other key goes straight through.".into(),
                "To select text in a pane, hold Shift.".into(),
            ],
            vec!["[ Close ]".into()],
        ),
        Modal::Note(text) => (
            " Weft ".into(),
            text.lines().map(str::to_string).collect(),
            vec!["[ OK ]".into()],
        ),
        Modal::Ask { text, target } => {
            let who = app.panes.get(*target).map(|p| p.harness.clone()).unwrap_or_default();
            let mut lines = wrap(text, 58);
            if text.is_empty() {
                lines = vec!["_".into()];
            } else {
                lines.push("_".into());
            }
            lines.push("".into());
            lines.push(format!("which agent   ▸ {who}        Tab to change"));
            (" What do you want done? ".into(), lines, vec!["[ Send ]   [ Cancel ]".into()])
        }
        Modal::Confirm(p) => {
            let mut lines = p.why.clone();
            lines.push("".into());
            let text = String::from_utf8_lossy(&p.payload);
            for l in wrap(&text, 58).into_iter().take(8) {
                lines.push(l);
            }
            if text.lines().count() > 8 {
                lines.push("…".into());
            }
            (
                format!(" {} ", p.what),
                lines,
                vec!["[ Do it ]".into(), "[ Cancel ]".into()],
            )
        }
        Modal::StartAgent { .. } => {
            let found = App::available_agents();
            (
                " Start an agent ".into(),
                vec![
                    "It runs in this project, with your own settings.".into(),
                    String::new(),
                ],
                found.iter().map(|a| format!("[ {a} ]")).collect(),
            )
        }
        Modal::View { title, lines, offset } => {
            let shown: Vec<String> = lines.iter().skip(*offset).take(room).cloned().collect();
            let more = lines.len().saturating_sub(*offset + shown.len());
            let mut body = shown;
            if more > 0 {
                body.push(format!("… {more} more lines · ↓ or the wheel"));
            }
            (format!(" {title} "), body, vec!["[ Close ]".into()])
        }
        Modal::Detail { unit, record } => detail(app, *unit, record.as_ref(), room),
    }
}

fn detail(
    app: &App,
    index: usize,
    record: Option<&crate::record::Record>,
    room: usize,
) -> (String, Vec<String>, Vec<String>) {
    let Some(unit) = app.units.get(index) else {
        return (" Detail ".into(), vec!["gone".into()], vec!["[ Close ]".into()]);
    };

    let mut head = vec![
        format!("asked {} · {}", &unit.asked_at, unit.harness),
        format!("route {} · {}", unit.route, unit.status()),
    ];

    // The conclusion goes above the evidence. A verdict scrolled off the
    // bottom of the box is worse than one out of narrative order.
    if let (Some(c), Some(r)) = (&unit.check, record) {
        head.push(String::new());
        head.push(format!("{} · {}", c.verdict.plain().to_uppercase(), r.agreed(c.agreement)));
        head.push("A check is a judgement, not a guarantee.".into());
        head.push(format!(
            "Recorded as: {}, {:.2}.",
            c.verdict.recorded(),
            c.agreement
        ));
    }

    let Some(r) = record else {
        head.push(String::new());
        head.push("No check has been run on this yet.".into());
        head.push("Press C to have the agent's judges look at it.".into());
        return (format!(" {} ", clip(&unit.title, 40)), head, vec!["[ Close ]".into()]);
    };

    let mut tail = vec![
        String::new(),
        format!(
            "{} judges checked the work · none of them did it",
            r.judges.len()
        ),
        format!("judged by {}", r.judged_by().join(" and ")),
        String::new(),
    ];

    let mut shown = 0usize;
    for item in &r.items {
        // Three lines per item at most, and only while there is room left.
        if head.len() + tail.len() + 4 > room {
            break;
        }
        let mark = match item.plain_majority() {
            "yes" => "✓",
            "no" => "✗",
            _ => "?",
        };
        tail.push(format!("{mark} {}", clip(&item.text, 44)));
        tail.push(format!("     {}   {}", item.plain_majority(), r.agreed(item.agreement)));
        if let Some(reason) = item.reasons.first() {
            tail.push(format!("     {}", clip(reason, 50)));
        }
        shown += 1;
    }
    if shown < r.items.len() {
        tail.push(format!("… and {} more", r.items.len() - shown));
    }
    if !r.unexplained.is_empty() && head.len() + tail.len() + 2 <= room {
        tail.push(String::new());
        tail.push(format!(
            "{} changed and no judge could tie it to what you asked",
            r.unexplained.join(", ")
        ));
    }

    head.extend(tail);
    (
        format!(" {} ", clip(&unit.title, 40)),
        head,
        vec!["[P] The wording   [J] The judges   [S] Decide   [ Close ]".into()],
    )
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

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height: height.min(area.height),
    }
}
