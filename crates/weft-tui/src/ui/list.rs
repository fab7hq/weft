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
        let at = Rect { y: area.y + lines.len() as u16, height: 1, ..area };
        app.hits.push((at, Target::Row(i)));
        lines.push(sidebar_row(app, row, i == app.selected, width));
    }
    // An agent open and nothing asked yet: say what comes next under it.
    if app.units().is_empty() {
        let next: Vec<String> = match app.not_ready() {
            Some((name, state)) => vec![
                state.say(&name).unwrap_or_default(),
                "Nothing is written down until it is set up.".into(),
            ],
            None if app.record_available() => {
                vec!["[A]SK for something and Weft writes it down.".into()]
            }
            None => vec!["No ringframe on PATH, so there is no record to read.".into()],
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
            let mut tail = if *open == 1 { "1 open".to_string() } else { format!("{open} open") };
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
        Row::Harness { project, name, folded, waiting, running } => {
            // The badge of an agent asking for input: the one thing NEEDS YOU is.
            let tail = match waiting {
                0 => "   ".to_string(),
                1 => "● needs your input  ".to_string(),
                n => format!("● {n} need your input  "),
            };
            let gone = if *running { "" } else { " · not running" };
            // ⚡: an agent of it runs in turbo mode, as its tab says too.
            let turbo = if app.harness_turbo(*project, name) { " ⚡" } else { "" };
            let head = format!("   {} {name}{turbo}{gone}", if *folded { "▸" } else { "▾" });
            let room = width.saturating_sub(cells(&tail) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(tail, th.needs_you()),
            ])
        }
        Row::Action { project, unit } => {
            // The row's own project, not the focused one: a unit in the
            // background was being drawn from whichever board was in front.
            let Some(unit) = app.units_of(*project).get(*unit) else { return Line::raw("") };
            // Where the Ask stands, and never an alarm: what needs you is an
            // agent asking, and that is its harness's row.
            let word = row_state(unit);
            // The agent's state, when its session reported one; nothing
            // rather than a guess when it did not.
            let agent = app.unit_turn(*project, unit).map(|t| format!("{} · ", t.plain()));
            let state = format!("{}{word}  ", agent.unwrap_or_default());
            let head = format!("      {}", unit.title);
            let room = width.saturating_sub(cells(&state) + 1);
            Line::from(vec![
                pick(padded(&clip(&head, room), room)),
                Span::styled(state, th.label()),
            ])
        }
    }
}

/// Where a unit stands, in one word: the furthest act that has happened, in
/// the past tense.
///
/// The same three acts the detail view names, so the sidebar and the view
/// teach one vocabulary between them rather than two.
pub(super) fn row_state(unit: &Unit) -> &'static str {
    if unit.cancelled {
        "CANCELLED"
    } else if unit.requested_only {
        // Asked, and its prompt still being composed: under way, not done.
        "ASKING"
    } else if unit.sealed.is_some() {
        "SEALED"
    } else if unit.check.is_some() {
        "EVALED"
    } else {
        "ASKED"
    }
}
