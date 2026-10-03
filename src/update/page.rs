//! The document size (Edit > Document size): its dialog, and setting it.

use crate::page::{Page, Preset};
use crate::*;

/// The document size dialog, as filled in so far.
#[derive(Debug, Clone)]
pub struct PageDialog {
    pub preset: Preset,
    /// In pixels, as typed (so half-typed numbers aren't overwritten).
    pub width: String,
    pub height: String,
    /// Whether to put it around the drawing (else where it was).
    pub centre: bool,
}

impl PageDialog {
    /// The size typed in, if it will do.
    pub fn size(&self) -> Option<iced::Size> {
        let side = |text: &str| text.trim().parse::<f32>().ok();
        let size = iced::Size::new(side(&self.width)?, side(&self.height)?);
        Page::fits(size).then_some(size)
    }
}

#[derive(Debug, Clone)]
pub enum PageMessage {
    /// Opens the dialog (filled in from the size set, if any), or closes it.
    Show(bool),
    Preset(Preset),
    Width(String),
    Height(String),
    /// Turns it the other way: portrait or landscape.
    Turn,
    Centre(bool),
    /// Sets the size as filled in; one undo step.
    Apply,
    /// No document size; one undo step.
    Remove,
}

impl Tessera {
    pub(crate) fn update_page(&mut self, message: PageMessage) -> Task<Message> {
        match message {
            PageMessage::Show(open) => {
                self.menu = None;
                self.page_dialog = open.then(|| {
                    let page = self.layers.page();
                    let (width, height) = page.map_or((2480, 3508), |page| {
                        (
                            page.size.width.round() as u32,
                            page.size.height.round() as u32,
                        )
                    });
                    PageDialog {
                        preset: Preset::of(width, height),
                        width: width.to_string(),
                        height: height.to_string(),
                        centre: page.is_none(),
                    }
                });
            }
            PageMessage::Preset(preset) => {
                if let Some(dialog) = &mut self.page_dialog {
                    dialog.preset = preset;
                    // As it comes (turning it is up to the orientation).
                    if let Some((w, h)) = preset.pixels() {
                        dialog.width = w.to_string();
                        dialog.height = h.to_string();
                    }
                }
            }
            PageMessage::Width(width) => {
                if let Some(dialog) = &mut self.page_dialog {
                    dialog.width = width;
                    dialog.preset = preset_of(dialog);
                }
            }
            PageMessage::Height(height) => {
                if let Some(dialog) = &mut self.page_dialog {
                    dialog.height = height;
                    dialog.preset = preset_of(dialog);
                }
            }
            PageMessage::Turn => {
                if let Some(dialog) = &mut self.page_dialog {
                    std::mem::swap(&mut dialog.width, &mut dialog.height);
                }
            }
            PageMessage::Centre(centre) => {
                if let Some(dialog) = &mut self.page_dialog {
                    dialog.centre = centre;
                }
            }
            PageMessage::Apply => {
                let Some(dialog) = self.page_dialog.take() else {
                    return Task::none();
                };
                let Some(size) = dialog.size() else {
                    self.page_dialog = Some(dialog);
                    return Task::none();
                };
                let center = match self.layers.page() {
                    Some(page) if !dialog.centre => page.center,
                    _ => self.middle(),
                };
                // Its corner on the grid (which stays put), so it lines up
                // with it.
                let step = self.snap.grid;
                let on_grid = |middle: f32, side: f32| {
                    ((middle - side / 2.0) / step).round() * step + side / 2.0
                };
                let center = iced::Point::new(
                    on_grid(center.x, size.width),
                    on_grid(center.y, size.height),
                );
                let page = Page { center, size };
                self.set_page(Some(page));
                // All of it in view, clear of the panels over the canvas.
                const MARGIN: f32 = 60.0;
                let canvas = Self::canvas_size(self.window_size.unwrap_or(WINDOW_SIZE));
                self.camera = self.camera.framing(&page.corners(), canvas, MARGIN);
                self.caches.grid.clear();
            }
            PageMessage::Remove => {
                self.page_dialog = None;
                self.set_page(None);
            }
        }
        Task::none()
    }

    /// The middle of the drawing (what's shown), else of the view.
    fn middle(&self) -> iced::Point {
        if self
            .layers
            .layers()
            .iter()
            .any(|layer| self.layers.shown(layer.id) && !layer.document.is_empty())
        {
            let (corner, size) = svg::area(&self.layers, None);
            return iced::Point::new(corner.x + size.width / 2.0, corner.y + size.height / 2.0);
        }
        let canvas = Self::canvas_size(self.window_size.unwrap_or(WINDOW_SIZE));
        self.camera
            .to_world(iced::Point::new(canvas.width / 2.0, canvas.height / 2.0))
    }

    /// Sets the document size, as an undo step if it changes.
    fn set_page(&mut self, page: Option<Page>) {
        if self.layers.page() == page {
            return;
        }
        let before = self.layers.clone();
        self.layers.set_page(page);
        self.push_undo(before);
        self.caches.grid.clear();
    }
}

/// The preset the size typed in is, if any.
fn preset_of(dialog: &PageDialog) -> Preset {
    match dialog.size() {
        Some(size) if size.width.fract() == 0.0 && size.height.fract() == 0.0 => {
            Preset::of(size.width as u32, size.height as u32)
        }
        _ => Preset::Custom,
    }
}
