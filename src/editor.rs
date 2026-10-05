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

use iced::advanced::graphics::cache::{Cached, Group};
use iced::keyboard;
use iced::mouse;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{
    self, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use iced::window;
use iced::{Color, Element, Point, Rectangle, Renderer, Size, Theme, Vector};

use crate::background::Background;
use crate::camera::Camera;
use crate::document::{self as doc, EdgeStyle, Mirror, Piece};
use crate::document::{Changes, Document, Edit, TriangleId, VertexId, opposite};
use crate::geometry::{
    Affine, area2, closest_on_line, closest_on_segment, inside_polygon, min_height,
};
use crate::icons;
use crate::joints;
use crate::keys::{Action, Chord, Context, Keymap};
use crate::layers::NodeId;
use crate::page::Page;
use crate::paint::{self, Brush};

mod drag;
mod draw;
mod extrude;
mod grid;
mod guides;
mod input;
mod meshes;
mod mirror;
mod painter;
mod painting;
mod selection;

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use drag::*;
use draw::*;
pub use grid::Grid;
use grid::MAJOR;
use guides::*;
use meshes::{Drawn, EditorView, LayerMeshes};
use mirror::*;
use painter::*;
use selection::*;

/// How far (screen px) a press on the selection may wander and still be a
/// click, letting go of it: a pen's tap rarely stays put.
const CLICK_SLOP: f32 = 8.0;
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
/// How far (radians) an edge loop may turn at a vertex unless tweaked
/// (30°), and at most (90°).
const LOOP_TURN: f32 = std::f32::consts::FRAC_PI_6;
const MAX_LOOP_TURN: f32 = std::f32::consts::FRAC_PI_2;
/// How far (screen px) to drag with Ctrl and the middle button to zoom by e.
const ZOOM_DRAG: f32 = 200.0;
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
/// The checkerboard behind what's see-through: its two greys, a step
/// either side of the page's.
const CHECKER_DARK: Color = Color::from_rgb8(0x2a, 0x2c, 0x3e);
const CHECKER_LIGHT: Color = Color::from_rgb8(0x3c, 0x3f, 0x56);

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
    /// Holding the opacity key (Shift+C) and moving: the brush's colour
    /// more opaque (right) or less (left), by how far (screen px).
    TweakOpacity(f32),
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
    /// Fit these points (world) in view: the shape to frame (/), its layer
    /// isolated; on another layer (the one pointed at), switching to it.
    /// `pointed`: a shape pointed at (or the selection), rather than the
    /// layer's.
    Frame {
        points: Vec<Point>,
        layer: Option<NodeId>,
        pointed: bool,
    },
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
    /// The backdrop: cleared when it changes (the background image, the
    /// grid, the document size).
    pub backdrop: BackdropCache,
    /// Each layer's drawing.
    layers: RefCell<HashMap<NodeId, LayerCache>>,
    /// What's remembered of drawings to draw them (see `draw::Memos`).
    memos: painter::Memos,
}

/// The backdrop as last drawn: what goes under the checkerboard (behind
/// what's see-through), what over it, and the document's size over all
/// (see `Editor::draw_backdrop`).
#[derive(Default)]
pub struct BackdropCache {
    under: canvas::Cache,
    over: canvas::Cache,
    label: canvas::Cache,
}

impl BackdropCache {
    pub fn clear(&self) {
        self.under.clear();
        self.over.clear();
        self.label.clear();
    }
}

/// A layer as last drawn, and what that depended on.
struct LayerCache {
    key: LayerKey,
    /// Its own, kept from one drawing of it to the next.
    group: Group,
    /// What's drawn on the canvas, kept (on the GPU) while unchanged.
    geometry: <Geometry as Cached>::Cache,
    /// Its faces and painted edges as meshes (under and over what the
    /// canvas draws), if drawn with meshes.
    meshes: Option<(
        iced::advanced::graphics::mesh::Cache,
        iced::advanced::graphics::mesh::Cache,
    )>,
    /// The checkerboard behind what's see-through of it, if anything is: to
    /// draw under all the layers.
    beneath: Option<Beneath>,
}

