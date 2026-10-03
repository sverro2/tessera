//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns input into [`Message`]s. It never mutates the document itself.
//!
//! In the shape mode, left-drag on blank canvas creates a triangle; from
//! inside a face, it extends from the last edge the cursor crossed; from a
//! corner, it moves it. Only middle-drag pans; the wheel zooms around the
//! cursor, Shift+wheel rotates around it. Keys (as the user has them; see
//! [`crate::keys`]) cut the hovered edge, delete, and so on.
//!
//! Q, then a drag, draws a lasso, adding the vertices inside it to the
//! selection; Shift+click adds or removes a vertex. The selection is then
//! dragged, grabbed, rotated or scaled, copied and pasted (see
//! [`selection`]).
//!
//! In the paint mode, clicking a face or dragging over faces paints them
//! with the brush; the shape can't be changed (see [`painting`]).
//!
//! A layer with a mirror shows its mirror image, which is edited (and
//! painted) like the drawing itself (see [`mirror`]).
//!
//! A dragged point snaps onto nearby vertices and edges, and the mirror line (see
//! [`Editor::snaps`]); holding Shift, it lines up with guides (see
//! [`guides`]). The document then joins things up by itself, so the editor
//! only decides where points go.
//!
//! Here: the canvas's state, its events (one handler each) and changing the
//! shape; drawing it is in [`draw`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use rayon::prelude::*;

use iced::keyboard;
use iced::mouse;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{
    self, Canvas, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use iced::widget::stack;
use iced::window;
use iced::{Color, Element, Fill, Point, Rectangle, Renderer, Size, Theme, Vector};

use crate::background::Background;
use crate::camera::Camera;
use crate::document::{self as doc, EdgeStyle, Mirror, Piece};
use crate::document::{Changes, Document, Edit, TriangleId, VertexId, opposite};
use crate::fade;
use crate::geometry::{
    Affine, area2, closest_on_line, closest_on_segment, inside_polygon, min_height,
};
use crate::icons;
use crate::joints;
use crate::keys::{Action, Chord, Context, Keymap};
use crate::layers::{Crossfade, NodeId};
use crate::page::Page;
use crate::paint::{self, Brush};

mod draw;
mod extrude;
mod grid;
mod guides;
mod meshes;
mod mirror;
mod painting;
mod selection;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use draw::*;
use meshes::{Drawn, EditorView, LayerMeshes};
pub use grid::Grid;
use grid::MAJOR;
use guides::*;
use mirror::*;
use selection::*;

/// Screen-space distance within which a corner is grabbed.
const VERTEX_HIT: f32 = 10.0;
/// Screen-space distance within which an edge is grabbed.
const EDGE_HIT: f32 = 7.0;
/// Minimum height (screen px) of any triangle we create; thinner slivers
/// would be invisible and impossible to grab.
const MIN_THICKNESS: f32 = 5.0;
/// How far (screen px) inside a limiting edge a dragged vertex stops: just
/// clear of making its triangle with that edge a sliver.
const SLIDE_INSET: f32 = MIN_THICKNESS + 0.5;
/// How long working out what a drag does may take per frame: 60% of a frame
/// at 60 Hz, leaving the rest for drawing it. (The rare heavy drag may slow
/// a faster screen down to 60 frames a second; that's plenty.)
const BUDGET: Duration = Duration::from_millis(10);
/// How many snap targets to try (closest first) before giving up on
/// snapping.
const MAX_SNAPS: usize = 4;
/// How long the shape highlight takes to move over to another shape.
const SHAPE_FADE: Duration = Duration::from_millis(200);
/// Rotation per wheel notch with Shift held.
const ROTATE_STEP: f32 = 5.0 * std::f32::consts::PI / 180.0;

pub const BACKGROUND: Color = Color::from_rgb8(0x1a, 0x1b, 0x26);
pub const GRID_DOT: Color = Color::from_rgb8(0x2f, 0x33, 0x4d);
/// The document (its size set): a sheet a little lighter than around it,
/// and its edge.
const PAGE: Color = Color::from_rgb8(0x22, 0x24, 0x34);
const PAGE_EDGE: Color = Color::from_rgb8(0x56, 0x5f, 0x89);
const FILL: Color = Color::from_rgba8(0x7a, 0xa2, 0xf7, 0.35);
pub const EDGE: Color = Color::from_rgb8(0xc0, 0xca, 0xf5);
/// Washed over the shapes other than the highlighted one: fading them
/// into the background keeps their colour, just dimmer.
const UNFOCUSED: Color = BACKGROUND;
/// What an action would add (and the cursor that creates triangles).
const ADDED: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
/// What an action would remove.
const REMOVED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
/// Where an action joins things up: edges that become shared, vertices
/// welded together (and the cursor, when it lands on such a join).
const JOINED: Color = Color::from_rgb8(0xe0, 0xaf, 0x68);
/// The hovered vertex or edge.
pub const HOVER: Color = Color::from_rgb8(0x7d, 0xcf, 0xff);

/// What dragging (and the wheel) on the canvas does.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Tool {
    /// Change the shape: create, extend, move, cut, delete.
    #[default]
    Shape,
    /// Paint faces or edges with the brush (edges `width` wide, in world
    /// units); the shape can't be changed. `picking` (or holding Ctrl), a
    /// click picks the brush up from what's clicked instead.
    Paint {
        target: paint::Target,
        brush: Brush,
        width: f32,
        picking: bool,
    },
    /// Adjust the background image; the drawing rests, `painted` or not.
    Background { painted: bool },
    /// Place a mirror on the current layer: drag a line clear of its
    /// drawing.
    Mirror,
}

/// What the pipette picked up: a face's colour or an edge's style (`None`
/// if unpainted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Picked {
    Face(Option<Color>),
    Edge(Option<EdgeStyle>),
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The pipette picked this up.
    Pick(Picked),
    /// Holding W while painting edges, the mouse moved this far across
    /// (screen px): make the stroke wider (or, to the left, thinner).
    TweakWidth(f32),
    /// Holding C while painting, the mouse moved this far across (screen
    /// px): make the colour lighter (or, to the left, darker).
    TweakLightness(f32),
    /// Copied (Ctrl+C): the selected faces.
    Copy(Piece),
    /// Cut (Ctrl+X): the selected faces copied, and these edits removing
    /// them (for the document `revision`).
    Cut {
        piece: Piece,
        edits: Vec<Edit>,
        revision: u64,
    },
    /// A mirror was placed on the current layer (instead of the one it
    /// had).
    SetMirror(Mirror),
    /// Holding Shift+O, editing proportionally, the mouse
    /// moved this far across (screen px): reach further (or less far).
    TweakReach(f32),
    Pan(Vector),
    Zoom {
        anchor: Point,
        factor: f32,
    },
    /// Rotate clockwise by `angle` radians around `anchor`.
    Rotate {
        anchor: Point,
        angle: f32,
    },
    /// Fit these points (world) in view: the shape to frame.
    Frame(Vec<Point>),
    /// Move the background image by this much (world).
    MoveBackground(Vector),
    /// Scale the background image by `factor` around `anchor` (world).
    ScaleBackground {
        anchor: Point,
        factor: f32,
    },
    /// Rotate the background image clockwise by `angle` radians around
    /// `anchor` (world).
    RotateBackground {
        anchor: Point,
        angle: f32,
    },
    /// One user action, made of edits applied together (all or nothing).
    Edit {
        edits: Vec<Edit>,
        /// The document revision they were worked out for; against another
        /// (e.g. after an undo mid-drag) they may not make sense.
        revision: u64,
    },
}

/// What the canvas draws, cached so that as little as possible is drawn
/// anew: each layer apart, redrawn only when what it's drawn from changed
/// (its drawing, its look, the view); a drag's preview only draws the
/// current layer afresh.
#[derive(Default)]
pub struct Caches {
    /// The background image and grid; cleared when either changes.
    pub grid: canvas::Cache,
    /// Each layer's drawing.
    layers: RefCell<HashMap<NodeId, LayerCache>>,
    /// Painted edges' outlines worked out, by drawing (its revision).
    outlines: RefCell<HashMap<u64, EdgeOutlines>>,
}

/// A drawing's painted edges' outlines (world), as at `zoom`, and around
/// each vertex its painted edges (to work out more).
#[derive(Default)]
struct EdgeOutlines {
    zoom: f32,
    outlines: HashMap<(VertexId, VertexId), Vec<Point>>,
    around: HashMap<VertexId, Vec<(VertexId, VertexId)>>,
}

/// A layer as last drawn, and what that depended on.
struct LayerCache {
    key: Option<LayerKey>,
    cache: canvas::Cache,
    /// Its faces and painted edges as meshes (under and over what the
    /// canvas draws), if drawn with meshes.
    meshes: Option<(
        iced::advanced::graphics::mesh::Cache,
        iced::advanced::graphics::mesh::Cache,
    )>,
}

/// What a layer's drawing depends on.
#[derive(Debug, Clone, PartialEq)]
struct LayerKey {
    revision: u64,
    look: Look,
    crossfade: Crossfade,
    show_edges: bool,
    vertices: bool,
    mirrors: Vec<Mirror>,
    camera: (Vector, f32, f32, Option<Affine>),
    size: Size,
    /// Drawn with meshes (else all on the canvas).
    meshes: bool,
}

/// A layer on the canvas.
#[derive(Clone, Copy)]
pub struct SceneLayer<'a> {
    pub id: NodeId,
    pub document: &'a Document,
    pub crossfade: Crossfade,
    /// Whether its edges are drawn in the paint mode.
    pub show_edges: bool,
    /// The mirrors it's shown mirrored across, one after the other (a
    /// layer has one at most).
    pub mirrors: &'a [Mirror],
}

/// The layers on the canvas: the current one and the other visible ones.
pub struct Scene<'a> {
    /// The current layer: the one edited.
    pub current: SceneLayer<'a>,
    /// Whether the current layer is shown (it and the groups it's in are
    /// visible). Hidden, it's neither drawn nor edited.
    pub shown: bool,
    /// The other visible layers behind and in front of it, back to front.
    pub below: Vec<SceneLayer<'a>>,
    pub above: Vec<SceneLayer<'a>>,
}

/// How the canvas edits: with what, and how.
pub struct Settings<'a> {
    pub tool: Tool,
    /// Proportional editing: how far it reaches (screen px), if on.
    pub proportional: Option<f32>,
    /// What pasting pastes.
    pub clipboard: Option<&'a Piece>,
    /// Which keys do what; none while they're being looked up (or set).
    pub keys: Option<&'a Keymap>,
    /// Layers to light up (pointed at in the layers panel), back to
    /// front; also those out of view while others are isolated.
    pub lit: Vec<SceneLayer<'a>>,
    /// The document size, if one's set: shown as a sheet behind it all.
    pub page: Option<Page>,
    /// The grid points snap to, holding Ctrl.
    pub grid: Grid,
}

