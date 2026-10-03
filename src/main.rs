mod background;
mod camera;
mod compass;
mod dialogs;
mod disk;
mod document;
mod editor;
mod fade;
mod file;
mod geometry;
mod icons;
mod joints;
mod keys;
mod layers;
mod page;
mod paint;
mod panels;
mod places;
mod raster;
mod recent;
mod snap;
mod svg;
mod update;
mod wheel;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::keyboard::{self, Key};
use iced::time::{Duration, Instant};
use iced::widget::{
    button, center, checkbox, column, container, mouse_area, opaque, row, scrollable, slider,
    space, stack, text, text_input, tooltip,
};
use iced::window;
use iced::{Center, Color, Element, Fill, Padding, Subscription, Task, Theme};

use background::Background;
use camera::Camera;
use dialogs::dropping_hint;
use disk::{open_file, pick_image, read_file, save_file, save_png, save_svg};
use document::{Document, Edit};
use editor::Tool;
use keys::{Action, Chord, Context, Keymap};
use layers::{Layers, NodeId, Place, Signature};
use paint::{Brush, Hsv, Target};
use panels::{joined, tip};
use update::{
    BackgroundMessage, ExportMessage, FileMessage, Format, KeysMessage, LayerAction, MenuMessage,
    PageDialog, PageMessage, PaintMessage, ShapeMessage, SnapMessage,
};

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
/// How far proportional editing may reach (screen px): least, most.
const REACH: (f32, f32) = (5.0, 1000.0);
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
/// The layers panel's width, right of the canvas.
const LAYERS_WIDTH: f32 = 250.0;
/// A line's height in the layers panel.
const LAYER_ROW: f32 = 26.0;
/// The layer name field, to focus when renaming.
const NAME_FIELD: &str = "layer-name";

#[derive(Default)]
struct Tessera {
    /// The data: the layers, each drawn on only through `Document::apply`;
    /// or replaced as a whole (new, open).
    layers: Layers,
    /// The layer being edited (see [`Self::current`]).
    current: NodeId,
    /// The layer or group selected in the layers panel.
    selected: NodeId,
    /// The layer or group whose name is being typed: its renaming is one
    /// undo step.
    renaming: Option<NodeId>,
    /// The layer or group whose name is shown as a field to type in.
    naming: Option<NodeId>,
    /// A layer or group being dragged in the layers panel, and where
    /// dropping it would put it.
    dragging: Option<(NodeId, Option<Place>)>,
    /// The layer (or group) pointed at in the layers panel: lit up on the
    /// canvas.
    pointed_layer: Option<NodeId>,
    /// The layers and groups isolated in the layers panel: while any are,
    /// only they (and what's in them) are in view. Not part of the drawing.
    isolated: HashSet<NodeId>,
    /// The layers as they were before each change, most recent last; and
    /// as they were before each undo. Unchanged drawings are shared.
    undo: Vec<Layers>,
    redo: Vec<Layers>,
    /// View state, independent of the data.
    camera: Camera,
    /// What's drawn on the canvas; see `editor::Caches` for when to clear
    /// which.
    caches: editor::Caches,
    /// Where the document was last saved to or opened from.
    path: Option<PathBuf>,
    /// The layers as last saved or opened; `None` for a new document
    /// (which starts out saved). Anything else has unsaved changes; moving
    /// the view doesn't count.
    saved: Option<Signature>,
    /// The menu that is open, if any.
    menu: Option<Menu>,
    /// Whether the about box is open.
    about: bool,
    /// Asking what to do with unsaved changes before doing this.
    confirming: Option<Replace>,
    /// A short message about what just happened, fading out.
    notice: Option<Notice>,
    /// An image to draw over, kept with the document (not part of undo).
    /// The layers' background images (each layer's own), by layer: kept
    /// apart from the layers, so undoing edits doesn't move them.
    backgrounds: HashMap<NodeId, Background>,
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
    /// Files opened or saved lately, most recent first.
    recent_files: recent::Recent,
    /// Where the user left off in each file lately.
    places: places::Places,
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
    /// Proportional editing (shape mode): moving vertices takes those within
    /// `reach` (screen px, so zoomed out it reaches further) along, the less
    /// the further.
    proportional: bool,
    reach: f32,
    /// The layers before the fade width slider was dragged: the drag is one
    /// undo step.
    fading_from: Option<Layers>,
    /// Whether the next click on the canvas picks up a brush (the pipette).
    picking: bool,
    /// Placing a mirror on the current layer (Ctrl+M).
    placing_mirror: bool,
    /// A file dragged over the window: what dropping it would do is shown.
    dropping: Option<PathBuf>,
    /// What was copied (Ctrl+C), to paste on any layer (Ctrl+V).
    clipboard: Option<document::Piece>,
    /// Which keys do what (kept in the config folder).
    keys: Keymap,
    /// Exporting: its dialog open (for what), whether just the document is
    /// exported (its size set), and how large a PNG is and on what (as last
    /// chosen).
    export_open: Option<Format>,
    export_to_page: bool,
    png_scale: f32,
    png_backdrop: raster::Backdrop,
    /// The keyboard shortcuts shown (F1); and one being set: the next key
    /// pressed goes to this action (instead of its key at that place, or
    /// as another).
    keys_open: bool,
    /// The document size dialog, as filled in so far, while it's open.
    page_dialog: Option<PageDialog>,
    /// How dragged points snap (kept in the config folder); and its
    /// dialog's grid step, as typed, while it's open.
    snap: snap::Snap,
    snap_dialog: Option<String>,
    rebinding: Option<(Action, Option<usize>)>,
    /// The modifier keys held (Shift turns painting a layer into painting
    /// all layers).
    modifiers: keyboard::Modifiers,
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
    Help,
}

/// Things that replace (or close) the document, so unsaved changes would be
/// lost.
#[derive(Debug, Clone)]
enum Replace {
    New,
    Open,
    /// Open this file (a recent one).
    OpenPath(PathBuf),
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
    Layers(LayerAction),
    SetMode(Mode),
    ToggleMode,
    WindowResized(iced::Size),
    Undo,
    Redo,
    ResetView,
    CloseRequested(window::Id),
    Key(keyboard::Event),
    /// A frame, while a notice fades.
    Frame(Instant),
    Paint(PaintMessage),
    Shape(ShapeMessage),
    Keys(KeysMessage),
    File(FileMessage),
    Export(ExportMessage),
    Background(BackgroundMessage),
    Menu(MenuMessage),
    Page(PageMessage),
    Snap(SnapMessage),
}

