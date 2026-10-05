//! Drags: creating, extending and moving, and the shape kept valid.

use super::*;

#[test]
fn a_drag_is_worked_out_once_a_frame() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let send = |state: &mut State, event: Event, x: f32, y: f32| {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        editor.update(state, &event, bounds, cursor);
    };

    // Grab corner 2 (200, 100), and move it in many small steps
    // within one frame, as a fast mouse reports.
    send(&mut state, Event::Mouse(MOVE), 200.0, 100.0);
    send(&mut state, Event::Mouse(PRESS), 200.0, 100.0);
    for i in 1..=500 {
        send(
            &mut state,
            Event::Mouse(MOVE),
            200.0,
            100.0 - i as f32 * 0.1,
        );
    }
    // Nothing worked out yet (the preview is still where it was
    // grabbed): just where it's aimed.
    let at = state.pending.as_ref().map(|pending| pending.at);
    assert_eq!(at, Some(Point::new(200.0, 100.0)));
    assert_eq!(state.aim, Some(Point::new(200.0, 50.0)));

    // The frame: once, for where the mouse is now.
    send(
        &mut state,
        Event::Window(window::Event::RedrawRequested(Instant::now())),
        200.0,
        50.0,
    );
    assert!(state.aim.is_none());
    let pending = state.pending.as_ref().expect("worked out");
    assert!(pending.at.distance(Point::new(200.0, 50.0)) < 1e-3);
}

#[test]
fn a_hidden_layer_is_not_edited() {
    let doc = one();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.shown = false;
    let strokes = [
        // Blank canvas, an edge, a corner.
        [(400.0, 400.0), (450.0, 450.0)],
        [(200.0, 300.0), (200.0, 350.0)],
        [(100.0, 300.0), (50.0, 350.0)],
    ];
    for tool in [
        Tool::Shape,
        Tool::Paint {
            target: paint::Target::Faces,
            brush: Brush::default(),
            width: 1.0,
            picking: false,
        },
    ] {
        editor.tool = tool;
        for [(x0, y0), (x1, y1)] in strokes {
            let edits = drag(
                &editor,
                &[
                    (MOVE, x0, y0),
                    (PRESS, x0, y0),
                    (MOVE, x1, y1),
                    (RELEASE, x1, y1),
                ],
            );
            assert!(edits.is_empty(), "{tool:?}: {edits:?}");
        }
    }
}

#[test]
fn dragging_out_of_a_face_extends_from_the_edge_crossed() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // Out through the bottom edge: as if dragged from that edge.
    let from_face = drag(
        &editor,
        &[
            (MOVE, 200.0, 250.0),
            (PRESS, 200.0, 250.0),
            (MOVE, 200.0, 290.0),
            (MOVE, 200.0, 350.0),
            (RELEASE, 200.0, 350.0),
        ],
    );
    let from_edge = drag(
        &editor,
        &[
            (MOVE, 200.0, 300.0),
            (PRESS, 200.0, 300.0),
            (MOVE, 200.0, 350.0),
            (RELEASE, 200.0, 350.0),
        ],
    );
    assert_eq!(from_face, from_edge);
    assert!(
        matches!(&from_face[..], [edits] if matches!(edits[..], [Edit::AddTriangle { .. }])),
        "{from_face:?}"
    );

    // Out, then back into the face: nothing.
    let back = drag(
        &editor,
        &[
            (MOVE, 200.0, 250.0),
            (PRESS, 200.0, 250.0),
            (MOVE, 200.0, 350.0),
            (MOVE, 200.0, 250.0),
            (RELEASE, 200.0, 250.0),
        ],
    );
    assert!(back.is_empty(), "{back:?}");
}

#[test]
fn changing_direction_mid_drag() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // Inwards, back over the source edge, then out the other side.
    let mut events = vec![(MOVE, 200.0, 300.0), (PRESS, 200.0, 300.0)];
    for y in [290.0, 250.0, 200.0, 250.0, 299.0, 330.0, 350.0] {
        events.push((MOVE, 200.0, y));
    }
    events.push((RELEASE, 200.0, 350.0));

    let edits = drag(&editor, &events);
    assert!(
        matches!(&edits[..], [edits] if matches!(edits[..], [Edit::AddTriangle { .. }])),
        "{edits:?}"
    );
}

