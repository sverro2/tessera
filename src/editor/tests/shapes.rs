//! Shapes: what's hovered, highlighted, and framed (/).

use super::*;

#[test]
fn slash_frames_the_shape_pointed_at_else_the_layer() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let slash = || press(letter('/'), keyboard::Modifiers::empty());
    let framed = |x: f32, y: f32| {
        let mut state = State::default();
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        let published = editor
            .update(&mut state, &slash(), bounds, cursor)
            .and_then(|action| action.into_inner().0);
        match published {
            Some(Message::Frame {
                points,
                layer: None,
                pointed,
            }) => {
                let mut points: Vec<_> = points.iter().map(|p| (p.x, p.y)).collect();
                points.sort_by(|a, b| a.partial_cmp(b).unwrap());
                points.dedup();
                (points, pointed)
            }
            other => panic!("{other:?}"),
        }
    };
    // Over the left triangle: it.
    let left = [0, 1, 2].map(|v| (doc.vertex(v).x, doc.vertex(v).y));
    let mut left = left.to_vec();
    left.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(framed(150.0, 280.0), (left, true));
    // Off the drawing: all of it (not pointed at).
    let (all, pointed) = framed(700.0, 550.0);
    assert_eq!((all.len(), pointed), (6, false));
}

/// What / frames over `(x, y)` with `editor`: how many points, and on
/// which other layer.
fn slash_frames(editor: &Editor, x: f32, y: f32) -> (usize, Option<NodeId>) {
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let slash = press(letter('/'), keyboard::Modifiers::empty());
    let cursor = mouse::Cursor::Available(Point::new(x, y));
    let published = editor
        .update(&mut State::default(), &slash, bounds, cursor)
        .and_then(|action| action.into_inner().0);
    match published {
        Some(Message::Frame { points, layer, .. }) => (points.len(), layer),
        other => panic!("{other:?}"),
    }
}

#[test]
fn slash_over_another_layer_frames_its_shape_and_switches_to_it() {
    let doc = one();
    let other = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.below = vec![SceneLayer {
        id: 7,
        document: &other,
        show_edges: true,
        mirrors: &[],
    }];
    // Over this layer's triangle: it, here (though the other's is there,
    // behind).
    assert_eq!(slash_frames(&editor, 150.0, 280.0), (3, None));
    // Over only the other layer's right triangle: it, there.
    assert_eq!(slash_frames(&editor, 320.0, 280.0), (3, Some(7)));
    // Over neither: all of this layer.
    assert_eq!(slash_frames(&editor, 700.0, 550.0), (3, None));
}

#[test]
fn slash_frames_the_shape_in_front_as_drawn() {
    let doc = one();
    let other = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.above = vec![SceneLayer {
        id: 7,
        document: &other,
        show_edges: true,
        mirrors: &[],
    }];
    // Over the other layer's left triangle, in front of this layer's:
    // shaping, this layer's (drawn over all); painting, the other's.
    assert_eq!(slash_frames(&editor, 150.0, 280.0), (3, None));
    editor.tool = painting_faces();
    assert_eq!(slash_frames(&editor, 150.0, 280.0), (3, Some(7)));
    // Over this layer's alone: it.
    assert_eq!(slash_frames(&editor, 230.0, 200.0), (3, None));
}

#[test]
fn hovering_inside_a_triangle_finds_the_face() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    assert!(matches!(
        editor.hit_test(Point::new(200.0, 250.0)),
        Some(Hover::Face(0))
    ));
    assert!(matches!(
        editor.hit_test(Point::new(200.0, 302.0)),
        Some(Hover::Edge { .. })
    ));
    assert!(editor.hit_test(Point::new(500.0, 500.0)).is_none());
}

#[test]
fn the_hovered_shape_stays_until_another_is_hovered() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let hover = |editor: &Editor, state: &mut State, x, y| {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        editor.update(state, &Event::Mouse(MOVE), bounds, cursor);
        state.shape.as_ref().map(|shape| shape.triangles.clone())
    };
    let shape = |v| Some(editor.highlight(v).triangles);

    let left = hover(&editor, &mut state, 150.0, 270.0);
    assert_eq!(left, shape(0));
    state.fading = None;
    assert_eq!(hover(&editor, &mut state, 600.0, 500.0), left); // blank canvas
    assert_eq!(hover(&editor, &mut state, 100.0, 300.0), left); // another corner
    assert!(state.fading.is_none(), "same shape: no fade");

    assert_eq!(hover(&editor, &mut state, 300.0, 270.0), shape(3));
    assert!(matches!(state.fading, Some((Some(_), _))), "fades over");

    // After an edit joining the two, the highlight follows (once).
    let mut joined = doc.clone();
    joined.apply(Edit::MoveVertex {
        id: 1,
        to: doc.vertex(3),
    });
    let editor = super::tests::editor(&joined, &cache);
    let triangles = hover(&editor, &mut state, 600.0, 500.0).unwrap();
    assert_eq!(triangles.len(), 2);
}

#[test]
fn a_highlight_from_another_drawing_is_dropped() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let cursor = |x, y| mouse::Cursor::Available(Point::new(x, y));
    // The right triangle (vertices 3–5) highlighted, the left fading out.
    editor.update(
        &mut state,
        &Event::Mouse(MOVE),
        bounds,
        cursor(150.0, 270.0),
    );
    editor.update(
        &mut state,
        &Event::Mouse(MOVE),
        bounds,
        cursor(300.0, 270.0),
    );
    assert!(matches!(state.fading, Some((Some(_), _))));

    // Another drawing (opened, or another layer) with fewer vertices: no
    // shape to find from vertex 3, so no highlight (not a panic).
    let mut smaller = Document::default();
    smaller.apply(Edit::AddTriangle {
        corners: [
            Point::new(500.0, 500.0),
            Point::new(560.0, 500.0),
            Point::new(530.0, 450.0),
        ],
        snap: 0.0,
    });
    let editor = super::tests::editor(&smaller, &cache);
    editor.update(
        &mut state,
        &Event::Mouse(MOVE),
        bounds,
        cursor(700.0, 100.0),
    );
    assert!(state.shape.is_none());
    assert!(
        matches!(state.fading, Some((Some(_), _))),
        "vertex 0 is there"
    );
}

#[test]
fn a_shape_is_everything_connected_through_vertices() {
    let mut doc = two_apart();
    assert_eq!(doc.connected(0).len(), 1);
    assert_eq!(outline_of(&doc.connected(0)).len(), 3);

    // Welded tip to tip, the two triangles are one shape.
    doc.apply(Edit::MoveVertex {
        id: 1,
        to: doc.vertex(3),
    });
    assert_eq!(doc.connected(0).len(), 2);
    assert_eq!(outline_of(&doc.connected(0)).len(), 6);
}
