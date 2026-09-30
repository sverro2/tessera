//! Face colours: writing them as `#rrggbb`, the preset palette, and the
//! colours used recently.

use iced::Color;

/// The colours to pick from right away.
pub const PALETTE: [Color; 12] = [
    Color::from_rgb8(0xf7, 0x76, 0x8e),
    Color::from_rgb8(0xff, 0x9e, 0x64),
    Color::from_rgb8(0xe0, 0xaf, 0x68),
    Color::from_rgb8(0x9e, 0xce, 0x6a),
    Color::from_rgb8(0x73, 0xda, 0xca),
    Color::from_rgb8(0x7d, 0xcf, 0xff),
    Color::from_rgb8(0x7a, 0xa2, 0xf7),
    Color::from_rgb8(0xbb, 0x9a, 0xf7),
    Color::from_rgb8(0xff, 0xff, 0xff),
    Color::from_rgb8(0xa9, 0xb1, 0xd6),
    Color::from_rgb8(0x56, 0x5f, 0x89),
    Color::from_rgb8(0x00, 0x00, 0x00),
];

/// What paint mode paints: faces or edges, never both in one stroke.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Faces,
    Edges,
}

/// What painting a face does: give it a colour, or clear it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Brush {
    Color(Color),
    Eraser,
}

impl Brush {
    /// The colour a painted face gets; `None` for the eraser.
    pub fn color(self) -> Option<Color> {
        match self {
            Brush::Color(color) => Some(color),
            Brush::Eraser => None,
        }
    }
}

impl Default for Brush {
    fn default() -> Self {
        Brush::Color(PALETTE[6])
    }
}

/// How many recently used colours are kept.
const RECENT: usize = 10;

/// `#rrggbb`.
pub fn to_hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Reads `#rrggbb` (the `#` optional; also `#rgb`).
pub fn from_hex(text: &str) -> Option<Color> {
    let hex = text.trim().trim_start_matches('#');
    let digit = |i: usize, n: usize| u8::from_str_radix(hex.get(i..i + n)?, 16).ok();
    let (r, g, b) = match hex.len() {
        6 => (digit(0, 2)?, digit(2, 2)?, digit(4, 2)?),
        3 => (digit(0, 1)? * 17, digit(1, 1)? * 17, digit(2, 1)? * 17),
        _ => return None,
    };
    Some(Color::from_rgb8(r, g, b))
}

/// Hue (0 to 1, from red round through green and blue), saturation and
/// value (both 0 to 1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Hsv {
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
}

impl Hsv {
    pub fn to_color(self) -> Color {
        let Hsv {
            hue,
            saturation: s,
            value: v,
        } = self;
        let h = hue.rem_euclid(1.0) * 6.0;
        let f = h.fract();
        let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
        let (r, g, b) = match h as u32 {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            _ => (v, p, q),
        };
        Color::from_rgb(r, g, b)
    }

    /// `color` as hue, saturation and value; for greys (which have no hue)
    /// keeping `hue`.
    pub fn from_color(color: Color, hue: f32) -> Hsv {
        let Color { r, g, b, .. } = color;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let hue = if d <= 1e-6 {
            hue
        } else if max == r {
            ((g - b) / d / 6.0).rem_euclid(1.0)
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        Hsv {
            hue,
            saturation: if max <= 1e-6 { 0.0 } else { d / max },
            value: max,
        }
    }
}

/// Puts `color` first in the recently used colours, most recent first.
pub fn remember(recent: &mut Vec<Color>, color: Color) {
    recent.retain(|&c| c != color);
    recent.insert(0, color);
    recent.truncate(RECENT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        for color in PALETTE {
            assert_eq!(from_hex(&to_hex(color)), Some(color));
        }
        assert_eq!(from_hex("f00"), Some(Color::from_rgb8(255, 0, 0)));
        assert_eq!(from_hex("#12345"), None);
        assert_eq!(from_hex("#gggggg"), None);
    }

    #[test]
    fn hsv_round_trips() {
        for color in PALETTE {
            let back = Hsv::from_color(color, 0.0).to_color();
            assert_eq!(to_hex(back), to_hex(color));
        }
        // Greys keep the hue they had.
        let grey = Hsv::from_color(Color::from_rgb(0.5, 0.5, 0.5), 0.3);
        assert_eq!((grey.hue, grey.saturation), (0.3, 0.0));
    }

    #[test]
    fn remembers_the_most_recent_first() {
        let mut recent = vec![];
        let [a, b] = [PALETTE[0], PALETTE[1]];
        remember(&mut recent, a);
        remember(&mut recent, b);
        remember(&mut recent, a);
        assert_eq!(recent, vec![a, b]);
    }
}
