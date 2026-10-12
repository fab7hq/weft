//! The agent's pane, the detail view that takes its place, and no agent.

use super::*;

/// The agent's pane, in a frame that is its state: grey while Weft has the
/// keys, blue and heavy while the agent has them. Beside the list it starts
/// at the divider.
pub(super) fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.pane_count() == 0 {
        frame.render_widget(no_agent(app, area), area);
        return;
    }
    if app.panes().count() > 1 {
        return panes(frame, app, area);
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
    let id = app.panes().active().id;
    app.hits.push((area, Target::Pane(id)));
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

/// Every pane at its place, each with its own tab bar above its frame and
/// its agent drawn at the frame's size. A window too small for the layout
/// shows the pane with the keys alone; the layout is kept.
fn panes(frame: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let active = app.panes().active().id;
    let mut rects = app.panes().rects(area);
    if !app.panes().fits(area) {
        rects = vec![(active, area)];
    }
    let numbers = app.tabs();
    for (id, r) in rects {
        let Some(pane) = app.panes().pane(id).cloned() else { continue };
        let keys = id == active;
        let bar = Rect { height: 1, ..r };
        let mut spans = vec![Span::raw(" ")];
        let mut col = bar.x + 1;
        for (i, &(at, aid)) in pane.tabs.iter().enumerate() {
            let Some(idx) = app.running_pane(at, aid) else { continue };
            let Some(n) = numbers.iter().position(|t| (t.project, t.pane) == (at, idx)) else {
                continue;
            };
            let tab = &numbers[n];
            if i > 0 {
                spans.push(Span::raw("    "));
                col += 4;
            }
            let shown = i == pane.shown;
            let text = format!(
                "{}{} {} · {}{}",
                if shown { "▸ " } else { "" },
                n + 1,
                clip(&tab.project_name, PROJECT_IN_TAB),
                tab.harness,
                if tab.turbo { " ⚡" } else { "" },
            );
            let held = app.drag.is_some_and(|d| d.moved && d.agent == (at, aid));
            let style = match (shown, keys && app.focus == Focus::Agent) {
                _ if held => th.rule(),
                (true, true) => Style::default().fg(th.accent()).add_modifier(Modifier::BOLD),
                (true, false) => Style::default().fg(th.primary()).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(th.primary()),
            };
            let from = col;
            col += cells(&text) as u16;
            spans.push(Span::styled(text, style));
            app.hits.push((Rect { x: from, width: col - from, ..bar }, Target::Tab(at, idx)));
            for (mark, on, style) in
                [(" ●", tab.waiting, th.needs_you()), (" ⚠", !tab.ready, th.needs_you())]
            {
                if on {
                    spans.push(Span::styled(mark, style));
                    col += cells(mark) as u16;
                }
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), bar);
        let body = Rect { y: r.y + 1, height: r.height.saturating_sub(1), ..r };
        let block = match (keys, app.focus == Focus::Agent) {
            (true, true) => Block::bordered()
                .border_type(BorderType::Thick)
                .border_style(Style::default().fg(th.warp())),
            (true, false) => Block::bordered().border_style(Style::default().fg(th.warp())),
            _ => Block::bordered().border_style(th.rule()),
        };
        let inside = block.inner(body);
        frame.render_widget(block, body);
        let Some((at, aid)) = pane.showing() else { continue };
        let Some(idx) = app.running_pane(at, aid) else { continue };
        if keys {
            app.hits.push((inside, Target::Screen(idx)));
        }
        app.hits.push((bar, Target::Bar(id)));
        app.hits.push((body, Target::Pane(id)));
        app.with_pane_screen_in(at, idx, inside.height, inside.width, |screen| {
            frame.render_widget(PseudoTerminal::new(screen), inside);
        });
    }
}

/// A tab held by the mouse: where it would land is shaded blue (a tab bar, a
/// pane's middle, or the half a split would give it, behind a dashed line),
/// and the tab follows the pointer.
pub(super) fn drag_overlay(frame: &mut Frame, app: &App, drag: crate::app::Drag) {
    use crate::panes::{Drop, Side};
    let th = app.theme;
    let zone = Style::default().bg(th.drop_zone());
    let line = Style::default().fg(th.warp());
    let hit = |want: Target| app.hits.iter().find(|(_, t)| *t == want).map(|(r, _)| *r);
    let buf = frame.buffer_mut();
    match app.drop_at(drag.at) {
        Some(Drop::Bar { pane, .. }) => {
            if let Some(r) = hit(Target::Bar(pane)) {
                buf.set_style(r, zone);
            }
        }
        Some(Drop::Middle { pane }) => {
            if let Some(r) = hit(Target::Pane(pane)) {
                buf.set_style(r.inner(ratatui::layout::Margin::new(1, 1)), zone);
            }
        }
        Some(Drop::Edge { pane, side }) => {
            let Some(r) = hit(Target::Pane(pane)) else { return };
            let inner = r.inner(ratatui::layout::Margin::new(1, 1));
            if matches!(side, Side::Up | Side::Down) {
                let mid = inner.y + inner.height / 2;
                let shade = if side == Side::Down {
                    Rect { y: mid + 1, height: inner.bottom().saturating_sub(mid + 1), ..inner }
                } else {
                    Rect { height: mid - inner.y, ..inner }
                };
                buf.set_style(shade, zone);
                let dashed = format!("├{}┤", "╌".repeat(r.width.saturating_sub(2) as usize));
                buf.set_string(r.x, mid, dashed, line);
            } else {
                let mid = inner.x + inner.width / 2;
                let shade = if side == Side::Right {
                    Rect { x: mid + 1, width: inner.right().saturating_sub(mid + 1), ..inner }
                } else {
                    Rect { width: mid - inner.x, ..inner }
                };
                buf.set_style(shade, zone);
                buf.set_string(mid, r.y, "┬", line);
                for y in inner.y..inner.bottom() {
                    buf.set_string(mid, y, "╎", line);
                }
                buf.set_string(mid, r.bottom().saturating_sub(1), "┴", line);
            }
        }
        None => {}
    }
    let tabs = app.tabs();
    let (at, id) = drag.agent;
    let Some(pane) = app.running_pane(at, id) else { return };
    let Some(n) = tabs.iter().position(|t| (t.project, t.pane) == (at, pane)) else { return };
    let label = format!(" {} {} · {} ", n + 1, tabs[n].project_name, tabs[n].harness);
    let area = buf.area;
    let x = drag.at.0.min(area.right().saturating_sub(cells(&label) as u16));
    let held = Style::default().fg(th.primary()).bg(th.warp()).add_modifier(Modifier::BOLD);
    buf.set_string(x, drag.at.1, label, held);
}
