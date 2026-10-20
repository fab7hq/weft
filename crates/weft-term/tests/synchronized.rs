//! A frame is never half a redraw: while a program's synchronized update
//! (`?2026h` … `?2026l`) is open, the terminal says to hold the frame, for up
//! to a second from its start.

use std::path::Path;
use std::time::{Duration, Instant};

use weft_term::Term;

const CHUNK: usize = 128;
const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";

/// Whether a synchronized update is open after the first `at` bytes: the last
/// whole marker before it begins one.
fn open_at(bytes: &[u8], at: usize) -> bool {
    let seen = &bytes[..at];
    let last = |marker: &[u8]| seen.windows(marker.len()).rposition(|w| w == marker);
    match (last(BEGIN), last(END)) {
        (Some(begin), Some(end)) => begin > end,
        (Some(_), None) => true,
        _ => false,
    }
}

#[test]
fn no_frame_is_taken_inside_a_synchronized_update() {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../weftd/tests/fixtures/screens/codex.bin"),
    )
    .unwrap();
    let mut pane = Term::new(24, 80, 0, |_| {});
    let (mut held, mut taken) = (0, 0);
    for (n, chunk) in bytes.chunks(CHUNK).enumerate() {
        pane.feed(chunk);
        let open = open_at(&bytes, (n * CHUNK + chunk.len()).min(bytes.len()));
        let now = Instant::now();
        assert_eq!(pane.holding(now), open, "chunk {n}: a frame now would show half a redraw");
        // Past a second, the frame is taken anyway.
        assert!(!pane.holding(now + Duration::from_millis(1100)), "chunk {n} held past 1 s");
        if open { held += 1 } else { taken += 1 }
    }
    assert!(held > 0 && taken > 0, "{held} held, {taken} taken: the fixture tests nothing");
}

/// A marker split between two reads counts once it is whole, and a second
/// begin inside an open update keeps its start.
#[test]
fn a_marker_split_across_reads_counts_when_whole() {
    let mut pane = Term::new(24, 80, 0, |_| {});
    let start = Instant::now();
    pane.feed(b"a\x1b[?20");
    assert!(!pane.holding(start), "half a marker");
    pane.feed(b"26hb");
    assert!(pane.holding(Instant::now()));
    pane.feed(b"\x1b[?2026hc");
    assert!(!pane.holding(start + Duration::from_millis(1100)), "the first begin is the start");
    pane.feed(b"\x1b[?2026");
    assert!(pane.holding(Instant::now()), "half an end");
    pane.feed(b"l");
    assert!(!pane.holding(Instant::now()));
}
