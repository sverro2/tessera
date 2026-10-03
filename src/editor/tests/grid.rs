//! The grid (holding Ctrl): dragged points landing on it.

use super::*;

#[test]
fn with_ctrl_a_dragged_point_lands_on_the_grid() {
    let ctrl = keyboard::Modifiers::CTRL;
    let drag_to = |editor: &Editor, at: (f32, f32), to: (f32, f32), held| {
        let mut state = State::default();
        run(editor, &mut state, Event::Mouse(MOVE), at.0, at.1);
        run(editor, &mut state, Event::Mouse(PRESS), at.0, at.1);
        run(editor, &mut state, hold(held), at.0, at.1);
        run(editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
        state.interaction
    };

    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    // A new triangle's corner.
    let Interaction::Creating { to, .. } = drag_to(&editor, (500.0, 400.0), (533.0, 417.0), ctrl)
    else {
        panic!()
    };
    assert_eq!(to, Point::new(530.0, 420.0));
    // A corner moved.
    let Interaction::MovingVertex { to, .. } =
        drag_to(&editor, (200.0, 100.0), (213.0, 86.0), ctrl)
    else {
        panic!()
    };
    assert_eq!(to, Point::new(210.0, 90.0));
    // With Shift too: a grid point on a guide (upright from where it
    // started) rather than the nearest.
    let both = ctrl | keyboard::Modifiers::SHIFT;
    let Interaction::Creating { to, .. } = drag_to(&editor, (500.0, 400.0), (506.0, 470.0), both)
    else {
        panic!()
    };
    assert_eq!(to, Point::new(500.0, 470.0));
}

#[test]
fn with_ctrl_a_grabbed_selection_puts_its_nearest_vertex_on_the_grid() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Corner 5 (300, 200), nearest the cursor, moved to (313, 204): onto
    // (310, 200).
    run(&editor, &mut state, key('g'), 300.0, 250.0);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::CTRL),
        300.0,
        250.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 313.0, 254.0);
    // The grid's shown around the vertices as they land, not the cursor.
    let by = Vector::new(10.0, 0.0);
    let placed = editor.placed(&state, state.pending.as_ref().unwrap());
    assert_eq!(placed, [3, 4, 5].map(|v| doc.vertex(v) + by));
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 313.0, 254.0);
    assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
}

#[test]
fn zoomed_far_out_ctrl_lands_on_the_points_shown() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    // At a quarter, points 10 apart would be 2.5 px apart: every tenth.
    editor.camera.zoom = 0.25;
    let at = |x: f32, y: f32| editor.camera.to_screen(Point::new(x, y));
    let mut state = State::default();
    let (from, to) = (at(1000.0, 1000.0), at(1130.0, 1040.0));
    run(&editor, &mut state, Event::Mouse(MOVE), from.x, from.y);
    run(&editor, &mut state, Event::Mouse(PRESS), from.x, from.y);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::CTRL),
        from.x,
        from.y,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), to.x, to.y);
    let Interaction::Creating { to, .. } = state.interaction else {
        panic!()
    };
    assert!(to.distance(Point::new(1100.0, 1000.0)) < 1e-2, "{to:?}");
}
