//! The gradient as a bar, its stops marked under it: click a stop to pick
//! it, drag it to move it, click elsewhere to add one. Also, as a small
//! strip that does nothing, a preset's look.

use iced::mouse;
use iced::widget::canvas::{self, Canvas, Event, Frame, Geometry, Path, Stroke};
use iced::{Color, Element, Point, Rectangle, Renderer, Size, Theme, Vector};

use crate::gradient::Gradient;

/// How tall the colours are, and the stops' markers under them.
const BAR: f32 = 20.0;
const MARKER: f32 = 12.0;
/// How far (px) from a marker a click still picks it.
const GRAB: f32 = 6.0;
/// How many slices the colours are drawn in.
const SLICES: usize = 64;
/// The checkerboard behind what's see-through: its squares, and greys.
const SQUARE: f32 = 5.0;
const DARK: Color = Color::from_rgb8(0x2a, 0x2c, 0x3e);
const LIGHT: Color = Color::from_rgb8(0x3c, 0x3f, 0x56);

/// What's done on the bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BarEvent {
    /// Stop `i` picked.
    Pick(usize),
    /// The picked stop dragged to here (0 to 1 along it).
    Move(f32),
    /// A stop added here.
    Add(f32),
}

/// The bar, `width` px wide, with stop `selected` marked as picked.
pub fn view<'a>(gradient: &Gradient, selected: usize, width: f32) -> Element<'a, BarEvent> {
    Canvas::new(Bar {
        gradient: gradient.clone(),
        selected: Some(selected),
    })
    .width(width)
    .height(BAR + MARKER)
    .into()
}

/// Just its colours, `size` big: a preset's look.
pub fn strip<'a>(gradient: &Gradient, size: Size) -> Element<'a, BarEvent> {
    Canvas::new(Bar {
        gradient: gradient.clone(),
        selected: None,
    })
    .width(size.width)
    .height(size.height)
    .into()
}

struct Bar {
    gradient: Gradient,
    /// The stop picked; `None`: only the colours, no markers, nothing to do.
    selected: Option<usize>,
}

#[derive(Default)]
pub struct State {
    dragging: bool,
}

impl Bar {
    /// Where across `bounds` (0 to 1) `x` (px, within them) is.
    fn at(bounds: Rectangle, x: f32) -> f32 {
        (x / bounds.width.max(1.0)).clamp(0.0, 1.0)
    }

    /// The stop whose marker is at `x` (px), if any: the nearest.
    fn stop_at(&self, bounds: Rectangle, x: f32) -> Option<usize> {
        self.gradient
            .stops()
            .iter()
            .enumerate()
            .map(|(i, stop)| (i, (stop.at * bounds.width - x).abs()))
            .filter(|&(_, d)| d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }
}

impl canvas::Program<BarEvent> for Bar {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<BarEvent>> {
        self.selected?;
        let publish = |event: BarEvent| Some(canvas::Action::publish(event).and_capture());
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let pos = cursor.position_in(bounds)?;
                match self.stop_at(bounds, pos.x) {
                    Some(i) => {
                        state.dragging = true;
                        publish(BarEvent::Pick(i))
                    }
                    None => publish(BarEvent::Add(Self::at(bounds, pos.x))),
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => {
                let pos = cursor.position()? - Vector::new(bounds.x, bounds.y);
                publish(BarEvent::Move(Self::at(bounds, pos.x)))
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
        _state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let width = bounds.width;
        let height = if self.selected.is_some() {
            BAR
        } else {
            bounds.height
        };
        // What's see-through shows a checkerboard.
        frame.fill_rectangle(Point::ORIGIN, Size::new(width, height), DARK);
        let (columns, rows) = (
            (width / SQUARE).ceil() as usize,
            (height / SQUARE).ceil() as usize,
        );
        for row in 0..rows {
            for column in (row % 2..columns).step_by(2) {
                let corner = Point::new(column as f32 * SQUARE, row as f32 * SQUARE);
                let size = Size::new(SQUARE.min(width - corner.x), SQUARE.min(height - corner.y));
                frame.fill_rectangle(corner, size, LIGHT);
            }
        }
        let slice = width / SLICES as f32;
        for i in 0..SLICES {
            let color = self.gradient.color_at((i as f32 + 0.5) / SLICES as f32);
            // A little wider than a slice, so no seams show.
            frame.fill_rectangle(
                Point::new(i as f32 * slice, 0.0),
                Size::new(slice + 0.5, height),
                color,
            );
        }

        if let Some(selected) = self.selected {
            for (i, stop) in self.gradient.stops().iter().enumerate() {
                let x = stop.at * width;
                let marker = Path::new(|p| {
                    p.move_to(Point::new(x, BAR));
                    p.line_to(Point::new(x + 5.0, BAR + MARKER - 1.0));
                    p.line_to(Point::new(x - 5.0, BAR + MARKER - 1.0));
                    p.close();
                });
                frame.fill(
                    &marker,
                    Color {
                        a: 1.0,
                        ..stop.color
                    },
                );
                let picked = i == selected;
                frame.stroke(
                    &marker,
                    Stroke::default()
                        .with_color(if picked { Color::WHITE } else { Color::BLACK })
                        .with_width(if picked { 2.0 } else { 1.0 }),
                );
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.selected.is_none() {
            return mouse::Interaction::default();
        }
        match cursor.position_in(bounds) {
            _ if state.dragging => mouse::Interaction::Grabbing,
            Some(pos) if self.stop_at(bounds, pos.x).is_some() => mouse::Interaction::Grab,
            Some(_) => mouse::Interaction::Crosshair,
            None => mouse::Interaction::default(),
        }
    }
}
