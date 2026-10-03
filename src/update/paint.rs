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
                    self.paint.brush = Brush::Color(color);
                    self.paint.hsv = Hsv::from_color(color, self.paint.hsv.hue);
                }
                self.paint.hex = typed;
                Task::none()
            }
            PaintMessage::WheelPicked(hsv) => {
                let color = hsv.to_color();
                self.paint.hsv = hsv;
                self.paint.brush = Brush::Color(color);
                self.paint.hex = paint::to_hex(color);
                Task::none()
            }
            PaintMessage::TogglePicking => {
                if self.mode == Mode::Paint {
                    self.paint.picking = !self.paint.picking;
                } else {
                    self.paint.picking = true;
                    self.mode = Mode::Paint;
                }
                Task::none()
            }
            PaintMessage::PaintEverything(all_layers) => {
                self.paint_everything(all_layers);
                Task::none()
            }
            PaintMessage::SetTarget(target) => {
                self.paint.target = target;
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
                if self.paint.fading_from.is_none() {
                    self.paint.fading_from = Some(self.layers.clone());
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
                if let Some(before) = self.paint.fading_from.take() {
                    self.push_undo(before);
                }
                Task::none()
            }
            PaintMessage::EdgeWidth(width) => {
                self.paint.edge_width = width;
                Task::none()
            }
        }
    }
}

/// The brush and painting with it.
pub struct Painting {
    /// What painting a face does, in the paint mode.
    pub brush: Brush,
    /// Colours painted with lately, most recent first.
    pub recent: Vec<Color>,
    /// The paint panel's colour field: as typed, or the colour picked.
    pub hex: String,
    /// The brush's colour on the colour wheel (keeping the hue of greys).
    pub hsv: Hsv,
    /// Whether painting paints faces or edges.
    pub target: Target,
    /// How wide painted edges are (world units).
    pub edge_width: f32,
    /// The layers before the fade width slider was dragged: the drag is one
    /// undo step.
    pub fading_from: Option<Layers>,
    /// Whether the next click on the canvas picks up a brush (the pipette).
    pub picking: bool,
}

impl Default for Painting {
    fn default() -> Self {
        let brush = Brush::default();
        let color = brush.color().unwrap_or(Color::WHITE);
        Painting {
            brush,
            recent: Vec::new(),
            hex: paint::to_hex(color),
            hsv: Hsv::from_color(color, 0.0),
            target: Target::default(),
            edge_width: 2.0,
            fading_from: None,
            picking: false,
        }
    }
}
