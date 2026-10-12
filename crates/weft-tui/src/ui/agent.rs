//! The agent's pane, the detail view that takes its place, and no agent.

use super::*;

/// The agent's pane, in a frame that is its state: grey while Weft has the
/// keys, blue and heavy while the agent has them. Beside the list it starts
/// at the divider.
pub(super) fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    if let Some(split) = app.split_now() {
        return two_agents(frame, app, area, split);
    }
    if app.pane_count() == 0 {
        frame.render_widget(no_agent(app, area), area);
        return;
    }
    let th = app.theme;
    let block = if app.focus == Focus::Agent {
        Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::default().fg(th.warp()))
    } else {
        Block::bordered().border_style(th.rule())
    };
    let inside = block.inner(area);
    app.hits.push((inside, crate::app::Target::Screen(app.pane_focus)));
    frame.render_widget(block, area);
    let focus = app.pane_focus;
    app.with_pane_screen(focus, inside.height, inside.width, |screen| {
        frame.render_widget(PseudoTerminal::new(screen), inside);
    });
}

/// A Details view, blocking (T11.2–T11.6). Lines fold rather than being
/// cut: this is where the exact wording is read, and a prompt missing its
/// right-hand end is not the exact wording. A line's part after a tab is
/// drawn against the right edge; the picked link is drawn picked.
pub(super) fn detail(app: &mut App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let room = area.height as usize;
    let width = (area.width.saturating_sub(1) as usize).max(1);
    // Folded before it is scrolled, so one press of `↓` moves one drawn line
    // rather than a whole paragraph of them. Each keeps the line it came from.
    let (folded, offset, picked) = {
        let Some(d) = app.detail() else { return Paragraph::new("") };
        let mut folded: Vec<(usize, String)> = Vec::new();
        for (i, l) in d.lines.iter().enumerate() {
            match l.split_once('\t') {
                Some((left, right)) => {
                    let gap = width.saturating_sub(cells(left) + cells(right) + 1).max(1);
                    folded.push((i, format!("{left}{}{right}", " ".repeat(gap))));
                }
                None => folded.extend(fold(l, width).into_iter().map(|f| (i, f))),
            }
        }
        (folded, d.offset, d.picked_line())
    };
    app.note_detail_reach(folded.len().saturating_sub(room), room);
    // The picked line is kept in view.
    let offset = match picked.and_then(|p| folded.iter().position(|(i, _)| *i == p)) {
        Some(at) if at < offset => at,
        Some(at) if at >= offset + room => at + 1 - room,
        _ => offset,
    };
    app.set_detail_offset(offset);

    let mut lines: Vec<Line> = folded
        .iter()
        .skip(offset)
        .take(room)
        .map(|(i, l)| {
            // A section heading, flush left, is the one thing with weight.
            let style = if Some(*i) == picked {
                th.selected()
            } else if !l.starts_with(' ') {
                th.title()
            } else {
                th.label()
            };
            let text = format!(" {l}");
            if Some(*i) == picked {
                return Line::styled(padded(&text, width + 1), style);
            }
            Line::styled(text, style)
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
pub(super) const LOGO: [(&str, &str); 7] = [
    ("  ┃   ╹   ┃", "  b   b   b"),
    ("━╸┃╺━━━━━╸┃╺━    ╻   ╻  ┏━━╸  ┏━━╸  ╺┳╸", "mmbmmmmmmmbmm    x   x  xxxx  xxxx  xxx"),
    ("  ╹   ╻   ╹      ┃   ┃  ┃     ┃      ┃", "  b   b   b      x   x  x     x      x"),
    ("━━━━━╸┃╺━━━━━•   ┃ ╻ ┃  ┣━━   ┣━━•   ┃", "ttttttbttttttt   x x x  xtt   xttt   x"),
    ("  ╻   ╹   ╻      ┃ ┃ ┃  ┃     ┃      ┃", "  b   b   b      x x x  x     x      x"),
    ("━╸┃╺━━━━━╸┃╺━    ┗━┻━┛  ┗━━╸  ╹      ╹", "mmbmmmmmmmbmm    xxxxx  xxxx  x      x"),
    ("  ┃   ╻   ┃", "  b   b   b"),
];

pub(super) fn logo(th: Theme, indent: &str) -> Vec<Line<'static>> {
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

pub(super) fn logo_span(th: Theme, kind: char, text: String) -> Span<'static> {
    match kind {
        'b' => Span::styled(text, Style::default().fg(th.warp())),
        'm' => Span::styled(text, th.label()),
        't' => Span::styled(text, Style::default().fg(th.accent())),
        'x' => Span::styled(text, th.title()),
        _ => Span::raw(text),
    }
}

/// No agent is running: the mark, and the two ways in.
pub(super) fn no_agent(app: &App, area: Rect) -> Paragraph<'static> {
    let th = app.theme;
    let mut lines = vec![Line::raw("")];
    if LOGO.len() + 5 <= area.height as usize {
        lines.extend(logo(th, "   "));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled("   [N] starts an agent.".to_string(), th.label()));
    Paragraph::new(lines)
}

/// The two halves of the agents' area, left and right, with a column
/// between them.
pub(super) fn halves(area: Rect) -> (Rect, Rect) {
    let left = area.width.saturating_sub(1) / 2;
    let l = Rect { width: left, ..area };
    let r = Rect { x: area.x + left + 1, width: area.width.saturating_sub(left + 1), ..area };
    (l, r)
}

/// Two agents side by side, each in its own frame at its half's size; the
/// side with the keys is framed as the one pane is.
fn two_agents(frame: &mut Frame, app: &mut App, area: Rect, split: crate::app::Split) {
    let th = app.theme;
    let (l, r) = halves(area);
    let rule = Rect { x: l.x + l.width, width: 1, ..area };
    frame.render_widget(Paragraph::new(vlines(rule.height, th)), rule);
    for (side, half) in [l, r].into_iter().enumerate() {
        let (at, id) = split.sides[side];
        let Some(pane) = app.running_pane(at, id) else { continue };
        let keys = side == split.side && app.focus == Focus::Agent;
        let block = if keys {
            Block::bordered()
                .border_type(BorderType::Thick)
                .border_style(Style::default().fg(th.warp()))
        } else if side == split.side {
            Block::bordered().border_style(Style::default().fg(th.warp()))
        } else {
            Block::bordered().border_style(th.rule())
        };
        let inside = block.inner(half);
        if side == split.side {
            app.hits.push((inside, crate::app::Target::Screen(app.pane_focus)));
        }
        frame.render_widget(block, half);
        app.with_pane_screen_in(at, pane, inside.height, inside.width, |screen| {
            frame.render_widget(PseudoTerminal::new(screen), inside);
        });
    }
}
