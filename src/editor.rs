//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns mouse input into [`Message`]s. It never mutates the document itself.
//!
//! In the shape mode, left-drag on blank canvas creates a triangle; from
//! inside a face, it
//! extends from the last edge the cursor crossed. Only middle-drag pans; the
//! wheel zooms around the cursor, Shift+wheel rotates around it. C cuts the
//! hovered edge at the cursor; D removes the triangle under it (with a
//! selection, the selected vertices: every triangle using one).
//!
//! Ctrl+drag draws a lasso, adding the vertices inside it to the
//! selection; Shift+click adds or removes a vertex; Ctrl+L adds the whole
//! shape under the cursor (or those the selection is in). The selection is then
//! dragged, grabbed with G, or rotated (R) or scaled (T) around its
//! centre (a click applies those); a click without dragging, or Esc,
//! drops it.
//!
//! In the paint mode, clicking a face or dragging over faces paints them
//! with the brush; the shape can't be changed.
//!
//! A layer with a mirror shows its mirror image, which is edited (and
//! painted) like the drawing itself: over there, the editor sees the layer
//! through the mirror (see [`Editor::flipped`]). (What works with a mirror
//! works with several one after the other, too; layers have one at most.)
//!
//! A dragged point snaps onto nearby vertices and edges, and the mirror line (see
//! [`Editor::snaps`]); the document then joins things up by itself, so the
//! editor only decides where points go.

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
use crate::layers::{Crossfade, NodeId};
use crate::paint::{self, Brush};

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
/// World-space spacing of the background dot grid.
const GRID: f32 = 50.0;
/// Rotation per wheel notch with Shift held.
const ROTATE_STEP: f32 = 5.0 * std::f32::consts::PI / 180.0;

pub const BACKGROUND: Color = Color::from_rgb8(0x1a, 0x1b, 0x26);
pub const GRID_DOT: Color = Color::from_rgb8(0x2f, 0x33, 0x4d);
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
/// Guides a point dragged with Shift lines up with.
const GUIDE: Color = Color::from_rgb8(0x73, 0xda, 0xca);
/// Screen-space distance within which a dragged point lines up with a
/// guide; and with a point on guides (where two cross, say).
const GUIDE_HIT: f32 = 8.0;
const GUIDE_POINT_HIT: f32 = 10.0;
/// Screen-space distance within which guides are shown, as coming up; and
/// how many at most.
const GUIDE_NEAR: f32 = 20.0;
const GUIDES_SHOWN: usize = 3;
/// What's selected (shape mode).
const SELECTED: Color = Color::from_rgb8(0xff, 0x9e, 0x3b);
/// A mirror line.
const MIRROR: Color = Color::from_rgb8(0xbb, 0x9a, 0xf7);
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
    /// Holding Alt while painting edges, the mouse moved this far across
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
    /// Holding Alt with a selection (proportional editing on), the mouse
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
}

/// A layer as last drawn, and what that depended on.
struct LayerCache {
    key: Option<LayerKey>,
    cache: canvas::Cache,
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

/// The canvas for editing the current layer of `scene`, over the backdrop
/// (background image and grid).
pub fn view<'a>(
    scene: Scene<'a>,
    camera: Camera,
    caches: &'a Caches,
    background: Option<&'a Background>,
    tool: Tool,
    proportional: Option<f32>,
    clipboard: Option<&'a Piece>,
) -> Element<'a, Message> {
    let Scene {
        current,
        shown,
        below,
        above,
    } = scene;
    let backdrop = Canvas::new(Backdrop {
        cache: &caches.grid,
        background,
        camera,
    });
    let editor = Canvas::new(Editor {
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
        deadline: Cell::new(None),
    });
    // Apart, as images (the background) are drawn after shapes within a
    // canvas: the stack draws the drawing in a pass of its own, over it.
    stack![
        backdrop.width(Fill).height(Fill),
        editor.width(Fill).height(Fill)
    ]
    .into()
}

/// The background colour, the background image and the grid.
struct Backdrop<'a> {
    cache: &'a canvas::Cache,
    background: Option<&'a Background>,
    camera: Camera,
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
            if let Some(background) = self.background {
                background.draw(frame, self.camera);
            }
            draw_grid(frame, self.camera);
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
    /// When working out what a drag does should give up, so a heavy case
    /// (e.g. a fill through crowded geometry) can't make it crawl.
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
    /// Dragging a point with Shift: the guides coming up, and where it
    /// lined up, if it did.
    guides: Vec<Guide>,
    lined: Option<Lined>,
    /// The lasso being drawn (world).
    lasso: Vec<Point>,
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
    /// The stroke width (Alt, painting edges).
    Width,
    /// How light the colour is (C).
    Lightness,
    /// How far proportional editing reaches (Alt, with a selection).
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

/// A line (or circle) a point dragged with Shift can line up with, from
/// the edges around it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Guide {
    /// Straight on from edge `m`–`n`, past `n`: no bend there.
    Straight { m: VertexId, n: VertexId },
    /// From `n`, square to edge `m`–`n`: a right angle there.
    Square { m: VertexId, n: VertexId },
    /// From `n`, at the same angle as edge `u`–`v`.
    Parallel {
        n: VertexId,
        u: VertexId,
        v: VertexId,
    },
    /// Between `a` and `b`: the outline straight there.
    Between { a: VertexId, b: VertexId },
    /// Where edges to `a` and `b` meet at a right angle: the circle over
    /// `a`–`b` (Thales').
    Corner { a: VertexId, b: VertexId },
    /// From `n`, level (or `upright`) as seen in the view.
    Level { n: VertexId, upright: bool },
}

/// Where a guide runs (world).
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// Through a point, along a direction.
    Line(Point, Vector),
    /// Around a centre, this far.
    Circle(Point, f32),
}

impl Guide {
    /// Where it runs, seen through `camera`. Lines go from the end the new
    /// edge starts at, as far as the edge they follow is long.
    fn shape(self, document: &Document, camera: Camera) -> Shape {
        let at = |v| document.vertex(v);
        match self {
            Guide::Level { n, upright } => {
                // Across (or up) the screen, in the world.
                let along = if upright {
                    Vector::new(0.0, 1.0)
                } else {
                    Vector::new(1.0, 0.0)
                };
                let origin = camera.to_screen(at(n));
                let d = camera.to_world(origin + along) - at(n);
                let length = d.x.hypot(d.y).max(1e-9);
                Shape::Line(at(n), d * (1.0 / length))
            }
            Guide::Straight { m, n } => Shape::Line(at(n), at(n) - at(m)),
            Guide::Square { m, n } => {
                let d = at(n) - at(m);
                Shape::Line(at(n), Vector::new(-d.y, d.x))
            }
            Guide::Parallel { n, u, v } => Shape::Line(at(n), at(v) - at(u)),
            Guide::Between { a, b } => Shape::Line(at(a), at(b) - at(a)),
            Guide::Corner { a, b } => {
                let (a, b) = (at(a), at(b));
                Shape::Circle(a + (b - a) * 0.5, a.distance(b) / 2.0)
            }
        }
    }

    /// The edge it follows, if one.
    fn followed(self) -> Option<(VertexId, VertexId)> {
        match self {
            Guide::Straight { m, n } | Guide::Square { m, n } => Some((m, n)),
            Guide::Parallel { u, v, .. } => Some((u, v)),
            Guide::Between { .. } | Guide::Corner { .. } | Guide::Level { .. } => None,
        }
    }

    /// The end the new edge starts at, for those from one.
    fn from(self) -> Option<VertexId> {
        match self {
            Guide::Straight { n, .. }
            | Guide::Square { n, .. }
            | Guide::Parallel { n, .. }
            | Guide::Level { n, .. } => Some(n),
            Guide::Between { .. } | Guide::Corner { .. } => None,
        }
    }
}

/// Where a point dragged with Shift lines up: on one guide, where two
/// cross, or on one from an end as far as its edge is long (`equal`).
#[derive(Debug, Clone, Copy)]
struct Lined {
    at: Point,
    guides: [Option<Guide>; 2],
    equal: bool,
}

/// Where a dragged point lined up, and what releasing there does.
type LinedUp = (Lined, Pending);

/// What's being dragged, to line up with what's around it.
#[derive(Debug, Clone, Copy)]
enum Dragged {
    Vertex(VertexId),
    /// The new corner of a triangle on edge `a`–`b`, grabbed at `start`.
    Apex {
        a: VertexId,
        b: VertexId,
        start: Point,
    },
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

