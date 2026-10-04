//! Faces and painted edges drawn as triangle meshes, handed to the GPU as
//! they are, rather than as paths it tessellates first: the faces are
//! triangles already, and each painted edge's outline is a fan around its
//! middle. (Only the GPU renderer draws meshes; without one, they're drawn
//! as paths, as before.)
//!
//! The canvas can't return meshes, so the editor is drawn by [`EditorView`], a
//! widget like iced's canvas but drawing [`Drawn`] parts: canvas geometry,
//! or meshes, in order.

use std::sync::Arc;

use iced::advanced::Shell;
use iced::advanced::graphics::color;
use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::graphics::mesh::{self, Mesh, Renderer as _, SolidVertex2D};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self as advanced_renderer, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget};
use iced::{Event, Length, Transformation};

use super::*;

/// Something the scene draws: canvas geometry, or meshes (kept on the GPU
/// while unchanged).
pub(super) enum Drawn {
    Geometry(Geometry),
    Meshes(mesh::Cache),
}

/// A layer's meshes: its faces, under its lines (drawn on the canvas), and
/// its painted edges, over them.
#[derive(Default)]
pub(super) struct LayerMeshes {
    pub under: Builder,
    pub over: Builder,
}

impl LayerMeshes {
    /// Done: the meshes under and over, for a canvas `size`.
    pub(super) fn finish(self, size: Size) -> (mesh::Cache, mesh::Cache) {
        (self.under.into_cache(size), self.over.into_cache(size))
    }
}

/// How wide (screen px) the checkerboard's squares are.
const CHECKER: f32 = 8.0;

/// The checkerboard behind what's see-through (in the paint mode): under
/// each of `triangles` (screen), dark, with the light squares of a board
/// level on screen cut to it, on a canvas `size` big. Drawn under all the
/// layers, so it shows only where nothing opaque lies between.
pub(super) fn checker(triangles: &[[Point; 3]], size: Size) -> Builder {
    let mut builder = Builder::default();
    for &t in triangles {
        builder.triangle(t, CHECKER_DARK);
        for square in light_squares(t, size) {
            if let [first, rest @ ..] = &square[..] {
                for pair in rest.windows(2) {
                    builder.triangle([*first, pair[0], pair[1]], CHECKER_LIGHT);
                }
            }
        }
    }
    builder
}

/// The checkerboard's light squares that `t` (screen) covers on a canvas
/// `size` big, each cut to it: polygons. Only those on the canvas, as
/// zoomed in a triangle may reach far past it.
pub(super) fn light_squares(t: [Point; 3], size: Size) -> Vec<Vec<Point>> {
    let min = Point::new(
        t[0].x.min(t[1].x).min(t[2].x).max(0.0),
        t[0].y.min(t[1].y).min(t[2].y).max(0.0),
    );
    let max = Point::new(
        t[0].x.max(t[1].x).max(t[2].x).min(size.width),
        t[0].y.max(t[1].y).max(t[2].y).min(size.height),
    );
    if min.x > max.x || min.y > max.y {
        return Vec::new();
    }
    let cell = |v: f32| (v / CHECKER).floor() as i64;
    let mut squares = Vec::new();
    for j in cell(min.y)..=cell(max.y) {
        for i in cell(min.x)..=cell(max.x) {
            if (i + j).rem_euclid(2) != 0 {
                continue;
            }
            let (x, y) = (i as f32 * CHECKER, j as f32 * CHECKER);
            let square = vec![
                Point::new(x, y),
                Point::new(x + CHECKER, y),
                Point::new(x + CHECKER, y + CHECKER),
                Point::new(x, y + CHECKER),
            ];
            let cut = clip(square, t);
            if cut.len() >= 3 {
                squares.push(cut);
            }
        }
    }
    squares
}

/// `polygon` (convex) cut to triangle `t`: what of it lies inside.
fn clip(mut polygon: Vec<Point>, t: [Point; 3]) -> Vec<Point> {
    let cross =
        |a: Point, b: Point, p: Point| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    // Inside is the side its third corner is on.
    let sign = cross(t[0], t[1], t[2]).signum();
    for k in 0..3 {
        let (a, b) = (t[k], t[(k + 1) % 3]);
        let inside = |p: Point| cross(a, b, p) * sign >= 0.0;
        let mut kept = Vec::with_capacity(polygon.len() + 1);
        for (i, &p) in polygon.iter().enumerate() {
            let q = polygon[(i + 1) % polygon.len()];
            if inside(p) {
                kept.push(p);
            }
            if inside(p) != inside(q) {
                let (dp, dq) = (cross(a, b, p), cross(a, b, q));
                kept.push(p + (q - p) * (dp / (dp - dq)));
            }
        }
        polygon = kept;
        if polygon.len() < 3 {
            return Vec::new();
        }
    }
    polygon
}

/// A mesh being built: triangles, each corner its colour (screen
/// positions).
#[derive(Default)]
pub(super) struct Builder {
    vertices: Vec<SolidVertex2D>,
    indices: Vec<u32>,
}

