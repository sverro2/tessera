//! Drawing the canvas: the layers (each cached), and over them what a
//! drag would do.

use super::*;

/// Edges of `before` that the pending result no longer has, as ids after
/// it. Vertices are never removed, so they can be drawn where their ends
/// are afterwards. One that was cut at a vertex is still there, in pieces.
pub(super) fn removed_edges(before: &Document, pending: &Pending) -> Vec<(VertexId, VertexId)> {
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
pub(super) fn edges_used(document: &Document, uses: usize) -> Vec<(VertexId, VertexId)> {
    edges_used_by(document.triangle_ids(), uses)
}

/// The outline of some `triangles`: edges used by one of them.
pub(super) fn outline_of(triangles: &[[VertexId; 3]]) -> Vec<(VertexId, VertexId)> {
    edges_used_by(triangles, 1)
}

pub(super) fn edges_used_by(triangles: &[[VertexId; 3]], uses: usize) -> Vec<(VertexId, VertexId)> {
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
pub(super) fn outline(document: &Document) -> Vec<(VertexId, VertexId)> {
    edges_used(document, 1)
}

/// Edges between two triangles.
pub(super) fn interior(document: &Document) -> Vec<(VertexId, VertexId)> {
    edges_used(document, 2)
}

pub(super) fn draw_grid(frame: &mut Frame, camera: Camera) {
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

/// The painted faces, by colour.
pub(super) fn painted_faces(document: &Document) -> Vec<(Color, Vec<[Point; 3]>)> {
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
pub(super) enum Look {
    /// One colour for all faces and edges, with a hint of their paint.
    Plain,
    /// With its face colours and edge styles.
    Painted,
    /// Plain, and faint: a layer other than the one being shaped.
    Faded,
}

/// The layers, with the overlay over them.
pub(super) fn with_overlay(mut layers: Vec<Geometry>, overlay: Frame) -> Vec<Geometry> {
    layers.push(overlay.into_geometry());
    layers
}

pub(super) fn handle(frame: &mut Frame, at: Point, color: Color) {
    frame.fill(&Path::circle(at, 6.0), color);
    frame.stroke(&Path::circle(at, 6.0), stroke(BACKGROUND, 1.5));
}

pub(super) fn stroke(color: Color, width: f32) -> Stroke<'static> {
    // Round joins/caps: miter joins spike far past the corner at acute angles.
    Stroke::default()
        .with_color(color)
        .with_width(width)
        .with_line_join(LineJoin::Round)
        .with_line_cap(LineCap::Round)
}

impl Editor<'_> {
    /// What to draw: the layers and, over them, what's going on.
    /// What to draw: the layers and, over them, what's going on. The
    /// other layers as `others` sees them (not mirrored, when this works
    /// through the mirror image).
    pub(super) fn draw_scene(
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
                // Tweaking a painted edge (holding W or C): the layer as
                // it will be, that edge in its new style only, so a wide one
                // made thin doesn't show from under it.
                None if let Some(tweaked) = self.tweaked_edge(state) => {
                    let mut frame = Frame::new(renderer, size);
                    self.draw_layer(
                        &mut frame,
                        &tweaked,
                        look,
                        self.crossfade,
                        self.show_edges,
                        self.mirrors,
                    );
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

        // Layers pointed at in the layers panel: on their own (ish), the
        // rest (and the backdrop) faded far back, they over it as they look.
        if !self.lit.is_empty() {
            let mut wash = Frame::new(renderer, size);
            wash.fill_rectangle(
                Point::ORIGIN,
                size,
                Color {
                    a: 0.85,
                    ..BACKGROUND
                },
            );
            layers.push(wash.into_geometry());
            let current = SceneLayer {
                id: self.current,
                document: self.document,
                crossfade: self.crossfade,
                show_edges: self.show_edges,
                mirrors: self.mirrors,
            };
            let lit: Vec<SceneLayer> = std::iter::once(current)
                .chain(self.below.iter().copied())
                .chain(self.above.iter().copied())
                .filter(|layer| self.lit.contains(&layer.id))
                .collect();
            for layer in lit {
                self.draw_cached(renderer, size, layer, look, &mut layers);
            }
        }

        let mut overlay = Frame::new(renderer, bounds.size());
        self.draw_lit(&mut overlay);
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

        if !state.near_guides.is_empty()
            && matches!(
                state.interaction,
                Interaction::Creating { .. }
                    | Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
            )
        {
            self.draw_guides(&mut overlay, state, pending, bounds);
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
                | Interaction::PivotingSelection { .. }
                | Interaction::Extruding { .. },
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

    /// Draws the current layer, or what it's about to become.
    pub(super) fn draw_document(&self, frame: &mut Frame, document: &Document) {
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
    pub(super) fn draw_layer(
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

        // Unpainted faces hatched, as they're not exported: plainly not
        // painted (and not some colour). Then the painted ones by colour.
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let unpainted: Vec<_> = document
            .triangles()
            .zip(document.colors())
            .filter(|(_, color)| color.is_none())
            .map(|(t, _)| t)
            .collect();
        frame.fill(
            &self.mesh(unpainted.iter().copied()),
            Color { a: 0.05, ..EDGE },
        );
        frame.stroke(
            &self.hatch(&unpainted),
            stroke(Color { a: 0.3, ..EDGE }, 1.0),
        );
        for (color, triangles) in painted_faces(document) {
            frame.fill(&self.mesh(triangles.into_iter()), color);
        }
        if !show_edges {
            return;
        }

        // Unpainted edges dashed, thin: not exported either.
        let dashed = Stroke {
            line_dash: LineDash {
                segments: &[4.0, 4.0],
                offset: 0,
            },
            ..stroke(Color { a: 0.5, ..EDGE }, 1.0)
        };
        let unpainted = Path::new(|p| {
            for (a, b) in document.unique_edges() {
                if document.edge_style(a, b).is_none() {
                    p.move_to(screen(a));
                    p.line_to(screen(b));
                }
            }
        });
        frame.stroke(&unpainted, dashed);

        // The painted edges, each its own outline, so edges of different
        // widths meet without lying over each other.
        let edges: Vec<_> = document
            .edge_styles()
            .map(|(a, b, style)| (a, b, self.edge_width(style.width), style.color))
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
    pub(super) fn shows_vertices(&self, look: Look) -> bool {
        self.tool == Tool::Shape && look == Look::Plain
    }

    /// A dot on each vertex.
    pub(super) fn draw_vertices(&self, frame: &mut Frame, document: &Document) {
        let dots = Path::new(|p| {
            for v in document.unique_vertices() {
                p.circle(self.camera.to_screen(document.vertex(v)), 2.5);
            }
        });
        frame.fill(&dots, EDGE);
    }

    /// The plain look, `strength` strong (1: full), edges `width` wide, with
    /// a hint of the face colours and edge colours.
    pub(super) fn draw_hinted(
        &self,
        frame: &mut Frame,
        document: &Document,
        strength: f32,
        width: f32,
    ) {
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
    pub(super) fn draw_cached(
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

    /// The layers pointed at in the layers panel, lit up (more than the
    /// current one ever is), whatever the mode: their faces tinted a little,
    /// their outlines bright; with their mirror images, as seen. (Everything
    /// else is faded back meanwhile: see `draw_scene`.)
    fn draw_lit(&self, frame: &mut Frame) {
        if self.lit.is_empty() {
            return;
        }
        let camera = self.plain_camera();
        let current = SceneLayer {
            id: self.current,
            document: self.document,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
        };
        let layers = std::iter::once(&current)
            .chain(&self.below)
            .chain(&self.above)
            .filter(|layer| self.lit.contains(&layer.id));
        for layer in layers {
            let document = layer.document;
            let images = doc::images(layer.mirrors);
            let faces = Path::new(|p| {
                for image in &images {
                    for t in document.triangles() {
                        let [a, b, c] = t.map(|q| camera.to_screen(image.apply(q)));
                        p.move_to(a);
                        p.line_to(b);
                        p.line_to(c);
                        p.close();
                    }
                }
            });
            // Lightly: they're shown as they look underneath.
            frame.fill(&faces, Color { a: 0.06, ..HOVER });
            let outline = Path::new(|p| {
                for image in &images {
                    for (a, b) in outline(document) {
                        p.move_to(camera.to_screen(image.apply(document.vertex(a))));
                        p.line_to(camera.to_screen(image.apply(document.vertex(b))));
                    }
                }
            });
            frame.stroke(&outline, stroke(BACKGROUND, 4.0));
            frame.stroke(&outline, stroke(HOVER, 2.0));
        }
    }

    /// Stripes across `triangles` (world), on screen: one pattern over them
    /// all, so faces next to each other look one.
    fn hatch(&self, triangles: &[[Point; 3]]) -> Path {
        /// How far apart the stripes are (screen px, across them).
        const SPACING: f32 = 7.0;
        let step = SPACING * std::f32::consts::SQRT_2;
        Path::new(|p| {
            for t in triangles {
                let corners = t.map(|q| self.camera.to_screen(q));
                // Along x + y = c: where each stripe runs.
                let along = corners.map(|q| q.x + q.y);
                let low = along.iter().copied().fold(f32::INFINITY, f32::min);
                let high = along.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let mut c = (low / step).ceil() * step;
                while c <= high {
                    let mut ends = Vec::with_capacity(2);
                    for k in 0..3 {
                        let (i, j) = (k, (k + 1) % 3);
                        let (si, sj) = (along[i], along[j]);
                        if (si - c) * (sj - c) <= 0.0 && si != sj {
                            let t = (c - si) / (sj - si);
                            ends.push(corners[i] + (corners[j] - corners[i]) * t);
                        }
                    }
                    if let [a, b, ..] = ends[..] {
                        p.move_to(a);
                        p.line_to(b);
                    }
                    c += step;
                }
            }
        })
    }

    /// A painted edge's width on screen: never too thin to see.
    pub(super) fn edge_width(&self, width: f32) -> f32 {
        (width * self.camera.zoom).max(1.0)
    }

    /// Highlights how the pending result differs from the current document:
    /// added triangles in green; removed edges, and triangles squashed flat
    /// (as the line they collapse into), in red; and in yellow where things
    /// join up: vertices welded together, outline edges that become shared
    /// (other than `source`) and outline edges cut where a vertex lands on
    /// them.
    pub(super) fn draw_changes(
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
    pub(super) fn highlight(&self, start: VertexId) -> Highlight {
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
    pub(super) fn draw_shape(
        &self,
        frame: &mut Frame,
        document: &Document,
        shape: &Highlight,
        strength: f32,
    ) {
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
    pub(super) fn joins(
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

    pub(super) fn mesh(&self, triangles: impl Iterator<Item = [Point; 3]>) -> Path {
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
