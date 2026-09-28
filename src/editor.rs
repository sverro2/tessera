//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns mouse input into [`Message`]s. It never mutates the document itself.
//!
//! Left-drag on blank canvas:
//! - creates the first triangle while the document is empty,
//! - otherwise pans (hold Shift to create a separate triangle instead).
//!
//! Middle-drag always pans; the wheel zooms around the cursor. C cuts the
//! hovered edge at the cursor.

use iced::keyboard;
use iced::mouse;
use iced::widget::canvas::{
    self, Canvas, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use iced::{Color, Element, Fill, Point, Rectangle, Renderer, Theme, Vector};

use crate::camera::Camera;
use crate::document::{Corner, Document, Edit, TriangleId, VertexId};
use crate::geometry::{area2, closest_on_segment, min_height, overlap};

/// Screen-space distance within which a corner is grabbed.
const VERTEX_HIT: f32 = 10.0;
/// Screen-space distance within which an edge is grabbed.
const EDGE_HIT: f32 = 7.0;
/// Minimum height (screen px) of any triangle we create; thinner slivers
/// would be invisible and impossible to grab.
const MIN_THICKNESS: f32 = 5.0;
/// A triangle whose height is below this fraction of its longest side is
/// considered flat.
const FLAT_RATIO: f32 = 0.02;
/// How far (screen px) inside a limiting edge a dragged vertex stops: just
/// clear of making its triangle with that edge a sliver.
const SLIDE_INSET: f32 = MIN_THICKNESS + 0.5;
/// How far (screen px) past an edge's line a vertex is put when snapped
/// across it.
const ACROSS_NUDGE: f32 = 0.01;
/// World-space spacing of the background dot grid.
const GRID: f32 = 50.0;

const BACKGROUND: Color = Color::from_rgb8(0x1a, 0x1b, 0x26);
const GRID_DOT: Color = Color::from_rgb8(0x2f, 0x33, 0x4d);
const FILL: Color = Color::from_rgba8(0x7a, 0xa2, 0xf7, 0.35);
const EDGE: Color = Color::from_rgb8(0xc0, 0xca, 0xf5);
const CURSOR: Color = Color::from_rgb8(0x9e, 0xce, 0x6a);
const VERTEX_HANDLE: Color = Color::from_rgb8(0xbb, 0x9a, 0xf7);
const EDGE_HANDLE: Color = Color::from_rgb8(0x7d, 0xcf, 0xff);
const CANCEL: Color = Color::from_rgb8(0xf7, 0x76, 0x8e);

#[derive(Debug, Clone)]
pub enum Message {
    Pan(Vector),
    Zoom {
        anchor: Point,
        factor: f32,
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
    modifiers: keyboard::Modifiers,
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
            // C cuts the hovered edge where the cursor is.
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                ..
            }) if key.to_latin(*physical_key) == Some('c')
                && !modifiers.command()
                && matches!(state.interaction, Interaction::Idle) =>
            {
                let Some(Hover::Edge { a, b, at }) = self.hit_test(inside?) else {
                    return None;
                };
                let edit = Edit::SplitEdge { a, b, at };
                let (edits, _) = self.check(vec![edit])?;

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
                        None if self.document.is_empty() || state.modifiers.shift() => {
                            Interaction::Creating {
                                from: world,
                                to: world,
                            }
                        }
                        None => Interaction::Panning { last: pos },
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
                        Some((edits, _)) => canvas::Action::publish(Message::Edit(edits)),
                        None => canvas::Action::request_redraw(),
                    }
                    .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let anchor = inside?;
                let factor = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => 1.15f32.powf(y),
                    mouse::ScrollDelta::Pixels { y, .. } => (y / 200.0).exp(),
                };

                Some(canvas::Action::publish(Message::Zoom { anchor, factor }).and_capture())
            }
            Event::Mouse(mouse::Event::CursorLeft) => Some(canvas::Action::request_redraw()),
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

        let accent = match state.interaction {
            Interaction::Creating { .. } => CURSOR,
            Interaction::MovingVertex { .. } => VERTEX_HANDLE,
            _ => EDGE_HANDLE,
        };

        // While dragging, draw the document as it will be after release.
        let pending = self.pending(state.interaction);

        let content = match &pending {
            Some((_, result)) => {
                let mut frame = Frame::new(renderer, bounds.size());
                self.draw_document(&mut frame, result);
                self.draw_changes(&mut frame, result, accent);
                frame.into_geometry()
            }
            None => self.cache.draw(renderer, bounds.size(), |frame| {
                self.draw_document(frame, self.document);
            }),
        };

        let mut overlay = Frame::new(renderer, bounds.size());
        let cursor_pos = cursor.position_in(bounds);

        match (state.interaction, &pending) {
            (Interaction::Idle, _) => match cursor_pos.map(|p| (p, self.hit_test(p))) {
                Some((_, Some(Hover::Vertex(id)))) => {
                    handle(
                        &mut overlay,
                        camera.to_screen(self.document.vertex(id)),
                        VERTEX_HANDLE,
                    );
                }
                Some((_, Some(Hover::Edge { a, b, at }))) => {
                    let a = camera.to_screen(self.document.vertex(a));
                    let b = camera.to_screen(self.document.vertex(b));
                    overlay.stroke(&Path::line(a, b), stroke(EDGE_HANDLE, 2.5));
                    handle(&mut overlay, camera.to_screen(at), EDGE_HANDLE);
                }
                Some((p, None)) => {
                    overlay.stroke(&Path::circle(p, 6.0), stroke(CURSOR, 2.0));
                    overlay.fill(&Path::circle(p, 1.5), CURSOR);
                }
                None => {}
            },
            (Interaction::Panning { .. }, _) => {}
            (_, Some((edits, result))) => {
                let at = camera.to_screen(placed(edits, result));

                // Stuck away from the cursor (e.g. a vertex held back by an
                // edge): show where the mouse really is, so the jump on
                // release is expected. Small snaps don't count.
                if let Some(p) = cursor_pos.filter(|p| p.distance(at) > VERTEX_HIT) {
                    let dashed = Stroke {
                        line_dash: LineDash {
                            segments: &[4.0, 4.0],
                            offset: 0,
                        },
                        ..stroke(Color { a: 0.5, ..accent }, 1.0)
                    };
                    overlay.stroke(&Path::line(p, at), dashed);
                    overlay.stroke(
                        &Path::circle(p, 4.0),
                        stroke(Color { a: 0.7, ..accent }, 1.5),
                    );
                }

                handle(&mut overlay, at, accent);
            }
            // Releasing now would do nothing.
            (_, None) => {
                if let Some(p) = cursor_pos {
                    handle(&mut overlay, p, CANCEL);
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
    /// Finds the vertex or edge under a screen position. Vertices win.
    fn hit_test(&self, screen: Point) -> Option<Hover> {
        let camera = self.camera;

        if let Some(id) = self.nearest_vertex(screen, self.document.used_vertices()) {
            return Some(Hover::Vertex(id));
        }

        self.document
            .edges()
            .filter_map(|(a, b)| {
                let pa = camera.to_screen(self.document.vertex(a));
                let pb = camera.to_screen(self.document.vertex(b));
                let (closest, d) = closest_on_segment(screen, pa, pb);
                (d <= EDGE_HIT).then(|| {
                    (
                        Hover::Edge {
                            a,
                            b,
                            at: camera.to_world(closest),
                        },
                        d,
                    )
                })
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(hover, _)| hover)
    }

    /// The edits releasing `interaction` now would make, together with the
    /// resulting document. `None` if releasing would do nothing.
    fn pending(&self, interaction: Interaction) -> Option<(Vec<Edit>, Document)> {
        let edits = match interaction {
            Interaction::Idle | Interaction::Panning { .. } => {
                return None;
            }
            Interaction::Creating { from, to } => vec![Edit::AddTriangle(equilateral(from, to))],
            Interaction::MovingVertex { id, to } => vec![self.move_edit(id, to)],
            Interaction::Extending { a, b, start, apex } => {
                // Back on the source edge, the drag reads as cancelled.
                if self.is_near_edge(apex, a, b) {
                    return None;
                }
                self.extend_edits(a, b, start, apex)
            }
        };

        self.check(edits)
    }

    /// Applies `edits` to a copy of the document, if the document accepts
    /// them and they leave no slivers.
    fn check(&self, edits: Vec<Edit>) -> Option<(Vec<Edit>, Document)> {
        let mut result = self.document.clone();
        let applied = result.apply_all(&edits);

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

        (applied && !has_sliver).then_some((edits, result))
    }

    fn is_near_edge(&self, point: Point, a: VertexId, b: VertexId) -> bool {
        let screen = |v| self.camera.to_screen(self.document.vertex(v));
        closest_on_segment(self.camera.to_screen(point), screen(a), screen(b)).1 <= EDGE_HIT
    }

    /// Dragging from edge `a`–`b` (grabbed at `start`) to `apex`. On a side of
    /// the edge that has a triangle, split that triangle and then move the
    /// new vertex to `apex` like any dragged vertex, so it can go on to
    /// squash a child or rearrange the neighbours. On an empty side, add a
    /// triangle whose third corner is a nearby vertex or a new one.
    fn extend_edits(&self, a: VertexId, b: VertexId, start: Point, apex: Point) -> Vec<Edit> {
        if let Some(triangle) = self.triangle_beside(a, b, apex) {
            let Some((split, document, id)) = self.split_from_edge(a, b, start, triangle) else {
                return vec![Edit::SplitTriangle { triangle, at: apex }];
            };
            let then = self.with_document(&document).move_edit(id, apex);
            return vec![split, then];
        }

        let snapped = self.nearest_vertex(
            self.camera.to_screen(apex),
            self.document.used_vertices().filter(|&v| v != a && v != b),
        );

        vec![Edit::ExtendEdge {
            a,
            b,
            apex: snapped.map_or(Corner::New(apex), Corner::Existing),
        }]
    }

    /// Splits `triangle` (on edge `a`–`b`) at a point just inside it from
    /// `start` on the edge: where a drag into it begins. Returns the split,
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
        let split = Edit::SplitTriangle { triangle, at };

        let mut document = self.document.clone();
        document.apply(split).then(|| {
            let id = document.last_vertex();
            (split, document, id)
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

    /// Dragging vertex `id` to `to`: weld onto a neighbouring vertex, snap onto
    /// the opposite edge of one of its triangles, or just move.
    fn move_edit(&self, id: VertexId, to: Point) -> Edit {
        let camera = self.camera;
        let screen = camera.to_screen(to);

        if let Some(into) = self.nearest_vertex(screen, self.document.neighbours(id)) {
            return Edit::MergeVertex { id, into };
        }

        self.document
            .triangle_ids()
            .iter()
            .filter_map(|t| {
                let i = t.iter().position(|&v| v == id)?;
                let (a, b) = (t[(i + 1) % 3], t[(i + 2) % 3]);
                let pa = camera.to_screen(self.document.vertex(a));
                let pb = camera.to_screen(self.document.vertex(b));
                let (closest, d) = closest_on_segment(screen, pa, pb);
                let at = camera.to_world(closest);
                (d <= EDGE_HIT).then_some((Edit::CollapseOntoEdge { id, a, b, at }, d))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(edit, _)| edit)
            .or_else(|| self.move_across_edge_line(id, screen))
            .unwrap_or(Edit::MoveVertex { id, to })
    }

    /// Close to the line through the opposite edge of one of its triangles
    /// (past the edge's ends, as the edge itself snaps), the vertex would
    /// leave that triangle a sliver. Put it just across the line instead, so
    /// the triangles rearrange right away, if that works out.
    fn move_across_edge_line(&self, id: VertexId, screen: Point) -> Option<Edit> {
        let camera = self.camera;

        let (_, line_point) = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|t| {
                let i = t.iter().position(|&v| v == id)?;
                let a = camera.to_screen(self.document.vertex(t[(i + 1) % 3]));
                let b = camera.to_screen(self.document.vertex(t[(i + 2) % 3]));
                let dir = (b - a) * (1.0 / a.distance(b));
                // Signed distance, positive on the vertex's current side.
                let distance = cross(dir, screen - a);
                let across = screen - Vector::new(-dir.y, dir.x) * (distance + ACROSS_NUDGE);
                (distance.abs() < MIN_THICKNESS).then_some((distance.abs(), across))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))?;

        let edit = Edit::MoveVertex {
            id,
            to: camera.to_world(line_point),
        };
        self.check(vec![edit]).map(|_| edit)
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
            // New triangle: stay clear of the source edge and of the edges of
            // (and the lines from `a` and `b` past) anything it would overlap.
            None => {
                let (wa, wb) = (self.document.vertex(a), self.document.vertex(b));
                let wanted = if area2(wa, wb, to) > 0.0 {
                    [wa, wb, to]
                } else {
                    [wb, wa, to]
                };

                let mut lines = vec![offset_line(pa, pb, cursor, SLIDE_INSET)];
                for [u, v, w] in self.document.triangles() {
                    if !overlap(wanted, [u, v, w]) {
                        continue;
                    }
                    let [u, v, w] = [u, v, w].map(|p| camera.to_screen(p));
                    lines.extend([
                        offset_line(u, v, w, -SLIDE_INSET),
                        offset_line(v, w, u, -SLIDE_INSET),
                        offset_line(w, u, v, -SLIDE_INSET),
                    ]);
                    for corner in [u, v, w] {
                        for end in [pa, pb] {
                            if end.distance(corner) > 0.0 {
                                lines.push((end, (corner - end) * (1.0 / end.distance(corner))));
                            }
                        }
                    }
                }
                lines
            }
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

    /// The candidate vertex closest to a screen position, within grab range.
    fn nearest_vertex(
        &self,
        screen: Point,
        candidates: impl Iterator<Item = VertexId>,
    ) -> Option<VertexId> {
        candidates
            .map(|id| {
                let d = self
                    .camera
                    .to_screen(self.document.vertex(id))
                    .distance(screen);
                (id, d)
            })
            .filter(|&(_, d)| d <= VERTEX_HIT)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(id, _)| id)
    }

    fn draw_document(&self, frame: &mut Frame, document: &Document) {
        frame.fill_rectangle(Point::ORIGIN, frame.size(), BACKGROUND);
        draw_grid(frame, self.camera);

        let mesh = self.mesh(document.triangles());
        frame.fill(&mesh, FILL);
        frame.stroke(&mesh, stroke(EDGE, 1.5));

        // Existing triangles that have become (almost) flat are easy to miss,
        // yet they block moves that would fold them; show them as dashed red.
        let flat = Path::new(|p| {
            for t in document.triangles() {
                let t = t.map(|v| self.camera.to_screen(v));
                let (from, to) = longest_edge(t);
                if min_height(t) < FLAT_RATIO * from.distance(to) {
                    p.move_to(from);
                    p.line_to(to);
                }
            }
        });
        frame.stroke(
            &flat,
            Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(CANCEL, 2.0)
            },
        );

        // Edges lying (partly) on top of each other mean the mesh isn't
        // joined up properly there; show the overlapping stretches in pink.
        let mut edges: Vec<(VertexId, VertexId)> = document
            .edges()
            .map(|(u, v)| (u.min(v), u.max(v)))
            .collect();
        edges.sort_unstable();
        edges.dedup();
        let edges: Vec<(Point, Point)> = edges
            .into_iter()
            .map(|(u, v)| {
                let screen = |v| self.camera.to_screen(document.vertex(v));
                (screen(u), screen(v))
            })
            .collect();

        let overlapping = Path::new(|p| {
            for (i, &first) in edges.iter().enumerate() {
                for &second in &edges[i + 1..] {
                    if let Some((from, to)) = collinear_overlap(first, second) {
                        p.move_to(from);
                        p.line_to(to);
                    }
                }
            }
        });
        frame.stroke(&overlapping, stroke(CANCEL, 3.0));
    }

    /// Highlights how `result` differs from the current document: added
    /// triangles in `accent`, and triangles squashed flat by the edit as the
    /// red line they collapse into.
    fn draw_changes(&self, frame: &mut Frame, result: &Document, accent: Color) {
        let added =
            self.mesh(new_triangles(self.document, result).map(|t| t.map(|v| result.vertex(v))));
        frame.fill(&added, Color { a: 0.2, ..accent });
        frame.stroke(&added, stroke(accent, 1.5));

        // Removed triangles whose corners now (nearly) line up were squashed;
        // other removed ones (e.g. a split parent) were replaced.
        let squashed = Path::new(|p| {
            for t in new_triangles(result, self.document) {
                let [a, b, c] = t.map(|v| self.camera.to_screen(result.vertex(v)));

                if min_height([a, b, c]) < 1.0 {
                    let (from, to) = longest_edge([a, b, c]);
                    p.move_to(from);
                    p.line_to(to);
                }
            }
        });
        frame.stroke(&squashed, stroke(CANCEL, 3.0));
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

/// Where two screen-space segments lie on top of each other (on the same
/// line, within half a pixel, and overlapping by more than that).
fn collinear_overlap((a, b): (Point, Point), (c, d): (Point, Point)) -> Option<(Point, Point)> {
    const TOLERANCE: f32 = 0.5;

    let length = a.distance(b);
    if length < TOLERANCE {
        return None;
    }
    let dir = (b - a) * (1.0 / length);

    if cross(dir, c - a).abs() > TOLERANCE || cross(dir, d - a).abs() > TOLERANCE {
        return None;
    }

    let (tc, td) = (dot(dir, c - a), dot(dir, d - a));
    let from = tc.min(td).max(0.0);
    let to = tc.max(td).min(length);

    (to - from > TOLERANCE).then(|| (a + dir * from, a + dir * to))
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

/// Triangles in `after` that `before` doesn't have (compared by corner ids).
fn new_triangles<'a>(
    before: &'a Document,
    after: &'a Document,
) -> impl Iterator<Item = [VertexId; 3]> + 'a {
    let key = |t: &[VertexId; 3]| {
        let mut t = *t;
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

/// The point the user is placing with `edits`, where its handle is drawn.
fn placed(edits: &[Edit], result: &Document) -> Point {
    match *edits.last().expect("at least one edit") {
        Edit::AddTriangle([_, to, _]) => to,
        Edit::ExtendEdge {
            apex: Corner::New(at),
            ..
        }
        | Edit::SplitTriangle { at, .. }
        | Edit::SplitEdge { at, .. }
        | Edit::CollapseOntoEdge { at, .. }
        | Edit::MoveVertex { to: at, .. } => at,
        Edit::ExtendEdge {
            apex: Corner::Existing(id),
            ..
        }
        | Edit::MergeVertex { into: id, .. } => result.vertex(id),
    }
}

fn draw_grid(frame: &mut Frame, camera: Camera) {
    let spacing = GRID * camera.zoom;
    if spacing < 8.0 {
        return;
    }

    let size = frame.size();
    let top_left = camera.to_world(Point::ORIGIN);
    let start_x = (top_left.x / GRID).floor() * GRID;
    let start_y = (top_left.y / GRID).floor() * GRID;

    let dots = Path::new(|p| {
        let mut y = start_y;
        while camera.to_screen(Point::new(0.0, y)).y <= size.height {
            let mut x = start_x;
            while camera.to_screen(Point::new(x, 0.0)).x <= size.width {
                p.circle(camera.to_screen(Point::new(x, y)), 1.2);
                x += GRID;
            }
            y += GRID;
        }
    });

    frame.fill(&dots, GRID_DOT);
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

    fn event(e: mouse::Event) -> Event {
        Event::Mouse(e)
    }

    #[test]
    fn changing_direction_mid_drag() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();
        let at = |x, y| mouse::Cursor::Available(Point::new(x, y));
        let moved = |x, y| {
            event(mouse::Event::CursorMoved {
                position: Point::new(x, y),
            })
        };

        let mut published = vec![];
        let mut run = |state: &mut State, e: Event, c| {
            if let Some(action) = editor.update(state, &e, bounds, c) {
                let (msg, _, _) = action.into_inner();
                published.extend(msg);
            }
        };
        run(&mut state, moved(200.0, 300.0), at(200.0, 300.0));
        run(
            &mut state,
            event(mouse::Event::ButtonPressed(mouse::Button::Left)),
            at(200.0, 300.0),
        );
        // Inwards, back over the source edge, then out the other side.
        for y in [290.0, 250.0, 200.0, 250.0, 299.0, 330.0, 350.0] {
            run(&mut state, moved(200.0, y), at(200.0, y));
        }
        run(
            &mut state,
            event(mouse::Event::ButtonReleased(mouse::Button::Left)),
            at(200.0, 350.0),
        );

        let edits: Vec<_> = published
            .iter()
            .filter_map(|m| match m {
                Message::Edit(edits) => Some(edits.clone()),
                _ => None,
            })
            .collect();
        assert!(matches!(
            &edits[..],
            [edits] if matches!(edits[..], [Edit::ExtendEdge {
                apex: Corner::New(_),
                ..
            }])
        ));
    }

    #[test]
    fn slivers_are_rejected() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };

        let edit = |y| {
            editor
                .pending(Interaction::Extending {
                    a: 0,
                    b: 1,
                    start: Point::new(200.0, 300.0),
                    apex: Point::new(200.0, y),
                })
                .map(|(edits, _)| edits)
        };
        assert_eq!(edit(297.0), None); // on the source edge: cancelled
        assert_eq!(edit(303.0), None);
        assert!(matches!(
            edit(250.0).as_deref(),
            Some([
                Edit::SplitTriangle { triangle: 0, .. },
                Edit::MoveVertex { .. }
            ])
        ));
        assert!(matches!(
            edit(350.0).as_deref(),
            Some([Edit::ExtendEdge {
                apex: Corner::New(_),
                ..
            }])
        ));
    }

    #[test]
    fn dragging_a_centre_out_of_the_outline_stretches_the_shape() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
            at: Point::new(200.0, 230.0),
        });
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();

        let mut published = vec![];
        let moved = mouse::Event::CursorMoved {
            position: Point::ORIGIN,
        };
        for (e, x, y) in [
            (
                mouse::Event::ButtonPressed(mouse::Button::Left),
                200.0,
                230.0,
            ),
            (moved, 200.0, 280.0),
            (moved, 200.0, 360.0),
            // Far past the edge, off to the side: slides along it.
            (moved, 260.0, 400.0),
            (
                mouse::Event::ButtonReleased(mouse::Button::Left),
                260.0,
                400.0,
            ),
        ] {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            if let Some(action) = editor.update(&mut state, &Event::Mouse(e), bounds, cursor) {
                published.extend(action.into_inner().0);
            }
        }

        // The centre follows the cursor out through the bottom edge 0–1: the
        // triangle on that edge is dropped and the other two stretch along.
        let [Message::Edit(edits)] = &published[..] else {
            panic!("expected one edit, got {published:?}");
        };
        assert_eq!(
            edits[..],
            [Edit::MoveVertex {
                id: 3,
                to: Point::new(260.0, 400.0),
            }]
        );
        assert!(doc.clone().apply_all(edits));
    }

    #[test]
    fn limited_moves_never_leave_flat_triangles() {
        // A fan around vertex 0 whose outline has a dent at (30, 30), so the
        // area vertex 0 may move in is bounded by some edges' extensions.
        let mut doc = Document::default();
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
        assert!(doc.apply(Edit::AddTriangle([centre, ring[0], ring[1]])));
        for (b, apex) in [
            (2, Corner::New(ring[2])),
            (3, Corner::New(ring[3])),
            (4, Corner::New(ring[4])),
            (5, Corner::Existing(1)),
        ] {
            assert!(doc.apply(Edit::ExtendEdge { a: 0, b, apex }));
        }
        assert_eq!(doc.triangle_ids().len(), 5);

        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };

        for x in (0..16).map(|i| i as f32 * 50.0) {
            for y in (0..12).map(|i| i as f32 * 50.0) {
                let to = editor.limit_move(0, centre, Point::new(x, y));
                let (edits, result) = editor
                    .pending(Interaction::MovingVertex { id: 0, to })
                    .expect("limited position is valid");

                if let [edit @ Edit::MoveVertex { .. }] = edits[..] {
                    for t in result.triangle_ids().iter().filter(|t| t.contains(&0)) {
                        let h = min_height(t.map(|v| result.vertex(v)));
                        assert!(
                            h >= MIN_THICKNESS,
                            "cursor ({x}, {y}): {edit:?} leaves height {h}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_new_triangle_over_other_geometry_stays_an_extension() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        // A separate triangle to the right.
        doc.apply(Edit::AddTriangle([
            Point::new(400.0, 300.0),
            Point::new(500.0, 300.0),
            Point::new(450.0, 200.0),
        ]));
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };
        let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let mut state = State::default();

        let moved = mouse::Event::CursorMoved {
            position: Point::ORIGIN,
        };
        let mut published = vec![];
        for (e, x, y) in [
            // From the right side of the first triangle out, then into the
            // other triangle: must not turn into splitting it.
            (
                mouse::Event::ButtonPressed(mouse::Button::Left),
                250.0,
                200.0,
            ),
            (moved, 320.0, 220.0),
            (moved, 380.0, 260.0),
            (moved, 460.0, 270.0),
            (
                mouse::Event::ButtonReleased(mouse::Button::Left),
                460.0,
                270.0,
            ),
        ] {
            let cursor = mouse::Cursor::Available(Point::new(x, y));
            if let Some(action) = editor.update(&mut state, &Event::Mouse(e), bounds, cursor) {
                published.extend(action.into_inner().0);
            }
        }

        let [Message::Edit(edits)] = &published[..] else {
            panic!("expected one edit, got {published:?}");
        };
        assert!(
            matches!(
                edits[..],
                [Edit::ExtendEdge {
                    a: 1 | 2,
                    b: 1 | 2,
                    ..
                }]
            ),
            "{edits:?}"
        );
        assert!(doc.clone().apply_all(edits));
    }

    #[test]
    fn moving_close_to_an_edge_line_rearranges_instead_of_stopping() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
            at: Point::new(200.0, 230.0),
        });
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };

        // The centre 3, dragged to just above the line through the bottom
        // edge 0–1, past its end: its triangle on that edge would be a sliver.
        for y in [296.0, 298.0, 299.5] {
            let (edit, result) = editor
                .pending(Interaction::MovingVertex {
                    id: 3,
                    to: Point::new(400.0, y),
                })
                .unwrap_or_else(|| panic!("moving to y = {y} is refused"));
            assert!(
                matches!(edit[..], [Edit::MoveVertex { id: 3, .. }]),
                "{edit:?}"
            );
            assert!(
                result.triangles().all(|t| min_height(t) >= MIN_THICKNESS),
                "y = {y}: {:?}",
                result.triangle_ids()
            );
        }
    }

    #[test]
    fn dragging_into_a_triangle_can_go_on_into_its_neighbour() {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        // A neighbour on the right edge 1–2, with far corner 3.
        doc.apply(Edit::ExtendEdge {
            a: 1,
            b: 2,
            apex: Corner::New(Point::new(350.0, 150.0)),
        });
        let area = |doc: &Document| {
            doc.triangles()
                .map(|[a, b, c]| crate::geometry::area2(a, b, c) / 2.0)
                .sum::<f32>()
        };
        let cache = canvas::Cache::new();
        let editor = Editor {
            document: &doc,
            camera: Camera::default(),
            cache: &cache,
        };

        // From the bottom edge 0–1 into the triangle, then onto the shared
        // edge (squashing that child) or through it (flipping it). Either
        // way the new vertex 4 gets an edge to the neighbour's far corner 3,
        // and the outline stays the same.
        for (apex, collapses) in [
            (Point::new(253.0, 200.0), true),
            (Point::new(290.0, 200.0), false),
        ] {
            let (edits, result) = editor
                .pending(Interaction::Extending {
                    a: 0,
                    b: 1,
                    start: Point::new(200.0, 300.0),
                    apex,
                })
                .unwrap_or_else(|| panic!("dragging to {apex:?} is refused"));

            assert!(matches!(edits[0], Edit::SplitTriangle { triangle: 0, .. }));
            assert_eq!(
                matches!(edits[1], Edit::CollapseOntoEdge { .. }),
                collapses,
                "{edits:?}"
            );
            assert_eq!(
                result.triangle_ids().len(),
                4,
                "{:?}",
                result.triangle_ids()
            );
            assert!(result.edges().any(|e| e == (4, 3) || e == (3, 4)));
            assert!((area(&result) - area(&doc)).abs() < 0.5);
        }
    }
}
