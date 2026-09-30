mod background;
mod camera;
mod compass;
mod document;
mod editor;
mod file;
mod geometry;
mod paint;
mod wheel;
mod svg;

use std::path::{Path, PathBuf};

use iced::keyboard::{self, Key};
use iced::time::{Duration, Instant};
use iced::widget::{
    button, canvas, center, column, container, mouse_area, opaque, row, slider, space, stack, text,
    text_input,
};
use iced::window;
use iced::{Center, Color, Element, Fill, Padding, Subscription, Task, Theme};

use background::Background;
use camera::Camera;
use document::{Document, Edit};
use editor::Tool;
use paint::{Brush, Hsv, Target};

pub fn main() -> iced::Result {
    iced::application(Tessera::new, Tessera::update, Tessera::view)
        .title(Tessera::title)
        .subscription(Tessera::subscription)
        .theme(Theme::TokyoNight)
        .exit_on_close_request(false)
        .window_size(WINDOW_SIZE)
        .centered()
        .run()
}

/// The window's size when it opens.
const WINDOW_SIZE: iced::Size = iced::Size::new(1152.0, 768.0);
/// What the canvas is for: changing the shape, or painting it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Mode {
    #[default]
    Shape,
    Paint,
}

/// Width of a menu's button in the menu bar.
const MENU_WIDTH: f32 = 64.0;
/// How many edits can be undone.
const MAX_UNDO: usize = 200;
/// How long a notice takes to fade out.
const NOTICE_FADE: Duration = Duration::from_millis(600);
/// The menu bar's height: where menus drop down from.
const MENU_BAR_HEIGHT: f32 = 34.0;
/// Icons for the background's visibility toggle.
const EYE_OPEN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/></svg>"#;
const ERASER: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21"/><path d="M22 21H7"/><path d="m5 11 9 9"/></svg>"#;
const EYE_CLOSED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/><path d="M3 3l18 18"/></svg>"#;

#[derive(Default)]
struct Tessera {
    /// The data. Only ever changed through `Document::apply`, or replaced
    /// as a whole (new, open).
    document: Document,
    /// The document as it was before each edit, most recent last; and as
    /// it was before each undo.
    undo: Vec<Document>,
    redo: Vec<Document>,
    /// View state, independent of the data.
    camera: Camera,
    /// The drawn document; cleared when it or the camera changes.
    cache: canvas::Cache,
    /// The drawn background grid; cleared when the camera changes.
    grid: canvas::Cache,
    /// Where the document was last saved to or opened from.
    path: Option<PathBuf>,
    /// The document revision last saved or opened; `None` for a new
    /// document (which starts out saved). Anything else has unsaved
    /// changes; moving the view doesn't count.
    saved: Option<u64>,
    /// The menu that is open, if any.
    menu: Option<Menu>,
    /// Asking what to do with unsaved changes before doing this.
    confirming: Option<Replace>,
    /// A short message about what just happened, fading out.
    notice: Option<Notice>,
    /// An image to draw over, kept with the document (not part of undo).
    background: Option<Background>,
    /// Counts changes to the background, like the document's revision.
    background_version: u64,
    /// The background version last saved or opened.
    saved_background: u64,
    /// Whether the background mode is on: its panel is open and the canvas
    /// adjusts the image rather than edits the drawing.
    editing_background: bool,
    /// The background panel's number fields as typed.
    fields: Fields,
    /// What dragging on the canvas does.
    mode: Mode,
    /// What painting a face does, in the paint mode.
    brush: Brush,
    /// Colours painted with lately, most recent first.
    recent: Vec<Color>,
    /// The paint panel's colour field: as typed, or the colour picked.
    hex: String,
    /// The brush's colour on the colour wheel (keeping the hue of greys).
    hsv: Hsv,
    /// Whether painting paints faces or edges.
    target: Target,
    /// How wide painted edges are (world units).
    edge_width: f32,
    /// The window's size, once it has been resized (or first laid out);
    /// until then, the size it opens at.
    window_size: Option<iced::Size>,
}

/// The background panel's number fields, as typed (so half-typed numbers
/// aren't overwritten).
#[derive(Default)]
struct Fields {
    x: String,
    y: String,
    scale: String,
    rotation: String,
}

#[derive(Debug, Clone, Copy)]
enum Field {
    X,
    Y,
    /// In percent.
    Scale,
    /// In degrees.
    Rotation,
}

/// A short message shown at the bottom right for a while, then fading out.
struct Notice {
    text: String,
    /// Something went wrong: shown longer, and in red.
    error: bool,
    since: Instant,
    /// How long it has been shown, as of the last frame.
    shown: Duration,
}

impl Notice {
    /// How long it's shown before fading out.
    fn duration(&self) -> Duration {
        if self.error {
            Duration::from_secs(6)
        } else {
            Duration::from_secs(3)
        }
    }

