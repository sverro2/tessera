use super::*;
use canvas::Program;

fn triangle(doc: &mut Document, corners: [(f32, f32); 3]) {
    assert!(doc.apply(Edit::AddTriangle {
        corners: corners.map(|(x, y)| Point::new(x, y)),
        snap: 0.0
    }));
}

/// One triangle, 0–1 along the bottom and 2 at the top.
fn one() -> Document {
    let mut doc = Document::default();
    triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)]);
    doc
}

/// Two separate triangles side by side, a gap between corners 1 and 3.
fn two_apart() -> Document {
    let mut doc = Document::default();
    triangle(&mut doc, [(100.0, 300.0), (200.0, 300.0), (150.0, 200.0)]);
    triangle(&mut doc, [(250.0, 300.0), (350.0, 300.0), (300.0, 200.0)]);
    doc
}

pub(super) fn editor<'a>(doc: &'a Document, caches: &'a Caches) -> Editor<'a> {
    Editor {
        document: doc,
        current: 0,
        crossfade: Crossfade::default(),
        show_edges: true,
        mirrors: &[],
        shown: true,
        below: Vec::new(),
        above: Vec::new(),
        camera: Camera::default(),
        caches,
        background: None,
        tool: Tool::Shape,
        proportional: None,
        clipboard: None,
        keys: Some(crate::keys::defaults()),
        lit: Vec::new(),
        deadline: Cell::new(None),
    }
}

/// Runs mouse events, each at a cursor position; returns the published
/// edits.
fn drag(editor: &Editor, events: &[(mouse::Event, f32, f32)]) -> Vec<Vec<Edit>> {
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut state = State::default();
    let mut published = vec![];
    for &(e, x, y) in events {
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        // Each mouse event in a frame of its own, as when moving slowly.
        let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
        for event in [Event::Mouse(e), frame] {
            if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                published.extend(action.into_inner().0);
            }
        }
    }
    published
        .into_iter()
        .filter_map(|m| match m {
            Message::Edit { edits, .. } => Some(edits),
            _ => None,
        })
        .collect()
}

const PRESS: mouse::Event = mouse::Event::ButtonPressed(mouse::Button::Left);
const RELEASE: mouse::Event = mouse::Event::ButtonReleased(mouse::Button::Left);
const MOVE: mouse::Event = mouse::Event::CursorMoved {
    position: Point::ORIGIN,
};

/// Runs `event` at cursor (`x`, `y`), then a frame; the edits published.
fn run(editor: &Editor, state: &mut State, event: Event, x: f32, y: f32) -> Vec<Vec<Edit>> {
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let cursor = mouse::Cursor::Available(Point::new(x, y));
    let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
    [event, frame]
        .into_iter()
        .filter_map(|event| editor.update(state, &event, bounds, cursor))
        .flat_map(|action| action.into_inner().0)
        .filter_map(|m| match m {
            Message::Edit { edits, .. } => Some(edits),
            _ => None,
        })
        .collect()
}

/// `key` pressed, holding `modifiers`.
fn press(key: keyboard::Key, modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

fn letter(c: char) -> keyboard::Key {
    keyboard::Key::Character(c.to_string().into())
}

/// A letter's key pressed.
fn key(c: char) -> Event {
    press(letter(c), keyboard::Modifiers::empty())
}

/// A letter's key pressed, holding Ctrl.
fn ctrl(c: char) -> Event {
    press(letter(c), keyboard::Modifiers::CTRL)
}

/// Esc pressed.
fn escape() -> Event {
    press(
        keyboard::Key::Named(keyboard::key::Named::Escape),
        keyboard::Modifiers::empty(),
    )
}

/// Holding `modifiers` now (none: letting go).
fn hold(modifiers: keyboard::Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers))
}

/// Lassos the right triangle of [`two_apart`] (vertices 3, 4, 5).
fn lasso_right(editor: &Editor, state: &mut State) {
    run(editor, state, hold(keyboard::Modifiers::CTRL), 230.0, 180.0);
    run(editor, state, Event::Mouse(PRESS), 230.0, 180.0);
    for (x, y) in [(370.0, 180.0), (370.0, 320.0), (230.0, 320.0)] {
        run(editor, state, Event::Mouse(MOVE), x, y);
    }
    run(editor, state, Event::Mouse(RELEASE), 230.0, 320.0);
    run(
        editor,
        state,
        hold(keyboard::Modifiers::empty()),
        230.0,
        320.0,
    );
    assert!([3, 4, 5].iter().all(|v| state.selection.contains(v)));
}

