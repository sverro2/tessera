//! The interactive canvas: renders a [`Document`] through a [`Camera`] and
//! turns mouse input into [`Message`]s. It never mutates the document itself.
//!
//! Left-drag on blank canvas:
//! - creates the first triangle while the document is empty,
//! - otherwise pans (hold Shift to create a separate triangle instead).
//!
//! Middle-drag always pans; the wheel zooms around the cursor.

use iced::keyboard;
use iced::mouse;
use iced::widget::canvas::{
    self, Canvas, Event, Frame, Geometry, LineCap, LineDash, LineJoin, Path, Stroke,
};
use iced::{Color, Element, Fill, Point, Rectangle, Renderer, Theme, Vector};

use crate::camera::Camera;
use crate::document::{Corner, Document, Edit, VertexId};
use crate::geometry::{closest_on_segment, min_height};

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
/// How far (screen px) inside a limiting edge a dragged vertex stops.
const SLIDE_INSET: f32 = 1.0;
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

#[derive(Debug, Clone, Copy)]
pub enum Message {
    Pan(Vector),
    Zoom { anchor: Point, factor: f32 },
    Edit(Edit),
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
                        Some(Hover::Edge { a, b, at }) => Interaction::Extending { a, b, apex: at },
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
                    Interaction::Extending { apex, .. } => *apex = world,
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
                        Some((edit, _)) => canvas::Action::publish(Message::Edit(edit)),
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
            (_, Some((edit, result))) => {
                let at = camera.to_screen(placed(*edit, result));

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

    /// The edit releasing `interaction` now would make, together with the
    /// resulting document. `None` if releasing would do nothing.
    fn pending(&self, interaction: Interaction) -> Option<(Edit, Document)> {
        let edit = match interaction {
            Interaction::Idle | Interaction::Panning { .. } => {
                return None;
            }
            Interaction::Creating { from, to } => Edit::AddTriangle(equilateral(from, to)),
            Interaction::MovingVertex { id, to } => self.move_edit(id, to),
            Interaction::Extending { a, b, apex, .. } => self.extend_edit(a, b, apex),
        };

        let mut result = self.document.clone();
        let applied = result.apply(edit);

        // Slivers would be invisible and impossible to grab.
        let has_sliver = new_triangles(self.document, &result)
            .map(|t| t.map(|v| result.vertex(v)))
            .any(|t| min_height(t.map(|p| self.camera.to_screen(p))) < MIN_THICKNESS);

        (applied && !has_sliver).then_some((edit, result))
    }

    /// Dragging from edge `a`–`b` to `apex`: attach to a nearby vertex, split
    /// the triangle under the cursor, or add a triangle with a new corner.
    fn extend_edit(&self, a: VertexId, b: VertexId, apex: Point) -> Edit {
        let snapped = self.nearest_vertex(
            self.camera.to_screen(apex),
            self.document.used_vertices().filter(|&v| v != a && v != b),
        );

        match (snapped, self.document.triangle_at(apex)) {
            (Some(id), _) => Edit::ExtendEdge {
                a,
                b,
                apex: Corner::Existing(id),
            },
            (None, Some(triangle)) => Edit::SplitTriangle { triangle, at: apex },
            (None, None) => Edit::ExtendEdge {
                a,
                b,
                apex: Corner::New(apex),
            },
        }
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
            .map_or(Edit::MoveVertex { id, to }, |(edit, _)| edit)
    }

    /// Where vertex `id` should be when the cursor is at `to`, given that
    /// `from` was valid: the cursor itself if the move is allowed, otherwise
    /// the closest allowed point, so the vertex slides along the edges that
    /// keep it from folding over its neighbours.
    fn limit_move(&self, id: VertexId, from: Point, to: Point) -> Point {
        let valid = |p| {
            self.pending(Interaction::MovingVertex { id, to: p })
                .is_some()
        };

        if valid(to) {
            return to;
        }

        let camera = self.camera;
        let cursor = camera.to_screen(to);
        let origin = camera.to_screen(self.document.vertex(id));

        // The vertex must stay on its own side of the opposite edge of each
        // of its triangles. Those edges' lines, nudged inwards (screen space)
        // as (point, unit direction):
        let lines: Vec<(Point, Vector)> = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|t| {
                let i = t.iter().position(|&v| v == id)?;
                let a = camera.to_screen(self.document.vertex(t[(i + 1) % 3]));
                let b = camera.to_screen(self.document.vertex(t[(i + 2) % 3]));
                let dir = (b - a) * (1.0 / a.distance(b));
                let mut normal = Vector::new(-dir.y, dir.x);
                if dot(normal, origin - a) < 0.0 {
                    normal *= -1.0;
                }
                Some((a + normal * SLIDE_INSET, dir))
            })
            .collect();

        // Candidates: the cursor projected onto each line (sliding along an
        // edge) and the lines' crossings (stuck in a corner).
        let projections = lines.iter().map(|&(p, d)| p + d * dot(cursor - p, d));
        let corners = lines.iter().enumerate().flat_map(|(i, &(p1, d1))| {
            lines[i + 1..].iter().filter_map(move |&(p2, d2)| {
                let den = cross(d1, d2);
                (den.abs() > 1e-6).then(|| p1 + d1 * (cross(p2 - p1, d2) / den))
            })
        });

        projections
            .chain(corners)
            .map(|p| (camera.to_world(p), p.distance(cursor)))
            .filter(|&(p, _)| valid(p))
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map_or(from, |(p, _)| p)
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

/// The point the user is placing with `edit`, where its handle is drawn.
fn placed(edit: Edit, result: &Document) -> Point {
    match edit {
        Edit::AddTriangle([_, to, _]) => to,
        Edit::ExtendEdge {
            apex: Corner::New(at),
            ..
        }
        | Edit::SplitTriangle { at, .. }
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
                Message::Edit(edit) => Some(*edit),
                _ => None,
            })
            .collect();
        assert!(matches!(
            edits[..],
            [Edit::ExtendEdge {
                apex: Corner::New(_),
                ..
            }]
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
                    apex: Point::new(200.0, y),
                })
                .map(|(edit, _)| edit)
        };
        assert_eq!(edit(297.0), None); // thin split
        assert_eq!(edit(303.0), None); // thin extension
        assert_eq!(edit(50.0), None); // out through the far side: would overlap
        assert!(matches!(
            edit(250.0),
            Some(Edit::SplitTriangle { triangle: 0, .. })
        ));
        assert!(matches!(
            edit(350.0),
            Some(Edit::ExtendEdge {
                apex: Corner::New(_),
                ..
            })
        ));
    }

    #[test]
    fn dragging_a_vertex_across_an_edge_stops_at_it() {
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

        // The centre can't pass the bottom edge 0–1; it slides along it to
        // right above the cursor and snaps onto it, and the result is valid.
        let [Message::Edit(edit)] = published[..] else {
            panic!("expected one edit, got {published:?}");
        };
        let Edit::CollapseOntoEdge {
            a: 0 | 1,
            b: 0 | 1,
            at,
            ..
        } = edit
        else {
            panic!("expected a collapse onto 0–1, got {edit:?}");
        };
        assert!((at.x - 260.0).abs() < 1.0, "{at:?}");
        assert!(doc.clone().apply(edit));
    }
}
