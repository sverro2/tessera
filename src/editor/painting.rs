//! The paint mode: painting faces and edges with the brush, picking a
//! brush up, and the brush as the cursor.

use super::*;

/// How big (screen px) the paint mode's cursor icon is drawn.
pub(super) const CURSOR_SIZE: f32 = 26.0;

/// The paint mode's cursor: a bucket for faces, a brush for edges, both
/// dipped in the colour; an eraser to clear. The icon (24 units square) as
/// SVG, and where its tip is (screen px from its corner).
pub(super) fn paint_cursor(target: paint::Target, color: Option<Color>) -> (String, Vector) {
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
pub(super) fn pipette_cursor() -> (String, Vector) {
    let hex = paint::to_hex;
    let outline = icons::PIPETTE;
    let icon = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke-linecap="round" stroke-linejoin="round"><g stroke="{}" stroke-width="4">{outline}</g><g stroke="{}" stroke-width="2">{outline}</g></svg>"#,
        hex(BACKGROUND),
        hex(EDGE),
    );
    (icon, Vector::new(2.0, 22.0) * (CURSOR_SIZE / 24.0))
}

impl Editor<'_> {
    /// Paints `faces` or `edges` (whichever is painted) with the brush: the
    /// edit for those that change, if any.
    pub(super) fn paint(
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
    pub(super) fn paint_at(&self, state: &mut State, screen: Point) {
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
    pub(super) fn edge_at(&self, screen: Point) -> Option<(VertexId, VertexId)> {
        edge_near(self.document, screen, self.camera)
    }

    /// Whether edges are being painted (where holding W tweaks the width).
    pub(super) fn paints_edges(&self) -> bool {
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
    pub(super) fn picking(&self, state: &State) -> bool {
        matches!(self.tool, Tool::Paint { picking, .. } if picking || state.modifiers.command())
    }

    /// What the pipette picks up at `screen`: from the current layer, or
    /// else the visible layers in front, then behind it (frontmost first).
    pub(super) fn pick(&self, screen: Point) -> Option<Picked> {
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
                let (a, b) = edge_near(document, screen, camera)?;
                Some(Picked::Edge(document.edge_style(a, b)))
            }),
        }
    }

    /// Tweaking the brush (holding W, C or Shift+C): the current layer with
    /// the face or edge being tweaked painted as it will be, so it shows
    /// just as it will (its colour exactly as light and as opaque, over
    /// nothing of what it was).
    pub(super) fn tweaked(&self, state: &State) -> Option<Document> {
        let Tool::Paint {
            target,
            brush,
            width,
            ..
        } = self.tool
        else {
            return None;
        };
        let at = state.tweaking?.at;
        let edit = match target {
            paint::Target::Faces => Edit::Paint {
                triangle: self.document.triangle_at(self.camera.to_world(at))?,
                color: brush.color(),
            },
            // Edges hidden, the overlay shows it instead.
            paint::Target::Edges if !self.show_edges => return None,
            paint::Target::Edges => {
                let (a, b) = self.edge_at(at)?;
                let style = brush.color().map(|color| EdgeStyle { color, width });
                Edit::PaintEdge { a, b, style }
            }
        };
        let (tweaked, _) = self.document.preview(&[edit])?;
        Some(tweaked)
    }

    /// The paint mode's overlay: the stroke so far, what's under the
    /// cursor, and the brush (or pipette) following the cursor.
    pub(super) fn draw_painting(&self, frame: &mut Frame, state: &State, cursor: Option<Point>) {
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
                // Tweaking its lightness or opacity, the layer itself shows
                // it as it will be (see `tweaked`): nothing over it. Else a
                // hint of it, half as opaque as the brush, outlined.
                if let Some(t) = hovered
                    && state.tweaking.is_none()
                {
                    let face = faces(&[t]);
                    if let Some(color) = color.filter(|_| !picking) {
                        frame.fill(
                            &face,
                            Color {
                                a: color.a * 0.5,
                                ..color
                            },
                        );
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
                // Tweaking its width (or lightness), the layer itself shows
                // it as it will be (see `tweaked`): nothing over it.
                if let Some(edge) = focus.and_then(|p| self.edge_at(p))
                    && self.tweaked(state).is_none()
                {
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
            // Tweaking: the width, lightness or opacity, by the brush.
            if let Some(tweak) = state.tweaking {
                let content = match (tweak.what, color) {
                    (Tweaking::Width, _) => format!("{width:.1}"),
                    (Tweaking::Lightness, Some(color)) => {
                        let hsv = paint::Hsv::from_color(color, 0.0);
                        format!("{:.0}%", hsv.value * 100.0)
                    }
                    (Tweaking::Opacity, Some(color)) => format!("{:.0}%", color.a * 100.0),
                    (Tweaking::Lightness | Tweaking::Opacity, None)
                    | (Tweaking::Reach | Tweaking::LoopTurn, _) => String::new(),
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
}

/// The edge (lowest end first) of `document` closest to `screen`, as seen
/// through `camera`, within grabbing distance.
fn edge_near(document: &Document, screen: Point, camera: Camera) -> Option<(VertexId, VertexId)> {
    let at = |v| camera.to_screen(document.vertex(v));
    document
        .unique_edges()
        .into_iter()
        .map(|(a, b)| ((a, b), closest_on_segment(screen, at(a), at(b)).1))
        .filter(|&(_, distance)| distance <= EDGE_HIT)
        .min_by(|(_, d1), (_, d2)| d1.total_cmp(d2))
        .map(|(edge, _)| edge)
}