impl Builder {
    fn vertex(&mut self, p: Point, color: color::Packed) -> u32 {
        self.vertices.push(SolidVertex2D {
            position: [p.x, p.y],
            color,
        });
        (self.vertices.len() - 1) as u32
    }

    /// A triangle, all of it `color`.
    pub(super) fn triangle(&mut self, corners: [Point; 3], color: Color) {
        let packed = color::pack(color);
        let [a, b, c] = corners.map(|p| self.vertex(p, packed));
        self.indices.extend([a, b, c]);
    }

    /// A line from `a` to `b`, `width` wide, `color`: a thin rectangle,
    /// reaching half its width past each end (as a round cap does, near
    /// enough at the widths lines are).
    pub(super) fn line(&mut self, a: Point, b: Point, width: f32, color: Color) {
        let d = b - a;
        let length = d.x.hypot(d.y);
        if length == 0.0 {
            return;
        }
        let h = width / 2.0;
        let along = d * (h / length);
        let across = Vector::new(-along.y, along.x);
        let (a, b) = (a - along, b + along);
        let packed = color::pack(color);
        let [p, q, r, s] =
            [a + across, b + across, b - across, a - across].map(|p| self.vertex(p, packed));
        self.indices.extend([p, q, r, p, r, s]);
    }

    /// The polygon through `outline`, `color`, as a fan of triangles round
    /// `centre` (from where all of it is seen).
    pub(super) fn fan(&mut self, centre: Point, outline: &[Point], color: Color) {
        if outline.len() < 2 {
            return;
        }
        let packed = color::pack(color);
        let middle = self.vertex(centre, packed);
        let first = self.vertex(outline[0], packed);
        let mut last = first;
        for &p in &outline[1..] {
            let next = self.vertex(p, packed);
            self.indices.extend([middle, last, next]);
            last = next;
        }
        self.indices.extend([middle, last, first]);
    }

    /// A dot round `centre`, `radius` across, `color`: a polygon close
    /// enough to a circle at the sizes dots are.
    pub(super) fn dot(&mut self, centre: Point, radius: f32, color: Color) {
        const SIDES: usize = 10;
        let packed = color::pack(color);
        let middle = self.vertex(centre, packed);
        let first = middle + 1;
        for i in 0..SIDES {
            let angle = i as f32 * std::f32::consts::TAU / SIDES as f32;
            let corner = centre + Vector::new(angle.cos(), angle.sin()) * radius;
            let _ = self.vertex(corner, packed);
            let next = first + ((i + 1) % SIDES) as u32;
            self.indices.extend([middle, first + i as u32, next]);
        }
    }

    /// Done: the mesh, for a canvas `size`, to keep on the GPU.
    pub(super) fn into_cache(self, size: Size) -> mesh::Cache {
        let meshes: Arc<[Mesh]> = self.mesh(size).into_iter().collect();
        mesh::Cache::new(meshes)
    }

    fn mesh(self, size: Size) -> Option<Mesh> {
        (!self.indices.is_empty()).then(|| Mesh::Solid {
            buffers: mesh::Indexed {
                vertices: self.vertices,
                indices: self.indices,
            },
            transformation: Transformation::IDENTITY,
            clip_bounds: Rectangle::new(Point::ORIGIN, size),
        })
    }
}

/// The editor as a widget: as iced's canvas, drawing the editor's
/// [`Drawn`] parts (with meshes on the GPU renderer).
pub(super) struct EditorView<'a> {
    editor: Editor<'a>,
    last_interaction: Option<mouse::Interaction>,
}

impl<'a> EditorView<'a> {
    pub(super) fn new(editor: Editor<'a>) -> Self {
        EditorView {
            editor,
            last_interaction: None,
        }
    }
}

/// Whether `renderer` draws meshes: the GPU one does, its fallback doesn't.
fn draws_meshes(renderer: &Renderer) -> bool {
    matches!(renderer, iced_renderer::fallback::Renderer::Primary(_))
}

impl Widget<Message, Theme, Renderer> for EditorView<'_> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) {
        tree.size = layout::atomic(limits, Length::Fill, Length::Fill);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        use canvas::Program;
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        let redrawing = matches!(event, Event::Window(window::Event::RedrawRequested(_)));

        if let Some(action) = self.editor.update(state, event, bounds, cursor) {
            let (message, redraw, status) = action.into_inner();
            shell.request_redraw_at(redraw);
            if let Some(message) = message {
                shell.publish(message);
            }
            if status == iced::event::Status::Captured {
                shell.capture_event();
            }
        }

        // The cursor changing (drawn by the editor) needs a frame too.
        if shell.redraw_request() != window::RedrawRequest::NextFrame {
            let interaction = self.mouse_interaction(tree, layout, cursor, viewport, renderer);
            if redrawing {
                self.last_interaction = Some(interaction);
            } else if self
                .last_interaction
                .is_some_and(|last| last != interaction)
            {
                shell.request_redraw();
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        use canvas::Program;
        let state = tree.state.downcast_ref::<State>();
        self.editor
            .mouse_interaction(state, layout.bounds(), cursor)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &advanced_renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        let state = tree.state.downcast_ref::<State>();
        let meshes = draws_meshes(renderer);
        self.editor.render(renderer, state, bounds, cursor, meshes);
    }
}

