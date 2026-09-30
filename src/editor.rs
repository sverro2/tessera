//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns mouse input into [`Message`]s. It never mutates the document itself.
//!
//! In the shape mode, left-drag on blank canvas creates a triangle; from
//! inside a face, it
//! extends from the last edge the cursor crossed. Only middle-drag pans; the
//! wheel zooms around the cursor, Shift+wheel rotates around it. C cuts the
//! hovered edge at the cursor; D removes the triangle under it.
//!
//! In the paint mode, clicking a face or dragging over faces paints them
//! with the brush; the shape can't be changed.
//!
//! A dragged point snaps onto nearby vertices and edges (see
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
use crate::document::EdgeStyle;
use crate::document::{Changes, Document, Edit, TriangleId, VertexId, opposite};
use crate::fade;
use crate::geometry::{area2, closest_on_line, closest_on_segment, min_height};
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
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerKey {
    revision: u64,
    look: Look,
    crossfade: Crossfade,
    show_edges: bool,
    camera: (Vector, f32, f32),
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
        shown,
        below,
        above,
        camera,
        caches,
        background,
        tool,
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
    shown: bool,
    camera: Camera,
    background: Option<&'a Background>,
    tool: Tool,
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
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches,
            background: self.background,
            tool: self.tool,
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
    /// Holding Alt while painting edges: where the edge previewed meanwhile
    /// is looked for, and where the mouse last was (screen).
    tweaking: Option<(Point, Point)>,
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

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                // Holding Alt over the canvas, painting edges, tweaks the
                // stroke width; the brush stays where it is meanwhile.
                state.tweaking = match (state.tweaking, inside) {
                    _ if !(modifiers.alt() && self.paints_edges()) => None,
                    (Some(tweaking), _) => Some(tweaking),
                    (None, Some(pos)) => Some((pos, pos)),
                    (None, None) => None,
                };
                // Holding Ctrl turns the brush into the pipette.
                matches!(self.tool, Tool::Paint { .. }).then(canvas::Action::request_redraw)
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

                let edit = match key.to_latin(*physical_key)? {
                    // C cuts the hovered edge where the cursor is.
                    'c' => match self.hit_test(pos)? {
                        Hover::Edge { at, .. } => Edit::InsertVertex { at },
                        Hover::Vertex(_) | Hover::Face(_) => return None,
                    },
                    // D removes the triangle under the cursor.
                    'd' => Edit::RemoveTriangle {
                        triangle: self.document.triangle_at(self.camera.to_world(pos))?,
                    },
                    _ => return None,
                };
                let edits = self.check(vec![edit], self.camera.to_world(pos))?.edits;

                let revision = self.document.revision();
                Some(canvas::Action::publish(Message::Edit { edits, revision }).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let pos = inside?;
                let world = self.camera.to_world(pos);
                if *button == mouse::Button::Left && !self.editable() {
                    return None;
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
                    mouse::Button::Left if matches!(self.tool, Tool::Paint { .. }) => {
                        state.stroke.clear();
                        state.edge_stroke.clear();
                        self.paint_at(state, pos);
                        Interaction::Painting { last: world }
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

                if let (Some((_, last)), Interaction::Idle) =
                    (&mut state.tweaking, state.interaction)
                {
                    let across = pos.x - last.x;
                    *last = pos;
                    return Some(
                        canvas::Action::publish(Message::TweakWidth(across)).and_capture(),
                    );
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
                    _ => {}
                }

                // Worked out when the frame is drawn (see `follow`).
                state.aim = Some(world);
                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left | mouse::Button::Middle,
            )) => {
                // Where it was let go, if not worked out yet.
                if let Some(world) = state.aim.take() {
                    self.follow(state, world);
                }
                let finished = std::mem::take(&mut state.interaction);
                let pending = state.pending.take();

                if let Interaction::Idle = finished {
                    return None;
                }
                if let Interaction::Painting { .. } = finished {
                    let faces = std::mem::take(&mut state.stroke);
                    let edges = std::mem::take(&mut state.edge_stroke);
                    // The tool changed mid-stroke: dropped.
                    let Tool::Paint {
                        target,
                        brush,
                        width,
                        ..
                    } = self.tool
                    else {
                        return Some(canvas::Action::request_redraw().and_capture());
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
                        return Some(canvas::Action::request_redraw().and_capture());
                    }
                    let revision = self.document.revision();
                    return Some(
                        canvas::Action::publish(Message::Edit { edits, revision }).and_capture(),
                    );
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
        self.draw_scene(state, renderer, bounds, cursor)
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
    fn draw_scene(
        &self,
        state: &State,
        renderer: &Renderer,
        bounds: Rectangle,
        cursor: mouse::Cursor,
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
            self.draw_cached(renderer, size, *layer, others, &mut layers);
        }
        let mut above = Vec::new();
        for layer in &self.above {
            self.draw_cached(renderer, size, *layer, others, &mut above);
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

        match (state.interaction, &pending) {
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

    /// Where a drag to `world` gets to, and what releasing there does. If
    /// nothing works out (in time), it stays where it was.
    fn follow(&self, state: &mut State, world: Point) {
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        match &mut state.interaction {
            Interaction::Creating { to, .. } => {
                *to = world;
                state.pending = self.pending(state.interaction);
            }
            Interaction::MovingVertex { id, to } => {
                if let Some((p, pending)) = self.limit_move(*id, world) {
                    *to = p;
                    state.pending = Some(pending);
                }
            }
            Interaction::Extending { a, b, start, apex } => {
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
                        if let Some((p, pending)) = self.limit_extend(a, b, start, world) {
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
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. } => {}
        }
        self.deadline.set(None);
    }

    /// Finds what is under a screen position: a vertex, else an edge, else
    /// a face.
    fn hit_test(&self, screen: Point) -> Option<Hover> {
        match self.snaps(screen, |_| false, |_, _| false, &[]).first() {
            Some(&(Target::Vertex(id), _)) => Some(Hover::Vertex(id)),
            Some(&(Target::Edge(a, b), at)) => Some(Hover::Edge { a, b, at }),
            None => self
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
    /// grab range, then the closest points on edges in range. `skip_vertex`
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

        vertices.sort_by(|x, y| x.2.total_cmp(&y.2));
        edges.sort_by(|x, y| x.2.total_cmp(&y.2));
        vertices
            .into_iter()
            .chain(edges)
            .map(|(target, point, _)| (target, point))
            .collect()
    }

    /// What releasing `interaction` now would do. `None` if nothing.
    fn pending(&self, interaction: Interaction) -> Option<Pending> {
        match interaction {
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. } => None,
            Interaction::Creating { from, to } => self.check(
                vec![Edit::AddTriangle {
                    corners: equilateral(from, to),
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
        document.apply(insert).then(|| {
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
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches: self.caches,
            background: self.background,
            tool: self.tool,
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
            shown: self.shown,
            camera: self.camera,
            background: self.background,
            tool: self.tool,
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
            Tool::Shape | Tool::Background { painted: false } => Look::Plain,
        }
    }

    /// Draws the current layer, or what it's about to become.
    fn draw_document(&self, frame: &mut Frame, document: &Document) {
        self.draw_layer(frame, document, self.look(), Crossfade::default(), true);
    }

    /// Draws a layer; in the painted look crossfaded as `crossfade` says,
    /// and its edges only if `show_edges`.
    fn draw_layer(
        &self,
        frame: &mut Frame,
        document: &Document,
        look: Look,
        crossfade: Crossfade,
        show_edges: bool,
    ) {
        match look {
            Look::Painted => {}
            // Both with a hint of the colours, to still tell what's
            // painted; other layers fainter.
            Look::Plain => {
                self.draw_hinted(frame, document, 1.0, 1.5);
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
            camera: (camera.pan, camera.zoom, camera.rotation),
            size,
        };
        let mut caches = self.caches.layers.borrow_mut();
        let cache = caches.entry(layer.id).or_insert_with(|| LayerCache {
            key: None,
            cache: canvas::Cache::new(),
        });
        if cache.key != Some(key) {
            cache.key = Some(key);
            cache.cache.clear();
        }
        layers.push(cache.cache.draw(renderer, size, |frame| {
            self.draw_layer(frame, layer.document, look, crossfade, show_edges);
        }));
    }

    /// A painted edge's width on screen: never too thin to see.
    fn edge_width(&self, width: f32) -> f32 {
        (width * self.camera.zoom).max(1.0)
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
        self.edge_in(self.document, screen)
    }

    /// Like [`Self::edge_at`], on another layer.
    fn edge_in(&self, document: &Document, screen: Point) -> Option<(VertexId, VertexId)> {
        let at = |v| self.camera.to_screen(document.vertex(v));
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
        let world = self.camera.to_world(screen);
        let mut layers = std::iter::once(self.document)
            .chain(self.above.iter().rev().map(|layer| layer.document))
            .chain(self.below.iter().rev().map(|layer| layer.document));
        match target {
            paint::Target::Faces => layers.find_map(|document| {
                let t = document.triangle_at(world)?;
                Some(Picked::Face(document.color(t)))
            }),
            paint::Target::Edges => layers.find_map(|document| {
                let (a, b) = self.edge_in(document, screen)?;
                Some(Picked::Edge(document.edge_style(a, b)))
            }),
        }
    }

    /// The paint mode's overlay: the stroke so far, what's under the
    /// cursor, and the brush (or pipette) following the cursor.
    fn draw_painting(&self, frame: &mut Frame, state: &State, cursor: Option<Point>) {
        // Tweaking the width: the edge previewed stays the one it was, while
        // the brush follows the mouse.
        let focus = state.tweaking.map(|(at, _)| at).or(cursor);
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
                        frame.fill(&face, Color { a: 0.5, ..color });
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
            // Tweaking: the width, by the brush.
            if state.tweaking.is_some() {
                let label = Point::new(p.x + CURSOR_SIZE * 0.5, p.y - CURSOR_SIZE * 0.9);
                for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
                    frame.fill_text(canvas::Text {
                        content: format!("{width:.1}"),
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
            shown: true,
            below: Vec::new(),
            above: Vec::new(),
            camera: Camera::default(),
            caches,
            background: None,
            tool: Tool::Shape,
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
                    assert_eq!(state.tweaking.unwrap().0, Point::new(200.0, 300.0));
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
