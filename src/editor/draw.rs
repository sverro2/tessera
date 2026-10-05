//! Drawing the canvas: the layers (each cached, those changed drawn side by
//! side; see `painter`), and over them what's going on: what a drag would
//! do, the shape pointed at, what's lit up.

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

/// The dot grid: a dot every [`MAJOR`] steps of the snapping grid, from
/// the world's origin (so dots are where it snaps to); fading as they
/// crowd zoomed out, none once they'd blur together.
pub(super) fn draw_grid(frame: &mut Frame, camera: Camera, grid: Grid) {
    /// How far apart (screen px) dots are drawn fully; closer, they fade.
    const SHOWN: f32 = 16.0;
    /// Closer than this (screen px), no dots.
    const HIDDEN: f32 = 8.0;
    let spacing = grid.step * MAJOR as f32;
    let apart = spacing * camera.zoom;
    if apart < HIDDEN {
        return;
    }
    let shown = ((apart - HIDDEN) / (SHOWN - HIDDEN)).min(1.0);
    let color = Color {
        a: GRID_DOT.a * shown,
        ..GRID_DOT
    };
    let o = Point::ORIGIN;

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

    // Each dot a little square, as tiny as they are: one path of
    // thousands of circles takes ages to fill (lyon sweeps it all), and
    // rectangles are filled directly.
    let dot = Size::new(2.0, 2.0);
    // Counted off from the origin, so they don't drift far from it.
    let first = |min: f32, o: f32| ((min - o) / spacing).floor() as i64;
    let (i0, j0) = (first(min(|p| p.x), o.x), first(min(|p| p.y), o.y));
    let mut j = j0;
    while o.y + j as f32 * spacing <= max(|p| p.y) {
        let mut i = i0;
        while o.x + i as f32 * spacing <= max(|p| p.x) {
            let world = Point::new(o.x + i as f32 * spacing, o.y + j as f32 * spacing);
            let at = camera.to_screen(world);
            if visible.contains(at) {
                frame.fill_rectangle(at - Vector::new(1.0, 1.0), dot, color);
            }
            i += 1;
        }
        j += 1;
    }
}

/// A layer drawn on `frame`, with (with meshes) its faces under and its
/// painted edges over.
pub(super) fn push_layer(
    layers: &mut Vec<Drawn>,
    frame: Frame,
    meshes: Option<LayerMeshes>,
    size: Size,
) {
    match meshes.map(|meshes| meshes.finish(size)) {
        Some((under, over)) => {
            layers.push(Drawn::Meshes(under));
            layers.push(Drawn::Geometry(frame.into_geometry()));
            layers.push(Drawn::Meshes(over));
        }
        None => layers.push(Drawn::Geometry(frame.into_geometry())),
    }
}

/// The checkerboard behind `triangles` (screen; see `meshes::checker`):
/// as a mesh, or on the canvas without `meshes`. None if there's nothing
/// see-through.
pub(super) fn draw_checker(
    renderer: &Renderer,
    size: Size,
    triangles: &[[Point; 3]],
    meshes: bool,
) -> Option<Drawn> {
    if triangles.is_empty() {
        return None;
    }
    if meshes {
        return Some(Drawn::Meshes(
            meshes::checker(triangles, size).into_cache(size),
        ));
    }
    let mut frame = Frame::new(renderer, size);
    let mesh = |triangles: &mut dyn Iterator<Item = &[Point]>| {
        Path::new(|p| {
            for polygon in triangles {
                p.move_to(polygon[0]);
                for &q in &polygon[1..] {
                    p.line_to(q);
                }
                p.close();
            }
        })
    };
    frame.fill(&mesh(&mut triangles.iter().map(|t| &t[..])), CHECKER_DARK);
    let light: Vec<Vec<Point>> = triangles
        .iter()
        .flat_map(|&t| meshes::light_squares(t, size))
        .collect();
    frame.fill(&mesh(&mut light.iter().map(|p| &p[..])), CHECKER_LIGHT);
    Some(Drawn::Geometry(frame.into_geometry()))
}

/// The scene: the checkerboard behind what's see-through (drawn into the
/// backdrop, under its image and grid), and the drawing (the layers and
/// what's going on over them).
pub(super) struct Parts {
    pub(super) beneath: Vec<Drawn>,
    pub(super) drawing: Vec<Drawn>,
}

/// Why nothing can be edited: the current layer is out of view.
pub(super) fn draw_out_of_view(overlay: &mut Frame, bounds: Rectangle) {
    overlay.fill_text(canvas::Text {
        content:
            "The current layer is out of view (hidden, or others isolated): show it to edit it"
                .into(),
        position: Point::new(bounds.width / 2.0, 14.0),
        color: EDGE,
        size: 13.0.into(),
        align_x: iced::widget::text::Alignment::Center,
        ..canvas::Text::default()
    });
}