impl Editor<'_> {
    /// Draws all of the editor at `bounds`: the backdrop, the checkerboard
    /// in it, then the drawing over them, in a layer of its own (images,
    /// the background, are drawn after shapes within one), and over it
    /// the document's size.
    pub(super) fn render(
        &self,
        renderer: &mut Renderer,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
        meshes: bool,
    ) {
        let Parts { beneath, drawing } = self.draw_parts(state, renderer, bounds, cursor, meshes);
        let [under, over, label] = self.draw_backdrop(renderer, bounds.size());
        let draw = |renderer: &mut Renderer, parts: Vec<Drawn>| {
            renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
                for part in parts {
                    match part {
                        Drawn::Geometry(geometry) => renderer.draw_geometry(geometry),
                        Drawn::Meshes(meshes) if !meshes.is_empty() => {
                            renderer.draw_mesh_cache(meshes);
                        }
                        Drawn::Meshes(_) => {}
                    }
                }
            });
        };
        let backdrop = [Drawn::Geometry(under)]
            .into_iter()
            .chain(beneath)
            .chain([Drawn::Geometry(over)]);
        draw(renderer, backdrop.collect());
        let drawing = drawing.into_iter().chain([Drawn::Geometry(label)]);
        renderer.with_layer(bounds, |renderer| draw(renderer, drawing.collect()));
    }
}

impl<'a> From<EditorView<'a>> for Element<'a, Message> {
    fn from(scene: EditorView<'a>) -> Self {
        Element::new(scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: Size = Size::new(800.0, 600.0);

    /// The area of a polygon.
    fn area(polygon: &[Point]) -> f32 {
        let twice: f32 = (0..polygon.len())
            .map(|i| {
                let (p, q) = (polygon[i], polygon[(i + 1) % polygon.len()]);
                p.x * q.y - q.x * p.y
            })
            .sum();
        twice.abs() / 2.0
    }

    #[test]
    fn the_checkerboards_light_squares_are_cut_to_the_triangle() {
        // Half of two squares by two: the light one at the corner whole,
        // the one across the long side not at all.
        let t = [
            Point::new(0.0, 0.0),
            Point::new(2.0 * CHECKER, 0.0),
            Point::new(0.0, 2.0 * CHECKER),
        ];
        let light: f32 = light_squares(t, CANVAS)
            .iter()
            .map(|square| area(square))
            .sum();
        assert!((light - CHECKER * CHECKER).abs() < 0.01, "{light}");
        // Turned the other way round: the same.
        let light: f32 = light_squares([t[0], t[2], t[1]], CANVAS)
            .iter()
            .map(|square| area(square))
            .sum();
        assert!((light - CHECKER * CHECKER).abs() < 0.01, "{light}");
    }

    #[test]
    fn the_checkerboard_lies_level_on_screen_wherever_the_triangle_is() {
        // Across many squares, off the board's lines: half of it light.
        let t = [
            Point::new(13.0, 53.0),
            Point::new(261.0, 30.0),
            Point::new(110.0, 240.0),
        ];
        let light: f32 = light_squares(t, CANVAS)
            .iter()
            .map(|square| area(square))
            .sum();
        let ratio = light / area(&t);
        assert!((ratio - 0.5).abs() < 0.02, "{ratio}");
        for square in light_squares(t, CANVAS) {
            for p in square {
                assert!(p.x >= 12.99 && p.x <= 261.01 && p.y >= 29.99 && p.y <= 240.01);
            }
        }
    }

    #[test]
    fn the_checkerboard_is_worked_out_only_on_the_canvas() {
        // Zoomed far in: a triangle far wider than the canvas, round it.
        let t = [
            Point::new(-1e6, -1e6),
            Point::new(1e6, -1e6),
            Point::new(400.0, 1e6),
        ];
        let squares = light_squares(t, CANVAS);
        let light: f32 = squares.iter().map(|square| area(square)).sum();
        // (Those it touches at its far sides whole.)
        let most = (CANVAS.width + CHECKER) * (CANVAS.height + CHECKER) / 2.0;
        assert!(
            light >= CANVAS.width * CANVAS.height / 2.0 && light <= most,
            "{light}"
        );
        // Off it: none.
        let off = t.map(|p| p + Vector::new(3e6, 0.0));
        assert!(light_squares(off, CANVAS).is_empty());
    }

    #[test]
    fn nothing_see_through_has_no_checkerboard() {
        assert!(checker(&[], CANVAS).indices.is_empty());
        let t = [
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
            Point::new(0.0, 20.0),
        ];
        assert!(!checker(&[t], CANVAS).indices.is_empty());
    }
}
