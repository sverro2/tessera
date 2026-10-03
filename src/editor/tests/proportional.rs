//! Proportional editing: what's near a moved vertex coming along.

use super::*;

#[test]
fn proportional_editing_takes_vertices_nearby_along() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.proportional = Some(100.0);
    let mut state = State::default();
    lasso_right(&editor, &mut state);

    run(&editor, &mut state, key('g'), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 310.0, 270.0);
    let by = Vector::new(10.0, 20.0);
    // Vertex 1 is halfway within reach of 3: half as far (smoothly, so
    // just that); 0 and 2 are out of reach.
    let mut expected = vec![(1, doc.vertex(1) + by * 0.5)];
    expected.extend([3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
    assert_eq!(moves(&edits), expected);
}

#[test]
fn proportional_editing_reaches_as_far_on_screen_at_any_zoom() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.proportional = Some(100.0);
    // Zoomed out to half: 100 px reach 200 world units, from vertex 3 at
    // (250, 300) as far as vertex 0 at (100, 300).
    editor.camera.zoom = 0.5;
    let weights = editor.weights(&[3]);
    let weight = |v| weights.iter().find(|&&(u, _)| u == v).map(|&(_, w)| w);
    assert_eq!(weight(3), Some(1.0));
    assert!(weight(0).is_some_and(|w| w > 0.0 && w < 1.0));
    editor.camera.zoom = 1.0;
    let weights = editor.weights(&[3]);
    assert!(!weights.iter().any(|&(v, _)| v == 0));
}

#[test]
fn dragging_a_vertex_takes_those_nearby_along_proportionally() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.proportional = Some(100.0);

    // Vertex 3 at (250, 300); vertex 1, 50 away, goes half as far.
    let edits = drag(
        &editor,
        &[
            (MOVE, 250.0, 300.0),
            (PRESS, 250.0, 300.0),
            (MOVE, 260.0, 320.0),
            (RELEASE, 260.0, 320.0),
        ],
    );
    let by = Vector::new(10.0, 20.0);
    let moved = moves(&edits);
    assert!(moved.contains(&(3, doc.vertex(3) + by)), "{moved:?}");
    assert!(moved.contains(&(1, doc.vertex(1) + by * 0.5)), "{moved:?}");
    assert!(!moved.iter().any(|&(v, _)| v == 0), "{moved:?}");
}

#[test]
fn shift_o_tweaks_the_reach_while_the_move_holds_still() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.proportional = Some(100.0);
    let mut state = State::default();
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    lasso_right(&editor, &mut state);

    run(&editor, &mut state, key('g'), 300.0, 250.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
    run(
        &editor,
        &mut state,
        press(letter('O'), keyboard::Modifiers::SHIFT),
        310.0,
        270.0,
    );
    let cursor = mouse::Cursor::Available(Point::new(340.0, 260.0));
    let moved = editor
        .update(&mut state, &Event::Mouse(MOVE), bounds, cursor)
        .and_then(|action| action.into_inner().0);
    assert!(
        matches!(moved, Some(Message::TweakReach(across)) if across == 30.0),
        "{moved:?}"
    );
    // Let go of O away from where the move was (Shift first, which
    // doesn't end it): it goes on from there.
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        340.0,
        260.0,
    );
    assert!(state.tweaking.is_some());
    let o = letter('o');
    let release = Event::Keyboard(keyboard::Event::KeyReleased {
        key: o.clone(),
        modified_key: o,
        physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyO),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
    });
    run(&editor, &mut state, release, 340.0, 260.0);
    assert!(state.tweaking.is_none());
    run(&editor, &mut state, Event::Mouse(MOVE), 345.0, 260.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 345.0, 260.0);
    let by = Vector::new(15.0, 20.0);
    let moved = moves(&edits);
    assert!(moved.contains(&(3, doc.vertex(3) + by)), "{moved:?}");
}
