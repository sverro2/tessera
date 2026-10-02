//! Exporting: to SVG, and (as chosen in its dialog) to PNG.

use crate::*;

#[derive(Debug, Clone)]
pub enum ExportMessage {
    ExportSvg,
    /// Opens the PNG export's dialog, or closes it.
    ShowExportPng(bool),
    /// How large the PNG is (pixels to a unit), and on what.
    PngScale(f32),
    PngBackdrop(raster::Backdrop),
    /// Exports the PNG as chosen: asks where.
    ExportPng,
    Exported(Result<Option<PathBuf>, String>),
}

impl Tessera {
    pub(crate) fn update_export(&mut self, message: ExportMessage) -> Task<Message> {
        match message {
            ExportMessage::ExportSvg => {
                self.menu = None;
                let svg = svg::export(&self.layers);
                Task::perform(save_svg(svg), |value| {
                    Message::Export(ExportMessage::Exported(value))
                })
            }
            ExportMessage::ShowExportPng(open) => {
                self.menu = None;
                self.png_open = open;
                Task::none()
            }
            ExportMessage::PngScale(scale) => {
                self.png_scale = scale;
                Task::none()
            }
            ExportMessage::PngBackdrop(backdrop) => {
                self.png_backdrop = backdrop;
                Task::none()
            }
            ExportMessage::ExportPng => {
                self.png_open = false;
                let svg = svg::export(&self.layers);
                Task::perform(
                    save_png(svg, self.png_scale.max(0.1), self.png_backdrop),
                    |value| Message::Export(ExportMessage::Exported(value)),
                )
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
