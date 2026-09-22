//! Responsive layout.
//!
//! The pane never drops below 80 columns, which is what the harnesses
//! assume. Below the threshold there is one surface at a time — the sidebar
//! *or* the agent.

/// The narrowest the work list is ever drawn.
pub const LIST_MIN: u16 = 30;
/// The widest it grows to; past this the extra columns go to the pane.
pub const LIST_MAX: u16 = 44;
pub const MIN_PANE_WIDTH: u16 = 80;
/// Below this, one surface at a time.
pub const SPLIT_THRESHOLD: u16 = 112;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One thing at a time: the list full width, or the agent full width.
    Single { width: u16 },
    /// The work list beside the agent, with a divider column between them.
    Split { list: u16, pane: u16 },
}

pub fn for_width(width: u16) -> Layout {
    if width < SPLIT_THRESHOLD {
        return Layout::Single { width };
    }
    let list = (width - 1 - MIN_PANE_WIDTH).clamp(LIST_MIN, LIST_MAX);
    Layout::Split { list, pane: width - 1 - list }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighty_columns_gets_one_surface_at_a_time() {
        assert_eq!(for_width(80), Layout::Single { width: 80 });
    }

    #[test]
    fn the_split_starts_at_a_hundred_and_twelve() {
        assert_eq!(for_width(SPLIT_THRESHOLD - 1), Layout::Single { width: SPLIT_THRESHOLD - 1 });
        assert!(matches!(for_width(SPLIT_THRESHOLD), Layout::Split { .. }));
    }

    #[test]
    fn a_comfortable_laptop_draws_the_screen_the_spec_drew() {
        // Screen 4 of the spec, at 120 columns: list 39, pane 80.
        assert_eq!(for_width(120), Layout::Split { list: 39, pane: 80 });
    }

    #[test]
    fn the_list_grows_to_forty_four_and_then_stops() {
        let Layout::Split { list, .. } = for_width(130) else { panic!() };
        assert_eq!(list, LIST_MAX);
        let Layout::Split { list, pane } = for_width(200) else { panic!() };
        assert_eq!(list, LIST_MAX);
        assert_eq!(pane, 200 - 1 - LIST_MAX, "the extra columns go to the agent");
    }

    #[test]
    fn a_pane_is_never_narrower_than_eighty_when_the_list_is_beside_it() {
        for width in 1u16..400 {
            if let Layout::Split { pane, .. } = for_width(width) {
                assert!(pane >= MIN_PANE_WIDTH, "width {width} gave pane {pane}");
            }
        }
    }

    #[test]
    fn the_list_stays_within_the_width_the_spec_allows_it() {
        for width in SPLIT_THRESHOLD..400 {
            let Layout::Split { list, .. } = for_width(width) else { panic!("{width}") };
            assert!((LIST_MIN..=LIST_MAX).contains(&list), "width {width} gave list {list}");
        }
    }

    #[test]
    fn the_split_always_accounts_for_every_column() {
        for width in SPLIT_THRESHOLD..400 {
            let Layout::Split { list, pane } = for_width(width) else { panic!("{width}") };
            assert_eq!(list + 1 + pane, width);
        }
    }
}
