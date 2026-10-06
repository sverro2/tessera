//! Laying a gradient over the current layer (in the paint mode): its kind
//! and stops, its line (drawn on the canvas), the layer shown as it would
//! colour it, applying it; and the presets kept between runs.

use crate::gradient::{self, Gradient, Kind, Presets};
use crate::gradient_bar::BarEvent;
use crate::layers::NodeId;
use crate::*;

#[derive(Debug, Clone)]
pub enum GradientMessage {
    /// Starts laying a gradient (in the paint mode), or stops.
    Open(bool),
    SetKind(Kind),
    /// On the gradient bar: a stop picked, moved or added.
    Bar(BarEvent),
    /// Takes the picked stop away (two are left at least).
    RemoveStop,
    /// The other way round.
    Reverse,
    /// The picked stop's colour, from the wheel or its sliders.
    WheelPicked(Hsv),
    HexTyped(String),
    /// Colours the faces (or edges) as shown: one undo step.
    Apply,
    /// Colours them as shown (if there's a line), and stops laying it.
    Done,
    /// The name to keep it under, as typed.
    NameTyped(String),
    /// Keeps it as a preset, under the name typed.
    SavePreset,
    LoadPreset(usize),
    RemovePreset(usize),
}

impl Tessera {
    pub(crate) fn update_gradient(&mut self, message: GradientMessage) -> Task<Message> {
        let gradients = &mut self.gradients;
        match message {
            GradientMessage::Open(open) => {
                gradients.open = open;
                if open {
                    gradients.pick(gradients.selected);
                    self.mode = Mode::Paint;
                    self.paint.picking = false;
                }
            }
            GradientMessage::SetKind(kind) => gradients.gradient.kind = kind,
            GradientMessage::Bar(BarEvent::Pick(i)) => gradients.pick(i),
            GradientMessage::Bar(BarEvent::Move(at)) => {
                let i = gradients.gradient.move_stop(gradients.selected, at);
                gradients.selected = i;
            }
            GradientMessage::Bar(BarEvent::Add(at)) => {
                let i = gradients.gradient.add_stop(at);
                gradients.pick(i);
            }
            GradientMessage::RemoveStop => {
                if gradients.gradient.remove_stop(gradients.selected) {
                    gradients.pick(gradients.selected.saturating_sub(1));
                }
            }
            GradientMessage::Reverse => {
                gradients.gradient.reverse();
                let last = gradients.gradient.stops().len() - 1;
                gradients.pick(last - gradients.selected.min(last));
            }
            GradientMessage::WheelPicked(hsv) => {
                let color = hsv.to_color();
                gradients.gradient.set_color(gradients.selected, color);
                gradients.hsv = hsv;
                gradients.hex = paint::to_hex(color);
            }
            GradientMessage::HexTyped(typed) => {
                if let Some(color) = paint::from_hex(&typed) {
                    gradients.gradient.set_color(gradients.selected, color);
                    gradients.hsv = Hsv::from_color(color, gradients.hsv.hue);
                }
                gradients.hex = typed;
            }
            GradientMessage::Apply => self.apply_gradient(true),
            GradientMessage::Done => {
                self.apply_gradient(false);
                self.gradients.open = false;
            }
            GradientMessage::NameTyped(name) => gradients.name = name,
            GradientMessage::SavePreset => {
                let name = gradients.name.trim();
                if !name.is_empty() {
                    gradients.presets.save(name, &gradients.gradient);
                    gradients.name.clear();
                }
            }
            GradientMessage::LoadPreset(i) => {
                if let Some(preset) = gradients.presets.list().get(i) {
                    gradients.gradient = preset.gradient.clone();
                    gradients.pick(0);
                }
            }
            GradientMessage::RemovePreset(i) => gradients.presets.remove(i),
        }
        Task::none()
    }

