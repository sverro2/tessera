//! Guides (holding Shift): lining dragged points up with what's around.

use super::*;

#[test]
fn with_shift_a_dragged_vertex_lines_up_with_the_edges_around_it() {
    // A square: A (100, 300), B (300, 300), C (300, 100), D (100, 100),
    // as triangles A B C and A C D.
    let mut doc = Document::default();
    triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (300.0, 100.0)]);
    triangle(&mut doc, [(100.0, 300.0), (300.0, 100.0), (100.0, 100.0)]);
    let d = (0..doc.vertex_slots())
        .find(|&v| doc.vertex(v) == Point::new(100.0, 100.0))
        .unwrap();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let drag_d = |to: (f32, f32), modifiers| {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), 100.0, 100.0);
        run(&editor, &mut state, Event::Mouse(PRESS), 100.0, 100.0);
        run(&editor, &mut state, hold(modifiers), 100.0, 100.0);
        run(&editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
        let lit = state
            .pending
            .as_ref()
            .map_or_else(Vec::new, |pending| editor.lit(&state, pending));
        let edits = run(&editor, &mut state, Event::Mouse(RELEASE), to.0, to.1);
        let [edits] = &edits[..] else {
            panic!("{edits:?}");
        };
        let [Edit::MoveVertex { id, to }] = edits[..] else {
            panic!("{edits:?}");
        };
        assert_eq!(id, d);
        (to, lit)
    };
    let has = |lit: &[Guide], kind: fn(&Guide) -> bool| lit.iter().any(kind);

    // Without Shift, where it's let go.
    let (to, lit) = drag_d((103.0, 40.0), keyboard::Modifiers::empty());
    assert_eq!(to, Point::new(103.0, 40.0));
    assert!(lit.is_empty());
    // With: edge A D square to A B (and so parallel to B C).
    let (to, lit) = drag_d((103.0, 40.0), keyboard::Modifiers::SHIFT);
    assert!(to.distance(Point::new(100.0, 40.0)) < 1e-3, "{to:?}");
    assert!(has(&lit, |g| matches!(g, Guide::Square { .. })), "{lit:?}");
    // Straight on from B C, past C, as long as it: at (300, -100).
    let (to, lit) = drag_d((303.0, -97.0), keyboard::Modifiers::SHIFT);
    assert!(to.distance(Point::new(300.0, -100.0)) < 1e-3, "{to:?}");
    assert!(
        has(&lit, |g| matches!(g, Guide::Straight { .. })),
        "{lit:?}"
    );
    // A right angle at D itself, between D A and D C: on the circle over
    // A C (centre (200, 200)).
    let (to, lit) = drag_d((64.0, 150.0), keyboard::Modifiers::SHIFT);
    let centre = Point::new(200.0, 200.0);
    assert!(
        (to.distance(centre) - 200.0 * 2f32.sqrt() / 2.0).abs() < 1e-2,
        "{to:?}"
    );
    assert!(has(&lit, |g| matches!(g, Guide::Corner { .. })), "{lit:?}");
    let (a, c) = (Point::new(100.0, 300.0), Point::new(300.0, 100.0));
    let dot = (a - to).x * (c - to).x + (a - to).y * (c - to).y;
    assert!(dot.abs() < 1.0, "{dot}");
}

#[test]
fn with_shift_a_new_edge_lines_up_level_in_the_view() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    // The view turned a little: level on screen is at an angle in the
    // world, along no edge.
    editor.camera = Camera {
        rotation: std::f32::consts::FRAC_PI_6,
        pan: Vector::new(200.0, -50.0),
        ..Camera::default()
    };
    let camera = editor.camera;
    // Corner 2 dragged to near level (on screen) with corner 0.
    let (s0, s2) = (
        camera.to_screen(doc.vertex(0)),
        camera.to_screen(doc.vertex(2)),
    );
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), s2.x, s2.y);
    run(&editor, &mut state, Event::Mouse(PRESS), s2.x, s2.y);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        s2.x,
        s2.y,
    );
    run(
        &editor,
        &mut state,
        Event::Mouse(MOVE),
        s2.x - 30.0,
        s0.y + 4.0,
    );
    let pending = state.pending.as_ref().expect("lined up");
    let (at, lit) = (pending.at, editor.lit(&state, pending));
    assert!(
        lit.iter().any(|guide| matches!(
            guide,
            Guide::Level {
                n: 0,
                upright: false
            }
        )),
        "{lit:?}"
    );
    let on_screen = camera.to_screen(at);
    assert!((on_screen.y - s0.y).abs() < 1e-2, "{on_screen:?} {s0:?}");
}

