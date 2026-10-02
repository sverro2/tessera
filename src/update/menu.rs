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
                self.menu = None;
                self.about = open;
                Task::none()
            }
            MenuMessage::ToggleMenu(menu) => {
                self.menu = (self.menu != Some(menu)).then_some(menu);
                Task::none()
            }
            MenuMessage::HoverMenu(menu) => {
                if self.menu.is_some() {
                    self.menu = Some(menu);
                }
                Task::none()
            }
            MenuMessage::CloseMenu => {
                self.menu = None;
                Task::none()
            }
        }
    }
}
