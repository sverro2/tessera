//! The backdrop: the document's sheet, the background image, the grid.

use super::*;

#[test]
fn the_document_size_shows_over_wide_edges_along_its_top() {
    // A painted sheet's worth, its edges painted: zoomed far in, wide.
    let mut doc = painted(
        &[[(0.0, 0.0), (400.0, 0.0), (0.0, 300.0)]],
        Color::from_rgb8(200, 60, 60),
    );
    let edge = EdgeStyle {
        color: Color::from_rgb8(190, 200, 240),
        width: 2.0,
    };
    assert!(doc.apply(Edit::PaintAllEdges { style: Some(edge) }));
    let size = Size::new(400.0, 300.0);
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.tool = painting_faces();
    // Its top left corner at (60, 60) on screen.
    editor.camera = Camera {
        zoom: 16.0,
        pan: Vector::new(60.0, 60.0),
        ..Camera::default()
    };
    let without = rendered(&editor, size);
    let cache = Caches::default();
    editor.caches = &cache;
    editor.page = Some(Page {
        center: Point::new(200.0, 150.0),
        size: Size::new(400.0, 300.0),
    });
    let with = rendered(&editor, size);

    // The top edge reaches 16 px past it, past the label (18 px above the
    // corner, to its right).
    assert_eq!(without(150, 50), [190, 200, 240]);
    assert_eq!(with(150, 50), [190, 200, 240]);
    // But the label's seen, over it.
    let differ = (45..60)
        .flat_map(|y| (60..110).map(move |x| (x, y)))
        .filter(|&(x, y)| without(x, y) != with(x, y))
        .count();
    assert!(differ > 20, "{differ}");
}
