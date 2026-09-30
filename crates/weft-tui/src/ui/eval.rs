//! The Eval view, blocking: RingFrame's results for one Eval, requirements
//! first. Every row is weft-core's (`eval_view::rows`); this draws them.

use super::*;
use crate::app::Layer;
use weft_core::eval_view::{Mode, RowKind, State};

fn mark_style(state: Option<&State>, th: Theme) -> Style {
    match state {
        Some(State::Met) => Style::default().fg(th.accent()),
        Some(State::NotMet) => th.needs_you(),
        Some(State::Unsettled) => th.value(),
        _ => th.label(),
    }
}

pub(super) fn eval_view(app: &mut App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let width = area.width.saturating_sub(1) as usize;
    // The header and the line under it are always there; the rest scrolls.
    let room = (area.height as usize).saturating_sub(2);
    app.note_eval_reach(room);
    app.keep_eval_cursor_in_view(room);
    let Some(e) = app.eval_screen() else { return Paragraph::new("") };

    let mut lines = vec![Line::from(vec![
        Span::styled(" EVAL ", th.title()),
        Span::styled(clip(&e.view.header(), width.saturating_sub(6)), th.value()),
    ])];
    if let Layer::Hunk { window, lines: hunk, offset } = e.top() {
        let path = e.view.change(window).map(|c| c.path.clone()).unwrap_or_default();
        lines.push(Line::styled(clip(&format!(" CHANGE {path}  {window}"), width), th.label()));
        for (text, lit) in hunk.iter().skip(*offset).take(room) {
            let style = if *lit { th.needs_you().add_modifier(Modifier::BOLD) } else { th.label() };
            lines.push(Line::styled(clip(&format!(" {text}"), width), style));
        }
        let more = hunk.len().saturating_sub(offset + room);
        if more > 0 {
            lines.pop();
            lines.push(Line::styled(format!(" … {more} more lines · [↓]"), th.label()));
        }
        return Paragraph::new(lines);
    }
    let under = match (e.top(), e.mode) {
        (Layer::Step { .. }, _) => " A STEP".to_string(),
        (_, Mode::Steps) => " BY STEP".to_string(),
        (_, Mode::Files) => " BY FILE".to_string(),
    };
    lines.push(Line::styled(under, th.label()));
    let rows = e.rows();
    let (cursor, offset) = (e.cursor(), e.offset());
    for (i, r) in rows.iter().enumerate().skip(offset).take(room) {
        let indent = "  ".repeat(r.depth as usize);
        let picked = i == cursor && r.pickable();
        let lead = format!(" {indent}{}{}", r.mark, if r.mark.is_empty() { "" } else { " " });
        let aside = if r.aside.is_empty() { String::new() } else { format!("  {}", r.aside) };
        // The text keeps what room it needs; the aside gives way first.
        let left = cells(&lead);
        let text_room = width.saturating_sub(left).saturating_sub(cells(&aside).min(width / 3));
        let text = clip(&r.text, text_room);
        let aside = clip(&aside, width.saturating_sub(left + cells(&text)));
        let gap = width.saturating_sub(left + cells(&text) + cells(&aside));
        let text_style = match (picked, r.kind) {
            (true, _) => th.selected(),
            (_, RowKind::Heading) => th.title(),
            (_, RowKind::Group) => th.value().add_modifier(Modifier::BOLD),
            (_, RowKind::Text) => th.label(),
            _ => th.value(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                lead,
                if picked { th.selected() } else { mark_style(r.state.as_ref(), th) },
            ),
            Span::styled(text, text_style),
            Span::styled(" ".repeat(gap), if picked { th.selected() } else { th.label() }),
            Span::styled(aside, if picked { th.selected() } else { th.label() }),
        ]));
    }
    let more = rows.len().saturating_sub(offset + room);
    if more > 0 && lines.len() > 2 {
        lines.pop();
        lines.push(Line::styled(format!(" … {more} more · [↓]"), th.label()));
    }
    Paragraph::new(lines)
}
