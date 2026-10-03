//! Mirrors: placing one, and editing through it.

use super::*;

#[test]
fn a_mirror_is_placed_clear_of_the_drawing() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.tool = Tool::Mirror;
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let place = |from: (f32, f32), to: (f32, f32)| {
        let mut state = State::default();
        let mut published = Vec::new();
        for (event, (x, y)) in [(PRESS, from), (MOVE, to), (RELEASE, to)] {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
            for event in [Event::Mouse(event), frame] {
                if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                    published.extend(action.into_inner().0);
                }
            }
        }
        published.into_iter().find_map(|m| match m {
            Message::SetMirror(mirror) => Some(mirror),
            _ => None,
        })
    };

    // Clear of the triangle (x 100 to 300): placed.
    assert_eq!(
        place((400.0, 100.0), (400.0, 400.0)),
        Some(Mirror {
            a: Point::new(400.0, 100.0),
            b: Point::new(400.0, 400.0),
        })
    );
    // Through it: not.
    assert_eq!(place((200.0, 50.0), (200.0, 450.0)), None);
    // Starting on a vertex, it runs right through it.
    let mirror = place((303.0, 298.0), (303.0, 500.0)).unwrap();
    assert_eq!(mirror.a, Point::new(300.0, 300.0));
}

#[test]
fn a_mirror_snaps_to_run_through_where_the_drawing_starts() {
    let doc = one();
    let none = keyboard::Modifiers::empty();
    let through = |mirror: Mirror, p: (f32, f32)| {
        let p = Point::new(p.0, p.1);
        area2(mirror.a, mirror.b, p).abs() / mirror.a.distance(mirror.b) < 1e-3
    };

    // Drawn just right of corner 1 (300, 300): moved over onto it.
    let mirror = place_mirror(&doc, &[], none, (306.0, 50.0), (306.0, 450.0)).unwrap();
    assert!(through(mirror, (300.0, 300.0)), "{mirror:?}");
    assert!((mirror.a.x - mirror.b.x).abs() < 1e-3, "{mirror:?}");

    // From corner 1 nearly along its edge to corner 2 (200, 100), past
    // it: turned to run right along it.
    let mirror = place_mirror(&doc, &[], none, (300.0, 300.0), (165.0, 22.0)).unwrap();
    assert_eq!(mirror.a, Point::new(300.0, 300.0));
    assert!(through(mirror, (200.0, 100.0)), "{mirror:?}");
}

#[test]
fn the_mirror_image_is_edited_like_the_drawing() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let mirrors = [Mirror {
        a: Point::new(400.0, 0.0),
        b: Point::new(400.0, 600.0),
    }];
    editor.mirrors = &mirrors;
    // Corner 1 (300, 300) shows mirrored at (500, 300); dragged there
    // right and down, it goes left and down.
    let edits = drag(
        &editor,
        &[
            (MOVE, 500.0, 300.0),
            (PRESS, 500.0, 300.0),
            (MOVE, 520.0, 320.0),
            (RELEASE, 520.0, 320.0),
        ],
    );
    let [edits] = &edits[..] else {
        panic!("{edits:?}");
    };
    let [Edit::MoveVertex { id: 1, to }] = edits[..] else {
        panic!("{edits:?}");
    };
    assert!(to.distance(Point::new(280.0, 320.0)) < 1e-3, "{to:?}");

    // Painting its image paints it.
    let brush = Brush::default();
    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush,
        width: 1.0,
        picking: false,
    };
    let edits = drag(
        &editor,
        &[
            (MOVE, 600.0, 250.0),
            (PRESS, 600.0, 250.0),
            (RELEASE, 600.0, 250.0),
        ],
    );
    assert_eq!(
        edits,
        vec![vec![Edit::Paint {
            triangle: 0,
            color: brush.color()
        }]]
    );
}

#[test]
fn a_dragged_vertex_snaps_onto_the_mirror_line() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let mirror = Mirror {
        a: Point::new(320.0, 0.0),
        b: Point::new(320.0, 600.0),
    };
    let mirrors = [mirror];
    editor.mirrors = &mirrors;
    let edits = drag(
        &editor,
        &[
            (MOVE, 300.0, 300.0),
            (PRESS, 300.0, 300.0),
            (MOVE, 315.0, 290.0),
            (RELEASE, 315.0, 290.0),
        ],
    );
    let [edits] = &edits[..] else {
        panic!("{edits:?}");
    };
    let [Edit::MoveVertex { id: 1, to }] = edits[..] else {
        panic!("{edits:?}");
    };
    assert!(
        (to.x - 320.0).abs() < 1e-3 && (to.y - 290.0).abs() < 1e-3,
        "{to:?}"
    );

    // Applied, the two halves share it.
    let mut doc = doc.clone();
    assert!(doc.apply(edits[0].clone()));
    assert!(doc.apply(Edit::Mirror { mirror }));
    assert_eq!(doc.vertex_count(), 5);
}

#[test]
fn the_drawing_is_kept_clear_of_its_mirror_image() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let mirrors = [Mirror {
        a: Point::new(320.0, 0.0),
        b: Point::new(320.0, 600.0),
    }];
    editor.mirrors = &mirrors;
    // Dragging corner 1 (300, 300) over the mirror line at x = 320.
    let edits = drag(
        &editor,
        &[
            (MOVE, 300.0, 300.0),
            (PRESS, 300.0, 300.0),
            (MOVE, 360.0, 300.0),
            (RELEASE, 360.0, 300.0),
        ],
    );
    for edit in edits.iter().flatten() {
        if let Edit::MoveVertex { to, .. } = edit {
            assert!(to.x <= 320.0, "{edits:?}");
        }
    }
}

/// Places a mirror (instead of `mirrors`) by dragging `from`
/// → `to`, holding `modifiers`; the one placed, if any.
fn place_mirror(
    doc: &Document,
    mirrors: &[Mirror],
    modifiers: keyboard::Modifiers,
    from: (f32, f32),
    to: (f32, f32),
) -> Option<Mirror> {
    let cache = Caches::default();
    let mut editor = editor(doc, &cache);
    editor.tool = Tool::Mirror;
    editor.mirrors = mirrors;
    let mut state = State::default();
    let held = Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers));
    let mut published = Vec::new();
    for (event, (x, y)) in [
        (held, from),
        (Event::Mouse(PRESS), from),
        (Event::Mouse(MOVE), to),
        (Event::Mouse(RELEASE), to),
    ] {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
        for event in [event, frame] {
            if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                published.extend(action.into_inner().0);
            }
        }
    }
    published.into_iter().find_map(|m| match m {
        Message::SetMirror(mirror) => Some(mirror),
        _ => None,
    })
}
