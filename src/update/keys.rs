//! The keyboard shortcuts: showing them, and setting them.

use crate::*;

#[derive(Debug, Clone)]
pub enum KeysMessage {
    /// Shows the keyboard shortcuts (F1), or closes them.
    ShowKeys(bool),
    /// Sets a key for an action: the next one pressed, instead of its key
    /// at that place (else as another); `None` stops.
    Rebind(Option<(Action, Option<usize>)>),
    /// Takes an action's key (at that place) away.
    Unbind(Action, usize),
    /// An action's keys back to out of the box; or all of them.
    ResetKey(Action),
    ResetKeys,
}

impl Tessera {
    pub(crate) fn update_keys(&mut self, message: KeysMessage) -> Task<Message> {
        match message {
            KeysMessage::ShowKeys(open) => {
                self.dialogs.keys_open = open;
                self.dialogs.rebinding = None;
                self.dialogs.menu = None;
                Task::none()
            }
            KeysMessage::Rebind(rebinding) => {
                self.dialogs.rebinding = rebinding;
                Task::none()
            }
            KeysMessage::Unbind(action, index) => {
                self.keys.unbind(action, index);
                self.dialogs.rebinding = None;
                Task::none()
            }
            KeysMessage::ResetKey(action) => {
                self.keys.reset(action);
                self.dialogs.rebinding = None;
                Task::none()
            }
            KeysMessage::ResetKeys => {
                self.keys.reset_all();
                self.dialogs.rebinding = None;
                self.notify("All shortcuts back to their defaults".into(), false);
                Task::none()
            }
        }
    }
}
