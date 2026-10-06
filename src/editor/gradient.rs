//! Laying a gradient (in the paint mode): dragging its line across the
//! shapes to colour, or one of its ends to change it. The app works out
//! the colours, and the layer is drawn as they'd be.

use super::*;

/// How many directions a line held with Shift keeps to: every 15°.
const ANGLES: f32 = 24.0;

impl Editor<'_> {
    /// Pressed at `pos` (screen): on an end of the line, moving it; else
    /// drawing a new one from here.
    pub(super) fn start_gradient(&self, pos: Point) -> Interaction {
        let Tool::Gradient { line, .. } = self.tool else {
            return Interaction::Idle;
        };
        let world = self.camera.to_world(pos);
        let near = |p: Point| self.camera.to_screen(p).distance(pos) <= VERTEX_HIT;
        let (start, end, moving_start) = match line {
            Some((start, end)) if near(end) => (start, end, false),
            Some((start, end)) if near(start) => (start, end, true),
            _ => (world, world, false),
        };
        Interaction::Gradienting {
            start,
            end,
            moving_start,
            before: line,
        }
    }

    /// The end being moved follows the cursor (at `world`); holding Shift,
    /// the line keeps to every 15°. The app is told where it is now.
    pub(super) fn move_gradient(&self, state: &mut State, world: Point) -> canvas::Action<Message> {
        let shift = state.modifiers.shift();
        let Interaction::Gradienting {
            start,
            end,
            moving_start,
            ..
        } = &mut state.interaction
        else {
            return canvas::Action::request_redraw();
        };
        let (moved, fixed) = if *moving_start {
            (start, *end)
        } else {
            (end, *start)
        };
        *moved = if shift {
            let d = world - fixed;
            let step = std::f32::consts::TAU / ANGLES;
            let angle = (d.y.atan2(d.x) / step).round() * step;
            fixed + Vector::new(angle.cos(), angle.sin()) * d.x.hypot(d.y)
        } else {
            world
        };
        canvas::Action::publish(Message::GradientLine(self.gradient_line(state))).and_capture()
    }

    /// The line as it is now: as being dragged (if long enough to tell its
    /// direction; else as it was), or as the app has it.
    pub(super) fn gradient_line(&self, state: &State) -> Option<(Point, Point)> {
        match state.interaction {
            Interaction::Gradienting {
                start, end, before, ..
            } => {
                let long = self
                    .camera
                    .to_screen(start)
                    .distance(self.camera.to_screen(end))
                    >= VERTEX_HIT;
                if long { Some((start, end)) } else { before }
            }
            _ => match self.tool {
                Tool::Gradient { line, .. } => line,
                _ => None,
            },
        }
    }

    /// The line, its start and end to grab; radial, the circle as far as
    /// it reaches.
    pub(super) fn draw_gradient_line(&self, overlay: &mut Frame, state: &State) {
        let Tool::Gradient { radial, .. } = self.tool else {
            return;
        };
        let Some((start, end)) = self.gradient_line(state) else {
            return;
        };
        let (a, b) = (self.camera.to_screen(start), self.camera.to_screen(end));
        if radial {
            let rim = Path::circle(a, a.distance(b));
            let dashed = |color, width| Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(color, width)
            };
            overlay.stroke(&rim, dashed(BACKGROUND, 3.5));
            overlay.stroke(&rim, dashed(HOVER, 1.5));
        }
        // Outlined, to show over any colour.
        let line = Path::line(a, b);
        overlay.stroke(&line, stroke(BACKGROUND, 4.0));
        overlay.stroke(&line, stroke(EDGE, 2.0));
        handle(overlay, a, EDGE);
        handle(overlay, b, HOVER);
    }
}