#[test]
fn with_shift_a_vertex_lines_up_in_the_middle_of_its_ends() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    // Corner 2 (200, 100), between 0 (100, 300) and 1 (300, 300),
    // dragged up and a little aside: back to as far from both.
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 100.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 100.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        200.0,
        100.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 205.0, 60.0);
    let pending = state.pending.as_ref().expect("moved");
    assert!(
        pending.at.distance(Point::new(200.0, 60.0)) < 1e-3,
        "{:?}",
        pending.at
    );
    let lit = editor.lit(&state, pending);
    assert!(lit.contains(&Guide::Middle { a: 0, b: 1 }), "{lit:?}");
}

#[test]
fn guides_go_with_the_drag_they_came_up_in() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 100.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 100.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        200.0,
        100.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 205.0, 60.0);
    assert!(!state.near_guides.is_empty());
    run(&editor, &mut state, Event::Mouse(RELEASE), 205.0, 60.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        205.0,
        60.0,
    );
    assert!(state.near_guides.is_empty() && state.guides.is_empty());
}

#[test]
fn with_shift_a_new_triangle_lines_up_too() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let create = |to: (f32, f32)| {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), 500.0, 400.0);
        run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
        run(
            &editor,
            &mut state,
            hold(keyboard::Modifiers::SHIFT),
            500.0,
            400.0,
        );
        run(&editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
        let Interaction::Creating { to, .. } = state.interaction else {
            panic!("{:?}", state.interaction);
        };
        (to, state)
    };

    // Level with where it started: its base level.
    let (to, state) = create((600.0, 404.0));
    assert_eq!(to, Point::new(600.0, 400.0));
    assert!(state.guides.contains(&Guide::Axis {
        at: Point::new(500.0, 400.0),
        upright: false
    }));
    // Upright under corner 1 (300, 300) of the triangle there.
    let (to, _) = create((303.0, 450.0));
    assert_eq!(to, Point::new(300.0, 450.0));
}

#[test]
fn shapes_apart_line_up_with_each_other() {
    // Two triangles apart: 0 1 2 on the left, 3 4 5 on the right.
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // The top of the left one, 2 (150, 200), dragged up and over: edge
    // 0 2 parallel to edge 3 5 of the other one.
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 150.0, 200.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 150.0, 200.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        150.0,
        200.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 172.0, 158.0);
    let pending = state.pending.as_ref().unwrap();
    let lit = editor.lit(&state, pending);
    assert!(
        lit.contains(&Guide::Parallel { n: 0, u: 3, v: 5 }),
        "{lit:?}"
    );
    let d = pending.at - doc.vertex(0);
    assert!(
        cross(d, doc.vertex(5) - doc.vertex(3)).abs() < 1e-2,
        "{d:?}"
    );

    // A new triangle's corner in line with edge 3 5, past its end.
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 175.0, 580.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 175.0, 580.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        175.0,
        580.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 178.0, 452.0);
    let lit = &state.guides;
    assert!(lit.contains(&Guide::Along { u: 3, v: 5 }), "{lit:?}");

    // A new triangle's base parallel to edge 3 5 (and as long, at the
    // point that's so).
    let start = Point::new(500.0, 500.0);
    let end = start + (doc.vertex(5) - doc.vertex(3));
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), start.x, start.y);
    run(&editor, &mut state, Event::Mouse(PRESS), start.x, start.y);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        start.x,
        start.y,
    );
    run(
        &editor,
        &mut state,
        Event::Mouse(MOVE),
        end.x + 4.0,
        end.y + 3.0,
    );
    let Interaction::Creating { to, .. } = state.interaction else {
        panic!("{:?}", state.interaction);
    };
    assert!(to.distance(end) < 1e-3, "{to:?}");
    let lit = &state.guides;
    // (Edge 0 2 of the other one runs alike: the guide is kept once.)
    assert!(
        lit.iter()
            .any(|g| matches!(g, Guide::ParallelFrom { at, .. } if *at == start)),
        "{lit:?}"
    );
}

