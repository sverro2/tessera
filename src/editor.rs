//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns mouse input into [`Message`]s. It never mutates the document itself.
//!
//! Left-drag on blank canvas creates a triangle. Only middle-drag pans; the
//! wheel zooms around the cursor, Shift+wheel rotates around it. C cuts the
//! hovered edge at the cursor; D removes the triangle under it.
//!
//! A dragged point snaps onto nearby vertices and edges (see
//! [`Editor::snaps`]); the document then joins things up by itself, so the
//! editor only decides where points go.

use iced::keyboard;
use iced::mouse;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{
    self, Canvas, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use iced::window;
use iced::{Color, Element, Fill, Point, Rectangle, Renderer, Theme, Vector};

use crate::camera::Camera;
use crate::document::{Changes, Document, Edit, TriangleId, VertexId, opposite};
use crate::geometry::{area2, closest_on_line, closest_on_segment, min_height};

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
/// How long the shape highlight takes to move over to another shape.
const SHAPE_FADE: Duration = Duration::from_millis(200);
/// World-space spacing of the background dot grid.
const GRID: f32 = 50.0;
/// Rotation per wheel notch with Shift held.
const ROTATE_STEP: f32 = 5.0 * std::f32::consts::PI / 180.0;
/// Radius and margin (screen px) of the compass showing the rotation.
const COMPASS: f32 = 16.0;
const COMPASS_MARGIN: f32 = 12.0;

const BACKGROUND: Color = Color::from_rgb8(0x1a, 0x1b, 0x26);
const GRID_DOT: Color = Color::from_rgb8(0x2f, 0x33, 0x4d);
const FILL: Color = Color::from_rgba8(0x7a, 0xa2, 0xf7, 0.35);
const EDGE: Color = Color::from_rgb8(0xc0, 0xca, 0xf5);
/// Washed over the shapes other than the highlighted one.
const UNFOCUSED: Color = Color::from_rgb8(0x3b, 0x3e, 0x4f);
/// What an action would add (and the cursor that creates triangles).
const ADDED: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
/// What an action would remove.
const REMOVED: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);
/// Where an action joins things up: edges that become shared, vertices
/// welded together (and the cursor, when it lands on such a join).
const JOINED: Color = Color::from_rgb8(0xe0, 0xaf, 0x68);
/// The hovered vertex or edge.
const HOVER: Color = Color::from_rgb8(0x7d, 0xcf, 0xff);

#[derive(Debug, Clone)]
pub enum Message {
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
    /// One user action, made of edits applied together (all or nothing).
    Edit(Vec<Edit>),
}

pub fn view<'a>(
    document: &'a Document,
    camera: Camera,
    cache: &'a canvas::Cache,
) -> Element<'a, Message> {
    Canvas::new(Editor {
        document,
        camera,
        cache,
    })
    .width(Fill)
    .height(Fill)
    .into()
}

struct Editor<'a> {
    document: &'a Document,
    camera: Camera,
    cache: &'a canvas::Cache,
}

