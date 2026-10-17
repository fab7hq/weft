//! What a screen that paints needs: only the rows that changed since it last
//! painted, each row on its own, so nothing copies the whole grid per frame.

use weft_term::Term;

fn quiet(rows: u16, cols: u16) -> Term {
    Term::new(rows, cols, 1000, |_| {})
}

#[test]
fn only_the_rows_a_program_wrote_are_changed() {
    let mut t = quiet(6, 20);
    assert_eq!(t.changed_rows(), None, "a new terminal paints every row");
    assert_eq!(t.changed_rows(), Some(vec![0]), "nothing since but the cursor's row");
    t.feed(b"\x1b[3;1Hthird");
    let changed = t.changed_rows().expect("some rows");
    assert!(changed.contains(&2), "{changed:?}");
    // Row 0 is where the cursor was: it is painted again without it.
    assert!([1, 3, 4, 5].iter().all(|r| !changed.contains(r)), "{changed:?}");
    assert_eq!(t.row(2)[0].text, "t");
    assert_eq!(t.row(2).len(), 20, "a cell per column");
}

#[test]
fn a_resize_a_scroll_and_a_selection_change_every_row() {
    let mut t = quiet(4, 10);
    for i in 0..10 {
        t.feed(format!("line {i}\r\n").as_bytes());
    }
    t.changed_rows();
    t.resize(5, 10);
    assert_eq!(t.changed_rows(), None, "resized");
    t.scroll(2);
    assert_eq!(t.changed_rows(), None, "scrolled back");
    t.scroll_to_bottom();
    t.changed_rows();
    t.select_from((0, 0));
    t.select_to((1, 3));
    assert_eq!(t.changed_rows(), None, "a selection");
    t.clear_selection();
    assert_eq!(t.changed_rows(), None, "and its end");
    assert_eq!(t.changed_rows().map(|r| r.len()), Some(1), "then the cursor's row alone");
}

#[test]
fn rows_are_the_cells_on_view() {
    let mut t = quiet(3, 8);
    t.feed(b"ab\r\ncd");
    let all = t.cells();
    for r in 0..3 {
        assert_eq!(t.row(r), all[r as usize]);
    }
}