        // After an edit, the highlighted shapes may have grown or shrunk.
        let revision = self.document.revision();
        let previous = state
            .fading
            .as_mut()
            .and_then(|(previous, _)| previous.as_mut());
        for shape in state.shape.iter_mut().chain(previous) {
            if shape.revision != revision {
                *shape = self.highlight(shape.start);
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
            if matches!(
                state.interaction,
                Interaction::Lassoing
                    | Interaction::MovingSelection { .. }
                    | Interaction::PivotingSelection { .. }
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

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                // Holding Alt over the canvas, painting edges, tweaks the
                // stroke width; the edge previewed stays meanwhile. With a
                // selection to edit proportionally, how far that reaches;
                // what's being moved stays meanwhile.
                let alt = if !modifiers.alt() {
                    None
                } else if self.paints_edges() {
                    Some(Tweaking::Width)
                } else if self.tool == Tool::Shape && self.proportional.is_some() {
                    Some(Tweaking::Reach)
                } else {
                    None
                };
                let was = state.tweaking;
                state.tweaking = match (was, alt) {
                    (Some(tweak), _) if tweak.what == Tweaking::Lightness => Some(tweak),
                    (Some(tweak), Some(what)) if tweak.what == what => Some(tweak),
                    (_, Some(what)) => {
                        let at = if what == Tweaking::Reach {
                            screen
                        } else {
                            inside
                        };
                        at.map(|at| Tweak { what, at, last: at })
                    }
                    (_, None) => None,
                };
                // Done: what's being moved goes on from where the cursor is.
                if was.is_some_and(|tweak| tweak.what == Tweaking::Reach)
                    && state.tweaking.is_none()
                    && let Some(pos) = screen
                {
                    rebase(&mut state.interaction, self.camera.to_world(pos));
                }
                if self.tool == Tool::Shape {
                    // Shift down or up mid-drag: guides on or off.
                    if matches!(
                        state.interaction,
                        Interaction::MovingVertex { .. }
                            | Interaction::Extending { .. }
                            | Interaction::LeavingFace { .. }
                    ) && let Some(pos) = screen
                    {
                        state.aim = Some(self.camera.to_world(pos));
                    }
                    return Some(canvas::Action::request_redraw());
                }
                // Holding Ctrl turns the brush into the pipette.
                matches!(self.tool, Tool::Paint { .. }).then(canvas::Action::request_redraw)
            }
            // Holding C while painting (with a colour) tweaks how light it
            // is; the face or edge previewed stays meanwhile.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                repeat,
                ..
            }) if !modifiers.command()
                && matches!(
                    self.tool,
                    Tool::Paint {
                        brush: Brush::Color(_),
                        ..
                    }
                )
                && key.to_latin(*physical_key) == Some('c') =>
            {
                if !repeat && state.tweaking.is_none() {
                    state.tweaking = Some(Tweak {
                        what: Tweaking::Lightness,
                        at: inside?,
                        last: inside?,
                    });
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Keyboard(keyboard::Event::KeyReleased {
                key, physical_key, ..
            }) if key.to_latin(*physical_key) == Some('c')
                && state
                    .tweaking
                    .is_some_and(|tweak| tweak.what == Tweaking::Lightness) =>
            {
                state.tweaking = None;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Ctrl+C copies the selected faces, Ctrl+X cuts them; Ctrl+V
            // picks up a copy of what was copied, to put down with a click.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if modifiers.command()
                && self.tool == Tool::Shape
                && self.shown
                && matches!(state.interaction, Interaction::Idle)
                && matches!(key.to_latin(*physical_key), Some('c' | 'x' | 'v')) =>
            {
                let latin = key.to_latin(*physical_key);
                if latin == Some('c') {
                    let piece = self.document.piece(&state.selection);
                    return (!piece.is_empty())
                        .then(|| canvas::Action::publish(Message::Copy(piece)).and_capture());
                }
                if latin == Some('x') {
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
            // Ctrl+L selects the whole shape under the cursor; off the
            // drawing, the whole shapes the selection is in.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if modifiers.command()
                && self.tool == Tool::Shape
                && self.shown
                && matches!(state.interaction, Interaction::Idle)
                && key.to_latin(*physical_key) == Some('l') =>
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
            // With a selection: G grabs it, R rotates it, T scales it; Esc
            // lets go of that, or else of the selection.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if !modifiers.command()
                && ((*key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && (!state.selection.is_empty()
                        || matches!(state.interaction, Interaction::Pasting { .. })))
                    || (!state.selection.is_empty()
                        && matches!(key.to_latin(*physical_key), Some('g' | 'r' | 't')))) =>
            {
                let latin = key.to_latin(*physical_key);
                match (latin, state.interaction) {
                    (None, Interaction::Idle) => state.selection.clear(),
                    (None, _) => {
                        state.interaction = Interaction::Idle;
                        state.pending = None;
                        state.aim = None;
                    }
                    (Some(key), Interaction::Idle) => {
                        let at = self.camera.to_world(inside?);
                        let center = self.centre(&state.selection);
                        let pivot = |pivot| Interaction::PivotingSelection {
                            pivot,
                            center,
                            from: at,
                            to: at,
                        };
                        state.interaction = match key {
                            'g' => Interaction::MovingSelection {
                                from: at,
                                to: at,
                                held: false,
                            },
                            'r' => pivot(Pivot::Rotate),
                            _ => pivot(Pivot::Scale),
                        };
                        state.pending = None;
                    }
                    _ => return None,
                }
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if !modifiers.command()
                && self.tool == Tool::Shape
                && self.shown
                && matches!(state.interaction, Interaction::Idle) =>
            {
                let pos = inside?;

                let edits = match key.to_latin(*physical_key)? {
                    // C cuts the hovered edge where the cursor is.
                    'c' => match self.hit_test(pos)? {
                        Hover::Edge { at, .. } => vec![Edit::InsertVertex { at }],
                        Hover::Vertex(_) | Hover::Face(_) => return None,
                    },
                    // D removes the selected vertices: every triangle using
                    // one (last first, so the others keep their places).
                    'd' if !state.selection.is_empty() => {
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
                    'd' => vec![Edit::RemoveTriangle {
                        triangle: self.document.triangle_at(self.camera.to_world(pos))?,
                    }],
                    _ => return None,
                };
                let edits = self.check(edits, self.camera.to_world(pos))?.edits;

                let revision = self.document.revision();
                Some(canvas::Action::publish(Message::Edit { edits, revision }).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let pos = inside?;
                let world = self.camera.to_world(pos);
                if *button == mouse::Button::Left && !self.editable() {
                    return None;
                }
                // Grabbed, rotating or scaling, the selection follows the cursor: a
                // click applies that; a right click cancels it.
                if state.interaction.is_transforming() {
                    if let Some(world) = state.aim.take() {
                        self.follow(state, world);
                    }
                    let pasting = matches!(state.interaction, Interaction::Pasting { .. });
                    // Pasting, a click where it doesn't fit does nothing.
                    if pasting && *button == mouse::Button::Left && state.pending.is_none() {
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
                            canvas::Action::publish(Message::Edit {
                                edits: pending.edits,
                                revision: self.document.revision(),
                            })
                        }
                        _ => canvas::Action::request_redraw(),
                    };
                    return Some(action.and_capture());
                }
                // Shift+click adds a vertex to the selection, or removes it.
                if *button == mouse::Button::Left
                    && self.tool == Tool::Shape
                    && state.modifiers.shift()
                    && !state.modifiers.command()
                    && matches!(state.interaction, Interaction::Idle)
                {
                    if let Some(Hover::Vertex(v)) = self.hit_test(pos) {
                        match state.selection.iter().position(|&s| s == v) {
                            Some(i) => {
                                state.selection.remove(i);
                            }
                            None => state.selection.push(v),
                        }
                        state.selected_in = self.document.revision();
                    }
                    return Some(canvas::Action::request_redraw().and_capture());
                }
                if *button == mouse::Button::Left && self.picking(state) {
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
                    // Adjusting the brush (holding Alt or C): a click applies it
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
                    // Ctrl: a lasso.
                    mouse::Button::Left if state.modifiers.command() => {
                        state.lasso = vec![world];
                        Interaction::Lassoing
                    }
                    // With a selection, dragging moves it (and a click lets go
                    // of it).
                    mouse::Button::Left if !state.selection.is_empty() => {
                        Interaction::MovingSelection {
                            from: world,
                            to: world,
                            held: true,
                        }
                    }
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
                        None => Interaction::Creating {
                            from: world,
                            to: world,
                        },
                    },
                    _ => return None,
                };
                self.deadline.set(Some(std::time::Instant::now() + BUDGET));
                state.pending = self.pending(state.interaction);
                self.deadline.set(None);

                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let pos = screen?;
                let world = self.camera.to_world(pos);

                if let Some(tweak) = &mut state.tweaking
                    && (tweak.what == Tweaking::Reach
                        || matches!(state.interaction, Interaction::Idle))
                {
                    let across = pos.x - tweak.last.x;
                    tweak.last = pos;
                    let message = match tweak.what {
                        Tweaking::Width => Message::TweakWidth(across),
                        Tweaking::Lightness => Message::TweakLightness(across),
                        Tweaking::Reach => Message::TweakReach(across),
                    };
                    return Some(canvas::Action::publish(message).and_capture());
                }

                match &mut state.interaction {
                    Interaction::Idle if self.tool != Tool::Shape || !self.shown => {
                        return Some(canvas::Action::request_redraw());
                    }
                    Interaction::Painting { last } => {
                        // Everything along the way, however fast the move.
                        let from = self.camera.to_screen(*last);
                        *last = world;
                        let steps = (from.distance(pos) / 3.0).ceil().max(1.0) as usize;
                        for i in 1..=steps {
                            self.paint_at(state, from + (pos - from) * (i as f32 / steps as f32));
                        }
                        return Some(canvas::Action::request_redraw().and_capture());
                    }
                    Interaction::MovingBackground { last } => {
                        let delta = self.camera.to_world(pos) - self.camera.to_world(*last);
                        *last = pos;
                        return Some(
                            canvas::Action::publish(Message::MoveBackground(delta)).and_capture(),
                        );
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
                        return Some(canvas::Action::request_redraw());
                    }
                    Interaction::Panning { last } => {
                        let delta = pos - *last;
                        *last = pos;
                        return Some(canvas::Action::publish(Message::Pan(delta)).and_capture());
                    }
                    Interaction::Lassoing => {
                        let far = state
                            .lasso
                            .last()
                            .is_none_or(|&p| self.camera.to_screen(p).distance(pos) >= 3.0);
                        if far {
                            state.lasso.push(world);
                        }
                        return Some(canvas::Action::request_redraw().and_capture());
                    }
                    _ => {}
                }

                // Worked out when the frame is drawn (see `follow`).
                state.aim = Some(world);
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left | mouse::Button::Middle,
            )) => {
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
                        for v in caught {
                            if !state.selection.contains(&v) {
                                state.selection.push(v);
                            }
                        }
                    }
                    state.selected_in = self.document.revision();
                    return Some(canvas::Action::request_redraw().and_capture());
                }
                // A click (not a drag) on the selection lets go of it.
                if let (Interaction::MovingSelection { from, .. }, None) = (finished, &pending) {
                    let still = screen.is_some_and(|pos| {
                        self.camera.to_screen(from).distance(pos) < VERTEX_HIT / 2.0
                    });
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
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let anchor = inside?;

                if state.modifiers.shift() {
                    // Some platforms turn Shift+wheel into horizontal scrolling.
                    let (x, y, per_notch) = match *delta {
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
                    return Some(canvas::Action::publish(message).and_capture());
                }

                let factor = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => 1.15f32.powf(y),
                    mouse::ScrollDelta::Pixels { y, .. } => (y / 200.0).exp(),
                };
                let message = if matches!(self.tool, Tool::Background { .. }) {
                    let anchor = self.camera.to_world(anchor);
                    Message::ScaleBackground { anchor, factor }
                } else {
                    Message::Zoom { anchor, factor }
                };

                Some(canvas::Action::publish(message).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => Some(canvas::Action::request_redraw()),
            Event::Window(window::Event::RedrawRequested(now)) => {
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
        match self.flipped(state) {
            Some(flipped) => flipped.draw_scene(state, renderer, bounds, cursor, self),
            None => self.draw_scene(state, renderer, bounds, cursor, self),
        }
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
    /// What to draw: the layers and, over them, what's going on.
    /// What to draw: the layers and, over them, what's going on. The
    /// other layers as `others` sees them (not mirrored, when this works
    /// through the mirror image).
    fn draw_scene(
        &self,
        state: &State,
        renderer: &Renderer,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        others_view: &Editor,
    ) -> Vec<Geometry> {
        let camera = self.camera;

        // While dragging, draw the document as it will be after release.
        let pending = match state.interaction {
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. } => None,
            _ => state.pending.as_ref(),
        };
        // Extending always shares its source edge; that's no news.
        let source = match state.interaction {
            Interaction::Extending { a, b, .. }
            | Interaction::LeavingFace {
                edge: Some((a, b, _)),
                ..
            } => Some((a, b)),
            _ => None,
        };

        // The layers: in the paint mode as they are, in their order;
        // otherwise the others faded, and the current layer over them all,
        // so it's always seen whole.
        let size = bounds.size();
        let look = self.look();
        let others = if look == Look::Painted {
            Look::Painted
        } else {
            Look::Faded
        };
        let mut layers = Vec::new();
        for layer in &self.below {
            others_view.draw_cached(renderer, size, *layer, others, &mut layers);
        }
        let mut above = Vec::new();
        for layer in &self.above {
            others_view.draw_cached(renderer, size, *layer, others, &mut above);
        }
        if look != Look::Painted {
            layers.append(&mut above);
        }
        if self.shown {
            match &pending {
                Some(pending) => {
                    let mut frame = Frame::new(renderer, size);
                    self.draw_document(&mut frame, &pending.result);
                    self.draw_changes(&mut frame, pending, source);
                    layers.push(frame.into_geometry());
                }
                None => {
                    let current = SceneLayer {
                        id: self.current,
                        document: self.document,
                        crossfade: self.crossfade,
                        show_edges: self.show_edges,
                        mirrors: self.mirrors,
                    };
                    self.draw_cached(renderer, size, current, look, &mut layers);
                }
            }
        }
        layers.append(&mut above);
        // Forget layers no longer shown.
        self.caches.layers.borrow_mut().retain(|id, _| {
            *id == self.current
                || self
                    .below
                    .iter()
                    .chain(&self.above)
                    .any(|layer| layer.id == *id)
        });

        let mut overlay = Frame::new(renderer, bounds.size());
        let cursor_pos = cursor.position_in(bounds);

        // Adjusting the background: just its outline; the drawing rests.
        if matches!(self.tool, Tool::Background { .. }) {
            if let Some(background) = self.background {
                let corners = background.corners().map(|p| camera.to_screen(p));
                let outline = Path::new(|p| {
                    p.move_to(corners[0]);
                    for &corner in &corners[1..] {
                        p.line_to(corner);
                    }
                    p.close();
                });
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(HOVER, 2.0)
                };
                overlay.stroke(&outline, dashed);
            }
            return with_overlay(layers, overlay);
        }

        // Hidden: nothing to edit; say why clicking does nothing.
        if !self.shown {
            overlay.fill_text(canvas::Text {
                content: "The current layer is hidden: show it to edit it".into(),
                position: Point::new(bounds.width / 2.0, 14.0),
                color: EDGE,
                size: 13.0.into(),
                align_x: iced::widget::text::Alignment::Center,
                ..canvas::Text::default()
            });
            return with_overlay(layers, overlay);
        }

        // Painting: what the stroke paints so far, what clicking would
        // paint, and the brush as the cursor.
        if matches!(self.tool, Tool::Paint { .. }) {
            self.draw_painting(&mut overlay, state, cursor_pos);
            return with_overlay(layers, overlay);
        }

        if self.tool == Tool::Mirror {
            self.draw_placing_mirror(&mut overlay, state, bounds, cursor_pos);
            return with_overlay(layers, overlay);
        }
        for &mirror in self.mirrors {
            self.draw_mirror_line(&mut overlay, mirror, bounds, Color { a: 0.6, ..MIRROR });
        }

        let hover = cursor_pos.and_then(|p| self.hit_test(p));

        // The outline of the shape last hovered, so it's clear what hangs
        // together and what is loose; while dragging, where the preview puts
        // its vertices. Moving over to another shape, the old outline fades
        // out as the new one fades in.
        let document = pending.map_or(self.document, |pending| &pending.result);
        let shown = match state.fading {
            Some((_, since)) => {
                let t = (since.elapsed().as_secs_f32() / SHAPE_FADE.as_secs_f32()).min(1.0);
                1.0 - (1.0 - t).powi(2)
            }
            None => 1.0,
        };
        // A highlight from another document (just opened, say) is ignored
        // until it's worked out again.
        let current = |shape: &&Highlight| shape.revision == self.document.revision();
        let previous = state
            .fading
            .as_ref()
            .and_then(|(previous, _)| previous.as_ref())
            .filter(current);
        let highlighted = [
            (previous, 1.0 - shown),
            (state.shape.as_ref().filter(current), shown),
        ];

        // Everything else is dimmed, so the highlighted shape stands
        // apart. What the pending edit adds never is.
        if highlighted[1].0.is_some() {
            let added = pending.map_or(&[][..], |pending| &pending.added);
            let mut dimmed: Vec<(f32, Vec<[Point; 3]>)> = Vec::new();
            for t in document
                .triangle_ids()
                .iter()
                .filter(|t| !added.contains(t))
            {
                let mut key = *t;
                key.sort_unstable();
                let lit: f32 = highlighted
                    .iter()
                    .filter_map(|&(shape, strength)| Some((shape?, strength)))
                    .filter(|(shape, _)| shape.triangles.binary_search(&key).is_ok())
                    .map(|(_, strength)| strength)
                    .sum();
                let dim = (1.0 - lit).clamp(0.0, 1.0);
                let corners = t.map(|v| document.vertex(v));
                match dimmed.iter_mut().find(|(d, _)| *d == dim) {
                    Some((_, group)) => group.push(corners),
                    None => dimmed.push((dim, vec![corners])),
                }
            }
            for (dim, triangles) in dimmed {
                if dim > 0.0 {
                    let wash = self.mesh(triangles.into_iter());
                    let wash_color = Color {
                        a: 0.45 * dim,
                        ..UNFOCUSED
                    };
                    overlay.fill(&wash, wash_color);
                    overlay.stroke(&wash, stroke(wash_color, 1.5));
                }
            }
        }

        for (shape, strength) in highlighted {
            if let Some(shape) = shape {
                self.draw_shape(&mut overlay, document, shape, strength);
            }
        }

        if !state.guides.is_empty()
            && matches!(
                state.interaction,
                Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
            )
        {
            self.draw_guides(&mut overlay, state, bounds);
        }
        // Pasting where it doesn't fit: the piece, in red.
        if let (Interaction::Pasting { base, from, to }, None, Some(piece)) =
            (state.interaction, pending, self.clipboard)
        {
            let piece = piece.moved(base + (to - from));
            let triangles = piece.triangles.iter().map(|t| t.map(|v| piece.vertices[v]));
            let mesh = self.mesh(triangles);
            overlay.fill(&mesh, Color { a: 0.3, ..REMOVED });
            overlay.stroke(&mesh, stroke(REMOVED, 1.5));
        }
        self.draw_selection(&mut overlay, state, pending, cursor_pos);

        match (state.interaction, &pending) {
            (
                Interaction::Lassoing
                | Interaction::MovingSelection { .. }
                | Interaction::PivotingSelection { .. },
                _,
            ) => {}
            // With a selection, clicking is about it: no hovering, but for
            // the vertex Shift+click would add or remove.
            (Interaction::Idle, _) if !state.selection.is_empty() => {
                if let (true, Some(Hover::Vertex(id))) = (state.modifiers.shift(), hover) {
                    let at = camera.to_screen(self.document.vertex(id));
                    overlay.stroke(&Path::circle(at, 11.0), stroke(HOVER, 2.5));
                }
            }
            (Interaction::Idle, _) => match cursor_pos.map(|p| (p, hover)) {
                Some((_, Some(Hover::Vertex(id)))) => {
                    // A ring as well, to stand out against the shape outline.
                    let at = camera.to_screen(self.document.vertex(id));
                    overlay.stroke(&Path::circle(at, 11.0), stroke(HOVER, 2.5));
                    handle(&mut overlay, at, HOVER);
                }
                Some((_, Some(Hover::Edge { a, b, at }))) => {
                    let a = camera.to_screen(self.document.vertex(a));
                    let b = camera.to_screen(self.document.vertex(b));
                    overlay.stroke(&Path::line(a, b), stroke(HOVER, 2.5));
                    handle(&mut overlay, camera.to_screen(at), HOVER);
                }
                Some((p, hover)) => {
                    // A face lights up: it's what D would delete.
                    if let Some(Hover::Face(t)) = hover {
                        let face =
                            self.mesh(std::iter::once(self.document.triangles().nth(t).unwrap()));
                        overlay.fill(&face, Color { a: 0.15, ..HOVER });
                        overlay.stroke(&face, stroke(HOVER, 2.0));
                    }
                    overlay.stroke(&Path::circle(p, 6.0), stroke(ADDED, 2.0));
                    overlay.fill(&Path::circle(p, 1.5), ADDED);
                }
                None => {}
            },
            (Interaction::Panning { .. }, _) => {}
            (_, Some(pending)) => {
                let at = camera.to_screen(pending.at);

                // Stuck away from the cursor (e.g. a vertex held back by an
                // edge): show where the mouse really is, so the jump on
                // release is expected. Small snaps don't count.
                if let Some(p) = cursor_pos.filter(|p| p.distance(at) > VERTEX_HIT) {
                    let dashed = Stroke {
                        line_dash: LineDash {
                            segments: &[4.0, 4.0],
                            offset: 0,
                        },
                        ..stroke(Color { a: 0.5, ..ADDED }, 1.0)
                    };
                    overlay.stroke(&Path::line(p, at), dashed);
                    overlay.stroke(
                        &Path::circle(p, 4.0),
                        stroke(Color { a: 0.7, ..ADDED }, 1.5),
                    );
                }

                // Landing on a join (a weld, or cutting into an edge) shows
                // as that, even if it squashes something on the way.
                let (_, rings) = self.joins(pending, source);
                let joins = rings
                    .iter()
                    .any(|&v| camera.to_screen(pending.result.vertex(v)).distance(at) < 1.0);
                let color = if joins {
                    JOINED
                } else if !pending.changes.squashed.is_empty() {
                    REMOVED
                } else {
                    ADDED
                };
                handle(&mut overlay, at, color);
            }
            // Releasing now would do nothing.
            (_, None) => {
                if let Some(p) = cursor_pos {
                    handle(&mut overlay, p, EDGE);
                }
            }
        }

        with_overlay(layers, overlay)
    }

    /// The lasso being drawn, the selected vertices (where the pending move
    /// puts them), the centre they rotate or scale around, and the cursor.
    fn draw_selection(
        &self,
        frame: &mut Frame,
        state: &State,
        pending: Option<&Pending>,
        cursor: Option<Point>,
    ) {
        let camera = self.camera;

        if let (Interaction::Lassoing, Some((first, rest))) =
            (state.interaction, state.lasso.split_first())
        {
            let lasso = Path::new(|p| {
                p.move_to(camera.to_screen(*first));
                for &point in rest {
                    p.line_to(camera.to_screen(point));
                }
                p.close();
            });
            frame.fill(&lasso, Color { a: 0.08, ..HOVER });
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(HOVER, 1.5)
            };
            frame.stroke(&lasso, dashed);
        }

        let at = |v: VertexId| match pending {
            Some(pending) => pending.result.vertex(pending.changes.kept(v)),
            None => self.document.vertex(v),
        };
        // Editing proportionally: how far that reaches around what's moved
        // (the selection, else the vertex dragged or hovered), and how much
        // the vertices there go along.
        let tweaking = state
            .tweaking
            .is_some_and(|tweak| tweak.what == Tweaking::Reach);
        let subjects = match state.interaction {
            _ if !state.selection.is_empty() => state.selection.clone(),
            Interaction::MovingVertex { id, .. } => vec![id],
            Interaction::Idle => {
                let near = match state.tweaking {
                    Some(tweak) if tweaking => Some(tweak.at),
                    _ => cursor,
                };
                match near.and_then(|p| self.hit_test(p)) {
                    Some(Hover::Vertex(v)) => vec![v],
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        if let Some(reach) = self.proportional.filter(|_| !subjects.is_empty()) {
            let around = Path::new(|p| {
                for &v in &subjects {
                    p.circle(camera.to_screen(at(v)), reach);
                }
            });
            frame.fill(
                &around,
                Color {
                    a: if tweaking { 0.12 } else { 0.06 },
                    ..HOVER
                },
            );
            if let [v] = subjects[..] {
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(Color { a: 0.5, ..HOVER }, 1.0)
                };
                frame.stroke(&Path::circle(camera.to_screen(at(v)), reach), dashed);
            }
            for (v, w) in self.weights(&subjects) {
                if w < 1.0 {
                    let p = camera.to_screen(at(v));
                    frame.fill(&Path::circle(p, 3.5), Color { a: w, ..SELECTED });
                }
            }
            if let (true, Some(p)) = (tweaking, cursor) {
                let label = Point::new(p.x + 14.0, p.y - 18.0);
                for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
                    frame.fill_text(canvas::Text {
                        content: format!("{:.0} px", self.proportional.unwrap_or_default()),
                        position: label + Vector::new(offset, offset),
                        color,
                        size: 13.0.into(),
                        ..canvas::Text::default()
                    });
                }
            }
        }

        // The selection: its faces (all corners selected) tinted, its
        // edges (both ends) lit, its vertices as dots of their own colour.
        if !state.selection.is_empty() {
            let selected: std::collections::HashSet<VertexId> =
                state.selection.iter().copied().collect();
            let faces = Path::new(|p| {
                for t in self.document.triangle_ids() {
                    if t.iter().all(|v| selected.contains(v)) {
                        let [a, b, c] = t.map(|v| camera.to_screen(at(v)));
                        p.move_to(a);
                        p.line_to(b);
                        p.line_to(c);
                        p.close();
                    }
                }
            });
            frame.fill(
                &faces,
                Color {
                    a: 0.18,
                    ..SELECTED
                },
            );
            let edges = Path::new(|p| {
                for (a, b) in self.document.unique_edges() {
                    if selected.contains(&a) && selected.contains(&b) {
                        p.move_to(camera.to_screen(at(a)));
                        p.line_to(camera.to_screen(at(b)));
                    }
                }
            });
            frame.stroke(&edges, stroke(SELECTED, 2.0));
        }
        for &v in &state.selection {
            let p = camera.to_screen(at(v));
            frame.fill(&Path::circle(p, 5.0), SELECTED);
            frame.stroke(&Path::circle(p, 5.0), stroke(BACKGROUND, 1.5));
        }

        if let Interaction::PivotingSelection { center, .. } = state.interaction {
            let c = camera.to_screen(center);
            let plus = Path::new(|p| {
                p.move_to(c - Vector::new(6.0, 0.0));
                p.line_to(c + Vector::new(6.0, 0.0));
                p.move_to(c - Vector::new(0.0, 6.0));
                p.line_to(c + Vector::new(0.0, 6.0));
            });
            frame.stroke(&plus, stroke(JOINED, 2.0));
            if let Some(cursor) = cursor {
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[4.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(Color { a: 0.6, ..JOINED }, 1.0)
                };
                frame.stroke(&Path::line(c, cursor), dashed);
            }
        }

        // The cursor: a cross-hair for the lasso, arrows each way with a
        // selection to move.
        let Some(p) = cursor else {
            return;
        };
        match state.interaction {
            Interaction::Lassoing => {
                frame.stroke(&Path::circle(p, 4.0), stroke(HOVER, 1.5));
            }
            Interaction::Idle
            | Interaction::MovingSelection { .. }
            | Interaction::PivotingSelection { .. }
                if !state.selection.is_empty() =>
            {
                let arrows = Path::new(|path| {
                    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                        let tip = p + Vector::new(dx * 9.0, dy * 9.0);
                        path.move_to(p);
                        path.line_to(tip);
                        path.move_to(tip + Vector::new(-dx * 3.0 - dy * 3.0, -dy * 3.0 - dx * 3.0));
                        path.line_to(tip);
                        path.line_to(tip + Vector::new(-dx * 3.0 + dy * 3.0, -dy * 3.0 + dx * 3.0));
                    }
                });
                frame.stroke(&arrows, stroke(BACKGROUND, 3.5));
                frame.stroke(&arrows, stroke(HOVER, 1.5));
            }
            _ => {}
        }
    }

    /// Which of the drawing's mirror images `world` is in: the map from
    /// the drawing to it, `None` for the drawing itself. Going back through
    /// the mirrors, last first, it's mirrored back across each it's on the
    /// far side of (from all there is before that mirror).
    fn image_at(&self, world: Point) -> Option<Affine> {
        let used = self.document.unique_vertices();
        if self.mirrors.is_empty() || used.is_empty() {
            return None;
        }
        // The middle of the drawing; of all there is up to a mirror, where
        // its images take it, on average (the maps are affine).
        let sum = used
            .iter()
            .map(|&v| self.document.vertex(v))
            .fold(Vector::ZERO, |sum, p| sum + Vector::new(p.x, p.y));
        let middle = Point::ORIGIN + sum * (1.0 / used.len() as f32);
        let mut p = world;
        let mut back = Affine::IDENTITY;
        for (k, mirror) in self.mirrors.iter().enumerate().rev() {
            let images = doc::images(&self.mirrors[..k]);
            let sum = images
                .iter()
                .map(|image| image.apply(middle))
                .fold(Vector::ZERO, |sum, q| sum + Vector::new(q.x, q.y));
            let centre = Point::ORIGIN + sum * (1.0 / images.len() as f32);
            if area2(mirror.a, mirror.b, p) * area2(mirror.a, mirror.b, centre) < 0.0 {
                p = mirror.reflect(p);
                back = back.then(mirror.affine());
            }
        }
        let image = back.inverse();
        (!image.near(Affine::IDENTITY)).then_some(image)
    }

    /// Whether `mirror` can go on the current layer (instead of the one it
    /// has): the drawing doesn't overlap its image across it.
    fn fits(&self, mirror: Mirror) -> bool {
        mirror.a != mirror.b && !self.document.overlaps_under(None, &[mirror.affine()])
    }

    /// This editor seeing the current layer through its mirror, if the
    /// cursor went over to its image (shaping or painting): there it's the
    /// image that's picked, dragged, painted and previewed, and the edits
    /// made to it go to the drawing, mirrored back.
    fn flipped(&self, state: &State) -> Option<Editor<'_>> {
        let image = state.image?;
        let edits = matches!(self.tool, Tool::Shape | Tool::Paint { .. });
        if self.mirrors.is_empty() || !edits || self.camera.image.is_some() {
            return None;
        }
        Some(Editor {
            document: self.document,
            current: self.current,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: self.below.clone(),
            above: self.above.clone(),
            camera: Camera {
                image: Some(image),
                ..self.camera
            },
            caches: self.caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            deadline: self.deadline.clone(),
        })
    }

    /// Where the drawing's vertices are: where a mirror can run through it
    /// and its image join up.
    fn mirror_points(&self) -> Vec<Point> {
        self.document
            .unique_vertices()
            .into_iter()
            .map(|v| self.document.vertex(v))
            .collect()
    }

    /// Where an end of a mirror being placed snaps, the cursor at `screen`:
    /// onto a vertex of the drawing in reach, so the mirror
    /// runs right through it, and its image joins up there.
    fn mirror_end(&self, screen: Point) -> Option<Point> {
        self.mirror_points()
            .into_iter()
            .map(|p| (p, self.camera.to_screen(p).distance(screen)))
            .filter(|&(_, d)| d <= VERTEX_HIT)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(p, _)| p)
    }

    /// A mirror being placed along `from` → `to`, snapped to run right
    /// through a vertex of the drawing it passes close by:
    /// turned around `from` if `turning` (it's on a vertex itself), else
    /// moved across, keeping its angle. Only to where the mirror fits. The
    /// line, and the vertex it now runs through.
    fn snap_mirror(&self, from: Point, to: Point, turning: bool) -> Option<(Point, Point, Point)> {
        let camera = self.camera;
        let (a, b) = (camera.to_screen(from), camera.to_screen(to));
        if a.distance(b) < 2.0 * VERTEX_HIT {
            return None;
        }
        let mut near: Vec<(Point, f32)> = self
            .mirror_points()
            .into_iter()
            .filter(|&p| camera.to_screen(p).distance(a) > 1.0)
            .map(|p| (p, closest_on_line(camera.to_screen(p), a, b).1))
            .filter(|&(_, d)| d <= VERTEX_HIT)
            .collect();
        near.sort_by(|x, y| x.1.total_cmp(&y.1));
        near.into_iter().find_map(|(p, _)| {
            let (from, to) = if turning {
                let d = p - from;
                let length = (to - from).x.hypot((to - from).y);
                (from, from + d * (length / d.x.hypot(d.y)))
            } else {
                let (foot, _) = closest_on_line(p, from, to);
                let across = p - foot;
                (from + across, to + across)
            };
            self.fits(Mirror { a: from, b: to })
                .then_some((from, to, p))
        })
    }

    /// The camera as it sees what's not seen through a mirror image: the
    /// mirror lines, other layers.
    fn plain_camera(&self) -> Camera {
        Camera {
            image: None,
            ..self.camera
        }
    }

    /// The whole line through `mirror`, across the view, dashed.
    fn draw_mirror_line(&self, frame: &mut Frame, mirror: Mirror, bounds: Rectangle, color: Color) {
        let camera = self.plain_camera();
        let (a, b) = (camera.to_screen(mirror.a), camera.to_screen(mirror.b));
        let d = b - a;
        let length = d.x.hypot(d.y);
        if length == 0.0 {
            return;
        }
        let far = d * ((bounds.width + bounds.height) * 2.0 / length);
        let dashed = Stroke {
            line_dash: LineDash {
                segments: &[10.0, 6.0],
                offset: 0,
            },
            ..stroke(color, 1.5)
        };
        frame.stroke(&Path::line(a - far, b + far), dashed);
    }

    /// Placing a mirror: where it can't go (over the drawing), the line
    /// being drawn (green if it does, red if it doesn't) and the mirror
    /// image it makes, and the cursor.
    fn draw_placing_mirror(
        &self,
        frame: &mut Frame,
        state: &State,
        bounds: Rectangle,
        cursor: Option<Point>,
    ) {
        let camera = self.camera;
        frame.fill_text(canvas::Text {
            content: "Drag a line clear of the drawing to mirror it across · it snaps to run through vertices, to join up there · Shift: snap the angle · Esc: cancel".into(),
            position: Point::new(bounds.width / 2.0, 14.0),
            color: EDGE,
            size: 13.0.into(),
            align_x: iced::widget::text::Alignment::Center,
            ..canvas::Text::default()
        });

        // Lines crossing the drawing's outline (its convex hull) mirror it
        // over itself; any line clear of it will do.
        let points: Vec<Point> = self
            .mirror_points()
            .into_iter()
            .map(|p| camera.to_screen(p))
            .collect();
        let hull = crate::geometry::convex_hull(&points);
        if let Some((first, rest)) = hull.split_first() {
            let outline = Path::new(|p| {
                p.move_to(*first);
                for &point in rest {
                    p.line_to(point);
                }
                p.close();
            });
            frame.fill(&outline, Color { a: 0.10, ..REMOVED });
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[4.0, 4.0],
                    offset: 0,
                },
                ..stroke(Color { a: 0.6, ..REMOVED }, 1.5)
            };
            frame.stroke(&outline, dashed);
        }

        // The mirror there is now, being replaced.
        for &mirror in self.mirrors {
            self.draw_mirror_line(frame, mirror, bounds, Color { a: 0.3, ..MIRROR });
        }

        if let Interaction::PlacingMirror { from, to, .. } = state.interaction
            && from != to
        {
            let mirror = Mirror { a: from, b: to };
            let color = if state.mirror_fits { ADDED } else { REMOVED };
            let image = self.document.reflection(mirror);
            let mesh = self.mesh(image.triangles());
            frame.fill(&mesh, Color { a: 0.2, ..color });
            frame.stroke(&mesh, stroke(Color { a: 0.6, ..color }, 1.0));
            self.draw_mirror_line(frame, mirror, bounds, Color { a: 0.7, ..color });
            let (a, b) = (camera.to_screen(from), camera.to_screen(to));
            frame.stroke(&Path::line(a, b), stroke(color, 2.5));
            // Where it runs through the drawing: there it joins up with its
            // image.
            if let Some(through) = state.mirror_through {
                let at = camera.to_screen(through);
                frame.stroke(&Path::circle(at, 11.0), stroke(MIRROR, 2.5));
                frame.fill(&Path::circle(at, 3.0), MIRROR);
            }
            handle(frame, a, color);
            handle(frame, b, color);
            return;
        }

        if let Some(p) = cursor {
            let snapped = self.mirror_end(p);
            let at = snapped.map_or(p, |q| camera.to_screen(q));
            if snapped.is_some() {
                frame.stroke(&Path::circle(at, 11.0), stroke(MIRROR, 2.5));
            }
            handle(frame, at, MIRROR);
        }
    }