#[test]
fn dragging_a_corner_onto_a_separate_triangle_welds() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // Corner 1 (200, 300) dropped next to corner 3 (250, 300).
    let pending = editor.move_to(1, Point::new(245.0, 302.0)).unwrap();
    assert_eq!(
        pending.edits,
        [Edit::MoveVertex {
            id: 1,
            to: Point::new(250.0, 300.0)
        }]
    );
    assert_eq!(pending.changes.welded, [(1, 3)]);

    // Nothing goes: no triangle squashed, no edge lost.
    assert!(pending.changes.squashed.is_empty());
    assert!(removed_edges(&doc, &pending).is_empty());
    let kept = |v| pending.changes.kept(v);
    assert_eq!(new_triangles(&doc, &pending.result, &kept).count(), 0);
}

#[test]
fn extending_onto_a_separate_corner_shows_as_a_join() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // From the right side 1–2 of the left triangle out to (near) the top
    // corner 5 of the right one.
    let pending = editor
        .pending(Interaction::Extending {
            a: 1,
            b: 2,
            start: Point::new(175.0, 250.0),
            apex: Point::new(296.0, 204.0),
        })
        .unwrap();
    assert_eq!(pending.at, doc.vertex(5));
    let (_, rings) = editor.joins(&pending, Some((1, 2)));
    assert_eq!(rings, [5]);
}

#[test]
fn welding_onto_a_neighbour_shows_as_a_join() {
    let mut doc = one();
    doc.apply(Edit::InsertVertex {
        at: Point::new(200.0, 230.0),
    });
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // The centre 3 onto corner 0: it squashes two triangles, but what
    // it lands on is a weld.
    let pending = editor.move_to(3, Point::new(104.0, 297.0)).unwrap();
    assert!(!pending.changes.squashed.is_empty());
    let (_, rings) = editor.joins(&pending, None);
    assert_eq!(rings, [0]);
    assert_eq!(pending.at, doc.vertex(0));
}

#[test]
fn dragging_a_corner_onto_a_separate_outline_edge_joins() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // Corner 1 dropped near the middle of edge 3–5, (275, 250): the
    // other triangle is cut there, joining both at the corner.
    let pending = editor.move_to(1, Point::new(272.0, 250.0)).unwrap();
    assert!(
        pending
            .changes
            .cut
            .iter()
            .any(|&(a, b, w)| (a.min(b), a.max(b), w) == (3, 5, 1)),
        "{:?}",
        pending.changes
    );
    assert_eq!(pending.result.triangle_ids().len(), 3);
    let edges = pending.result.unique_edges();
    assert!(edges.contains(&(1, 3)) && edges.contains(&(1, 5)));
    assert!(removed_edges(&doc, &pending).is_empty());
}

#[test]
fn pulling_from_an_edge_is_cancelled_back_by_it() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    let edit = |y| {
        editor
            .pending(Interaction::Extending {
                a: 0,
                b: 1,
                start: Point::new(200.0, 300.0),
                apex: Point::new(200.0, y),
            })
            .map(|pending| pending.edits)
    };
    assert_eq!(edit(297.0), None); // on the source edge: cancelled
    assert_eq!(edit(303.0), None); // within reach of it: the same
    assert!(matches!(
        edit(250.0).as_deref(),
        Some([Edit::InsertVertex { .. }, Edit::MoveVertex { .. }])
    ));
    assert!(matches!(
        edit(350.0).as_deref(),
        Some([Edit::AddTriangle { .. }])
    ));
}

