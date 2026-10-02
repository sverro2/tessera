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
                self.editing_background = !self.editing_background;
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
                let background = match self.backgrounds.remove(&current) {
                    Some(old) => old.replaced_by(picked),
                    None => picked,
                };
                self.backgrounds.insert(current, background);
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
                if self.backgrounds.remove(&current).is_some() {
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
                    Field::X => &mut self.fields.x,
                    Field::Y => &mut self.fields.y,
                    Field::Scale => &mut self.fields.scale,
                    Field::Rotation => &mut self.fields.rotation,
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
                    self.background_version += 1;
                    self.caches.grid.clear();
                }
                Task::none()
            }
        }
    }
}