    /// Where a drag to `world` gets to, and what releasing there does. If
    /// nothing works out (in time), it stays where it was.
    fn follow(&self, state: &mut State, world: Point) {
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        // With Shift, a dragged point lines up with what's around it.
        let shift = state.modifiers.shift();
        state.guides.clear();
        state.lined = None;
        let guided = |dragged: Dragged, state_guides: &mut Vec<Guide>| {
            if !shift {
                return None;
            }
            let (near, lined) = self.guided(dragged, world)?;
            *state_guides = near;
            lined
        };
        match &mut state.interaction {
            Interaction::Creating { to, .. } => {
                *to = world;
                state.pending = self.pending(state.interaction);
            }
            Interaction::MovingVertex { id, to } => {
                if let Some((lined, pending)) = guided(Dragged::Vertex(*id), &mut state.guides) {
                    *to = lined.at;
                    state.pending = Some(pending);
                    state.lined = Some(lined);
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
            Interaction::Extending { a, b, start, apex } => {
                let dragged = Dragged::Apex {
                    a: *a,
                    b: *b,
                    start: *start,
                };
                if let Some((lined, pending)) = guided(dragged, &mut state.guides) {
                    *apex = lined.at;
                    state.pending = Some(pending);
                    state.lined = Some(lined);
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
                    Some((a, b, start)) => {
                        let dragged = Dragged::Apex { a, b, start };
                        if let Some((lined, pending)) = guided(dragged, &mut state.guides) {
                            *apex = lined.at;
                            state.pending = Some(pending);
                            state.lined = Some(lined);
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
            | Interaction::Pasting { .. } => None,
            Interaction::Creating { from, to } => self.check(
                vec![Edit::AddTriangle {
                    // Seen mirrored, the other way round, so it's still to
                    // the left as seen.
                    corners: if self.camera.image.is_some_and(Affine::flips) {
                        equilateral(to, from)
                    } else {
                        equilateral(from, to)
                    },
                    snap: self.snap_distance(),
                }],
                to,
            ),
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
        let (result, _) = self
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
        // Taken: now with its paint (the same again, so it works out).
        let (result, changes) = self.document.preview_until(&edits, None)?;
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

    /// Lining up `dragged` (the cursor at `world`) with the guides around
    /// it: those coming up, and where it lines up (and what releasing there
    /// does), if anywhere that works out. `None` if a vertex is in reach, to
    /// snap onto instead.
    fn guided(&self, dragged: Dragged, world: Point) -> Option<(Vec<Guide>, Option<LinedUp>)> {
        let camera = self.camera;
        let cursor = camera.to_screen(world);
        let moving = match dragged {
            Dragged::Vertex(id) => Some(id),
            Dragged::Apex { .. } => None,
        };
        let vertex_near = self
            .snaps(cursor, |v| Some(v) == moving, |_, _| true, &[])
            .iter()
            .any(|(target, _)| matches!(target, Target::Vertex(_)));
        if vertex_near {
            return None;
        }
        let ends: Vec<VertexId> = match dragged {
            Dragged::Vertex(id) => {
                let mut ends: Vec<_> = self
                    .document
                    .unique_edges()
                    .into_iter()
                    .filter_map(|(u, v)| match (u == id, v == id) {
                        (true, _) => Some(v),
                        (_, true) => Some(u),
                        _ => None,
                    })
                    .collect();
                ends.sort_unstable();
                ends.dedup();
                ends
            }
            Dragged::Apex { a, b, .. } => vec![a, b],
        };
        let guides = self.guides(moving, &ends);
        let (near, candidates) = self.line_up(&guides, world);

        // Moving it to `at`: what that does, if it works out, lands there
        // (not snapped on elsewhere) and keeps the shape (nothing flattened
        // or dropped).
        let attempt = |at: Point| {
            let pending = match dragged {
                Dragged::Vertex(id) => match self.proportional {
                    Some(_) => self.move_proportionally(id, at).map(|(_, p)| p),
                    None => self.move_to(id, at),
                },
                Dragged::Apex { a, b, start } => self.extend(a, b, start, at),
            }?;
            let landed = camera.to_screen(pending.at).distance(camera.to_screen(at)) < 0.5;
            let kept = pending.changes.squashed.is_empty()
                && pending.result.triangle_ids().len() >= self.document.triangle_ids().len();
            (landed && kept).then_some(pending)
        };
        let found = candidates
            .into_iter()
            .take(2 * MAX_SNAPS)
            .find_map(|lined| Some((lined, attempt(lined.at)?)));

        // Shown: those it lines up with, and the nearest few others that
        // would do (where they come closest).
        let mut shown: Vec<Guide> = found
            .iter()
            .flat_map(|(lined, _)| lined.guides)
            .flatten()
            .collect();
        for (guide, closest) in near {
            if shown.len() >= GUIDES_SHOWN {
                break;
            }
            if !shown.contains(&guide) && attempt(closest).is_some() {
                shown.push(guide);
            }
        }
        Some((shown, found))
    }

    /// The guides a point dragged (`moving`, if it's a vertex already),
    /// with edges to `ends`, can line up with: straight on from the edges
    /// at its ends, or square to them; at the angle of the edges at its
    /// other ends; level or upright in the view from its ends; between two
    /// ends, or square at the point itself. Each once.
    fn guides(&self, moving: Option<VertexId>, ends: &[VertexId]) -> Vec<Guide> {
        let document = self.document;
        let camera = self.camera;
        let edges = document.unique_edges();
        let mut around: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
        for &(u, v) in &edges {
            around.entry(u).or_default().push(v);
            around.entry(v).or_default().push(u);
        }
        let skip = |v| Some(v) == moving;

        let mut guides = Vec::new();
        for &n in ends {
            for &m in around.get(&n).into_iter().flatten() {
                if !skip(m) {
                    guides.push(Guide::Straight { m, n });
                    guides.push(Guide::Square { m, n });
                }
            }
            // Parallel to the edges at the other ends (completing a
            // parallelogram, say); not those further off: too many.
            for &(u, v) in &edges {
                if skip(u) || skip(v) || u == n || v == n {
                    continue;
                }
                if ends.contains(&u) || ends.contains(&v) {
                    guides.push(Guide::Parallel { n, u, v });
                }
            }
        }
        // Last: where one's the same line as one of the above, that one says
        // more.
        for &n in ends {
            for upright in [false, true] {
                guides.push(Guide::Level { n, upright });
            }
        }
        for (i, &a) in ends.iter().enumerate() {
            for &b in &ends[i + 1..] {
                if edges.binary_search(&(a.min(b), a.max(b))).is_err() {
                    guides.push(Guide::Between { a, b });
                }
                guides.push(Guide::Corner { a, b });
            }
        }

        // The same line (or circle) from several edges: once.
        let mut unique: Vec<Guide> = Vec::new();
        for guide in guides {
            let shape = guide.shape(document, camera);
            let same = |other: &Guide| match (shape, other.shape(document, camera)) {
                (Shape::Line(o, d), Shape::Line(p, e)) => {
                    let length = d.x.hypot(d.y);
                    cross(d, e).abs() <= 1e-4 * length * e.x.hypot(e.y)
                        && cross(p - o, d).abs() <= 1e-3 * length
                }
                (Shape::Circle(c, r), Shape::Circle(k, s)) => {
                    c.distance(k) <= 1e-3 && (r - s).abs() <= 1e-3
                }
                _ => false,
            };
            let empty = match shape {
                Shape::Line(_, d) => d.x == 0.0 && d.y == 0.0,
                Shape::Circle(_, r) => r == 0.0,
            };
            if !empty && !unique.iter().any(same) {
                unique.push(guide);
            }
        }
        unique
    }

    /// Of `guides`, those coming up near the cursor (at `world`), nearest
    /// first, with where on them it comes closest; and where it lines up
    /// with them, best first: points (where two cross, or as far as an edge
    /// is long) before lines, nearest first.
    fn line_up(&self, guides: &[Guide], world: Point) -> (Vec<(Guide, Point)>, Vec<Lined>) {
        let camera = self.camera;
        let document = self.document;
        let cursor = camera.to_screen(world);
        let off = |p: Point| camera.to_screen(p).distance(cursor);
        let mut near = Vec::new();
        let mut points = Vec::new();
        let mut lines = Vec::new();
        for &guide in guides {
            let shape = guide.shape(document, camera);
            // The closest point on it.
            let closest = match shape {
                Shape::Line(o, d) => {
                    let (q, _) =
                        closest_on_line(cursor, camera.to_screen(o), camera.to_screen(o + d));
                    camera.to_world(q)
                }
                Shape::Circle(c, r) => {
                    let away = world - c;
                    let length = away.x.hypot(away.y);
                    if length == 0.0 {
                        continue;
                    }
                    c + away * (r / length)
                }
            };
            let distance = off(closest);
            if distance > GUIDE_NEAR {
                continue;
            }
            near.push((distance, guide, closest));
            if distance <= GUIDE_HIT {
                let lined = Lined {
                    at: closest,
                    guides: [Some(guide), None],
                    equal: false,
                };
                lines.push((distance, lined));
            }
            // As long as the edge it follows: straight on, past its end;
            // square or parallel, either way.
            let ways: &[f32] = match guide {
                Guide::Straight { .. } => &[1.0],
                Guide::Square { .. } | Guide::Parallel { .. } => &[1.0, -1.0],
                Guide::Between { .. } | Guide::Corner { .. } | Guide::Level { .. } => &[],
            };
            if let Shape::Line(o, d) = shape {
                for &way in ways {
                    let p = o + d * way;
                    if off(p) <= GUIDE_POINT_HIT {
                        let lined = Lined {
                            at: p,
                            guides: [Some(guide), None],
                            equal: true,
                        };
                        points.push((off(p), lined));
                    }
                }
            }
        }
        near.sort_by(|x, y| x.0.total_cmp(&y.0));
        let near: Vec<(Guide, Point)> = near.into_iter().map(|(_, g, at)| (g, at)).collect();
        for (i, &(g, _)) in near.iter().enumerate() {
            for &(h, _) in &near[i + 1..] {
                for at in crossings(g.shape(document, camera), h.shape(document, camera)) {
                    if off(at) <= GUIDE_POINT_HIT {
                        let lined = Lined {
                            at,
                            guides: [Some(g), Some(h)],
                            equal: false,
                        };
                        points.push((off(at), lined));
                    }
                }
            }
        }
        points.sort_by(|x, y| x.0.total_cmp(&y.0));
        lines.sort_by(|x, y| x.0.total_cmp(&y.0));
        let lined = points.into_iter().chain(lines).map(|(_, l)| l).collect();
        (near, lined)
    }

    /// The guides coming up for a point dragged with Shift, faint; the ones
    /// it lined up with bold, with the edges they follow, marked: parallel
    /// edges with an arrowhead each, edges as long with two ticks each, a
    /// right angle with a square in its corner, where two cross with a
    /// diamond.
    fn draw_guides(&self, frame: &mut Frame, state: &State, bounds: Rectangle) {
        let camera = self.camera;
        let document = self.document;
        let at = |v| camera.to_screen(document.vertex(v));
        let active: Vec<Guide> = state
            .lined
            .map(|lined| lined.guides.iter().flatten().copied().collect())
            .unwrap_or_default();
        let reach = bounds.width + bounds.height;
        for guide in &state.guides {
            let on = active.contains(guide);
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(
                    Color {
                        a: if on { 0.9 } else { 0.3 },
                        ..GUIDE
                    },
                    if on { 1.5 } else { 1.0 },
                )
            };
            match guide.shape(document, camera) {
                Shape::Line(o, d) => {
                    let (a, b) = (camera.to_screen(o), camera.to_screen(o + d));
                    let length = a.distance(b).max(1e-6);
                    let far = (b - a) * (reach * 2.0 / length);
                    frame.stroke(&Path::line(a - far, b + far), dashed);
                }
                Shape::Circle(c, r) => {
                    frame.stroke(&Path::circle(camera.to_screen(c), r * camera.zoom), dashed);
                }
            }
            if let (true, Some((u, v))) = (on, guide.followed()) {
                frame.stroke(&Path::line(at(u), at(v)), stroke(GUIDE, 3.0));
            }
        }

        let Some(lined) = state.lined else {
            return;
        };
        let p = camera.to_screen(lined.at);
        // Marks halfway along `a`–`b`: an arrowhead pointing along it, or
        // two ticks across.
        let mark = |frame: &mut Frame, a: Point, b: Point, arrow: bool| {
            let d = b - a;
            let length = d.x.hypot(d.y);
            if length < 1.0 {
                return;
            }
            let (x, y) = (d.x / length, d.y / length);
            let (along, across) = (Vector::new(x, y), Vector::new(-y, x));
            let mid = a + d * 0.5;
            let path = Path::new(|path| {
                if arrow {
                    path.move_to(mid - along * 4.0 + across * 4.0);
                    path.line_to(mid + along * 2.0);
                    path.line_to(mid - along * 4.0 - across * 4.0);
                } else {
                    for k in [-2.0, 2.0] {
                        path.move_to(mid + along * k - across * 5.0);
                        path.line_to(mid + along * k + across * 5.0);
                    }
                }
            });
            frame.stroke(&path, stroke(GUIDE, 2.0));
        };
        // A right angle at `corner`, between the ways to `a` and `b`.
        let square = |frame: &mut Frame, corner: Point, a: Point, b: Point| {
            let unit = |q: Point| {
                let d = q - corner;
                d * (1.0 / d.x.hypot(d.y).max(1e-6))
            };
            let (u, w) = (unit(a) * 9.0, unit(b) * 9.0);
            let path = Path::new(|path| {
                path.move_to(corner + u);
                path.line_to(corner + u + w);
                path.line_to(corner + w);
            });
            frame.stroke(&path, stroke(GUIDE, 2.0));
        };
        for guide in lined.guides.iter().flatten() {
            if let Guide::Corner { a, b } = *guide {
                frame.stroke(&Path::line(at(a), p), stroke(GUIDE, 2.0));
                frame.stroke(&Path::line(at(b), p), stroke(GUIDE, 2.0));
                square(frame, p, at(a), at(b));
                continue;
            }
            if let Guide::Level { n, .. } = *guide {
                frame.stroke(&Path::line(at(n), p), stroke(GUIDE, 2.0));
                continue;
            }
            // The new edge, from the end it starts at, and the one it
            // follows; marked alike.
            let (Some(from), Some((u, v))) = (guide.from(), guide.followed()) else {
                continue;
            };
            frame.stroke(&Path::line(at(from), p), stroke(GUIDE, 2.0));
            match guide {
                Guide::Parallel { .. } => {
                    // Pointing the same way.
                    let (a, b) = (at(u), at(v));
                    let ahead = p - at(from);
                    let same_way = (b - a).x * ahead.x + (b - a).y * ahead.y >= 0.0;
                    let (a, b) = if same_way { (a, b) } else { (b, a) };
                    mark(frame, a, b, true);
                    mark(frame, at(from), p, true);
                }
                Guide::Square { m, n } => square(frame, at(*n), at(*m), p),
                _ => {}
            }
            if lined.equal {
                mark(frame, at(u), at(v), false);
                mark(frame, at(from), p, false);
            }
        }
        if lined.guides[1].is_some() {
            let diamond = Path::new(|path| {
                path.move_to(p + Vector::new(0.0, -7.0));
                path.line_to(p + Vector::new(7.0, 0.0));
                path.line_to(p + Vector::new(0.0, 7.0));
                path.line_to(p + Vector::new(-7.0, 0.0));
                path.close();
            });
            frame.stroke(&diamond, stroke(GUIDE, 2.0));
        }
    }

    /// How far to move `points` (world, already moved, together) so that
    /// one of them lands on something to snap to (see [`Self::snaps`]),
    /// best first: onto vertices, nearest first, then onto the rest. What
    /// `moving` says moves along isn't snapped to.
    fn snap_offsets(&self, points: &[Point], moving: &dyn Fn(VertexId) -> bool) -> Vec<Vector> {
        let mut found: Vec<(bool, f32, Vector)> = Vec::new();
        for &p in points {
            let at = self.camera.to_screen(p);
            for (target, q) in self.snaps(at, moving, |u, v| moving(u) || moving(v), &[]) {
                let distance = self.camera.to_screen(q).distance(at);
                found.push((!matches!(target, Target::Vertex(_)), distance, q - p));
            }
        }
        found.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let mut offsets: Vec<Vector> = Vec::new();
        for (_, _, offset) in found {
            let near = |o: &Vector| (o.x - offset.x).hypot(o.y - offset.y) < 1e-4;
            if !offsets.iter().any(near) {
                offsets.push(offset);
            }
            if offsets.len() == MAX_SNAPS {
                break;
            }
        }
        offsets
    }

    /// Moving each of `selection` (and, editing proportionally, those
    /// around it) to where `f` takes it, all at once (the cursor `at`). `f`
    /// is told how much to move each: 1 fully, less for those around.
    /// `None` if that doesn't work out: it folds something over, say.
    fn transform(
        &self,
        selection: &[VertexId],
        at: Point,
        f: impl Fn(Point, f32) -> Point,
    ) -> Option<Pending> {
        let moves = self
            .weights(selection)
            .into_iter()
            .map(|(v, w)| (v, f(self.document.vertex(v), w)))
            .collect();
        self.check(vec![Edit::MoveVertices { moves }], at)
    }

    /// Dragging vertex `id` to `to`, editing proportionally: those around
    /// it go along. Onto the best snap that works out (a vertex to weld
    /// with, an edge to join), else `to` itself. `None` if nothing works
    /// out: it would fold something over, say.
    fn move_proportionally(&self, id: VertexId, to: Point) -> Option<(Point, Pending)> {
        let from = self.document.vertex(id);
        let snaps = self.snaps(
            self.camera.to_screen(to),
            |v| v == id,
            |u, v| u == id || v == id,
            &[],
        );
        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain([to])
            .find_map(|p| {
                let by = p - from;
                Some((p, self.transform(&[id], p, |q, w| q + by * w)?))
            })
    }

    /// How much each vertex goes along with `selection`: those selected
    /// fully (1); editing proportionally, those within reach (on screen) of
    /// one the less the further, smoothly down to nothing.
    fn weights(&self, selection: &[VertexId]) -> Vec<(VertexId, f32)> {
        let mut weights: Vec<_> = selection.iter().map(|&v| (v, 1.0)).collect();
        let Some(reach) = self.proportional.filter(|_| !selection.is_empty()) else {
            return weights;
        };
        let reach = reach / self.camera.zoom;
        let selected: Vec<Point> = selection.iter().map(|&v| self.document.vertex(v)).collect();
        for v in self.document.unique_vertices() {
            if selection.contains(&v) {
                continue;
            }
            let p = self.document.vertex(v);
            let near = selected
                .iter()
                .map(|q| p.distance(*q))
                .fold(f32::INFINITY, f32::min);
            if near < reach {
                let t = 1.0 - near / reach;
                weights.push((v, t * t * (3.0 - 2.0 * t)));
            }
        }
        weights
    }

    /// The centre of `selection`: the average of its vertices.
    fn centre(&self, selection: &[VertexId]) -> Point {
        let sum = selection
            .iter()
            .map(|&v| self.document.vertex(v))
            .fold(Vector::ZERO, |sum, p| sum + Vector::new(p.x, p.y));
        let n = selection.len().max(1) as f32;
        Point::new(sum.x / n, sum.y / n)
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

    /// Draws the current layer, or what it's about to become.
    fn draw_document(&self, frame: &mut Frame, document: &Document) {
        self.draw_layer(
            frame,
            document,
            self.look(),
            Crossfade::default(),
            true,
            self.mirrors,
        );
    }

    /// Draws a layer; in the painted look crossfaded as `crossfade` says,
    /// and its edges only if `show_edges`. With `mirrors`, mirrored: in the
    /// painted look as it will be, otherwise its mirror images faint, so
    /// it's clear they're not there to edit. (Seen through one of them,
    /// that one's drawn as the drawing, and the drawing faint.)
    fn draw_layer(
        &self,
        frame: &mut Frame,
        document: &Document,
        look: Look,
        crossfade: Crossfade,
        show_edges: bool,
        mirrors: &[Mirror],
    ) {
        if !mirrors.is_empty() {
            // Mapped so that, as this sees it, it's where it is.
            let seen = self.camera.image.unwrap_or(Affine::IDENTITY);
            let unseen = seen.inverse();
            if look == Look::Painted {
                let mirrored = document.with_mirrors(mirrors).transformed(unseen);
                self.draw_layer(frame, &mirrored, look, crossfade, show_edges, &[]);
            } else {
                let strength = if look == Look::Faded { 0.35 } else { 1.0 };
                for image in doc::images(mirrors) {
                    if !image.near(seen) {
                        let ghost = document.transformed(image.then(unseen));
                        self.draw_hinted(frame, &ghost, strength * 0.4, 1.0);
                    }
                }
                self.draw_layer(frame, document, look, crossfade, show_edges, &[]);
            }
            return;
        }
        match look {
            Look::Painted => {}
            // Both with a hint of the colours, to still tell what's
            // painted; other layers fainter.
            Look::Plain => {
                self.draw_hinted(frame, document, 1.0, 1.5);
                if self.shows_vertices(look) {
                    self.draw_vertices(frame, document);
                }
                return;
            }
            Look::Faded => {
                self.draw_hinted(frame, document, 0.35, 1.0);
                return;
            }
        }

        // Unpainted faces, then the painted ones by colour.
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let unpainted = document
            .triangles()
            .zip(document.colors())
            .filter(|(_, color)| color.is_none())
            .map(|(t, _)| t);
        frame.fill(&self.mesh(unpainted), FILL);
        for (color, triangles) in painted_faces(document) {
            frame.fill(&self.mesh(triangles.into_iter()), color);
        }
        if !show_edges {
            return;
        }

        // The edges, each its own outline, so edges of different widths
        // meet without lying over each other.
        let edges: Vec<_> = document
            .unique_edges()
            .into_iter()
            .map(|(a, b)| match document.edge_style(a, b) {
                Some(style) => (a, b, self.edge_width(style.width), style.color),
                None => (a, b, 1.5, EDGE),
            })
            .collect();
        let outlines = joints::outlines(
            &edges
                .iter()
                .map(|&(a, b, width, _)| (a, b, width))
                .collect::<Vec<_>>(),
            screen,
        );
        let path = |outlines: &[&Vec<Point>]| {
            Path::new(|p| {
                for outline in outlines {
                    p.move_to(outline[0]);
                    for &point in &outline[1..] {
                        p.line_to(point);
                    }
                    p.close();
                }
            })
        };
        // Crossfaded: each painted edge its own colour, fading at its ends
        // (a gradient along it; see `fade`).
        let fades = if crossfade.edges {
            fade::edge_fades(document, crossfade.width * self.camera.zoom, screen)
        } else {
            HashMap::new()
        };
        let mut by_color: Vec<(Color, Vec<&Vec<Point>>)> = Vec::new();
        for (&(a, b, _, color), outline) in edges.iter().zip(&outlines) {
            if let Some(stops) = fades.get(&(a, b)) {
                let (pa, pb) = (screen(a), screen(b));
                let length = stops[3].0;
                let gradient = stops.iter().fold(
                    canvas::gradient::Linear::new(pa, pb),
                    |gradient, &(at, color)| gradient.add_stop(at / length, color),
                );
                frame.fill(&path(&[outline]), gradient);
                continue;
            }
            match by_color.iter_mut().find(|(c, _)| *c == color) {
                Some((_, group)) => group.push(outline),
                None => by_color.push((color, vec![outline])),
            }
        }
        for (color, outlines) in by_color {
            frame.fill(&path(&outlines), color);
        }
    }

    /// Whether the vertices are drawn: in the shape mode, on the layer
    /// being shaped (they're what the shape hangs on).
    fn shows_vertices(&self, look: Look) -> bool {
        self.tool == Tool::Shape && look == Look::Plain
    }

    /// A dot on each vertex.
    fn draw_vertices(&self, frame: &mut Frame, document: &Document) {
        let dots = Path::new(|p| {
            for v in document.unique_vertices() {
                p.circle(self.camera.to_screen(document.vertex(v)), 2.5);
            }
        });
        frame.fill(&dots, EDGE);
    }

    /// The plain look, `strength` strong (1: full), edges `width` wide, with
    /// a hint of the face colours and edge colours.
    fn draw_hinted(&self, frame: &mut Frame, document: &Document, strength: f32, width: f32) {
        let faint = |color: Color, alpha: f32| Color {
            a: color.a * alpha * strength,
            ..color
        };
        let mesh = self.mesh(document.triangles());
        frame.fill(&mesh, faint(FILL, 1.0));
        for (color, triangles) in painted_faces(document) {
            frame.fill(&self.mesh(triangles.into_iter()), faint(color, 0.25));
        }
        frame.stroke(&mesh, stroke(faint(EDGE, 1.0), width));
        let screen = |v| self.camera.to_screen(document.vertex(v));
        for (a, b, style) in document.edge_styles() {
            frame.stroke(
                &Path::line(screen(a), screen(b)),
                stroke(faint(style.color, 0.5), width),
            );
        }
    }

    /// Adds a layer's drawing to `layers`, from its cache if what it's drawn
    /// from hasn't changed.
    fn draw_cached(
        &self,
        renderer: &Renderer,
        size: Size,
        layer: SceneLayer<'_>,
        look: Look,
        layers: &mut Vec<Geometry>,
    ) {
        // Only the paint mode shows colours as they are, and may hide the
        // edges.
        let painted = look == Look::Painted;
        let crossfade = if painted {
            layer.crossfade
        } else {
            Crossfade::default()
        };
        let show_edges = !painted || layer.show_edges;
        let camera = self.camera;
        let key = LayerKey {
            revision: layer.document.revision(),
            look,
            crossfade,
            show_edges,
            vertices: self.shows_vertices(look),
            mirrors: layer.mirrors.to_vec(),
            camera: (camera.pan, camera.zoom, camera.rotation, camera.image),
            size,
        };
        let mut caches = self.caches.layers.borrow_mut();
        let cache = caches.entry(layer.id).or_insert_with(|| LayerCache {
            key: None,
            cache: canvas::Cache::new(),
        });
        if cache.key.as_ref() != Some(&key) {
            cache.key = Some(key);
            cache.cache.clear();
        }
        layers.push(cache.cache.draw(renderer, size, |frame| {
            self.draw_layer(
                frame,
                layer.document,
                look,
                crossfade,
                show_edges,
                layer.mirrors,
            );
        }));
    }

    /// A painted edge's width on screen: never too thin to see.
    fn edge_width(&self, width: f32) -> f32 {
        (width * self.camera.zoom).max(1.0)
    }

    /// Paints `faces` or `edges` (whichever is painted) with the brush: the
    /// edit for those that change, if any.
    fn paint(
        &self,
        faces: Vec<TriangleId>,
        edges: Vec<(VertexId, VertexId)>,
    ) -> canvas::Action<Message> {
        // The tool changed meanwhile: nothing.
        let Tool::Paint {
            target,
            brush,
            width,
            ..
        } = self.tool
        else {
            return canvas::Action::request_redraw();
        };
        // Only what changes.
        let edits: Vec<_> = match target {
            paint::Target::Faces => {
                let color = brush.color();
                faces
                    .into_iter()
                    .filter(|&t| self.document.color(t) != color)
                    .map(|triangle| Edit::Paint { triangle, color })
                    .collect()
            }
            paint::Target::Edges => {
                let style = brush.color().map(|color| EdgeStyle { color, width });
                edges
                    .into_iter()
                    .filter(|&(a, b)| self.document.edge_style(a, b) != style)
                    .map(|(a, b)| Edit::PaintEdge { a, b, style })
                    .collect()
            }
        };
        if edits.is_empty() {
            return canvas::Action::request_redraw();
        }
        let revision = self.document.revision();
        canvas::Action::publish(Message::Edit { edits, revision })
    }

    /// Adds what's under `screen` to the stroke: the face or the edge,
    /// depending on the target.
    fn paint_at(&self, state: &mut State, screen: Point) {
        let Tool::Paint { target, .. } = self.tool else {
            return;
        };
        match target {
            paint::Target::Faces => {
                if let Some(t) = self.document.triangle_at(self.camera.to_world(screen))
                    && !state.stroke.contains(&t)
                {
                    state.stroke.push(t);
                }
            }
            paint::Target::Edges => {
                if let Some(edge) = self.edge_at(screen)
                    && !state.edge_stroke.contains(&edge)
                {
                    state.edge_stroke.push(edge);
                }
            }
        }
    }

    /// The edge (lowest end first) closest to `screen`, within grabbing
    /// distance.
    fn edge_at(&self, screen: Point) -> Option<(VertexId, VertexId)> {
        self.edge_in(self.document, screen, self.camera)
    }

    /// Like [`Self::edge_at`], on another layer.
    fn edge_in(
        &self,
        document: &Document,
        screen: Point,
        camera: Camera,
    ) -> Option<(VertexId, VertexId)> {
        let at = |v| camera.to_screen(document.vertex(v));
        document
            .unique_edges()
            .into_iter()
            .map(|(a, b)| ((a, b), closest_on_segment(screen, at(a), at(b)).1))
            .filter(|&(_, distance)| distance <= EDGE_HIT)
            .min_by(|(_, d1), (_, d2)| d1.total_cmp(d2))
            .map(|(edge, _)| edge)
    }

    /// Whether edges are being painted (where Alt tweaks the width).
    fn paints_edges(&self) -> bool {
        matches!(
            self.tool,
            Tool::Paint {
                target: paint::Target::Edges,
                ..
            }
        )
    }

    /// Whether a click picks the brush up rather than paints: with the
    /// pipette, or holding Ctrl.
    fn picking(&self, state: &State) -> bool {
        matches!(self.tool, Tool::Paint { picking, .. } if picking || state.modifiers.command())
    }

    /// What the pipette picks up at `screen`: from the current layer, or
    /// else the visible layers in front, then behind it (frontmost first).
    fn pick(&self, screen: Point) -> Option<Picked> {
        let Tool::Paint { target, .. } = self.tool else {
            return None;
        };
        // The other layers as they are, even working through the current
        // layer's mirror image.
        let plain = self.plain_camera();
        let mut layers = std::iter::once((self.document, self.camera)).chain(
            self.above
                .iter()
                .rev()
                .chain(self.below.iter().rev())
                .map(|layer| (layer.document, plain)),
        );
        match target {
            paint::Target::Faces => layers.find_map(|(document, camera)| {
                let t = document.triangle_at(camera.to_world(screen))?;
                Some(Picked::Face(document.color(t)))
            }),
            paint::Target::Edges => layers.find_map(|(document, camera)| {
                let (a, b) = self.edge_in(document, screen, camera)?;
                Some(Picked::Edge(document.edge_style(a, b)))
            }),
        }
    }

    /// The paint mode's overlay: the stroke so far, what's under the
    /// cursor, and the brush (or pipette) following the cursor.
    fn draw_painting(&self, frame: &mut Frame, state: &State, cursor: Option<Point>) {
        // Tweaking the brush: the face or edge previewed stays the one it
        // was, while the brush follows the mouse.
        let focus = state.tweaking.map(|tweak| tweak.at).or(cursor);
        let Tool::Paint {
            target,
            brush,
            width,
            ..
        } = self.tool
        else {
            return;
        };
        let picking = self.picking(state);
        let screen = |v| self.camera.to_screen(self.document.vertex(v));
        // Picking shows what's under the cursor as it is.
        let color = brush.color().filter(|_| !picking);
        match target {
            paint::Target::Faces => {
                let triangles: Vec<_> = self.document.triangles().collect();
                let faces = |ids: &[TriangleId]| self.mesh(ids.iter().map(|&t| triangles[t]));
                let painted = faces(&state.stroke);
                match color {
                    Some(color) => frame.fill(&painted, color),
                    // Cleared: as unpainted faces look.
                    None => {
                        frame.fill(&painted, BACKGROUND);
                        frame.fill(&painted, FILL);
                    }
                }
                frame.stroke(&painted, stroke(EDGE, 1.5));

                let hovered =
                    focus.and_then(|p| self.document.triangle_at(self.camera.to_world(p)));
                if let Some(t) = hovered {
                    let face = faces(&[t]);
                    if let Some(color) = color.filter(|_| !picking) {
                        // Tweaking its lightness: as it will be.
                        let tweaking = state.tweaking.is_some();
                        let alpha = if tweaking { 1.0 } else { 0.5 };
                        frame.fill(&face, Color { a: alpha, ..color });
                    }
                    frame.stroke(&face, stroke(HOVER, 2.5));
                }
            }
            paint::Target::Edges => {
                let line = |(a, b)| Path::line(screen(a), screen(b));
                let look = match color {
                    Some(color) => stroke(color, self.edge_width(width)),
                    // Cleared: as plain edges look.
                    None => stroke(EDGE, 1.5),
                };
                for &edge in &state.edge_stroke {
                    frame.stroke(&line(edge), look);
                }
                if let Some(edge) = focus.and_then(|p| self.edge_at(p)) {
                    frame.stroke(&line(edge), stroke(HOVER, self.edge_width(width) + 4.0));
                    if !picking {
                        frame.stroke(&line(edge), look);
                    }
                }
            }
        }

        if let Some(p) = cursor {
            let (icon, hotspot) = if picking {
                pipette_cursor()
            } else {
                paint_cursor(target, color)
            };
            frame.draw_svg(
                Rectangle::new(p - hotspot, iced::Size::new(CURSOR_SIZE, CURSOR_SIZE)),
                &iced::widget::svg::Handle::from_memory(icon.into_bytes()),
            );
            // Tweaking: the width or lightness, by the brush.
            if let Some(tweak) = state.tweaking {
                let content = match (tweak.what, color) {
                    (Tweaking::Width, _) => format!("{width:.1}"),
                    (Tweaking::Lightness, Some(color)) => {
                        let hsv = paint::Hsv::from_color(color, 0.0);
                        format!("{:.0}%", hsv.value * 100.0)
                    }
                    (Tweaking::Lightness, None) | (Tweaking::Reach, _) => String::new(),
                };
                let label = Point::new(p.x + CURSOR_SIZE * 0.5, p.y - CURSOR_SIZE * 0.9);
                for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
                    frame.fill_text(canvas::Text {
                        content: content.clone(),
                        position: label + Vector::new(offset, offset),
                        color,
                        size: 13.0.into(),
                        ..canvas::Text::default()
                    });
                }
            }
        }
    }

    /// Highlights how the pending result differs from the current document:
    /// added triangles in green; removed edges, and triangles squashed flat
    /// (as the line they collapse into), in red; and in yellow where things
    /// join up: vertices welded together, outline edges that become shared
    /// (other than `source`) and outline edges cut where a vertex lands on
    /// them.
    fn draw_changes(
        &self,
        frame: &mut Frame,
        pending: &Pending,
        source: Option<(VertexId, VertexId)>,
    ) {
        let Pending {
            result, changes, ..
        } = pending;
        let before = self.document;
        let screen = |v| self.camera.to_screen(result.vertex(v));
        let segments = |edges: &[(VertexId, VertexId)]| {
            Path::new(|p| {
                for &(u, v) in edges {
                    p.move_to(screen(u));
                    p.line_to(screen(v));
                }
            })
        };

        let added = self.mesh(pending.added.iter().map(|t| t.map(|v| result.vertex(v))));
        frame.fill(&added, Color { a: 0.2, ..ADDED });
        frame.stroke(&added, stroke(ADDED, 1.5));

        frame.stroke(
            &segments(&removed_edges(before, pending)),
            stroke(REMOVED, 2.0),
        );

        let squashed = Path::new(|p| {
            for t in &changes.squashed {
                let (from, to) = longest_edge(t.map(screen));
                p.move_to(from);
                p.line_to(to);
            }
        });
        frame.stroke(&squashed, stroke(REMOVED, 3.0));

        let (joined, rings) = self.joins(pending, source);
        frame.stroke(&segments(&joined), stroke(JOINED, 4.0));
        for v in rings {
            frame.stroke(&Path::circle(screen(v), 10.0), stroke(JOINED, 2.5));
        }
    }

    /// The shape `start` belongs to, to highlight.
    fn highlight(&self, start: VertexId) -> Highlight {
        let mut triangles = self.document.connected(start);
        for t in &mut triangles {
            t.sort_unstable();
        }
        triangles.sort_unstable();
        Highlight {
            revision: self.document.revision(),
            start,
            outline: outline_of(&triangles),
            triangles,
        }
    }

    /// Highlights a `shape` (with its vertices where `document` has them):
    /// a faint tint, and its outline with a soft glow. `strength` (0 to 1)
    /// fades it.
    fn draw_shape(&self, frame: &mut Frame, document: &Document, shape: &Highlight, strength: f32) {
        if strength <= 0.0 {
            return;
        }
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let edges = Path::new(|p| {
            for &(u, v) in &shape.outline {
                p.move_to(screen(u));
                p.line_to(screen(v));
            }
        });
        let tint = self.mesh(
            shape
                .triangles
                .iter()
                .map(|t| t.map(|v| document.vertex(v))),
        );
        let faded = |a: f32| Color {
            a: a * strength,
            ..HOVER
        };

        frame.fill(&tint, faded(0.08));
        frame.stroke(&edges, stroke(faded(0.2), 8.0));
        frame.stroke(&edges, stroke(faded(1.0), 2.0));
    }

    /// Where the pending result joins things up: outline edges that become
    /// shared (other than `source`) or are cut where a vertex lands on
    /// them, and the vertices that join, as ids after it.
    fn joins(
        &self,
        pending: &Pending,
        source: Option<(VertexId, VertexId)>,
    ) -> (Vec<(VertexId, VertexId)>, Vec<VertexId>) {
        let Pending {
            result, changes, ..
        } = pending;
        let before = self.document;
        let kept = |v| changes.kept(v);
        let key = |(u, v): (VertexId, VertexId)| {
            let (u, v) = (kept(u), kept(v));
            (u.min(v), u.max(v))
        };

        let outline_before: Vec<_> = outline(before).into_iter().map(key).collect();
        let interior_before: Vec<_> = interior(before).into_iter().map(key).collect();
        let source = source.map(key);
        let mut joined: Vec<_> = interior(result)
            .into_iter()
            .filter(|&e| Some(e) != source)
            .filter(|e| outline_before.contains(e) && !interior_before.contains(e))
            .collect();
        let mut rings: Vec<VertexId> = joined.iter().flat_map(|&(u, v)| [u, v]).collect();
        rings.extend(changes.welded.iter().map(|&(_, k)| kept(k)));
        // Where added triangles attach to existing vertices; the ends of the
        // source edge are no news.
        rings.extend(
            changes
                .attached
                .iter()
                .map(|&v| kept(v))
                .filter(|&v| source.is_none_or(|(a, b)| v != a && v != b)),
        );

        // A vertex landing on an outline edge it wasn't connected to.
        let connected = |w, x| {
            before
                .triangle_ids()
                .iter()
                .any(|t| t.iter().any(|&v| kept(v) == w) && t.iter().any(|&v| kept(v) == x))
        };
        for &(a, b, w) in &changes.cut {
            let edge = (a.min(b), a.max(b));
            if outline_before.contains(&edge) && !connected(w, a) && !connected(w, b) {
                joined.extend([(a, w), (w, b)]);
                rings.push(w);
            }
        }
        rings.sort_unstable();
        rings.dedup();

        (joined, rings)
    }

    fn mesh(&self, triangles: impl Iterator<Item = [Point; 3]>) -> Path {
        Path::new(|p| {
            for [a, b, c] in triangles {
                p.move_to(self.camera.to_screen(a));
                p.line_to(self.camera.to_screen(b));
                p.line_to(self.camera.to_screen(c));
                p.close();
            }
        })
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

fn dot(a: Vector, b: Vector) -> f32 {
    a.x * b.x + a.y * b.y
}

/// Where two guides cross (world): lines once (unless parallel), a line
/// and a circle up to twice; circles aren't crossed with each other.
fn crossings(g: Shape, h: Shape) -> Vec<Point> {
    match (g, h) {
        (Shape::Line(o, d), Shape::Line(p, e)) => {
            let den = cross(d, e);
            if den.abs() <= 1e-4 * d.x.hypot(d.y) * e.x.hypot(e.y) {
                return Vec::new();
            }
            vec![o + d * (cross(p - o, e) / den)]
        }
        (Shape::Line(o, d), Shape::Circle(c, r)) | (Shape::Circle(c, r), Shape::Line(o, d)) => {
            // o + t d at distance r from c.
            let f = o - c;
            let a = d.x * d.x + d.y * d.y;
            let b = 2.0 * (f.x * d.x + f.y * d.y);
            let k = f.x * f.x + f.y * f.y - r * r;
            let disc = b * b - 4.0 * a * k;
            if a == 0.0 || disc < 0.0 {
                return Vec::new();
            }
            let root = disc.sqrt();
            [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
                .into_iter()
                .map(|t| o + d * t)
                .collect()
        }
        (Shape::Circle(..), Shape::Circle(..)) => Vec::new(),
    }
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
    let existing: Vec<_> = before.triangle_ids().iter().map(key).collect();

    after
        .triangle_ids()
        .iter()
        .filter(move |t| !existing.contains(&key(t)))
        .copied()
}

/// Edges of `before` that the pending result no longer has, as ids after
/// it. Vertices are never removed, so they can be drawn where their ends
/// are afterwards. One that was cut at a vertex is still there, in pieces.
fn removed_edges(before: &Document, pending: &Pending) -> Vec<(VertexId, VertexId)> {
    let Pending {
        result, changes, ..
    } = pending;
    let now = result.unique_edges();
    let was_cut = |e| {
        changes
            .cut
            .iter()
            .any(|&(a, b, _)| (a.min(b), a.max(b)) == e)
    };

    before
        .unique_edges()
        .into_iter()
        .map(|(u, v)| {
            let (u, v) = (changes.kept(u), changes.kept(v));
            (u.min(v), u.max(v))
        })
        .filter(|&e| e.0 != e.1 && !now.contains(&e) && !was_cut(e))
        .collect()
}

/// Edges used by exactly `uses` triangles, as `(low, high)`.
fn edges_used(document: &Document, uses: usize) -> Vec<(VertexId, VertexId)> {
    edges_used_by(document.triangle_ids(), uses)
}

/// The outline of some `triangles`: edges used by one of them.
fn outline_of(triangles: &[[VertexId; 3]]) -> Vec<(VertexId, VertexId)> {
    edges_used_by(triangles, 1)
}

fn edges_used_by(triangles: &[[VertexId; 3]], uses: usize) -> Vec<(VertexId, VertexId)> {
    let mut edges: Vec<_> = triangles
        .iter()
        .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
        .map(|(u, v)| (u.min(v), u.max(v)))
        .collect();
    edges.sort_unstable();
    edges
        .chunk_by(|x, y| x == y)
        .filter(|run| run.len() == uses)
        .map(|run| run[0])
        .collect()
}

/// Edges on the outline: used by one triangle.
fn outline(document: &Document) -> Vec<(VertexId, VertexId)> {
    edges_used(document, 1)
}

/// Edges between two triangles.
fn interior(document: &Document) -> Vec<(VertexId, VertexId)> {
    edges_used(document, 2)
}

fn draw_grid(frame: &mut Frame, camera: Camera) {
    let spacing = GRID * camera.zoom;
    if spacing < 8.0 {
        return;
    }

    // The world-space bounds of the (possibly rotated) visible area.
    let size = frame.size();
    let corners = [
        Point::ORIGIN,
        Point::new(size.width, 0.0),
        Point::new(0.0, size.height),
        Point::new(size.width, size.height),
    ]
    .map(|p| camera.to_world(p));
    let min = |f: fn(&Point) -> f32| corners.iter().map(f).fold(f32::INFINITY, f32::min);
    let max = |f: fn(&Point) -> f32| corners.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
    let visible = Rectangle::new(Point::ORIGIN, size);

    let dots = Path::new(|p| {
        let mut y = (min(|p| p.y) / GRID).floor() * GRID;
        while y <= max(|p| p.y) {
            let mut x = (min(|p| p.x) / GRID).floor() * GRID;
            while x <= max(|p| p.x) {
                let dot = camera.to_screen(Point::new(x, y));
                if visible.contains(dot) {
                    p.circle(dot, 1.2);
                }
                x += GRID;
            }
            y += GRID;
        }
    });

    frame.fill(&dots, GRID_DOT);
}

/// How big (screen px) the paint mode's cursor icon is drawn.
const CURSOR_SIZE: f32 = 26.0;

/// The paint mode's cursor: a bucket for faces, a brush for edges, both
/// dipped in the colour; an eraser to clear. The icon (24 units square) as
/// SVG, and where its tip is (screen px from its corner).
fn paint_cursor(target: paint::Target, color: Option<Color>) -> (String, Vector) {
    let hex = paint::to_hex;
    let (shadow, lines) = (hex(BACKGROUND), hex(EDGE));
    // Outlined in the background colour, to show on any colour; the paint
    // part (the bucket's drop, the brush's tip) filled with the colour.
    let icon = |outline: &str, paint: &str| {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke-linecap="round" stroke-linejoin="round"><g stroke="{shadow}" stroke-width="4">{outline}{paint}</g>{paint_filled}<g stroke="{lines}" stroke-width="2">{outline}</g></svg>"#,
            paint_filled = color.map_or(String::new(), |color| {
                let c = hex(color);
                paint.replace(
                    "<path ",
                    &format!(r#"<path fill="{c}" stroke="{c}" stroke-width="1.5" "#),
                )
            }),
        )
    };
    let unit = CURSOR_SIZE / 24.0;
    match (color, target) {
        (None, _) => (icon(icons::ERASER, ""), Vector::new(7.0, 20.5) * unit),
        (Some(_), paint::Target::Faces) => (
            icon(icons::PAINT_BUCKET, icons::PAINT_BUCKET_DROP),
            Vector::new(20.0, 22.0) * unit,
        ),
        (Some(_), paint::Target::Edges) => (
            icon(icons::BRUSH, icons::BRUSH_TIP),
            Vector::new(3.0, 21.0) * unit,
        ),
    }
}

/// The pipette cursor, like [`paint_cursor`].
fn pipette_cursor() -> (String, Vector) {
    let hex = paint::to_hex;
    let outline = icons::PIPETTE;
    let icon = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke-linecap="round" stroke-linejoin="round"><g stroke="{}" stroke-width="4">{outline}</g><g stroke="{}" stroke-width="2">{outline}</g></svg>"#,
        hex(BACKGROUND),
        hex(EDGE),
    );
    (icon, Vector::new(2.0, 22.0) * (CURSOR_SIZE / 24.0))
}

/// The painted faces, by colour.
fn painted_faces(document: &Document) -> Vec<(Color, Vec<[Point; 3]>)> {
    let mut painted: Vec<(Color, Vec<[Point; 3]>)> = Vec::new();
    for (t, color) in document.triangles().zip(document.colors()) {
        if let Some(color) = color {
            match painted.iter_mut().find(|(c, _)| *c == color) {
                Some((_, group)) => group.push(t),
                None => painted.push((color, vec![t])),
            }
        }
    }
    painted
}

/// How a layer is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    /// One colour for all faces and edges, with a hint of their paint.
    Plain,
    /// With its face colours and edge styles.
    Painted,
    /// Plain, and faint: a layer other than the one being shaped.
    Faded,
}

/// Moving the selection, the cursor now at `cursor` (world) after it was
/// away (adjusting how far proportional editing reaches, say): the move
/// goes on from there, as it was, rather than jumping to the cursor.
fn rebase(interaction: &mut Interaction, cursor: Point) {
    match interaction {
        Interaction::MovingSelection { from, to, .. } => {
            *from += cursor - *to;
            *to = cursor;
        }
        Interaction::PivotingSelection {
            pivot,
            center,
            from,
            to,
        } => {
            let d = cursor - *center;
            *from = match pivot {
                Pivot::Rotate => {
                    let angle = |p: Point| (p.y - center.y).atan2(p.x - center.x);
                    let (sin, cos) = (angle(*from) - angle(*to)).sin_cos();
                    *center + Vector::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
                }
                Pivot::Scale => {
                    let scale = to.distance(*center) / from.distance(*center);
                    if scale > 1e-6 {
                        *center + d * (1.0 / scale)
                    } else {
                        *from
                    }
                }
            };
            *to = cursor;
        }
        _ => {}
    }
}

/// The layers, with the overlay over them.
fn with_overlay(mut layers: Vec<Geometry>, overlay: Frame) -> Vec<Geometry> {
    layers.push(overlay.into_geometry());
    layers
}

fn handle(frame: &mut Frame, at: Point, color: Color) {
    frame.fill(&Path::circle(at, 6.0), color);
    frame.stroke(&Path::circle(at, 6.0), stroke(BACKGROUND, 1.5));
}

fn stroke(color: Color, width: f32) -> Stroke<'static> {
    // Round joins/caps: miter joins spike far past the corner at acute angles.
    Stroke::default()
        .with_color(color)
        .with_width(width)
        .with_line_join(LineJoin::Round)
        .with_line_cap(LineCap::Round)
}

/// Equilateral triangle with base `from`–`to`, apex to the left of the drag.
fn equilateral(from: Point, to: Point) -> [Point; 3] {
    let d = to - from;
    let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let h = 3f32.sqrt() / 2.0;
    [from, to, mid + Vector::new(d.y * h, -d.x * h)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas::Program;

    fn triangle(doc: &mut Document, corners: [(f32, f32); 3]) {
        assert!(doc.apply(Edit::AddTriangle {
            corners: corners.map(|(x, y)| Point::new(x, y)),
            snap: 0.0
        }));
    }

    /// One triangle, 0–1 along the bottom and 2 at the top.
    fn one() -> Document {
        let mut doc = Document::default();
        triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)]);
        doc
    }

    /// Two separate triangles side by side, a gap between corners 1 and 3.
    fn two_apart() -> Document {
        let mut doc = Document::default();
        triangle(&mut doc, [(100.0, 300.0), (200.0, 300.0), (150.0, 200.0)]);
        triangle(&mut doc, [(250.0, 300.0), (350.0, 300.0), (300.0, 200.0)]);
        doc
    }

    pub(super) fn editor<'a>(doc: &'a Document, caches: &'a Caches) -> Editor<'a> {
        Editor {
            document: doc,
            current: 0,
            crossfade: Crossfade::default(),
            show_edges: true,
            mirrors: &[],
            shown: true,
            below: Vec::new(),
            above: Vec::new(),
            camera: Camera::default(),
            caches,
            background: None,
            tool: Tool::Shape,
            proportional: None,
            clipboard: None,
            deadline: Cell::new(None),
        }
    }

    /// Runs mouse events, each at a cursor position; returns the published
    /// edits.
    fn drag(editor: &Editor, events: &[(mouse::Event, f32, f32)]) -> Vec<Vec<Edit>> {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        let mut published = vec![];
        for &(e, x, y) in events {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            // Each mouse event in a frame of its own, as when moving slowly.
            let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
            for event in [Event::Mouse(e), frame] {
                if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                    published.extend(action.into_inner().0);
                }
            }
        }
        published
            .into_iter()
            .filter_map(|m| match m {
                Message::Edit { edits, .. } => Some(edits),
                _ => None,
            })
            .collect()
    }

    const PRESS: mouse::Event = mouse::Event::ButtonPressed(mouse::Button::Left);
    const RELEASE: mouse::Event = mouse::Event::ButtonReleased(mouse::Button::Left);
    const MOVE: mouse::Event = mouse::Event::CursorMoved {
        position: Point::ORIGIN,
    };

    /// Runs `event` at cursor (`x`, `y`), then a frame; the edits published.
    fn run(editor: &Editor, state: &mut State, event: Event, x: f32, y: f32) -> Vec<Vec<Edit>> {
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let cursor = mouse::Cursor::Available(Point::new(x, y));
        let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
        [event, frame]
            .into_iter()
            .filter_map(|event| editor.update(state, &event, bounds, cursor))
            .flat_map(|action| action.into_inner().0)
            .filter_map(|m| match m {
                Message::Edit { edits, .. } => Some(edits),
                _ => None,
            })
            .collect()
    }

    fn key(c: char) -> Event {
        let key = keyboard::Key::Character(c.to_string().into());
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    }

    /// Lassos the right triangle of [`two_apart`] (vertices 3, 4, 5).
    fn lasso_right(editor: &Editor, state: &mut State) {
        let ctrl = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        run(editor, state, ctrl(keyboard::Modifiers::CTRL), 230.0, 180.0);
        run(editor, state, Event::Mouse(PRESS), 230.0, 180.0);
        for (x, y) in [(370.0, 180.0), (370.0, 320.0), (230.0, 320.0)] {
            run(editor, state, Event::Mouse(MOVE), x, y);
        }
        run(editor, state, Event::Mouse(RELEASE), 230.0, 320.0);
        run(
            editor,
            state,
            ctrl(keyboard::Modifiers::empty()),
            230.0,
            320.0,
        );
        assert!([3, 4, 5].iter().all(|v| state.selection.contains(v)));
    }

    /// The vertices `edits` move, and where to, sorted.
    fn moves(edits: &[Vec<Edit>]) -> Vec<(VertexId, Point)> {
        let [edits] = edits else {
            panic!("{edits:?}");
        };
        let [Edit::MoveVertices { moves }] = &edits[..] else {
            panic!("{edits:?}");
        };
        let mut moves = moves.clone();
        moves.sort_by_key(|&(v, _)| v);
        moves
    }

    #[test]
    fn a_lassoed_selection_is_dragged_together() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 340.0, 200.0);
        let edits = run(&editor, &mut state, Event::Mouse(RELEASE), 340.0, 200.0);
        let by = Vector::new(40.0, -50.0);
        assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
        // Still selected, to move on.
        assert_eq!(state.selection.len(), 3);

        // A click lets go of it.
        run(&editor, &mut state, Event::Mouse(PRESS), 500.0, 500.0);
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 500.0, 500.0).is_empty());
        assert!(state.selection.is_empty());
    }