#[test]
fn dragging_a_centre_out_of_the_outline_stretches_the_shape() {
    let mut doc = one();
    doc.apply(Edit::InsertVertex {
        at: Point::new(200.0, 230.0),
    });
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    let edits = drag(
        &editor,
        &[
            (PRESS, 200.0, 230.0),
            (MOVE, 200.0, 280.0),
            (MOVE, 200.0, 360.0),
            // Far past the edge, off to the side.
            (MOVE, 260.0, 400.0),
            (RELEASE, 260.0, 400.0),
        ],
    );

    // The centre follows the cursor out through the bottom edge 0–1: the
    // triangle on that edge is dropped and the other two stretch along.
    assert_eq!(
        edits,
        [[Edit::MoveVertex {
            id: 3,
            to: Point::new(260.0, 400.0),
        }]]
    );
    assert!(doc.clone().apply_all(&edits[0]));
}

#[test]
fn a_new_triangle_over_other_geometry_stays_an_extension() {
    let mut doc = one();
    // A separate triangle to the right.
    triangle(&mut doc, [(400.0, 300.0), (500.0, 300.0), (450.0, 200.0)]);
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // From the right side of the first triangle out, then into the
    // other triangle: must not turn into splitting it.
    let edits = drag(
        &editor,
        &[
            (PRESS, 250.0, 200.0),
            (MOVE, 320.0, 220.0),
            (MOVE, 380.0, 260.0),
            (MOVE, 460.0, 270.0),
            (RELEASE, 460.0, 270.0),
        ],
    );

    let [edits] = &edits[..] else {
        panic!("expected one edit, got {edits:?}");
    };
    let (p1, p2) = (doc.vertex(1), doc.vertex(2));
    assert!(
        matches!(edits[..], [Edit::AddTriangle { corners: [a, b, _], .. }]
            if (a, b) == (p1, p2) || (a, b) == (p2, p1)),
        "{edits:?}"
    );
    assert!(doc.clone().apply_all(edits));
}

#[test]
fn moving_close_to_an_edge_line_snaps_onto_it() {
    let mut doc = one();
    doc.apply(Edit::InsertVertex {
        at: Point::new(200.0, 230.0),
    });
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // The centre 3, dragged to just above the line through the bottom
    // edge 0–1, past its end: its triangle on that edge would be a
    // sliver. It snaps onto the line, squashing that triangle, and the
    // mesh rearranges around it.
    for y in [296.0, 298.0, 299.5] {
        let pending = editor
            .pending(Interaction::MovingVertex {
                id: 3,
                to: Point::new(400.0, y),
            })
            .unwrap_or_else(|| panic!("moving to y = {y} is refused"));
        assert!(
            matches!(pending.edits[..], [Edit::MoveVertex { id: 3, to }]
                if (to.y - 300.0).abs() < 1e-3),
            "{:?}",
            pending.edits
        );
        assert!(
            pending
                .result
                .triangles()
                .all(|t| min_height(t) >= MIN_THICKNESS),
            "y = {y}: {:?}",
            pending.result.triangle_ids()
        );
    }
}

#[test]
fn dragging_into_a_triangle_can_go_on_into_its_neighbour() {
    let mut doc = one();
    // A neighbour on the right edge 1–2, with far corner 3.
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(1), doc.vertex(2), Point::new(350.0, 150.0)],
        snap: 0.0,
    });
    let area = |doc: &Document| {
        doc.triangles()
            .map(|[a, b, c]| area2(a, b, c) / 2.0)
            .sum::<f32>()
    };
    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    // From the bottom edge 0–1 into the triangle, then onto the shared
    // edge (squashing that child) or through it (flipping it). Either
    // way the new vertex 4 gets an edge to the neighbour's far corner 3,
    // and the outline stays the same.
    for (apex, squashes) in [
        (Point::new(253.0, 200.0), true),
        (Point::new(290.0, 200.0), false),
    ] {
        let pending = editor
            .pending(Interaction::Extending {
                a: 0,
                b: 1,
                start: Point::new(200.0, 300.0),
                apex,
            })
            .unwrap_or_else(|| panic!("dragging to {apex:?} is refused"));
        let result = &pending.result;

        assert!(matches!(pending.edits[0], Edit::InsertVertex { .. }));
        assert_eq!(
            !pending.changes.squashed.is_empty(),
            squashes,
            "{:?}",
            pending.changes
        );
        assert_eq!(
            result.triangle_ids().len(),
            4,
            "{:?}",
            result.triangle_ids()
        );
        assert!(result.edges().any(|e| e == (4, 3) || e == (3, 4)));
        assert!((area(result) - area(&doc)).abs() < 0.5);
    }
}