/// The layers, with the overlay over them.
pub(super) fn with_overlay(mut layers: Vec<Drawn>, overlay: Frame) -> Vec<Drawn> {
    layers.push(Drawn::Geometry(overlay.into_geometry()));
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

/// A layer to draw, with the painter to draw it with, and how.
pub(super) struct Wanted<'a> {
    painter: Painter<'a>,
    layer: SceneLayer<'a>,
    look: Look,
}

impl Editor<'_> {
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
        meshes: bool,
    ) -> Parts {
        // While dragging, draw the document as it will be after release.
        let pending = match state.interaction {
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::Zooming { .. }
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

        let (beneath, layers) = self.draw_layers(
            state,
            renderer,
            bounds.size(),
            others_view,
            meshes,
            pending,
            source,
        );

        let mut overlay = Frame::new(renderer, bounds.size());
        self.draw_lit(&mut overlay);
        let cursor_pos = cursor.position_in(bounds);
        match self.tool {
            // Adjusting the background: just its outline; the drawing rests.
            Tool::Background { .. } => self.draw_background_outline(&mut overlay),
            // Hidden: nothing to edit; say why clicking does nothing.
            _ if !self.shown => draw_out_of_view(&mut overlay, bounds),
            // Painting: what the stroke paints so far, what clicking would
            // paint, and the brush as the cursor.
            Tool::Paint { .. } => self.draw_painting(&mut overlay, state, cursor_pos),
            Tool::Mirror => self.draw_placing_mirror(&mut overlay, state, bounds, cursor_pos),
            Tool::Shape => {
                self.draw_shaping(&mut overlay, state, bounds, cursor_pos, pending, source)
            }
        }
        Parts {
            beneath,
            drawing: with_overlay(layers, overlay),
        }
    }

    /// The backdrop: the background colour and the document's sheet,
    /// under the checkerboard; the background image, the grid and the
    /// sheet's outline, over it (see-through, they're seen). And the
    /// sheet's size, to go over the drawing, so wide edges along the
    /// sheet's top (zoomed in) don't hide it.
    pub(super) fn draw_backdrop(&self, renderer: &Renderer, size: Size) -> [Geometry; 3] {
        let sheet = self.page.map(|page| {
            let [first, rest @ ..] = page.corners().map(|p| self.camera.to_screen(p));
            Path::new(|path| {
                path.move_to(first);
                for p in rest {
                    path.line_to(p);
                }
                path.close();
            })
        });
        let cache = &self.caches.backdrop;
        let under = cache.under.draw(renderer, size, |frame| {
            frame.fill_rectangle(Point::ORIGIN, frame.size(), BACKGROUND);
            if let Some(sheet) = &sheet {
                frame.fill(sheet, PAGE);
            }
        });
        let over = cache.over.draw(renderer, size, |frame| {
            if let Some(background) = self.background {
                background.draw(frame, self.camera);
            }
            draw_grid(frame, self.camera, self.grid);
            if let Some(sheet) = &sheet {
                frame.stroke(sheet, stroke(PAGE_EDGE, 1.0));
            }
        });
        let label = cache.label.draw(renderer, size, |frame| {
            if let Some(page) = self.page {
                // Above its top left corner, turned with it.
                let corner = self.camera.to_screen(page.min());
                frame.with_save(|frame| {
                    frame.translate(corner - Point::ORIGIN);
                    frame.rotate(self.camera.rotation);
                    frame.fill_text(canvas::Text {
                        content: format!("{:.0} × {:.0}", page.size.width, page.size.height),
                        position: Point::new(0.0, -18.0),
                        color: PAGE_EDGE,
                        size: 12.0.into(),
                        ..canvas::Text::default()
                    });
                });
            }
        });
        [under, over, label]
    }

    /// The layers: in the paint mode as they are, in their order;
    /// otherwise the others faded, and the current layer over them all, so
    /// it's always seen whole; while dragging (`pending`) or tweaking an
    /// edge, the current one as it will be. Then those pointed at in the
    /// layers panel, lit up. The others as `others_view` sees them. Apart,
    /// the checkerboard behind what's see-through of them.
    #[allow(clippy::too_many_arguments)]
    fn draw_layers(
        &self,
        state: &State,
        renderer: &Renderer,
        size: Size,
        others_view: &Editor,
        meshes: bool,
        pending: Option<&Pending>,
        source: Option<(VertexId, VertexId)>,
    ) -> (Vec<Drawn>, Vec<Drawn>) {
        let look = self.look();
        let others = if look == Look::Painted {
            Look::Painted
        } else {
            Look::Faded
        };
        // Each from its cache, or drawn anew side by side: those behind,
        // those in front, and (unless drawn as it will be, below) the
        // current one.
        let current = SceneLayer {
            id: self.current,
            document: self.document,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
        };
        let tweaked = (self.shown && pending.is_none())
            .then(|| self.tweaked(state))
            .flatten();
        let cached_current = self.shown && pending.is_none() && tweaked.is_none();
        let others_painter = others_view.painter();
        let wanted: Vec<Wanted> = self
            .below
            .iter()
            .chain(&self.above)
            .map(|&layer| Wanted {
                painter: others_painter,
                layer,
                look: others,
            })
            .chain(cached_current.then(|| Wanted {
                painter: self.painter(),
                layer: current,
                look,
            }))
            .collect();
        let drawn = self.draw_cached(renderer, size, &wanted, meshes);
        // What's see-through shows a checkerboard behind it, under all
        // the layers: where nothing opaque lies between.
        let (checkers, drawn): (Vec<_>, Vec<_>) = drawn.into_iter().unzip();
        let mut beneath: Vec<Drawn> = checkers.into_iter().flatten().collect();
        let mut drawn = drawn.into_iter();
        let mut layers: Vec<Drawn> = drawn.by_ref().take(self.below.len()).flatten().collect();
        let mut above: Vec<Drawn> = drawn.by_ref().take(self.above.len()).flatten().collect();
        if look != Look::Painted {
            layers.append(&mut above);
        }
        if self.shown {
            match &pending {
                Some(pending) => {
                    let mut sink = meshes.then(LayerMeshes::default);
                    let mut frame = Frame::new(renderer, size);
                    let mut see_through = Vec::new();
                    self.draw_document(
                        &mut frame,
                        &pending.result,
                        sink.as_mut(),
                        &mut see_through,
                    );
                    beneath.extend(draw_checker(renderer, size, &see_through, meshes));
                    push_layer(&mut layers, frame, sink, size);
                    let mut changes = Frame::new(renderer, size);
                    self.draw_changes(&mut changes, pending, source);
                    layers.push(Drawn::Geometry(changes.into_geometry()));
                }
                // Tweaking the brush (holding W, C or Shift+C): the layer as
                // it will be, the face or edge tweaked painted anew only, so
                // what it was doesn't show from under it (a wide edge made
                // thin, a colour made see-through).
                None if let Some(tweaked) = &tweaked => {
                    let mut sink = meshes.then(LayerMeshes::default);
                    let mut frame = Frame::new(renderer, size);
                    let style = Style::of(look, self.show_edges);
                    let mut see_through = Vec::new();
                    self.draw_layer(
                        &mut frame,
                        tweaked,
                        style,
                        self.mirrors,
                        sink.as_mut(),
                        &mut see_through,
                    );
                    beneath.extend(draw_checker(renderer, size, &see_through, meshes));
                    push_layer(&mut layers, frame, sink, size);
                }
                None => layers.extend(drawn.flatten()),
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
                    .chain(&self.lit)
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
            layers.push(Drawn::Geometry(wash.into_geometry()));
            let lit: Vec<Wanted> = self
                .lit
                .iter()
                .map(|&layer| Wanted {
                    painter: self.painter(),
                    layer,
                    look,
                })
                .collect();
            layers.extend(
                self.draw_cached(renderer, size, &lit, meshes)
                    .into_iter()
                    .flat_map(|(_, parts)| parts),
            );
        }
        (beneath, layers)
    }

    /// Adjusting the background: its outline, dashed.
    fn draw_background_outline(&self, overlay: &mut Frame) {
        let Some(background) = self.background else {
            return;
        };
        let corners = background.corners().map(|p| self.camera.to_screen(p));
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

    /// The shape mode's overlay: the mirror lines, the shape hovered (and
    /// the rest dimmed), the grid and guides a drag lines up with, and what
    /// the drag would do.
    fn draw_shaping(
        &self,
        overlay: &mut Frame,
        state: &State,
        bounds: Rectangle,
        cursor_pos: Option<Point>,
        pending: Option<&Pending>,
        source: Option<(VertexId, VertexId)>,
    ) {
        for &mirror in self.mirrors {
            self.draw_mirror_line(overlay, mirror, bounds, Color { a: 0.6, ..MIRROR });
        }

        let hover = cursor_pos.and_then(|p| self.hit_test(p));

        self.draw_highlights(overlay, state, pending);
        self.draw_lining_up(overlay, state, bounds, cursor_pos, hover, pending);
        self.draw_drag(overlay, state, cursor_pos, hover, pending, source);

        // Tweaking the edge loop: how far it may turn, by where it began.
        if let Some(tweak) = state.tweaking
            && tweak.what == Tweaking::LoopTurn
        {
            let content = format!("{:.0}°", self.loop_turn(state).to_degrees());
            for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
                overlay.fill_text(canvas::Text {
                    content: content.clone(),
                    position: tweak.at + Vector::new(12.0 + offset, -22.0 + offset),
                    color,
                    size: 13.0.into(),
                    ..canvas::Text::default()
                });
            }
        }
    }

    /// The outline of the shape last hovered, so it's clear what hangs
    /// together and what is loose (everything else dimmed); while dragging,
    /// where the preview puts its vertices. Moving over to another shape,
    /// the old outline fades out as the new one fades in.
    fn draw_highlights(&self, overlay: &mut Frame, state: &State, pending: Option<&Pending>) {
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
                self.draw_shape(overlay, document, shape, strength);
            }
        }
    }

    /// What a drag lines up with: the grid (holding Ctrl) and the guides
    /// (holding Shift); before pressing, as pressing then would.
    #[allow(clippy::too_many_arguments)]
    fn draw_lining_up(
        &self,
        overlay: &mut Frame,
        state: &State,
        bounds: Rectangle,
        cursor_pos: Option<Point>,
        hover: Option<Hover>,
        pending: Option<&Pending>,
    ) {
        let camera = self.camera;
        // Holding Ctrl while dragging: the grid, around what lands on it.
        let gridded = matches!(
            state.interaction,
            Interaction::Creating { .. }
                | Interaction::MovingVertex { .. }
                | Interaction::Extending { .. }
                | Interaction::LeavingFace { .. }
                | Interaction::Extruding { .. }
                | Interaction::MovingSelection { .. }
                | Interaction::Pasting { .. }
        );
        if state.modifiers.command() && gridded {
            let points = match pending {
                Some(pending) => self.placed(state, pending),
                None => cursor_pos.map(|p| camera.to_world(p)).into_iter().collect(),
            };
            self.draw_snap_grid(overlay, &points);
        }
        // Holding Ctrl or Shift before pressing, as pressing then would:
        // with Ctrl, the grid around the cursor; on blank canvas, where a
        // new triangle would start (with Shift, the guides it lines up
        // with). Over the drawing, nothing until something's dragged.
        let (grid, shift) = (state.modifiers.command(), state.modifiers.shift());
        if (grid || shift)
            && matches!(state.interaction, Interaction::Idle)
            && self.tool == Tool::Shape
            && self.shown
            && !state.lasso_armed
            && let Some(cursor) = cursor_pos
        {
            let world = camera.to_world(cursor);
            if grid {
                self.draw_snap_grid(overlay, &[world]);
            }
            if state.selection.is_empty() && hover.is_none() {
                let (start, _, near) = self.creation_start(world, grid, shift);
                let reach = bounds.width + bounds.height;
                for &guide in near
                    .iter()
                    .filter(|&&guide| self.runs_through(guide, start))
                {
                    let shape = guide.shape(self.document, camera);
                    self.draw_guide_line(overlay, guide, shape, true, reach);
                }
                // With Shift alone, only where it lines up (else it's just
                // where the cursor is).
                if grid || start.distance(world) > 1e-3 {
                    overlay.stroke(
                        &Path::circle(camera.to_screen(start), 5.0),
                        stroke(ADDED, 1.5),
                    );
                }
            }
        }
        if !state.near_guides.is_empty()
            && matches!(
                state.interaction,
                Interaction::Creating { .. }
                    | Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
                    | Interaction::Extruding { .. }
            )
        {
            self.draw_guides(overlay, state, pending, bounds);
        }
    }

    /// What the drag (or click) in hand would do: what it adds, removes and
    /// joins up, the selection, and where it lands.
    #[allow(clippy::too_many_arguments)]
    fn draw_drag(
        &self,
        overlay: &mut Frame,
        state: &State,
        cursor_pos: Option<Point>,
        hover: Option<Hover>,
        pending: Option<&Pending>,
        source: Option<(VertexId, VertexId)>,
    ) {
        let camera = self.camera;
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
        self.draw_selection(overlay, state, pending, cursor_pos);

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
                    handle(overlay, at, HOVER);
                }
                Some((_, Some(Hover::Edge { a, b, at }))) => {
                    let a = camera.to_screen(self.document.vertex(a));
                    let b = camera.to_screen(self.document.vertex(b));
                    overlay.stroke(&Path::line(a, b), stroke(HOVER, 2.5));
                    handle(overlay, camera.to_screen(at), HOVER);
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
            (Interaction::Panning { .. } | Interaction::Zooming { .. }, _) => {}
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
                handle(overlay, at, color);
            }
            // Releasing now would do nothing.
            (_, None) => {
                if let Some(p) = cursor_pos {
                    handle(overlay, p, EDGE);
                }
            }
        }
    }

    /// Draws the current layer, or what it's about to become.
    pub(super) fn draw_document(
        &self,
        frame: &mut Frame,
        document: &Document,
        meshes: Option<&mut LayerMeshes>,
        beneath: &mut Vec<[Point; 3]>,
    ) {
        let style = Style::plain(self.look());
        self.draw_layer(frame, document, style, self.mirrors, meshes, beneath);
    }

    /// The drawings of the `wanted` layers, each from its cache if what
    /// it's drawn from hasn't changed; those that changed drawn anew, side
    /// by side (on several cores).
    pub(super) fn draw_cached(
        &self,
        renderer: &Renderer,
        size: Size,
        wanted: &[Wanted<'_>],
        meshes: bool,
    ) -> Vec<(Option<Drawn>, Vec<Drawn>)> {
        let mut caches = self.caches.layers.borrow_mut();
        // What each depends on, and a frame for those to draw anew.
        let mut jobs = Vec::new();
        let mut keys = Vec::with_capacity(wanted.len());
        for (i, wanted) in wanted.iter().enumerate() {
            let Wanted {
                painter,
                layer,
                look,
            } = *wanted;
            let style = Style::of(look, layer.show_edges);
            let camera = painter.camera;
            let key = LayerKey {
                revision: layer.document.revision(),
                style,
                vertices: painter.shows_vertices(look),
                mirrors: layer.mirrors.to_vec(),
                camera: (camera.pan, camera.zoom, camera.rotation, camera.image),
                size,
                meshes,
            };
            if caches.get(&layer.id).is_none_or(|cache| cache.key != key) {
                jobs.push((i, Frame::new(renderer, size), style));
            }
            keys.push(key);
        }

        let draw = |(i, mut frame, style): (usize, Frame, Style)| {
            let Wanted { painter, layer, .. } = wanted[i];
            let mut sink = meshes.then(LayerMeshes::default);
            let mut beneath = Vec::new();
            painter.draw_layer(
                &mut frame,
                layer.document,
                style,
                layer.mirrors,
                sink.as_mut(),
                &mut beneath,
            );
            // Its checkerboard: as a mesh here; without, on the canvas
            // below (frames are made on this thread).
            let checker = (meshes && !beneath.is_empty())
                .then(|| meshes::checker(&beneath, size).into_cache(size));
            let geometry = frame.into_geometry();
            (
                i,
                geometry,
                sink.map(|sink| sink.finish(size)),
                checker,
                beneath,
            )
        };
        // One alone isn't worth handing to another thread.
        let drawn: Vec<_> = if jobs.len() > 1 {
            jobs.into_par_iter().map(draw).collect()
        } else {
            jobs.into_iter().map(draw).collect()
        };
        for (i, geometry, built, checker, beneath) in drawn {
            let id = wanted[i].layer.id;
            let beneath = match checker {
                Some(mesh) => Some(Beneath::Meshes(mesh)),
                None => draw_checker(renderer, size, &beneath, false).map(|drawn| match drawn {
                    Drawn::Geometry(geometry) => {
                        Beneath::Geometry(geometry.cache(Group::unique(), None))
                    }
                    Drawn::Meshes(mesh) => Beneath::Meshes(mesh),
                }),
            };
            let previous = caches.remove(&id);
            let group = previous
                .as_ref()
                .map_or_else(Group::unique, |cache| cache.group);
            let geometry = geometry.cache(group, previous.map(|cache| cache.geometry));
            let key = keys[i].clone();
            caches.insert(
                id,
                LayerCache {
                    key,
                    group,
                    geometry,
                    meshes: built,
                    beneath,
                },
            );
        }

        wanted
            .iter()
            .map(|wanted| {
                let cache = &caches[&wanted.layer.id];
                let geometry = Drawn::Geometry(Cached::load(&cache.geometry));
                let parts = match &cache.meshes {
                    Some((under, over)) => vec![
                        Drawn::Meshes(under.clone()),
                        geometry,
                        Drawn::Meshes(over.clone()),
                    ],
                    None => vec![geometry],
                };
                (cache.beneath.as_ref().map(Beneath::drawn), parts)
            })
            .collect()
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
        for layer in &self.lit {
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
}
