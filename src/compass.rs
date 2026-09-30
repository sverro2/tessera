//! The compass: shows the view's rotation, with its needle pointing to world
//! "up", and turns the view. Click or drag on it to point the needle there;
//! double-click to point it straight up again.

use iced::mouse;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{self, Canvas, Event, Frame, Geometry, LineCap, Path, Stroke};
use iced::{Element, Point, Rectangle, Renderer, Theme, Vector};

use crate::editor::{BACKGROUND, EDGE, GRID_DOT, HOVER};

/// The dial's radius (screen px).
const RADIUS: f32 = 16.0;
/// How soon a second click makes a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy)]
pub enum Message {
    /// Turn the view to this rotation (radians, clockwise).
    TurnTo(f32),
}

/// The compass for a view turned by `rotation` (radians, clockwise).
pub fn view<'a>(rotation: f32) -> Element<'a, Message> {
    Canvas::new(Compass { rotation })
        .width(2.0 * RADIUS)
        .height(2.0 * RADIUS)
        .into()
}

struct Compass {
    rotation: f32,
}

#[derive(Debug, Default)]
pub struct State {
    /// Dragging the needle round.
    turning: bool,
    /// When the compass was last clicked, to tell a double-click.
    last_click: Option<Instant>,
}

impl canvas::Program<Message> for Compass {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                if pos.distance(centre(bounds)) > RADIUS {
                    return None;
                }
                let now = Instant::now();
                let double = state
                    .last_click
                    .is_some_and(|last| now.duration_since(last) < DOUBLE_CLICK);
                // A third click starts over.
                state.last_click = (!double).then_some(now);
                let rotation = if double {
                    0.0
                } else {
                    state.turning = true;
                    angle(bounds, pos)?
                };
                Some(canvas::Action::publish(Message::TurnTo(rotation)).and_capture())
            }
            // Dragging keeps turning, also outside the dial.
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.turning => {
                let pos = cursor.position()? - Vector::new(bounds.x, bounds.y);
                let rotation = angle(bounds, pos)?;
                Some(canvas::Action::publish(Message::TurnTo(rotation)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.turning => {
                state.turning = false;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // To light up as the cursor comes and goes.
            Event::Mouse(mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft) => {
                Some(canvas::Action::request_redraw())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let active = state.turning || cursor.is_over(bounds);

        let centre = centre(bounds);
        let (sin, cos) = self.rotation.sin_cos();
        let up = Vector::new(sin, -cos) * (RADIUS - 4.0);
        // Inset, so the ring isn't clipped at the edges.
        let dial = Path::circle(centre, RADIUS - 1.0);

        frame.fill(&dial, BACKGROUND);
        frame.stroke(&dial, stroke(if active { HOVER } else { GRID_DOT }, 1.5));
        let tail = centre - up * 0.6;
        frame.stroke(&Path::line(tail, centre), stroke(GRID_DOT, 2.0));
        frame.stroke(&Path::line(centre, centre + up), stroke(EDGE, 2.0));
        frame.fill(&Path::circle(centre + up, 2.5), EDGE);

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.turning {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }
}

/// The dial's centre, relative to its bounds.
fn centre(bounds: Rectangle) -> Point {
    Point::new(bounds.width / 2.0, bounds.height / 2.0)
}

/// The rotation that points the needle at `pos` (relative to the bounds):
/// clockwise from straight up. None right at the centre.
fn angle(bounds: Rectangle, pos: Point) -> Option<f32> {
    let d = pos - centre(bounds);
    (d.x.hypot(d.y) >= 2.0).then(|| d.x.atan2(-d.y))
}

fn stroke(color: iced::Color, width: f32) -> Stroke<'static> {
    Stroke::default()
        .with_color(color)
        .with_width(width)
        .with_line_cap(LineCap::Round)
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Program;
    use std::f32::consts::{FRAC_PI_2, PI};

    #[test]
    fn clicking_and_dragging_turns_the_view() {
        let compass = Compass { rotation: 0.0 };
        let bounds = Rectangle::new(Point::new(700.0, 12.0), iced::Size::new(32.0, 32.0));
        let middle = bounds.center();
        let mut state = State::default();
        let mut turns = vec![];
        let mut run = |state: &mut State, e, dx: f32, dy: f32| {
            let cursor = mouse::Cursor::Available(middle + Vector::new(dx, dy));
            if let Some(action) = compass.update(state, &Event::Mouse(e), bounds, cursor) {
                turns.extend(action.into_inner().0);
            }
        };
        let press = mouse::Event::ButtonPressed(mouse::Button::Left);
        let release = mouse::Event::ButtonReleased(mouse::Button::Left);
        let moved = mouse::Event::CursorMoved {
            position: Point::ORIGIN,
        };

        // Clicking right of centre points the needle there: a quarter turn
        // clockwise; dragging on, far below it, follows on.
        run(&mut state, press, 10.0, 0.0);
        run(&mut state, moved, 0.0, 100.0);
        run(&mut state, release, 0.0, 100.0);
        // Released: moving over it no longer turns; clicking beside it
        // doesn't either.
        run(&mut state, moved, -10.0, 0.0);
        run(&mut state, press, 40.0, 0.0);
        // A quick second click points it straight up again.
        run(&mut state, press, -10.0, 0.0);
        run(&mut state, release, -10.0, 0.0);

        let turns: Vec<f32> = turns.into_iter().map(|Message::TurnTo(r)| r).collect();
        let expected = [FRAC_PI_2, PI, 0.0];
        assert_eq!(turns.len(), expected.len(), "{turns:?}");
        for (turn, expected) in turns.iter().zip(expected) {
            assert!((turn.abs() - expected).abs() < 1e-5, "{turns:?}");
        }
    }
}
