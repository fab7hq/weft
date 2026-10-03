//! The frames a remote device draws (Phase 6 step 2, remote.md D3).
//!
//! A pane's recorded output, fed in chunks the way the reader thread sees it,
//! becomes one `state_formatted()` frame and then a `state_diff()` frame per
//! chunk. Each frame is written to `fixtures/screens/<name>.frames.json` with
//! the text vt100 shows after it, so the web app's test can feed the same
//! frames to xterm.js and compare (web-app.md §11 test 1).

use std::path::Path;

use serde_json::{Value, json};

const CHUNK: usize = 128;

fn frames_of(bytes: &[u8], rows: u16, cols: u16) -> Value {
    let mut pane = vt100::Parser::new(rows, cols, 0);
    let mut last: Option<vt100::Screen> = None;
    let mut frames = Vec::new();
    for chunk in bytes.chunks(CHUNK) {
        pane.process(chunk);
        let screen = pane.screen();
        let (full, frame) = match &last {
            None => (true, screen.state_formatted()),
            Some(prev) => (false, screen.state_diff(prev)),
        };
        let wire = weft_proto::output_json(0, &frame);
        frames.push(json!({
            "full": full,
            "bytes": wire["bytes"],
            "contents": screen.contents(),
            "alternate": screen.alternate_screen(),
        }));
        last = Some(screen.clone());
    }
    json!({"rows": rows, "cols": cols, "frames": frames})
}

/// What a device's own parser shows after every frame, fed nothing else.
fn drawn(frames: &Value, rows: u16, cols: u16) -> Vec<String> {
    let mut device = vt100::Parser::new(rows, cols, 0);
    frames["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let (_, bytes) =
                weft_proto::output_of(&json!({"pane": 0, "bytes": f["bytes"]})).unwrap();
            device.process(&bytes);
            device.screen().contents()
        })
        .collect()
}

#[test]
fn every_frame_draws_the_screen_the_pane_shows() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/screens");
    let mut alternate = false;
    let mut wide = false;
    for (name, rows, cols) in [("claude", 30, 100), ("codex", 30, 100), ("less", 24, 80)] {
        let bytes = std::fs::read(dir.join(format!("{name}.bin"))).unwrap();
        let frames = frames_of(&bytes, rows, cols);
        let expected: Vec<&str> = frames["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["contents"].as_str().unwrap())
            .collect();
        assert!(expected.len() > 1, "{name}: a full frame and differences");
        assert_eq!(drawn(&frames, rows, cols), expected, "{name}");
        alternate |= frames["frames"].as_array().unwrap().iter().any(|f| f["alternate"] == true);
        wide |= expected.iter().any(|c| c.chars().any(|ch| ch as u32 >= 0x1100));
        std::fs::write(
            dir.join(format!("{name}.frames.json")),
            serde_json::to_string_pretty(&frames).unwrap() + "\n",
        )
        .unwrap();
    }
    assert!(alternate, "a fixture draws the alternate screen");
    assert!(wide, "a fixture holds wide characters");
}
