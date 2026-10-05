//! The work list: one row per project, harness and unit of work.

use super::*;

/// One row per unit of work: what you asked for, and where it stands. The
/// selected row expands in place.
pub(super) fn work_list(app: &mut App, area: Rect, beside_agent: bool) -> Paragraph<'static> {
    let th = app.theme;
    let width = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    // What the arrows have to keep the selection inside, now that there is no
    // wheel. One line goes to the "N above" marker when the list is scrolled.
    app.note_list_rows(area.height.saturating_sub(1) as usize);

    // An agent is on the list the moment it is opened, so the empty list is
    // only for a project with no agent and nothing asked.
    if app.units().is_empty() && app.pane_count() == 0 {
        lines.push(Line::raw(""));
        if let Some((name, state)) = app.not_ready() {
            lines.push(Line::styled(
                format!(" {}", state.say(&name).unwrap_or_default()),
                th.label(),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::styled(format!(" {}", word("tree.runs_either_way")), th.label()));
            return Paragraph::new(lines);
        }
        let (said, next) = if app.record_available() && app.pane_count() == 0 {
            // When the agent area is beside the list, it says how to start one.
            (
                word("tree.nothing_asked"),
                if beside_agent { "" } else { "[N] starts an agent to ask." },
            )
        } else if app.record_available() {
            (word("tree.nothing_asked"), "[A]SK for something and Weft writes it down.")
        } else {
            // RingFrame owns the record. Without it there is nothing to read,
            // and saying so is better than an empty list that looks settled.
            (word("tree.no_ringframe"), word("tree.install_ringframe"))
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
        let at = Rect { y: area.y + lines.len() as u16, height: 1, ..area };
        app.hits.push((at, Target::Row(i)));
        lines.push(sidebar_row(app, row, i == app.selected, width));
    }
    // An agent open and nothing asked yet: say what comes next under it.
    if app.units().is_empty() {
        let next: Vec<String> = match app.not_ready() {
            Some((name, state)) => {
                vec![state.say(&name).unwrap_or_default(), word("tree.written_down").into()]
            }
            None if app.record_available() => {
                vec!["[A]SK for something and Weft writes it down.".into()]
            }
            None => vec![word("tree.no_ringframe").into()],
        };
        if lines.len() + 1 + next.len() <= room {
            lines.push(Line::raw(""));
            lines.extend(next.into_iter().map(|n| Line::styled(format!(" {n}"), th.label())));
        }
    }
    Paragraph::new(lines)
}

/// One row of the sidebar. `▾` and `▸` mark the levels that hold something;
/// an action is a leaf and carries no marker. `⌫` is the one gesture that
/// removes anything, so it is the one thing with a glyph of its own.
pub(super) fn sidebar_row(
    app: &App,
    row: &crate::app::Row,
    picked: bool,
    width: usize,
) -> Line<'static> {
    use crate::app::Row;
    let th = app.theme;
    let pick = |s: String| {
        if picked { Span::styled(s, th.selected()) } else { Span::styled(s, th.label()) }
    };
    match row {
        Row::Project { name, folded, open, waiting, .. } => {
            let mut tail = format!("{open} {}", word("title.open"));
            if *waiting > 0 {
                tail.push_str(&format!(" · {waiting} ●"));
            }
            let head = format!(" {} {name}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(cells(&tail) + 4);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(format!("{tail}  "), th.label()),
                Span::styled("⌫".to_string(), th.label()),
            ])
        }
        Row::Harness { name, folded, waiting, running, turbo, .. } => {
            // The badge of an agent asking for input: the one thing NEEDS YOU is.
            let tail = match waiting {
                0 => "   ".to_string(),
                1 => "● needs your input  ".to_string(),
                n => format!("● {n} need your input  "),
            };
            let gone =
                if *running { String::new() } else { format!(" · {}", word("tree.not_running")) };
            // ⚡: an agent of it runs in turbo mode, as its tab says too.
            let turbo = if *turbo { " ⚡" } else { "" };
            let head = format!("   {} {name}{turbo}{gone}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(cells(&tail) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(tail, th.needs_you()),
            ])
        }
        Row::Action { project, unit, word, turn } => {
            // The row's own project, not the focused one: a unit in the
            // background was being drawn from whichever board was in front.
            let Some(unit) = app.units_of(*project).get(*unit) else { return Line::raw("") };
            // Where the Ask stands, and never an alarm: what needs you is an
            // agent asking, and that is its harness's row. The agent's state
            // shows when its session reported one; nothing rather than a guess.
            let agent = turn.map(|t| format!("{} · ", t.say()));
            let state = format!("{}{}  ", agent.unwrap_or_default(), word.say());
            let head = format!("      {}", unit.title);
            let room = width.saturating_sub(cells(&state) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(state, th.label()),
            ])
        }
    }
}
