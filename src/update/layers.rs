//! The layers panel: picking, adding, grouping, ordering (by dragging),
//! naming and hiding layers.

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
    /// The selected one in a new group.
    Group,
    Ungroup,
    Remove,
    /// Shows a line's name as a field to type in.
    StartRename(NodeId),
    EndRename,
    Rename(String),
    ToggleVisible(NodeId),
    ToggleExpanded(NodeId),
}

impl Tessera {
    pub(crate) fn layer_action(&mut self, action: LayerAction) -> Task<Message> {
        let before = self.layers.clone();
        let selected = self.selected;
        if !matches!(action, LayerAction::Rename(_)) {
            self.renaming = None;
        }
        let changed = match action {
            LayerAction::Press(id) => {
                if self.naming != Some(id) {
                    self.naming = None;
                }
                self.select_layer(id);
                self.dragging = Some((id, None));
                return Task::none();
            }
            LayerAction::Hover(id, y) => {
                if let Some((dragged, place)) = &mut self.dragging {
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
                if let Some((_, place)) = &mut self.dragging {
                    *place = Some(Place::Last);
                }
                return Task::none();
            }
            LayerAction::Leave => {
                if let Some((_, place)) = &mut self.dragging {
                    *place = None;
                }
                return Task::none();
            }
            LayerAction::Drop => match self.dragging.take() {
                Some((id, Some(place))) => self.layers.move_to(id, place),
                _ => return Task::none(),
            },
            LayerAction::ToggleExpanded(id) => {
                self.layers.toggle_expanded(id);
                return Task::none();
            }
            LayerAction::StartRename(id) => {
                self.dragging = None;
                self.select_layer(id);
                self.naming = Some(id);
                return Task::batch([
                    iced::widget::operation::focus(NAME_FIELD),
                    iced::widget::operation::select_all(NAME_FIELD),
                ]);
            }
            LayerAction::EndRename => {
                self.naming = None;
                return Task::none();
            }
            LayerAction::Rename(name) => {
                self.layers.rename(selected, name);
                // Typing a name is one step.
                if self.renaming != Some(selected) {
                    self.renaming = Some(selected);
                    self.push_undo(before);
                }
                return Task::none();
            }
            LayerAction::Add => {
                let id = self.layers.add_layer(selected);
                self.current = id;
                self.selected = id;
                true
            }
            LayerAction::Group => match self.layers.group(selected) {
                Some(group) => {
                    self.selected = group;
                    true
                }
                None => false,
            },
            LayerAction::Ungroup => {
                let ungrouped = self.layers.ungroup(selected);
                if ungrouped {
                    self.selected = self.current();
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
