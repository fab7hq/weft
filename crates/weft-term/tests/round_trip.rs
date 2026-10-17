//! Recorded harness output, fed in the chunks the daemon's reader sees: a
//! second terminal fed only `formatted` and then each `diff` shows the same
//! screen after every chunk, and the text is the text vt100 showed for the
//! same bytes (the goldens in weftd's fixtures).
//!
//! vt100 kept the spaces a program wrote at the end of a row; an emulator
//! cell does not know it was written, so rows end at their last character.
//! Sense was trained on rows trimmed the same way.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use weft_term::Term;

const CHUNK: usize = 128;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../weftd/tests/fixtures/screens")
}

fn quiet(rows: u16, cols: u16) -> Term {
    Term::new(rows, cols, 0, |_| {})
}

/// Feed `bytes` chunk by chunk; after each, bring a device up to date with
/// frames alone and check it shows what the pane shows. The frames are
/// written to `<name>.frames.json` with the text after each, for the web
/// app's test to draw the same frames in xterm.js and compare.
fn round_trip(name: &str, rows: u16, cols: u16) {
    let bytes = std::fs::read(fixtures().join(format!("{name}.bin"))).unwrap();
    let mut pane = quiet(rows, cols);
    let mut device = quiet(rows, cols);
    let mut last = None;
    let mut frames = Vec::new();
    for (n, chunk) in bytes.chunks(CHUNK).enumerate() {
        pane.feed(chunk);
        let (full, frame) = match last.as_ref().and_then(|l| pane.diff(l)) {
            Some(frame) => (false, frame),
            None => {
                device = quiet(rows, cols);
                (true, pane.formatted(0))
            }
        };
        device.feed(&frame);
        assert_eq!(device.snapshot(), pane.snapshot(), "{name} chunk {n}");
        // Keys in sorted order: the file is the same whether serde_json keeps
        // a map's order or sorts it, which depends on the build.
        frames.push(json!({
            "alternate": pane.alt_screen(),
            "bytes": weft_proto::screen_json(&weft_proto::OpenScreen {
                pane: 0,
                seq: 0,
                rows,
                cols,
                bytes: frame.clone(),
            })["bytes"],
            "contents": pane.contents(),
            "full": full,
        }));
        last = Some(pane.snapshot());
    }
    assert!(frames.iter().filter(|f| f["full"] == false).count() > 0, "{name}: differences");
    let written = json!({"cols": cols, "frames": frames, "rows": rows});
    std::fs::write(
        fixtures().join(format!("{name}.frames.json")),
        serde_json::to_string_pretty(&written).unwrap() + "\n",
    )
    .unwrap();
}

/// The text after every chunk equals what vt100 showed.
fn same_text(name: &str) {
    let golden: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join(format!("{name}.text.json"))).unwrap(),
    )
    .unwrap();
    let rows = golden["rows"].as_u64().unwrap() as u16;
    let cols = golden["cols"].as_u64().unwrap() as u16;
    let chunk = golden["chunk"].as_u64().unwrap() as usize;
    let bytes = std::fs::read(fixtures().join(format!("{name}.bin"))).unwrap();
    let mut pane = quiet(rows, cols);
    for (n, (piece, want)) in
        bytes.chunks(chunk).zip(golden["frames"].as_array().unwrap()).enumerate()
    {
        pane.feed(piece);
        let want_contents = trimmed(want["contents"].as_str().unwrap());
        assert_eq!(pane.contents(), want_contents, "{name} {n} contents");
        let mut bottom: Vec<String> =
            pane.rows().into_iter().filter(|l| !l.trim().is_empty()).collect();
        bottom.drain(..bottom.len().saturating_sub(15));
        let want_bottom: Vec<&str> = want["bottom"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_str().unwrap().trim_end_matches(' '))
            .collect();
        assert_eq!(bottom, want_bottom, "{name} {n} bottom");
        let cursor = pane.cursor();
        assert_eq!(
            [cursor.0 as u64, cursor.1 as u64],
            [want["cursor"][0].as_u64().unwrap(), want["cursor"][1].as_u64().unwrap()],
            "{name} {n} cursor"
        );
        assert_eq!(pane.alt_screen(), want["alternate"].as_bool().unwrap(), "{name} {n} alt");
    }
}

/// Each row without its trailing spaces, trailing empty rows dropped.
fn trimmed(contents: &str) -> String {
    let rows: Vec<&str> = contents.split('\n').map(|r| r.trim_end_matches(' ')).collect();
    rows.join("\n").trim_end_matches('\n').to_string()
}

#[test]
fn claude_round_trips() {
    round_trip("claude", 30, 100);
}

#[test]
fn codex_round_trips() {
    round_trip("codex", 30, 100);
}

#[test]
fn less_round_trips() {
    round_trip("less", 24, 80);
}

#[test]
fn claude_text_is_vt100s() {
    same_text("claude");
}

#[test]
fn codex_text_is_vt100s() {
    same_text("codex");
}

#[test]
fn less_text_is_vt100s() {
    same_text("less");
}
