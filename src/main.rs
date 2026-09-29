mod camera;
mod document;
mod editor;
mod geometry;
mod svg;

use std::path::PathBuf;

use iced::widget::{button, canvas, column, container, row, space, stack, text};
use iced::{Center, Element, Fill, Task, Theme};

use camera::Camera;
use document::Document;

pub fn main() -> iced::Result {
    iced::application(Tessera::default, Tessera::update, Tessera::view)
        .title("Tessera")
        .theme(Theme::TokyoNight)
        .centered()
        .run()
}

#[derive(Default)]
struct Tessera {
    /// The data. Only ever changed through `Document::apply`.
    document: Document,
    /// View state, independent of the data.
    camera: Camera,
    /// The drawn document; cleared when it or the camera changes.
    cache: canvas::Cache,
    /// The drawn background grid; cleared when the camera changes.
    grid: canvas::Cache,
    status: String,
}

#[derive(Debug, Clone)]
enum Message {
    Editor(editor::Message),
    ResetView,
    ExportSvg,
    Exported(Result<Option<PathBuf>, String>),
}

impl Tessera {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Editor(editor::Message::Edit(edits)) => {
                self.document.apply_all(&edits);
                self.cache.clear();
                return Task::none();
            }
            Message::Editor(editor::Message::Pan(delta)) => {
                self.camera.pan += delta;
            }
            Message::Editor(editor::Message::Zoom { anchor, factor }) => {
                self.camera.zoom_at(anchor, factor);
            }
            Message::Editor(editor::Message::Rotate { anchor, angle }) => {
                self.camera.rotate_at(anchor, angle);
            }
            Message::ResetView => {
                self.camera = Camera::default();
            }
            Message::ExportSvg => {
                let svg = svg::export(&self.document);
                return Task::perform(save_svg(svg), Message::Exported);
            }
            Message::Exported(result) => {
                self.status = match result {
                    Ok(Some(path)) => format!("Exported to {}", path.display()),
                    Ok(None) => String::new(),
                    Err(error) => format!("Export failed: {error}"),
                };
                return Task::none();
            }
        }

        // Everything else above moves the camera.
        self.cache.clear();
        self.grid.clear();
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let toolbar = row![
            button("Export SVG").on_press_maybe(
                (!self.document.is_empty()).then_some(Message::ExportSvg)
            ),
            button("Reset view").on_press(Message::ResetView),
            text(format!(
                "{:.0}% · {:.0}°",
                self.camera.zoom * 100.0,
                self.camera.rotation.to_degrees()
            )),
            space::horizontal(),
            text(&self.status),
            text("Drag corner: move · Drag edge: extend · Drag blank: new tri · C: cut edge · D: delete tri · Middle-drag: pan · Wheel: zoom · Shift+wheel: rotate")
                .size(12),
        ]
        .spacing(10)
        .padding(8)
        .align_y(Center);

        let stats = container(
            text(format!(
                "{} vertices · {} edges · {} faces",
                self.document.vertex_count(),
                self.document.unique_edges().len(),
                self.document.triangle_ids().len(),
            ))
            .size(12),
        )
        .padding([4, 8])
        .style(container::dark);

        column![
            container(toolbar).width(Fill).style(container::dark),
            stack![
                editor::view(&self.document, self.camera, &self.cache, &self.grid)
                    .map(Message::Editor),
                container(stats)
                    .align_right(Fill)
                    .align_bottom(Fill)
                    .padding(10),
            ],
        ]
        .into()
    }
}

async fn save_svg(svg: String) -> Result<Option<PathBuf>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("SVG", &["svg"])
        .set_file_name("tessera.svg")
        .save_file()
        .await
    else {
        return Ok(None);
    };

    let path = file.path().to_path_buf();
    std::fs::write(&path, svg).map_err(|e| e.to_string())?;

    Ok(Some(path))
}
