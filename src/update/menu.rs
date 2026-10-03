//! The menus, and the about box.

use crate::*;

#[derive(Debug, Clone)]
pub enum MenuMessage {
    ToggleMenu(Menu),
    /// The pointer moved onto a menu's button.
    HoverMenu(Menu),
    CloseMenu,
    /// Show (`true`) or close the about box.
    About(bool),
}

impl Tessera {
    pub(crate) fn update_menu(&mut self, message: MenuMessage) -> Task<Message> {
        match message {
            MenuMessage::About(open) => {
                self.dialogs.menu = None;
                self.dialogs.about = open;
                Task::none()
            }
            MenuMessage::ToggleMenu(menu) => {
                self.dialogs.menu = (self.dialogs.menu != Some(menu)).then_some(menu);
                Task::none()
            }
            MenuMessage::HoverMenu(menu) => {
                if self.dialogs.menu.is_some() {
                    self.dialogs.menu = Some(menu);
                }
                Task::none()
            }
            MenuMessage::CloseMenu => {
                self.dialogs.menu = None;
                Task::none()
            }
        }
    }
}