/// The canvas for editing the current layer of `scene`, over the backdrop
/// (background image and grid).
pub fn view<'a>(
    scene: Scene<'a>,
    camera: Camera,
    caches: &'a Caches,
    background: Option<&'a Background>,
    settings: Settings<'a>,
) -> Element<'a, Message> {
    let Scene {
        current,
        shown,
        below,
        above,
    } = scene;
    let Settings {
        tool,
        proportional,
        clipboard,
        keys,
        lit,
        page,
        grid,
    } = settings;
    let backdrop = Canvas::new(Backdrop {
        cache: &caches.grid,
        background,
        camera,
        page,
        grid,
    });
    let editor = EditorView::new(Editor {
        document: current.document,
        current: current.id,
        crossfade: current.crossfade,
        show_edges: current.show_edges,
        mirrors: current.mirrors,
        shown,
        below,
        above,
        camera,
        caches,
        background,
        tool,
        proportional,
        clipboard,
        keys,
        lit,
        grid,
        deadline: Cell::new(None),
    });
    // Apart, as images (the background) are drawn after shapes within a
    // canvas: the stack draws the drawing in a pass of its own, over it.
    stack![backdrop.width(Fill).height(Fill), Element::from(editor)].into()
}

/// The background colour, the document (its size set), the background
/// image and the grid.
struct Backdrop<'a> {
    cache: &'a canvas::Cache,
    background: Option<&'a Background>,
    camera: Camera,
    page: Option<Page>,
    /// The snapping grid, the dot grid shows sparser.
    grid: Grid,
}

impl canvas::Program<Message> for Backdrop<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        vec![self.cache.draw(renderer, bounds.size(), |frame| {
            frame.fill_rectangle(Point::ORIGIN, frame.size(), BACKGROUND);
            let sheet = self.page.map(|page| {
                let [first, rest @ ..] = page.corners().map(|p| self.camera.to_screen(p));
                Path::new(|path| {
                    path.move_to(first);
                    for p in rest {
                        path.line_to(p);
                    }
                    path.close();
                })
            });
            if let Some(sheet) = &sheet {
                frame.fill(sheet, PAGE);
            }
            if let Some(background) = self.background {
                background.draw(frame, self.camera);
            }
            draw_grid(frame, self.camera, self.grid);
            if let (Some(sheet), Some(page)) = (&sheet, self.page) {
                frame.stroke(sheet, stroke(PAGE_EDGE, 1.0));
                // Its size, above its top left corner (as seen).
                let corner = self.camera.to_screen(page.min());
                frame.fill_text(canvas::Text {
                    content: format!("{:.0} × {:.0}", page.size.width, page.size.height),
                    position: corner + Vector::new(0.0, -18.0),
                    color: PAGE_EDGE,
                    size: 12.0.into(),
                    ..canvas::Text::default()
                });
            }
        })]
    }
}

/// An editor's view of the drawing, without what can't go to another
/// thread (its caches): to work out drags on several cores.
#[derive(Clone, Copy)]
struct Worker<'a> {
    document: &'a Document,
    current: NodeId,
    crossfade: Crossfade,
    show_edges: bool,
    mirrors: &'a [Mirror],
    shown: bool,
    camera: Camera,
    background: Option<&'a Background>,
    tool: Tool,
    proportional: Option<f32>,
    clipboard: Option<&'a Piece>,
    keys: Option<&'a Keymap>,
    grid: Grid,
    deadline: Option<std::time::Instant>,
}

impl<'a> Worker<'a> {
    /// An editor to work with, on this thread, drawing into `caches` (none
    /// is drawn).
    fn editor(self, caches: &'a Caches) -> Editor<'a> {
        Editor {
            document: self.document,
            current: self.current,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            lit: Vec::new(),
            grid: self.grid,
            deadline: Cell::new(self.deadline),
        }
    }
}

struct Editor<'a> {
    /// The current layer: the one edited.
    document: &'a Document,
    /// Its id, whether its paint crossfades, and whether its edges show
    /// in the paint mode.
    current: NodeId,
    crossfade: Crossfade,
    show_edges: bool,
    /// The mirrors it's mirrored across, one after the other: edits that
    /// would make it overlap a mirror image are turned down.
    mirrors: &'a [Mirror],
    /// Whether the current layer is shown; hidden, it's neither drawn nor
    /// edited.
    shown: bool,
    /// The other visible layers, back to front: only drawn (and picked
    /// from).
    below: Vec<SceneLayer<'a>>,
    above: Vec<SceneLayer<'a>>,
    camera: Camera,
    caches: &'a Caches,
    background: Option<&'a Background>,
    tool: Tool,
    /// Proportional editing: within this distance (screen px, so it reaches
    /// further zoomed out) of what's moved, vertices go along with it, the
    /// less the further.
    proportional: Option<f32>,
    /// What Ctrl+V pastes.
    clipboard: Option<&'a Piece>,
    /// Which keys do what; none while they're being looked up (or set).
    keys: Option<&'a Keymap>,
    /// Layers to light up, whatever the mode (pointed at in the layers
    /// panel): to see what's on them. Back to front.
    lit: Vec<SceneLayer<'a>>,
    /// When working out what a drag does should give up, so a heavy case
    /// (e.g. a fill through crowded geometry) can't make it crawl.
    /// The grid points snap to, holding Ctrl.
    grid: Grid,
    deadline: Cell<Option<std::time::Instant>>,
}

#[derive(Debug, Default)]
pub struct State {
    interaction: Interaction,
    /// The wheel event doesn't carry these, so keep track of them.
    modifiers: keyboard::Modifiers,
    /// The shape last hovered: it stays highlighted until another shape is
    /// hovered, so it doesn't flicker on and off.
    shape: Option<Highlight>,
    /// The shape highlighted before (if any), fading out since when.
    fading: Option<(Option<Highlight>, Instant)>,
    /// What releasing the current drag would do, worked out when the mouse
    /// last moved: drawn, and applied on release.
    pending: Option<Pending>,
    /// The faces or edges painted so far in this drag (paint mode): drawn,
    /// and painted on release.
    stroke: Vec<TriangleId>,
    edge_stroke: Vec<(VertexId, VertexId)>,
    /// Where a drag was moved to (world) and hasn't been worked out yet:
    /// that's done once per frame, for the latest position, not for every
    /// mouse move (a mouse may report a thousand a second).
    aim: Option<Point>,
    /// Holding a key while painting to adjust the brush by moving the mouse.
    tweaking: Option<Tweak>,
    /// The vertices selected with the lasso (shape mode), to move, rotate or scale
    /// together; as of document `selected_in`.
    selection: Vec<VertexId>,
    selected_in: u64,
    /// The layer the selection is of.
    selected_layer: NodeId,
    /// Dragging a point with Shift: the guides coming up that would do,
    /// nearest first; and all those near, done or not (what it lines up
    /// with is told from where it lands: see [`Editor::lit`]).
    guides: Vec<Guide>,
    near_guides: Vec<Guide>,
    /// The lasso being drawn (world); and whether the next drag draws one
    /// (Q pressed).
    lasso: Vec<Point>,
    lasso_armed: bool,
    /// Whether the lasso (made ready, or being drawn) takes what it catches
    /// out of the selection (Shift+Q), rather than adding it (Q).
    lasso_removes: bool,
    /// Creating a triangle: the guides its start lined up with when it was
    /// pressed (with Shift), those that would do and all those near.
    start_guides: (Vec<Guide>, Vec<Guide>),
    /// Working on the current layer through one of its mirror images (the
    /// cursor went over there), seen through this map: see
    /// [`Editor::flipped`].
    image: Option<Affine>,
    /// Placing a mirror: the vertex it was snapped to run through, if any.
    mirror_through: Option<Point>,
    /// Placing a mirror: whether the line so far would do (the drawing
    /// clear of its mirror image).
    mirror_fits: bool,
    /// The proportional editing the pending move was worked out with, and
    /// the zoom (its reach is on screen).
    proportional: Option<(f32, f32)>,
}

/// Adjusting the brush while painting, by holding a key and moving the
/// mouse across: what, where the face or edge previewed meanwhile is looked
/// for, and where the mouse last was (screen).
#[derive(Debug, Clone, Copy)]
struct Tweak {
    what: Tweaking,
    at: Point,
    last: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tweaking {
    /// The stroke width (W, painting edges).
    Width,
    /// How light the colour is (C).
    Lightness,
    /// How far proportional editing reaches (Shift+O, in the shape mode).
    Reach,
}

/// An in-progress drag. Positions are in world coordinates.
#[derive(Debug, Clone, Copy, Default)]
enum Interaction {
    #[default]
    Idle,
    Panning {
        last: Point,
    },
    /// Creating a triangle started at `from` (where it was pressed, or with
    /// Ctrl or Shift held, lined up from there), dragged to `to`.
    Creating {
        from: Point,
        to: Point,
    },
    MovingVertex {
        id: VertexId,
        to: Point,
    },
    Extending {
        a: VertexId,
        b: VertexId,
        /// Where on the edge the drag started.
        start: Point,
        apex: Point,
    },
    /// Dragging from inside a face: nothing until the cursor crosses an
    /// edge, then extending from the last edge crossed (`edge`: its ends
    /// and where it was crossed). Back in the face, nothing again.
    LeavingFace {
        face: TriangleId,
        /// Where the cursor was at the last move.
        last: Point,
        edge: Option<(VertexId, VertexId, Point)>,
        apex: Point,
    },
    /// Painting faces, the cursor last at `last` (world).
    Painting {
        last: Point,
    },
    /// Dragging the background image (screen position).
    MovingBackground {
        last: Point,
    },
    /// Drawing a lasso (see [`State::lasso`]), to add what's in it to the
    /// selection.
    Lassoing,
    /// Moving the selection by as much as the cursor moved `from` → `to`:
    /// dragging it (`held`), or grabbed with G, until a click.
    MovingSelection {
        from: Point,
        to: Point,
        held: bool,
    },
    /// Extruding the selection (E) by as much as the cursor moved `from` →
    /// `to`, until a click (see [`extrude`]).
    Extruding {
        from: Point,
        to: Point,
    },
    /// Placing a mirror (world): along the line `from` → `to`, as snapped;
    /// started at `start`, on a vertex if `anchored`.
    PlacingMirror {
        start: Point,
        anchored: bool,
        from: Point,
        to: Point,
    },
    /// Pasting (Ctrl+V): the clipboard's piece, moved by `base` and as much
    /// as the cursor moved `from` → `to`, follows the cursor; a click puts
    /// it down, where it doesn't overlap anything.
    Pasting {
        base: Vector,
        from: Point,
        to: Point,
    },
    /// Rotating (R) or scaling (T) the selection around `center` by as much
    /// as the cursor turned around it, or came closer or went further from
    /// it, going `from` → `to`; until a click.
    PivotingSelection {
        pivot: Pivot,
        center: Point,
        from: Point,
        to: Point,
    },
}

/// How the selection changes around its centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pivot {
    Rotate,
    Scale,
}

