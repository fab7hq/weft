//! The cases a screen copy gets wrong first, each on its own, and the
//! answers a program gets when it asks about its terminal.

use std::sync::{Arc, Mutex};

use weft_term::Term;

fn quiet(rows: u16, cols: u16) -> Term {
    Term::new(rows, cols, 1000, |_| {})
}

/// A fresh terminal fed `formatted` shows what `pane` shows.
fn copied(pane: &Term) -> Term {
    let (rows, cols) = pane.size();
    let mut device = quiet(rows, cols);
    device.feed(&pane.formatted(0));
    assert_eq!(device.snapshot(), pane.snapshot());
    device
}

/// A terminal that keeps every answer it gives.
fn asking(rows: u16, cols: u16) -> (Term, Arc<Mutex<Vec<String>>>) {
    let said = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&said);
    let term = Term::new(rows, cols, 0, move |bytes| {
        keep.lock().unwrap().push(String::from_utf8_lossy(bytes).into_owned());
    });
    (term, said)
}

#[test]
fn the_alternate_screen_is_copied_and_a_switch_is_never_diffed() {
    let mut pane = quiet(5, 20);
    pane.feed(b"normal\r\n");
    let before = pane.snapshot();
    pane.feed(b"\x1b[?1049halt");
    assert!(pane.alt_screen());
    assert!(pane.diff(&before).is_none(), "entering the alternate screen");
    let device = copied(&pane);
    assert!(device.alt_screen());
    assert_eq!(device.contents(), "\nalt", "the cursor stays on the second row");

    let inside = pane.snapshot();
    pane.feed(b"\x1b[?1049l");
    assert!(pane.diff(&inside).is_none(), "leaving it");
    assert_eq!(pane.contents(), "normal");
    copied(&pane);
}

#[test]
fn a_wide_character_that_does_not_fit_moves_to_the_next_row() {
    let mut pane = quiet(3, 10);
    pane.feed("123456789漢".as_bytes());
    assert_eq!(pane.contents(), "123456789\n漢");
    copied(&pane);
}

#[test]
fn combining_marks_stay_with_their_character() {
    let mut pane = quiet(3, 10);
    pane.feed("e\u{301}x".as_bytes());
    assert_eq!(pane.contents(), "e\u{301}x");
    copied(&pane);
}

#[test]
fn a_scroll_region_moves_only_its_rows() {
    let mut pane = quiet(6, 10);
    pane.feed(b"top\r\n\x1b[2;4r\x1b[2;1Ha\r\nb\r\nc\r\nd\r\n\x1b[r\x1b[6;1Hbottom");
    let device = copied(&pane);
    assert_eq!(device.contents(), pane.contents());
    assert!(pane.contents().starts_with("top\n"));
}

#[test]
fn bracketed_paste_is_carried_on_and_off() {
    let mut pane = quiet(3, 10);
    pane.feed(b"\x1b[?2004h");
    assert!(pane.bracketed_paste());
    assert!(copied(&pane).bracketed_paste());
    let on = pane.snapshot();
    pane.feed(b"\x1b[?2004l");
    let change = pane.diff(&on).unwrap();
    assert!(String::from_utf8_lossy(&change).contains("\x1b[?2004l"));
}

#[test]
fn colours_past_sixteen_and_true_colour_are_copied() {
    let mut pane = quiet(3, 20);
    pane.feed(b"\x1b[38;5;200mA\x1b[48;2;1;2;3mB\x1b[0;1;4:3;9mC\x1b[m");
    copied(&pane);
}

#[test]
fn a_hidden_cursor_stays_hidden() {
    let mut pane = quiet(3, 10);
    pane.feed(b"\x1b[?25l");
    let shown = quiet(3, 10).snapshot();
    assert_ne!(pane.snapshot(), shown);
    copied(&pane);
}

#[test]
fn the_style_in_use_carries_over_to_what_comes_next() {
    let mut pane = quiet(3, 20);
    pane.feed(b"\x1b[31mred");
    let mut device = copied(&pane);
    pane.feed(b" more");
    device.feed(b" more");
    assert_eq!(device.snapshot(), pane.snapshot());
}