    /// Colours the current layer's faces (or edges) as the gradient shows
    /// them: one undo step; then the line is gone, to draw the next. If
    /// that changes nothing, says so if `told`.
    fn apply_gradient(&mut self, told: bool) {
        let Some(line) = self.gradients.line else {
            return;
        };
        let edit = gradient::edit(
            self.document(),
            &self.gradients.gradient,
            line,
            self.paint.target,
            self.paint.edge_width,
        );
        let Some(edit) = edit else {
            if !told {
                return;
            }
            self.notify(
                "Nothing to colour: draw the line across shapes on this layer".into(),
                false,
            );
            return;
        };
        let before = self.layers.clone();
        let layer = self
            .layers
            .layer_mut(self.current())
            .expect("current layer");
        if Arc::make_mut(&mut layer.document).apply(edit) {
            self.push_undo(before);
            self.gradients.line = None;
        }
    }

    /// After each message: the current layer as the gradient would colour
    /// it, worked out again if anything it depends on changed.
    pub(crate) fn refresh_gradient(&mut self) {
        let line = self
            .gradients
            .line
            .filter(|_| self.gradients.open && self.mode == Mode::Paint);
        let Some(line) = line else {
            self.gradients.preview = None;
            return;
        };
        let key = PreviewKey {
            layer: self.current(),
            revision: self.document().revision(),
            gradient: self.gradients.gradient.clone(),
            line,
            target: self.paint.target,
            width: self.paint.edge_width,
        };
        if self
            .gradients
            .preview
            .as_ref()
            .is_some_and(|(was, _)| *was == key)
        {
            return;
        }
        let document = self.document();
        let preview = gradient::edit(document, &key.gradient, line, key.target, key.width)
            .and_then(|edit| document.preview(&[edit]))
            .map(|(preview, _)| preview);
        self.gradients.preview = Some((key, preview));
    }

    /// The current layer as the gradient would colour it, if it's being
    /// laid and changes anything.
    pub(crate) fn gradient_preview(&self) -> Option<&Document> {
        self.gradients.preview.as_ref()?.1.as_ref()
    }
}

/// Laying a gradient.
pub struct Gradients {
    /// Whether one's being laid: the canvas draws its line, the paint
    /// panel shows it.
    pub open: bool,
    pub gradient: Gradient,
    /// The stop picked, to colour: its colour on the wheel (keeping the
    /// hue of greys), and as typed.
    pub selected: usize,
    pub hsv: Hsv,
    pub hex: String,
    /// Its line (world): from its start to its end; none drawn yet.
    pub line: Option<(iced::Point, iced::Point)>,
    pub presets: Presets,
    /// The name to keep it under, as typed.
    pub name: String,
    /// The current layer as it would colour it (`None`: nothing would
    /// change), and what that was worked out from.
    preview: Option<(PreviewKey, Option<Document>)>,
}

impl Gradients {
    /// As it starts out, with these presets.
    pub fn with(presets: Presets) -> Self {
        Gradients {
            presets,
            ..Gradients::default()
        }
    }

    /// Picks stop `i` (kept to those there are): its colour to change.
    fn pick(&mut self, i: usize) {
        let stops = self.gradient.stops();
        self.selected = i.min(stops.len() - 1);
        let color = stops[self.selected].color;
        self.hsv = Hsv::from_color(color, self.hsv.hue);
        self.hex = paint::to_hex(color);
    }
}

impl Default for Gradients {
    fn default() -> Self {
        let mut gradients = Gradients {
            open: false,
            gradient: Gradient::default(),
            selected: 0,
            hsv: Hsv::default(),
            hex: String::new(),
            line: None,
            presets: Presets::default(),
            name: String::new(),
            preview: None,
        };
        gradients.pick(0);
        gradients
    }
}

/// What the preview was worked out from.
#[derive(Debug, PartialEq)]
struct PreviewKey {
    layer: NodeId,
    revision: u64,
    gradient: Gradient,
    line: (iced::Point, iced::Point),
    target: Target,
    width: f32,
}
