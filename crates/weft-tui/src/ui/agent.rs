//! The agent's pane, the detail view that takes its place, and no agent.

use super::*;

pub(super) fn agent(frame: &mut Frame, app: &mut App, area: Rect) {
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
pub(super) fn detail(app: &mut App, area: Rect) -> Paragraph<'static> {
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