#[test]
fn a_right_click_cancels_a_drag() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let right = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right));

    // Moving corner 2, creating on blank canvas, extending from an edge,
    // lassoing (none of it lands, or selects anything).
    for [(x0, y0), (x1, y1)] in [
        [(200.0, 100.0), (220.0, 60.0)],
        [(500.0, 400.0), (560.0, 420.0)],
        [(200.0, 300.0), (200.0, 380.0)],
    ] {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(PRESS), x0, y0);
        run(&editor, &mut state, Event::Mouse(MOVE), x1, y1);
        // (A new triangle's first side makes nothing yet.)
        assert!(
            state.pending.is_some() || matches!(state.interaction, Interaction::Creating { .. })
        );
        assert!(run(&editor, &mut state, right.clone(), x1, y1).is_empty());
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), x1, y1).is_empty());
        assert!(matches!(state.interaction, Interaction::Idle));
    }

    let mut state = State::default();
    run(&editor, &mut state, key('q'), 50.0, 50.0);
    run(&editor, &mut state, Event::Mouse(PRESS), 50.0, 50.0);
    for (x, y) in [(400.0, 50.0), (400.0, 400.0), (50.0, 400.0)] {
        run(&editor, &mut state, Event::Mouse(MOVE), x, y);
    }
    run(&editor, &mut state, right, 50.0, 400.0);
    run(&editor, &mut state, Event::Mouse(RELEASE), 50.0, 400.0);
    assert!(state.selection.is_empty());
}

#[test]
fn ctrl_middle_drag_zooms_about_where_it_started() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let mut messages = vec![];
    let events = [
        (hold(keyboard::Modifiers::CTRL), 400.0, 300.0),
        (
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)),
            400.0,
            300.0,
        ),
        (Event::Mouse(MOVE), 400.0, 250.0),
        (Event::Mouse(MOVE), 450.0, 350.0),
        (
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Middle)),
            450.0,
            350.0,
        ),
        (Event::Mouse(MOVE), 450.0, 300.0),
    ];
    for (event, x, y) in events {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
            messages.extend(action.into_inner().0);
        }
    }
    let zooms: Vec<_> = messages
        .into_iter()
        .filter_map(|m| match m {
            Message::Zoom { anchor, factor } => Some((anchor, factor)),
            _ => None,
        })
        .collect();
    // Up zooms in, down out; nothing after letting go.
    let [(a, up), (b, down)] = zooms[..] else {
        panic!("{zooms:?}");
    };
    assert_eq!(a, Point::new(400.0, 300.0));
    assert_eq!(b, a);
    assert!(up > 1.0 && down < 1.0, "{up} {down}");
    assert!((up * down * (50.0 / ZOOM_DRAG).exp() - 1.0).abs() < 1e-5);
}

#[test]
fn middle_drag_without_ctrl_still_pans() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let mut messages = vec![];
    for (event, y) in [
        (
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)),
            300.0,
        ),
        (Event::Mouse(MOVE), 250.0),
    ] {
        let cursor = mouse::Cursor::Available(Point::new(400.0, y));
        if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
            messages.extend(action.into_inner().0);
        }
    }
    assert!(
        matches!(&messages[..], [Message::Pan(v)] if v.y == -50.0),
        "{messages:?}"
    );
}

