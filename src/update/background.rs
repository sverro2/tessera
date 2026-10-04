//! The background image: picking it, placing it, showing it.

use crate::*;

#[derive(Debug, Clone)]
pub enum BackgroundMessage {
    ToggleBackgroundMode,
    /// Pick a (new) background image.
    PickBackground,
    /// A background image file was picked and read.
    BackgroundPicked(Result<Option<Vec<u8>>, String>),
    /// Fit the background image to the view (of this window size).
    FitBackground,
    BackgroundFitted(iced::Size),
    ToggleBackgroundVisible,
    RemoveBackground,
    BackgroundField(Field, String),
    BackgroundOpacity(f32),
}

impl Tessera {
    pub(crate) fn update_background(&mut self, message: BackgroundMessage) -> Task<Message> {
        match message {
            BackgroundMessage::ToggleBackgroundMode => {
                self.backgrounds.editing = !self.backgrounds.editing;
                self.refresh_fields();
                Task::none()
            }
            BackgroundMessage::PickBackground => Task::perform(pick_image(), |value| {
                Message::Background(BackgroundMessage::BackgroundPicked(value))
            }),
            BackgroundMessage::BackgroundPicked(Ok(None)) => Task::none(),
            BackgroundMessage::BackgroundPicked(Ok(Some(bytes))) => {
                let picked = match Background::load(bytes) {
                    Ok(picked) => picked,
                    Err(error) => {
                        self.notify(format!("Could not use the image: {error}"), true);
                        return Task::none();
                    }
                };
                // A new image takes the old one's place; a first one fills
                // the view.
                let first = self.background().is_none();
                let current = self.current();
                let background = match self.backgrounds.images.remove(&current) {
                    Some(old) => old.replaced_by(picked),
                    None => picked,
                };
                self.backgrounds.images.insert(current, background);
                self.background_changed();
                if first {
                    return Task::done(Message::Background(BackgroundMessage::FitBackground));
                }
                Task::none()
            }
            BackgroundMessage::BackgroundPicked(Err(error)) => {
                self.notify(format!("Could not open the image: {error}"), true);
                Task::none()
            }
            BackgroundMessage::FitBackground => window::latest()
                .and_then(window::size)
                .map(|value| Message::Background(BackgroundMessage::BackgroundFitted(value))),
            BackgroundMessage::BackgroundFitted(window) => {
                let viewport = Self::canvas_size(window);
                let camera = self.camera;
                if let Some(background) = self.background_mut() {
                    background.fit(camera, viewport);
                    self.background_changed();
                }
                Task::none()
            }
            BackgroundMessage::ToggleBackgroundVisible => {
                if let Some(background) = self.background_mut() {
                    background.visible = !background.visible;
                    self.background_changed();
                }
                Task::none()
            }
            BackgroundMessage::RemoveBackground => {
                let current = self.current();
                if self.backgrounds.images.remove(&current).is_some() {
                    self.background_changed();
                }
                Task::none()
            }
            BackgroundMessage::BackgroundOpacity(opacity) => {
                if let Some(background) = self.background_mut() {
                    background.opacity = opacity;
                    self.background_changed();
                }
                Task::none()
            }
            BackgroundMessage::BackgroundField(field, typed) => {
                let value = typed.trim().replace(',', ".").parse::<f32>().ok();
                let draft = match field {
                    Field::X => &mut self.backgrounds.fields.x,
                    Field::Y => &mut self.backgrounds.fields.y,
                    Field::Scale => &mut self.backgrounds.fields.scale,
                    Field::Rotation => &mut self.backgrounds.fields.rotation,
                };
                *draft = typed;
                if let (Some(background), Some(value)) = (self.background_mut(), value)
                    && value.is_finite()
                {
                    match field {
                        Field::X => background.center.x = value,
                        Field::Y => background.center.y = value,
                        Field::Scale if value > 0.0 => {
                            background.scale =
                                (value / 100.0).clamp(Background::MIN_SCALE, Background::MAX_SCALE);
                        }
                        Field::Scale => return Task::none(),
                        Field::Rotation => {
                            background.rotation = background::normalize(value.to_radians());
                        }
                    }
                    // Not `background_changed`: that would retype the field.
                    self.backgrounds.version += 1;
                    self.caches.backdrop.clear();
                }
                Task::none()
            }
        }
    }
}

/// The background panel's number fields, as typed (so half-typed numbers
/// aren't overwritten).
#[derive(Default)]
pub struct Fields {
    pub x: String,
    pub y: String,
    pub scale: String,
    pub rotation: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Field {
    X,
    Y,
    /// In percent.
    Scale,
    /// In degrees.
    Rotation,
}

/// The layers' background images, and adjusting them.
#[derive(Default)]
pub struct Backgrounds {
    /// The layers' background images (each layer's own), by layer: kept
    /// apart from the layers, so undoing edits doesn't move them.
    pub images: HashMap<NodeId, Background>,
    /// Counts changes to the background, like the document's revision.
    pub version: u64,
    /// The background version last saved or opened.
    pub saved: u64,
    /// Whether the background mode is on: its panel is open and the canvas
    /// adjusts the image rather than edits the drawing.
    pub editing: bool,
    /// The background panel's number fields as typed.
    pub fields: Fields,
}