#[test]
fn every_edge_a_guide_follows_offers_its_length() {
    // A (100, 300), B (400, 300), C (400, 100), and D (100, 60) above A:
    // edge A D is square to A B (300 long) and parallel to B C (200
    // long): one line, two lengths.
    let mut doc = Document::default();
    triangle(&mut doc, [(100.0, 300.0), (400.0, 300.0), (400.0, 100.0)]);
    triangle(&mut doc, [(100.0, 300.0), (400.0, 100.0), (100.0, 60.0)]);
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 100.0, 60.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 100.0, 60.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        100.0,
        60.0,
    );
    // Near as long as B C.
    run(&editor, &mut state, Event::Mouse(MOVE), 103.0, 103.0);
    let pending = state.pending.as_ref().unwrap();
    assert!(
        pending.at.distance(Point::new(100.0, 100.0)) < 1e-3,
        "{:?}",
        pending.at
    );
    let lit = editor.lit(&state, pending);
    assert!(
        lit.contains(&Guide::Parallel { n: 0, u: 1, v: 2 }),
        "{lit:?}"
    );
    // Faint, the line's shown once.
    let x100 = state
        .guides
        .iter()
        .filter(|g| matches!(g, Guide::Square { n: 0, .. } | Guide::Parallel { n: 0, .. }))
        .count();
    assert!(x100 <= 1, "{:?}", state.guides);
}

#[test]
fn guides_that_would_flatten_the_shape_are_left_out() {
    // The square A B C D again; D (100, 100) dragged with Shift to just
    // off the line through A and C, past A: there triangle A C D would
    // be flat.
    let mut doc = Document::default();
    triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (300.0, 100.0)]);
    triangle(&mut doc, [(100.0, 300.0), (300.0, 100.0), (100.0, 100.0)]);
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 100.0, 100.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 100.0, 100.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        100.0,
        100.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 60.0, 343.0);

    let on_diagonal = |guide: &Guide| match guide.shape(&doc, Camera::default()) {
        Shape::Line(o, d) => {
            let length = d.x.hypot(d.y);
            (o.x + o.y - 400.0).abs() < 1e-3 && (d.x + d.y).abs() < 1e-3 * length
        }
        Shape::Circle(..) => false,
    };
    assert!(!state.guides.iter().any(on_diagonal), "{:?}", state.guides);
    let lit = state
        .pending
        .as_ref()
        .map_or_else(Vec::new, |pending| editor.lit(&state, pending));
    assert!(!lit.iter().any(on_diagonal), "{lit:?}");
}

#[test]
fn a_new_triangles_third_corner_snaps_onto_a_vertex_in_reach() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // Its first side from (500, 400) to (500, 250); its third corner put
    // down a few px off corner 1 (300, 300): welded on.
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 500.0, 250.0);
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 500.0, 250.0).is_empty());
    run(&editor, &mut state, Event::Mouse(MOVE), 304.0, 303.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 304.0, 303.0);
    let mut after = doc.clone();
    assert!(after.apply_all(&edits[0]));
    assert_eq!(after.vertex_count(), 5);
    assert!(after.unique_vertices().contains(&1));
}

#[test]
fn with_shift_a_new_triangle_lines_up_by_each_corner() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        500.0,
        400.0,
    );

    // Its first side, drawn to (603, 404): level with where it started.
    run(&editor, &mut state, Event::Mouse(MOVE), 603.0, 404.0);
    let Interaction::Creating { to, .. } = state.interaction else {
        panic!("{:?}", state.interaction);
    };
    assert!((to.y - 400.0).abs() < 1e-3, "{to:?}");
    assert!(
        state
            .guides
            .iter()
            .any(|guide| matches!(guide, Guide::Axis { upright: false, .. }))
    );
    run(&editor, &mut state, Event::Mouse(RELEASE), 603.0, 404.0);
    let Interaction::Completing { b, .. } = state.interaction else {
        panic!("{:?}", state.interaction);
    };

    // Its third corner at (497, 300): upright over where it started.
    run(&editor, &mut state, Event::Mouse(MOVE), 497.0, 330.0);
    let Interaction::Completing { apex, .. } = state.interaction else {
        panic!("{:?}", state.interaction);
    };
    assert!((apex.x - 500.0).abs() < 1e-3, "{apex:?}");

    // Near where its sides would be as long: there.
    let even = Point::new(
        (500.0 + b.x) / 2.0,
        400.0 - (b.x - 500.0) * 3f32.sqrt() / 2.0,
    );
    run(
        &editor,
        &mut state,
        Event::Mouse(MOVE),
        even.x + 4.0,
        even.y + 3.0,
    );
    let Interaction::Completing { apex, .. } = state.interaction else {
        panic!("{:?}", state.interaction);
    };
    assert!(apex.distance(even) < 1e-2, "{apex:?} {even:?}");
}

