//! The shape mode's tools: proportional editing, and the current layer's
//! mirror.

use crate::*;

#[derive(Debug, Clone)]
pub enum ShapeMessage {
    /// Turns proportional editing on or off (O).
    ToggleProportional,
    /// Starts placing a mirror on the current layer (Ctrl+M), or stops.
    PlaceMirror(bool),
    /// Makes the current layer's mirror image actual geometry.
    ApplyMirror,
    /// Takes the current layer's mirror away.
    RemoveMirror,
    /// How far proportional editing reaches (screen px).
    Reach(f32),
}

impl Tessera {
    pub(crate) fn update_shape(&mut self, message: ShapeMessage) -> Task<Message> {
        match message {
            ShapeMessage::PlaceMirror(placing) => {
                self.shape.placing_mirror = placing;
                Task::none()
            }
            ShapeMessage::ApplyMirror => {
                let current = self.current();
                let Some(mirror) = self.layers.layer(current).and_then(|layer| layer.mirror) else {
                    return Task::none();
                };
                let before = self.layers.clone();
                let layer = self.layers.layer_mut(current).expect("current layer");
                if Arc::make_mut(&mut layer.document).apply(Edit::Mirror { mirror }) {
                    self.layers.set_mirror(current, None);
                    self.push_undo(before);
                } else {
                    self.notify(
                        "Can't apply the mirror: the drawing overlaps its mirror image".into(),
                        true,
                    );
                }
                Task::none()
            }
            ShapeMessage::RemoveMirror => {
                let before = self.layers.clone();
                self.layers.set_mirror(self.current(), None);
                self.push_undo(before);
                Task::none()
            }
            ShapeMessage::ToggleProportional => {
                self.shape.proportional = !self.shape.proportional;
                Task::none()
            }
            ShapeMessage::Reach(reach) => {
                self.shape.reach = reach.clamp(REACH.0, REACH.1);
                Task::none()
            }
        }
    }
}

/// The shape mode's tools: proportional editing, and placing a mirror.
pub struct Shaping {
    /// Proportional editing (shape mode): moving vertices takes those within
    /// `reach` (screen px, so zoomed out it reaches further) along, the less
    /// the further.
    pub proportional: bool,
    pub reach: f32,
    /// Placing a mirror on the current layer (Ctrl+M).
    pub placing_mirror: bool,
}

impl Default for Shaping {
    fn default() -> Self {
        Shaping {
            proportional: false,
            reach: 100.0,
            placing_mirror: false,
        }
    }
}
