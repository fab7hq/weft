//! Write every TUI screen the Atlas changed as text, one file per screen ID,
//! for the owner to compare with the cards:
//!   cargo run --example atlas_screens -- [out dir, default target/atlas-screens]
//!
//! The screens are the TUI's snapshot tests (`atlas_t4_5` is T4.5,
//! `atlas_t11_3` T11.3): `cargo test` fails unless each is exactly what the
//! TUI draws from its fixture, so these files are the screens as built.

use std::path::Path;

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "target/atlas-screens".into());
    std::fs::create_dir_all(&out)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut written = 0;
    for dir in ["src/snapshots", "crates/weft-tui/src/ui/snapshots"] {
        for entry in std::fs::read_dir(root.join(dir))? {
            let path = entry?.path();
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let Some(id) = screen_id(name) else { continue };
            let text = std::fs::read_to_string(&path)?;
            // The snapshot's own header sits between the first two `---` lines.
            let screen = text.splitn(3, "---\n").nth(2).unwrap_or(&text);
            std::fs::write(Path::new(&out).join(format!("{id}.txt")), screen)?;
            written += 1;
        }
    }
    println!("{written} screens in {out}");
    Ok(())
}

/// `…__atlas_t4_11` is T4.11.
fn screen_id(name: &str) -> Option<String> {
    let (_, at) = name.rsplit_once("__atlas_t")?;
    let (section, screen) = at.split_once('_')?;
    Some(format!("T{section}.{screen}"))
}
