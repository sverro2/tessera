//! Laying a gradient: drawing its line, moving its ends, cancelling.

use super::*;

fn laying(line: Option<(Point, Point)>) -> Tool {
    Tool::Gradient {
        line,
        radial: false,
    }
}

/// Runs `events` at cursor positions; the gradient lines published.
fn lines(
    editor: &Editor,
    state: &mut State,
    events: &[(Event, f32, f32)],
) -> Vec<Option<(Point, Point)>> {
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
    let mut published = vec![];
    for (event, x, y) in events {
        let cursor = mouse::Cursor::Available(Point::new(*x, *y));
        if let Some(action) = editor.update(state, event, bounds, cursor) {
            published.extend(action.into_inner().0);
        }
    }
    published
        .into_iter()
        .filter_map(|m| match m {
            Message::GradientLine(line) => Some(line),
            _ => None,
        })
        .collect()
}

const P: fn(f32, f32) -> Point = Point::new;

#[test]
fn dragging_draws_the_gradients_line_from_where_it_was_pressed() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.tool = laying(None);
    let mut state = State::default();
    let published = lines(
        &editor,
        &mut state,
        &[
            (Event::Mouse(PRESS), 100.0, 250.0),
            (Event::Mouse(MOVE), 102.0, 250.0),
            (Event::Mouse(MOVE), 300.0, 250.0),
            (Event::Mouse(RELEASE), 300.0, 250.0),
        ],
    );
    // Too short to tell at first: none yet.
    assert_eq!(
        published,
        vec![None, Some((P(100.0, 250.0), P(300.0, 250.0)))]
    );
    assert!(matches!(state.interaction, Interaction::Idle));
}

#[test]
fn an_end_of_the_line_is_dragged_alone_and_a_click_keeps_the_line() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let line = (P(100.0, 250.0), P(300.0, 250.0));
    editor.tool = laying(Some(line));
    let mut state = State::default();
    // The start, moved.
    let published = lines(
        &editor,
        &mut state,
        &[
            (Event::Mouse(PRESS), 103.0, 252.0),
            (Event::Mouse(MOVE), 150.0, 200.0),
            (Event::Mouse(RELEASE), 150.0, 200.0),
        ],
    );
    assert_eq!(published, vec![Some((P(150.0, 200.0), P(300.0, 250.0)))]);
    // A click elsewhere: no new line, the one there was kept.
    let published = lines(
        &editor,
        &mut state,
        &[
            (Event::Mouse(PRESS), 500.0, 500.0),
            (Event::Mouse(MOVE), 501.0, 500.0),
            (Event::Mouse(RELEASE), 501.0, 500.0),
        ],
    );
    assert_eq!(published, vec![Some(line)]);
}

#[test]
fn a_right_click_mid_drag_puts_the_line_back_as_it_was() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    let line = (P(100.0, 250.0), P(300.0, 250.0));
    editor.tool = laying(Some(line));
    let mut state = State::default();
    let published = lines(
        &editor,
        &mut state,
        &[
            (Event::Mouse(PRESS), 300.0, 250.0),
            (Event::Mouse(MOVE), 400.0, 400.0),
            (
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
                400.0,
                400.0,
            ),
            (Event::Mouse(RELEASE), 400.0, 400.0),
        ],
    );
    assert_eq!(
        published,
        vec![Some((P(100.0, 250.0), P(400.0, 400.0))), Some(line)]
    );
}

#[test]
fn holding_shift_the_line_keeps_to_every_15_degrees() {
    let doc = two_apart();
    let cache = Caches::default();
    let mut editor = editor(&doc, &cache);
    editor.tool = laying(None);
    let mut state = State::default();
    let published = lines(
        &editor,
        &mut state,
        &[
            (hold(keyboard::Modifiers::SHIFT), 100.0, 100.0),
            (Event::Mouse(PRESS), 100.0, 100.0),
            // 2° off level.
            (Event::Mouse(MOVE), 300.0, 107.0),
        ],
    );
    let Some(Some((start, end))) = published.last().copied() else {
        panic!("{published:?}");
    };
    assert_eq!(start, P(100.0, 100.0));
    assert!((end.y - 100.0).abs() < 1e-3, "{end:?}");
}
