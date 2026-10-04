//! The editor's tests: events (keys, the mouse) run through the canvas
//! program, and what it publishes or how its state changes looked at.

use super::*;
use canvas::Program;

mod checker;
mod drags;
mod extrude;
mod grid;
mod guides;
mod mirror;
mod painting;
mod proportional;
mod selection;
mod shapes;

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
        page: None,
        grid: Grid::default(),
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
    run(editor, state, key('q'), 230.0, 180.0);
    run(editor, state, Event::Mouse(PRESS), 230.0, 180.0);
    for (x, y) in [(370.0, 180.0), (370.0, 320.0), (230.0, 320.0)] {
        run(editor, state, Event::Mouse(MOVE), x, y);
    }
    run(editor, state, Event::Mouse(RELEASE), 230.0, 320.0);
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
