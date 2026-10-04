//! View > Snap settings: how dragged points snap.

use crate::*;

#[derive(Debug, Clone)]
pub enum SnapMessage {
    /// Opens the dialog (filled in from the settings), or closes it.
    Show(bool),
    /// The grid step, as typed.
    Grid(String),
    /// Keeps the settings as filled in.
    Apply,
}

impl Tessera {
    pub(crate) fn update_snap(&mut self, message: SnapMessage) -> Task<Message> {
        match message {
            SnapMessage::Show(open) => {
                self.dialogs.menu = None;
                self.dialogs.snap_dialog = open.then(|| format_step(self.snap.grid));
            }
            SnapMessage::Grid(text) => {
                if let Some(dialog) = &mut self.dialogs.snap_dialog {
                    *dialog = text;
                }
            }
            SnapMessage::Apply => {
                let step = self
                    .dialogs
                    .snap_dialog
                    .as_deref()
                    .and_then(|text| text.trim().parse::<f32>().ok());
                if step.is_some_and(|step| self.snap.set_grid(step)) {
                    self.dialogs.snap_dialog = None;
                    // The dot grid goes with it.
                    self.caches.backdrop.clear();
                }
            }
        }
        Task::none()
    }
}

/// A grid step as shown: whole if it is.
fn format_step(step: f32) -> String {
    if step.fract() == 0.0 {
        format!("{step:.0}")
    } else {
        step.to_string()
    }
}