    #[test]
    fn g_grabs_the_selection_until_a_click() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        run(&editor, &mut state, key('g'), 300.0, 250.0);
        assert!(run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0).is_empty());
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 310.0, 270.0);
        let by = Vector::new(10.0, 20.0);
        assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 310.0, 270.0).is_empty());
        // Still selected, to go on with.
        assert_eq!(state.selection.len(), 3);
    }

    #[test]
    fn a_moved_selection_snaps_to_join_up() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        // Corner 3 (250, 300) to within a few px of corner 1 (200, 300):
        // onto it.
        run(&editor, &mut state, key('g'), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 254.0, 253.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 254.0, 253.0);
        let by = Vector::new(-50.0, 0.0);
        assert_eq!(moves(&edits), [3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
    }

    #[test]
    fn r_rotates_the_selection_around_its_centre() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        // Its centre is (300, 266.67); a quarter turn round it.
        let c = Point::new(300.0, 800.0 / 3.0);
        run(&editor, &mut state, key('r'), c.x + 50.0, c.y);
        run(&editor, &mut state, Event::Mouse(MOVE), c.x, c.y + 50.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), c.x, c.y + 50.0);
        for (v, to) in moves(&edits) {
            let d = doc.vertex(v) - c;
            let expected = c + Vector::new(-d.y, d.x);
            assert!(to.distance(expected) < 1e-3, "{v}: {to:?} != {expected:?}");
        }
        assert_eq!(state.selection.len(), 3);
    }

    #[test]
    fn t_scales_the_selection_around_its_centre() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        // Twice as far from its centre (300, 266.67): twice the size.
        let c = Point::new(300.0, 800.0 / 3.0);
        run(&editor, &mut state, key('t'), c.x + 20.0, c.y);
        run(&editor, &mut state, Event::Mouse(MOVE), c.x, c.y - 40.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), c.x, c.y - 40.0);
        for (v, to) in moves(&edits) {
            let expected = c + (doc.vertex(v) - c) * 2.0;
            assert!(to.distance(expected) < 1e-3, "{v}: {to:?} != {expected:?}");
        }
        assert_eq!(state.selection.len(), 3);
    }

    #[test]
    fn ctrl_l_selects_the_whole_shape() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        let ctrl_l = || {
            let key = keyboard::Key::Character("l".into());
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyL),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::CTRL,
                text: None,
                repeat: false,
            })
        };
        let selected = |state: &State| {
            let mut selected = state.selection.clone();
            selected.sort_unstable();
            selected
        };

        // Over the left triangle: its vertices.
        run(&editor, &mut state, ctrl_l(), 150.0, 280.0);
        assert_eq!(selected(&state), [0, 1, 2]);

        // Off the drawing, from one selected vertex: its shape.
        state.selection = vec![4];
        run(&editor, &mut state, ctrl_l(), 600.0, 500.0);
        assert_eq!(selected(&state), [3, 4, 5]);
    }

    #[test]
    fn proportional_editing_takes_vertices_nearby_along() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.proportional = Some(100.0);
        let mut state = State::default();
        lasso_right(&editor, &mut state);

        run(&editor, &mut state, key('g'), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 310.0, 270.0);
        let by = Vector::new(10.0, 20.0);
        // Vertex 1 is halfway within reach of 3: half as far (smoothly, so
        // just that); 0 and 2 are out of reach.
        let mut expected = vec![(1, doc.vertex(1) + by * 0.5)];
        expected.extend([3, 4, 5].map(|v| (v, doc.vertex(v) + by)));
        assert_eq!(moves(&edits), expected);
    }

    #[test]
    fn proportional_editing_reaches_as_far_on_screen_at_any_zoom() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.proportional = Some(100.0);
        // Zoomed out to half: 100 px reach 200 world units, from vertex 3 at
        // (250, 300) as far as vertex 0 at (100, 300).
        editor.camera.zoom = 0.5;
        let weights = editor.weights(&[3]);
        let weight = |v| weights.iter().find(|&&(u, _)| u == v).map(|&(_, w)| w);
        assert_eq!(weight(3), Some(1.0));
        assert!(weight(0).is_some_and(|w| w > 0.0 && w < 1.0));
        editor.camera.zoom = 1.0;
        let weights = editor.weights(&[3]);
        assert!(!weights.iter().any(|&(v, _)| v == 0));
    }

    #[test]
    fn dragging_a_vertex_takes_those_nearby_along_proportionally() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.proportional = Some(100.0);

        // Vertex 3 at (250, 300); vertex 1, 50 away, goes half as far.
        let edits = drag(
            &editor,
            &[
                (MOVE, 250.0, 300.0),
                (PRESS, 250.0, 300.0),
                (MOVE, 260.0, 320.0),
                (RELEASE, 260.0, 320.0),
            ],
        );
        let by = Vector::new(10.0, 20.0);
        let moved = moves(&edits);
        assert!(moved.contains(&(3, doc.vertex(3) + by)), "{moved:?}");
        assert!(moved.contains(&(1, doc.vertex(1) + by * 0.5)), "{moved:?}");
        assert!(!moved.iter().any(|&(v, _)| v == 0), "{moved:?}");
    }

    #[test]
    fn alt_tweaks_the_reach_while_the_move_holds_still() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.proportional = Some(100.0);
        let mut state = State::default();
        let alt = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        lasso_right(&editor, &mut state);

        run(&editor, &mut state, key('g'), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
        run(
            &editor,
            &mut state,
            alt(keyboard::Modifiers::ALT),
            310.0,
            270.0,
        );
        let cursor = mouse::Cursor::Available(Point::new(340.0, 260.0));
        let moved = editor
            .update(&mut state, &Event::Mouse(MOVE), bounds, cursor)
            .and_then(|action| action.into_inner().0);
        assert!(
            matches!(moved, Some(Message::TweakReach(across)) if across == 30.0),
            "{moved:?}"
        );
        // Let go of Alt away from where the move was: it goes on from there.
        run(
            &editor,
            &mut state,
            alt(keyboard::Modifiers::empty()),
            340.0,
            260.0,
        );
        run(&editor, &mut state, Event::Mouse(MOVE), 345.0, 260.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 345.0, 260.0);
        let by = Vector::new(15.0, 20.0);
        let moved = moves(&edits);
        assert!(moved.contains(&(3, doc.vertex(3) + by)), "{moved:?}");
    }

    #[test]
    fn a_mirror_is_placed_clear_of_the_drawing() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.tool = Tool::Mirror;
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let place = |from: (f32, f32), to: (f32, f32)| {
            let mut state = State::default();
            let mut published = Vec::new();
            for (event, (x, y)) in [(PRESS, from), (MOVE, to), (RELEASE, to)] {
                let cursor = mouse::Cursor::Available(Point::new(x, y));
                let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
                for event in [Event::Mouse(event), frame] {
                    if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                        published.extend(action.into_inner().0);
                    }
                }
            }
            published.into_iter().find_map(|m| match m {
                Message::SetMirror(mirror) => Some(mirror),
                _ => None,
            })
        };

        // Clear of the triangle (x 100 to 300): placed.
        assert_eq!(
            place((400.0, 100.0), (400.0, 400.0)),
            Some(Mirror {
                a: Point::new(400.0, 100.0),
                b: Point::new(400.0, 400.0),
            })
        );
        // Through it: not.
        assert_eq!(place((200.0, 50.0), (200.0, 450.0)), None);
        // Starting on a vertex, it runs right through it.
        let mirror = place((303.0, 298.0), (303.0, 500.0)).unwrap();
        assert_eq!(mirror.a, Point::new(300.0, 300.0));
    }

    /// Places a mirror (instead of `mirrors`) by dragging `from`
    /// → `to`, holding `modifiers`; the one placed, if any.
    fn place_mirror(
        doc: &Document,
        mirrors: &[Mirror],
        modifiers: keyboard::Modifiers,
        from: (f32, f32),
        to: (f32, f32),
    ) -> Option<Mirror> {
        let cache = Caches::default();
        let mut editor = editor(doc, &cache);
        editor.tool = Tool::Mirror;
        editor.mirrors = mirrors;
        let mut state = State::default();
        let held = Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers));
        let mut published = Vec::new();
        for (event, (x, y)) in [
            (held, from),
            (Event::Mouse(PRESS), from),
            (Event::Mouse(MOVE), to),
            (Event::Mouse(RELEASE), to),
        ] {
            let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
            for event in [event, frame] {
                if let Some(action) = editor.update(&mut state, &event, bounds, cursor) {
                    published.extend(action.into_inner().0);
                }
            }
        }
        published.into_iter().find_map(|m| match m {
            Message::SetMirror(mirror) => Some(mirror),
            _ => None,
        })
    }

    #[test]
    fn a_mirror_snaps_to_run_through_where_the_drawing_starts() {
        let doc = one();
        let none = keyboard::Modifiers::empty();
        let through = |mirror: Mirror, p: (f32, f32)| {
            let p = Point::new(p.0, p.1);
            area2(mirror.a, mirror.b, p).abs() / mirror.a.distance(mirror.b) < 1e-3
        };

        // Drawn just right of corner 1 (300, 300): moved over onto it.
        let mirror = place_mirror(&doc, &[], none, (306.0, 50.0), (306.0, 450.0)).unwrap();
        assert!(through(mirror, (300.0, 300.0)), "{mirror:?}");
        assert!((mirror.a.x - mirror.b.x).abs() < 1e-3, "{mirror:?}");

        // From corner 1 nearly along its edge to corner 2 (200, 100), past
        // it: turned to run right along it.
        let mirror = place_mirror(&doc, &[], none, (300.0, 300.0), (165.0, 22.0)).unwrap();
        assert_eq!(mirror.a, Point::new(300.0, 300.0));
        assert!(through(mirror, (200.0, 100.0)), "{mirror:?}");
    }

    #[test]
    fn the_mirror_image_is_edited_like_the_drawing() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let mirrors = [Mirror {
            a: Point::new(400.0, 0.0),
            b: Point::new(400.0, 600.0),
        }];
        editor.mirrors = &mirrors;
        // Corner 1 (300, 300) shows mirrored at (500, 300); dragged there
        // right and down, it goes left and down.
        let edits = drag(
            &editor,
            &[
                (MOVE, 500.0, 300.0),
                (PRESS, 500.0, 300.0),
                (MOVE, 520.0, 320.0),
                (RELEASE, 520.0, 320.0),
            ],
        );
        let [edits] = &edits[..] else {
            panic!("{edits:?}");
        };
        let [Edit::MoveVertex { id: 1, to }] = edits[..] else {
            panic!("{edits:?}");
        };
        assert!(to.distance(Point::new(280.0, 320.0)) < 1e-3, "{to:?}");

        // Painting its image paints it.
        let brush = Brush::default();
        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush,
            width: 1.0,
            picking: false,
        };
        let edits = drag(
            &editor,
            &[
                (MOVE, 600.0, 250.0),
                (PRESS, 600.0, 250.0),
                (RELEASE, 600.0, 250.0),
            ],
        );
        assert_eq!(
            edits,
            vec![vec![Edit::Paint {
                triangle: 0,
                color: brush.color()
            }]]
        );
    }

    #[test]
    fn with_shift_a_dragged_vertex_lines_up_with_the_edges_around_it() {
        // A square: A (100, 300), B (300, 300), C (300, 100), D (100, 100),
        // as triangles A B C and A C D.
        let mut doc = Document::default();
        triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (300.0, 100.0)]);
        triangle(&mut doc, [(100.0, 300.0), (300.0, 100.0), (100.0, 100.0)]);
        let d = (0..doc.vertex_slots())
            .find(|&v| doc.vertex(v) == Point::new(100.0, 100.0))
            .unwrap();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let shift = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        let drag_d = |to: (f32, f32), modifiers| {
            let mut state = State::default();
            run(&editor, &mut state, Event::Mouse(MOVE), 100.0, 100.0);
            run(&editor, &mut state, Event::Mouse(PRESS), 100.0, 100.0);
            run(&editor, &mut state, shift(modifiers), 100.0, 100.0);
            run(&editor, &mut state, Event::Mouse(MOVE), to.0, to.1);
            let lined = state.lined;
            let edits = run(&editor, &mut state, Event::Mouse(RELEASE), to.0, to.1);
            let [edits] = &edits[..] else {
                panic!("{edits:?}");
            };
            let [Edit::MoveVertex { id, to }] = edits[..] else {
                panic!("{edits:?}");
            };
            assert_eq!(id, d);
            (to, lined)
        };

        // Without Shift, where it's let go.
        let (to, lined) = drag_d((103.0, 40.0), keyboard::Modifiers::empty());
        assert_eq!(to, Point::new(103.0, 40.0));
        assert!(lined.is_none());
        // With: edge A D square to A B (and so parallel to B C).
        let (to, lined) = drag_d((103.0, 40.0), keyboard::Modifiers::SHIFT);
        assert!(to.distance(Point::new(100.0, 40.0)) < 1e-3, "{to:?}");
        assert!(matches!(
            lined.unwrap().guides[0],
            Some(Guide::Square { .. })
        ));
        // Straight on from B C, past C, as long as it: at (300, -100).
        let (to, lined) = drag_d((303.0, -97.0), keyboard::Modifiers::SHIFT);
        assert!(to.distance(Point::new(300.0, -100.0)) < 1e-3, "{to:?}");
        assert!(lined.unwrap().equal);
        // A right angle at D itself, between D A and D C: on the circle over
        // A C (centre (200, 200)).
        let (to, lined) = drag_d((64.0, 150.0), keyboard::Modifiers::SHIFT);
        let centre = Point::new(200.0, 200.0);
        assert!(
            (to.distance(centre) - 200.0 * 2f32.sqrt() / 2.0).abs() < 1e-2,
            "{to:?}"
        );
        assert!(matches!(
            lined.unwrap().guides[0],
            Some(Guide::Corner { .. })
        ));
        let (a, c) = (Point::new(100.0, 300.0), Point::new(300.0, 100.0));
        let dot = (a - to).x * (c - to).x + (a - to).y * (c - to).y;
        assert!(dot.abs() < 1.0, "{dot}");
    }

    #[test]
    fn with_shift_a_new_edge_lines_up_level_in_the_view() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        // The view turned a little: level on screen is at an angle in the
        // world, along no edge.
        editor.camera = Camera {
            rotation: std::f32::consts::FRAC_PI_6,
            pan: Vector::new(200.0, -50.0),
            ..Camera::default()
        };
        let camera = editor.camera;
        let shift = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        // Corner 2 dragged to near level (on screen) with corner 0.
        let (s0, s2) = (
            camera.to_screen(doc.vertex(0)),
            camera.to_screen(doc.vertex(2)),
        );
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), s2.x, s2.y);
        run(&editor, &mut state, Event::Mouse(PRESS), s2.x, s2.y);
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::SHIFT),
            s2.x,
            s2.y,
        );
        run(
            &editor,
            &mut state,
            Event::Mouse(MOVE),
            s2.x - 30.0,
            s0.y + 4.0,
        );
        let lined = state.lined.expect("lined up");
        assert!(
            lined.guides.iter().flatten().any(|guide| matches!(
                guide,
                Guide::Level {
                    n: 0,
                    upright: false
                }
            )),
            "{lined:?}"
        );
        let on_screen = camera.to_screen(lined.at);
        assert!((on_screen.y - s0.y).abs() < 1e-2, "{on_screen:?} {s0:?}");
    }

    #[test]
    fn guides_that_would_flatten_the_shape_are_left_out() {
        // The square A B C D again; D (100, 100) dragged with Shift to just
        // off the line through A and C, past A: there triangle A C D would
        // be flat.
        let mut doc = Document::default();
        triangle(&mut doc, [(100.0, 300.0), (300.0, 300.0), (300.0, 100.0)]);
        triangle(&mut doc, [(100.0, 300.0), (300.0, 100.0), (100.0, 100.0)]);
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let shift = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        let mut state = State::default();
        run(&editor, &mut state, Event::Mouse(MOVE), 100.0, 100.0);
        run(&editor, &mut state, Event::Mouse(PRESS), 100.0, 100.0);
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::SHIFT),
            100.0,
            100.0,
        );
        run(&editor, &mut state, Event::Mouse(MOVE), 60.0, 343.0);

        let on_diagonal = |guide: &Guide| match guide.shape(&doc, Camera::default()) {
            Shape::Line(o, d) => {
                let length = d.x.hypot(d.y);
                (o.x + o.y - 400.0).abs() < 1e-3 && (d.x + d.y).abs() < 1e-3 * length
            }
            Shape::Circle(..) => false,
        };
        assert!(!state.guides.iter().any(on_diagonal), "{:?}", state.guides);
        assert!(
            !state
                .lined
                .is_some_and(|lined| lined.guides.iter().flatten().any(on_diagonal)),
            "{:?}",
            state.lined
        );
    }

    #[test]
    fn a_dragged_vertex_snaps_onto_the_mirror_line() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let mirror = Mirror {
            a: Point::new(320.0, 0.0),
            b: Point::new(320.0, 600.0),
        };
        let mirrors = [mirror];
        editor.mirrors = &mirrors;
        let edits = drag(
            &editor,
            &[
                (MOVE, 300.0, 300.0),
                (PRESS, 300.0, 300.0),
                (MOVE, 315.0, 290.0),
                (RELEASE, 315.0, 290.0),
            ],
        );
        let [edits] = &edits[..] else {
            panic!("{edits:?}");
        };
        let [Edit::MoveVertex { id: 1, to }] = edits[..] else {
            panic!("{edits:?}");
        };
        assert!(
            (to.x - 320.0).abs() < 1e-3 && (to.y - 290.0).abs() < 1e-3,
            "{to:?}"
        );

        // Applied, the two halves share it.
        let mut doc = doc.clone();
        assert!(doc.apply(edits[0].clone()));
        assert!(doc.apply(Edit::Mirror { mirror }));
        assert_eq!(doc.vertex_count(), 5);
    }

    #[test]
    fn the_drawing_is_kept_clear_of_its_mirror_image() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let mirrors = [Mirror {
            a: Point::new(320.0, 0.0),
            b: Point::new(320.0, 600.0),
        }];
        editor.mirrors = &mirrors;
        // Dragging corner 1 (300, 300) over the mirror line at x = 320.
        let edits = drag(
            &editor,
            &[
                (MOVE, 300.0, 300.0),
                (PRESS, 300.0, 300.0),
                (MOVE, 360.0, 300.0),
                (RELEASE, 360.0, 300.0),
            ],
        );
        for edit in edits.iter().flatten() {
            if let Edit::MoveVertex { to, .. } = edit {
                assert!(to.x <= 320.0, "{edits:?}");
            }
        }
    }

    #[test]
    fn shift_click_and_another_lasso_add_to_the_selection() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        let shift = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));
        lasso_right(&editor, &mut state);

        // Shift+click adds vertex 0, then removes vertex 3; missing a vertex
        // leaves the selection be.
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::SHIFT),
            0.0,
            0.0,
        );
        for (x, y) in [(100.0, 300.0), (250.0, 300.0), (500.0, 500.0)] {
            run(&editor, &mut state, Event::Mouse(PRESS), x, y);
            run(&editor, &mut state, Event::Mouse(RELEASE), x, y);
        }
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::empty()),
            0.0,
            0.0,
        );
        let mut selected = state.selection.clone();
        selected.sort_unstable();
        assert_eq!(selected, [0, 4, 5]);

        // Lassoing the right triangle again adds 3 back.
        lasso_right(&editor, &mut state);
        assert_eq!(state.selection.len(), 4);
    }

    #[test]
    fn d_removes_the_selected_vertices() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        let shift = |m| Event::Keyboard(keyboard::Event::ModifiersChanged(m));

        // Corner 0 of the left triangle and 4 of the right one: both go,
        // whatever's under the cursor.
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::SHIFT),
            0.0,
            0.0,
        );
        for (x, y) in [(100.0, 300.0), (350.0, 300.0)] {
            run(&editor, &mut state, Event::Mouse(PRESS), x, y);
            run(&editor, &mut state, Event::Mouse(RELEASE), x, y);
        }
        run(
            &editor,
            &mut state,
            shift(keyboard::Modifiers::empty()),
            0.0,
            0.0,
        );
        let edits = run(&editor, &mut state, key('d'), 600.0, 500.0);
        assert_eq!(
            edits,
            vec![vec![
                Edit::RemoveTriangle { triangle: 1 },
                Edit::RemoveTriangle { triangle: 0 },
            ]]
        );
        let mut after = doc.clone();
        assert!(after.apply_all(&edits[0]));
        assert!(after.is_empty());
    }

    #[test]
    fn copied_faces_are_pasted_where_they_fit() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let ctrl = |c: char| {
            let key = keyboard::Key::Character(c.to_string().into());
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::CTRL,
                text: None,
                repeat: false,
            })
        };
        let mut state = State::default();
        lasso_right(&editor, &mut state);
        let cursor = mouse::Cursor::Available(Point::new(300.0, 250.0));
        let copied = editor
            .update(&mut state, &ctrl('c'), bounds, cursor)
            .and_then(|action| action.into_inner().0);
        let Some(Message::Copy(piece)) = copied else {
            panic!("{copied:?}");
        };
        assert_eq!(piece.triangles.len(), 1);

        editor.clipboard = Some(&piece);
        let mut state = State::default();
        // Over the original (in its middle): it doesn't fit, a click does
        // nothing.
        run(&editor, &mut state, ctrl('v'), 300.0, 250.0);
        assert!(matches!(state.interaction, Interaction::Pasting { .. }));
        assert!(run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 250.0).is_empty());
        assert!(matches!(state.interaction, Interaction::Pasting { .. }));

        // Below it, clear: put down there, and selected.
        run(&editor, &mut state, Event::Mouse(MOVE), 300.0, 450.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 300.0, 450.0);
        assert_eq!(
            edits,
            vec![vec![Edit::Paste {
                piece: piece.moved(Vector::new(0.0, 200.0))
            }]]
        );
        assert!(matches!(state.interaction, Interaction::Idle));
        assert_eq!(state.selection.len(), 3);
        let mut after = doc.clone();
        assert!(after.apply_all(&edits[0]));
        assert_eq!(after.triangle_ids().len(), 3);
    }

    #[test]
    fn a_paste_snaps_to_join_up_and_a_cut_takes_the_faces_away() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let ctrl = |c: char| {
            let key = keyboard::Key::Character(c.to_string().into());
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::CTRL,
                text: None,
                repeat: false,
            })
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        lasso_right(&editor, &mut state);
        let cursor = mouse::Cursor::Available(Point::new(600.0, 500.0));
        let cut = editor
            .update(&mut state, &ctrl('x'), bounds, cursor)
            .and_then(|action| action.into_inner().0);
        let Some(Message::Cut { piece, edits, .. }) = cut else {
            panic!("{cut:?}");
        };
        assert_eq!(piece.triangles.len(), 1);
        assert_eq!(edits, [Edit::RemoveTriangle { triangle: 1 }]);

        // The right triangle (corners 250..350 along y = 300), pasted so its
        // left corner comes within a few px of corner 1 (200, 300) of the
        // left one: snapped onto it, sharing it.
        let mut after = doc.clone();
        assert!(after.apply_all(&edits));
        let mut editor = super::tests::editor(&after, &cache);
        editor.clipboard = Some(&piece);
        let mut state = State::default();
        run(&editor, &mut state, ctrl('v'), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 254.0, 253.0);
        let edits = run(&editor, &mut state, Event::Mouse(PRESS), 254.0, 253.0);
        assert_eq!(
            edits,
            vec![vec![Edit::Paste {
                piece: piece.moved(Vector::new(-50.0, 0.0))
            }]]
        );
    }

    #[test]
    fn escape_cancels_a_grab_and_then_the_selection() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let mut state = State::default();
        lasso_right(&editor, &mut state);
        let escape = || {
            let key = keyboard::Key::Named(keyboard::key::Named::Escape);
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            })
        };

        run(&editor, &mut state, key('g'), 300.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 310.0, 270.0);
        run(&editor, &mut state, escape(), 310.0, 270.0);
        assert!(matches!(state.interaction, Interaction::Idle));
        assert!(state.pending.is_none());
        assert_eq!(state.selection.len(), 3);
        run(&editor, &mut state, escape(), 310.0, 270.0);
        assert!(state.selection.is_empty());
    }

    #[test]
    fn painting_paints_the_faces_dragged_over_and_nothing_else() {
        let doc = two_apart();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let brush = Brush::default();
        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush,
            width: 1.0,
            picking: false,
        };
        let color = brush.color();

        // From inside the first, over the gap (fast), into the second.
        let edits = drag(
            &editor,
            &[
                (MOVE, 150.0, 280.0),
                (PRESS, 150.0, 280.0),
                (MOVE, 300.0, 280.0),
                (RELEASE, 300.0, 280.0),
            ],
        );
        assert_eq!(
            edits,
            vec![vec![
                Edit::Paint { triangle: 0, color },
                Edit::Paint { triangle: 1, color },
            ]]
        );

        // Dragging a corner or an edge doesn't change the shape.
        let edits = drag(
            &editor,
            &[
                (MOVE, 100.0, 300.0),
                (PRESS, 100.0, 300.0),
                (MOVE, 50.0, 350.0),
                (RELEASE, 50.0, 350.0),
            ],
        );
        assert_eq!(edits, vec![vec![Edit::Paint { triangle: 0, color }]]);
    }

    #[test]
    fn painting_edges_paints_only_edges_and_faces_only_faces() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let brush = Brush::default();
        let style = EdgeStyle {
            color: brush.color().unwrap(),
            width: 2.0,
        };
        // From the bottom edge, through the face, over the right edge.
        let stroke = [
            (MOVE, 200.0, 300.0),
            (PRESS, 200.0, 300.0),
            (MOVE, 200.0, 250.0),
            (MOVE, 280.0, 250.0),
            (RELEASE, 280.0, 250.0),
        ];

        editor.tool = Tool::Paint {
            target: paint::Target::Edges,
            brush,
            width: 2.0,
            picking: false,
        };
        let edits = drag(&editor, &stroke);
        let edge = |a, b| Edit::PaintEdge {
            a,
            b,
            style: Some(style),
        };
        assert_eq!(edits, vec![vec![edge(0, 1), edge(1, 2)]]);

        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush,
            width: 2.0,
            picking: false,
        };
        let edits = drag(&editor, &stroke);
        assert_eq!(
            edits,
            vec![vec![Edit::Paint {
                triangle: 0,
                color: brush.color()
            }]]
        );
    }

    #[test]
    fn ctrl_click_picks_instead_of_painting() {
        let mut doc = two_apart();
        let red = Color::from_rgb8(255, 0, 0);
        assert!(doc.apply(Edit::Paint {
            triangle: 1,
            color: Some(red),
        }));
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush: Brush::default(),
            width: 1.0,
            picking: false,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let click = |state: &mut State, x: f32, y: f32| {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            let mut out = vec![];
            for e in [PRESS, RELEASE] {
                if let Some(action) = editor.update(state, &Event::Mouse(e), bounds, cursor) {
                    out.extend(action.into_inner().0);
                }
            }
            out
        };

        let mut state = State::default();
        let _ = editor.update(
            &mut state,
            &Event::Keyboard(keyboard::Event::ModifiersChanged(keyboard::Modifiers::CTRL)),
            bounds,
            mouse::Cursor::Unavailable,
        );
        // The painted face, an unpainted one, and nothing.
        let picked = |messages: Vec<Message>| match &messages[..] {
            [Message::Pick(picked)] => Some(*picked),
            [] => None,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            picked(click(&mut state, 300.0, 280.0)),
            Some(Picked::Face(Some(red)))
        );
        assert_eq!(
            picked(click(&mut state, 150.0, 280.0)),
            Some(Picked::Face(None))
        );
        assert_eq!(picked(click(&mut state, 500.0, 500.0)), None);

        // Without Ctrl it paints again.
        let mut state = State::default();
        assert!(matches!(
            &click(&mut state, 150.0, 280.0)[..],
            [Message::Edit { .. }]
        ));
    }

    #[test]
    fn alt_and_moving_tweaks_the_edge_width() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let run = |editor: &Editor, state: &mut State, event: Event, x: f32| {
            let cursor = mouse::Cursor::Available(Point::new(x, 300.0));
            editor
                .update(state, &event, bounds, cursor)
                .map(|action| action.into_inner().0)
        };
        let alt = |modifiers| Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers));

        for (target, tweaks) in [(paint::Target::Edges, true), (paint::Target::Faces, false)] {
            editor.tool = Tool::Paint {
                target,
                brush: Brush::default(),
                width: 2.0,
                picking: false,
            };
            let mut state = State::default();
            let _ = run(&editor, &mut state, alt(keyboard::Modifiers::ALT), 200.0);
            let moved = run(&editor, &mut state, Event::Mouse(MOVE), 230.0).flatten();
            match moved {
                Some(Message::TweakWidth(across)) => {
                    assert!(tweaks);
                    assert_eq!(across, 30.0);
                    // The edge previewed is still looked for where Alt was
                    // pressed.
                    assert_eq!(state.tweaking.unwrap().at, Point::new(200.0, 300.0));
                }
                _ => assert!(!tweaks, "{target:?}: {moved:?}"),
            }
            // Let go: back to following the mouse.
            let _ = run(
                &editor,
                &mut state,
                alt(keyboard::Modifiers::empty()),
                230.0,
            );
            assert!(state.tweaking.is_none());
        }
    }

    #[test]
    fn a_drag_is_worked_out_once_a_frame() {
        let doc = one();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        let send = |state: &mut State, event: Event, x: f32, y: f32| {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            editor.update(state, &event, bounds, cursor);
        };

        // Grab corner 2 (200, 100), and move it in many small steps
        // within one frame, as a fast mouse reports.
        send(&mut state, Event::Mouse(MOVE), 200.0, 100.0);
        send(&mut state, Event::Mouse(PRESS), 200.0, 100.0);
        for i in 1..=500 {
            send(
                &mut state,
                Event::Mouse(MOVE),
                200.0,
                100.0 - i as f32 * 0.1,
            );
        }
        // Nothing worked out yet (the preview is still where it was
        // grabbed): just where it's aimed.
        let at = state.pending.as_ref().map(|pending| pending.at);
        assert_eq!(at, Some(Point::new(200.0, 100.0)));
        assert_eq!(state.aim, Some(Point::new(200.0, 50.0)));

        // The frame: once, for where the mouse is now.
        send(
            &mut state,
            Event::Window(window::Event::RedrawRequested(Instant::now())),
            200.0,
            50.0,
        );
        assert!(state.aim.is_none());
        let pending = state.pending.as_ref().expect("worked out");
        assert!(pending.at.distance(Point::new(200.0, 50.0)) < 1e-3);
    }

    #[test]
    fn c_and_moving_tweaks_the_lightness() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let run = |editor: &Editor, state: &mut State, event: Event, x: f32| {
            let cursor = mouse::Cursor::Available(Point::new(x, 250.0));
            editor
                .update(state, &event, bounds, cursor)
                .and_then(|action| action.into_inner().0)
        };
        let c = keyboard::Key::Character("c".into());
        let physical = keyboard::key::Physical::Code(keyboard::key::Code::KeyC);
        let press = Event::Keyboard(keyboard::Event::KeyPressed {
            key: c.clone(),
            modified_key: c.clone(),
            physical_key: physical,
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        });
        let release = Event::Keyboard(keyboard::Event::KeyReleased {
            key: c.clone(),
            modified_key: c,
            physical_key: physical,
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
        });

        for (brush, tweaks) in [(Brush::default(), true), (Brush::Eraser, false)] {
            editor.tool = Tool::Paint {
                target: paint::Target::Faces,
                brush,
                width: 2.0,
                picking: false,
            };
            let mut state = State::default();
            run(&editor, &mut state, press.clone(), 200.0);
            let moved = run(&editor, &mut state, Event::Mouse(MOVE), 180.0);
            match moved {
                Some(Message::TweakLightness(across)) => {
                    assert!(tweaks);
                    assert_eq!(across, -20.0);
                    // The face previewed stays the one C was pressed over.
                    assert_eq!(state.tweaking.unwrap().at, Point::new(200.0, 250.0));
                }
                _ => assert!(!tweaks, "{brush:?}: {moved:?}"),
            }
            run(&editor, &mut state, release.clone(), 180.0);
            assert!(state.tweaking.is_none());
        }
    }

    #[test]
    fn a_click_while_adjusting_paints_what_is_previewed() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let run = |editor: &Editor, state: &mut State, event: Event, x: f32, y: f32| {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            editor
                .update(state, &event, bounds, cursor)
                .and_then(|action| action.into_inner().0)
        };
        let alt = |modifiers| Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers));
        let c = keyboard::Key::Character("c".into());
        let press_c = Event::Keyboard(keyboard::Event::KeyPressed {
            key: c.clone(),
            modified_key: c,
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyC),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        });
        let brush = Brush::default();

        // Alt over the bottom edge; the mouse wanders off; a click there
        // paints the bottom edge, with the width set.
        editor.tool = Tool::Paint {
            target: paint::Target::Edges,
            brush,
            width: 5.0,
            picking: false,
        };
        let mut state = State::default();
        run(
            &editor,
            &mut state,
            alt(keyboard::Modifiers::ALT),
            200.0,
            300.0,
        );
        run(&editor, &mut state, Event::Mouse(MOVE), 200.0, 450.0);
        let clicked = run(&editor, &mut state, Event::Mouse(PRESS), 200.0, 450.0);
        let style = Some(EdgeStyle {
            color: brush.color().unwrap(),
            width: 5.0,
        });
        assert!(
            matches!(&clicked, Some(Message::Edit { edits, .. })
                if edits[..] == [Edit::PaintEdge { a: 0, b: 1, style }]),
            "{clicked:?}"
        );
        assert!(run(&editor, &mut state, Event::Mouse(RELEASE), 200.0, 450.0).is_none());

        // C over the face; off the drawing; a click paints the face.
        editor.tool = Tool::Paint {
            target: paint::Target::Faces,
            brush,
            width: 5.0,
            picking: false,
        };
        let mut state = State::default();
        run(&editor, &mut state, press_c, 200.0, 250.0);
        run(&editor, &mut state, Event::Mouse(MOVE), 600.0, 500.0);
        let clicked = run(&editor, &mut state, Event::Mouse(PRESS), 600.0, 500.0);
        assert!(
            matches!(&clicked, Some(Message::Edit { edits, .. })
                if edits[..] == [Edit::Paint { triangle: 0, color: brush.color() }]),
            "{clicked:?}"
        );
    }

    #[test]
    fn a_hidden_layer_is_not_edited() {
        let doc = one();
        let cache = Caches::default();
        let mut editor = editor(&doc, &cache);
        editor.shown = false;
        let strokes = [
            // Blank canvas, an edge, a corner.
            [(400.0, 400.0), (450.0, 450.0)],
            [(200.0, 300.0), (200.0, 350.0)],
            [(100.0, 300.0), (50.0, 350.0)],
        ];
        for tool in [
            Tool::Shape,
            Tool::Paint {
                target: paint::Target::Faces,
                brush: Brush::default(),
                width: 1.0,
                picking: false,
            },
        ] {
            editor.tool = tool;
            for [(x0, y0), (x1, y1)] in strokes {
                let edits = drag(
                    &editor,
                    &[
                        (MOVE, x0, y0),
                        (PRESS, x0, y0),
                        (MOVE, x1, y1),
                        (RELEASE, x1, y1),
                    ],
                );
                assert!(edits.is_empty(), "{tool:?}: {edits:?}");
            }
        }
    }

    #[test]
    fn dragging_out_of_a_face_extends_from_the_edge_crossed() {
        let doc = one();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // Out through the bottom edge: as if dragged from that edge.
        let from_face = drag(
            &editor,
            &[
                (MOVE, 200.0, 250.0),
                (PRESS, 200.0, 250.0),
                (MOVE, 200.0, 290.0),
                (MOVE, 200.0, 350.0),
                (RELEASE, 200.0, 350.0),
            ],
        );
        let from_edge = drag(
            &editor,
            &[
                (MOVE, 200.0, 300.0),
                (PRESS, 200.0, 300.0),
                (MOVE, 200.0, 350.0),
                (RELEASE, 200.0, 350.0),
            ],
        );
        assert_eq!(from_face, from_edge);
        assert!(
            matches!(&from_face[..], [edits] if matches!(edits[..], [Edit::AddTriangle { .. }])),
            "{from_face:?}"
        );

        // Out, then back into the face: nothing.
        let back = drag(
            &editor,
            &[
                (MOVE, 200.0, 250.0),
                (PRESS, 200.0, 250.0),
                (MOVE, 200.0, 350.0),
                (MOVE, 200.0, 250.0),
                (RELEASE, 200.0, 250.0),
            ],
        );
        assert!(back.is_empty(), "{back:?}");
    }

    #[test]
    fn changing_direction_mid_drag() {
        let doc = one();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // Inwards, back over the source edge, then out the other side.
        let mut events = vec![(MOVE, 200.0, 300.0), (PRESS, 200.0, 300.0)];
        for y in [290.0, 250.0, 200.0, 250.0, 299.0, 330.0, 350.0] {
            events.push((MOVE, 200.0, y));
        }
        events.push((RELEASE, 200.0, 350.0));

        let edits = drag(&editor, &events);
        assert!(
            matches!(&edits[..], [edits] if matches!(edits[..], [Edit::AddTriangle { .. }])),
            "{edits:?}"
        );
    }

    #[test]
    fn hovering_inside_a_triangle_finds_the_face() {
        let doc = one();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        assert!(matches!(
            editor.hit_test(Point::new(200.0, 250.0)),
            Some(Hover::Face(0))
        ));
        assert!(matches!(
            editor.hit_test(Point::new(200.0, 302.0)),
            Some(Hover::Edge { .. })
        ));
        assert!(editor.hit_test(Point::new(500.0, 500.0)).is_none());
    }

    #[test]
    fn dragging_a_corner_onto_a_separate_triangle_welds() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // Corner 1 (200, 300) dropped next to corner 3 (250, 300).
        let pending = editor.move_to(1, Point::new(245.0, 302.0)).unwrap();
        assert_eq!(
            pending.edits,
            [Edit::MoveVertex {
                id: 1,
                to: Point::new(250.0, 300.0)
            }]
        );
        assert_eq!(pending.changes.welded, [(1, 3)]);

        // Nothing goes: no triangle squashed, no edge lost.
        assert!(pending.changes.squashed.is_empty());
        assert!(removed_edges(&doc, &pending).is_empty());
        let kept = |v| pending.changes.kept(v);
        assert_eq!(new_triangles(&doc, &pending.result, &kept).count(), 0);
    }

    #[test]
    fn the_hovered_shape_stays_until_another_is_hovered() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        let hover = |editor: &Editor, state: &mut State, x, y| {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            editor.update(state, &Event::Mouse(MOVE), bounds, cursor);
            state.shape.as_ref().map(|shape| shape.triangles.clone())
        };
        let shape = |v| Some(editor.highlight(v).triangles);

        let left = hover(&editor, &mut state, 150.0, 270.0);
        assert_eq!(left, shape(0));
        state.fading = None;
        assert_eq!(hover(&editor, &mut state, 600.0, 500.0), left); // blank canvas
        assert_eq!(hover(&editor, &mut state, 100.0, 300.0), left); // another corner
        assert!(state.fading.is_none(), "same shape: no fade");

        assert_eq!(hover(&editor, &mut state, 300.0, 270.0), shape(3));
        assert!(matches!(state.fading, Some((Some(_), _))), "fades over");

        // After an edit joining the two, the highlight follows (once).
        let mut joined = doc.clone();
        joined.apply(Edit::MoveVertex {
            id: 1,
            to: doc.vertex(3),
        });
        let editor = super::tests::editor(&joined, &cache);
        let triangles = hover(&editor, &mut state, 600.0, 500.0).unwrap();
        assert_eq!(triangles.len(), 2);
    }

    #[test]
    fn a_shape_is_everything_connected_through_vertices() {
        let mut doc = two_apart();
        assert_eq!(doc.connected(0).len(), 1);
        assert_eq!(outline_of(&doc.connected(0)).len(), 3);

        // Welded tip to tip, the two triangles are one shape.
        doc.apply(Edit::MoveVertex {
            id: 1,
            to: doc.vertex(3),
        });
        assert_eq!(doc.connected(0).len(), 2);
        assert_eq!(outline_of(&doc.connected(0)).len(), 6);
    }

    #[test]
    fn extending_onto_a_separate_corner_shows_as_a_join() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // From the right side 1–2 of the left triangle out to (near) the top
        // corner 5 of the right one.
        let pending = editor
            .pending(Interaction::Extending {
                a: 1,
                b: 2,
                start: Point::new(175.0, 250.0),
                apex: Point::new(296.0, 204.0),
            })
            .unwrap();
        assert_eq!(pending.at, doc.vertex(5));
        let (_, rings) = editor.joins(&pending, Some((1, 2)));
        assert_eq!(rings, [5]);
    }

    #[test]
    fn welding_onto_a_neighbour_shows_as_a_join() {
        let mut doc = one();
        doc.apply(Edit::InsertVertex {
            at: Point::new(200.0, 230.0),
        });
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // The centre 3 onto corner 0: it squashes two triangles, but what
        // it lands on is a weld.
        let pending = editor.move_to(3, Point::new(104.0, 297.0)).unwrap();
        assert!(!pending.changes.squashed.is_empty());
        let (_, rings) = editor.joins(&pending, None);
        assert_eq!(rings, [0]);
        assert_eq!(pending.at, doc.vertex(0));
    }

    #[test]
    fn dragging_a_corner_onto_a_separate_outline_edge_joins() {
        let doc = two_apart();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // Corner 1 dropped near the middle of edge 3–5, (275, 250): the
        // other triangle is cut there, joining both at the corner.
        let pending = editor.move_to(1, Point::new(272.0, 250.0)).unwrap();
        assert!(
            pending
                .changes
                .cut
                .iter()
                .any(|&(a, b, w)| (a.min(b), a.max(b), w) == (3, 5, 1)),
            "{:?}",
            pending.changes
        );
        assert_eq!(pending.result.triangle_ids().len(), 3);
        let edges = pending.result.unique_edges();
        assert!(edges.contains(&(1, 3)) && edges.contains(&(1, 5)));
        assert!(removed_edges(&doc, &pending).is_empty());
    }

    #[test]
    fn slivers_are_rejected() {
        let doc = one();
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        let edit = |y| {
            editor
                .pending(Interaction::Extending {
                    a: 0,
                    b: 1,
                    start: Point::new(200.0, 300.0),
                    apex: Point::new(200.0, y),
                })
                .map(|pending| pending.edits)
        };
        assert_eq!(edit(297.0), None); // on the source edge: cancelled
        assert_eq!(edit(303.0), None);
        assert!(matches!(
            edit(250.0).as_deref(),
            Some([Edit::InsertVertex { .. }, Edit::MoveVertex { .. }])
        ));
        assert!(matches!(
            edit(350.0).as_deref(),
            Some([Edit::AddTriangle { .. }])
        ));
    }

    #[test]
    fn dragging_a_centre_out_of_the_outline_stretches_the_shape() {
        let mut doc = one();
        doc.apply(Edit::InsertVertex {
            at: Point::new(200.0, 230.0),
        });
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        let edits = drag(
            &editor,
            &[
                (PRESS, 200.0, 230.0),
                (MOVE, 200.0, 280.0),
                (MOVE, 200.0, 360.0),
                // Far past the edge, off to the side.
                (MOVE, 260.0, 400.0),
                (RELEASE, 260.0, 400.0),
            ],
        );

        // The centre follows the cursor out through the bottom edge 0–1: the
        // triangle on that edge is dropped and the other two stretch along.
        assert_eq!(
            edits,
            [[Edit::MoveVertex {
                id: 3,
                to: Point::new(260.0, 400.0),
            }]]
        );
        assert!(doc.clone().apply_all(&edits[0]));
    }

    #[test]
    fn limited_moves_never_leave_slivers() {
        // A fan around vertex 0 whose outline has a dent at (30, 30), so the
        // area vertex 0 may move in is bounded by some edges' extensions.
        let ring = [
            Point::new(100.0, 0.0),
            Point::new(30.0, 30.0),
            Point::new(0.0, 100.0),
            Point::new(-100.0, 0.0),
            Point::new(0.0, -100.0),
        ]
        .map(|p| p + Vector::new(400.0, 300.0));
        let centre = Point::new(390.0, 290.0);

        // Vertex 0 is the centre, 1..=5 the ring.
        let mut doc = Document::default();
        for (from, to) in [(0, 1), (1, 2), (2, 3), (3, 4), (4, 0)] {
            assert!(doc.apply(Edit::AddTriangle {
                corners: [centre, ring[from], ring[to]],
                snap: 0.0
            }));
        }
        assert_eq!(doc.triangle_ids().len(), 5);

        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        for x in (0..16).map(|i| i as f32 * 50.0) {
            for y in (0..12).map(|i| i as f32 * 50.0) {
                // Without a valid position the vertex stays where it was.
                let Some((_, pending)) = editor.limit_move(0, Point::new(x, y)) else {
                    continue;
                };

                for t in pending
                    .result
                    .triangle_ids()
                    .iter()
                    .filter(|t| t.contains(&0))
                {
                    let h = min_height(t.map(|v| pending.result.vertex(v)));
                    assert!(
                        h >= MIN_THICKNESS,
                        "cursor ({x}, {y}): {:?} leaves height {h}",
                        pending.edits
                    );
                }
            }
        }
    }

    #[test]
    fn a_new_triangle_over_other_geometry_stays_an_extension() {
        let mut doc = one();
        // A separate triangle to the right.
        triangle(&mut doc, [(400.0, 300.0), (500.0, 300.0), (450.0, 200.0)]);
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // From the right side of the first triangle out, then into the
        // other triangle: must not turn into splitting it.
        let edits = drag(
            &editor,
            &[
                (PRESS, 250.0, 200.0),
                (MOVE, 320.0, 220.0),
                (MOVE, 380.0, 260.0),
                (MOVE, 460.0, 270.0),
                (RELEASE, 460.0, 270.0),
            ],
        );

        let [edits] = &edits[..] else {
            panic!("expected one edit, got {edits:?}");
        };
        let (p1, p2) = (doc.vertex(1), doc.vertex(2));
        assert!(
            matches!(edits[..], [Edit::AddTriangle { corners: [a, b, _], .. }]
                if (a, b) == (p1, p2) || (a, b) == (p2, p1)),
            "{edits:?}"
        );
        assert!(doc.clone().apply_all(edits));
    }

    #[test]
    fn moving_close_to_an_edge_line_snaps_onto_it() {
        let mut doc = one();
        doc.apply(Edit::InsertVertex {
            at: Point::new(200.0, 230.0),
        });
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // The centre 3, dragged to just above the line through the bottom
        // edge 0–1, past its end: its triangle on that edge would be a
        // sliver. It snaps onto the line, squashing that triangle, and the
        // mesh rearranges around it.
        for y in [296.0, 298.0, 299.5] {
            let pending = editor
                .pending(Interaction::MovingVertex {
                    id: 3,
                    to: Point::new(400.0, y),
                })
                .unwrap_or_else(|| panic!("moving to y = {y} is refused"));
            assert!(
                matches!(pending.edits[..], [Edit::MoveVertex { id: 3, to }]
                    if (to.y - 300.0).abs() < 1e-3),
                "{:?}",
                pending.edits
            );
            assert!(
                pending
                    .result
                    .triangles()
                    .all(|t| min_height(t) >= MIN_THICKNESS),
                "y = {y}: {:?}",
                pending.result.triangle_ids()
            );
        }
    }

    #[test]
    fn dragging_into_a_triangle_can_go_on_into_its_neighbour() {
        let mut doc = one();
        // A neighbour on the right edge 1–2, with far corner 3.
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(1), doc.vertex(2), Point::new(350.0, 150.0)],
            snap: 0.0,
        });
        let area = |doc: &Document| {
            doc.triangles()
                .map(|[a, b, c]| area2(a, b, c) / 2.0)
                .sum::<f32>()
        };
        let cache = Caches::default();
        let editor = editor(&doc, &cache);

        // From the bottom edge 0–1 into the triangle, then onto the shared
        // edge (squashing that child) or through it (flipping it). Either
        // way the new vertex 4 gets an edge to the neighbour's far corner 3,
        // and the outline stays the same.
        for (apex, squashes) in [
            (Point::new(253.0, 200.0), true),
            (Point::new(290.0, 200.0), false),
        ] {
            let pending = editor
                .pending(Interaction::Extending {
                    a: 0,
                    b: 1,
                    start: Point::new(200.0, 300.0),
                    apex,
                })
                .unwrap_or_else(|| panic!("dragging to {apex:?} is refused"));
            let result = &pending.result;

            assert!(matches!(pending.edits[0], Edit::InsertVertex { .. }));
            assert_eq!(
                !pending.changes.squashed.is_empty(),
                squashes,
                "{:?}",
                pending.changes
            );
            assert_eq!(
                result.triangle_ids().len(),
                4,
                "{:?}",
                result.triangle_ids()
            );
            assert!(result.edges().any(|e| e == (4, 3) || e == (3, 4)));
            assert!((area(result) - area(&doc)).abs() < 0.5);
        }
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant as T;

    /// The candidates tried on all cores give what trying them in turn
    /// (on one thread) does.
    #[test]
    fn parallel_search_finds_what_a_sequential_one_does() {
        let doc = grid(6, 40.0);
        let cache = Caches::default();
        let editor = super::tests::editor(&doc, &cache);
        let one_thread = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let mut moved = 0;
        for v in doc.unique_vertices() {
            let from = doc.vertex(v);
            // Far across the grid, where most positions are blocked.
            for to in [
                Point::new(250.0, 250.0),
                Point::new(-50.0, 120.0),
                Point::new(400.0, 20.0),
            ] {
                let target = from + (to - from) * 0.8;
                let parallel = editor.limit_move(v, target).map(|(p, _)| p);
                let worker = editor.worker();
                let sequential = one_thread.install(|| {
                    let caches = Caches::default();
                    let editor = worker.editor(&caches);
                    editor.limit_move(v, target).map(|(p, _)| p)
                });
                assert_eq!(parallel, sequential, "vertex {v} to {target:?}");
                moved += usize::from(parallel.is_some_and(|p| p != target));
            }
        }
        // Some moves were held back, so candidates were tried.
        assert!(moved > 0);
    }

    fn grid(n: usize, size: f32) -> Document {
        let mut doc = Document::default();
        let p = |i: usize, j: usize| Point::new(50.0 + i as f32 * size, 50.0 + j as f32 * size);
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                for corners in [
                    [p(i, j), p(i + 1, j), p(i, j + 1)],
                    [p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)],
                ] {
                    doc.apply(Edit::AddTriangle { corners, snap: 0.0 });
                }
            }
        }
        doc
    }

    fn time<R>(label: &str, runs: u32, mut f: impl FnMut() -> R) {
        let start = T::now();
        for _ in 0..runs {
            std::hint::black_box(f());
        }
        println!("BENCH {label}: {:?}", start.elapsed() / runs);
    }

    /// Drags each vertex of a real drawing (the layer with most triangles
    /// of the file in `TESSERA_BENCH`) across its shape, timing each mouse
    /// move's work without the time budget. Run with
    /// `TESSERA_BENCH=file.tessera cargo test bench_file --release -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_file() {
        let Ok(path) = std::env::var("TESSERA_BENCH") else {
            return;
        };
        let contents = crate::file::open(&std::fs::read_to_string(path).unwrap()).unwrap();
        let layers = contents.layers.layers();
        let doc = &layers
            .iter()
            .max_by_key(|layer| layer.document.triangle_ids().len())
            .unwrap()
            .document;
        let cache = Caches::default();
        let mut editor = super::tests::editor(doc, &cache);
        editor.camera = contents.camera;
        let (min, max) = doc.bounds().unwrap();
        let centre = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        println!(
            "BENCH file: {} vertices, {} triangles",
            doc.vertex_count(),
            doc.triangle_ids().len()
        );

        let mut all = Vec::new();
        for v in doc.unique_vertices() {
            let from = doc.vertex(v);
            // Across the shape: through its middle, and as far out again.
            let to = centre + (centre - from);
            for step in 1..=40 {
                let p = from + (to - from) * (step as f32 / 40.0);
                let start = std::time::Instant::now();
                editor.limit_move(v, p);
                all.push((start.elapsed(), v, step));
            }
        }
        use canvas::Program;
        // And as the canvas gets it: pressing on each vertex, dragging.
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(1000.0, 800.0));
        let mut updates = Vec::new();
        for v in doc.unique_vertices() {
            let mut state = State::default();
            let from = editor.camera.to_screen(doc.vertex(v));
            let to = editor.camera.to_screen(centre + (centre - doc.vertex(v)));
            // Each move and the frame after it.
            let mut send = |event: mouse::Event, p: Point| {
                let cursor = mouse::Cursor::Available(p);
                let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
                let start = std::time::Instant::now();
                editor.update(&mut state, &Event::Mouse(event), bounds, cursor);
                editor.update(&mut state, &frame, bounds, cursor);
                start.elapsed()
            };
            send(mouse::Event::CursorMoved { position: from }, from);
            send(mouse::Event::ButtonPressed(mouse::Button::Left), from);
            for step in 1..=40 {
                let p = from + (to - from) * (step as f32 / 40.0);
                updates.push(send(mouse::Event::CursorMoved { position: p }, p));
            }
            send(mouse::Event::ButtonReleased(mouse::Button::Left), to);
        }
        updates.sort();
        let total_updates: Duration = updates.iter().sum();
        println!(
            "BENCH file: update mean {:?}, p90 {:?}, max {:?}",
            total_updates / updates.len() as u32,
            updates[updates.len() * 9 / 10],
            updates.last().unwrap()
        );

        all.sort();
        let total: Duration = all.iter().map(|(d, ..)| *d).sum();
        println!(
            "BENCH file: {} moves, mean {:?}",
            all.len(),
            total / all.len() as u32
        );
        for q in [0.5, 0.9, 0.99] {
            println!(
                "BENCH file: p{} {:?}",
                q * 100.0,
                all[(all.len() as f32 * q) as usize].0
            );
        }
        for (d, v, step) in all.iter().rev().take(5) {
            println!("BENCH file: slowest {d:?} (vertex {v}, step {step})");
        }
    }

    /// Times one mouse move's worth of work (as `update` does it) on a
    /// 10×10 grid, at normal zoom and zoomed far out, without and with the
    /// time budget. Run with `cargo test bench -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_editor() {
        let doc = grid(10, 40.0);
        let cache = Caches::default();
        let at = |x: f32, y: f32| {
            doc.unique_vertices()
                .into_iter()
                .find(|&v| doc.vertex(v) == Point::new(x, y))
                .unwrap()
        };
        let centre = at(210.0, 210.0);
        let corner = at(410.0, 410.0);
        let (a, b) = doc
            .unique_edges()
            .into_iter()
            .find(|&(u, v)| doc.vertex(u).x == 410.0 && doc.vertex(v).x == 410.0)
            .unwrap();
        let start = doc.vertex(a);

        for (label, zoom) in [("near", 1.0), ("far", 0.1)] {
            for budget in [None, Some(BUDGET)] {
                let editor = Editor {
                    document: &doc,
                    current: 0,
                    crossfade: Crossfade::default(),
                    show_edges: true,
                    mirrors: &[],
                    shown: true,
                    camera: Camera {
                        zoom,
                        ..Camera::default()
                    },
                    below: Vec::new(),
                    above: Vec::new(),
                    caches: &cache,
                    background: None,
                    tool: Tool::Shape,
                    proportional: None,
                    clipboard: None,
                    deadline: Cell::new(None),
                };
                let run = |f: &dyn Fn(&Editor)| {
                    editor
                        .deadline
                        .set(budget.map(|budget| std::time::Instant::now() + budget));
                    f(&editor);
                };
                let label = format!(
                    "{label} {}",
                    if budget.is_some() {
                        "budget"
                    } else {
                        "unbounded"
                    }
                );
                let far = zoom < 1.0;
                let scale = if far { 10.0 } else { 1.0 };

                time(&format!("{label}: small move"), 5, || {
                    run(&|e| {
                        e.limit_move(centre, Point::new(215.0, 205.0));
                    })
                });
                time(&format!("{label}: blocked move"), 3, || {
                    run(&|e| {
                        e.limit_move(centre, Point::new(600.0 * scale, 600.0 * scale));
                    })
                });
                time(&format!("{label}: corner outward"), 5, || {
                    run(&|e| {
                        e.limit_move(corner, Point::new(450.0, 450.0));
                    })
                });
                time(&format!("{label}: extend over crowded"), 3, || {
                    run(&|e| {
                        e.limit_extend(
                            a,
                            b,
                            start,
                            Point::new(470.0 * scale.sqrt(), 600.0 * scale.sqrt()),
                        );
                    })
                });
            }
        }
    }
}
