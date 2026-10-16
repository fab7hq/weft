//! The two families every surface draws in, bundled (SIL Open Font
//! License, `fonts/OFL-*.txt`): Sora for the interface, JetBrains Mono for
//! the terminal and code.

use std::borrow::Cow;

use gpui::App;

pub const SANS: &str = "Sora";
pub const MONO: &str = "JetBrains Mono";

/// The bundled faces: the weights the cards draw in, and the terminal's
/// bold and italics.
pub const FILES: [&[u8]; 8] = [
    include_bytes!("../fonts/Sora-Regular.ttf"),
    include_bytes!("../fonts/Sora-SemiBold.ttf"),
    include_bytes!("../fonts/Sora-Bold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/JetBrainsMono-SemiBold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Italic.ttf"),
    include_bytes!("../fonts/JetBrainsMono-BoldItalic.ttf"),
];

/// Gives the text system the bundled faces, before the first window draws.
pub fn load(cx: &App) {
    if let Err(err) = cx.text_system().add_fonts(FILES.iter().map(|f| Cow::Borrowed(*f)).collect())
    {
        eprintln!("weft-desktop: the bundled fonts did not load: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The family a face is asked for by: its typographic family when it
    /// names one, else its family.
    fn family(face: &ttf_parser::Face) -> String {
        let named = |id| {
            face.names()
                .into_iter()
                .find(|n| n.name_id == id && n.is_unicode())
                .and_then(|n| n.to_string())
        };
        named(ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
            .or_else(|| named(ttf_parser::name_id::FAMILY))
            .unwrap_or_default()
    }

    #[test]
    fn every_bundled_face_is_one_of_the_two_families() {
        let families: Vec<String> =
            FILES.iter().map(|f| family(&ttf_parser::Face::parse(f, 0).unwrap())).collect();
        assert_eq!(families.iter().filter(|f| *f == SANS).count(), 3, "{families:?}");
        assert_eq!(families.iter().filter(|f| *f == MONO).count(), 5, "{families:?}");
    }

    #[test]
    fn the_mono_is_one_cell_wide_for_every_character() {
        for file in &FILES[3..] {
            let face = ttf_parser::Face::parse(file, 0).unwrap();
            let advance = |c| face.glyph_hor_advance(face.glyph_index(c).unwrap()).unwrap();
            assert_eq!(advance('m'), advance('i'));
            assert_eq!(advance('m'), advance('─'));
        }
    }
}
