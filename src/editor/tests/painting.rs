//! The paint mode: painting faces and edges, picking, and tweaking the brush.

use super::*;

#[test]
fn painting_paints_the_faces_dragged_over_and_nothing_else() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let brush = Brush::default();
    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush,
        width: 1.0,
        picking: false,
    };
    let color = brush.color();

    // From inside the first, over the gap (fast), into the second.
    let edits = drag(
        &editor,
        &[
            (MOVE, 150.0, 280.0),
            (PRESS, 150.0, 280.0),
            (MOVE, 300.0, 280.0),
            (RELEASE, 300.0, 280.0),
        ],
    );
    assert_eq!(
        edits,
        vec![vec![
            Edit::Paint { triangle: 0, color },
            Edit::Paint { triangle: 1, color },
        ]]
    );

    // Dragging a corner or an edge doesn't change the shape.
    let edits = drag(
        &editor,
        &[
            (MOVE, 100.0, 300.0),
            (PRESS, 100.0, 300.0),
            (MOVE, 50.0, 350.0),
            (RELEASE, 50.0, 350.0),
        ],
    );
    assert_eq!(edits, vec![vec![Edit::Paint { triangle: 0, color }]]);
}

#[test]
fn painting_edges_paints_only_edges_and_faces_only_faces() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let brush = Brush::default();
    let style = EdgeStyle {
        color: brush.color().unwrap(),
        width: 2.0,
    };
    // From the bottom edge, through the face, over the right edge.
    let stroke = [
        (MOVE, 200.0, 300.0),
        (PRESS, 200.0, 300.0),
        (MOVE, 200.0, 250.0),
        (MOVE, 280.0, 250.0),
        (RELEASE, 280.0, 250.0),
    ];

    editor.tool = Tool::Paint {
        target: paint::Target::Edges,
        brush,
        width: 2.0,
        picking: false,
    };
    let edits = drag(&editor, &stroke);
    let edge = |a, b| Edit::PaintEdge {
        a,
        b,
        style: Some(style),
    };
    assert_eq!(edits, vec![vec![edge(0, 1), edge(1, 2)]]);

    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush,
        width: 2.0,
        picking: false,
    };
    let edits = drag(&editor, &stroke);
    assert_eq!(
        edits,
        vec![vec![Edit::Paint {
            triangle: 0,
            color: brush.color()
        }]]
    );
}

#[test]
fn ctrl_click_picks_instead_of_painting() {
    let mut doc = two_apart();
    let red = Color::from_rgb8(255, 0, 0);
    assert!(doc.apply(Edit::Paint {
        triangle: 1,
        color: Some(red),
    }));
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush: Brush::default(),
        width: 1.0,
        picking: false,
    };
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let click = |state: &mut State, x: f32, y: f32| {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        let mut out = vec![];
        for e in [PRESS, RELEASE] {
            if let Some(action) = editor.update(state, &Event::Mouse(e), bounds, cursor) {
                out.extend(action.into_inner().0);
            }
        }
        out
    };

    let mut state = State::default();
    let _ = editor.update(
        &mut state,
        &Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::CTRL)),
        bounds,
        mouse::Cursor::Unavailable,
    );
    // The face painted red, one as made (the default), and nothing.
    let picked = |messages: Vec<Message>| match &messages[..] {
        [Message::Pick(picked)] => Some(*picked),
        [] => None,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        picked(click(&mut state, 300.0, 280.0)),
        Some(Picked::Face(Some(red)))
    );
    assert_eq!(
        picked(click(&mut state, 150.0, 280.0)),
        Some(Picked::Face(Some(crate::document::DEFAULT_FACE)))
    );
    assert_eq!(picked(click(&mut state, 500.0, 500.0)), None);

    // Without Ctrl it paints again.
    let mut state = State::default();
    assert!(matches!(
        &click(&mut state, 150.0, 280.0)[..],
        [Message::Edit { .. }]
    ));
}

#[test]
fn w_and_moving_tweaks_the_edge_width() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let run = |editor: &Editor, state: &mut State, event: Event, x: f32| {
        let cursor = mouse::Cursor::Available(Point::new(x, 300.0));
        editor
            .update(state, &event, bounds, cursor)
            .map(|action| action.into_inner().0)
    };

    for (target, tweaks) in [(paint::Target::Edges, true), (paint::Target::Faces, false)] {
        editor.tool = Tool::Paint {
            target,
            brush: Brush::default(),
            width: 2.0,
            picking: false,
        };
        let mut state = State::default();
        let _ = run(&editor, &mut state, key('w'), 200.0);
        let moved = run(&editor, &mut state, Event::Mouse(MOVE), 230.0).flatten();
        match moved {
            Some(Message::TweakWidth(across)) => {
                assert!(tweaks);
                assert_eq!(across, 30.0);
                // The edge previewed is still looked for where W was
                // pressed.
                assert_eq!(state.tweaking.unwrap().at, Point::new(200.0, 300.0));
            }
            _ => assert!(!tweaks, "{target:?}: {moved:?}"),
        }
        // Let go: back to following the mouse.
        let w = letter('w');
        let release = Event::Keyboard(keyboard::Event::KeyReleased {
            key: w.clone(),
            modified_key: w,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyW),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
        });
        let _ = run(&editor, &mut state, release, 230.0);
        assert!(state.tweaking.is_none());
    }
}