    /// How opaque it is now, from 1 down to 0.
    fn opacity(&self) -> f32 {
        let fading = self.shown.saturating_sub(self.duration());
        1.0 - (fading.as_secs_f32() / NOTICE_FADE.as_secs_f32()).min(1.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Menu {
    File,
    Edit,
    View,
}

/// Things that replace (or close) the document, so unsaved changes would be
/// lost.
#[derive(Debug, Clone, Copy)]
enum Replace {
    New,
    Open,
    Quit(window::Id),
}

/// The answers to "save the changes first?".
#[derive(Debug, Clone, Copy)]
enum Choice {
    Save,
    Discard,
    Cancel,
}

#[derive(Debug, Clone)]
enum Message {
    Editor(editor::Message),
    Compass(compass::Message),
    SetMode(Mode),
    PickBrush(Brush),
    HexTyped(String),
    /// Picked on the colour wheel, or its brightness slider.
    WheelPicked(Hsv),
    SetTarget(Target),
    EdgeWidth(f32),
    WindowResized(iced::Size),
    ToggleMenu(Menu),
    /// The pointer moved onto a menu's button.
    HoverMenu(Menu),
    CloseMenu,
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
    ExportSvg,
    ResetView,
    /// Do it: confirmed, or there was nothing to lose.
    Replace(Replace),
    Confirm(Choice),
    /// A file was picked and read: its path and text.
    Opened(Result<Option<(PathBuf, String)>, String>),
    /// Saving this revision finished (to this path, unless cancelled);
    /// then this was to happen.
    Saved(Result<Option<PathBuf>, String>, Versions, Option<Replace>),
    Exported(Result<Option<PathBuf>, String>),
    CloseRequested(window::Id),
    Key(keyboard::Event),
    /// A frame, while a notice fades.
    Frame(Instant),
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

/// What was saved: the document revision and background version.
#[derive(Debug, Clone, Copy)]
struct Versions {
    revision: u64,
    background: u64,
}

impl Tessera {
    fn title(&self) -> String {
        let changed = if self.is_unsaved() { " •" } else { "" };
        format!("{}{changed} — Tessera", self.name())
    }

    /// The document's file name.
    fn name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map_or("Untitled".into(), |name| {
                name.to_string_lossy().into_owned()
            })
    }

    /// Whether there's something that would be lost.
    fn is_unsaved(&self) -> bool {
        let edited = self
            .saved
            .is_some_and(|saved| saved != self.document.revision());
        (edited && !self.document.is_empty()) || self.background_version != self.saved_background
    }

    /// Nothing drawn, no background: nothing a new document would change.
    fn is_blank(&self) -> bool {
        self.document.is_empty() && self.background.is_none()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            keyboard::listen().map(Message::Key),
            window::close_requests().map(Message::CloseRequested),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
            if self.notice.is_some() {
                window::frames().map(Message::Frame)
            } else {
                Subscription::none()
            },
        ])
    }

    fn new() -> Self {
        let brush = Brush::default();
        let color = brush.color().unwrap_or(Color::WHITE);
        Tessera {
            hex: paint::to_hex(color),
            hsv: Hsv::from_color(color, 0.0),
            edge_width: 2.0,
            brush,
            ..Tessera::default()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Editor(editor::Message::Edit { edits, revision }) => {
                if revision != self.document.revision() {
                    return Task::none();
                }
                // A new document starts out saved.
                self.saved.get_or_insert(revision);
                let before = self.document.clone();
                if self.document.apply_all(&edits) {
                    for edit in &edits {
                        let color = match *edit {
                            Edit::Paint { color, .. } => color,
                            Edit::PaintEdge { style, .. } => style.map(|style| style.color),
                            _ => None,
                        };
                        if let Some(color) = color {
                            paint::remember(&mut self.recent, color);
                        }
                    }
                    self.undo.push(before);
                    if self.undo.len() > MAX_UNDO {
                        self.undo.remove(0);
                    }
                    self.redo.clear();
                }
                self.cache.clear();
                return Task::none();
            }
            Message::Undo => {
                self.menu = None;
                let Some(before) = self.undo.pop() else {
                    return Task::none();
                };
                self.redo
                    .push(std::mem::replace(&mut self.document, before));
                self.cache.clear();
                return Task::none();
            }
            Message::Redo => {
                self.menu = None;
                let Some(after) = self.redo.pop() else {
                    return Task::none();
                };
                self.undo.push(std::mem::replace(&mut self.document, after));
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
            Message::Compass(compass::Message::TurnTo(rotation)) => {
                // Around the middle of the canvas.
                let window = self.window_size.unwrap_or(WINDOW_SIZE);
                let anchor = iced::Point::new(
                    window.width / 2.0,
                    (window.height - MENU_BAR_HEIGHT) / 2.0,
                );
                self.camera.rotate_to(anchor, rotation);
            }
            Message::SetMode(mode) => {
                if mode != self.mode {
                    self.mode = mode;
                    // Colours show in the paint mode only.
                    self.cache.clear();
                }
                return Task::none();
            }
            Message::PickBrush(brush) => {
                self.brush = brush;
                if let Brush::Color(color) = brush {
                    self.hsv = Hsv::from_color(color, self.hsv.hue);
                    self.hex = paint::to_hex(color);
                }
                return Task::none();
            }
            Message::HexTyped(typed) => {
                if let Some(color) = paint::from_hex(&typed) {
                    self.brush = Brush::Color(color);
                    self.hsv = Hsv::from_color(color, self.hsv.hue);
                }
                self.hex = typed;
                return Task::none();
            }
            Message::WheelPicked(hsv) => {
                let color = hsv.to_color();
                self.hsv = hsv;
                self.brush = Brush::Color(color);
                self.hex = paint::to_hex(color);
                return Task::none();
            }
            Message::SetTarget(target) => {
                self.target = target;
                return Task::none();
            }
            Message::EdgeWidth(width) => {
                self.edge_width = width;
                return Task::none();
            }
            Message::WindowResized(size) => {
                self.window_size = Some(size);
                return Task::none();
            }
            Message::ResetView => {
                self.menu = None;
                self.camera = Camera::default();
            }
            Message::ToggleMenu(menu) => {
                self.menu = (self.menu != Some(menu)).then_some(menu);
                return Task::none();
            }
            Message::HoverMenu(menu) => {
                if self.menu.is_some() {
                    self.menu = Some(menu);
                }
                return Task::none();
            }
            Message::CloseMenu => {
                self.menu = None;
                return Task::none();
            }
            Message::New => {
                self.menu = None;
                // A blank canvas is new already.
                if self.is_blank() {
                    return Task::none();
                }
                return self.replace(Replace::New);
            }
            Message::Open => {
                self.menu = None;
                return self.replace(Replace::Open);
            }
            Message::Replace(Replace::New) => {
                self.document = Document::default();
                self.undo.clear();
                self.redo.clear();
                self.camera = Camera::default();
                self.path = None;
                self.saved = None;
                self.notice = None;
                self.background = None;
                self.editing_background = false;
                self.saved_background = self.background_version;
            }
            Message::Replace(Replace::Open) => {
                return Task::perform(open_file(), Message::Opened);
            }
            Message::Replace(Replace::Quit(id)) => return window::close(id),
            Message::Confirm(choice) => {
                let Some(then) = self.confirming.take() else {
                    return Task::none();
                };
                return match choice {
                    Choice::Save => self.save(false, Some(then)),
                    Choice::Discard => Task::done(Message::Replace(then)),
                    Choice::Cancel => Task::none(),
                };
            }
            Message::Opened(Ok(None)) => return Task::none(),
            Message::Opened(Ok(Some((path, text)))) => match file::open(&text) {
                Ok(contents) => {
                    self.saved = Some(contents.document.revision());
                    // Its colours, to pick again.
                    let colors: Vec<_> = contents.document.colors().flatten().collect();
                    for color in colors.into_iter().rev() {
                        paint::remember(&mut self.recent, color);
                    }
                    self.document = contents.document;
                    self.undo.clear();
                    self.redo.clear();
                    self.camera = contents.camera;
                    self.background = contents.background;
                    self.editing_background = false;
                    self.background_version += 1;
                    self.saved_background = self.background_version;
                    self.refresh_fields();
                    self.path = Some(path);
                    self.notify(format!("Opened {}", self.name()), false);
                }
                Err(error) => {
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    self.notify(format!("Could not open {name}: {error}"), true);
                    return Task::none();
                }
            },
            Message::Opened(Err(error)) => {
                self.notify(format!("Could not open: {error}"), true);
                return Task::none();
            }
            Message::Save => {
                self.menu = None;
                return self.save(false, None);
            }
            Message::SaveAs => {
                self.menu = None;
                return self.save(true, None);
            }
            Message::Saved(result, versions, then) => {
                match result {
                    Ok(Some(path)) => {
                        self.path = Some(path);
                        self.notify(format!("Saved {}", self.name()), false);
                        self.saved = Some(versions.revision);
                        self.saved_background = versions.background;
                        if let Some(then) = then {
                            return Task::done(Message::Replace(then));
                        }
                    }
                    // Cancelled; so is what was to follow.
                    Ok(None) => {}
                    Err(error) => self.notify(format!("Save failed: {error}"), true),
                }
                return Task::none();
            }
            Message::ExportSvg => {
                self.menu = None;
                let svg = svg::export(&self.document);
                return Task::perform(save_svg(svg), Message::Exported);
            }
            Message::Exported(result) => {
                match result {
                    Ok(Some(path)) => {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        self.notify(format!("Exported {name}"), false);
                    }
                    Ok(None) => {}
                    Err(error) => self.notify(format!("Export failed: {error}"), true),
                }
                return Task::none();
            }
            Message::CloseRequested(id) => return self.replace(Replace::Quit(id)),
            Message::Key(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) => {
                if key == Key::Named(keyboard::key::Named::Escape) {
                    self.menu = None;
                    self.confirming = None;
                    self.editing_background = false;
                    return Task::none();
                }
                if self.confirming.is_some() {
                    return Task::none();
                }
                if !modifiers.command() {
                    let message = match key.to_latin(physical_key) {
                        Some('s') => Message::SetMode(Mode::Shape),
                        Some('p') => Message::SetMode(Mode::Paint),
                        Some('f') if self.mode == Mode::Paint => Message::SetTarget(Target::Faces),
                        Some('e') if self.mode == Mode::Paint => Message::SetTarget(Target::Edges),
                        _ => return Task::none(),
                    };
                    return Task::done(message);
                }
                let message = match key.to_latin(physical_key) {
                    Some('n') => Message::New,
                    Some('o') => Message::Open,
                    Some('s') if modifiers.shift() => Message::SaveAs,
                    Some('s') => Message::Save,
                    Some('z') if modifiers.shift() => Message::Redo,
                    Some('z') => Message::Undo,
                    Some('y') => Message::Redo,
                    _ => return Task::none(),
                };
                return Task::done(message);
            }
            Message::Key(_) => return Task::none(),
            Message::ToggleBackgroundMode => {
                self.editing_background = !self.editing_background;
                self.refresh_fields();
                return Task::none();
            }
            Message::PickBackground => {
                return Task::perform(pick_image(), Message::BackgroundPicked);
            }
            Message::BackgroundPicked(Ok(None)) => return Task::none(),
            Message::BackgroundPicked(Ok(Some(bytes))) => {
                let picked = match Background::load(bytes) {
                    Ok(picked) => picked,
                    Err(error) => {
                        self.notify(format!("Could not use the image: {error}"), true);
                        return Task::none();
                    }
                };
                // A new image takes the old one's place; a first one fills
                // the view.
                let first = self.background.is_none();
                self.background = Some(match &self.background {
                    Some(old) => old.replaced_by(picked),
                    None => picked,
                });
                self.background_changed();
                if first {
                    return Task::done(Message::FitBackground);
                }
                return Task::none();
            }
            Message::BackgroundPicked(Err(error)) => {
                self.notify(format!("Could not open the image: {error}"), true);
                return Task::none();
            }
            Message::FitBackground => {
                return window::latest()
                    .and_then(window::size)
                    .map(Message::BackgroundFitted);
            }
            Message::BackgroundFitted(window) => {
                let viewport = iced::Size::new(window.width, window.height - MENU_BAR_HEIGHT);
                if let Some(background) = &mut self.background {
                    background.fit(self.camera, viewport);
                    self.background_changed();
                }
                return Task::none();
            }
            Message::ToggleBackgroundVisible => {
                if let Some(background) = &mut self.background {
                    background.visible = !background.visible;
                    self.background_changed();
                }
                return Task::none();
            }
            Message::RemoveBackground => {
                if self.background.take().is_some() {
                    self.background_changed();
                }
                return Task::none();
            }
            Message::BackgroundOpacity(opacity) => {
                if let Some(background) = &mut self.background {
                    background.opacity = opacity;
                    self.background_changed();
                }
                return Task::none();
            }
            Message::BackgroundField(field, typed) => {
                let value = typed.trim().replace(',', ".").parse::<f32>().ok();
                let draft = match field {
                    Field::X => &mut self.fields.x,
                    Field::Y => &mut self.fields.y,
                    Field::Scale => &mut self.fields.scale,
                    Field::Rotation => &mut self.fields.rotation,
                };
                *draft = typed;
                if let (Some(background), Some(value)) = (&mut self.background, value)
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
                    self.grid.clear();
                }
                return Task::none();
            }
            Message::Editor(editor::Message::MoveBackground(delta)) => {
                if let Some(background) = &mut self.background {
                    background.center += delta;
                    self.background_changed();
                }
                return Task::none();
            }
            Message::Editor(editor::Message::ScaleBackground { anchor, factor }) => {
                if let Some(background) = &mut self.background {
                    background.scale_at(anchor, factor);
                    self.background_changed();
                }
                return Task::none();
            }
            Message::Editor(editor::Message::RotateBackground { anchor, angle }) => {
                if let Some(background) = &mut self.background {
                    background.rotate_at(anchor, angle);
                    self.background_changed();
                }
                return Task::none();
            }
            Message::Frame(now) => {
                if let Some(notice) = &mut self.notice {
                    notice.shown = now.saturating_duration_since(notice.since);
                    if notice.opacity() <= 0.0 {
                        self.notice = None;
                    }
                }
                return Task::none();
            }
        }

        // Everything else above moves the camera or replaces the document.
        self.cache.clear();
        self.grid.clear();
        Task::none()
    }

    /// What saving now would save.
    fn versions(&self) -> Versions {
        Versions {
            revision: self.document.revision(),
            background: self.background_version,
        }
    }

    /// After the background changed: it's unsaved, and needs drawing.
    fn background_changed(&mut self) {
        self.background_version += 1;
        self.grid.clear();
        self.refresh_fields();
    }

    /// Fills the background panel's fields in from the background.
    fn refresh_fields(&mut self) {
        let Some(background) = &self.background else {
            self.fields = Fields::default();
            return;
        };
        self.fields = Fields {
            x: format!("{:.1}", background.center.x),
            y: format!("{:.1}", background.center.y),
            scale: format!("{:.2}", background.scale * 100.0),
            rotation: format!("{:.1}", background.rotation.to_degrees()),
        };
    }

    fn notify(&mut self, text: String, error: bool) {
        self.notice = Some(Notice {
            text,
            error,
            since: Instant::now(),
            shown: Duration::ZERO,
        });
    }

    /// Does `replace`, first asking what to do with unsaved changes.
    fn replace(&mut self, replace: Replace) -> Task<Message> {
        if self.is_unsaved() {
            self.confirming = Some(replace);
            Task::none()
        } else {
            Task::done(Message::Replace(replace))
        }
    }

    /// Saves to where the document came from, or (if `choose`, or it never
    /// was saved) to a file picked first; then does `then`.
    fn save(&mut self, choose: bool, then: Option<Replace>) -> Task<Message> {
        let text = file::save(&self.document, self.camera, self.background.as_ref());
        let versions = self.versions();
        let path = self.path.clone().filter(|_| !choose);
        let name = match &self.path {
            Some(_) => self.name(),
            None => format!("untitled.{}", file::EXTENSION),
        };

        Task::perform(save_file(path, name, text), move |result| {
            Message::Saved(result, versions, then)
        })
    }

    fn view(&self) -> Element<'_, Message> {
        let menu_button = |label, menu| {
            let button = button(text(label).size(14))
                .width(MENU_WIDTH)
                .padding([6, 12])
                .style(if self.menu == Some(menu) {
                    button::secondary
                } else {
                    button::text
                })
                .on_press(Message::ToggleMenu(menu));
            // With a menu open, moving over another opens that one instead.
            mouse_area(button).on_enter(Message::HoverMenu(menu))
        };

        let toolbar = row![
            menu_button("File", Menu::File),
            menu_button("Edit", Menu::Edit),
            menu_button("View", Menu::View),
            self.mode_switch(),
            space::horizontal(),
            text(match self.mode {
                Mode::Shape => "Drag corner: move · Drag edge: extend · Drag blank: new tri · C: cut edge · D: delete tri · Middle-drag: pan · Wheel: zoom · Shift+wheel: rotate",
                Mode::Paint => "Click or drag over faces: paint · Middle-drag: pan · Wheel: zoom · Shift+wheel: rotate",
            })
            .size(12),
        ]
        .spacing(10)
        .padding([2, 8])
        .align_y(Center);

        let stats = container(
            row![
                text(format!(
                    "{:.0}% · {:.0}°",
                    self.camera.zoom * 100.0,
                    self.camera.rotation.to_degrees()
                ))
                .size(12),
                text("|").size(12).style(text::secondary),
                text(format!(
                    "{} vertices · {} edges · {} faces",
                    self.document.vertex_count(),
                    self.document.unique_edges().len(),
                    self.document.triangle_ids().len(),
                ))
                .size(12),
            ]
            .spacing(8),
        )
        .padding([4, 8])
        .style(container::dark);

        let canvas = stack![
            editor::view(
                &self.document,
                self.camera,
                &self.cache,
                &self.grid,
                self.background.as_ref(),
                self.tool(),
            )
            .map(Message::Editor),
            container(
                column![self.notice(), stats]
                    .spacing(6)
                    .align_x(iced::Right)
            )
            .align_right(Fill)
            .align_bottom(Fill)
            .padding(10),
            self.background_panel(),
            self.paint_panel(),
        ];

        let mut screen = stack![column![
            // Clicking the bar outside its buttons closes an open menu.
            mouse_area(
                container(toolbar)
                    .width(Fill)
                    .height(MENU_BAR_HEIGHT)
                    .align_y(Center)
                    .style(container::dark)
            )
            .on_press(Message::CloseMenu),
            canvas,
        ]];
        if let Some(menu) = self.menu {
            screen = screen.push(self.dropdown(menu));
        }
        if self.confirming.is_some() {
            screen = screen.push(self.confirmation());
        }
        screen.into()
    }

