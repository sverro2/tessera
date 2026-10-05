//! Tweaking: a key held (C, Shift+C, W, Shift+O, Shift+Q) and the mouse
//! moved across changes a setting. Each has a [`Dial`]: its range, and how
//! far the mouse moves it. While tweaking, a slider under the cursor shows
//! how far the mouse can go each way, and what each way does.

use super::*;
use crate::paint::Hsv;

/// How a setting is tweaked: its range, and how much a pixel moved across
/// changes it (by that much; or, `scaled`, by that share of it, so it goes
/// finely when small and quickly when large).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dial {
    pub min: f32,
    pub max: f32,
    pub per_px: f32,
    pub scaled: bool,
}

impl Dial {
    /// An edge's width (W).
    pub const WIDTH: Dial = Dial {
        min: 0.5,
        max: 12.0,
        per_px: 0.01,
        scaled: true,
    };
    /// How light the colour is (C): through black at 0 and back to it at
    /// 1, to white at 2 (see [`Hsv`]).
    pub const LIGHTNESS: Dial = Dial {
        min: 0.0,
        max: 2.0,
        per_px: 0.004,
        scaled: false,
    };
    /// How opaque the colour is (Shift+C): see-through to opaque in some
    /// 500 px.
    pub const OPACITY: Dial = Dial {
        min: 0.0,
        max: 1.0,
        per_px: 0.002,
        scaled: false,
    };
    /// How far proportional editing reaches (Shift+O), screen px.
    pub const REACH: Dial = Dial {
        min: 5.0,
        max: 1000.0,
        per_px: 0.01,
        scaled: true,
    };
    /// How far an edge loop may turn at a vertex (Shift+Q), radians.
    pub const LOOP_TURN: Dial = Dial {
        min: 0.0,
        max: MAX_LOOP_TURN,
        per_px: 0.2 * std::f32::consts::PI / 180.0,
        scaled: false,
    };

    /// `value` with the mouse moved `across` (screen px; to the right, up).
    pub fn moved(self, value: f32, across: f32) -> f32 {
        let moved = if self.scaled {
            value * (across * self.per_px).exp()
        } else {
            value + across * self.per_px
        };
        moved.clamp(self.min, self.max)
    }

    /// How far (screen px) the mouse can go from `value` each way before
    /// the setting stops: to the left (down to its least), to the right.
    pub fn room(self, value: f32) -> (f32, f32) {
        let value = value.clamp(self.min, self.max);
        if self.scaled {
            (
                (value / self.min).ln() / self.per_px,
                (self.max / value).ln() / self.per_px,
            )
        } else {
            (
                (value - self.min) / self.per_px,
                (self.max - value) / self.per_px,
            )
        }
    }
}

/// How far below the cursor (screen px) the slider runs.
const BELOW: f32 = 30.0;

impl Editor<'_> {
    /// What's being tweaked: its dial, where it is, and that as said.
    fn tweaked_value(&self, state: &State, what: Tweaking) -> Option<(Dial, f32, String)> {
        Some(match what {
            Tweaking::Width => {
                let Tool::Paint { width, .. } = self.tool else {
                    return None;
                };
                (Dial::WIDTH, width, format!("{width:.1}"))
            }
            Tweaking::Lightness => {
                let value = self.brush.value;
                // Signed, from the colour itself.
                let off = ((value - 1.0) * 100.0).round();
                let said = if off == 0.0 {
                    "0".to_string()
                } else {
                    format!("{off:+.0}%")
                };
                (Dial::LIGHTNESS, value, said)
            }
            Tweaking::Opacity => {
                let alpha = self.brush.alpha;
                (Dial::OPACITY, alpha, format!("{:.0}%", alpha * 100.0))
            }
            Tweaking::Reach => {
                let reach = self.proportional?;
                (Dial::REACH, reach, format!("{reach:.0} px"))
            }
            Tweaking::LoopTurn => {
                let turn = self.loop_turn(state);
                (Dial::LOOP_TURN, turn, format!("{:.0}°", turn.to_degrees()))
            }
        })
    }

    /// While tweaking: a slider under the cursor (at `cursor`, screen), as
    /// long as the mouse can go each way, the handle at the cursor; at its
    /// ends what that way does; by the handle which ways it can still go,
    /// and the value.
    pub(super) fn draw_tweak(
        &self,
        frame: &mut Frame,
        state: &State,
        bounds: Size,
        cursor: Option<Point>,
    ) {
        let Some(tweak) = state.tweaking else {
            return;
        };
        let Some((dial, value, said)) = self.tweaked_value(state, tweak.what) else {
            return;
        };
        let at = cursor.unwrap_or(tweak.last);
        let y = at.y + BELOW;
        let (left, right) = dial.room(value);
        // Its ends, kept in view: past the edge, it goes on.
        let margin = 14.0;
        let ends = (
            (at.x - left).max(margin).min(at.x),
            (at.x + right).min(bounds.width - margin).max(at.x),
        );
        let track = Path::line(Point::new(ends.0, y), Point::new(ends.1, y));
        frame.stroke(&track, stroke(BACKGROUND, 5.0));
        frame.stroke(&track, stroke(Color { a: 0.6, ..EDGE }, 1.5));

        // Where the colour is itself, on the way from black to white.
        if tweak.what == Tweaking::Lightness {
            let x = at.x + (1.0 - value) / dial.per_px;
            if (ends.0..=ends.1).contains(&x) {
                let tick = Path::circle(Point::new(x, y), 3.0);
                frame.fill(
                    &tick,
                    Hsv {
                        value: 1.0,
                        alpha: 1.0,
                        ..self.brush
                    }
                    .to_color(),
                );
                frame.stroke(&tick, stroke(BACKGROUND, 1.0));
            }
        }

        // What each way does, at its end.
        let glyph = |frame: &mut Frame, x: f32, more: bool| {
            draw_glyph(frame, Point::new(x, y), tweak.what, more, self.brush);
        };
        if left > 0.5 {
            glyph(frame, ends.0 - 9.0, false);
        }
        if right > 0.5 {
            glyph(frame, ends.1 + 9.0, true);
        }

        // The handle, and which ways it can still go.
        let handle = Path::circle(Point::new(at.x, y), 4.5);
        frame.fill(&handle, EDGE);
        frame.stroke(&handle, stroke(BACKGROUND, 1.5));
        for (way, room) in [(-1.0f32, left), (1.0, right)] {
            if room <= 0.5 {
                continue;
            }
            let tip = Point::new(at.x + way * 12.0, y);
            let chevron = Path::new(|path| {
                path.move_to(tip + Vector::new(-way * 3.0, -3.5));
                path.line_to(tip);
                path.line_to(tip + Vector::new(-way * 3.0, 3.5));
            });
            frame.stroke(&chevron, stroke(BACKGROUND, 3.5));
            frame.stroke(&chevron, stroke(EDGE, 1.5));
        }

        // The value, under the handle.
        for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
            frame.fill_text(canvas::Text {
                content: said.clone(),
                position: Point::new(at.x + offset, y + 9.0 + offset),
                color,
                size: 12.0.into(),
                align_x: iced::widget::text::Alignment::Center,
                ..canvas::Text::default()
            });
        }
    }
}