/// What was saved: the layers and background version.
#[derive(Debug, Clone)]
struct Versions {
    layers: Signature,
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
            .as_ref()
            .is_some_and(|saved| *saved != self.layers.signature());
        (edited && !self.layers.is_empty()) || self.background_version != self.saved_background
    }

    /// Nothing drawn, no background: nothing a new document would change.
    fn is_blank(&self) -> bool {
        self.layers.is_empty()
            && !self
                .layers
                .layers()
                .iter()
                .any(|layer| self.backgrounds.contains_key(&layer.id))
    }

    /// The layer being edited: `current`, or (if that's gone, e.g. after an
    /// undo) the frontmost.
    fn current(&self) -> NodeId {
        if self.layers.layer(self.current).is_some() {
            self.current
        } else {
            self.layers.first_layer()
        }
    }

    /// The current layer's background image, if it has one.
    fn background(&self) -> Option<&Background> {
        self.backgrounds.get(&self.current())
    }

    fn background_mut(&mut self) -> Option<&mut Background> {
        let current = self.current();
        self.backgrounds.get_mut(&current)
    }

    /// The current layer's drawing.
    fn document(&self) -> &Document {
        &self
            .layers
            .layer(self.current())
            .expect("current layer")
            .document
    }

    /// Before changing the layers: `before` becomes an undo step.
    fn push_undo(&mut self, before: Layers) {
        // A new document starts out saved.
        self.saved.get_or_insert_with(|| before.signature());
        self.undo.push(before);
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// After the layers were replaced (undo, new, open): keeps the current
    /// and selected layer to ones that exist.
    fn layers_replaced(&mut self) {
        self.current = self.current();
        if !self.layers.contains(self.selected) {
            self.selected = self.current;
        }
        self.renaming = None;
        self.naming = None;
        self.dragging = None;
        // The document size may be another.
        self.caches.grid.clear();
    }

    /// Paints every face (or edge) of the current layer, or of all layers,
    /// with the brush; one undo step.
    fn paint_everything(&mut self, all_layers: bool) {
        let edit = match self.target {
            Target::Faces => Edit::PaintAll {
                color: self.brush.color(),
            },
            Target::Edges => Edit::PaintAllEdges {
                style: self.brush.color().map(|color| document::EdgeStyle {
                    color,
                    width: self.edge_width,
                }),
            },
        };
        let ids: Vec<_> = if all_layers {
            self.layers.layers().iter().map(|layer| layer.id).collect()
        } else {
            vec![self.current()]
        };
        let before = self.layers.clone();
        for id in ids {
            let layer = self.layers.layer_mut(id).expect("layer");
            Arc::make_mut(&mut layer.document).apply(edit.clone());
        }
        if let Some(color) = self.brush.color() {
            paint::remember(&mut self.recent, color);
        }
        self.push_undo(before);
    }

    /// Paints with `brush` from now on; the colour controls follow.
    fn set_brush(&mut self, brush: Brush) {
        self.brush = brush;
        if let Brush::Color(color) = brush {
            self.hsv = Hsv::from_color(color, self.hsv.hue);
            self.hex = paint::to_hex(color);
        }
    }

    /// Selects a line in the layers panel; a layer becomes the one edited.
    fn select_layer(&mut self, id: NodeId) {
        self.selected = id;
        if self.layers.layer(id).is_some() && id != self.current() {
            self.current = id;
        }
    }

    /// The canvas's size in a window of this size.
    fn canvas_size(window: iced::Size) -> iced::Size {
        iced::Size::new(window.width - LAYERS_WIDTH, window.height - MENU_BAR_HEIGHT)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            keyboard::listen().map(Message::Key),
            window::close_requests().map(Message::CloseRequested),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
            // Files dragged onto the window. (Not yet on Wayland: winit,
            // which iced uses, doesn't report them there before 0.31.)
            iced::event::listen_with(|event, _, _| match event {
                iced::Event::Window(window::Event::FileHovered(path)) => {
                    Some(Message::File(FileMessage::Hovered(Some(path))))
                }
                iced::Event::Window(window::Event::FilesHoveredLeft) => {
                    Some(Message::File(FileMessage::Hovered(None)))
                }
                iced::Event::Window(window::Event::FileDropped(path)) => {
                    Some(Message::File(FileMessage::Dropped(path)))
                }
                _ => None,
            }),
            // Letting go of a dragged layer anywhere drops it.
            if self.dragging.is_some() {
                iced::event::listen_with(|event, _, _| match event {
                    iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                        iced::mouse::Button::Left,
                    )) => Some(Message::Layers(LayerAction::Drop)),
                    _ => None,
                })
            } else {
                Subscription::none()
            },
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
            reach: 100.0,
            keys: Keymap::load(),
            png_scale: 2.0,
            export_to_page: true,
            snap: snap::Snap::load(),
            brush,
            recent_files: recent::Recent::load(),
            places: places::Places::load(),
            ..Tessera::default()
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let current = self.current();
        let task = self.handle(message);
        // Another layer, another background image.
        if self.current() != current {
            self.caches.grid.clear();
        }
        task
    }

    /// What `message` does (see `update`).
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Editor(message) => self.update_canvas(message),
            Message::Layers(action) => self.layer_action(action),
            Message::Paint(message) => self.update_paint(message),
            Message::Shape(message) => self.update_shape(message),
            Message::Keys(message) => self.update_keys(message),
            Message::File(message) => self.update_file(message),
            Message::Export(message) => self.update_export(message),
            Message::Background(message) => self.update_background(message),
            Message::Menu(message) => self.update_menu(message),
            Message::Page(message) => self.update_page(message),
            Message::Snap(message) => self.update_snap(message),
            Message::Undo => {
                self.menu = None;
                let Some(before) = self.undo.pop() else {
                    return Task::none();
                };
                self.redo.push(std::mem::replace(&mut self.layers, before));
                self.layers_replaced();
                Task::none()
            }
            Message::Redo => {
                self.menu = None;
                let Some(after) = self.redo.pop() else {
                    return Task::none();
                };
                self.undo.push(std::mem::replace(&mut self.layers, after));
                self.layers_replaced();
                Task::none()
            }
            Message::Compass(compass::Message::TurnTo(rotation)) => {
                // Around the middle of the canvas.
                let canvas = Self::canvas_size(self.window_size.unwrap_or(WINDOW_SIZE));
                let anchor = iced::Point::new(canvas.width / 2.0, canvas.height / 2.0);
                self.camera.rotate_to(anchor, rotation);
                self.view_changed()
            }
            Message::SetMode(mode) => {
                if mode == Mode::Shape {
                    self.picking = false;
                }
                if mode != self.mode {
                    self.mode = mode;
                }
                Task::none()
            }
            Message::ToggleMode => {
                let mode = match self.mode {
                    Mode::Shape => Mode::Paint,
                    Mode::Paint => Mode::Shape,
                };
                self.update(Message::SetMode(mode))
            }
            Message::WindowResized(size) => {
                self.window_size = Some(size);
                Task::none()
            }
            Message::ResetView => {
                self.menu = None;
                self.camera = Camera::default();
                self.view_changed()
            }
            Message::CloseRequested(id) => self.replace(Replace::Quit(id)),
            Message::Key(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) => self.key_pressed(key, physical_key, modifiers),
            Message::Key(keyboard::Event::ModifiersChanged(modifiers)) => {
                self.modifiers = modifiers;
                Task::none()
            }
            Message::Key(_) => Task::none(),
            Message::Frame(now) => {
                if let Some(notice) = &mut self.notice {
                    notice.shown = now.saturating_duration_since(notice.since);
                    if notice.opacity() <= 0.0 {
                        self.notice = None;
                    }
                }
                Task::none()
            }
        }
    }

    /// After the view (or the drawing as a whole) changed: the backdrop
    /// (background image and grid) is drawn anew.
    fn view_changed(&mut self) -> Task<Message> {
        self.caches.grid.clear();
        Task::none()
    }

    /// A key pressed (anywhere in the window): setting a key for an action,
    /// Esc, or what the key does as the user has it (if it's not the
    /// canvas's).
    fn key_pressed(
        &mut self,
        key: Key,
        physical_key: keyboard::key::Physical,
        modifiers: keyboard::Modifiers,
    ) -> Task<Message> {
        let escape = key == Key::Named(keyboard::key::Named::Escape);
        // Setting a key: the next one pressed (Esc: never mind).
        if let Some((action, index)) = self.rebinding {
            if escape {
                self.rebinding = None;
            } else if let Some(chord) = Chord::pressed(&key, physical_key, modifiers) {
                self.rebinding = None;
                let taken = self.keys.bind(action, index, chord.clone());
                if !taken.is_empty() {
                    let from: Vec<_> = taken.iter().map(|a| a.label()).collect();
                    self.notify(
                        format!("{chord} was taken from: {}", from.join(", ")),
                        false,
                    );
                }
            }
            return Task::none();
        }
        if escape {
            self.menu = None;
            self.confirming = None;
            self.about = false;
            self.keys_open = false;
            self.export_open = None;
            self.page_dialog = None;
            self.snap_dialog = None;
            self.editing_background = false;
            self.picking = false;
            self.placing_mirror = false;
            return Task::none();
        }
        if self.confirming.is_some()
            || self.about
            || self.keys_open
            || self.export_open.is_some()
            || self.page_dialog.is_some()
            || self.snap_dialog.is_some()
        {
            return Task::none();
        }
        let context = match self.mode {
            Mode::Shape => Context::Shape,
            Mode::Paint => Context::Paint,
        };
        let Some(action) = Chord::pressed(&key, physical_key, modifiers)
            .and_then(|chord| self.keys.action(&chord, context))
        else {
            return Task::none();
        };
        // The rest are the canvas's.
        let message = match action {
            Action::ToggleMode => Message::ToggleMode,
            Action::New => Message::File(FileMessage::New),
            Action::Open => Message::File(FileMessage::Open),
            Action::Save => Message::File(FileMessage::Save),
            Action::SaveAs => Message::File(FileMessage::SaveAs),
            Action::Undo => Message::Undo,
            Action::Redo => Message::Redo,
            Action::Mirror => Message::Shape(ShapeMessage::PlaceMirror(!self.placing_mirror)),
            Action::DuplicateLayer => Message::Layers(LayerAction::Duplicate),
            Action::ShowKeys => Message::Keys(KeysMessage::ShowKeys(true)),
            Action::Proportional => Message::Shape(ShapeMessage::ToggleProportional),
            Action::PaintFaces => Message::Paint(PaintMessage::SetTarget(Target::Faces)),
            Action::PaintEdges => Message::Paint(PaintMessage::SetTarget(Target::Edges)),
            Action::Pipette => Message::Paint(PaintMessage::TogglePicking),
            _ => return Task::none(),
        };
        self.update(message)
    }

    /// What saving now would save.
    fn versions(&self) -> Versions {
        Versions {
            layers: self.layers.signature(),
            background: self.background_version,
        }
    }

    /// After the background changed: it's unsaved, and needs drawing.
    fn background_changed(&mut self) {
        self.background_version += 1;
        self.caches.grid.clear();
        self.refresh_fields();
    }

    /// Fills the background panel's fields in from the background.
    fn refresh_fields(&mut self) {
        let Some(background) = self.background() else {
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
        // Leaving this file (unless that's cancelled): where the user was.
        self.remember_place();
        if self.is_unsaved() {
            self.confirming = Some(replace);
            Task::none()
        } else {
            Task::done(Message::File(FileMessage::Replace(replace)))
        }
    }

    /// Remembers where the user is in the file (the current layer, the
    /// view), for opening it again; if it's a file.
    fn remember_place(&mut self) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let current = self.current();
        let layer = self
            .layers
            .layers()
            .iter()
            .position(|layer| layer.id == current)
            .unwrap_or(0);
        self.places
            .remember(path, places::Place::new(layer, self.camera));
    }

    /// Saves to where the document came from, or (if `choose`, or it never
    /// was saved) to a file picked first; then does `then`.
    fn save(&mut self, choose: bool, then: Option<Replace>) -> Task<Message> {
        let text = file::save(&self.layers, self.camera, &self.backgrounds, self.current());
        let versions = self.versions();
        let path = self.path.clone().filter(|_| !choose);
        let name = match &self.path {
            Some(_) => self.name(),
            None => format!("untitled.{}", file::EXTENSION),
        };

        Task::perform(save_file(path, name, text), move |result| {
            Message::File(FileMessage::Saved(result, versions, then))
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
                .on_press(Message::Menu(MenuMessage::ToggleMenu(menu)));
            // With a menu open, moving over another opens that one instead.
            mouse_area(button).on_enter(Message::Menu(MenuMessage::HoverMenu(menu)))
        };

        let toolbar = row![
            menu_button("File", Menu::File),
            menu_button("Edit", Menu::Edit),
            menu_button("View", Menu::View),
            menu_button("Help", Menu::Help),
            self.mode_switch(),
            space::horizontal(),
            button(text(self.hint()).size(12))
                .padding([3, 10])
                .style(button::secondary)
                .on_press(Message::Keys(KeysMessage::ShowKeys(true))),
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
                    self.document().vertex_count(),
                    self.document().unique_edges().len(),
                    self.document().triangle_ids().len(),
                ))
                .size(12),
            ]
            .spacing(8),
        )
        .padding([4, 8])
        .style(container::dark);

        let canvas = stack![
            {
                let current = self.current();
                let (below, above) = self.layers.around(current, &self.isolated);
                let scene = editor::Scene {
                    current: scene_layer(self.layers.layer(current).expect("current layer")),
                    shown: self.layers.in_view(current, &self.isolated),
                    below: below.into_iter().map(scene_layer).collect(),
                    above: above.into_iter().map(scene_layer).collect(),
                };
                editor::view(
                    scene,
                    self.camera,
                    &self.caches,
                    self.background(),
                    editor::Settings {
                        tool: self.tool(),
                        proportional: self.proportional.then_some(self.reach),
                        clipboard: self.clipboard.as_ref(),
                        // Not while they're being looked up (or set).
                        keys: (!self.keys_open).then_some(&self.keys),
                        // Back to front; hidden ones not, though out of view
                        // while others are isolated.
                        lit: self
                            .pointed_layer
                            .map(|id| self.layers.layers_of(id))
                            .unwrap_or_default()
                            .into_iter()
                            .rev()
                            .filter(|&id| self.layers.shown(id))
                            .filter_map(|id| self.layers.layer(id))
                            .map(scene_layer)
                            .collect(),
                        page: self.layers.page(),
                        grid: editor::Grid {
                            step: self.snap.grid,
                        },
                    },
                )
            }
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
            self.shape_panel(),
            self.mirror_bar(),
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
            .on_press(Message::Menu(MenuMessage::CloseMenu)),
            row![canvas, self.layers_panel()],
        ]];
        if let Some(menu) = self.menu {
            screen = screen.push(self.dropdown(menu));
        }
        if self.about {
            screen = screen.push(self.about_box());
        }
        if self.keys_open {
            screen = screen.push(self.shortcuts());
        }
        if let Some(format) = self.export_open {
            screen = screen.push(self.export_dialog(format));
        }
        if let Some(dialog) = &self.page_dialog {
            screen = screen.push(self.page_dialog(dialog));
        }
        if let Some(step) = &self.snap_dialog {
            screen = screen.push(self.snap_dialog(step));
        }
        if let Some(path) = &self.dropping {
            screen = screen.push(dropping_hint(path));
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
            _ if self.placing_mirror => Tool::Mirror,
            Mode::Shape => Tool::Shape,
            Mode::Paint => Tool::Paint {
                target: self.target,
                brush: self.brush,
                width: self.edge_width,
                picking: self.picking,
            },
        }
    }
}

