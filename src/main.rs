//! Tessera: drawing low-poly pictures out of triangles that share their
//! corners, then painting their faces and edges, layer by layer.
//!
//! An iced application in the Elm style: [`Tessera`] holds the state,
//! [`Message`] says what happened, `update` (by area, in [`update`]) changes
//! the state, and `view` (with [`panels`] and [`dialogs`]) shows it. The
//! canvas is [`editor`], a widget of its own that sends its own messages.

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
                    current: editor::SceneLayer::of(
                        self.layers.layer(current).expect("current layer"),
                    ),
                    shown: self.layers.in_view(current, &self.isolated),
                    below: below.into_iter().map(editor::SceneLayer::of).collect(),
                    above: above.into_iter().map(editor::SceneLayer::of).collect(),
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
                            .map(editor::SceneLayer::of)
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

#[cfg(test)]
mod tests;