    /// What the canvas does: the background mode takes over from the mode
    /// while it's on.
    fn tool(&self) -> Tool {
        match self.mode {
            _ if self.editing_background => Tool::Background {
                painted: self.mode == Mode::Paint,
            },
            Mode::Shape => Tool::Shape,
            Mode::Paint => Tool::Paint {
                target: self.target,
                brush: self.brush,
                width: self.edge_width,
            },
        }
    }

    /// Shape and paint, side by side as one control.
    fn mode_switch(&self) -> Element<'_, Message> {
        joined(vec![
            ("Shape", self.mode == Mode::Shape, Message::SetMode(Mode::Shape)),
            ("Paint", self.mode == Mode::Paint, Message::SetMode(Mode::Paint)),
        ])
    }

    /// In the paint mode: what to paint (faces or edges) and the brush to
    /// paint with: a colour (from the wheel, the palette, recently used ones,
    /// or typed) or the eraser; for edges also how wide.
    fn paint_panel(&self) -> Element<'_, Message> {
        if self.mode != Mode::Paint {
            return space().into();
        }
        /// The panel's width inside its padding.
        const INNER: f32 = 180.0;
        const SWATCH: f32 = 24.0;
        let gap = (INNER - 6.0 * SWATCH) / 5.0;
        let small = |label| text(label).size(13);
        let swatch = |color: Color| {
            let picked = self.brush == Brush::Color(color);
            button(space().width(SWATCH).height(SWATCH))
                .padding(0)
                .style(move |theme: &Theme, status| button::Style {
                    background: Some(color.into()),
                    border: iced::Border {
                        color: if picked {
                            theme.palette().primary.base.color
                        } else if status == button::Status::Hovered {
                            theme.palette().background.base.text
                        } else {
                            Color::TRANSPARENT
                        },
                        width: 2.0,
                        radius: 3.0.into(),
                    },
                    ..button::Style::default()
                })
                .on_press(Message::PickBrush(Brush::Color(color)))
        };
        let swatches =
            |colors: &[Color]| row(colors.iter().map(|&color| swatch(color).into())).spacing(gap);

        let current: Element<'_, Message> = match self.brush {
            Brush::Color(color) => container(space().width(24).height(24))
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    border: iced::Border {
                        radius: 3.0.into(),
                        ..iced::Border::default()
                    },
                    ..container::Style::default()
                })
                .into(),
            Brush::Eraser => container(icon(ERASER, button::secondary))
                .center_x(24)
                .into(),
        };
        let eraser_style = if self.brush == Brush::Eraser {
            button::primary
        } else {
            button::secondary
        };
        let eraser = button(icon(ERASER, eraser_style))
            .padding([4, 6])
            .style(eraser_style)
            .on_press(Message::PickBrush(Brush::Eraser));

        let hsv = self.hsv;
        let mut body = column![
            joined(vec![
                (
                    "Faces",
                    self.target == Target::Faces,
                    Message::SetTarget(Target::Faces)
                ),
                (
                    "Edges",
                    self.target == Target::Edges,
                    Message::SetTarget(Target::Edges)
                ),
            ]),
            row![
                current,
                text_input("#rrggbb", &self.hex)
                    .size(13)
                    .padding([3, 6])
                    .on_input(Message::HexTyped),
                eraser,
            ]
            .spacing(8)
            .align_y(Center),
            container(wheel::view(hsv, INNER - 20.0).map(Message::WheelPicked)).center_x(Fill),
            row![
                small("Light").width(44),
                slider(0.0..=1.0, hsv.value, move |value| {
                    Message::WheelPicked(Hsv { value, ..hsv })
                })
                .step(0.01),
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(8);
        if self.target == Target::Edges {
            body = body.push(
                row![
                    small("Width").width(44),
                    slider(0.5..=12.0, self.edge_width, Message::EdgeWidth).step(0.5),
                    text(format!("{:.1}", self.edge_width)).size(13).width(28),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        body = body
            .push(swatches(&paint::PALETTE[..6]))
            .push(swatches(&paint::PALETTE[6..]));
        if !self.recent.is_empty() {
            body = body.push(text("Recent").size(11).style(text::secondary));
            for chunk in self.recent.chunks(6) {
                body = body.push(swatches(chunk));
            }
        }

        container(opaque(
            container(body)
                .padding(10)
                .width(INNER + 20.0)
                .style(container::bordered_box),
        ))
        .padding(12)
        .into()
    }

    /// The compass and, below it, the background button and (in the
    /// background mode) the panel to set up the image with.
    fn background_panel(&self) -> Element<'_, Message> {
        let small = |label| text(label).size(13);
        // The background button, joined on its left by the eye to show or
        // hide the image without opening the panel: one control, split in
        // two, styled alike.
        let base = if self.editing_background {
            button::primary
        } else {
            button::secondary
        };
        let visible = self.background.as_ref().map(|background| background.visible);
        let toggle = button(small("Background"))
            .padding([4, 10])
            .style(move |theme: &Theme, status| {
                let mut style = base(theme, status);
                if visible.is_some() {
                    let radius = style.border.radius;
                    style.border.radius = radius.top_left(0).bottom_left(0);
                }
                style
            })
            .on_press(Message::ToggleBackgroundMode);

        let mut buttons = row![].spacing(1);
        if let Some(visible) = visible {
            let icon = if visible { EYE_OPEN } else { EYE_CLOSED };
            let eye = iced::widget::svg(iced::widget::svg::Handle::from_memory(icon))
                .width(16)
                .height(Fill)
                // The same colour as the button's label.
                .style(move |theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(base(theme, button::Status::Active).text_color),
                });
            buttons = buttons.push(
                button(eye)
                    .padding([4, 8])
                    .height(Fill)
                    .style(move |theme: &Theme, status| {
                        let mut style = base(theme, status);
                        let radius = style.border.radius;
                        style.border.radius = radius.top_right(0).bottom_right(0);
                        style
                    })
                    .on_press(Message::ToggleBackgroundVisible),
            );
        }
        let buttons = buttons.push(toggle).height(iced::Shrink);

        let mut panel = column![
            compass::view(self.camera.rotation).map(Message::Compass),
            buttons,
        ]
        .spacing(6)
        .align_x(iced::Right);

        if self.editing_background {
            let body: Element<'_, Message> = match &self.background {
                None => column![
                    small("No background image yet."),
                    row![
                        button(small("Choose image…")).on_press(Message::PickBackground),
                        space::horizontal(),
                        button(small("Done"))
                            .style(button::secondary)
                            .on_press(Message::ToggleBackgroundMode),
                    ],
                ]
                .spacing(10)
                .into(),
                Some(background) => {
                    let field = |label, field| {
                        let value = match field {
                            Field::X => &self.fields.x,
                            Field::Y => &self.fields.y,
                            Field::Scale => &self.fields.scale,
                            Field::Rotation => &self.fields.rotation,
                        };
                        row![
                            small(label).width(80),
                            text_input("", value)
                                .size(13)
                                .padding([3, 6])
                                .on_input(move |typed| Message::BackgroundField(field, typed)),
                        ]
                        .spacing(8)
                        .align_y(Center)
                    };
                    column![
                        row![
                            button(small("Change…")).on_press(Message::PickBackground),
                            space::horizontal(),
                            button(small("Remove"))
                                .style(button::danger)
                                .on_press(Message::RemoveBackground),
                        ]
                        .spacing(6),
                        field("X", Field::X),
                        field("Y", Field::Y),
                        field("Scale %", Field::Scale),
                        field("Rotation °", Field::Rotation),
                        row![
                            small("Opacity").width(80),
                            slider(0.0..=1.0, background.opacity, Message::BackgroundOpacity)
                                .step(0.01),
                        ]
                        .spacing(8)
                        .align_y(Center),
                        text("Drag: move · Wheel: scale · Shift+wheel: rotate")
                            .size(11)
                            .style(text::secondary),
                        row![
                            button(small("Fit to view"))
                                .style(button::secondary)
                                .on_press(Message::FitBackground),
                            space::horizontal(),
                            button(small("Done")).on_press(Message::ToggleBackgroundMode),
                        ],
                    ]
                    .spacing(8)
                    .into()
                }
            };
            panel = panel.push(
                container(body)
                    .padding(10)
                    .width(280)
                    .style(container::bordered_box),
            );
        }

        // In the top right corner.
        container(opaque(panel))
            .align_right(Fill)
            .padding(12)
            .into()
    }

    /// The current notice, if any, fading as it goes.
    fn notice(&self) -> Element<'_, Message> {
        let Some(notice) = &self.notice else {
            return space().into();
        };
        let opacity = notice.opacity();
        let error = notice.error;

        container(text(&notice.text).size(13))
            .padding([6, 10])
            .style(move |theme: &Theme| {
                let palette = theme.palette();
                let (background, color) = if error {
                    (palette.danger.base.color, palette.danger.base.text)
                } else {
                    (
                        palette.background.strong.color,
                        palette.background.strong.text,
                    )
                };
                let fade = |color: Color| Color {
                    a: color.a * opacity,
                    ..color
                };
                container::Style {
                    background: Some(fade(background).into()),
                    text_color: Some(fade(color)),
                    border: iced::border::rounded(6),
                    ..container::Style::default()
                }
            })
            .into()
    }

    /// The open menu's items, below its button; clicking anywhere else
    /// (but another menu's button) closes it.
    fn dropdown(&self, menu: Menu) -> Element<'_, Message> {
        let item = |label, shortcut, message: Option<Message>| {
            button(
                row![
                    text(label).size(14),
                    space::horizontal(),
                    text(shortcut).size(12).style(text::secondary),
                ]
                .spacing(20),
            )
            .width(Fill)
            .padding([6, 12])
            .style(button::text)
            .on_press_maybe(message)
        };
        let has_content = !self.document.is_empty();

        let (offset, items) = match menu {
            Menu::File => (
                0.0,
                column![
                    item("New", "Ctrl+N", has_content.then_some(Message::New)),
                    item("Open…", "Ctrl+O", Some(Message::Open)),
                    item("Save", "Ctrl+S", Some(Message::Save)),
                    item("Save As…", "Ctrl+Shift+S", Some(Message::SaveAs)),
                    item("Export SVG…", "", has_content.then_some(Message::ExportSvg)),
                ],
            ),
            Menu::Edit => (
                MENU_WIDTH + 10.0,
                column![
                    item(
                        "Undo",
                        "Ctrl+Z",
                        (!self.undo.is_empty()).then_some(Message::Undo)
                    ),
                    item(
                        "Redo",
                        "Ctrl+Shift+Z",
                        (!self.redo.is_empty()).then_some(Message::Redo)
                    ),
                ],
            ),
            Menu::View => (
                2.0 * (MENU_WIDTH + 10.0),
                column![item("Reset view", "", Some(Message::ResetView))],
            ),
        };

        let panel = container(items.width(220))
            .padding(4)
            .style(container::bordered_box);

        // Below the menu bar: its buttons stay usable, to switch menus.
        let outside = column![
            space().height(MENU_BAR_HEIGHT),
            mouse_area(space().width(Fill).height(Fill)).on_press(Message::CloseMenu),
        ];

        stack![
            outside,
            container(opaque(panel)).padding(Padding {
                top: MENU_BAR_HEIGHT,
                left: 8.0 + offset,
                ..Padding::ZERO
            }),
        ]
        .into()
    }

    /// Asks what to do with unsaved changes, over the dimmed window.
    fn confirmation(&self) -> Element<'_, Message> {
        let dialog = container(
            column![
                text(format!("Save changes to “{}”?", self.name())).size(18),
                text("Your changes will be lost if you don't save them.").size(14),
                row![
                    button("Don't save")
                        .style(button::danger)
                        .on_press(Message::Confirm(Choice::Discard)),
                    space::horizontal(),
                    button("Cancel")
                        .style(button::secondary)
                        .on_press(Message::Confirm(Choice::Cancel)),
                    button("Save").on_press(Message::Confirm(Choice::Save)),
                ]
                .spacing(10),
            ]
            .spacing(16)
            .width(380),
        )
        .padding(20)
        .style(container::bordered_box);

        let backdrop = center(opaque(dialog)).style(|_| {
            container::Style::default().background(Color {
                a: 0.5,
                ..Color::BLACK
            })
        });

        opaque(mouse_area(backdrop).on_press(Message::Confirm(Choice::Cancel)))
    }
}

