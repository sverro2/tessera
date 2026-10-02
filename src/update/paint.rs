//! The brush and painting: its colour, what it paints, how wide, and the
//! current layer's painted look.

use crate::*;

#[derive(Debug, Clone)]
pub enum PaintMessage {
    PickBrush(Brush),
    HexTyped(String),
    /// Picked on the colour wheel, or its brightness slider.
    WheelPicked(Hsv),
    SetTarget(Target),
    /// Paint all of the current layer (or, `true`, all layers) with the
    /// brush: every face, or every edge.
    PaintEverything(bool),
    /// Turns the pipette on (in the paint mode) or off.
    TogglePicking,
    EdgeWidth(f32),
    /// Whether the current layer's painted faces or edges (whichever is
    /// being painted) blend into their neighbours.
    Crossfade(bool),
    /// How wide the current layer's faces fade into each other; and the
    /// slider let go.
    FadeWidth(f32),
    FadeWidthDone,
    /// Whether the current layer's edges show when painting (non-destructive).
    ShowEdges(bool),
}

impl Tessera {
    pub(crate) fn update_paint(&mut self, message: PaintMessage) -> Task<Message> {
        match message {
            PaintMessage::PickBrush(brush) => {
                self.set_brush(brush);
                Task::none()
            }
            PaintMessage::HexTyped(typed) => {
                if let Some(color) = paint::from_hex(&typed) {
                    self.brush = Brush::Color(color);
                    self.hsv = Hsv::from_color(color, self.hsv.hue);
                }
                self.hex = typed;
                Task::none()
            }
            PaintMessage::WheelPicked(hsv) => {
                let color = hsv.to_color();
                self.hsv = hsv;
                self.brush = Brush::Color(color);
                self.hex = paint::to_hex(color);
                Task::none()
            }
            PaintMessage::TogglePicking => {
                if self.mode == Mode::Paint {
                    self.picking = !self.picking;
                } else {
                    self.picking = true;
                    self.mode = Mode::Paint;
                }
                Task::none()
            }
            PaintMessage::PaintEverything(all_layers) => {
                self.paint_everything(all_layers);
                Task::none()
            }
            PaintMessage::SetTarget(target) => {
                self.target = target;
                Task::none()
            }
            PaintMessage::Crossfade(on) => {
                let current = self.current();
                let before = self.layers.clone();
                let crossfade = self.layers.layer(current).expect("current layer").crossfade;
                self.layers.set_crossfade(
                    current,
                    layers::Crossfade {
                        edges: on,
                        ..crossfade
                    },
                );
                self.push_undo(before);
                Task::none()
            }
            PaintMessage::FadeWidth(width) => {
                let current = self.current();
                if self.fading_from.is_none() {
                    self.fading_from = Some(self.layers.clone());
                }
                let crossfade = self.layers.layer(current).expect("current layer").crossfade;
                self.layers
                    .set_crossfade(current, layers::Crossfade { width, ..crossfade });
                Task::none()
            }
            PaintMessage::ShowEdges(show) => {
                let before = self.layers.clone();
                self.layers.set_show_edges(self.current(), show);
                self.push_undo(before);
                Task::none()
            }
            PaintMessage::FadeWidthDone => {
                if let Some(before) = self.fading_from.take() {
                    self.push_undo(before);
                }
                Task::none()
            }
            PaintMessage::EdgeWidth(width) => {
                self.edge_width = width;
                Task::none()
            }
        }
    }
}