impl Interaction {
    /// Where the cursor moving the selection (or a vertex) got to, if it's
    /// being moved.
    fn selection_to(self) -> Option<Point> {
        match self {
            Interaction::MovingSelection { to, .. }
            | Interaction::PivotingSelection { to, .. }
            | Interaction::MovingVertex { to, .. } => Some(to),
            _ => None,
        }
    }

    /// Grabbing, rotating or scaling the selection: applied on a click, not
    /// a drag.
    fn is_transforming(self) -> bool {
        matches!(
            self,
            Interaction::MovingSelection { held: false, .. }
                | Interaction::PivotingSelection { .. }
                | Interaction::Extruding { .. }
                | Interaction::Pasting { .. }
        )
    }
}

/// What the cursor is hovering over.
#[derive(Debug, Clone, Copy)]
enum Hover {
    Vertex(VertexId),
    Edge { a: VertexId, b: VertexId, at: Point },
    Face(TriangleId),
}

/// Something a dragged point can snap to.
#[derive(Debug, Clone, Copy)]
enum Target {
    Vertex(VertexId),
    Edge(VertexId, VertexId),
    /// The mirror line: on it, a point joins up with its mirror image.
    Mirror,
}

/// A highlighted shape, worked out once (when it's hovered, and again
/// after an edit) rather than every frame. While dragging it's drawn where
/// the preview puts its vertices.
#[derive(Debug, Clone)]
struct Highlight {
    /// The document revision it was worked out for.
    revision: u64,
    /// The vertex it was found from.
    start: VertexId,
    /// Its triangles, each with sorted corners, sorted.
    triangles: Vec<[VertexId; 3]>,
    outline: Vec<(VertexId, VertexId)>,
}

/// What releasing the current drag would do.
#[derive(Debug)]
struct Pending {
    edits: Vec<Edit>,
    /// The document afterwards, and how normalizing it joined things up.
    result: Document,
    changes: Changes,
    /// Triangles of `result` that are new (not just renumbered by welds).
    added: Vec<[VertexId; 3]>,
    /// Where the point being placed ends up.
    at: Point,
}

impl canvas::Program<Message> for Editor<'_> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        // While dragging we keep tracking the cursor outside the bounds.
        let screen = cursor
            .position()
            .map(|p| p - Vector::new(bounds.x, bounds.y));
        let inside = cursor.position_in(bounds);

        // Over the mirror image, working through it: it's edited as if it
        // were the drawing. Which side is settled when nothing's going on.
        if self.camera.image.is_none() {
            if matches!(state.interaction, Interaction::Idle)
                && let Some(pos) = screen
            {
                state.image = self.image_at(self.camera.to_world(pos));
            }
            if let Some(flipped) = self.flipped(state) {
                return flipped.update(state, event, bounds, cursor);
            }
        }

        self.keep_up(state);

        match event {
            Event::Keyboard(event) => self.on_key(state, event, inside, screen),
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                self.on_press(state, *button, inside?)
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => Some(self.on_move(state, screen?)),
            Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left | mouse::Button::Middle,
            )) => self.on_release(state, screen),
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                Some(self.on_wheel(state, *delta, inside?))
            }
            Event::Mouse(mouse::Event::CursorLeft) => Some(canvas::Action::request_redraw()),
            Event::Window(window::Event::RedrawRequested(now)) => self.on_frame(state, *now),
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        // All on the canvas: no meshes.
        self.draw_parts(state, renderer, bounds, cursor, false)
            .into_iter()
            .filter_map(|part| match part {
                Drawn::Geometry(geometry) => Some(geometry),
                Drawn::Meshes(_) => None,
            })
            .collect()
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match state.interaction {
            Interaction::Panning { .. } | Interaction::MovingBackground { .. } => {
                mouse::Interaction::Grabbing
            }
            Interaction::Idle if !cursor.is_over(bounds) || !self.editable() => {
                mouse::Interaction::default()
            }
            Interaction::Idle if matches!(self.tool, Tool::Background { .. }) => {
                mouse::Interaction::Grab
            }
            // Our own cursor/handles are drawn on the canvas.
            _ => mouse::Interaction::Hidden,
        }
    }
}

