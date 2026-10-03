//! PNG export: the SVG export (see [`crate::svg`]), rasterised, so the two
//! always look alike.

use iced::Color;
use resvg::tiny_skia;
use resvg::usvg;

/// The largest image made, either way (pixels): larger ones take too much
/// memory to be worth it.
pub const MAX_SIDE: u32 = 16_384;

/// What the background of an exported image is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// See-through.
    #[default]
    Transparent,
    White,
    /// As on the canvas.
    Canvas,
}

impl Backdrop {
    pub const ALL: [Backdrop; 3] = [Backdrop::Transparent, Backdrop::White, Backdrop::Canvas];

    pub fn label(self) -> &'static str {
        match self {
            Backdrop::Transparent => "Transparent",
            Backdrop::White => "White",
            Backdrop::Canvas => "Canvas",
        }
    }

    fn color(self) -> Option<Color> {
        match self {
            Backdrop::Transparent => None,
            Backdrop::White => Some(Color::WHITE),
            Backdrop::Canvas => Some(crate::editor::BACKGROUND),
        }
    }
}

/// How large an image of `width` × `height` (world units) is at `scale`, in
/// whole pixels (at least one each way).
pub fn pixels(width: f32, height: f32, scale: f32) -> (u32, u32) {
    let side = |length: f32| (length * scale).ceil().max(1.0) as u32;
    (side(width), side(height))
}

/// `svg` as a PNG, `scale` pixels to its unit, on `backdrop`.
pub fn png(svg: &str, scale: f32, backdrop: Backdrop) -> Result<Vec<u8>, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (width, height) = pixels(size.width(), size.height(), scale);
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!("{width} × {height} pixels is too large"));
    }
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| format!("can't make an image of {width} × {height} pixels"))?;
    if let Some(color) = backdrop.color() {
        pixmap.fill(tiny_skia::Color::from_rgba8(
            (color.r * 255.0).round() as u8,
            (color.g * 255.0).round() as u8,
            (color.b * 255.0).round() as u8,
            255,
        ));
    }
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Edit};
    use crate::layers::Layers;
    use iced::Point;

    #[test]
    fn the_svg_export_rasterised() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(40.0, 0.0),
                Point::new(0.0, 30.0),
            ],
            snap: 0.0,
        });
        document.apply(Edit::Paint {
            triangle: 0,
            color: Some(Color::from_rgb8(255, 0, 0)),
        });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let svg = crate::svg::export(&layers, None);

        let png = png(&svg, 2.0, Backdrop::White).unwrap();
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .unwrap()
            .to_rgba8();
        // 40 × 30, with 10 room all round, twice over.
        assert_eq!(image.dimensions(), (120, 100));
        // In the face, red; off it, the backdrop.
        assert_eq!(image.get_pixel(30, 30).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(110, 90).0, [255, 255, 255, 255]);

        let png = super::png(&svg, 1.0, Backdrop::Transparent).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(55, 45).0[3], 0);

        assert!(super::png(&svg, 1000.0, Backdrop::White).is_err());

        // Unpainted, nothing: see-through there.
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new({
            let mut document = (*layers.layers()[0].document).clone();
            document.apply(Edit::PaintAll { color: None });
            document.apply(Edit::PaintAllEdges { style: None });
            document
        });
        let bare = crate::svg::export(&layers, None);
        let png = super::png(&bare, 2.0, Backdrop::Transparent).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert!(image.pixels().all(|pixel| pixel.0[3] == 0));
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new({
            let mut document = (*layers.layers()[0].document).clone();
            document.apply(Edit::PaintAll {
                color: Some(Color::from_rgb8(255, 0, 0)),
            });
            document
        });

        // Two faces sharing an edge, nothing behind: where they meet, the
        // seal leaves no see-through line (unsealed, there is one), right
        // into the corners; and nothing of it shows past them.
        let mut square = Document::default();
        for corners in [
            [
                Point::new(0.0, 0.0),
                Point::new(40.0, 0.0),
                Point::new(40.0, 40.0),
            ],
            [
                Point::new(0.0, 0.0),
                Point::new(40.0, 40.0),
                Point::new(0.0, 40.0),
            ],
        ] {
            square.apply(Edit::AddTriangle { corners, snap: 0.0 });
        }
        square.apply(Edit::PaintAll {
            color: Some(Color::from_rgb8(255, 0, 0)),
        });
        square.apply(Edit::PaintAllEdges { style: None });
        let mut two = Layers::default();
        let only = two.first_layer();
        two.layer_mut(only).unwrap().document = std::sync::Arc::new(square);
        let render = |seal: f32| {
            let svg = crate::svg::export_sealed(&two, None, seal);
            let png = super::png(&svg, 1.0, Backdrop::Transparent).unwrap();
            image::load_from_memory(&png).unwrap().to_rgba8()
        };
        // (Ten pixels of room all round.)
        let on_the_seam = |image: &image::RgbaImage| {
            (0..40)
                .map(|i| image.get_pixel(10 + i, 10 + i).0[3])
                .min()
                .unwrap()
        };
        let (unsealed, sealed) = (render(0.0), render(crate::svg::SEAL_PIXELS));
        assert!(on_the_seam(&unsealed) < 250, "unsealed, see-through");
        assert_eq!(on_the_seam(&sealed), 255);
        assert!(
            unsealed
                .enumerate_pixels()
                .filter(|(x, y, _)| !(10..50).contains(x) || !(10..50).contains(y))
                .all(|(x, y, pixel)| sealed.get_pixel(x, y) == pixel),
            "the same past the faces"
        );

        // A hidden layer isn't in it, as in the SVG.
        layers.set_visible(first, false);
        let png = super::png(&crate::svg::export(&layers, None), 2.0, Backdrop::White).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(image.get_pixel(30, 30).0, [255, 255, 255, 255]);
    }
}
