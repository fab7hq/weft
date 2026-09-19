//! Responsive layout.
//!
//! Spec: `plans/weft/spec/interface.md`. The rail must never squeeze a pane
//! below the 80 columns harness TUIs assume.

pub const RAIL_WIDTH: u16 = 18;
pub const MIN_PANE_WIDTH: u16 = 80;
pub const RAIL_THRESHOLD: u16 = RAIL_WIDTH + 1 + MIN_PANE_WIDTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One thing at a time: the list full width, or the agent full width.
    Single { width: u16 },
    /// List rail beside the agent.
    Split { rail: u16, pane: u16 },
}

pub fn for_width(width: u16) -> Layout {
    if width >= RAIL_THRESHOLD {
        Layout::Split { rail: RAIL_WIDTH, pane: width - RAIL_WIDTH - 1 }
    } else {
        Layout::Single { width }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighty_columns_gets_no_rail() {
        assert_eq!(for_width(80), Layout::Single { width: 80 });
    }

    #[test]
    fn the_rail_appears_only_when_the_pane_still_gets_eighty() {
        assert_eq!(for_width(RAIL_THRESHOLD - 1), Layout::Single { width: RAIL_THRESHOLD - 1 });
        assert_eq!(for_width(RAIL_THRESHOLD), Layout::Split { rail: 18, pane: 80 });
    }

    #[test]
    fn a_pane_is_never_narrower_than_eighty_when_the_rail_is_shown() {
        for width in 1u16..300 {
            if let Layout::Split { pane, .. } = for_width(width) {
                assert!(pane >= MIN_PANE_WIDTH, "width {width} gave pane {pane}");
            }
        }
    }

    #[test]
    fn the_split_always_accounts_for_every_column() {
        for width in RAIL_THRESHOLD..300 {
            let Layout::Split { rail, pane } = for_width(width) else { panic!("{width}") };
            assert_eq!(rail + 1 + pane, width);
        }
    }
}