/// The vertices `edits` move, and where to, sorted.
fn moves(edits: &[Vec<Edit>]) -> Vec<(VertexId, Point)> {
    let [edits] = edits else {
        panic!("{edits:?}");
    };
    let [Edit::MoveVertices { moves }] = &edits[..] else {
        panic!("{edits:?}");
    };
    let mut moves = moves.clone();
    moves.sort_by_key(|&(v, _)| v);
    moves
}

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
            Some(Message::Frame(points)) => {
                let mut points: Vec<_> = points.iter().map(|p| (p.x, p.y)).collect();
                points.sort_by(|a, b| a.partial_cmp(b).unwrap());
                points.dedup();
                points
            }
            other => panic!("{other:?}"),
        }
    };
    // Over the left triangle: it.
    let left = [0, 1, 2].map(|v| (doc.vertex(v).x, doc.vertex(v).y));
    let mut left = left.to_vec();
    left.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(framed(150.0, 280.0), left);
    // Off the drawing: all of it.
    assert_eq!(framed(700.0, 550.0).len(), 6);
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
fn alt_tweaks_the_reach_while_the_move_holds_still() {
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
        hold(keyboard::Modifiers::ALT),
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
    // Let go of Alt away from where the move was: it goes on from there.
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::empty()),
        340.0,
        260.0,
    );
    run(&editor, &mut state, Event::Mouse(MOVE), 345.0, 260.0);
    let edits = run(&editor, &mut state, Event::Mouse(PRESS), 345.0, 260.0);
    let by = Vector::new(15.0, 20.0);
    let moved = moves(&edits);
    assert!(moved.contains(&(3, doc.vertex(3) + by)), "{moved:?}");
}

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
    let lit = editor.lit(&state, state.pending.as_ref().unwrap());
    assert!(lit.contains(&Guide::Axis {
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
    let lit = editor.lit(&state, state.pending.as_ref().unwrap());
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
    let pending = state.pending.as_ref().unwrap();
    assert!(pending.at.distance(end) < 1e-3, "{:?}", pending.at);
    let lit = editor.lit(&state, pending);
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
fn alt_and_moving_tweaks_the_edge_width() {
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
        let _ = run(&editor, &mut state, hold(keyboard::Modifiers::ALT), 200.0);
        let moved = run(&editor, &mut state, Event::Mouse(MOVE), 230.0).flatten();
        match moved {
            Some(Message::TweakWidth(across)) => {
                assert!(tweaks);
                assert_eq!(across, 30.0);
                // The edge previewed is still looked for where Alt was
                // pressed.
                assert_eq!(state.tweaking.unwrap().at, Point::new(200.0, 300.0));
            }
            _ => assert!(!tweaks, "{target:?}: {moved:?}"),
        }
        // Let go: back to following the mouse.
        let _ = run(
            &editor,
            &mut state,
            hold(keyboard::Modifiers::empty()),
            230.0,
        );
        assert!(state.tweaking.is_none());
    }
}

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
fn tweaking_an_edge_shows_the_layer_with_just_its_new_width() {
    // The bottom edge (0 1) painted wide; Alt over it, the brush thin.
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
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::ALT),
        200.0,
        300.0,
    );
    let tweaked = editor.tweaked_edge(&state).expect("tweaking");
    let style = tweaked.edge_style(0, 1).unwrap();
    assert_eq!(style.width, 2.0);
    assert_eq!(Some(style.color), brush.color());
    // The layer itself is left be.
    assert_eq!(doc.edge_style(0, 1), Some(wide));
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

    // Alt over the bottom edge; the mouse wanders off; a click there
    // paints the bottom edge, with the width set.
    editor.tool = Tool::Paint {
        target: paint::Target::Edges,
        brush,
        width: 5.0,
        picking: false,
    };
    let mut state = State::default();
    run(
        &editor,
        &mut state,
        hold(keyboard::Modifiers::ALT),
        200.0,
        300.0,
    );
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
fn slivers_are_rejected() {
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
    assert_eq!(edit(303.0), None);
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
fn limited_moves_never_leave_slivers() {
    // A fan around vertex 0 whose outline has a dent at (30, 30), so the
    // area vertex 0 may move in is bounded by some edges' extensions.
    let ring = [
        Point::new(100.0, 0.0),
        Point::new(30.0, 30.0),
        Point::new(0.0, 100.0),
        Point::new(-100.0, 0.0),
        Point::new(0.0, -100.0),
    ]
    .map(|p| p + Vector::new(400.0, 300.0));
    let centre = Point::new(390.0, 290.0);

    // Vertex 0 is the centre, 1..=5 the ring.
    let mut doc = Document::default();
    for (from, to) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)] {
        assert!(doc.apply(Edit::AddTriangle {
            corners: [centre, ring[from], ring[to]],
            snap: 0.0
        }));
    }
    assert_eq!(doc.triangle_ids().len(), 5);

    let cache = Caches::default();
    let editor = editor(&doc, &cache);

    for x in (0..16).map(|i| i as f32 * 50.0) {
        for y in (0..12).map(|i| i as f32 * 50.0) {
            // Without a valid position the vertex stays where it was.
            let Some((_, pending)) = editor.limit_move(0, Point::new(x, y)) else {
                continue;
            };

            for t in pending
                .result
                .triangle_ids()
                .iter()
                .filter(|t| t.contains(&0))
            {
                let h = min_height(t.map(|v| pending.result.vertex(v)));
                assert!(
                    h >= MIN_THICKNESS,
                    "cursor ({x}, {y}): {:?} leaves height {h}",
                    pending.edits
                );
            }
        }
    }
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
