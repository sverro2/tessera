//! What the canvas asks for: edits to the current layer, and changing the
//! view, the brush, the clipboard, the mirror or the background as it
//! goes.

use crate::*;

impl Tessera {
    pub(crate) fn update_canvas(&mut self, message: editor::Message) -> Task<Message> {
        match message {
            editor::Message::Edit { edits, revision } => {
                if revision != self.document().revision() {
                    return Task::none();
                }
                self.layers_panel.renaming = None;
                let before = self.layers.clone();
                let current = self.current();
                let layer = self.layers.layer_mut(current).expect("current layer");
                if Arc::make_mut(&mut layer.document).apply_all(&edits) {
                    for edit in &edits {
                        let color = match *edit {
                            Edit::Paint { color, .. } => color,
                            Edit::PaintEdge { style, .. } => style.map(|style| style.color),
                            _ => None,
                        };
                        if let Some(color) = color {
                            paint::remember(&mut self.paint.recent, color);
                        }
                    }
                    self.push_undo(before);
                }
                Task::none()
            }
            editor::Message::Frame { points, layer } => {
                if let Some(layer) = layer {
                    self.select_layer(layer);
                }
                // Clear of the edges, and of the panels over the canvas.
                const MARGIN: f32 = 60.0;
                let canvas = Self::canvas_size(self.window_size.unwrap_or(WINDOW_SIZE));
                self.camera = self.camera.framing(&points, canvas, MARGIN);
                self.view_changed()
            }
            editor::Message::Pan(delta) => {
                self.camera.pan += delta;
                self.view_changed()
            }
            editor::Message::Zoom { anchor, factor } => {
                self.camera.zoom_at(anchor, factor);
                self.view_changed()
            }
            editor::Message::Rotate { anchor, angle } => {
                self.camera.rotate_at(anchor, angle);
                self.view_changed()
            }
            editor::Message::TweakWidth(across) => {
                // In proportion: fine when thin, quicker when thick.
                self.paint.edge_width =
                    (self.paint.edge_width * (across * 0.01).exp()).clamp(0.5, 12.0);
                Task::none()
            }
            editor::Message::TweakReach(across) => {
                self.shape.reach =
                    (self.shape.reach * (across * 0.01).exp()).clamp(REACH.0, REACH.1);
                Task::none()
            }
            editor::Message::TweakLightness(across) => {
                // On the colour's value itself: through black and back, its
                // hue and saturation stay.
                if let Brush::Color(_) = self.paint.brush {
                    self.paint.hsv.value = (self.paint.hsv.value + across * 0.004).clamp(0.0, 2.0);
                    let color = self.paint.hsv.to_color();
                    self.paint.brush = Brush::Color(color);
                    self.paint.hex = paint::to_hex(color);
                }
                Task::none()
            }
            editor::Message::TweakOpacity(across) => {
                // From see-through to opaque in some 500 px.
                if let Brush::Color(_) = self.paint.brush {
                    let alpha = (self.paint.hsv.alpha + across * 0.002).clamp(0.0, 1.0);
                    self.paint.hsv.alpha = alpha;
                    let color = self.paint.hsv.to_color();
                    self.paint.brush = Brush::Color(color);
                    self.paint.hex = paint::to_hex(color);
                }
                Task::none()
            }
            editor::Message::Pick(picked) => {
                // Brushes that paint just like what was picked.
                let color = match picked {
                    editor::Picked::Face(color) => color,
                    editor::Picked::Edge(style) => {
                        if let Some(style) = style {
                            self.paint.edge_width = style.width;
                        }
                        style.map(|style| style.color)
                    }
                };
                self.paint.picking = false;
                self.set_brush(match color {
                    Some(color) => Brush::Color(color),
                    None => Brush::Eraser,
                });
                Task::none()
            }
            editor::Message::Copy(piece) => {
                let faces = piece.triangles.len();
                self.clipboard = Some(piece);
                let plural = if faces == 1 { "" } else { "s" };
                self.notify(format!("Copied {faces} face{plural}"), false);
                Task::none()
            }
            editor::Message::Cut {
                piece,
                edits,
                revision,
            } => {
                if revision != self.document().revision() {
                    return Task::none();
                }
                let faces = piece.triangles.len();
                self.clipboard = Some(piece);
                let plural = if faces == 1 { "" } else { "s" };
                self.notify(format!("Cut {faces} face{plural}"), false);
                self.update(Message::Editor(editor::Message::Edit { edits, revision }))
            }
            editor::Message::SetMirror(mirror) => {
                let before = self.layers.clone();
                self.layers.set_mirror(self.current(), Some(mirror));
                self.shape.placing_mirror = false;
                self.push_undo(before);
                Task::none()
            }
            editor::Message::MoveBackground(delta) => {
                if let Some(background) = self.background_mut() {
                    background.center += delta;
                    self.background_changed();
                }
                Task::none()
            }
            editor::Message::ScaleBackground { anchor, factor } => {
                if let Some(background) = self.background_mut() {
                    background.scale_at(anchor, factor);
                    self.background_changed();
                }
                Task::none()
            }
            editor::Message::RotateBackground { anchor, angle } => {
                if let Some(background) = self.background_mut() {
                    background.rotate_at(anchor, angle);
                    self.background_changed();
                }
                Task::none()
            }
        }
    }
}
