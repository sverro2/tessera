//! The document size: a page the drawing is meant for, shown on the canvas
//! as a guide (drawing past it is fine) and, if chosen, what's exported.

use iced::{Point, Size};

/// The page: a rectangle in world units (a pixel each, exported at 1×),
/// level with the world's axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Page {
    pub center: Point,
    pub size: Size,
}

impl Page {
    /// The largest page either way (pixels at 1×), and the smallest.
    pub const MAX_SIDE: f32 = 100_000.0;
    pub const MIN_SIDE: f32 = 1.0;

    /// Its top left corner.
    pub fn min(self) -> Point {
        Point::new(
            self.center.x - self.size.width / 2.0,
            self.center.y - self.size.height / 2.0,
        )
    }

    /// Its corners, clockwise from the top left.
    pub fn corners(self) -> [Point; 4] {
        let Point { x, y } = self.min();
        let Size { width, height } = self.size;
        [
            Point::new(x, y),
            Point::new(x + width, y),
            Point::new(x + width, y + height),
            Point::new(x, y + height),
        ]
    }

    /// Whether `size` will do for a page: finite, and neither too small nor
    /// too large.
    pub fn fits(size: Size) -> bool {
        [size.width, size.height]
            .iter()
            .all(|&side| side.is_finite() && (Self::MIN_SIDE..=Self::MAX_SIDE).contains(&side))
    }
}

/// A common document size, in pixels, upright (portrait) where it has an
/// orientation of its own on paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    A3At300,
    A4At300,
    A4At150,
    A5At300,
    LetterAt300,
    FullHd,
    Uhd4k,
    Square,
    InstagramPortrait,
    PhoneWallpaper,
    /// Any size, typed in pixels.
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 11] = [
        Preset::A3At300,
        Preset::A4At300,
        Preset::A4At150,
        Preset::A5At300,
        Preset::LetterAt300,
        Preset::FullHd,
        Preset::Uhd4k,
        Preset::Square,
        Preset::InstagramPortrait,
        Preset::PhoneWallpaper,
        Preset::Custom,
    ];

    /// Its size in pixels (width, height); `None` for a custom one.
    pub fn pixels(self) -> Option<(u32, u32)> {
        Some(match self {
            Preset::A3At300 => (3508, 4961),
            Preset::A4At300 => (2480, 3508),
            Preset::A4At150 => (1240, 1754),
            Preset::A5At300 => (1748, 2480),
            Preset::LetterAt300 => (2550, 3300),
            Preset::FullHd => (1920, 1080),
            Preset::Uhd4k => (3840, 2160),
            Preset::Square => (1080, 1080),
            Preset::InstagramPortrait => (1080, 1350),
            Preset::PhoneWallpaper => (1080, 1920),
            Preset::Custom => return None,
        })
    }

    /// The preset `width` × `height` is (either way round), if any; else
    /// custom.
    pub fn of(width: u32, height: u32) -> Preset {
        Preset::ALL
            .into_iter()
            .find(|preset| {
                preset
                    .pixels()
                    .is_some_and(|(w, h)| (w, h) == (width, height) || (h, w) == (width, height))
            })
            .unwrap_or(Preset::Custom)
    }
}

impl std::fmt::Display for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Preset::A3At300 => "A3, 300 PPI",
            Preset::A4At300 => "A4, 300 PPI",
            Preset::A4At150 => "A4, 150 PPI",
            Preset::A5At300 => "A5, 300 PPI",
            Preset::LetterAt300 => "US Letter, 300 PPI",
            Preset::FullHd => "Full HD",
            Preset::Uhd4k => "4K UHD",
            Preset::Square => "Square",
            Preset::InstagramPortrait => "Instagram portrait",
            Preset::PhoneWallpaper => "Phone wallpaper",
            Preset::Custom => return f.write_str("Custom size"),
        };
        let (w, h) = self.pixels().unwrap_or_default();
        write!(f, "{name} ({w} × {h})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_told_as_its_preset_either_way_round() {
        assert_eq!(Preset::of(2480, 3508), Preset::A4At300);
        assert_eq!(Preset::of(3508, 2480), Preset::A4At300);
        assert_eq!(Preset::of(1000, 700), Preset::Custom);
    }

    #[test]
    fn a_page_lies_around_its_centre() {
        let page = Page {
            center: Point::new(100.0, 50.0),
            size: Size::new(40.0, 20.0),
        };
        assert_eq!(page.min(), Point::new(80.0, 40.0));
        assert_eq!(page.corners()[2], Point::new(120.0, 60.0));
        assert!(Page::fits(Size::new(2480.0, 3508.0)));
        assert!(!Page::fits(Size::new(0.0, 10.0)));
        assert!(!Page::fits(Size::new(f32::NAN, 10.0)));
    }
}
