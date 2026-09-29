mod camera;
mod document;
mod editor;
mod file;
mod geometry;
mod svg;

use std::path::{Path, PathBuf};

use iced::keyboard::{self, Key};
use iced::time::{Duration, Instant};
use iced::widget::{
    button, canvas, center, column, container, mouse_area, opaque, row, space, stack, text,
};
use iced::window;
use iced::{Center, Color, Element, Fill, Padding, Subscription, Task, Theme};

use camera::Camera;
use document::Document;

pub fn main() -> iced::Result {
    iced::application(Tessera::default, Tessera::update, Tessera::view)
        .title(Tessera::title)
        .subscription(Tessera::subscription)
        .theme(Theme::TokyoNight)
        .exit_on_close_request(false)
        .centered()
        .run()
}

/// Width of a menu's button in the menu bar.
const MENU_WIDTH: f32 = 64.0;
/// How many edits can be undone.
const MAX_UNDO: usize = 200;
/// How long a notice takes to fade out.
const NOTICE_FADE: Duration = Duration::from_millis(600);
/// The menu bar's height: where menus drop down from.
const MENU_BAR_HEIGHT: f32 = 34.0;

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
    Saved(Result<Option<PathBuf>, String>, u64, Option<Replace>),
    Exported(Result<Option<PathBuf>, String>),
    CloseRequested(window::Id),
    Key(keyboard::Event),
    /// A frame, while a notice fades.
    Frame(Instant),
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
        edited && !self.document.is_empty()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            keyboard::listen().map(Message::Key),
            window::close_requests().map(Message::CloseRequested),
            if self.notice.is_some() {
                window::frames().map(Message::Frame)
            } else {
                Subscription::none()
            },
        ])
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
                if self.document.is_empty() {
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
                    self.document = contents.document;
                    self.undo.clear();
                    self.redo.clear();
                    self.camera = contents.camera;
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
            Message::Saved(result, revision, then) => {
                match result {
                    Ok(Some(path)) => {
                        self.path = Some(path);
                        self.notify(format!("Saved {}", self.name()), false);
                        self.saved = Some(revision);
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
                    return Task::none();
                }
                if !modifiers.command() || self.confirming.is_some() {
                    return Task::none();
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
        let text = file::save(&self.document, self.camera);
        let revision = self.document.revision();
        let path = self.path.clone().filter(|_| !choose);
        let name = match &self.path {
            Some(_) => self.name(),
            None => format!("untitled.{}", file::EXTENSION),
        };

        Task::perform(save_file(path, name, text), move |result| {
            Message::Saved(result, revision, then)
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
            space::horizontal(),
            text("Drag corner: move · Drag edge: extend · Drag blank: new tri · C: cut edge · D: delete tri · Middle-drag: pan · Wheel: zoom · Shift+wheel: rotate")
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
            editor::view(&self.document, self.camera, &self.cache, &self.grid).map(Message::Editor),
            container(
                column![self.notice(), stats]
                    .spacing(6)
                    .align_x(iced::Right)
            )
            .align_right(Fill)
            .align_bottom(Fill)
            .padding(10),
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
        let revision = app.document.revision();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), revision, None));

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
        let revision = app.document.revision();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), revision, None));

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
        let revision = app.document.revision();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Saved(Ok(Some(path)), revision, None));

        assert!(!app.is_unsaved());
        assert_eq!(app.title(), "drawing.tessera — Tessera");
        let _ = app.update(Message::New);
        assert!(app.confirming.is_none());

        // A cancelled save leaves new changes unsaved.
        edit(&mut app, 500.0);
        let _ = app.update(Message::Saved(Ok(None), app.document.revision(), None));
        assert!(app.is_unsaved());
    }

    #[test]
    fn opening_restores_the_document_and_view() {
        let mut source = Tessera::default();
        edit(&mut source, 0.0);
        source.camera.zoom = 3.0;
        let text = file::save(&source.document, source.camera);

        let mut app = Tessera::default();
        let path = PathBuf::from("/tmp/drawing.tessera");
        let _ = app.update(Message::Opened(Ok(Some((path.clone(), text)))));

        assert_eq!(app.document.to_parts(), source.document.to_parts());
        assert_eq!(app.camera.zoom, 3.0);
        assert_eq!(app.path, Some(path));
        assert!(!app.is_unsaved());
    }
}
