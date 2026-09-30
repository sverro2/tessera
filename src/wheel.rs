//! A colour wheel: hue round the circle, saturation out from the centre,
//! drawn at a given value (brightness). Click or drag on it to pick.

use std::cell::Cell;
use std::f32::consts::TAU;

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Event, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Point, Rectangle, Renderer, Theme, Vector};

use crate::paint::Hsv;

/// How many hue steps and saturation rings the wheel is drawn with.
const HUES: usize = 90;
const RINGS: usize = 10;

/// The wheel, `size` px across, with a marker at `color`. Picking gives
/// hue and saturation (the value stays `color`'s).
pub fn view<'a>(color: Hsv, size: f32) -> Element<'a, Hsv> {
    Canvas::new(Wheel { color })
        .width(size)
        .height(size)
        .into()
}

struct Wheel {
    color: Hsv,
}

#[derive(Default)]
pub struct State {
    dragging: bool,
    /// The wheel, drawn at the value in `drawn_at`.
    cache: canvas::Cache,
    drawn_at: Cell<Option<f32>>,
}

impl canvas::Program<Hsv> for Wheel {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Hsv>> {
        let pick = |pos: Point| {
            let d = pos - centre(bounds);
            Hsv {
                hue: (d.y.atan2(d.x) / TAU).rem_euclid(1.0),
                saturation: (d.x.hypot(d.y) / radius(bounds)).min(1.0),
                value: self.color.value,
            }
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                if pos.distance(centre(bounds)) > radius(bounds) {
                    return None;
                }
                state.dragging = true;
                Some(canvas::Action::publish(pick(pos)).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                let pos = cursor.position()? - Vector::new(bounds.x, bounds.y);
                Some(canvas::Action::publish(pick(pos)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.dragging => {
                state.dragging = false;
                Some(canvas::Action::capture())
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
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let value = self.color.value;
        if state.drawn_at.get() != Some(value) {
            state.cache.clear();
            state.drawn_at.set(Some(value));
        }
        let centre = centre(bounds);
        let radius = radius(bounds);

        let wheel = state.cache.draw(renderer, bounds.size(), |frame| {
            let at = |angle: f32, r: f32| centre + Vector::new(angle.cos(), angle.sin()) * r;
            for ring in 0..RINGS {
                let (r0, r1) = (
                    radius * ring as f32 / RINGS as f32,
                    radius * (ring + 1) as f32 / RINGS as f32,
                );
                for step in 0..HUES {
                    // A little wider than a step, so no seams show.
                    let a0 = TAU * step as f32 / HUES as f32;
                    let a1 = TAU * (step as f32 + 1.2) / HUES as f32;
                    let cell = Path::new(|p| {
                        p.move_to(at(a0, r0));
                        p.line_to(at(a0, r1));
                        p.line_to(at(a1, r1));
                        p.line_to(at(a1, r0));
                        p.close();
                    });
                    let color = Hsv {
                        hue: (step as f32 + 0.5) / HUES as f32,
                        saturation: (ring as f32 + 0.5) / RINGS as f32,
                        value,
                    };
                    frame.fill(&cell, color.to_color());
                }
            }
        });

        // Where the colour is.
        let mut marker = Frame::new(renderer, bounds.size());
        let angle = self.color.hue * TAU;
        let at = centre + Vector::new(angle.cos(), angle.sin()) * (self.color.saturation * radius);
        let ring = Path::circle(at, 5.0);
        marker.stroke(&ring, Stroke::default().with_color(Color::BLACK).with_width(3.0));
        marker.stroke(&ring, Stroke::default().with_color(Color::WHITE).with_width(1.5));

        vec![wheel, marker.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let over = cursor
            .position_in(bounds)
            .is_some_and(|p| p.distance(centre(bounds)) <= radius(bounds));
        if state.dragging || over {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

fn centre(bounds: Rectangle) -> Point {
    Point::new(bounds.width / 2.0, bounds.height / 2.0)
}

fn radius(bounds: Rectangle) -> f32 {
    bounds.width.min(bounds.height) / 2.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Program;

    #[test]
    fn picks_hue_and_saturation_where_clicked() {
        let wheel = Wheel {
            color: Hsv {
                value: 0.8,
                ..Hsv::default()
            },
        };
        let bounds = Rectangle::new(Point::new(10.0, 10.0), iced::Size::new(102.0, 102.0));
        let mut state = State::default();
        let mut picked = vec![];
        let mut run = |state: &mut State, e, at: Point| {
            let cursor = mouse::Cursor::Available(at);
            if let Some(action) = wheel.update(state, &Event::Mouse(e), bounds, cursor) {
                picked.extend(action.into_inner().0);
            }
        };
        let middle = bounds.center();

        // Halfway out, straight down: a quarter of the way round.
        run(
            &mut state,
            mouse::Event::ButtonPressed(mouse::Button::Left),
            middle + Vector::new(0.0, 25.0),
        );
        // Dragged far outside: fully saturated, straight left, halfway round.
        run(
            &mut state,
            mouse::Event::CursorMoved {
                position: Point::ORIGIN,
            },
            middle + Vector::new(-500.0, 0.0),
        );

        assert_eq!(picked.len(), 2, "{picked:?}");
        assert!((picked[0].hue - 0.25).abs() < 1e-3);
        assert!((picked[0].saturation - 0.5).abs() < 1e-3);
        assert_eq!(picked[0].value, 0.8);
        assert!((picked[1].hue - 0.5).abs() < 1e-3);
        assert_eq!(picked[1].saturation, 1.0);
    }
}