/// Buttons side by side as one control, split in parts, the active ones
/// highlighted.
fn joined<'a>(parts: Vec<(&'a str, bool, Message)>) -> Element<'a, Message> {
    let last = parts.len().saturating_sub(1);
    row(parts
        .into_iter()
        .enumerate()
        .map(|(i, (label, active, message))| {
            button(text(label).size(13))
                .padding([4, 12])
                .style(move |theme: &Theme, status| {
                    let mut style = if active {
                        button::primary(theme, status)
                    } else {
                        button::secondary(theme, status)
                    };
                    let mut radius = style.border.radius;
                    if i > 0 {
                        radius = radius.top_left(0).bottom_left(0);
                    }
                    if i < last {
                        radius = radius.top_right(0).bottom_right(0);
                    }
                    style.border.radius = radius;
                    style
                })
                .on_press(message)
                .into()
        }))
    .spacing(1)
    .into()
}

/// A 16 px icon, coloured as the label of a button in `style`.
fn icon<'a>(
    svg: &'static [u8],
    style: fn(&Theme, button::Status) -> button::Style,
) -> iced::widget::Svg<'a> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(svg))
        .width(16)
        .height(16)
        .style(move |theme: &Theme, _| iced::widget::svg::Style {
            color: Some(style(theme, button::Status::Active).text_color),
        })
}