#[test]
fn tweaking_an_edge_shows_the_layer_with_just_its_new_width() {
    // The bottom edge (0 1) painted wide; W over it, the brush thin.
    let mut doc = one();
    let wide = EdgeStyle {
        color: Color::from_rgb(1.0, 0.0, 0.0),
        width: 12.0,
    };
    assert!(doc.apply(Edit::PaintEdge {
        a: 0,
        b: 1,
        style: Some(wide)
    }));
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let brush = Brush::default();
    editor.tool = Tool::Paint {
        target: paint::Target::Edges,
        brush,
        width: 2.0,
        picking: false,
    };
    let mut state = State::default();
    assert!(editor.tweaked_edge(&state).is_none());
    run(&editor, &mut state, key('w'), 200.0, 300.0);
    let tweaked = editor.tweaked_edge(&state).expect("tweaking");
    let style = tweaked.edge_style(0, 1).unwrap();
    assert_eq!(style.width, 2.0);
    assert_eq!(Some(style.color), brush.color());
    // The layer itself is left be.
    assert_eq!(doc.edge_style(0, 1), Some(wide));
    // Another drawing to what's derived from it (the edges' outlines):
    // drawn as it is, not as the layer is.
    assert_ne!(tweaked.revision(), doc.revision());
}

#[test]
fn c_and_moving_tweaks_the_lightness() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let run = |editor: &Editor, state: &mut State, event: Event, x: f32| {
        let cursor = mouse::Cursor::Available(Point::new(x, 250.0));
        editor
            .update(state, &event, bounds, cursor)
            .and_then(|action| action.into_inner().0)
    };
    let c = letter('c');
    let physical = keyboard::key::Physical::Code(keyboard::key::Code::KeyC);
    let press = key('c');
    let release = Event::Keyboard(keyboard::Event::KeyReleased {
        key: c.clone(),
        modified_key: c,
        physical_key: physical,
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
    });

    for (brush, tweaks) in [(Brush::default(), true), (Brush::Eraser, false)] {
        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush,
            width: 2.0,
            picking: false,
        };
        let mut state = State::default();
        run(&editor, &mut state, press.clone(), 200.0);
        let moved = run(&editor, &mut state, Event::Mouse(MOVE), 180.0);
        match moved {
            Some(Message::TweakLightness(across)) => {
                assert!(tweaks);
                assert_eq!(across, -20.0);
                // The face previewed stays the one C was pressed over.
                assert_eq!(state.tweaking.unwrap().at, Point::new(200.0, 250.0));
            }
            _ => assert!(!tweaks, "{brush:?}: {moved:?}"),
        }
        run(&editor, &mut state, release.clone(), 180.0);
        assert!(state.tweaking.is_none());
    }
}

#[test]
fn a_click_while_adjusting_paints_what_is_previewed() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let run = |editor: &Editor, state: &mut State, event: Event, x: f32, y: f32| {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        editor
            .update(state, &event, bounds, cursor)
            .and_then(|action| action.into_inner().0)
    };
    let press_c = key('c');
    let brush = Brush::default();

    // W over the bottom edge; the mouse wanders off; a click there
    // paints the bottom edge, with the width set.
    editor.tool = Tool::Paint {
        target: paint::Target::Edges,
        brush,
        width: 5.0,
        picking: false,
    };
    let mut state = State::default();
    run(&editor, &mut state, key('w'), 200.0, 300.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 450.0);
    let clicked = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 450.0);
    let style = Some(EdgeStyle {
        color: brush.color().unwrap(),
        width: 5.0,
    });
    assert!(
        matches!(&clicked, Some(Message::Edit { edits, .. })
            if edits[..] == [Edit::PaintEdge { a: 0, b: 1, style }]),
        "{clicked:?}"
    );
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 200.0, 450.0).is_none());

    // C over the face; off the drawing; a click paints the face.
    editor.tool = Tool::Paint {
        target: paint::Target::Faces,
        brush,
        width: 5.0,
        picking: false,
    };
    let mut state = State::default();
    run(&editor, &mut state, press_c, 200.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 500.0);
    let clicked = run(&editor, &mut state, Event::Mouse(PRESS), 600.0, 500.0);
    assert!(
        matches!(&clicked, Some(Message::Edit { edits, .. })
            if edits[..] == [Edit::Paint { triangle: 0, color: brush.color() }]),
        "{clicked:?}"
    );
}

#[test]
fn an_edge_is_dashed_on_its_own() {
    let dashes = super::painter::dashes(Point::new(0.0, 0.0), Point::new(20.0, 0.0), 4.0, 4.0);
    let x = |p: Point| (p.x, p.y);
    let dashes: Vec<_> = dashes.into_iter().map(|(a, b)| (x(a), x(b))).collect();
    // From its start, the last cut short at its end; nothing past it.
    assert_eq!(
        dashes,
        [
            ((0.0, 0.0), (4.0, 0.0)),
            ((8.0, 0.0), (12.0, 0.0)),
            ((16.0, 0.0), (20.0, 0.0)),
        ]
    );
    let point = Point::new(3.0, 3.0);
    assert!(super::painter::dashes(point, point, 4.0, 4.0).is_empty());
}
