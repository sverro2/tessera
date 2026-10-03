//! The selection: lassoing, selecting shapes, grabbing, rotating, scaling, deleting, copying and pasting.

use super::*;

#[test]
fn a_lassoed_selection_is_dragged_together() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 340.0, 200.0);
    let edits = run(&editor, &mut state, Event::Mouse(RELEASE), 340.0, 200.0);
    let by = Vector::new(40.0, -50.0);
    assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
    // Still selected, to move on.
    assert_eq!(state.selection.len(), 3);

    // A click lets go of it.
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 500.0);
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 500.0, 500.0).is_empty());
    assert!(state.selection.is_empty());
}

#[test]
fn g_grabs_the_selection_until_a_click() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    run(&editor, &mut state, key('g'), 300.0, 250.0);
    assert!(run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0).is_empty());
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 310.0, 270.0);
    let by = Vector::new(10.0, 20.0);
    assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 310.0, 270.0).is_empty());
    // Still selected, to go on with.
    assert_eq!(state.selection.len(), 3);
}

#[test]
fn a_moved_selection_snaps_to_join_up() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Corner 3 (250, 300) to within a few px of corner 1 (200, 300):
    // onto it.
    run(&editor, &mut state, key('g'), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 254.0, 253.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 254.0, 253.0);
    let by = Vector::new(-50.0, 0.0);
    assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
}

#[test]
fn r_rotates_the_selection_around_its_centre() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Its centre is (300, 266.67); a quarter turn round it.
    let c = Point::new(300.0, 800.0 / 3.0);
    run(&editor, &mut state, key('r'), c.x + 50.0, c.y);
    run(&editor, &mut state, Event::Mouse(MOVE), c.x, c.y + 50.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), c.x, c.y + 50.0);
    for (v, to) in moves(&edits) {
        let d = doc.vertex(v) - c;
        let expected = c + Vector::new(-d.y, d.x);
        assert!(to.distance(expected) < 1e-3, "{v}: {to:?} != {expected:?}");
    }
    assert_eq!(state.selection.len(), 3);
}

#[test]
fn t_scales_the_selection_around_its_centre() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Twice as far from its centre (300, 266.67): twice the size.
    let c = Point::new(300.0, 800.0 / 3.0);
    run(&editor, &mut state, key('t'), c.x + 20.0, c.y);
    run(&editor, &mut state, Event::Mouse(MOVE), c.x, c.y - 40.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), c.x, c.y - 40.0);
    for (v, to) in moves(&edits) {
        let expected = c + (doc.vertex(v) - c) * 2.0;
        assert!(to.distance(expected) < 1e-3, "{v}: {to:?} != {expected:?}");
    }
    assert_eq!(state.selection.len(), 3);
}

#[test]
fn ctrl_a_selects_every_vertex_of_the_layer() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, ctrl('a'), 600.0, 500.0);
    let mut selected = state.selection.clone();
    selected.sort_unstable();
    assert_eq!(selected, doc.unique_vertices());
}

#[test]
fn ctrl_l_selects_the_whole_shape() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    let selected = |state: &State| {
        let mut selected = state.selection.clone();
        selected.sort_unstable();
        selected
    };

    // Over the left triangle: its vertices.
    run(&editor, &mut state, ctrl('l'), 150.0, 280.0);
    assert_eq!(selected(&state), [0, 1, 2]);

    // Off the drawing, from one selected vertex: its shape.
    state.selection = vec![4];
    run(&editor, &mut state, ctrl('l'), 600.0, 500.0);
    assert_eq!(selected(&state), [3, 4, 5]);
}

#[test]
fn shift_click_and_another_lasso_add_to_the_selection() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Shift+click adds vertex 0, then removes vertex 3; missing a vertex
    // leaves the selection be.
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        0.0,
        0.0,
    );
    for (x, y) in [(100.0, 300.0), (250.0, 300.0), (500.0, 500.0)] {
        run(&editor, &mut state, Event::Mouse(PRESS), x, y);
        run(&editor, &mut state, Event::Mouse(RELEASE), x, y);
    }
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        0.0,
        0.0,
    );
    let mut selected = state.selection.clone();
    selected.sort_unstable();
    assert_eq!(selected, [0, 4, 5]);

    // Lassoing the right triangle again adds 3 back.
    lasso_right(&editor, &mut state);
    assert_eq!(state.selection.len(), 4);
}

#[test]
fn d_removes_the_selected_vertices() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();

    // Corner 0 of the left triangle and 4 of the right one: both go,
    // whatever's under the cursor.
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::SHIFT),
        0.0,
        0.0,
    );
    for (x, y) in [(100.0, 300.0), (350.0, 300.0)] {
        run(&editor, &mut state, Event::Mouse(PRESS), x, y);
        run(&editor, &mut state, Event::Mouse(RELEASE), x, y);
    }
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        0.0,
        0.0,
    );
    let edits = run(&editor, &mut state, key('d'), 600.0, 500.0);
    assert_eq!(
        edits,
        vec![vec![
            Edit::RemoveTriangle { triangle: 1 },
            Edit::RemoveTriangle { triangle: 0 },
        ]]
    );
    let mut after = doc.clone();
    assert!(after.apply_all(&edits[0]));
    assert!(after.is_empty());
}

