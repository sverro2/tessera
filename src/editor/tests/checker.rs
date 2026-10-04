//! The checkerboard behind what's see-through: under all the layers and
//! the backdrop's image, over the background colour.

use super::*;
use iced::advanced::renderer::Headless;

/// The renderer the app falls back on without a GPU: to draw whole frames
/// in tests, anywhere.
fn software_renderer() -> Renderer {
    iced::futures::executor::block_on(<Renderer as Headless>::new(
        iced_renderer::core::renderer::Settings::default(),
        Some("tiny-skia"),
    ))
    .unwrap()
}

/// A layer of triangles, each `color`.
fn painted(triangles: &[[(f32, f32); 3]], color: Color) -> Document {
    let mut doc = Document::default();
    for &t in triangles {
        triangle(&mut doc, t);
    }
    assert!(doc.apply(Edit::PaintAll { color: Some(color) }));
    doc
}

const SEE_THROUGH: Color = Color {
    a: 0.3,
    ..Color::from_rgb8(40, 80, 220)
};

#[test]
fn only_whats_see_through_goes_behind_a_layer() {
    let mut doc = painted(
        &[[(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)]],
        Color::from_rgb8(200, 60, 60),
    );
    triangle(&mut doc, [(300.0, 300.0), (500.0, 300.0), (400.0, 100.0)]);
    let corners = [(300.0, 300.0), (500.0, 300.0), (400.0, 100.0)].map(|(x, y)| Point::new(x, y));
    let see_through = doc.triangle_at(Point::new(400.0, 250.0)).unwrap();
    assert!(doc.apply(Edit::Paint {
        triangle: see_through,
        color: Some(SEE_THROUGH)
    }));
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let renderer = software_renderer();
    let behind = |doc: &Document| {
        let mut frame = Frame::new(&renderer, Size::new(800.0, 600.0));
        let style = Style::of(Look::Painted, true);
        let mut beneath = Vec::new();
        editor.draw_layer(&mut frame, doc, style, &[], None, &mut beneath);
        beneath
    };

    // The see-through face alone.
    let beneath = behind(&doc);
    assert_eq!(beneath.len(), 1);
    assert!(corners.iter().all(|p| beneath[0].contains(p)));

    // An opaque edge adds nothing; a see-through one its outline.
    let edge = |color| EdgeStyle { color, width: 4.0 };
    let (a, b) = doc.unique_edges()[0];
    let mut opaque = doc.clone();
    assert!(opaque.apply(Edit::PaintEdge {
        a,
        b,
        style: Some(edge(Color::WHITE)),
    }));
    assert_eq!(behind(&opaque).len(), 1);
    let mut see_through = doc.clone();
    assert!(see_through.apply(Edit::PaintEdge {
        a,
        b,
        style: Some(edge(SEE_THROUGH)),
    }));
    assert!(behind(&see_through).len() > 1);
}

#[test]
fn see_through_shows_the_checkerboard_unless_something_lies_between() {
    let size = Size::new(400.0, 300.0);
    // A see-through band across: over the background image (on the left,
    // opaque), the bare backdrop (in the middle) and an opaque layer (on
    // the right).
    let band = painted(
        &[
            [(0.0, 100.0), (400.0, 100.0), (0.0, 200.0)],
            [(400.0, 100.0), (400.0, 200.0), (0.0, 200.0)],
        ],
        SEE_THROUGH,
    );
    let green = Color::from_rgb8(40, 200, 60);
    let below = painted(&[[(300.0, -100.0), (700.0, -100.0), (300.0, 700.0)]], green);
    let red = image::RgbaImage::from_pixel(100, 300, image::Rgba([220, 40, 40, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(red)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut background = Background::load(png).unwrap();
    background.center = Point::new(50.0, 150.0);
    background.opacity = 1.0;

    let cache = Caches::default();
    let mut editor = editor(&band, &cache);
    editor.current = 2;
    editor.below = vec![SceneLayer {
        id: 1,
        document: &below,
        show_edges: true,
        mirrors: &[],
    }];
    editor.background = Some(&background);
    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush: Brush::default(),
        width: 2.0,
        picking: false,
    };
    let mut renderer = software_renderer();
    let bounds = Rectangle::new(Point::ORIGIN, size);
    editor.render(
        &mut renderer,
        &State::default(),
        bounds,
        mouse::Cursor::Unavailable,
        false,
    );
    let pixels = renderer.screenshot(
        iced::Size::new(size.width as u32, size.height as u32),
        1.0,
        BACKGROUND,
    );
    let pixel = |x: u32, y: u32| {
        let at = ((y * size.width as u32 + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2]]
    };
    // Two neighbouring squares of the board: told apart, or not.
    let differ = |x: u32, y: u32| {
        let (p, q) = (pixel(x, y), pixel(x + 8, y));
        p.iter().zip(q).any(|(&p, q)| p.abs_diff(q) > 4)
    };

    // Over the bare backdrop: the checkerboard.
    assert!(differ(204, 116));
    // Over the image: the image, no checkerboard.
    assert!(!differ(20, 116));
    assert!(pixel(20, 116)[0] > 120);
    // Over the opaque layer: it, no checkerboard.
    assert!(!differ(340, 180));
    assert!(pixel(340, 180)[1] > 120);
    // Nothing see-through: the bare backdrop.
    assert!(!differ(204, 260));
}