impl Editor<'_> {
    /// What to draw, in order: canvas geometry, and (with `meshes`) faces
    /// and painted edges as meshes. Through a mirror image, as that sees it.
    fn draw_parts(
        &self,
        state: &State,
        renderer: &Renderer,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        meshes: bool,
    ) -> Vec<Drawn> {
        match self.flipped(state) {
            Some(flipped) => flipped.draw_scene(state, renderer, bounds, cursor, self, meshes),
            None => self.draw_scene(state, renderer, bounds, cursor, self, meshes),
        }
    }

    /// The shape to frame (world, as seen: with its mirror images): the
    /// selection; else the shape under the cursor (at `inside`); else the
    /// one last highlighted; else all of the layer.
    fn shape_to_frame(&self, state: &State, inside: Option<Point>) -> Vec<Point> {
        let document = self.document;
        let vertices: Vec<VertexId> = if !state.selection.is_empty() {
            state.selection.clone()
        } else if let Some(hover) = inside.and_then(|pos| self.hit_test(pos)) {
            document
                .connected(self.hover_vertex(hover))
                .into_iter()
                .flatten()
                .collect()
        } else if let Some(shape) = state
            .shape
            .as_ref()
            .filter(|shape| shape.revision == document.revision())
        {
            shape.triangles.iter().flatten().copied().collect()
        } else {
            document.unique_vertices()
        };
        let images = doc::images(self.mirrors);
        vertices
            .iter()
            .flat_map(|&v| {
                images
                    .iter()
                    .map(move |image| image.apply(document.vertex(v)))
            })
            .collect()
    }

    /// Keeping up with what changed since the last event: the shapes
    /// highlighted after an edit, what's moved after proportional editing
    /// changed, the selection (only the shape mode's, of vertices still
    /// there).
    fn keep_up(&self, state: &mut State) {
        // After an edit, the highlighted shapes may have grown or shrunk;
        // gone, if what they were found from is (undone, say, or another
        // layer or drawing now).
        let revision = self.document.revision();
        let mut used = None;
        let previous = state.fading.as_mut().map(|(previous, _)| previous);
        for slot in std::iter::once(&mut state.shape).chain(previous) {
            if let Some(shape) = slot.as_ref()
                && shape.revision != revision
            {
                let used = used.get_or_insert_with(|| self.document.unique_vertices());
                *slot = used
                    .binary_search(&shape.start)
                    .is_ok()
                    .then(|| self.highlight(shape.start));
            }
        }
        // Proportional editing changed (reaching further, say, or zoomed):
        // what's being moved is worked out again.
        let proportional = self.proportional.map(|reach| (reach, self.camera.zoom));
        if state.proportional != proportional {
            state.proportional = proportional;
            if let Some(to) = state.interaction.selection_to() {
                state.aim.get_or_insert(to);
            }
        }
        // The selection is only the shape mode's, and only of vertices still
        // there (not welded away, say, or undone).
        if self.tool != Tool::Shape || !self.shown {
            state.selection.clear();
            state.lasso.clear();
            state.lasso_armed = false;
            if matches!(
                state.interaction,
                Interaction::Lassoing
                    | Interaction::MovingSelection { .. }
                    | Interaction::PivotingSelection { .. }
                    | Interaction::Extruding { .. }
            ) {
                state.interaction = Interaction::Idle;
                state.pending = None;
            }
        } else if state.selected_layer != self.current {
            // Another layer: its vertices are others.
            state.selection.clear();
            state.selected_layer = self.current;
        } else if state.selected_in != revision {
            let used = self.document.unique_vertices();
            state.selection.retain(|v| used.binary_search(v).is_ok());
            state.selected_in = revision;
        }
    }

    /// A key pressed or let go, or the modifiers held changing; `inside`
    /// and `screen` are where the cursor is (see `update`).
    fn on_key(
        &self,
        state: &mut State,
        event: &keyboard::Event,
        inside: Option<Point>,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        // What a key pressed does here, as the user has it.
        let pressed = match event {
            keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            } => self.pressed(key, *physical_key, *modifiers),
            _ => None,
        };

        match event {
            keyboard::Event::ModifiersChanged(modifiers) => {
                state.modifiers = *modifiers;
                if self.tool == Tool::Shape {
                    // Shift or Ctrl down or up mid-drag: guides or the grid on
                    // or off.
                    if matches!(
                        state.interaction,
                        Interaction::Creating { .. }
                            | Interaction::MovingVertex { .. }
                            | Interaction::Extending { .. }
                            | Interaction::LeavingFace { .. }
                            | Interaction::Extruding { .. }
                            | Interaction::MovingSelection { .. }
                            | Interaction::Pasting { .. }
                    ) && state.tweaking.is_none()
                        && let Some(pos) = screen
                    {
                        state.aim = Some(self.camera.to_world(pos));
                    }
                    return Some(canvas::Action::request_redraw());
                }
                // Holding Ctrl turns the brush into the pipette.
                matches!(self.tool, Tool::Paint { .. }).then(canvas::Action::request_redraw)
            }
            // Holding the reach key (Shift+O) editing proportionally tweaks
            // how far that reaches; what's being moved stays meanwhile.
            keyboard::Event::KeyPressed { repeat, .. }
                if pressed == Some(Action::Reach)
                    && self.tool == Tool::Shape
                    && self.proportional.is_some() =>
            {
                if !repeat && state.tweaking.is_none() {
                    let at = screen?;
                    state.tweaking = Some(Tweak {
                        what: Tweaking::Reach,
                        at,
                        last: at,
                    });
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Holding the lightness key (C) while painting (with a colour)
            // tweaks how light it is; holding the width key (W) while
            // painting edges, how wide. The face or edge previewed stays
            // meanwhile.
            keyboard::Event::KeyPressed { repeat, .. }
                if (pressed == Some(Action::Lightness)
                    && matches!(
                        self.tool,
                        Tool::Paint {
                            brush: Brush::Color(_),
                            ..
                        }
                    ))
                    || (pressed == Some(Action::Width) && self.paints_edges()) =>
            {
                if !repeat && state.tweaking.is_none() {
                    let what = if pressed == Some(Action::Width) {
                        Tweaking::Width
                    } else {
                        Tweaking::Lightness
                    };
                    state.tweaking = Some(Tweak {
                        what,
                        at: inside?,
                        last: inside?,
                    });
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            keyboard::Event::KeyReleased {
                key, physical_key, ..
            } if state.tweaking.is_some_and(|tweak| {
                let action = match tweak.what {
                    Tweaking::Lightness => Action::Lightness,
                    Tweaking::Width => Action::Width,
                    Tweaking::Reach => Action::Reach,
                };
                self.keys.is_some_and(|keys| {
                    Chord::pressed(key, *physical_key, keyboard::Modifiers::empty())
                        .is_some_and(|chord| keys.releases(&chord, action))
                })
            }) =>
            {
                // Done reaching: what's being moved goes on from where the
                // cursor is.
                if state
                    .tweaking
                    .take()
                    .is_some_and(|tweak| tweak.what == Tweaking::Reach)
                    && let Some(pos) = screen
                {
                    rebase(&mut state.interaction, self.camera.to_world(pos));
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Copy (Ctrl+C) copies the selected faces, cut (Ctrl+X) cuts
            // them; paste (Ctrl+V) picks up a copy of what was copied, to put
            // down with a click.
            keyboard::Event::KeyPressed { .. }
                if self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle)
                    && matches!(
                        pressed,
                        Some(Action::Copy | Action::CutFaces | Action::Paste)
                    ) =>
            {
                if pressed == Some(Action::Copy) {
                    let piece = self.document.piece(&state.selection);
                    return (!piece.is_empty())
                        .then(|| canvas::Action::publish(Message::Copy(piece)).and_capture());
                }
                if pressed == Some(Action::CutFaces) {
                    let piece = self.document.piece(&state.selection);
                    if piece.is_empty() {
                        return None;
                    }
                    // The faces copied: those with all corners selected
                    // (last first, so the others keep their places).
                    let edits = self
                        .document
                        .triangle_ids()
                        .iter()
                        .enumerate()
                        .rev()
                        .filter(|(_, t)| t.iter().all(|v| state.selection.contains(v)))
                        .map(|(triangle, _)| Edit::RemoveTriangle { triangle })
                        .collect();
                    let revision = self.document.revision();
                    return Some(
                        canvas::Action::publish(Message::Cut {
                            piece,
                            edits,
                            revision,
                        })
                        .and_capture(),
                    );
                }
                let piece = self.clipboard.filter(|piece| !piece.is_empty())?;
                let at = self.camera.to_world(inside?);
                // Right where it was copied from, if it fits there (on
                // another layer, say); else in the middle of the cursor.
                let paste = |by: Vector| {
                    self.check(
                        vec![Edit::Paste {
                            piece: piece.moved(by),
                        }],
                        at,
                    )
                };
                let (base, pending) = match paste(Vector::ZERO) {
                    Some(pending) => (Vector::ZERO, Some(pending)),
                    None => {
                        let base = at - piece.centre();
                        (base, paste(base))
                    }
                };
                state.selection.clear();
                state.interaction = Interaction::Pasting {
                    base,
                    from: at,
                    to: at,
                };
                state.pending = pending;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Framing (/): the shape, as seen (mirrored too), in view.
            keyboard::Event::KeyPressed { .. }
                if pressed == Some(Action::FrameShape)
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                let points = self.shape_to_frame(state, inside);
                (!points.is_empty())
                    .then(|| canvas::Action::publish(Message::Frame(points)).and_capture())
            }
            // Lasso (Q, or Shift+Q to take out of the selection): the next
            // drag draws one (the same again: not).
            keyboard::Event::KeyPressed { repeat: false, .. }
                if matches!(pressed, Some(Action::Lasso | Action::LassoRemove))
                    && self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                let removes = pressed == Some(Action::LassoRemove);
                state.lasso_armed = !(state.lasso_armed && state.lasso_removes == removes);
                state.lasso_removes = removes;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Select all (Ctrl+A): every vertex of the layer.
            keyboard::Event::KeyPressed { .. }
                if pressed == Some(Action::SelectAll)
                    && self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                state.selection = self.document.unique_vertices();
                state.selected_in = self.document.revision();
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Select shape (Ctrl+L) selects the whole shape under the
            // cursor; off the drawing, the whole shapes the selection is in.
            keyboard::Event::KeyPressed { .. }
                if pressed == Some(Action::SelectShape)
                    && self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                let hovered = inside
                    .and_then(|pos| self.hit_test(pos))
                    .map(|hover| vec![self.hover_vertex(hover)]);
                let from = hovered.unwrap_or_else(|| state.selection.clone());
                let mut linked = Vec::new();
                for v in from {
                    if !linked.contains(&v) {
                        linked.extend(self.document.connected(v).into_iter().flatten());
                        linked.sort_unstable();
                        linked.dedup();
                    }
                }
                for v in linked {
                    if !state.selection.contains(&v) {
                        state.selection.push(v);
                    }
                }
                state.selected_in = self.document.revision();
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Without a selection, extrude (E) over an outer edge extrudes
            // it, as if its ends were selected.
            keyboard::Event::KeyPressed { .. }
                if pressed == Some(Action::Extrude)
                    && state.selection.is_empty()
                    && self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                let pos = inside?;
                let Hover::Edge { a, b, .. } = self.hit_test(pos)? else {
                    return None;
                };
                if self.extruded_ends(&[a, b]).is_empty() {
                    return None;
                }
                let at = self.camera.to_world(pos);
                state.selection = vec![a, b];
                state.selected_in = self.document.revision();
                state.interaction = Interaction::Extruding { from: at, to: at };
                state.pending = None;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // With a selection: grab (G), rotate (R), scale (T), extrude (E);
            // Esc lets go of that, or else of the selection.
            keyboard::Event::KeyPressed { key, .. }
                if (*key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && (!state.selection.is_empty()
                        || state.lasso_armed
                        || matches!(state.interaction, Interaction::Pasting { .. })))
                    || (!state.selection.is_empty()
                        && self.tool == Tool::Shape
                        && matches!(
                            pressed,
                            Some(Action::Grab | Action::Rotate | Action::Scale | Action::Extrude)
                        )) =>
            {
                let escape = *key == keyboard::Key::Named(keyboard::key::Named::Escape);
                match (escape, state.interaction) {
                    (false, Interaction::Idle) => {
                        let at = self.camera.to_world(inside?);
                        let center = self.centre(&state.selection);
                        let pivot = |pivot| Interaction::PivotingSelection {
                            pivot,
                            center,
                            from: at,
                            to: at,
                        };
                        state.interaction = match pressed {
                            Some(Action::Grab) => Interaction::MovingSelection {
                                from: at,
                                to: at,
                                held: false,
                            },
                            Some(Action::Rotate) => pivot(Pivot::Rotate),
                            Some(Action::Extrude) => Interaction::Extruding { from: at, to: at },
                            _ => pivot(Pivot::Scale),
                        };
                        state.pending = None;
                    }
                    (false, _) => return None,
                    // The lasso made ready first, then the selection.
                    (true, Interaction::Idle) if state.lasso_armed => state.lasso_armed = false,
                    (true, Interaction::Idle) => state.selection.clear(),
                    (true, _) => {
                        state.interaction = Interaction::Idle;
                        state.pending = None;
                        state.aim = None;
                    }
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            keyboard::Event::KeyPressed { .. }
                if matches!(pressed, Some(Action::CutEdge | Action::Delete))
                    && self.tool == Tool::Shape
                    && self.shown
                    && matches!(state.interaction, Interaction::Idle) =>
            {
                let pos = inside?;

                let edits = match pressed? {
                    // Cutting (C) cuts the hovered edge where the cursor is.
                    Action::CutEdge => match self.hit_test(pos)? {
                        Hover::Edge { at, .. } => vec![Edit::InsertVertex { at }],
                        Hover::Vertex(_) | Hover::Face(_) => return None,
                    },
                    // Deleting (D) removes the selected vertices: every
                    // triangle using one (last first, so the others keep
                    // their places).
                    _ if !state.selection.is_empty() => {
                        let edits: Vec<_> = self
                            .document
                            .triangle_ids()
                            .iter()
                            .enumerate()
                            .rev()
                            .filter(|(_, t)| t.iter().any(|v| state.selection.contains(v)))
                            .map(|(triangle, _)| Edit::RemoveTriangle { triangle })
                            .collect();
                        if edits.is_empty() {
                            return None;
                        }
                        edits
                    }
                    // Else the triangle under the cursor.
                    _ => vec![Edit::RemoveTriangle {
                        triangle: self.document.triangle_at(self.camera.to_world(pos))?,
                    }],
                };
                let edits = self.check(edits, self.camera.to_world(pos))?.edits;

                let revision = self.document.revision();
                Some(canvas::Action::publish(Message::Edit { edits, revision }).and_capture())
            }
            _ => None,
        }
    }

    /// A mouse button pressed, the cursor at `pos` (screen).
    fn on_press(
        &self,
        state: &mut State,
        button: mouse::Button,
        pos: Point,
    ) -> Option<canvas::Action<Message>> {
        // Guides are a drag's own: none from the one before.
        state.guides.clear();
        state.near_guides.clear();
        let world = self.camera.to_world(pos);
        if button == mouse::Button::Left && !self.editable() {
            return None;
        }
        // Mid-drag, a right click cancels it: all is as it was, and letting
        // go of the left button then does nothing.
        if button == mouse::Button::Right
            && matches!(
                state.interaction,
                Interaction::Creating { .. }
                    | Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
                    | Interaction::MovingSelection { held: true, .. }
                    | Interaction::Lassoing
                    | Interaction::PlacingMirror { .. }
                    | Interaction::Painting { .. }
            )
        {
            state.interaction = Interaction::Idle;
            state.pending = None;
            state.aim = None;
            state.lasso.clear();
            state.stroke.clear();
            state.edge_stroke.clear();
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // Grabbed, rotating or scaling, the selection follows the cursor: a
        // click applies that; a right click cancels it.
        if state.interaction.is_transforming() {
            if let Some(world) = state.aim.take() {
                self.follow(state, world);
            }
            let pasting = matches!(state.interaction, Interaction::Pasting { .. });
            let extruding = matches!(state.interaction, Interaction::Extruding { .. });
            // Pasting, a click where it doesn't fit does nothing.
            if pasting && button == mouse::Button::Left && state.pending.is_none() {
                return Some(canvas::Action::request_redraw().and_capture());
            }
            let pending = state.pending.take();
            state.interaction = Interaction::Idle;
            let action = match (button, pending) {
                (mouse::Button::Left, Some(pending)) => {
                    // What was pasted is selected, to go on with.
                    if pasting {
                        let n = self.document.vertex_slots();
                        let mut pasted: Vec<VertexId> = pending
                            .result
                            .unique_vertices()
                            .into_iter()
                            .filter(|&v| v >= n)
                            .collect();
                        pasted.extend(pending.changes.welded.iter().map(|&(_, kept)| kept));
                        pasted.sort_unstable();
                        pasted.dedup();
                        state.selection = pasted;
                        state.selected_in = self.document.revision();
                    }
                    // What was extruded: its far edges, to go on from.
                    if extruding {
                        state.selection = self.extruded(&state.selection, &pending);
                        state.selected_in = self.document.revision();
                    }
                    canvas::Action::publish(Message::Edit {
                        edits: pending.edits,
                        revision: self.document.revision(),
                    })
                }
                _ => canvas::Action::request_redraw(),
            };
            return Some(action.and_capture());
        }
        // The lasso Q made ready: a right click lets go of it.
        if button == mouse::Button::Right && state.lasso_armed {
            state.lasso_armed = false;
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // Shift+click adds a vertex to the selection, or removes it; with a
        // selection, it does nothing else. (Elsewhere, Shift+drag creates a
        // triangle that starts lined up.)
        if button == mouse::Button::Left
            && self.tool == Tool::Shape
            && state.modifiers.shift()
            && !state.modifiers.command()
            && !state.lasso_armed
            && matches!(state.interaction, Interaction::Idle)
        {
            let hovered = self.hit_test(pos);
            if let Some(Hover::Vertex(v)) = hovered {
                match state.selection.iter().position(|&s| s == v) {
                    Some(i) => {
                        state.selection.remove(i);
                    }
                    None => state.selection.push(v),
                }
                state.selected_in = self.document.revision();
            }
            if matches!(hovered, Some(Hover::Vertex(_))) || !state.selection.is_empty() {
                return Some(canvas::Action::request_redraw().and_capture());
            }
        }
        if button == mouse::Button::Left && self.picking(state) {
            return Some(
                match self.pick(pos) {
                    Some(picked) => canvas::Action::publish(Message::Pick(picked)),
                    None => canvas::Action::request_redraw(),
                }
                .and_capture(),
            );
        }

        state.interaction = match button {
            mouse::Button::Middle => Interaction::Panning { last: pos },
            mouse::Button::Left if matches!(self.tool, Tool::Background { .. }) => {
                Interaction::MovingBackground { last: pos }
            }
            mouse::Button::Left if self.tool == Tool::Mirror => {
                let snapped = self.mirror_end(pos);
                let start = snapped.unwrap_or(world);
                state.mirror_fits = false;
                state.mirror_through = None;
                Interaction::PlacingMirror {
                    start,
                    anchored: snapped.is_some(),
                    from: start,
                    to: start,
                }
            }
            // Adjusting the brush (holding W or C): a click applies it
            // to what's previewed, not what's under the mouse.
            mouse::Button::Left
                if state.tweaking.is_some() && matches!(self.tool, Tool::Paint { .. }) =>
            {
                let at = state.tweaking.map_or(pos, |tweak| tweak.at);
                state.stroke.clear();
                state.edge_stroke.clear();
                self.paint_at(state, at);
                let faces = std::mem::take(&mut state.stroke);
                let edges = std::mem::take(&mut state.edge_stroke);
                return Some(self.paint(faces, edges).and_capture());
            }
            mouse::Button::Left if matches!(self.tool, Tool::Paint { .. }) => {
                state.stroke.clear();
                state.edge_stroke.clear();
                self.paint_at(state, pos);
                Interaction::Painting { last: world }
            }
            // After Q: a lasso.
            mouse::Button::Left if state.lasso_armed => {
                state.lasso_armed = false;
                state.lasso = vec![world];
                Interaction::Lassoing
            }
            // With a selection, dragging moves it (and a click lets go
            // of it).
            mouse::Button::Left if !state.selection.is_empty() => Interaction::MovingSelection {
                from: world,
                to: world,
                held: true,
            },
            mouse::Button::Left => match self.hit_test(pos) {
                Some(Hover::Vertex(id)) => Interaction::MovingVertex {
                    id,
                    to: self.document.vertex(id),
                },
                Some(Hover::Edge { a, b, at }) => Interaction::Extending {
                    a,
                    b,
                    start: at,
                    apex: at,
                },
                Some(Hover::Face(face)) => Interaction::LeavingFace {
                    face,
                    last: world,
                    edge: None,
                    apex: world,
                },
                // With Ctrl, it starts on the grid; with Shift, lined up.
                None => {
                    let (from, fine, near) = self.creation_start(
                        world,
                        state.modifiers.command(),
                        state.modifiers.shift(),
                    );
                    state.start_guides = (fine, near);
                    Interaction::Creating { from, to: from }
                }
            },
            _ => return None,
        };
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        state.pending = self.pending(state.interaction);
        self.deadline.set(None);

        Some(canvas::Action::request_redraw().and_capture())
    }

    /// The cursor moved to `pos` (screen; outside too, while dragging).
    fn on_move(&self, state: &mut State, pos: Point) -> canvas::Action<Message> {
        let world = self.camera.to_world(pos);

        if let Some(tweak) = &mut state.tweaking
            && (tweak.what == Tweaking::Reach || matches!(state.interaction, Interaction::Idle))
        {
            let across = pos.x - tweak.last.x;
            tweak.last = pos;
            let message = match tweak.what {
                Tweaking::Width => Message::TweakWidth(across),
                Tweaking::Lightness => Message::TweakLightness(across),
                Tweaking::Reach => Message::TweakReach(across),
            };
            return canvas::Action::publish(message).and_capture();
        }

        match &mut state.interaction {
            Interaction::Idle if self.tool != Tool::Shape || !self.shown => {
                return canvas::Action::request_redraw();
            }
            Interaction::Painting { last } => {
                // Everything along the way, however fast the move.
                let from = self.camera.to_screen(*last);
                *last = world;
                let steps = (from.distance(pos) / 3.0).ceil().max(1.0) as usize;
                for i in 1..=steps {
                    self.paint_at(state, from + (pos - from) * (i as f32 / steps as f32));
                }
                return canvas::Action::request_redraw().and_capture();
            }
            Interaction::MovingBackground { last } => {
                let delta = self.camera.to_world(pos) - self.camera.to_world(*last);
                *last = pos;
                return canvas::Action::publish(Message::MoveBackground(delta)).and_capture();
            }
            Interaction::Idle => {
                if let Some(hover) = self.hit_test(pos) {
                    let vertex = self.hover_vertex(hover);
                    let same = state.shape.as_ref().is_some_and(|shape| {
                        shape.triangles.iter().flatten().any(|&v| v == vertex)
                    });
                    if !same {
                        let previous = state.shape.replace(self.highlight(vertex));
                        state.fading = Some((previous, Instant::now()));
                    }
                }
                return canvas::Action::request_redraw();
            }
            Interaction::Panning { last } => {
                let delta = pos - *last;
                *last = pos;
                return canvas::Action::publish(Message::Pan(delta)).and_capture();
            }
            Interaction::Lassoing => {
                let far = state
                    .lasso
                    .last()
                    .is_none_or(|&p| self.camera.to_screen(p).distance(pos) >= 3.0);
                if far {
                    state.lasso.push(world);
                }
                return canvas::Action::request_redraw().and_capture();
            }
            _ => {}
        }

        // Worked out when the frame is drawn (see `follow`).
        state.aim = Some(world);
        canvas::Action::request_redraw().and_capture()
    }

    /// The left or middle button let go, the cursor at `screen`.
    fn on_release(
        &self,
        state: &mut State,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        // Grabbed, rotating or scaling: only a click applies that.
        if state.interaction.is_transforming() {
            return None;
        }
        // Where it was let go, if not worked out yet.
        if let Some(world) = state.aim.take() {
            self.follow(state, world);
        }
        let finished = std::mem::take(&mut state.interaction);
        let pending = state.pending.take();
        state.guides.clear();
        state.near_guides.clear();

        // A mirror where it was let go, if it's long enough to tell
        // its direction and clear of the drawing.
        if let Interaction::PlacingMirror { from, to, .. } = finished {
            let long = self
                .camera
                .to_screen(from)
                .distance(self.camera.to_screen(to))
                >= 2.0 * VERTEX_HIT;
            let mirror = Mirror { a: from, b: to };
            return Some(
                if long && self.fits(mirror) {
                    canvas::Action::publish(Message::SetMirror(mirror))
                } else {
                    canvas::Action::request_redraw()
                }
                .and_capture(),
            );
        }
        if let Interaction::Lassoing = finished {
            let lasso = std::mem::take(&mut state.lasso);
            if lasso.len() >= 3 {
                let caught = self
                    .document
                    .unique_vertices()
                    .into_iter()
                    .filter(|&v| inside_polygon(self.document.vertex(v), &lasso));
                if state.lasso_removes {
                    let caught: Vec<VertexId> = caught.collect();
                    state.selection.retain(|v| !caught.contains(v));
                } else {
                    for v in caught {
                        if !state.selection.contains(&v) {
                            state.selection.push(v);
                        }
                    }
                }
            }
            state.selected_in = self.document.revision();
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // A click (not a drag) on the selection lets go of it.
        if let (Interaction::MovingSelection { from, .. }, None) = (finished, &pending) {
            let still = screen
                .is_some_and(|pos| self.camera.to_screen(from).distance(pos) < VERTEX_HIT / 2.0);
            if still {
                state.selection.clear();
            }
        }

        if let Interaction::Idle = finished {
            return None;
        }
        if let Interaction::Painting { .. } = finished {
            let faces = std::mem::take(&mut state.stroke);
            let edges = std::mem::take(&mut state.edge_stroke);
            return Some(self.paint(faces, edges).and_capture());
        }

        // Exactly what was shown.
        Some(
            match pending {
                Some(pending) => canvas::Action::publish(Message::Edit {
                    edits: pending.edits,
                    revision: self.document.revision(),
                }),
                None => canvas::Action::request_redraw(),
            }
            .and_capture(),
        )
    }

    /// The wheel turned, the cursor at `anchor` (screen).
    fn on_wheel(
        &self,
        state: &mut State,
        delta: mouse::ScrollDelta,
        anchor: Point,
    ) -> canvas::Action<Message> {
        if state.modifiers.shift() {
            // Some platforms turn Shift+wheel into horizontal scrolling.
            let (x, y, per_notch) = match delta {
                mouse::ScrollDelta::Lines { x, y } => (x, y, 1.0),
                mouse::ScrollDelta::Pixels { x, y } => (x, y, 50.0),
            };
            let angle = if y != 0.0 { y } else { x } / per_notch * ROTATE_STEP;
            let message = if matches!(self.tool, Tool::Background { .. }) {
                let anchor = self.camera.to_world(anchor);
                Message::RotateBackground { anchor, angle }
            } else {
                Message::Rotate { anchor, angle }
            };
            return canvas::Action::publish(message).and_capture();
        }

        let factor = match delta {
            mouse::ScrollDelta::Lines { y, .. } => 1.15f32.powf(y),
            mouse::ScrollDelta::Pixels { y, .. } => (y / 200.0).exp(),
        };
        let message = if matches!(self.tool, Tool::Background { .. }) {
            let anchor = self.camera.to_world(anchor);
            Message::ScaleBackground { anchor, factor }
        } else {
            Message::Zoom { anchor, factor }
        };

        canvas::Action::publish(message).and_capture()
    }

    /// A frame: where a drag got to since the last one, and the shape
    /// highlight fading over.
    fn on_frame(&self, state: &mut State, now: Instant) -> Option<canvas::Action<Message>> {
        // A frame: where the drag got to since the last one.
        if let Some(world) = state.aim.take() {
            self.follow(state, world);
        }
        // Keep drawing frames while the shape highlight moves over.
        let (_, since) = state.fading.as_ref()?;
        if now.duration_since(*since) >= SHAPE_FADE {
            state.fading = None;
        }
        Some(canvas::Action::request_redraw())
    }

    /// Where a drag to `world` gets to, and what releasing there does. If
    /// nothing works out (in time), it stays where it was.
    fn follow(&self, state: &mut State, world: Point) {
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        // With Shift, a dragged point lines up with what's around it; with
        // Ctrl, it lands on the grid (that first, with both).
        let shift = state.modifiers.shift();
        let grid = state.modifiers.command();
        let gridded = |state: &mut State, (found, fine, near): grid::Gridded| {
            (state.guides, state.near_guides) = (fine, near);
            found
        };
        state.guides.clear();
        state.near_guides.clear();
        let guided = |dragged: Dragged, guides: &mut Vec<Guide>, near: &mut Vec<Guide>| {
            if !shift {
                return None;
            }
            let found;
            (*guides, *near, found) = self.guided(dragged, world)?;
            found
        };
        match &mut state.interaction {
            Interaction::Creating { from, .. } => {
                let from = *from;
                // The corner dragged: with Ctrl on the grid; with Shift level
                // or upright with where it starts, or with the vertices there
                // are, and so on; else snapped.
                let (found, mut fine, mut near) = if grid {
                    self.grid_create(from, world, shift)
                } else if shift {
                    let (fine, near, found) = self.guided_creation(from, world);
                    (
                        found.or_else(|| self.create_snapped(from, world)),
                        fine,
                        near,
                    )
                } else {
                    (self.create_snapped(from, world), Vec::new(), Vec::new())
                };
                // And those its start lined up with.
                fine.extend(state.start_guides.0.iter().copied());
                near.extend(state.start_guides.1.iter().copied());
                (state.guides, state.near_guides) = (fine, near);
                let to = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
                state.interaction = Interaction::Creating { from, to };
            }
            Interaction::MovingVertex { id, .. } if grid => {
                let id = *id;
                if let Some((at, pending)) = gridded(state, self.grid_move(id, world, shift)) {
                    state.interaction = Interaction::MovingVertex { id, to: at };
                    state.pending = Some(pending);
                }
            }
            Interaction::MovingVertex { id, to } => {
                if let Some((at, pending)) = guided(
                    Dragged::Vertex(*id),
                    &mut state.guides,
                    &mut state.near_guides,
                ) {
                    *to = at;
                    state.pending = Some(pending);
                    self.deadline.set(None);
                    return;
                }
                let moved = match self.proportional {
                    Some(_) => self.move_proportionally(*id, world),
                    None => self.limit_move(*id, world),
                };
                if let Some((p, pending)) = moved {
                    *to = p;
                    state.pending = Some(pending);
                }
            }
            Interaction::Extending { a, b, start, .. } if grid => {
                let (a, b, start) = (*a, *b, *start);
                let found = gridded(state, self.grid_extend(a, b, start, world, shift));
                let apex = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
                state.interaction = Interaction::Extending { a, b, start, apex };
            }
            Interaction::Extending { a, b, start, apex } => {
                let dragged = Dragged::Apex {
                    a: *a,
                    b: *b,
                    start: *start,
                };
                if let Some((at, pending)) =
                    guided(dragged, &mut state.guides, &mut state.near_guides)
                {
                    *apex = at;
                    state.pending = Some(pending);
                    self.deadline.set(None);
                    return;
                }
                if let Some((p, pending)) = self.limit_extend(*a, *b, *start, world) {
                    *apex = p;
                    state.pending = pending;
                }
            }
            Interaction::LeavingFace {
                face,
                last,
                edge,
                apex,
            } => {
                let crossed = self.last_crossed(*last, world);
                *last = world;
                if crossed.is_some() {
                    *edge = crossed;
                    // What the previous edge would do is moot.
                    state.pending = None;
                }
                if self.document.triangle_at(world) == Some(*face) {
                    *edge = None;
                }
                match *edge {
                    Some((a, b, start)) if grid => {
                        let (found, fine, near) = self.grid_extend(a, b, start, world, shift);
                        (state.guides, state.near_guides) = (fine, near);
                        *apex = found.as_ref().map_or(world, |(at, _)| *at);
                        state.pending = found.map(|(_, pending)| pending);
                    }
                    Some((a, b, start)) => {
                        let dragged = Dragged::Apex { a, b, start };
                        if let Some((at, pending)) =
                            guided(dragged, &mut state.guides, &mut state.near_guides)
                        {
                            *apex = at;
                            state.pending = Some(pending);
                        } else if let Some((p, pending)) = self.limit_extend(a, b, start, world) {
                            *apex = p;
                            state.pending = pending;
                        }
                    }
                    None => {
                        *apex = world;
                        state.pending = None;
                    }
                }
            }
            Interaction::Pasting { base, from, to } if grid => {
                *to = world;
                let by = *base + (world - *from);
                state.pending = self
                    .clipboard
                    .and_then(|piece| self.grid_paste(piece, by, world));
            }
            Interaction::Pasting { base, from, to } => {
                *to = world;
                state.pending = self.clipboard.and_then(|piece| {
                    let by = *base + (world - *from);
                    // Snapped, so it joins up where it lands.
                    let moved = piece.moved(by);
                    self.snap_offsets(&moved.vertices, &|_| false)
                        .into_iter()
                        .chain([Vector::ZERO])
                        .find_map(|snap| {
                            let piece = piece.moved(by + snap);
                            self.check(vec![Edit::Paste { piece }], world)
                        })
                });
            }
            Interaction::MovingSelection { from, to, .. } if grid => {
                let by = world - *from;
                if let Some(pending) = self.grid_move_selection(&state.selection, by, world) {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::MovingSelection { from, to, .. } => {
                let by = world - *from;
                // Snapped, so it joins up where it lands; not onto what
                // moves along.
                let moving: std::collections::HashSet<VertexId> = self
                    .weights(&state.selection)
                    .into_iter()
                    .map(|(v, _)| v)
                    .collect();
                let moved: Vec<Point> = state
                    .selection
                    .iter()
                    .map(|&v| self.document.vertex(v) + by)
                    .collect();
                let found = self
                    .snap_offsets(&moved, &|v| moving.contains(&v))
                    .into_iter()
                    .chain([Vector::ZERO])
                    .find_map(|snap| {
                        let by = by + snap;
                        self.transform(&state.selection, world, |p, w| p + by * w)
                    });
                if let Some(pending) = found {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::Extruding { from, to } if grid => {
                let (found, fine, near) = self.grid_extrude(&state.selection, *from, world, shift);
                (state.guides, state.near_guides) = (fine, near);
                *to = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
            }
            Interaction::Extruding { from, to } => {
                // With Shift, it lines up with what's around it.
                if shift {
                    let (fine, near, found) = self.guided_extrusion(&state.selection, *from, world);
                    (state.guides, state.near_guides) = (fine, near);
                    if let Some((at, pending)) = found {
                        *to = at;
                        state.pending = Some(pending);
                        self.deadline.set(None);
                        return;
                    }
                }
                let by = world - *from;
                // Snapped, so its far edges join up where they land; not
                // onto what it's extruded from.
                let selected: std::collections::HashSet<VertexId> =
                    state.selection.iter().copied().collect();
                let ends: Vec<Point> = self
                    .extruded_ends(&state.selection)
                    .into_iter()
                    .map(|v| self.document.vertex(v) + by)
                    .collect();
                *to = world;
                state.pending = self
                    .snap_offsets(&ends, &|v| selected.contains(&v))
                    .into_iter()
                    .chain([Vector::ZERO])
                    .find_map(|snap| self.extrude(&state.selection, by + snap, world));
            }
            Interaction::PivotingSelection {
                pivot,
                center,
                from,
                to,
            } => {
                let center = *center;
                // Turned by `turn`, scaled by `scale`, around the centre.
                let (turn, scale) = match pivot {
                    Pivot::Rotate => {
                        let angle = |p: Point| (p.y - center.y).atan2(p.x - center.x);
                        (angle(world) - angle(*from), 1.0)
                    }
                    Pivot::Scale => {
                        // Started right on the centre: nothing to scale by.
                        let start = from.distance(center);
                        if start < 1e-3 {
                            self.deadline.set(None);
                            return;
                        }
                        (0.0, world.distance(center) / start)
                    }
                };
                // Those proportionally edited, partly.
                let transform = |p: Point, w: f32| {
                    let (sin, cos) = (turn * w).sin_cos();
                    let d = (p - center) * (1.0 + (scale - 1.0) * w);
                    center + Vector::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
                };
                if let Some(pending) = self.transform(&state.selection, world, transform) {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. }
            | Interaction::Lassoing => {}
            Interaction::PlacingMirror {
                start,
                anchored,
                from,
                to,
            } => {
                let shift = state.modifiers.shift();
                // Shift: at a multiple of 15°; else onto a vertex in reach.
                let snapped = if shift {
                    None
                } else {
                    self.mirror_end(self.camera.to_screen(world))
                };
                let mut end = snapped.unwrap_or(world);
                if shift {
                    let d = world - *start;
                    let step = std::f32::consts::PI / 12.0;
                    let angle = (d.y.atan2(d.x) / step).round() * step;
                    let length = d.x.hypot(d.y);
                    end = *start + Vector::new(angle.cos(), angle.sin()) * length;
                }
                (*from, *to) = (*start, end);
                state.mirror_through = snapped;
                // Not on a vertex at its end: running right through one it
                // passes close by, so its image joins up there.
                if snapped.is_none()
                    && let Some((a, b, through)) =
                        self.snap_mirror(*start, end, *anchored && !shift)
                {
                    (*from, *to) = (a, b);
                    state.mirror_through = Some(through);
                }
                state.mirror_fits = self.fits(Mirror { a: *from, b: *to });
            }
        }
        self.deadline.set(None);
    }

    /// Finds what is under a screen position: a vertex, else an edge, else
    /// a face.
    fn hit_test(&self, screen: Point) -> Option<Hover> {
        let snaps = self.snaps(screen, |_| false, |_, _| false, &[]);
        let drawn = snaps
            .iter()
            .find(|(target, _)| !matches!(target, Target::Mirror));
        match drawn {
            Some(&(Target::Vertex(id), _)) => Some(Hover::Vertex(id)),
            Some(&(Target::Edge(a, b), at)) => Some(Hover::Edge { a, b, at }),
            Some((Target::Mirror, _)) | None => self
                .document
                .triangle_at(self.camera.to_world(screen))
                .map(Hover::Face),
        }
    }

    /// A vertex of what `hover` is over, to find its shape by.
    fn hover_vertex(&self, hover: Hover) -> VertexId {
        match hover {
            Hover::Vertex(id) | Hover::Edge { a: id, .. } => id,
            Hover::Face(t) => self.document.triangle_ids()[t][0],
        }
    }

    /// Where a point dragged to `screen` may snap, best first: vertices in
    /// grab range, then where edges in range cross the mirror line (if
    /// any), the closest points on edges in range, and on the mirror line. `skip_vertex`
    /// and `skip_edge` leave some out; the edges in `lines` count as the
    /// whole line through them.
    fn snaps(
        &self,
        screen: Point,
        skip_vertex: impl Fn(VertexId) -> bool,
        skip_edge: impl Fn(VertexId, VertexId) -> bool,
        lines: &[(VertexId, VertexId)],
    ) -> Vec<(Target, Point)> {
        let camera = self.camera;
        let at = |v| camera.to_screen(self.document.vertex(v));

        let mut vertices: Vec<_> = self
            .document
            .unique_vertices()
            .into_iter()
            .filter(|&v| !skip_vertex(v))
            .map(|v| {
                (
                    Target::Vertex(v),
                    self.document.vertex(v),
                    at(v).distance(screen),
                )
            })
            .filter(|&(_, _, d)| d <= VERTEX_HIT)
            .collect();

        let mut edges: Vec<_> = self
            .document
            .unique_edges()
            .into_iter()
            .filter(|&(u, v)| !skip_edge(u, v))
            .filter_map(|(u, v)| {
                let (closest, d) = if lines.contains(&(u, v)) {
                    closest_on_line(screen, at(u), at(v))
                } else {
                    closest_on_segment(screen, at(u), at(v))
                };
                (d <= EDGE_HIT).then(|| (Target::Edge(u, v), camera.to_world(closest), d))
            })
            .collect();

        // On the mirror line, and where edges cross it: what lands there
        // joins up with its mirror image.
        let mut crossings = Vec::new();
        let mut mirror = Vec::new();
        // The lines are where they are, not seen through a mirror image.
        let plain = self.plain_camera();
        for &line in self.mirrors {
            let (a, b) = (plain.to_screen(line.a), plain.to_screen(line.b));
            let (closest, d) = closest_on_line(screen, a, b);
            if d <= EDGE_HIT {
                mirror.push((Target::Mirror, camera.to_world(closest), d));
            }
            for &(target, _, _) in &edges {
                let Target::Edge(u, v) = target else {
                    continue;
                };
                let (pu, pv) = (at(u), at(v));
                let (su, sv) = (area2(a, b, pu), area2(a, b, pv));
                if su * sv >= 0.0 {
                    continue;
                }
                let crossing = pu + (pv - pu) * (su / (su - sv));
                let d = crossing.distance(screen);
                if d <= VERTEX_HIT {
                    crossings.push((target, camera.to_world(crossing), d));
                }
            }
        }

        vertices.sort_by(|x, y| x.2.total_cmp(&y.2));
        crossings.sort_by(|x, y| x.2.total_cmp(&y.2));
        edges.sort_by(|x, y| x.2.total_cmp(&y.2));
        vertices
            .into_iter()
            .chain(crossings)
            .chain(edges)
            .chain(mirror)
            .map(|(target, point, _)| (target, point))
            .collect()
    }

    /// What releasing `interaction` now would do. `None` if nothing.
    fn pending(&self, interaction: Interaction) -> Option<Pending> {
        match interaction {
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. }
            | Interaction::Lassoing
            | Interaction::PlacingMirror { .. } => None,
            // Worked out as they move (see `follow`); not moved yet, nothing.
            Interaction::MovingSelection { .. }
            | Interaction::PivotingSelection { .. }
            | Interaction::Extruding { .. }
            | Interaction::Pasting { .. } => None,
            Interaction::Creating { from, to, .. } => self.create(from, to, to),
            Interaction::MovingVertex { id, to } => self.move_to(id, to),
            Interaction::LeavingFace { edge, apex, .. } => {
                let (a, b, start) = edge?;
                self.pending(Interaction::Extending { a, b, start, apex })
            }
            Interaction::Extending { a, b, start, apex } => {
                // Back on the source edge, the drag reads as cancelled.
                if self.is_near_edge(apex, a, b) {
                    return None;
                }
                self.extend(a, b, start, apex)
            }
        }
    }

    /// Applies `edits` (placing a point `at`) to a copy of the document, if
    /// the document accepts them and they leave no slivers.
    fn check(&self, edits: Vec<Edit>, at: Point) -> Option<Pending> {
        if self.out_of_time() {
            return None;
        }
        // The shape first: the paint only once it's taken (see below).
        let (result, changes) = self
            .document
            .preview_shape_until(&edits, self.deadline.get())?;

        // Slivers would be invisible and impossible to grab. Triangles that
        // were already that thin (e.g. when zoomed far out) may stay so, as
        // long as they don't get any thinner.
        let height = |document: &Document, t: [VertexId; 3]| {
            min_height(t.map(|v| self.camera.to_screen(document.vertex(v))))
        };
        let has_sliver = result.triangle_ids().iter().any(|&t| {
            let after = height(&result, t);
            after < MIN_THICKNESS
                && match self.document.find_triangle(t) {
                    Some(_) => after < height(self.document, t) - 1e-3,
                    None => true,
                }
        });

        if has_sliver {
            return None;
        }
        // Over a mirror line: it would overlap a mirror image.
        if !self.mirrors.is_empty() {
            let maps = doc::crossings(&doc::images(self.mirrors));
            if result.overlaps_under(Some(self.document), &maps) {
                return None;
            }
        }
        // Taken: now with its paint. One edit's is worked out on its shape;
        // several are made again, each painted after the one before.
        let (result, changes) = match &edits[..] {
            [edit] => {
                let mut result = result;
                self.document.paint_after(edit, &mut result);
                (result, changes)
            }
            _ => self.document.preview_until(&edits, None)?,
        };
        let added = new_triangles(self.document, &result, &|v| changes.kept(v)).collect();
        Some(Pending {
            edits,
            result,
            changes,
            added,
            at,
        })
    }

    /// How close (world units) a new triangle's corners and sides may come
    /// to existing geometry before counting as touching it: what a fill
    /// needs to leave no slivers.
    fn snap_distance(&self) -> f32 {
        MIN_THICKNESS / self.camera.zoom
    }

    fn out_of_time(&self) -> bool {
        self.deadline
            .get()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }

    fn is_near_edge(&self, point: Point, a: VertexId, b: VertexId) -> bool {
        let screen = |v| self.camera.to_screen(self.document.vertex(v));
        closest_on_segment(self.camera.to_screen(point), screen(a), screen(b)).1 <= EDGE_HIT
    }

    /// Dragging vertex `id` to `to`: onto the best snap that works out (a
    /// vertex to weld with, an edge to join, or the line through one of its
    /// opposite edges), else to `to` itself.
    fn move_to(&self, id: VertexId, to: Point) -> Option<Pending> {
        let lines: Vec<_> = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|&t| opposite(t, id))
            .map(|(u, v)| (u.min(v), u.max(v)))
            .collect();
        let snaps = self.snaps(
            self.camera.to_screen(to),
            |v| v == id,
            |u, v| u == id || v == id,
            &lines,
        );

        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain([to])
            .find_map(|p| self.check(vec![Edit::MoveVertex { id, to: p }], p))
    }

    /// Creating a triangle dragged from `from` to `to` (placing a point
    /// `at`): equilateral, its third corner to the left as seen.
    fn create(&self, from: Point, to: Point, at: Point) -> Option<Pending> {
        // Seen mirrored, the other way round, so it's still to the left as
        // seen.
        let flips = self.camera.image.is_some_and(Affine::flips);
        let corners = if flips {
            equilateral(to, from)
        } else {
            equilateral(from, to)
        };
        self.check(
            vec![Edit::AddTriangle {
                corners,
                snap: self.snap_distance(),
            }],
            at,
        )
    }

    /// Creating a triangle dragged from `from` to `to`, its dragged corner
    /// or its third one snapped onto the best snap that works out (a
    /// vertex, an edge), else as it is: where the drag ends up, and what
    /// that adds. `None` if nothing works out.
    fn create_snapped(&self, from: Point, to: Point) -> Option<(Point, Pending)> {
        let third = self.third_corner(from, to);

        // Each snap as (onto a vertex, how far on screen, where the drag
        // ends up, where the point snapped lands).
        let mut found: Vec<(bool, f32, Point, Point)> = Vec::new();
        for (corner, dragged) in [(to, true), (third, false)] {
            let at = self.camera.to_screen(corner);
            for (target, p) in self.snaps(at, |_| false, |_, _| false, &[]) {
                let end = if dragged {
                    p
                } else {
                    self.dragged_corner(from, p)
                };
                let distance = self.camera.to_screen(p).distance(at);
                found.push((matches!(target, Target::Vertex(_)), distance, end, p));
            }
        }
        found.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.total_cmp(&y.1)));

        found
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, _, end, p)| (end, p))
            .chain([(to, to)])
            .find_map(|(end, p)| Some((end, self.create(from, end, p)?)))
    }

    /// A new triangle's sixty degrees, as seen: turning its dragged corner
    /// by minus this around where it started gives its third corner (the
    /// other way round seen mirrored, so it's still to the left as seen).
    fn sixty(&self) -> f32 {
        let flips = self.camera.image.is_some_and(Affine::flips);
        let sign = if flips { -1.0 } else { 1.0 };
        sign * std::f32::consts::FRAC_PI_3
    }

    /// The third corner of a new triangle dragged from `from` to `to`.
    fn third_corner(&self, from: Point, to: Point) -> Point {
        from + turn(to - from, -self.sixty())
    }

    /// Where a new triangle started at `from` is dragged to for its third
    /// corner to be at `third`.
    fn dragged_corner(&self, from: Point, third: Point) -> Point {
        from + turn(third - from, self.sixty())
    }

    /// Dragging from edge `a`–`b` (grabbed at `start`) to `apex`. On a side of
    /// the edge that has a triangle, split that triangle and then move the
    /// new vertex to `apex` like any dragged vertex, so it can go on to
    /// squash a child or rearrange the neighbours. On an empty side, add a
    /// triangle on the edge, its third corner snapped like a dragged vertex.
    fn extend(&self, a: VertexId, b: VertexId, start: Point, apex: Point) -> Option<Pending> {
        if let Some(triangle) = self.triangle_beside(a, b, apex) {
            let Some((insert, document, id)) = self.split_from_edge(a, b, start, triangle) else {
                return self.check(vec![Edit::InsertVertex { at: apex }], apex);
            };
            let moved = self.with_document(&document).move_to(id, apex)?;
            let edits = [insert].into_iter().chain(moved.edits).collect();
            return self.check(edits, moved.at);
        }

        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        let source = (a.min(b), a.max(b));
        let snaps = self.snaps(
            self.camera.to_screen(apex),
            |v| v == a || v == b,
            |u, v| (u, v) == source,
            &[],
        );

        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain([apex])
            .find_map(|p| {
                self.check(
                    vec![Edit::AddTriangle {
                        corners: [pa, pb, p],
                        snap: self.snap_distance(),
                    }],
                    p,
                )
            })
    }

    /// Dragging from edge `a`–`b` (grabbed at `start`) to exactly `apex` (on
    /// the grid, say): as [`Self::extend`], but snapping onto nothing.
    fn extend_exact(&self, a: VertexId, b: VertexId, start: Point, apex: Point) -> Option<Pending> {
        if let Some(triangle) = self.triangle_beside(a, b, apex) {
            let Some((insert, document, id)) = self.split_from_edge(a, b, start, triangle) else {
                return self.check(vec![Edit::InsertVertex { at: apex }], apex);
            };
            let moved = self
                .with_document(&document)
                .check(vec![Edit::MoveVertex { id, to: apex }], apex)?;
            let edits = [insert].into_iter().chain(moved.edits).collect();
            return self.check(edits, apex);
        }
        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        self.check(
            vec![Edit::AddTriangle {
                corners: [pa, pb, apex],
                snap: self.snap_distance(),
            }],
            apex,
        )
    }

    /// Splits `triangle` (on edge `a`–`b`) at a point just inside it from
    /// `start` on the edge: where a drag into it begins. Returns the edit,
    /// the resulting document and the new vertex.
    fn split_from_edge(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        triangle: TriangleId,
    ) -> Option<(Edit, Document, VertexId)> {
        let t = self.document.triangle_ids()[triangle];
        let far = self
            .document
            .vertex(*t.iter().find(|&&v| v != a && v != b)?);

        let inset = (MIN_THICKNESS / self.camera.zoom).min(start.distance(far) / 2.0);
        let at = start + (far - start) * (inset / start.distance(far));
        let insert = Edit::InsertVertex { at };

        let mut document = self.document.clone();
        document.apply(insert.clone()).then(|| {
            let id = document.last_vertex();
            (insert, document, id)
        })
    }

    /// This editor, looking at another document (e.g. a step ahead).
    fn with_document<'b>(&'b self, document: &'b Document) -> Editor<'b> {
        Editor {
            document,
            current: self.current,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches: self.caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            lit: self.lit.clone(),
            grid: self.grid,
            deadline: self.deadline.clone(),
        }
    }

    /// The last edge the cursor crossed going from `from` to `to` (world),
    /// and where: its ends and the crossing point. `None` if it crossed
    /// none.
    fn last_crossed(&self, from: Point, to: Point) -> Option<(VertexId, VertexId, Point)> {
        let d = to - from;
        self.document
            .unique_edges()
            .into_iter()
            .filter_map(|(a, b)| {
                let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
                let e = pb - pa;
                let den = cross(d, e);
                if den.abs() < 1e-9 {
                    return None;
                }
                let t = cross(pa - from, e) / den;
                let u = cross(pa - from, d) / den;
                ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
                    .then(|| (t, (a, b, pa + e * u)))
            })
            .max_by(|(t1, _), (t2, _)| t1.total_cmp(t2))
            .map(|(_, crossed)| crossed)
    }

    /// The triangle on edge `a`–`b` lying on the same side as `point`.
    fn triangle_beside(&self, a: VertexId, b: VertexId, point: Point) -> Option<TriangleId> {
        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        let side = area2(pa, pb, point);

        self.document.triangle_ids().iter().position(|t| {
            let third = t.iter().find(|&&v| v != a && v != b);
            t.contains(&a)
                && t.contains(&b)
                && third.is_some_and(|&v| area2(pa, pb, self.document.vertex(v)) * side > 0.0)
        })
    }

    /// Where vertex `id` should be when the cursor is at `to`: the cursor itself if the move is allowed (the mesh
    /// rearranges itself around the vertex where needed), otherwise the
    /// closest allowed point, sliding along the edges around the vertex.
    /// That's the last resort, e.g. when a triangle would be swept over an
    /// unconnected part of the drawing.
    ///
    /// Returns the position and what releasing there would do; `None` if no
    /// valid position was found (in time).
    fn limit_move(&self, id: VertexId, to: Point) -> Option<(Point, Pending)> {
        let camera = self.camera;
        let origin = camera.to_screen(self.document.vertex(id));

        // The vertex must stay on its own side of the opposite edge of each
        // of its triangles.
        let lines: Vec<Line> = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|t| {
                let i = t.iter().position(|&v| v == id)?;
                let a = camera.to_screen(self.document.vertex(t[(i + 1) % 3]));
                let b = camera.to_screen(self.document.vertex(t[(i + 2) % 3]));
                Some(offset_line(a, b, origin, SLIDE_INSET))
            })
            .collect();

        self.closest_valid(&lines, to, move |p| Interaction::MovingVertex { id, to: p })
    }

    /// Where the new corner of an extension from edge `a`–`b` should be when
    /// the cursor is at `to`. Into a triangle,
    /// the new vertex is limited like any dragged vertex; on an empty side it
    /// slides along the geometry a new triangle would overlap.
    fn limit_extend(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        to: Point,
    ) -> Option<(Point, Option<Pending>)> {
        let camera = self.camera;
        let (pa, pb) = (
            camera.to_screen(self.document.vertex(a)),
            camera.to_screen(self.document.vertex(b)),
        );
        let cursor = camera.to_screen(to);

        // On the source edge the drag reads as cancelled, so don't hold the
        // corner back there.
        if closest_on_segment(cursor, pa, pb).1 <= EDGE_HIT {
            return Some((to, None));
        }

        let lines: Vec<Line> = match self.triangle_beside(a, b, to) {
            // Splitting, then moving the new vertex.
            Some(triangle) => {
                let apex = match self.split_from_edge(a, b, start, triangle) {
                    Some((_, document, id)) => self.with_document(&document).limit_move(id, to)?.0,
                    None => to,
                };
                let pending = self.pending(Interaction::Extending { a, b, start, apex })?;
                return Some((apex, Some(pending)));
            }
            // New triangle: the fill bends around other geometry rather than
            // leaving slivers, so just stay clear of the source edge.
            None => vec![offset_line(pa, pb, cursor, SLIDE_INSET)],
        };

        self.closest_valid(&lines, to, move |p| Interaction::Extending {
            a,
            b,
            start,
            apex: p,
        })
        .map(|(p, pending)| (p, Some(pending)))
    }

    /// The valid position closest to the cursor at `to` (world): the cursor
    /// itself, its projection onto one of `lines` (sliding along a limit) or
    /// where two of them cross (stuck in a corner), with what releasing the
    /// drag (`interaction` there) does. `None` if there's none (in time).
    ///
    /// The other positions are tried on all cores at once (nearest wins, as
    /// if tried in turn).
    fn closest_valid(
        &self,
        lines: &[Line],
        to: Point,
        interaction: impl Fn(Point) -> Interaction + Sync,
    ) -> Option<(Point, Pending)> {
        if let Some(pending) = self.pending(interaction(to)) {
            return Some((to, pending));
        }

        let camera = self.camera;
        let cursor = camera.to_screen(to);

        let projections = lines.iter().map(|&(p, d)| p + d * dot(cursor - p, d));
        let corners = lines.iter().enumerate().flat_map(|(i, &(p1, d1))| {
            lines[i + 1..].iter().filter_map(move |&(p2, d2)| {
                let den = cross(d1, d2);
                (den.abs() > 1e-6).then(|| p1 + d1 * (cross(p2 - p1, d2) / den))
            })
        });

        let mut candidates: Vec<_> = projections
            .chain(corners)
            .map(|p| (camera.to_world(p), p.distance(cursor)))
            .collect();
        candidates.sort_by(|x, y| x.1.total_cmp(&y.1));

        let worker = self.worker();
        candidates.par_iter().find_map_first(|&(p, _)| {
            let caches = Caches::default();
            let editor = worker.editor(&caches);
            if editor.out_of_time() {
                return None;
            }
            editor.pending(interaction(p)).map(|pending| (p, pending))
        })
    }

    /// What working out drags on another thread needs of this editor.
    fn worker(&self) -> Worker<'_> {
        Worker {
            document: self.document,
            current: self.current,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            camera: self.camera,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            grid: self.grid,
            deadline: self.deadline.get(),
        }
    }

    /// Whether clicking and dragging does something: not on a hidden
    /// layer (but the background can be adjusted).
    fn editable(&self) -> bool {
        self.shown || matches!(self.tool, Tool::Background { .. })
    }

    /// How layers look now: the shape mode keeps to the plain look.
    fn look(&self) -> Look {
        match self.tool {
            Tool::Paint { .. } | Tool::Background { painted: true } => Look::Painted,
            Tool::Shape | Tool::Mirror | Tool::Background { painted: false } => Look::Plain,
        }
    }

    /// What a key pressed does on the canvas now (in its mode), if anything.
    fn pressed(
        &self,
        key: &keyboard::Key,
        physical: keyboard::key::Physical,
        modifiers: keyboard::Modifiers,
    ) -> Option<Action> {
        let context = match self.tool {
            Tool::Shape => Context::Shape,
            Tool::Paint { .. } => Context::Paint,
            Tool::Background { .. } | Tool::Mirror => return None,
        };
        let chord = Chord::pressed(key, physical, modifiers)?;
        self.keys?.action(&chord, context)
    }
}

/// A line in screen space: a point on it and its unit direction.
type Line = (Point, Vector);

/// The line through `a` and `b`, shifted `offset` px towards `towards`
/// (away from it if negative).
fn offset_line(a: Point, b: Point, towards: Point, offset: f32) -> Line {
    let dir = (b - a) * (1.0 / a.distance(b));
    let mut normal = Vector::new(-dir.y, dir.x);
    if dot(normal, towards - a) < 0.0 {
        normal *= -1.0;
    }
    (a + normal * offset, dir)
}

fn longest_edge([a, b, c]: [Point; 3]) -> (Point, Point) {
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .max_by(|x, y| x.0.distance(x.1).total_cmp(&y.0.distance(y.1)))
        .unwrap()
}

/// `v` turned by `angle` (radians).
fn turn(v: Vector, angle: f32) -> Vector {
    let (sin, cos) = angle.sin_cos();
    Vector::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

fn dot(a: Vector, b: Vector) -> f32 {
    a.x * b.x + a.y * b.y
}

fn cross(a: Vector, b: Vector) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Triangles in `after` that `before` doesn't have (compared by corner ids,
/// through `same` for vertices that were welded).
fn new_triangles<'a>(
    before: &'a Document,
    after: &'a Document,
    same: &'a impl Fn(VertexId) -> VertexId,
) -> impl Iterator<Item = [VertexId; 3]> + 'a {
    let key = move |t: &[VertexId; 3]| {
        let mut t = t.map(same);
        t.sort_unstable();
        t
    };
    let existing: std::collections::HashSet<_> = before.triangle_ids().iter().map(key).collect();

    after
        .triangle_ids()
        .iter()
        .filter(move |t| !existing.contains(&key(t)))
        .copied()
}

/// Equilateral triangle with base `from`–`to`, apex to the left of the drag.
fn equilateral(from: Point, to: Point) -> [Point; 3] {
    let d = to - from;
    let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let h = 3f32.sqrt() / 2.0;
    [from, to, mid + Vector::new(d.y * h, -d.x * h)]
}
