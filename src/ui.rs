//! Drawing. Spec: `plans/weft/spec/interface.md`.

use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;
use tui_term::widget::PseudoTerminal;

use crate::app::{App, Modal};
use crate::keys::Focus;
use crate::layout::{self, Layout};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(1), // rule
            Constraint::Min(1),    // body
            Constraint::Length(1), // rule
            Constraint::Length(1), // actions
            Constraint::Length(1), // hint
        ])
        .split(area);

    frame.render_widget(title_bar(app), rows[0]);
    frame.render_widget(rule(area.width), rows[1]);
    body(frame, app, rows[2]);
    frame.render_widget(rule(area.width), rows[3]);
    frame.render_widget(action_bar(app), rows[4]);
    frame.render_widget(hint(app), rows[5]);

    if let Some(modal) = app.modal {
        overlay(frame, app, modal, area);
    }
}

fn title_bar(app: &App) -> Paragraph<'static> {
    let where_you_are = match app.focus {
        Focus::Weft => "in Weft",
        Focus::Agent => "in the agent",
    };
    let left = format!(" weft · {} · {}", app.project, where_you_are);
    let right = match app.focus {
        Focus::Weft => format!("{} panes ", app.panes.len()),
        Focus::Agent => format!("{} to come back ", app.toggle.label()),
    };
    Paragraph::new(Line::from(vec![
        Span::styled(left, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::raw(right),
    ]))
}

fn rule(width: u16) -> Paragraph<'static> {
    Paragraph::new("─".repeat(width as usize))
}

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    match layout::for_width(area.width) {
        Layout::Split { rail, pane } => {
            let cols = RLayout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(rail), Constraint::Length(1), Constraint::Length(pane)])
                .split(area);
            frame.render_widget(list(app), cols[0]);
            frame.render_widget(Paragraph::new(vlines(area.height)), cols[1]);
            agent(frame, app, cols[2]);
        }
        Layout::Single { .. } => match app.focus {
            Focus::Weft => frame.render_widget(list(app), area),
            Focus::Agent => agent(frame, app, area),
        },
    }
}

fn vlines(height: u16) -> String {
    std::iter::repeat_n("│", height as usize).collect::<Vec<_>>().join("\n")
}

fn list(app: &App) -> Paragraph<'static> {
    let mut lines: Vec<Line> = vec![Line::raw("")];
    for (i, pane) in app.panes.iter().enumerate() {
        let picked = i == app.selected && app.focus == Focus::Weft;
        let marker = if picked { " ▸ " } else { "   " };
        let style = if picked {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("{marker}{}", pane.title), style));
        lines.push(Line::raw(format!("     {}", pane.status)));
        lines.push(Line::raw(""));
    }
    if app.panes.is_empty() {
        lines.push(Line::raw("   Nothing is running yet."));
    }
    Paragraph::new(lines)
}

fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    app.note_pane_area(area);
    let Some(pane) = app.panes.get_mut(app.selected) else {
        frame.render_widget(Paragraph::new("   No agent running."), area);
        return;
    };
    let _ = pane.fit(area.height, area.width);
    pane.pty.with_screen(|screen| {
        frame.render_widget(PseudoTerminal::new(screen).block(Block::default()), area);
    });
}

fn action_bar(app: &App) -> Paragraph<'static> {
    // While the person is in the agent, Weft has no keys to offer.
    if app.focus == Focus::Agent {
        return Paragraph::new("");
    }
    let right = format!("{} to the agent ", app.toggle.label());
    Paragraph::new(Line::from(vec![
        Span::raw("  [A]sk   [C]heck   [D]ecide   [H]elp   [X] Quit"),
        Span::raw("   "),
        Span::raw(right),
    ]))
}

fn hint(app: &App) -> Paragraph<'static> {
    let text = match (app.focus, app.modal) {
        (_, Some(Modal::Quit)) => " Nothing you asked for is lost either way. It is all written down.".into(),
        (Focus::Agent, _) => format!(
            " Every key goes to the agent, Esc included. {} comes back to Weft.",
            app.toggle.label()
        ),
        (Focus::Weft, _) => " ↑↓ pick · Enter open · click anything".to_string(),
    };
    Paragraph::new(text)
}

fn overlay(frame: &mut Frame, app: &App, modal: Modal, area: Rect) {
    let (title, lines, buttons) = match modal {
        Modal::Quit => (
            " Quit Weft? ",
            vec![
                "Your agents are running in the background. They can".to_string(),
                "keep going without Weft open.".to_string(),
            ],
            vec!["[ Quit, leave the agents running ]", "[ Quit and stop the agents ]", "[ Cancel ]"],
        ),
        Modal::Help => (
            " Keys ",
            vec![
                "↑ ↓    pick an item".to_string(),
                "Enter  open it, or go into the agent".to_string(),
                "A      ask · C  check · D  decide · X  quit".to_string(),
                format!("{}  switch between Weft and the agent", app.toggle.label()),
                "In the agent every other key goes straight through.".to_string(),
            ],
            vec!["[ Close ]"],
        ),
    };

    let height = (lines.len() + buttons.len() + 4) as u16;
    let width = 62u16.min(area.width.saturating_sub(4));
    let box_area = centred(area, width, height);
    frame.render_widget(Clear, box_area);

    let mut body: Vec<Line> = vec![Line::raw("")];
    for l in lines {
        body.push(Line::raw(format!("  {l}")));
    }
    body.push(Line::raw(""));
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

fn centred(area: Rect, width: u16, height: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect { x, y, width, height: height.min(area.height) }
}