async fn open_file() -> Result<Option<(PathBuf, String)>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Tessera", &[file::EXTENSION])
        .pick_file()
        .await
    else {
        return Ok(None);
    };

    let path = file.path().to_path_buf();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;

    Ok(Some((path, text)))
}

/// Writes `text` to `path`, or to a file picked first (suggesting `name`).
async fn save_file(
    path: Option<PathBuf>,
    name: String,
    text: String,
) -> Result<Option<PathBuf>, String> {
    let path = match path {
        Some(path) => path,
        None => {
            let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter("Tessera", &[file::EXTENSION])
                .set_file_name(name)
                .save_file()
                .await
            else {
                return Ok(None);
            };
            let mut path = file.path().to_path_buf();
            if path.extension().is_none() {
                path.set_extension(file::EXTENSION);
            }
            path
        }
    };

    std::fs::write(&path, text).map_err(|e| e.to_string())?;

    Ok(Some(path))
}

async fn pick_image() -> Result<Option<Vec<u8>>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };

    std::fs::read(file.path())
        .map(Some)
        .map_err(|e| e.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use document::Edit;
    use iced::Point;

    /// Adds a triangle, `x` to the right.
    fn edit(app: &mut Tessera, x: f32) {
        let corners = [
            Point::new(x + 100.0, 300.0),
            Point::new(x + 300.0, 300.0),
            Point::new(x + 200.0, 100.0),
        ];
        let revision = app.document.revision();
        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::AddTriangle { corners, snap: 0.0 }],
            revision,
        }));
    }

    #[test]
    fn another_menu_opens_straight_away() {
        let mut app = Tessera::default();

        // Hovering the menu bar opens nothing by itself.
        let _ = app.update(Message::HoverMenu(Menu::View));
        assert_eq!(app.menu, None);

        let _ = app.update(Message::ToggleMenu(Menu::File));
        assert_eq!(app.menu, Some(Menu::File));
        let _ = app.update(Message::HoverMenu(Menu::View));
        assert_eq!(app.menu, Some(Menu::View));
        let _ = app.update(Message::ToggleMenu(Menu::File));
        assert_eq!(app.menu, Some(Menu::File));
        let _ = app.update(Message::ToggleMenu(Menu::File));
        assert_eq!(app.menu, None);
    }

    #[test]
    fn notices_fade_out() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), app.versions(), None));

        let since = app.notice.as_ref().unwrap().since;
        assert_eq!(app.notice.as_ref().unwrap().text, "Saved drawing.tessera");
        let at = |ms| Message::Frame(since + Duration::from_millis(ms));

        let _ = app.update(at(2_000));
        assert_eq!(app.notice.as_ref().unwrap().opacity(), 1.0);
        let _ = app.update(at(3_300));
        let opacity = app.notice.as_ref().unwrap().opacity();
        assert!(opacity > 0.0 && opacity < 1.0, "{opacity}");
        let _ = app.update(at(3_700));
        assert!(app.notice.is_none());

        // Errors stay longer.
        let _ = app.update(Message::Exported(Err("disk full".into())));
        let since = app.notice.as_ref().unwrap().since;
        let _ = app.update(Message::Frame(since + Duration::from_secs(5)));
        assert_eq!(app.notice.as_ref().unwrap().opacity(), 1.0);
    }

    #[test]
    fn undo_and_redo_step_through_edits() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let one = app.document.to_parts();
        edit(&mut app, 500.0);
        let two = app.document.to_parts();

        let _ = app.update(Message::Undo);
        assert_eq!(app.document.to_parts(), one);
        let _ = app.update(Message::Undo);
        assert!(app.document.is_empty());
        let _ = app.update(Message::Undo); // nothing left: no change
        assert!(app.document.is_empty());
        assert!(!app.is_unsaved(), "back where it started");

        let _ = app.update(Message::Redo);
        let _ = app.update(Message::Redo);
        assert_eq!(app.document.to_parts(), two);
        assert!(app.is_unsaved());

        // A new edit after undoing drops what could be redone.
        let _ = app.update(Message::Undo);
        edit(&mut app, 1000.0);
        assert!(app.redo.is_empty());
        let _ = app.update(Message::Redo);
        assert_eq!(app.document.triangle_ids().len(), 2);
    }

    #[test]
    fn undoing_to_what_was_saved_is_saved() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), app.versions(), None));

        edit(&mut app, 500.0);
        assert!(app.is_unsaved());
        let _ = app.update(Message::Undo);
        assert!(!app.is_unsaved());
    }

    #[test]
    fn edits_for_another_revision_are_ignored() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let stale = app.document.revision();
        let _ = app.update(Message::Undo);

        // Worked out before the undo (say, mid-drag): no longer applies.
        let corners = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(5.0, 10.0),
        ];
        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::AddTriangle { corners, snap: 0.0 }],
            revision: stale,
        }));
        assert!(app.document.is_empty());
    }

    #[test]
    fn new_starts_a_fresh_history() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let _ = app.update(Message::Replace(Replace::New));
        assert!(app.undo.is_empty() && app.redo.is_empty());
    }

    #[test]
    fn painting_remembers_the_colour_and_undoes() {
        let mut app = Tessera::default();
        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::AddTriangle {
                corners: [
                    Point::new(0.0, 0.0),
                    Point::new(10.0, 0.0),
                    Point::new(5.0, 10.0),
                ],
                snap: 0.0,
            }],
            revision: app.document.revision(),
        }));
        let _ = app.update(Message::SetMode(Mode::Paint));
        let _ = app.update(Message::HexTyped("#ff0000".into()));
        let red = Color::from_rgb8(255, 0, 0);
        assert_eq!(app.brush, Brush::Color(red));

        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::Paint {
                triangle: 0,
                color: Some(red),
            }],
            revision: app.document.revision(),
        }));
        assert_eq!(app.document.color(0), Some(red));
        assert_eq!(app.recent, vec![red]);

        let _ = app.update(Message::Undo);
        assert_eq!(app.document.color(0), None);
        assert_eq!(app.document.triangle_ids().len(), 1);
    }

    fn with_background() -> Tessera {
        let mut app = Tessera::default();
        let _ = app.update(Message::BackgroundPicked(Ok(Some(
            background::tests::pixel(),
        ))));
        app
    }

    #[test]
    fn a_background_image_fills_the_view() {
        let mut app = with_background();
        assert!(app.background.is_some());
        assert!(app.is_unsaved());

        // The window, less the menu bar: 800×600 for a 4×2 image.
        let _ = app.update(Message::BackgroundFitted(iced::Size::new(
            800.0,
            600.0 + MENU_BAR_HEIGHT,
        )));
        let background = app.background.as_ref().unwrap();
        assert!((background.scale - 200.0).abs() < 1e-3);
        assert_eq!(background.center, Point::new(400.0, 300.0));
    }

    #[test]
    fn the_background_panel_adjusts_the_image() {
        let mut app = with_background();
        let _ = app.update(Message::ToggleBackgroundMode);
        assert!(app.editing_background);
        assert_eq!(app.fields.scale, "100.00");

        let _ = app.update(Message::BackgroundField(Field::X, "12.5".into()));
        let _ = app.update(Message::BackgroundField(Field::Scale, "50".into()));
        let _ = app.update(Message::BackgroundField(Field::Rotation, "90".into()));
        // Half typed: kept as typed, the image left alone.
        let _ = app.update(Message::BackgroundField(Field::Y, "-".into()));
        let background = app.background.as_ref().unwrap();
        assert_eq!(background.center, Point::new(12.5, 0.0));
        assert_eq!(background.scale, 0.5);
        assert!((background.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert_eq!(app.fields.y, "-");

        // Dragging on the canvas updates the fields.
        let _ = app.update(Message::Editor(editor::Message::MoveBackground(
            iced::Vector::new(1.0, 2.0),
        )));
        assert_eq!(app.fields.x, "13.5");
        assert_eq!(app.fields.y, "2.0");

        let _ = app.update(Message::ToggleBackgroundVisible);
        assert!(!app.background.as_ref().unwrap().visible);
        let _ = app.update(Message::RemoveBackground);
        assert!(app.background.is_none());
    }

    #[test]
    fn changing_the_image_keeps_its_place() {
        let mut app = with_background();
        let _ = app.update(Message::BackgroundField(Field::X, "40".into()));
        let _ = app.update(Message::ToggleBackgroundVisible);
        let _ = app.update(Message::BackgroundPicked(Ok(Some(
            background::tests::pixel(),
        ))));
        assert_eq!(app.background.as_ref().unwrap().center.x, 40.0);
        // A new image is always shown.
        assert!(app.background.as_ref().unwrap().visible);
    }

    #[test]
    fn the_background_counts_as_content() {
        let mut app = with_background();
        let _ = app.update(Message::Saved(
            Ok(Some(PathBuf::from("/tmp/traced.tessera"))),
            app.versions(),
            None,
        ));
        assert!(!app.is_unsaved());
        let _ = app.update(Message::BackgroundOpacity(0.9));
        assert!(app.is_unsaved());

        // New asks, and clears it.
        let _ = app.update(Message::New);
        assert!(matches!(app.confirming, Some(Replace::New)));
        let _ = app.update(Message::Replace(Replace::New));
        assert!(app.background.is_none());
        assert!(!app.is_unsaved());
    }

    #[test]
    fn new_on_a_blank_canvas_does_nothing() {
        let mut app = Tessera::default();
        app.camera.zoom = 2.0;
        let _ = app.update(Message::New);
        assert!(app.confirming.is_none());
        assert_eq!(app.camera.zoom, 2.0);
    }

    #[test]
    fn new_asks_before_losing_changes() {
        let mut app = Tessera::default();
        assert!(!app.is_unsaved());
        edit(&mut app, 0.0);
        assert!(app.is_unsaved());
        assert_eq!(app.title(), "Untitled • — Tessera");

        let _ = app.update(Message::New);
        assert!(matches!(app.confirming, Some(Replace::New)));
        let _ = app.update(Message::Confirm(Choice::Cancel));
        assert!(app.confirming.is_none());
        assert!(!app.document.is_empty());

        // "Don't save" goes on with it: a blank document and view.
        app.camera.zoom = 2.0;
        let _ = app.update(Message::Replace(Replace::New));
        assert!(app.document.is_empty());
        assert_eq!(app.camera.zoom, 1.0);
        assert!(!app.is_unsaved());
    }

    #[test]
    fn saved_changes_need_no_asking() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), app.versions(), None));

        assert!(!app.is_unsaved());
        assert_eq!(app.title(), "drawing.tessera — Tessera");
        let _ = app.update(Message::New);
        assert!(app.confirming.is_none());

        // A cancelled save leaves new changes unsaved.
        edit(&mut app, 500.0);
        let _ = app.update(Message::Saved(Ok(None), app.versions(), None));
        assert!(app.is_unsaved());
    }

    #[test]
    fn opening_restores_the_document_and_view() {
        let mut source = Tessera::default();
        edit(&mut source, 0.0);
        source.camera.zoom = 3.0;
        let text = file::save(&source.document, source.camera, None);

        let mut app = Tessera::default();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Opened(Ok(Some((path.clone(), text)))));

        assert_eq!(app.document.to_parts(), source.document.to_parts());
        assert_eq!(app.camera.zoom, 3.0);
        assert_eq!(app.path, Some(path));
        assert!(!app.is_unsaved());
    }
}