#[test]
fn a_vertex_dragged_close_to_an_edge_lands_on_it_joined_up() {
    let doc = two_apart();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    // Vertex 1 (200, 300) of the left triangle, to a few px left of the
    // right one's left edge (which at y 280 is at x 260).
    for off in [1.0, 3.0, 5.0] {
        let edits = drag(
            &editor,
            &[
                (PRESS, 200.0, 300.0),
                (MOVE, 260.0 - off, 280.0),
                (RELEASE, 260.0 - off, 280.0),
            ],
        );
        let [edits] = &edits[..] else {
            panic!("{edits:?}");
        };
        let mut after = doc.clone();
        assert!(after.apply_all(edits));
        // On the edge, a corner of the right triangle too: joined up.
        let p = after.vertex(1);
        assert!((p.x - (250.0 + (300.0 - p.y) / 2.0)).abs() < 1e-3, "{p:?}");
        assert!(after.connected(1).len() > 2, "{:?}", after.triangle_ids());
    }
}

#[test]
fn a_new_triangle_is_its_first_side_dragged_then_a_click_for_its_third_corner() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();

    // The first side, dragged out: nothing made on letting go.
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 400.0);
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 600.0, 400.0).is_empty());

    // The third corner follows the cursor (no button held), anywhere: not
    // a third as long as the others, say; a click puts it down there.
    run(&editor, &mut state, Event::Mouse(MOVE), 570.0, 470.0);
    assert!(state.pending.is_some());
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 570.0, 470.0);
    assert_eq!(
        edits,
        [[Edit::AddTriangle {
            corners: [
                Point::new(500.0, 400.0),
                Point::new(600.0, 400.0),
                Point::new(570.0, 470.0)
            ],
            snap: editor.snap_distance(),
        }]]
    );
    assert!(matches!(state.interaction, Interaction::Idle));
}

#[test]
fn a_right_click_or_esc_drops_a_new_triangle_whole() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let right = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right));
    for cancel in [right, escape()] {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 400.0);
        run(&editor, &mut state, Event::Mouse(RELEASE), 600.0, 400.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 550.0, 480.0);
        assert!(run(&editor, &mut state, cancel, 550.0, 480.0).is_empty());
        assert!(matches!(state.interaction, Interaction::Idle));
        assert!(state.pending.is_none());
        // A click after makes nothing either.
        assert!(run(&editor, &mut state, Event::Mouse(PRESS), 550.0, 480.0).is_empty());
    }
}

#[test]
fn undo_while_making_a_triangle_cancels_it_and_undoes_nothing_else() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let cursor = mouse::Cursor::Available(Point::new(560.0, 440.0));
    // Ctrl+Z: whether the canvas keeps it (from the app, which would undo).
    let undo = |state: &mut State| {
        editor
            .update(state, &ctrl('z'), bounds, cursor)
            .is_some_and(|action| action.into_inner().2 == iced::event::Status::Captured)
    };

    // Dragging out its first side, then placing its third corner.
    for placing in [false, true] {
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 400.0);
        if placing {
            run(&editor, &mut state, Event::Mouse(RELEASE), 600.0, 400.0);
            run(&editor, &mut state, Event::Mouse(MOVE), 560.0, 440.0);
            assert!(state.pending.is_some());
        }
        assert!(undo(&mut state));
        assert!(matches!(state.interaction, Interaction::Idle));
        assert!(state.pending.is_none());
        // Letting go, or a click, after makes nothing.
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 560.0, 440.0).is_empty());
        assert!(run(&editor, &mut state, Event::Mouse(PRESS), 560.0, 440.0).is_empty());
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 560.0, 440.0).is_empty());

        // Nothing going on: it's the app's, to undo.
        assert!(!undo(&mut state));
    }
}

#[test]
fn a_click_without_dragging_out_a_side_makes_no_triangle() {
    let doc = one();
    let cache = Caches::default();
    let editor = editor(&doc, &cache);
    let mut state = State::default();
    run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 400.0);
    run(&editor, &mut state, Event::Mouse(MOVE), 503.0, 401.0);
    assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 503.0, 401.0).is_empty());
    assert!(matches!(state.interaction, Interaction::Idle));
}
