//! `~/.fab7/weft/layout.toml`: how the TUI last laid out its panes, each
//! agent named by its session id. The TUI writes it when the person changes
//! the layout and reads it when it starts; it is the viewer's, not the
//! daemon's.

use serde::{Deserialize, Serialize};

/// A layout as kept: a pane and its tabs in order, or panes one way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Saved {
    /// One pane's tabs, by session id, in the person's order.
    Pane(Vec<String>),
    /// Panes side by side, left first.
    Right(Vec<Saved>),
    /// Panes one below another, top first.
    Down(Vec<Saved>),
}

#[derive(Serialize, Deserialize)]
struct File {
    layout: Saved,
}

/// The file's text.
pub fn write(layout: &Saved) -> String {
    toml::to_string(&File { layout: layout.clone() }).unwrap_or_default()
}

/// The layout a file holds; none when it is not one.
pub fn read(text: &str) -> Option<Saved> {
    toml::from_str::<File>(text).ok().map(|f| f.layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_reads_back_as_it_was_written() {
        let one = |ids: &[&str]| Saved::Pane(ids.iter().map(|s| s.to_string()).collect());
        let layout =
            Saved::Right(vec![Saved::Down(vec![one(&["s1", "s2"]), one(&["s3"])]), one(&["s4"])]);
        assert_eq!(read(&write(&layout)), Some(layout));
        assert_eq!(read("not = [a layout"), None);
    }
}
