//! Files: starting a new drawing, opening one (lately opened, or dropped
//! on the window, too), saving, and asking first when changes would be lost.
//! Images dropped on the window become the current layer's background.

use crate::*;

#[derive(Debug, Clone)]
pub enum FileMessage {
    New,
    Open,
    Save,
    SaveAs,
    /// Do it: confirmed, or there was nothing to lose.
    Replace(Replace),
    Confirm(Choice),
    /// A file was picked and read: its path and text.
    Opened(Result<Option<(PathBuf, Vec<u8>)>, String>),
    /// Open a recently opened file.
    OpenRecent(PathBuf),
    /// A recent file couldn't be read (it's gone, say): why.
    RecentFailed(PathBuf, String),
    ClearRecent,
    /// A file dragged over the window (`None`: dragged off it again).
    Hovered(Option<PathBuf>),
    /// A file dropped on the window.
    Dropped(PathBuf),
    /// Saving this revision finished (to this path, unless cancelled);
    /// then this was to happen.
    Saved(Result<Option<PathBuf>, String>, Versions, Option<Replace>),
}

impl Tessera {
    pub(crate) fn update_file(&mut self, message: FileMessage) -> Task<Message> {
        match message {
            FileMessage::New => {
                self.dialogs.menu = None;
                // A blank canvas is new already.
                if self.is_blank() {
                    return Task::none();
                }
                self.replace(Replace::New)
            }
            FileMessage::Open => {
                self.dialogs.menu = None;
                self.replace(Replace::Open)
            }
            FileMessage::OpenRecent(path) => {
                self.dialogs.menu = None;
                self.replace(Replace::OpenPath(path))
            }
            FileMessage::Hovered(path) => {
                self.dropping = path;
                Task::none()
            }
            FileMessage::Dropped(path) => {
                self.dropping = None;
                match Dropped::of(&path) {
                    Dropped::Drawing => self.replace(Replace::OpenPath(path)),
                    Dropped::Image => Task::perform(
                        async move { std::fs::read(&path).map(Some).map_err(|e| e.to_string()) },
                        |read| Message::Background(BackgroundMessage::BackgroundPicked(read)),
                    ),
                    Dropped::Other => {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        self.notify(format!("Can't use {name}: not a drawing or an image"), true);
                        Task::none()
                    }
                }
            }
            FileMessage::Replace(Replace::OpenPath(path)) => {
                Task::perform(read_file(path), |result| match result {
                    Ok(opened) => Message::File(FileMessage::Opened(Ok(Some(opened)))),
                    Err((path, error)) => Message::File(FileMessage::RecentFailed(path, error)),
                })
            }
            FileMessage::RecentFailed(path, error) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                self.notify(format!("Could not open {name}: {error}"), true);
                self.recent_files.remove(&path);
                Task::none()
            }
            FileMessage::ClearRecent => {
                self.dialogs.menu = None;
                self.recent_files.clear();
                Task::none()
            }
            FileMessage::Replace(Replace::New) => {
                self.layers = Layers::default();
                self.layers_replaced();
                self.history = History::default();
                self.camera = Camera::default();
                self.path = None;
                self.saved = None;
                self.notice = None;
                self.backgrounds.images.clear();
                // Their ids start over.
                self.layers_panel.isolated.clear();
                self.backgrounds.editing = false;
                self.backgrounds.saved = self.backgrounds.version;
                self.view_changed()
            }
            FileMessage::Replace(Replace::Open) => Task::perform(open_file(), |value| {
                Message::File(FileMessage::Opened(value))
            }),
            FileMessage::Replace(Replace::Quit(id)) => window::close(id),
            FileMessage::Confirm(choice) => {
                let Some(then) = self.dialogs.confirming.take() else {
                    return Task::none();
                };
                match choice {
                    Choice::Save => self.save(false, Some(then)),
                    Choice::Discard => Task::done(Message::File(FileMessage::Replace(then))),
                    Choice::Cancel => Task::none(),
                }
            }
            FileMessage::Opened(Ok(None)) => Task::none(),
            FileMessage::Opened(Ok(Some((path, bytes)))) => match file::open(&bytes) {
                Ok(contents) => {
                    self.saved = Some(contents.layers.signature());
                    // Its colours, to pick again.
                    let colors: Vec<_> = contents
                        .layers
                        .layers()
                        .iter()
                        .flat_map(|layer| layer.document.colors().flatten())
                        .collect();
                    for color in colors.into_iter().rev() {
                        paint::remember(&mut self.paint.recent, color);
                    }
                    self.layers = contents.layers;
                    self.layers_replaced();
                    self.history = History::default();
                    self.camera = contents.camera;
                    if let Some(layer) = contents.current {
                        self.current = layer;
                        self.layers_panel.selected = layer;
                    }
                    self.backgrounds.images = contents.backgrounds;
                    // Their ids start over.
                    self.layers_panel.isolated.clear();
                    self.backgrounds.editing = false;
                    self.backgrounds.version += 1;
                    self.backgrounds.saved = self.backgrounds.version;
                    self.refresh_fields();
                    // Where the user left off there, if remembered.
                    if let Some(place) = self.places.get(&path) {
                        if let Some(camera) = place.camera() {
                            self.camera = camera;
                        }
                        if let Some(layer) = self.layers.layers().get(place.layer) {
                            self.current = layer.id;
                            self.layers_panel.selected = layer.id;
                        }
                    }
                    self.recent_files.add(path.clone());
                    self.path = Some(path);
                    self.notify(format!("Opened {}", self.name()), false);
                    self.view_changed()
                }
                Err(error) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.notify(format!("Could not open {name}: {error}"), true);
                    Task::none()
                }
            },
            FileMessage::Opened(Err(error)) => {
                self.notify(format!("Could not open: {error}"), true);
                Task::none()
            }
            FileMessage::Save => {
                self.dialogs.menu = None;
                self.save(false, None)
            }
            FileMessage::SaveAs => {
                self.dialogs.menu = None;
                self.save(true, None)
            }
            FileMessage::Saved(result, versions, then) => {
                match result {
                    Ok(Some(path)) => {
                        self.recent_files.add(path.clone());
                        self.path = Some(path);
                        self.remember_place();
                        self.notify(format!("Saved {}", self.name()), false);
                        self.saved = Some(versions.layers);
                        self.backgrounds.saved = versions.background;
                        if let Some(then) = then {
                            return Task::done(Message::File(FileMessage::Replace(then)));
                        }
                    }
                    // Cancelled; so is what was to follow.
                    Ok(None) => {}
                    Err(error) => self.notify(format!("Save failed: {error}"), true),
                }
                Task::none()
            }
        }
    }
}

/// What a file dropped on the window is, for Tessera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// A drawing, to open.
    Drawing,
    /// An image, to put behind the current layer.
    Image,
    Other,
}

impl Dropped {
    /// By its name.
    pub fn of(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        if extension.as_deref() == Some(file::EXTENSION) {
            Dropped::Drawing
        } else if image::ImageFormat::from_path(path).is_ok() {
            Dropped::Image
        } else {
            Dropped::Other
        }
    }
}