#[test]
fn a_new_triangle_starts_lined_up_too() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    // Held from before pressing.
    let start = |held: keyboard::Modifiers, at: (f32, f32), to: (f32, f32)| {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), at.0, at.1);
        run(&editor, &mut state, hold(held), at.0, at.1);
        run(&editor, &mut state, Event::Mouse(PRESS), at.0, at.1);
        run(&editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
        let Interaction::Creating { from, to } = state.interaction else {
            panic!("{:?}", state.interaction)
        };
        (from, to, state)
    };

    // Ctrl: it starts on the grid, as well as ending on it (not a lasso).
    let (from, to, mut state) = start(keyboard::Modifiers::CTRL, (503.0, 397.0), (533.0, 417.0));
    assert_eq!(
        (from, to),
        (Point::new(500.0, 400.0), Point::new(530.0, 420.0))
    );
    // Let go: it stays where it started.
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        533.0,
        417.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 534.0, 417.0);
    let Interaction::Creating { from, .. } = state.interaction else {
        panic!()
    };
    assert_eq!(from, Point::new(500.0, 400.0));

    // Shift: pressed a little below the level of the bottom corners
    // (y 300), it starts level with them.
    let (from, ..) = start(keyboard::Modifiers::SHIFT, (503.0, 304.0), (600.0, 350.0));
    assert_eq!(from, Point::new(503.0, 300.0));
}

#[test]
fn guides_come_from_the_geometry_nearest_only() {
    // Fifteen triangles in a row, 100 apart: 45 outline edges.
    let mut doc = Document::default();
    for i in 0..15 {
        let x = i as f32 * 100.0;
        triangle(&mut doc, [(x, 300.0), (x + 50.0, 300.0), (x + 25.0, 250.0)]);
    }
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let near = [Point::new(-50.0, 300.0)];

    let outlines = editor.nearest_outlines(editor.outlines(|_| false), &near);
    assert_eq!(outlines.len(), NEAREST);
    // Those of the first four triangles (x < 400), not further along.
    assert!(
        outlines
            .iter()
            .all(|&(u, v)| { doc.vertex(u).x < 400.0 && doc.vertex(v).x < 400.0 })
    );
    let vertices = editor.nearest_vertices(&near);
    assert_eq!(vertices.len(), NEAREST);
    assert!(vertices.iter().all(|&v| doc.vertex(v).x < 400.0));
}

#[test]
fn with_shift_a_new_triangle_shows_which_sides_are_as_long() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(MOVE), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 400.0);
    run(&editor, &mut state, Event::Mouse(RELEASE), 600.0, 400.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        600.0,
        400.0,
    );
    let (a, b) = (Point::new(500.0, 400.0), Point::new(600.0, 400.0));
    let place = |state: &mut State, x: f32, y: f32| {
        run(&editor, state, Event::Mouse(MOVE), x, y);
        let Interaction::Completing { apex, .. } = state.interaction else {
            panic!("{:?}", state.interaction);
        };
        let lit = state
            .pending
            .as_ref()
            .map_or_else(Vec::new, |pending| editor.lit(state, pending));
        (apex, lit)
    };

    // Near where all three are as long (nearer still where one is as long
    // as the outline is steep): there, and all three say so.
    let even = Point::new(550.0, 400.0 - 50.0 * 3f32.sqrt());
    let (apex, lit) = place(&mut state, even.x + 4.0, even.y - 3.0);
    assert!(apex.distance(even) < 1e-2, "{apex:?}");
    for guide in [
        Guide::Midway { a, b },
        Guide::AsLong { at: a, a, b },
        Guide::AsLong { at: b, a, b },
    ] {
        assert!(lit.contains(&guide), "{guide:?} not in {lit:?}");
    }

    // Off to the right: the side from the first side's end as long as it.
    let (apex, lit) = place(&mut state, 690.0, 352.0);
    assert!((apex.distance(b) - 100.0).abs() < 1e-2, "{apex:?}");
    assert_eq!(lit, [Guide::AsLong { at: b, a, b }]);
}
