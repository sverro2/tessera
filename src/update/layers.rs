//! The layers panel: picking, adding, grouping, ordering (by dragging),
//! naming, hiding and isolating layers.

use crate::*;

/// Something done in the layers panel.
#[derive(Debug, Clone)]
pub enum LayerAction {
    /// Pressed on a line: selects it, and starts dragging it.
    Press(NodeId),
    /// While dragging, over this line, this far down it (0 to 1).
    Hover(NodeId, f32),
    /// While dragging, below all lines.
    HoverEnd,
    /// While dragging, left the list.
    Leave,
    /// Let go (anywhere): moves what's dragged where it would go.
    Drop,
    /// A new layer in front of the selected one.
    Add,
    /// A copy of the selected layer (or group) in front of it.
    Duplicate,
    /// The selected one in a new group.
    Group,
    Ungroup,
    Remove,
    /// The pointer moved onto a line, or off it: what's on that layer (or
    /// group) lights up on the canvas meanwhile.
    Point(NodeId),
    Unpoint(NodeId),
    /// Shows a line's name as a field to type in.
    StartRename(NodeId),
    EndRename,
    Rename(String),
    ToggleVisible(NodeId),
    /// Isolates a line, or no longer: while any are, only they are in
    /// view.
    ToggleIsolated(NodeId),
    ToggleExpanded(NodeId),
}

impl Tessera {
    pub(crate) fn layer_action(&mut self, action: LayerAction) -> Task<Message> {
        let before = self.layers.clone();
        // None selected (yet, or any more): the current layer.
        let selected = if self.layers.contains(self.layers_panel.selected) {
            self.layers_panel.selected
        } else {
            self.current()
        };
        if !matches!(action, LayerAction::Rename(_)) {
            self.layers_panel.renaming = None;
        }
        let changed = match action {
            LayerAction::Point(id) => {
                self.layers_panel.pointed = Some(id);
                return Task::none();
            }
            // (Only if still on it: onto the next one may come first.)
            LayerAction::Unpoint(id) => {
                if self.layers_panel.pointed == Some(id) {
                    self.layers_panel.pointed = None;
                }
                return Task::none();
            }
            LayerAction::Press(id) => {
                if self.layers_panel.naming != Some(id) {
                    self.layers_panel.naming = None;
                }
                self.select_layer(id);
                self.layers_panel.dragging = Some((id, None));
                return Task::none();
            }
            LayerAction::Hover(id, y) => {
                if let Some((dragged, place)) = &mut self.layers_panel.dragging {
                    let group = self
                        .layers
                        .rows()
                        .iter()
                        .find(|row| row.id == id)
                        .map(|row| row.group);
                    *place = match group {
                        None => None,
                        // A group: its middle (or, expanded, its lower part:
                        // where its contents follow) puts it in.
                        Some(Some(expanded)) => Some(if y < 0.25 {
                            Place::Before(id)
                        } else if y > 0.75 && !expanded {
                            Place::After(id)
                        } else {
                            Place::Into(id)
                        }),
                        Some(None) => Some(if y < 0.5 {
                            Place::Before(id)
                        } else {
                            Place::After(id)
                        }),
                    }
                    // Onto itself: nowhere.
                    .filter(|_| id != *dragged);
                }
                return Task::none();
            }
            LayerAction::HoverEnd => {
                if let Some((_, place)) = &mut self.layers_panel.dragging {
                    *place = Some(Place::Last);
                }
                return Task::none();
            }
            LayerAction::Leave => {
                if let Some((_, place)) = &mut self.layers_panel.dragging {
                    *place = None;
                }
                return Task::none();
            }
            LayerAction::Drop => match self.layers_panel.dragging.take() {
                Some((id, Some(place))) => self.layers.move_to(id, place),
                _ => return Task::none(),
            },
            // A way of looking, not a change to the drawing.
            LayerAction::ToggleIsolated(id) => {
                if !self.layers_panel.isolated.remove(&id) {
                    self.layers_panel.isolated.insert(id);
                }
                return Task::none();
            }
            LayerAction::ToggleExpanded(id) => {
                self.layers.toggle_expanded(id);
                return Task::none();
            }
            LayerAction::StartRename(id) => {
                self.layers_panel.dragging = None;
                self.select_layer(id);
                self.layers_panel.naming = Some(id);
                return Task::batch([
                    iced::widget::operation::focus(NAME_FIELD),
                    iced::widget::operation::select_all(NAME_FIELD),
                ]);
            }
            LayerAction::EndRename => {
                self.layers_panel.naming = None;
                return Task::none();
            }
            LayerAction::Rename(name) => {
                self.layers.rename(selected, name);
                // Typing a name is one step.
                if self.layers_panel.renaming != Some(selected) {
                    self.layers_panel.renaming = Some(selected);
                    self.push_undo(before);
                }
                return Task::none();
            }
            LayerAction::Add => {
                let id = self.layers.add_layer(selected);
                self.current = id;
                self.layers_panel.selected = id;
                true
            }
            // The copy in front, and worked on (the original kept as it is).
            LayerAction::Duplicate => match self.layers.duplicate(selected) {
                Some(copy) => {
                    // Their background images too (the image itself shared).
                    let pairs: Vec<_> = self
                        .layers
                        .layers_of(selected)
                        .into_iter()
                        .zip(self.layers.layers_of(copy))
                        .collect();
                    for (original, copied) in pairs {
                        if let Some(background) = self.backgrounds.images.get(&original).cloned() {
                            self.backgrounds.images.insert(copied, background);
                        }
                    }
                    self.layers_panel.selected = copy;
                    if let Some(layer) = self.layers.first_layer_of(copy) {
                        self.current = layer;
                    }
                    true
                }
                None => false,
            },
            LayerAction::Group => match self.layers.group(selected) {
                Some(group) => {
                    self.layers_panel.selected = group;
                    true
                }
                None => false,
            },
            LayerAction::Ungroup => {
                let ungrouped = self.layers.ungroup(selected);
                if ungrouped {
                    self.layers_panel.selected = self.current();
                }
                ungrouped
            }
            LayerAction::Remove => self.layers.remove(selected),
            LayerAction::ToggleVisible(id) => {
                let visible = self
                    .layers
                    .rows()
                    .iter()
                    .find(|row| row.id == id)
                    .is_some_and(|row| row.visible);
                self.layers.set_visible(id, !visible);
                true
            }
        };
        if changed {
            self.push_undo(before);
            self.layers_replaced();
        }
        Task::none()
    }
}

/// The layers panel: what's selected, named, dragged, pointed at and isolated in it.
#[derive(Default)]
pub struct LayersPanel {
    /// The layer or group selected in the layers panel.
    pub selected: NodeId,
    /// The layer or group whose name is being typed: its renaming is one
    /// undo step.
    pub renaming: Option<NodeId>,
    /// The layer or group whose name is shown as a field to type in.
    pub naming: Option<NodeId>,
    /// A layer or group being dragged in the layers panel, and where
    /// dropping it would put it.
    pub dragging: Option<(NodeId, Option<Place>)>,
    /// The layer (or group) pointed at in the layers panel: lit up on the
    /// canvas.
    pub pointed: Option<NodeId>,
    /// The layers and groups isolated in the layers panel: while any are,
    /// only they (and what's in them) are in view. Not part of the drawing.
    pub isolated: HashSet<NodeId>,
}