#[test]
fn copied_faces_are_pasted_where_they_fit() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    lasso_right(&editor, &mut state);
    let cursor = mouse::Cursor::Available(Point::new(300.0, 250.0));
    let copied = editor
        .update(&mut state, &ctrl('c'), bounds, cursor)
        .and_then(|action| action.into_inner().0);
    let Some(Message::Copy(piece)) = copied else {
        panic!("{copied:?}");
    };
    assert_eq!(piece.triangles.len(), 1);

    editor.clipboard = Some(&piece);
    let mut state = State::default();
    // Over the original (in its middle): it doesn't fit, a click does
    // nothing.
    run(&editor, &mut state, ctrl('v'), 300.0, 250.0);
    assert!(matches!(state.interaction, Interaction::Pasting { .. }));
    assert!(run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 250.0).is_empty());
    assert!(matches!(state.interaction, Interaction::Pasting { .. }));

    // Below it, clear: put down there, and selected.
    run(&editor, &mut state, Event::Mouse(MOVE), 300.0, 450.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 450.0);
    assert_eq!(
        edits,
        vec![vec![Edit::Paste {
            piece: piece.moved(Vector::new(0.0, 200.0))
        }]]
    );
    assert!(matches!(state.interaction, Interaction::Idle));
    assert_eq!(state.selection.len(), 3);
    let mut after = doc.clone();
    assert!(after.apply_all(&edits[0]));
    assert_eq!(after.triangle_ids().len(), 3);
}

#[test]
fn a_paste_snaps_to_join_up_and_a_cut_takes_the_faces_away() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    lasso_right(&editor, &mut state);
    let cursor = mouse::Cursor::Available(Point::new(600.0, 500.0));
    let cut = editor
        .update(&mut state, &ctrl('x'), bounds, cursor)
        .and_then(|action| action.into_inner().0);
    let Some(Message::Cut { piece, edits, .. }) = cut else {
        panic!("{cut:?}");
    };
    assert_eq!(piece.triangles.len(), 1);
    assert_eq!(edits, [Edit::RemoveTriangle { triangle: 1 }]);

    // The right triangle (corners 250..350 along y = 300), pasted so its
    // left corner comes within a few px of corner 1 (200, 300) of the
    // left one: snapped onto it, sharing it.
    let mut after = doc.clone();
    assert!(after.apply_all(&edits));
    let mut editor = super::tests::editor(&after, &cache);
    editor.clipboard = Some(&piece);
    let mut state = State::default();
    run(&editor, &mut state, ctrl('v'), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 254.0, 253.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 254.0, 253.0);
    assert_eq!(
        edits,
        vec![vec![Edit::Paste {
            piece: piece.moved(Vector::new(-50.0, 0.0))
        }]]
    );
}

#[test]
fn escape_cancels_a_grab_and_then_the_selection() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    run(&editor, &mut state, key('g'), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
    run(&editor, &mut state, escape(), 310.0, 270.0);
    assert!(matches!(state.interaction, Interaction::Idle));
    assert!(state.pending.is_none());
    assert_eq!(state.selection.len(), 3);
    run(&editor, &mut state, escape(), 310.0, 270.0);
    assert!(state.selection.is_empty());
}

#[test]
fn q_then_a_drag_draws_a_lasso_and_esc_lets_go_of_it() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);
    assert!(!state.lasso_armed);

    // Made ready, then let go of: a drag creates again.
    let mut state = State::default();
    run(&editor, &mut state, key('q'), 500.0, 400.0);
    assert!(state.lasso_armed);
    run(&editor, &mut state, escape(), 500.0, 400.0);
    assert!(!state.lasso_armed);
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    assert!(matches!(state.interaction, Interaction::Creating { .. }));
}

#[test]
fn shift_q_lassos_vertices_out_of_the_selection() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    // Around the top of the right one, 5 (300, 200).
    run(
        &editor,
        &mut state,
        press(letter('Q'), keyboard::Modifiers::SHIFT),
        280.0,
        180.0,
    );
    assert!(state.lasso_armed && state.lasso_removes);
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        280.0,
        180.0,
    );
    run(&editor, &mut state, Event::Mouse(PRESS), 280.0, 180.0);
    for (x, y) in [(320.0, 180.0), (320.0, 220.0), (280.0, 220.0)] {
        run(&editor, &mut state, Event::Mouse(MOVE), x, y);
    }
    run(&editor, &mut state, Event::Mouse(RELEASE), 280.0, 220.0);
    let mut selected = state.selection.clone();
    selected.sort_unstable();
    assert_eq!(selected, [3, 4]);

    // Q again adds.
    run(&editor, &mut state, key('q'), 280.0, 180.0);
    assert!(state.lasso_armed && !state.lasso_removes);
}
