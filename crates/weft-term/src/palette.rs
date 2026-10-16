//! The colours Weft's terminals draw with, and answer with when a program
//! asks. The same as the web app's xterm.js: its default sixteen, on the
//! Atlas background.

/// A colour, 8 bits a channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl From<alacritty_terminal::vte::ansi::Rgb> for Rgb {
    fn from(c: alacritty_terminal::vte::ansi::Rgb) -> Rgb {
        Rgb { r: c.r, g: c.g, b: c.b }
    }
}

impl From<Rgb> for alacritty_terminal::vte::ansi::Rgb {
    fn from(c: Rgb) -> alacritty_terminal::vte::ansi::Rgb {
        alacritty_terminal::vte::ansi::Rgb { r: c.r, g: c.g, b: c.b }
    }
}

pub const BACKGROUND: Rgb = rgb(0x05080f);
pub const FOREGROUND: Rgb = rgb(0xf1f3f7);
pub const CURSOR: Rgb = rgb(0xf1f3f7);
pub const SELECTION: Rgb = rgb(0x2e4c76);

/// Black, red, green, yellow, blue, magenta, cyan, white, then the bright
/// eight.
pub const ANSI: [Rgb; 16] = [
    rgb(0x2e3436),
    rgb(0xcc0000),
    rgb(0x4e9a06),
    rgb(0xc4a000),
    rgb(0x3465a4),
    rgb(0x75507b),
    rgb(0x06989a),
    rgb(0xd3d7cf),
    rgb(0x555753),
    rgb(0xef2929),
    rgb(0x8ae234),
    rgb(0xfce94f),
    rgb(0x729fcf),
    rgb(0xad7fa8),
    rgb(0x34e2e2),
    rgb(0xeeeeec),
];

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

/// The colour at a palette index, as xterm numbers them: the sixteen, the
/// 6×6×6 cube, the grey ramp, then foreground, background and cursor.
pub fn color(index: usize) -> Rgb {
    match index {
        0..=15 => ANSI[index],
        16..=231 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            Rgb { r: level(i / 36), g: level(i / 6 % 6), b: level(i % 6) }
        }
        232..=255 => {
            let v = (8 + (index - 232) * 10) as u8;
            Rgb { r: v, g: v, b: v }
        }
        256 => FOREGROUND,
        258 => CURSOR,
        _ => BACKGROUND,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_indexes_xterm_uses() {
        assert_eq!(color(1), rgb(0xcc0000));
        assert_eq!(color(16), rgb(0x000000));
        assert_eq!(color(231), rgb(0xffffff));
        assert_eq!(color(232), rgb(0x080808));
        assert_eq!(color(256), FOREGROUND);
        assert_eq!(color(257), BACKGROUND);
    }
}