#[test]
fn history_is_copied_with_the_screen() {
    let mut pane = Term::new(24, 80, 10_000, |_| {});
    for i in 1..=3000 {
        pane.feed(format!("line-{i}\r\n").as_bytes());
    }
    let last = pane.history(2000);
    let lines: Vec<&str> = last.lines().collect();
    assert_eq!(lines.len(), 2000);
    assert_eq!(lines[0], "line-1001");
    assert_eq!(lines[1999], "line-3000");
    let mut device = Term::new(24, 80, 10_000, |_| {});
    device.feed(&pane.formatted(2000));
    assert_eq!(device.history(2000), last);
    assert_eq!(device.snapshot(), pane.snapshot());
}

#[test]
fn the_cursor_position_is_answered_where_it_was_asked() {
    let (mut term, said) = asking(5, 20);
    term.feed(b"abc\x1b[6nde");
    assert_eq!(*said.lock().unwrap(), ["\x1b[1;4R"]);
}

#[test]
fn a_question_split_across_two_reads_is_answered_once() {
    let (mut term, said) = asking(5, 20);
    term.feed(b"\x1b[");
    assert!(said.lock().unwrap().is_empty());
    term.feed(b"6n");
    assert_eq!(*said.lock().unwrap(), ["\x1b[1;1R"]);
}

#[test]
fn the_terminal_says_what_it_is() {
    let (mut term, said) = asking(5, 20);
    term.feed(b"\x1b[c");
    let said = said.lock().unwrap();
    assert_eq!(said.len(), 1);
    assert!(said[0].starts_with("\x1b[?"), "{:?}", said[0]);
}

#[test]
fn the_background_is_the_atlas_background() {
    let (mut term, said) = asking(5, 20);
    term.feed(b"\x1b]11;?\x07");
    let said = said.lock().unwrap();
    assert_eq!(said.len(), 1);
    assert!(said[0].contains("rgb:0505/0808/0f0f"), "{:?}", said[0]);
}

#[test]
fn a_copy_asks_nothing() {
    let (mut pane, _) = asking(5, 20);
    pane.feed(b"\x1b[?2004h\x1b[?1h\x1b[?1000h\x1b[?1006hhello");
    let (mut device, said) = asking(5, 20);
    device.feed(&pane.formatted(0));
    assert!(said.lock().unwrap().is_empty(), "{:?}", said.lock().unwrap());
    assert_eq!(device.modes(), pane.modes());
}

#[test]
fn cells_carry_resolved_colours_and_wide_characters() {
    let mut pane = quiet(2, 10);
    pane.feed("\x1b[31;1mR\x1b[0m\x1b[7mI\x1b[0m漢".as_bytes());
    let row = &pane.cells()[0];
    assert_eq!(row.len(), 10, "a cell per column");
    assert_eq!(row[0].text, "R");
    assert_eq!(row[0].fg, weft_term::palette::ANSI[1]);
    assert!(row[0].bold && row[0].bg.is_none());
    assert_eq!(row[1].bg, Some(weft_term::palette::FOREGROUND), "inverse swaps");
    assert!(row[2].wide && row[2].text == "漢");
    assert_eq!(row[3].text, "", "the second half of a wide character");
}

#[test]
fn a_selection_is_copied_as_text() {
    let mut pane = quiet(3, 20);
    pane.feed(b"hello world\r\nsecond line");
    assert_eq!(pane.selection_text(), None);
    pane.select_from((0, 6));
    pane.select_to((1, 5));
    assert_eq!(pane.selection_text().as_deref(), Some("world\nsecond"));
    assert!(pane.cells()[0][6].selected && !pane.cells()[0][5].selected);
    pane.clear_selection();
    assert_eq!(pane.selection_text(), None);
}

#[test]
fn the_cursor_hides_when_the_view_is_in_history() {
    let mut pane = quiet(3, 20);
    for i in 0..10 {
        pane.feed(format!("line {i}\r\n").as_bytes());
    }
    assert!(pane.cursor_visible());
    pane.scroll(2);
    assert!(!pane.cursor_visible());
}
