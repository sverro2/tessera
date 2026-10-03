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

use iced::advanced::graphics::color;
use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::graphics::mesh::{self, Mesh, Renderer as _, SolidVertex2D};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self as advanced_renderer, Renderer as _};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::Shell;
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
        let mesh = |builder: Builder| {
            let meshes: Arc<[Mesh]> = builder.mesh(size).into_iter().collect();
            mesh::Cache::new(meshes)
        };
        (mesh(self.under), mesh(self.over))
    }
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
            } else if self.last_interaction.is_some_and(|last| last != interaction) {
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
        self.editor.mouse_interaction(state, layout.bounds(), cursor)
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
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            for part in self.editor.draw_parts(state, renderer, bounds, cursor, meshes) {
                match part {
                    Drawn::Geometry(geometry) => renderer.draw_geometry(geometry),
                    Drawn::Meshes(meshes) if !meshes.is_empty() => {
                        renderer.draw_mesh_cache(meshes);
                    }
                    Drawn::Meshes(_) => {}
                }
            }
        });
    }
}

impl<'a> From<EditorView<'a>> for Element<'a, Message> {
    fn from(scene: EditorView<'a>) -> Self {
        Element::new(scene)
    }
}