#[derive(Debug, Default)]
pub struct State {
    interaction: Interaction,
    /// The wheel event doesn't carry these, so keep track of them.
    modifiers: keyboard::Modifiers,
    /// A vertex of the shape last hovered: it stays highlighted until
    /// another shape is hovered, so it doesn't flicker on and off.
    shape: Option<VertexId>,
    /// The shape highlighted before (if any), fading out since when.
    fading: Option<(Option<VertexId>, Instant)>,
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

/// What releasing the current drag would do.
struct Pending {
    edits: Vec<Edit>,
    /// The document afterwards, and how normalizing it joined things up.
    result: Document,
    changes: Changes,
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

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                None
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if !modifiers.command() && matches!(state.interaction, Interaction::Idle) => {
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

                Some(canvas::Action::publish(Message::Edit(edits)).and_capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let pos = inside?;
                let world = self.camera.to_world(pos);

                state.interaction = match button {
                    mouse::Button::Middle => Interaction::Panning { last: pos },
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
                        Some(Hover::Face(_)) | None => Interaction::Creating {
                            from: world,
                            to: world,
                        },
                    },
                    _ => return None,
                };

                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let pos = screen?;
                let world = self.camera.to_world(pos);

                match &mut state.interaction {
                    Interaction::Idle => {
                        if let Some(hover) = self.hit_test(pos) {
                            let vertex = self.hover_vertex(hover);
                            let same = state.shape.is_some_and(|shape| {
                                self.document
                                    .connected(shape)
                                    .iter()
                                    .flatten()
                                    .any(|&v| v == vertex)
                            });
                            if !same {
                                state.fading = Some((state.shape, Instant::now()));
                            }
                            state.shape = Some(vertex);
                        }
                        return Some(canvas::Action::request_redraw());
                    }
                    Interaction::Panning { last } => {
                        let delta = pos - *last;
                        *last = pos;
                        return Some(canvas::Action::publish(Message::Pan(delta)).and_capture());
                    }
                    Interaction::Creating { to, .. } => *to = world,
                    Interaction::MovingVertex { id, to } => *to = self.limit_move(*id, *to, world),
                    Interaction::Extending { a, b, start, apex } => {
                        *apex = self.limit_extend(*a, *b, *start, *apex, world);
                    }
                }

                Some(canvas::Action::request_redraw().and_capture())
            }
            Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left | mouse::Button::Middle,
            )) => {
                let finished = std::mem::take(&mut state.interaction);

                if let Interaction::Idle = finished {
                    return None;
                }

                Some(
                    match self.pending(finished) {
                        Some(pending) => canvas::Action::publish(Message::Edit(pending.edits)),
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
                    return Some(
                        canvas::Action::publish(Message::Rotate { anchor, angle }).and_capture(),
                    );
                }

                let factor = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => 1.15f32.powf(y),
                    mouse::ScrollDelta::Pixels { y, .. } => (y / 200.0).exp(),
                };

                Some(canvas::Action::publish(Message::Zoom { anchor, factor }).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => Some(canvas::Action::request_redraw()),
            // Keep drawing frames while the shape highlight moves over.
            Event::Window(window::Event::RedrawRequested(now)) => {
                let (_, since) = state.fading?;
                if now.duration_since(since) >= SHAPE_FADE {
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
        let camera = self.camera;

        // While dragging, draw the document as it will be after release.
        let pending = self.pending(state.interaction);
        // Extending always shares its source edge; that's no news.
        let source = match state.interaction {
            Interaction::Extending { a, b, .. } => Some((a, b)),
            _ => None,
        };

        let content = match &pending {
            Some(pending) => {
                let mut frame = Frame::new(renderer, bounds.size());
                self.draw_document(&mut frame, &pending.result);
                self.draw_changes(&mut frame, pending, source);
                frame.into_geometry()
            }
            None => self.cache.draw(renderer, bounds.size(), |frame| {
                self.draw_document(frame, self.document);
            }),
        };

        let mut overlay = Frame::new(renderer, bounds.size());
        draw_compass(&mut overlay, camera);
        let cursor_pos = cursor.position_in(bounds);

        let hover = cursor_pos.and_then(|p| self.hit_test(p));

        // The outline of the shape last hovered, so it's clear what hangs
        // together and what is loose. While dragging, as it will be after
        // release (grown, if the edit joins it to another shape). Moving
        // over to another shape, the old outline fades out as the new one
        // fades in.
        let (document, kept): (&Document, &dyn Fn(VertexId) -> VertexId) = match &pending {
            Some(pending) => (&pending.result, &|v| pending.changes.kept(v)),
            None => (self.document, &|v| v),
        };
        let shown = match state.fading {
            Some((_, since)) => {
                let t = (since.elapsed().as_secs_f32() / SHAPE_FADE.as_secs_f32()).min(1.0);
                1.0 - (1.0 - t).powi(2)
            }
            None => 1.0,
        };
        let previous = state.fading.and_then(|(previous, _)| previous);
        let highlighted = [(previous, 1.0 - shown), (state.shape, shown)]
            .map(|(start, strength)| (start.map(|v| document.connected(kept(v))), strength));

        // Everything else is grayed out, so the highlighted shape stands
        // apart. What the pending edit adds never is.
        if state.shape.is_some() {
            let added: Vec<[VertexId; 3]> = match &pending {
                Some(pending) => {
                    let kept = |v| pending.changes.kept(v);
                    new_triangles(self.document, &pending.result, &kept).collect()
                }
                None => Vec::new(),
            };
            let mut dimmed: Vec<(f32, Vec<[Point; 3]>)> = Vec::new();
            for t in document
                .triangle_ids()
                .iter()
                .filter(|t| !added.contains(t))
            {
                let lit: f32 = highlighted
                    .iter()
                    .filter(|(shape, _)| shape.as_ref().is_some_and(|s| s.contains(t)))
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
                    let gray = Color {
                        a: 0.55 * dim,
                        ..UNFOCUSED
                    };
                    overlay.fill(&wash, gray);
                    overlay.stroke(&wash, stroke(gray, 1.5));
                }
            }
        }

        for (shape, strength) in &highlighted {
            if let Some(shape) = shape {
                self.draw_shape(&mut overlay, document, shape, *strength);
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

        vec![content, overlay.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match state.interaction {
            Interaction::Panning { .. } => mouse::Interaction::Grabbing,
            Interaction::Idle if !cursor.is_over(bounds) => mouse::Interaction::default(),
            // Our own cursor/handles are drawn on the canvas.
            _ => mouse::Interaction::Hidden,
        }
    }
}

impl Editor<'_> {
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
            Interaction::Idle | Interaction::Panning { .. } => None,
            Interaction::Creating { from, to } => self.check(
                vec![Edit::AddTriangle {
                    corners: equilateral(from, to),
                    snap: self.snap_distance(),
                }],
                to,
            ),
            Interaction::MovingVertex { id, to } => self.move_to(id, to),
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
        let (result, changes) = self.document.preview(&edits)?;

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

        (!has_sliver).then_some(Pending {
            edits,
            result,
            changes,
            at,
        })
    }

    /// How close (world units) a new triangle's corners and sides may come
    /// to existing geometry before counting as touching it: what a fill
    /// needs to leave no slivers.
    fn snap_distance(&self) -> f32 {
        MIN_THICKNESS / self.camera.zoom
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
            camera: self.camera,
            cache: self.cache,
        }
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

    /// Where vertex `id` should be when the cursor is at `to`, given that
    /// `from` was valid: the cursor itself if the move is allowed (the mesh
    /// rearranges itself around the vertex where needed), otherwise the
    /// closest allowed point, sliding along the edges around the vertex.
    /// That's the last resort, e.g. when a triangle would be swept over an
    /// unconnected part of the drawing.
    fn limit_move(&self, id: VertexId, from: Point, to: Point) -> Point {
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

        self.closest_valid(&lines, from, to, |p| {
            self.pending(Interaction::MovingVertex { id, to: p })
                .is_some()
        })
    }

    /// Where the new corner of an extension from edge `a`–`b` should be when
    /// the cursor is at `to`, given that `from` was valid. Into a triangle,
    /// the new vertex is limited like any dragged vertex; on an empty side it
    /// slides along the geometry a new triangle would overlap.
    fn limit_extend(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        from: Point,
        to: Point,
    ) -> Point {
        let camera = self.camera;
        let (pa, pb) = (
            camera.to_screen(self.document.vertex(a)),
            camera.to_screen(self.document.vertex(b)),
        );
        let cursor = camera.to_screen(to);

        // On the source edge the drag reads as cancelled, so don't hold the
        // corner back there.
        if closest_on_segment(cursor, pa, pb).1 <= EDGE_HIT {
            return to;
        }

        let lines: Vec<Line> = match self.triangle_beside(a, b, to) {
            // Splitting, then moving the new vertex.
            Some(triangle) => {
                return match self.split_from_edge(a, b, start, triangle) {
                    Some((_, document, id)) => {
                        self.with_document(&document).limit_move(id, from, to)
                    }
                    None => to,
                };
            }
            // New triangle: the fill bends around other geometry rather than
            // leaving slivers, so just stay clear of the source edge.
            None => vec![offset_line(pa, pb, cursor, SLIDE_INSET)],
        };

        self.closest_valid(&lines, from, to, |p| {
            self.pending(Interaction::Extending {
                a,
                b,
                start,
                apex: p,
            })
            .is_some()
        })
    }

    /// The valid position closest to the cursor at `to` (world): the cursor
    /// itself, its projection onto one of `lines` (sliding along a limit) or
    /// where two of them cross (stuck in a corner). Falls back to `from`.
    fn closest_valid(
        &self,
        lines: &[Line],
        from: Point,
        to: Point,
        valid: impl Fn(Point) -> bool,
    ) -> Point {
        if valid(to) {
            return to;
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

        candidates
            .into_iter()
            .map(|(p, _)| p)
            .find(|&p| valid(p))
            .unwrap_or(from)
    }

    fn draw_document(&self, frame: &mut Frame, document: &Document) {
        frame.fill_rectangle(Point::ORIGIN, frame.size(), BACKGROUND);
        draw_grid(frame, self.camera);

        let mesh = self.mesh(document.triangles());
        frame.fill(&mesh, FILL);
        frame.stroke(&mesh, stroke(EDGE, 1.5));
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
        let kept = |v| changes.kept(v);
        let screen = |v| self.camera.to_screen(result.vertex(v));
        let segments = |edges: &[(VertexId, VertexId)]| {
            Path::new(|p| {
                for &(u, v) in edges {
                    p.move_to(screen(u));
                    p.line_to(screen(v));
                }
            })
        };

        let added =
            self.mesh(new_triangles(before, result, &kept).map(|t| t.map(|v| result.vertex(v))));
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

    /// Highlights a `shape` (its triangles): a faint tint, and its
    /// outline with a soft glow. `strength` (0 to 1) fades it.
    fn draw_shape(
        &self,
        frame: &mut Frame,
        document: &Document,
        shape: &[[VertexId; 3]],
        strength: f32,
    ) {
        if strength <= 0.0 {
            return;
        }
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let edges = Path::new(|p| {
            for (u, v) in outline_of(shape) {
                p.move_to(screen(u));
                p.line_to(screen(v));
            }
        });
        let tint = self.mesh(shape.iter().map(|t| t.map(|v| document.vertex(v))));
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

/// A compass in the top right corner whose needle points to world "up".
fn draw_compass(frame: &mut Frame, camera: Camera) {
    let centre = Point::new(
        frame.width() - COMPASS_MARGIN - COMPASS,
        COMPASS_MARGIN + COMPASS,
    );
    let (sin, cos) = camera.rotation.sin_cos();
    let up = Vector::new(sin, -cos) * (COMPASS - 4.0);
    let dial = Path::circle(centre, COMPASS);

    frame.fill(&dial, BACKGROUND);
    frame.stroke(&dial, stroke(GRID_DOT, 1.5));
    let tail = centre - up * 0.6;
    frame.stroke(&Path::line(tail, centre), stroke(GRID_DOT, 2.0));
    frame.stroke(&Path::line(centre, centre + up), stroke(EDGE, 2.0));
    frame.fill(&Path::circle(centre + up, 2.5), EDGE);
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

    fn editor<'a>(doc: &'a Document, cache: &'a canvas::Cache) -> Editor<'a> {
        Editor {
            document: doc,
            camera: Camera::default(),
            cache,
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
            if let Some(action) = editor.update(&mut state, &Event::Mouse(e), bounds, cursor) {
                published.extend(action.into_inner().0);
            }
        }
        published
            .into_iter()
            .filter_map(|m| match m {
                Message::Edit(edits) => Some(edits),
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
    fn changing_direction_mid_drag() {
        let doc = one();
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
        let editor = editor(&doc, &cache);
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        let hover = |state: &mut State, x, y| {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            editor.update(state, &Event::Mouse(MOVE), bounds, cursor);
            state.shape.map(|v| doc.connected(v))
        };

        let left = hover(&mut state, 150.0, 270.0);
        assert_eq!(left, Some(doc.connected(0)));
        state.fading = None;
        assert_eq!(hover(&mut state, 600.0, 500.0), left); // blank canvas
        assert_eq!(hover(&mut state, 100.0, 300.0), left); // another corner
        assert!(state.fading.is_none(), "same shape: no fade");

        assert_eq!(hover(&mut state, 300.0, 270.0), Some(doc.connected(3)));
        assert!(matches!(state.fading, Some((Some(_), _))), "fades over");
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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

        let cache = canvas::Cache::new();
        let editor = editor(&doc, &cache);

        for x in (0..16).map(|i| i as f32 * 50.0) {
            for y in (0..12).map(|i| i as f32 * 50.0) {
                let to = editor.limit_move(0, centre, Point::new(x, y));
                let pending = editor
                    .pending(Interaction::MovingVertex { id: 0, to })
                    .expect("limited position is valid");

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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
        let cache = canvas::Cache::new();
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