/// What tweaking `what` towards less (or `more`) does, drawn small at `at`
/// (screen): darker or lighter, see-through or opaque, thinner or wider,
/// reaching less or further, the loop straight or turning.
fn draw_glyph(frame: &mut Frame, at: Point, what: Tweaking, more: bool, brush: Hsv) {
    let halo = |frame: &mut Frame, path: &Path, width: f32, color: Color| {
        frame.stroke(path, stroke(BACKGROUND, width + 2.5));
        frame.stroke(path, stroke(color, width));
    };
    match what {
        Tweaking::Lightness => {
            let value = if more { 2.0 } else { 0.0 };
            let dot = Path::circle(at, 4.5);
            frame.fill(
                &dot,
                Hsv {
                    value,
                    alpha: 1.0,
                    ..brush
                }
                .to_color(),
            );
            halo(frame, &dot, 1.0, EDGE);
        }
        Tweaking::Opacity => {
            let dot = Path::circle(at, 4.5);
            if more {
                frame.fill(&dot, EDGE);
            }
            halo(frame, &dot, 1.0, EDGE);
        }
        Tweaking::Width => {
            let line = Path::line(at + Vector::new(0.0, -5.0), at + Vector::new(0.0, 5.0));
            halo(frame, &line, if more { 4.0 } else { 1.0 }, EDGE);
        }
        Tweaking::Reach => {
            let ring = Path::circle(at, if more { 6.0 } else { 2.5 });
            halo(frame, &ring, 1.0, EDGE);
        }
        Tweaking::LoopTurn => {
            let line = if more {
                Path::new(|path| {
                    path.move_to(at + Vector::new(-5.0, 4.0));
                    path.line_to(at + Vector::new(0.0, -2.0));
                    path.line_to(at + Vector::new(5.0, 4.0));
                })
            } else {
                Path::line(at + Vector::new(-5.0, 0.0), at + Vector::new(5.0, 0.0))
            };
            halo(frame, &line, 1.5, EDGE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_as_far_as_there_is_room_reaches_each_end_exactly() {
        for dial in [
            Dial::WIDTH,
            Dial::LIGHTNESS,
            Dial::OPACITY,
            Dial::REACH,
            Dial::LOOP_TURN,
        ] {
            let value = if dial.scaled {
                (dial.min * dial.max).sqrt()
            } else {
                (dial.min + dial.max) / 2.0
            };
            let (left, right) = dial.room(value);
            assert!(left > 0.0 && right > 0.0, "{dial:?}");
            let close = |a: f32, b: f32| (a - b).abs() <= 1e-3 * dial.max;
            assert!(close(dial.moved(value, -left), dial.min), "{dial:?}");
            assert!(close(dial.moved(value, right), dial.max), "{dial:?}");
            // And no further.
            assert_eq!(dial.moved(value, right + 100.0), dial.max);
            assert_eq!(dial.room(dial.max).1, 0.0);
        }
    }
}
