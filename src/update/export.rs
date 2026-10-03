//! Exporting: to SVG, and (as chosen in its dialog) to PNG. With a
//! document size set, either can be clipped to it.

use crate::*;

/// What's exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Svg,
    Png,
}

#[derive(Debug, Clone)]
pub enum ExportMessage {
    /// Opens the export dialog for a format, or closes it.
    ShowExport(Option<Format>),
    /// Whether to export just the document (its size set), not all there is.
    ToPage(bool),
    /// How large the PNG is (pixels to a unit), and on what.
    PngScale(f32),
    PngBackdrop(raster::Backdrop),
    /// Exports as chosen: asks where.
    ExportSvg,
    ExportPng,
    Exported(Result<Option<PathBuf>, String>),
}

impl Tessera {
    /// What's exported: the document, if its size is set and that's chosen.
    pub(crate) fn export_page(&self) -> Option<page::Page> {
        self.layers.page().filter(|_| self.export.to_page)
    }

    pub(crate) fn update_export(&mut self, message: ExportMessage) -> Task<Message> {
        match message {
            ExportMessage::ShowExport(format) => {
                self.dialogs.menu = None;
                self.dialogs.export_open = format;
                Task::none()
            }
            ExportMessage::ToPage(to_page) => {
                self.export.to_page = to_page;
                Task::none()
            }
            ExportMessage::PngScale(scale) => {
                self.export.png_scale = scale;
                Task::none()
            }
            ExportMessage::PngBackdrop(backdrop) => {
                self.export.png_backdrop = backdrop;
                Task::none()
            }
            ExportMessage::ExportSvg => {
                self.dialogs.menu = None;
                self.dialogs.export_open = None;
                let svg = svg::export(&self.layers, self.export_page());
                Task::perform(save_svg(svg), |value| {
                    Message::Export(ExportMessage::Exported(value))
                })
            }
            ExportMessage::ExportPng => {
                self.dialogs.export_open = None;
                let scale = self.export.png_scale.max(0.1);
                // Seals as wide in the image whatever its scale.
                let seal = svg::SEAL_PIXELS / scale;
                let svg = svg::export_sealed(&self.layers, self.export_page(), seal);
                Task::perform(save_png(svg, scale, self.export.png_backdrop), |value| {
                    Message::Export(ExportMessage::Exported(value))
                })
            }
            ExportMessage::Exported(result) => {
                match result {
                    Ok(Some(path)) => {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        self.notify(format!("Exported {name}"), false);
                    }
                    Ok(None) => {}
                    Err(error) => self.notify(format!("Export failed: {error}"), true),
                }
                Task::none()
            }
        }
    }
}

/// How to export, as last chosen in the export dialog.
pub struct ExportSettings {
    /// Just the document (its size set), else all of the drawing.
    pub to_page: bool,
    /// How large a PNG is (times the drawing's size), and on what.
    pub png_scale: f32,
    pub png_backdrop: raster::Backdrop,
}

impl Default for ExportSettings {
    fn default() -> Self {
        // The page, at its size: larger only when chosen.
        ExportSettings {
            to_page: true,
            png_scale: 1.0,
            png_backdrop: raster::Backdrop::default(),
        }
    }
}
