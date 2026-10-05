//! Extruding (E).

use super::*;

#[test]
fn e_extrudes_the_outer_edges_facing_the_cursor() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State {
        selection: vec![0, 1, 2],
        ..State::default()
    };

    // Down: only the bottom edge faces that way.
    run(&editor, &mut state, key('e'), 200.0, 250.0);
    assert!(run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 350.0).is_empty());
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 350.0);
    assert_eq!(extruded(&edits), 2);
    // The new bottom edge is selected, to go on from.
    let mut after = doc.clone();
    assert!(after.apply_all(&edits[0]));
    let mut at: Vec<Point> = state.selection.iter().map(|&v| after.vertex(v)).collect();
    at.sort_by(|p, q| p.x.total_cmp(&q.x));
    assert_eq!(at, [Point::new(100.0, 400.0), Point::new(300.0, 400.0)]);

    // Up: both slanted edges, joined at the swept apex.
    let mut state = State {
        selection: vec![0, 1, 2],
        ..State::default()
    };
    run(&editor, &mut state, key('e'), 200.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 150.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 150.0);
    assert_eq!(extruded(&edits), 4);
    assert_eq!(state.selection.len(), 3);
}

#[test]
fn an_extrusion_leaves_out_edges_running_into_something() {
    let mut doc = one();
    // In the way of the left edge swept up, not of the right.
    triangle(&mut doc, [(105.0, 215.0), (130.0, 215.0), (118.0, 195.0)]);
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State {
        selection: vec![0, 1, 2],
        ..State::default()
    };

    run(&editor, &mut state, key('e'), 200.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 150.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 150.0);
    assert_eq!(extruded(&edits), 2);
}

#[test]
fn an_extrusion_nearly_along_an_edge_leaves_no_hole_there() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State {
        selection: vec![0, 1, 2],
        ..State::default()
    };

    // Right and a little down: the bottom edge swept into a sliver 3 px
    // high, kept, as well as the right edge.
    run(&editor, &mut state, key('e'), 200.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 300.0, 253.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 253.0);
    assert_eq!(extruded(&edits), 4);
}

#[test]
fn escape_cancels_an_extrusion() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State {
        selection: vec![0, 1, 2],
        ..State::default()
    };

    run(&editor, &mut state, key('e'), 200.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 350.0);
    assert!(run(&editor, &mut state, escape(), 200.0, 350.0).is_empty());
    assert!(run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 350.0).is_empty());
    assert_eq!(state.selection.len(), 3);
}

#[test]
fn e_over_an_outer_edge_extrudes_it() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();

    // Over the bottom edge (0 1), nothing selected.
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 300.0);
    run(&editor, &mut state, key('e'), 200.0, 300.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 380.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 380.0);
    assert_eq!(extruded(&edits), 2);
    // The new edge selected, to go on from.
    assert_eq!(state.selection.len(), 2);
    assert!(!state.selection.contains(&0) && !state.selection.contains(&1));

    // Over the face, nothing.
    let mut state = State::default();
    run(&editor, &mut state, key('e'), 200.0, 250.0);
    assert!(matches!(state.interaction, Interaction::Idle));
}

#[test]
fn with_shift_an_extrusion_lines_up() {
    let extrude = |doc: &Document, at: (f32, f32), to: (f32, f32)| {
        let cache = Caches::default();
        let editor = editor(doc, &cache);
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), at.0, at.1);
        run(&editor, &mut state, key('e'), at.0, at.1);
        run(
            &editor,
            &mut state,
            hold(keyboard::Modifiers::SHIFT),
            at.0,
            at.1,
        );
        run(&editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
        let lit = editor.lit(&state, state.pending.as_ref().expect("lined up"));
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), to.0, to.1);
        (extruded_corners(&edits), lit)
    };
    let has = |corners: &[Point], x: f32, y: f32| {
        corners.iter().any(|p| p.distance(Point::new(x, y)) < 1e-3)
    };

    // The bottom edge (100–300, 300) pulled down a little askew: straight
    // out, square to it.
    let doc = one();
    let (corners, lit) = extrude(&doc, (200.0, 300.0), (206.0, 380.0));
    assert!(
        has(&corners, 100.0, 380.0) && has(&corners, 300.0, 380.0),
        "{corners:?}"
    );
    assert!(
        lit.iter().any(|g| matches!(g, Guide::Normal { .. })),
        "{lit:?}"
    );

    // Near as far as it's long: a square.
    let (corners, _) = extrude(&doc, (200.0, 300.0), (203.0, 496.0));
    assert!(
        has(&corners, 100.0, 500.0) && has(&corners, 300.0, 500.0),
        "{corners:?}"
    );

    // The left one's right edge (1 2) pulled right and up: a far end level
    // with the top of the other one, 5 (300, 200).
    let doc = two_apart();
    let (corners, lit) = extrude(&doc, (175.0, 250.0), (205.0, 153.0));
    assert!(
        corners.iter().any(|p| (p.y - 200.0).abs() < 1e-3),
        "{corners:?}"
    );
    assert!(
        lit.iter()
            .any(|g| matches!(g, Guide::Level { upright: false, .. })),
        "{lit:?}"
    );
}

/// The triangles an extrusion adds.
fn extruded(edits: &[Vec<Edit>]) -> usize {
    match edits {
        [edits] => match &edits[..] {
            [Edit::Paste { piece }] => piece.triangles.len(),
            _ => panic!("not an extrusion: {edits:?}"),
        },
        _ => panic!("expected one edit: {edits:?}"),
    }
}

/// The corners of what an extrusion adds.
fn extruded_corners(edits: &[Vec<Edit>]) -> Vec<Point> {
    match edits {
        [edits] => match &edits[..] {
            [Edit::Paste { piece }] => piece.vertices.clone(),
            _ => panic!("not an extrusion: {edits:?}"),
        },
        _ => panic!("expected one edit: {edits:?}"),
    }
}
