//! Colour.
//!
//! Weft styles its own chrome and nothing else. Inside a pane every cell
//! belongs to the harness, painted on the person's own terminal background, so
//! Weft never sets a background of its own — a near-black frame around a pane
//! drawn on a light terminal reads as broken rather than as designed.

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// 24-bit colour: the exact values.
    True,
    /// The terminal's own sixteen, so Weft matches the theme already chosen.
    Ansi,
}

impl Depth {
    pub fn detect() -> Self {
        let truecolor = std::env::var("COLORTERM")
            .map(|v| {
                let v = v.to_ascii_lowercase();
                v.contains("truecolor") || v.contains("24bit")
            })
            .unwrap_or(false);
        if truecolor { Depth::True } else { Depth::Ansi }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub depth: Depth,
}

impl Theme {
    pub fn new() -> Self {
        Self { depth: Depth::detect() }
    }

    /// What is live, chosen, or waiting on you.
    pub fn accent(self) -> Color {
        match self.depth {
            Depth::True => Color::Rgb(0x10, 0xb9, 0x81),
            Depth::Ansi => Color::Green,
        }
    }

    /// The brighter accent, for the one thing that must be seen.
    pub fn accent_bright(self) -> Color {
        match self.depth {
            Depth::True => Color::Rgb(0x34, 0xd3, 0x99),
            Depth::Ansi => Color::LightGreen,
        }
    }

    /// The default voice: labels, statuses, hints.
    pub fn muted(self) -> Color {
        match self.depth {
            Depth::True => Color::Rgb(0x8f, 0x92, 0x9b),
            Depth::Ansi => Color::DarkGray,
        }
    }

    /// Rules and borders. Present, not loud.
    pub fn line(self) -> Color {
        match self.depth {
            Depth::True => Color::Rgb(0x2e, 0x32, 0x3b),
            Depth::Ansi => Color::DarkGray,
        }
    }

    /// Reserved for what matters: titles, the selected row.
    pub fn primary(self) -> Color {
        // The terminal's own foreground, so Weft reads correctly on any theme.
        Color::Reset
    }

    pub fn label(self) -> Style {
        Style::default().fg(self.muted())
    }

    pub fn value(self) -> Style {
        Style::default().fg(self.accent())
    }

    pub fn rule(self) -> Style {
        Style::default().fg(self.line())
    }

    pub fn title(self) -> Style {
        Style::default().fg(self.primary()).add_modifier(Modifier::BOLD)
    }

    pub fn selected(self) -> Style {
        Style::default().fg(self.primary()).add_modifier(Modifier::BOLD)
    }

    pub fn needs_you(self) -> Style {
        Style::default().fg(self.accent_bright()).add_modifier(Modifier::BOLD)
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new()
    }
}

/// A label and its value, the one type pairing the whole look rests on.
pub fn pair(label: &str, value: &str) -> String {
    format!("{}  {}", label.to_uppercase(), value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truecolor_terminals_get_the_exact_values() {
        let t = Theme { depth: Depth::True };
        assert_eq!(t.accent(), Color::Rgb(0x10, 0xb9, 0x81));
        assert_eq!(t.muted(), Color::Rgb(0x8f, 0x92, 0x9b));
    }

    #[test]
    fn everything_else_falls_back_to_the_terminals_own_sixteen() {
        let t = Theme { depth: Depth::Ansi };
        assert_eq!(t.accent(), Color::Green);
        assert_eq!(t.muted(), Color::DarkGray);
    }

    #[test]
    fn weft_never_paints_a_background() {
        // Every style Weft applies sets a foreground and leaves the canvas to
        // the terminal and to the harness.
        let t = Theme { depth: Depth::True };
        for style in [t.label(), t.value(), t.rule(), t.title(), t.selected(), t.needs_you()] {
            assert_eq!(style.bg, None, "chrome must not paint a background");
        }
    }

    #[test]
    fn the_primary_voice_is_the_terminals_own_foreground() {
        assert_eq!(Theme { depth: Depth::True }.primary(), Color::Reset);
    }

    #[test]
    fn a_label_and_its_value_read_as_one_pairing() {
        assert_eq!(pair("open", "1"), "OPEN  1");
        assert_eq!(pair("needs you", "2"), "NEEDS YOU  2");
    }
}