/// A layer's checkerboard (see `meshes::checker`), kept: as a mesh, or on
/// the canvas without meshes.
enum Beneath {
    Meshes(iced::advanced::graphics::mesh::Cache),
    Geometry(<Geometry as Cached>::Cache),
}

impl Beneath {
    fn drawn(&self) -> Drawn {
        match self {
            Beneath::Meshes(mesh) => Drawn::Meshes(mesh.clone()),
            Beneath::Geometry(geometry) => Drawn::Geometry(Cached::load(geometry)),
        }
    }
}

/// What a layer's drawing depends on.
#[derive(Debug, Clone, PartialEq)]
struct LayerKey {
    revision: u64,
    style: Style,
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
    /// Whether its edges are drawn in the paint mode.
    pub show_edges: bool,
    /// The mirrors it's shown mirrored across, one after the other (a
    /// layer has one at most).
    pub mirrors: &'a [Mirror],
}

impl<'a> SceneLayer<'a> {
    /// A layer as the canvas gets it.
    pub fn of(layer: &'a crate::layers::Layer) -> Self {
        SceneLayer {
            id: layer.id,
            document: &layer.document,
            show_edges: layer.show_edges,
            mirrors: layer.mirror.as_slice(),
        }
    }
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
/// (the document's sheet, the background image and grid).
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
    EditorView::new(Editor {
        document: current.document,
        current: current.id,
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
        page,
        grid,
        deadline: Cell::new(None),
    })
    .into()
}

struct Editor<'a> {
    /// The current layer: the one edited.
    document: &'a Document,
    /// Its id, and whether its edges show in the paint mode.
    current: NodeId,
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
    /// The document size, if one's set: shown as a sheet behind it all.
    page: Option<Page>,
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
    /// out of the selection (Alt+Q), rather than adding it (Q).
    lasso_removes: bool,
    /// The edge the selection's edge loop was found from (Shift+Q), as of
    /// document revision: found again from it as how far it may turn is
    /// tweaked.
    edge_loop: Option<(VertexId, VertexId, u64)>,
    /// How far (radians) an edge loop may turn at a vertex, if tweaked
    /// (else `LOOP_TURN`).
    loop_turn: Option<f32>,
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
    /// How opaque the colour is (Shift+C).
    Opacity,
    /// How far proportional editing reaches (Shift+O, in the shape mode).
    Reach,
    /// How far the edge loop may turn at a vertex (Shift+Q).
    LoopTurn,
}

/// An in-progress drag. Positions are in world coordinates.
#[derive(Debug, Clone, Copy, Default)]
enum Interaction {
    #[default]
    Idle,
    Panning {
        last: Point,
    },
    /// Ctrl + middle drag: zooming about `anchor` (screen) as the cursor
    /// goes up (in) or down (out); for tablets, which have no wheel.
    Zooming {
        anchor: Point,
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
        // All on the canvas: no meshes. (`EditorView` draws the editor:
        // this, without its layers, puts the backdrop's image over all.)
        let Parts { beneath, drawing } = self.draw_parts(state, renderer, bounds, cursor, false);
        let [under, over, label] = self.draw_backdrop(renderer, bounds.size());
        let parts = beneath
            .into_iter()
            .chain(drawing)
            .filter_map(|part| match part {
                Drawn::Geometry(geometry) => Some(geometry),
                Drawn::Meshes(_) => None,
            });
        [under]
            .into_iter()
            .chain(parts)
            .chain([over, label])
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
            Interaction::Zooming { .. } => mouse::Interaction::ZoomIn,
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
    ) -> Parts {
        match self.flipped(state) {
            Some(flipped) => flipped.draw_scene(state, renderer, bounds, cursor, self, meshes),
            None => self.draw_scene(state, renderer, bounds, cursor, self, meshes),
        }
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
}
