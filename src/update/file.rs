//! Files: starting a new drawing, opening one (lately opened too), saving,
//! and asking first when changes would be lost.

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
    Opened(Result<Option<(PathBuf, String)>, String>),
    /// Open a recently opened file.
    OpenRecent(PathBuf),
    /// A recent file couldn't be read (it's gone, say): why.
    RecentFailed(PathBuf, String),
    ClearRecent,
    /// Saving this revision finished (to this path, unless cancelled);
    /// then this was to happen.
    Saved(Result<Option<PathBuf>, String>, Versions, Option<Replace>),
}

impl Tessera {
    pub(crate) fn update_file(&mut self, message: FileMessage) -> Task<Message> {
        match message {
            FileMessage::New => {
                self.menu = None;
                // A blank canvas is new already.
                if self.is_blank() {
                    return Task::none();
                }
                self.replace(Replace::New)
            }
            FileMessage::Open => {
                self.menu = None;
                self.replace(Replace::Open)
            }
            FileMessage::OpenRecent(path) => {
                self.menu = None;
                self.replace(Replace::OpenPath(path))
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
                self.menu = None;
                self.recent_files.clear();
                Task::none()
            }
            FileMessage::Replace(Replace::New) => {
                self.layers = Layers::default();
                self.layers_replaced();
                self.undo.clear();
                self.redo.clear();
                self.camera = Camera::default();
                self.path = None;
                self.saved = None;
                self.notice = None;
                self.backgrounds.clear();
                self.editing_background = false;
                self.saved_background = self.background_version;
                self.view_changed()
            }
            FileMessage::Replace(Replace::Open) => Task::perform(open_file(), |value| {
                Message::File(FileMessage::Opened(value))
            }),
            FileMessage::Replace(Replace::Quit(id)) => window::close(id),
            FileMessage::Confirm(choice) => {
                let Some(then) = self.confirming.take() else {
                    return Task::none();
                };
                match choice {
                    Choice::Save => self.save(false, Some(then)),
                    Choice::Discard => Task::done(Message::File(FileMessage::Replace(then))),
                    Choice::Cancel => Task::none(),
                }
            }
            FileMessage::Opened(Ok(None)) => Task::none(),
            FileMessage::Opened(Ok(Some((path, text)))) => match file::open(&text) {
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
                        paint::remember(&mut self.recent, color);
                    }
                    self.layers = contents.layers;
                    self.layers_replaced();
                    self.undo.clear();
                    self.redo.clear();
                    self.camera = contents.camera;
                    self.backgrounds = contents.backgrounds;
                    self.editing_background = false;
                    self.background_version += 1;
                    self.saved_background = self.background_version;
                    self.refresh_fields();
                    // Where the user left off there, if remembered.
                    if let Some(place) = self.places.get(&path) {
                        if let Some(camera) = place.camera() {
                            self.camera = camera;
                        }
                        if let Some(layer) = self.layers.layers().get(place.layer) {
                            self.current = layer.id;
                            self.selected = layer.id;
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
                self.menu = None;
                self.save(false, None)
            }
            FileMessage::SaveAs => {
                self.menu = None;
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
                        self.saved_background = versions.background;
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