/// A layer as the canvas gets it.
fn scene_layer(layer: &layers::Layer) -> editor::SceneLayer<'_> {
    editor::SceneLayer {
        id: layer.id,
        document: &layer.document,
        crossfade: layer.crossfade,
        show_edges: layer.show_edges,
        mirrors: layer.mirror.as_slice(),
    }
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
        let revision = app.document().revision();
        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::AddTriangle { corners, snap: 0.0 }],
            revision,
        }));
    }

    #[test]
    fn a_document_is_placed_with_its_corner_on_the_grid() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let page = |app: &mut Tessera, message| {
            let _ = app.update(Message::Page(message));
        };
        // Odd sizes, around the triangle and then where it was: its corner
        // lands on the (10 px) grid either way.
        for (width, height) in [("1005", "703"), ("333", "777")] {
            page(&mut app, PageMessage::Show(true));
            page(&mut app, PageMessage::Width(width.into()));
            page(&mut app, PageMessage::Height(height.into()));
            page(&mut app, PageMessage::Apply);
            let corner = app.layers.page().unwrap().min();
            for side in [corner.x, corner.y] {
                assert!(
                    (side / 10.0 - (side / 10.0).round()).abs() < 1e-4,
                    "{corner:?}"
                );
            }
        }
    }

    #[test]
    fn the_document_size_is_set_in_its_dialog_and_undone() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let page = |app: &mut Tessera, message| {
            let _ = app.update(Message::Page(message));
        };

        page(&mut app, PageMessage::Show(true));
        page(&mut app, PageMessage::Preset(page::Preset::FullHd));
        let dialog = app.page_dialog.as_ref().unwrap();
        assert_eq!((&*dialog.width, &*dialog.height), ("1920", "1080"));
        page(&mut app, PageMessage::Turn);
        page(&mut app, PageMessage::Apply);
        assert!(app.page_dialog.is_none());
        // Around the triangle, (100, 100)–(300, 300).
        let set = app.layers.page().expect("set");
        assert_eq!(set.size, iced::Size::new(1080.0, 1920.0));
        assert_eq!(set.center, Point::new(200.0, 200.0));
        // All of it in view.
        let canvas = Tessera::canvas_size(WINDOW_SIZE);
        for corner in set.corners() {
            let p = app.camera.to_screen(corner);
            assert!(
                (0.0..=canvas.width).contains(&p.x) && (0.0..=canvas.height).contains(&p.y),
                "{p:?}"
            );
        }

        // A custom size, where it was.
        page(&mut app, PageMessage::Show(true));
        assert!(!app.page_dialog.as_ref().unwrap().centre);
        page(&mut app, PageMessage::Width("1000".into()));
        page(&mut app, PageMessage::Height("700".into()));
        assert_eq!(
            app.page_dialog.as_ref().unwrap().preset,
            page::Preset::Custom
        );
        page(&mut app, PageMessage::Apply);
        let custom = app.layers.page().unwrap();
        assert_eq!(custom.size, iced::Size::new(1000.0, 700.0));
        assert_eq!(custom.center, set.center);

        // A size that won't do isn't applied.
        page(&mut app, PageMessage::Show(true));
        page(&mut app, PageMessage::Width("0".into()));
        page(&mut app, PageMessage::Apply);
        assert!(app.page_dialog.is_some());
        page(&mut app, PageMessage::Remove);
        assert_eq!(app.layers.page(), None);

        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.page(), Some(custom));
        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.page(), Some(set));
    }

    #[test]
    fn layers_hold_their_own_geometry() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let back = app.current();

        let _ = app.update(Message::Layers(LayerAction::Add));
        let front = app.current();
        assert_ne!(front, back);
        assert!(app.document().is_empty());
        // The same triangle again: it would overlap on one layer, not on two.
        edit(&mut app, 0.0);
        assert_eq!(app.document().triangle_ids().len(), 1);
        edit(&mut app, 0.0);
        assert_eq!(app.document().triangle_ids().len(), 1);

        // Drawn around the current layer, back to front.
        let (below, above) = app.layers.around(front, &Default::default());
        assert_eq!((below.len(), above.len()), (1, 0));

        // Back to the first layer; its drawing is untouched.
        let _ = app.update(Message::Layers(LayerAction::Press(back)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        assert_eq!(app.current(), back);
        assert_eq!(app.document().triangle_ids().len(), 1);

        // Undo goes back over the edit on the front layer, then its adding.
        let _ = app.update(Message::Undo);
        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.layers().len(), 1);
        assert_eq!(app.current(), back);
        assert!(app.is_unsaved());
    }

    #[test]
    fn dragging_a_layer_moves_it() {
        let mut app = Tessera::default();
        let back = app.current();
        let _ = app.update(Message::Layers(LayerAction::Add));
        let front = app.current();
        let names = |app: &Tessera| {
            app.layers
                .rows()
                .iter()
                .map(|row| row.name.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&app), ["Layer 2", "Layer 1"]);

        // Onto itself: nothing; then onto the lower half of the other.
        let _ = app.update(Message::Layers(LayerAction::Press(front)));
        let _ = app.update(Message::Layers(LayerAction::Hover(front, 0.9)));
        assert_eq!(app.dragging, Some((front, None)));
        let _ = app.update(Message::Layers(LayerAction::Hover(back, 0.9)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        assert_eq!(names(&app), ["Layer 1", "Layer 2"]);
        assert_eq!(app.dragging, None);

        // Into a group; out again to the very back.
        let _ = app.update(Message::Layers(LayerAction::Press(back)));
        let _ = app.update(Message::Layers(LayerAction::Group));
        let group = app.selected;
        let _ = app.update(Message::Layers(LayerAction::Press(front)));
        let _ = app.update(Message::Layers(LayerAction::Hover(group, 0.5)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
        assert_eq!(depths, [0, 1, 1]);
        let _ = app.update(Message::Layers(LayerAction::Press(front)));
        let _ = app.update(Message::Layers(LayerAction::HoverEnd));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
        assert_eq!(depths, [0, 1, 0]);

        // Letting go outside the list: nothing.
        let _ = app.update(Message::Layers(LayerAction::Press(front)));
        let _ = app.update(Message::Layers(LayerAction::Hover(group, 0.1)));
        let _ = app.update(Message::Layers(LayerAction::Leave));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        assert_eq!(app.layers.rows().last().unwrap().id, front);

        // Each move is an undo step.
        let _ = app.update(Message::Undo);
        let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
        assert_eq!(depths, [0, 1, 1]);
    }

    #[test]
    fn painting_a_whole_layer_or_all_layers() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let back = app.current();
        let _ = app.update(Message::Layers(LayerAction::Add));
        edit(&mut app, 0.0);
        let red = Color::from_rgb8(255, 0, 0);
        let _ = app.update(Message::Paint(PaintMessage::PickBrush(Brush::Color(red))));
        let painted = |app: &Tessera| {
            app.layers
                .layers()
                .iter()
                .map(|layer| layer.document.colors().all(|c| c == Some(red)))
                .collect::<Vec<_>>()
        };

        let _ = app.update(Message::Paint(PaintMessage::PaintEverything(false)));
        assert_eq!(painted(&app), [true, false]);
        let _ = app.update(Message::Undo);
        assert_eq!(painted(&app), [false, false]);

        let _ = app.update(Message::Paint(PaintMessage::PaintEverything(true)));
        assert_eq!(painted(&app), [true, true]);
        assert_eq!(app.recent, vec![red]);
        // Edges too, the current layer only.
        let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
        let _ = app.update(Message::Paint(PaintMessage::PaintEverything(false)));
        let red_edges = |id| {
            let document = &app.layers.layer(id).unwrap().document;
            document
                .edge_styles()
                .filter(|(_, _, style)| style.color == red)
                .count()
        };
        assert_eq!((red_edges(app.current()), red_edges(back)), (3, 0));
    }

    #[test]
    fn a_duplicate_is_worked_on_leaving_the_original_be() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let original = app.current();
        let faces = app.document().triangle_ids().len();

        let _ = app.update(Message::Layers(LayerAction::Duplicate));
        let copy = app.current();
        assert_ne!(copy, original);
        assert_eq!(app.layers.layers().len(), 2);
        // Going nuts with the copy: the original keeps its drawing.
        edit(&mut app, 400.0);
        assert_eq!(app.document().triangle_ids().len(), faces + 1);
        let kept = &app.layers.layer(original).unwrap().document;
        assert_eq!(kept.triangle_ids().len(), faces);

        // One undo step each.
        let _ = app.update(Message::Undo);
        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.layers().len(), 1);
    }

    #[test]
    fn pointing_at_a_layer_lights_it_up_until_off_it() {
        let mut app = Tessera::default();
        let back = app.current();
        let _ = app.update(Message::Layers(LayerAction::Add));
        let front = app.current();
        let before = app.layers.signature();

        let _ = app.update(Message::Layers(LayerAction::Point(back)));
        assert_eq!(app.pointed_layer, Some(back));
        // Onto the next line before off the last: the next one stays.
        let _ = app.update(Message::Layers(LayerAction::Point(front)));
        let _ = app.update(Message::Layers(LayerAction::Unpoint(back)));
        assert_eq!(app.pointed_layer, Some(front));
        let _ = app.update(Message::Layers(LayerAction::Unpoint(front)));
        assert_eq!(app.pointed_layer, None);
        // Not a change (nothing to undo).
        assert_eq!(app.layers.signature(), before);
        assert_eq!(app.undo.len(), 1);
    }

    #[test]
    fn opening_a_file_again_picks_up_where_it_was_left() {
        let mut app = Tessera::new();
        edit(&mut app, 0.0);
        let _ = app.update(Message::Layers(LayerAction::Add));
        let text = file::save(&app.layers, app.camera, &app.backgrounds, app.current());
        let path = PathBuf::from("/drawings/flower.tessera");
        let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
            path.clone(),
            text.clone(),
        ))))));

        // Working on the back layer, zoomed in; then leaving the file.
        let back_index = 1;
        let back = app.layers.layers()[back_index].id;
        let _ = app.update(Message::Layers(LayerAction::Press(back)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        app.camera.zoom = 3.0;
        app.camera.pan = iced::Vector::new(12.0, -4.0);
        let _ = app.update(Message::File(FileMessage::New));
        let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
        assert_eq!(app.camera.zoom, 1.0);

        let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((path, text))))));
        assert_eq!(app.camera.zoom, 3.0);
        assert_eq!(app.camera.pan, iced::Vector::new(12.0, -4.0));
        assert_eq!(app.current(), app.layers.layers()[back_index].id);
    }

    #[test]
    fn files_dropped_on_the_window_open_or_go_behind_the_layer() {
        use update::Dropped;
        assert_eq!(
            Dropped::of(Path::new("/a/flower.tessera")),
            Dropped::Drawing
        );
        assert_eq!(
            Dropped::of(Path::new("/a/FLOWER.TESSERA")),
            Dropped::Drawing
        );
        assert_eq!(Dropped::of(Path::new("/a/photo.JPG")), Dropped::Image);
        assert_eq!(Dropped::of(Path::new("/a/scan.png")), Dropped::Image);
        assert_eq!(Dropped::of(Path::new("/a/notes.txt")), Dropped::Other);

        let mut app = Tessera::default();
        // Dragged over, and off again: the hint comes and goes.
        let path = PathBuf::from("/a/flower.tessera");
        let _ = app.update(Message::File(FileMessage::Hovered(Some(path.clone()))));
        assert_eq!(app.dropping, Some(path.clone()));
        let _ = app.update(Message::File(FileMessage::Hovered(None)));
        assert_eq!(app.dropping, None);

        // A drawing dropped with changes unsaved: asked first.
        edit(&mut app, 0.0);
        let _ = app.update(Message::File(FileMessage::Dropped(path.clone())));
        assert!(matches!(&app.confirming, Some(Replace::OpenPath(p)) if *p == path));
        app.confirming = None;

        // Neither: said so.
        let _ = app.update(Message::File(FileMessage::Dropped("/a/notes.txt".into())));
        assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
    }

    #[test]
    fn deleting_a_group_with_every_layer_in_it() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let _ = app.update(Message::Layers(LayerAction::Add));
        edit(&mut app, 0.0);
        // Both in one group, then that group in another.
        let _ = app.update(Message::Layers(LayerAction::Group));
        let inner = app.selected;
        let back = app.layers.layers()[1].id;
        let _ = app.update(Message::Layers(LayerAction::Press(back)));
        let _ = app.update(Message::Layers(LayerAction::Hover(inner, 0.5)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        let _ = app.update(Message::Layers(LayerAction::Press(inner)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        assert_eq!(app.layers.nodes().len(), 1);

        let _ = app.update(Message::Layers(LayerAction::Remove));
        assert_eq!(app.layers.layers().len(), 1);
        assert!(app.document().is_empty());
        assert_eq!(app.current(), app.layers.first_layer());
        assert_eq!(app.selected, app.current());

        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.layers().len(), 2);
        assert!(app.layers.contains(inner));
    }

    #[test]
    fn the_pipette_picks_up_a_brush() {
        let mut app = Tessera::default();
        let _ = app.update(Message::Paint(PaintMessage::TogglePicking));
        assert_eq!(app.mode, Mode::Paint);
        assert!(app.picking);

        let red = Color::from_rgb8(255, 0, 0);
        let pick = |app: &mut Tessera, picked| {
            let _ = app.update(Message::Editor(editor::Message::Pick(picked)));
        };
        pick(&mut app, editor::Picked::Face(Some(red)));
        assert!(!app.picking);
        assert_eq!(app.brush, Brush::Color(red));

        pick(
            &mut app,
            editor::Picked::Edge(Some(document::EdgeStyle {
                color: red,
                width: 4.5,
            })),
        );
        assert_eq!(app.edge_width, 4.5);
        // Unpainted: the eraser, to paint it back like that.
        pick(&mut app, editor::Picked::Edge(None));
        assert_eq!(app.brush, Brush::Eraser);

        let _ = app.update(Message::Paint(PaintMessage::TogglePicking));
        let _ = app.update(Message::SetMode(Mode::Shape));
        assert!(!app.picking);
    }

    #[test]
    fn crossfading_edges_is_per_layer_and_undoable() {
        let mut app = Tessera::default();
        let back = app.current();
        let _ = app.update(Message::Layers(LayerAction::Add));
        let front = app.current();
        let crossfade = |app: &Tessera, id| app.layers.layer(id).unwrap().crossfade;

        let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
        let _ = app.update(Message::Paint(PaintMessage::Crossfade(true)));
        assert!(crossfade(&app, front).edges);
        assert_eq!(crossfade(&app, back), layers::Crossfade::default());

        let _ = app.update(Message::Undo);
        assert!(!crossfade(&app, front).edges);
    }

    #[test]
    fn the_fade_width_and_hiding_edges_are_undoable() {
        let mut app = Tessera::default();
        let layer = app.current();
        let look = |app: &Tessera| {
            let layer = app.layers.layer(layer).unwrap();
            (layer.crossfade.width, layer.show_edges)
        };
        let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
        let _ = app.update(Message::Paint(PaintMessage::Crossfade(true)));
        // Dragging the slider: one step.
        for width in [8.0, 12.0, 20.0] {
            let _ = app.update(Message::Paint(PaintMessage::FadeWidth(width)));
        }
        let _ = app.update(Message::Paint(PaintMessage::FadeWidthDone));
        assert_eq!(look(&app), (20.0, true));
        let _ = app.update(Message::Paint(PaintMessage::ShowEdges(false)));
        assert_eq!(look(&app), (20.0, false));

        let _ = app.update(Message::Undo);
        assert_eq!(look(&app), (20.0, true));
        let _ = app.update(Message::Undo);
        assert_eq!(look(&app), (layers::Crossfade::WIDTH, true));
    }

    #[test]
    fn recent_files() {
        let mut app = Tessera::default();
        let text = file::save(&app.layers, app.camera, &app.backgrounds, app.current());
        let path = PathBuf::from("/drawings/flower.tessera");
        let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
            path.clone(),
            text,
        ))))));
        assert_eq!(app.recent_files.paths(), std::slice::from_ref(&path));

        // With unsaved changes, opening a recent file asks first.
        edit(&mut app, 0.0);
        let _ = app.update(Message::File(FileMessage::OpenRecent(path.clone())));
        assert!(matches!(&app.confirming, Some(Replace::OpenPath(p)) if *p == path));

        // Gone: said so, and forgotten.
        let _ = app.update(Message::File(FileMessage::RecentFailed(
            path,
            "No such file".into(),
        )));
        assert!(app.recent_files.paths().is_empty());
        assert!(app.notice.as_ref().unwrap().error);

        // Saving (as) remembers it too.
        let saved = PathBuf::from("/drawings/copy.tessera");
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(Some(saved.clone())),
            app.versions(),
            None,
        )));
        assert_eq!(app.recent_files.paths(), [saved]);
    }

    #[test]
    fn alt_and_moving_tweaks_the_edge_width() {
        let mut app = Tessera {
            edge_width: 2.0,
            ..Tessera::default()
        };
        let start = app.edge_width;
        let _ = app.update(Message::Editor(editor::Message::TweakWidth(100.0)));
        assert!((app.edge_width - start * 1f32.exp()).abs() < 1e-4);
        let _ = app.update(Message::Editor(editor::Message::TweakWidth(-100.0)));
        assert!((app.edge_width - start).abs() < 1e-4);
        // Within the slider's range.
        let _ = app.update(Message::Editor(editor::Message::TweakWidth(10_000.0)));
        assert_eq!(app.edge_width, 12.0);
        let _ = app.update(Message::Editor(editor::Message::TweakWidth(-10_000.0)));
        assert_eq!(app.edge_width, 0.5);
    }

    #[test]
    fn the_about_box_opens_and_closes() {
        let mut app = Tessera::default();
        let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::Help)));
        let _ = app.update(Message::Menu(MenuMessage::About(true)));
        assert!(app.about);
        assert_eq!(app.menu, None);
        let _ = app.update(Message::Key(keyboard::Event::KeyPressed {
            key: Key::Named(keyboard::key::Named::Escape),
            modified_key: Key::Named(keyboard::key::Named::Escape),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        }));
        assert!(!app.about);
    }

    #[test]
    fn proportional_editing_toggles_and_reaches_as_far_as_set() {
        let mut app = Tessera::new();
        assert!(!app.proportional);
        let _ = app.update(Message::Shape(ShapeMessage::ToggleProportional));
        assert!(app.proportional);
        let start = app.reach;
        let _ = app.update(Message::Editor(editor::Message::TweakReach(100.0)));
        assert!((app.reach - start * 1f32.exp()).abs() < 1e-3);
        let _ = app.update(Message::Editor(editor::Message::TweakReach(-10_000.0)));
        assert_eq!(app.reach, REACH.0);
        let _ = app.update(Message::Shape(ShapeMessage::Reach(1e6)));
        assert_eq!(app.reach, REACH.1);
    }

    #[test]
    fn a_mirror_is_placed_moved_applied_and_undone() {
        let mut app = Tessera::new();
        edit(&mut app, 0.0);
        let faces = app.document().triangle_ids().len();
        let _ = app.update(Message::Shape(ShapeMessage::PlaceMirror(true)));
        assert_eq!(app.tool(), Tool::Mirror);
        let (_, max) = app.document().bounds().unwrap();
        let at = |x: f32| document::Mirror {
            a: iced::Point::new(x, 0.0),
            b: iced::Point::new(x, 10.0),
        };
        let _ = app.update(Message::Editor(editor::Message::SetMirror(
            at(max.x + 50.0),
        )));
        assert!(!app.placing_mirror);
        // Placed again: instead.
        let mirror = at(max.x + 20.0);
        let _ = app.update(Message::Editor(editor::Message::SetMirror(mirror)));
        let current = app.current();
        assert_eq!(app.layers.layer(current).unwrap().mirror, Some(mirror));

        let _ = app.update(Message::Shape(ShapeMessage::ApplyMirror));
        assert_eq!(app.document().triangle_ids().len(), 2 * faces);
        assert_eq!(app.layers.layer(current).unwrap().mirror, None);

        // One undo step back to mirrored, but not applied.
        let _ = app.update(Message::Undo);
        assert_eq!(app.document().triangle_ids().len(), faces);
        assert_eq!(app.layers.layer(current).unwrap().mirror, Some(mirror));
    }

    #[test]
    fn keys_are_looked_up_and_set() {
        let mut app = Tessera::new();
        let press = |app: &mut Tessera, key: Key, modifiers| {
            let _ = app.update(Message::Key(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            }));
        };
        let none = keyboard::Modifiers::empty();
        let letter = |c: &str| Key::Character(c.into());

        press(&mut app, Key::Named(keyboard::key::Named::F1), none);
        assert!(app.keys_open);
        // While they're open, keys don't do what they do.
        press(&mut app, Key::Named(keyboard::key::Named::Tab), none);
        assert_eq!(app.mode, Mode::Shape);

        // Switching modes on Q instead of Tab.
        let _ = app.update(Message::Keys(KeysMessage::Rebind(Some((
            Action::ToggleMode,
            Some(0),
        )))));
        press(&mut app, letter("q"), none);
        assert_eq!(app.rebinding, None);
        assert_eq!(app.keys.first(Action::ToggleMode), "Q");
        // Esc while setting one: never mind; then closes.
        let _ = app.update(Message::Keys(KeysMessage::Rebind(Some((
            Action::ToggleMode,
            None,
        )))));
        press(&mut app, Key::Named(keyboard::key::Named::Escape), none);
        assert!(app.keys_open);
        assert_eq!(app.keys.keys(Action::ToggleMode).len(), 1);
        press(&mut app, Key::Named(keyboard::key::Named::Escape), none);
        assert!(!app.keys_open);

        // Now Q switches, Tab doesn't.
        press(&mut app, Key::Named(keyboard::key::Named::Tab), none);
        assert_eq!(app.mode, Mode::Shape);
        press(&mut app, letter("q"), none);
        assert_eq!(app.mode, Mode::Paint);

        let _ = app.update(Message::Keys(KeysMessage::ResetKeys));
        assert_eq!(app.keys, Keymap::default());
    }

    #[test]
    fn c_and_moving_tweaks_the_lightness() {
        let mut app = Tessera::default();
        let _ = app.update(Message::Paint(PaintMessage::WheelPicked(Hsv {
            hue: 0.6,
            saturation: 0.8,
            value: 0.52,
        })));
        let _ = app.update(Message::Editor(editor::Message::TweakLightness(50.0)));
        assert!((app.hsv.value - 0.72).abs() < 1e-5);
        // Through black and back, and through white and back: the same
        // colour again.
        let before = app.hex.clone();
        let _ = app.update(Message::Editor(editor::Message::TweakLightness(-1000.0)));
        assert_eq!(app.hex, "#000000");
        let _ = app.update(Message::Editor(editor::Message::TweakLightness(180.0)));
        assert_eq!(app.hex, before);
        let _ = app.update(Message::Editor(editor::Message::TweakLightness(1000.0)));
        assert_eq!(app.hex, "#ffffff");
        let _ = app.update(Message::Editor(editor::Message::TweakLightness(-320.0)));
        assert_eq!(app.hex, before);
        assert_eq!(app.brush, Brush::Color(app.hsv.to_color()));
    }

    #[test]
    fn renaming_is_one_undo_step() {
        let mut app = Tessera::default();
        let layer = app.current();
        let _ = app.update(Message::Layers(LayerAction::Press(layer)));
        let _ = app.update(Message::Layers(LayerAction::Drop));
        for name in ["S", "Sk", "Sketch"] {
            let _ = app.update(Message::Layers(LayerAction::Rename(name.into())));
        }
        assert_eq!(app.layers.rows()[0].name, "Sketch");
        let _ = app.update(Message::Undo);
        assert_eq!(app.layers.rows()[0].name, "Layer 1");
    }

    #[test]
    fn another_menu_opens_straight_away() {
        let mut app = Tessera::default();

        // Hovering the menu bar opens nothing by itself.
        let _ = app.update(Message::Menu(MenuMessage::HoverMenu(Menu::View)));
        assert_eq!(app.menu, None);

        let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
        assert_eq!(app.menu, Some(Menu::File));
        let _ = app.update(Message::Menu(MenuMessage::HoverMenu(Menu::View)));
        assert_eq!(app.menu, Some(Menu::View));
        let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
        assert_eq!(app.menu, Some(Menu::File));
        let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
        assert_eq!(app.menu, None);
    }

    #[test]
    fn notices_fade_out() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(Some(path)),
            app.versions(),
            None,
        )));

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
        let _ = app.update(Message::Export(ExportMessage::Exported(Err(
            "disk full".into()
        ))));
        let since = app.notice.as_ref().unwrap().since;
        let _ = app.update(Message::Frame(since + Duration::from_secs(5)));
        assert_eq!(app.notice.as_ref().unwrap().opacity(), 1.0);
    }

    #[test]
    fn undo_and_redo_step_through_edits() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let one = app.document().to_parts();
        edit(&mut app, 500.0);
        let two = app.document().to_parts();

        let _ = app.update(Message::Undo);
        assert_eq!(app.document().to_parts(), one);
        let _ = app.update(Message::Undo);
        assert!(app.document().is_empty());
        let _ = app.update(Message::Undo); // nothing left: no change
        assert!(app.document().is_empty());
        assert!(!app.is_unsaved(), "back where it started");

        let _ = app.update(Message::Redo);
        let _ = app.update(Message::Redo);
        assert_eq!(app.document().to_parts(), two);
        assert!(app.is_unsaved());

        // A new edit after undoing drops what could be redone.
        let _ = app.update(Message::Undo);
        edit(&mut app, 1000.0);
        assert!(app.redo.is_empty());
        let _ = app.update(Message::Redo);
        assert_eq!(app.document().triangle_ids().len(), 2);
    }

    #[test]
    fn undoing_to_what_was_saved_is_saved() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(Some(path)),
            app.versions(),
            None,
        )));

        edit(&mut app, 500.0);
        assert!(app.is_unsaved());
        let _ = app.update(Message::Undo);
        assert!(!app.is_unsaved());
    }

    #[test]
    fn edits_for_another_revision_are_ignored() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let stale = app.document().revision();
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
        assert!(app.document().is_empty());
    }

    #[test]
    fn new_starts_a_fresh_history() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
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
            revision: app.document().revision(),
        }));
        let _ = app.update(Message::SetMode(Mode::Paint));
        let _ = app.update(Message::Paint(PaintMessage::HexTyped("#ff0000".into())));
        let red = Color::from_rgb8(255, 0, 0);
        assert_eq!(app.brush, Brush::Color(red));

        let _ = app.update(Message::Editor(editor::Message::Edit {
            edits: vec![Edit::Paint {
                triangle: 0,
                color: Some(red),
            }],
            revision: app.document().revision(),
        }));
        assert_eq!(app.document().color(0), Some(red));
        assert_eq!(app.recent, vec![red]);

        // Back to the colour it had: the default, as made.
        let _ = app.update(Message::Undo);
        assert_eq!(app.document().color(0), Some(document::DEFAULT_FACE));
        assert_eq!(app.document().triangle_ids().len(), 1);
    }

    fn with_background() -> Tessera {
        let mut app = Tessera::default();
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundPicked(
            Ok(Some(background::tests::pixel())),
        )));
        app
    }

    #[test]
    fn each_layer_has_its_own_background_image() {
        let mut app = with_background();
        let first = app.current();
        assert!(app.background().is_some());

        // A new layer: none of its own.
        let _ = app.update(Message::Layers(LayerAction::Add));
        assert_ne!(app.current(), first);
        assert!(app.background().is_none());
        let _ = app.update(Message::Layers(LayerAction::Press(first)));
        assert!(app.background().is_some());

        // A duplicate: its own, of the same image (not a copy of it).
        let _ = app.update(Message::Layers(LayerAction::Duplicate));
        let copy = app.current();
        assert_ne!(copy, first);
        let (a, b) = (&app.backgrounds[&first], &app.backgrounds[&copy]);
        assert!(a.same_image(b));
        // Moving the copy's leaves the original's be.
        let _ = app.update(Message::Editor(editor::Message::MoveBackground(
            iced::Vector::new(5.0, 0.0),
        )));
        assert_ne!(
            app.backgrounds[&first].center,
            app.backgrounds[&copy].center
        );
    }

    #[test]
    fn a_background_image_fills_the_view() {
        let mut app = with_background();
        assert!(app.background().is_some());
        assert!(app.is_unsaved());

        // The window, less the menu bar: 800×600 for a 4×2 image.
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundFitted(
            iced::Size::new(800.0 + LAYERS_WIDTH, 600.0 + MENU_BAR_HEIGHT),
        )));
        let background = app.background().unwrap();
        assert!((background.scale - 200.0).abs() < 1e-3);
        assert_eq!(background.center, Point::new(400.0, 300.0));
    }

    #[test]
    fn the_background_panel_adjusts_the_image() {
        let mut app = with_background();
        let _ = app.update(Message::Background(BackgroundMessage::ToggleBackgroundMode));
        assert!(app.editing_background);
        assert_eq!(app.fields.scale, "100.00");

        let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
            Field::X,
            "12.5".into(),
        )));
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
            Field::Scale,
            "50".into(),
        )));
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
            Field::Rotation,
            "90".into(),
        )));
        // Half typed: kept as typed, the image left alone.
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
            Field::Y,
            "-".into(),
        )));
        let background = app.background().unwrap();
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

        let _ = app.update(Message::Background(
            BackgroundMessage::ToggleBackgroundVisible,
        ));
        assert!(!app.background().unwrap().visible);
        let _ = app.update(Message::Background(BackgroundMessage::RemoveBackground));
        assert!(app.background().is_none());
    }

    #[test]
    fn changing_the_image_keeps_its_place() {
        let mut app = with_background();
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
            Field::X,
            "40".into(),
        )));
        let _ = app.update(Message::Background(
            BackgroundMessage::ToggleBackgroundVisible,
        ));
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundPicked(
            Ok(Some(background::tests::pixel())),
        )));
        assert_eq!(app.background().unwrap().center.x, 40.0);
        // A new image is always shown.
        assert!(app.background().unwrap().visible);
    }

    #[test]
    fn the_background_counts_as_content() {
        let mut app = with_background();
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(Some(PathBuf::from("/tmp/traced.tessera"))),
            app.versions(),
            None,
        )));
        assert!(!app.is_unsaved());
        let _ = app.update(Message::Background(BackgroundMessage::BackgroundOpacity(
            0.9,
        )));
        assert!(app.is_unsaved());

        // New asks, and clears it.
        let _ = app.update(Message::File(FileMessage::New));
        assert!(matches!(app.confirming, Some(Replace::New)));
        let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
        assert!(app.background().is_none());
        assert!(!app.is_unsaved());
    }

    #[test]
    fn new_on_a_blank_canvas_does_nothing() {
        let mut app = Tessera::default();
        app.camera.zoom = 2.0;
        let _ = app.update(Message::File(FileMessage::New));
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

        let _ = app.update(Message::File(FileMessage::New));
        assert!(matches!(app.confirming, Some(Replace::New)));
        let _ = app.update(Message::File(FileMessage::Confirm(Choice::Cancel)));
        assert!(app.confirming.is_none());
        assert!(!app.document().is_empty());

        // "Don't save" goes on with it: a blank document and view.
        app.camera.zoom = 2.0;
        let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
        assert!(app.document().is_empty());
        assert_eq!(app.camera.zoom, 1.0);
        assert!(!app.is_unsaved());
    }

    #[test]
    fn saved_changes_need_no_asking() {
        let mut app = Tessera::default();
        edit(&mut app, 0.0);
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(Some(path)),
            app.versions(),
            None,
        )));

        assert!(!app.is_unsaved());
        assert_eq!(app.title(), "drawing.tessera — Tessera");
        let _ = app.update(Message::File(FileMessage::New));
        assert!(app.confirming.is_none());

        // A cancelled save leaves new changes unsaved.
        edit(&mut app, 500.0);
        let _ = app.update(Message::File(FileMessage::Saved(
            Ok(None),
            app.versions(),
            None,
        )));
        assert!(app.is_unsaved());
    }

    #[test]
    fn opening_restores_the_document_and_view() {
        let mut source = Tessera::default();
        edit(&mut source, 0.0);
        source.camera.zoom = 3.0;
        let text = file::save(
            &source.layers,
            source.camera,
            &source.backgrounds,
            source.current(),
        );

        let mut app = Tessera::default();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
            path.clone(),
            text,
        ))))));

        assert_eq!(app.document().to_parts(), source.document().to_parts());
        assert_eq!(app.camera.zoom, 3.0);
        assert_eq!(app.path, Some(path));
        assert!(!app.is_unsaved());
    }
}
