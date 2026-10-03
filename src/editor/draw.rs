//! Drawing the canvas: the layers (each cached), and over them what a
//! drag would do.

use std::cell::OnceCell;

use super::*;
use crate::document::cells::Cells;

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

/// The dashes of a line from `a` to `b`: `on` long, `off` apart, from `a`.
/// Drawn ourselves, as a dashed stroke dashes a path as one line: from the
/// end of one of its pieces right on to the start of the next.
pub(super) fn dashes(a: Point, b: Point, on: f32, off: f32) -> Vec<(Point, Point)> {
    let length = a.distance(b);
    if length == 0.0 {
        return Vec::new();
    }
    let along = (b - a) * (1.0 / length);
    let mut dashes = Vec::new();
    let mut at = 0.0;
    while at < length {
        dashes.push((a + along * at, a + along * (at + on).min(length)));
        at += on + off;
    }
    dashes
}

/// Lines shorter than this (screen px) aren't drawn: zoomed out so far,
/// they'd crowd into a blur, too close together to tell apart or grab.
const DETAIL_HIDDEN: f32 = 3.0;
/// Lines at least this long (screen px) are drawn fully; shorter ones
/// fade out, so zooming out thins them gradually.
const DETAIL_SHOWN: f32 = 8.0;

/// How much of a line `length` long (screen px) to draw: from none (too
/// short to make out) to all of it.
pub(super) fn detail(length: f32) -> f32 {
    let t = ((length - DETAIL_HIDDEN) / (DETAIL_SHOWN - DETAIL_HIDDEN)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A side of a triangle (`ab`, `bc` or `ca`), as drawing it needs.
#[derive(Clone, Copy)]
pub(super) struct Side {
    /// How crowded it is (world units: times the zoom, on screen; see
    /// [`detail`]): by its length or by how close the edges beside it
    /// are, whichever is less. A sliver's long sides lie as close
    /// together as it's thin (its height across them), crowding as much as
    /// short edges do.
    near: f32,
    /// On the drawing's outline (used by this triangle only), which the
    /// plain look keeps however far out, so the shape stays.
    outline: bool,
    /// The first triangle (lowest id) with this edge: so each edge is
    /// drawn once. Whenever an edge may show, so may each triangle with it
    /// (it's within their bounds), so going through those that may show
    /// finds every edge that may.
    owns_edge: bool,
    /// The first triangle with the vertex it starts at, likewise.
    owns_start: bool,
    /// Its edge is painted.
    painted: bool,
}

/// What drawing a drawing needs worked out from it, beyond what it says:
/// remembered while it's drawn (by its revision), and each part only once
/// it's needed.
pub(super) struct Derived {
    /// Each triangle's sides.
    sides: OnceCell<Vec<[Side; 3]>>,
    /// Each vertex's shortest edge (by vertex id; world units).
    shortest: OnceCell<Vec<f32>>,
    /// Each triangle's colour, as [`Document::colors`] has them (looked up
    /// one by one, they take a while).
    colors: OnceCell<Vec<Option<Color>>>,
    /// The triangles by place, with the bounds of all of them, to find
    /// those in view without going through all of them; none if empty.
    index: OnceCell<Option<(Cells, Point, Point)>>,
    /// How often it was asked for: one drawn just once (a mirror image
    /// worked out anew each time) isn't worth indexing.
    uses: Cell<u32>,
}

impl Derived {
    fn new() -> Self {
        Derived {
            sides: OnceCell::new(),
            shortest: OnceCell::new(),
            colors: OnceCell::new(),
            index: OnceCell::new(),
            uses: Cell::new(0),
        }
    }

    pub(super) fn sides(&self, document: &Document) -> &[[Side; 3]] {
        self.sides.get_or_init(|| sides_of(document))
    }

    pub(super) fn shortest(&self, document: &Document) -> &[f32] {
        self.shortest.get_or_init(|| {
            let mut shortest = vec![f32::INFINITY; document.vertex_slots()];
            for &[a, b, c] in document.triangle_ids() {
                for (u, v) in [(a, b), (b, c), (c, a)] {
                    let length = document.vertex(u).distance(document.vertex(v));
                    shortest[u] = shortest[u].min(length);
                    shortest[v] = shortest[v].min(length);
                }
            }
            shortest
        })
    }

    pub(super) fn colors(&self, document: &Document) -> &[Option<Color>] {
        self.colors.get_or_init(|| document.colors().collect())
    }

    /// The triangles that may show in `seen`, in order. When it's only a
    /// little of the drawing, found by place.
    pub(super) fn visible(&self, document: &Document, seen: &View) -> Vec<TriangleId> {
        let triangles = document.triangle_ids();
        let shows = |t: TriangleId| seen.shows(&triangles[t].map(|v| document.vertex(v)));
        let area = |min: Point, max: Point| (max.x - min.x).max(0.0) * (max.y - min.y).max(0.0);
        // Drawn more than once, and big enough to bother.
        let indexed = (self.uses.get() > 1 && triangles.len() > 256)
            .then(|| {
                self.index.get_or_init(|| {
                    let (min, max) = document.bounds()?;
                    Some((Cells::of_shapes(document.triangles()), min, max))
                })
            })
            .and_then(Option::as_ref);
        match indexed {
            // A small part of it in view.
            Some((cells, min, max)) if 4.0 * area(seen.min, seen.max) < area(*min, *max) => {
                let mut found: Vec<TriangleId> = cells.within(seen.min, seen.max).collect();
                found.sort_unstable();
                found.dedup();
                found.retain(|&t| shows(t));
                found
            }
            _ => (0..triangles.len()).filter(|&t| shows(t)).collect(),
        }
    }
}

/// Each triangle's sides of `document`.
fn sides_of(document: &Document) -> Vec<[Side; 3]> {
    let triangles = document.triangle_ids();
    let mut first_with = vec![usize::MAX; document.vertex_slots()];
    // Each side of each triangle: its edge, how crowded, and where it is.
    let mut all = Vec::with_capacity(triangles.len() * 3);
    for (t, &[a, b, c]) in triangles.iter().enumerate() {
        let [pa, pb, pc] = [a, b, c].map(|v| document.vertex(v));
        let area2 = ((pb - pa).x * (pc - pa).y - (pb - pa).y * (pc - pa).x).abs();
        for (k, ((u, v), (p, q))) in [((a, b), (pa, pb)), ((b, c), (pb, pc)), ((c, a), (pc, pa))]
            .into_iter()
            .enumerate()
        {
            let length = p.distance(q);
            // Its height across this edge.
            let height = if length > 0.0 { area2 / length } else { 0.0 };
            all.push(((u.min(v), u.max(v)), length.min(height), t * 3 + k));
        }
        for v in [a, b, c] {
            if first_with[v] == usize::MAX {
                first_with[v] = t;
            }
        }
    }
    let unset = Side {
        near: f32::INFINITY,
        outline: false,
        owns_edge: false,
        owns_start: false,
        painted: false,
    };
    let mut sides = vec![[unset; 3]; triangles.len()];
    all.sort_unstable_by_key(|&(edge, ..)| edge);
    for run in all.chunk_by(|x, y| x.0 == y.0) {
        let (u, v) = run[0].0;
        let near = run.iter().map(|&(_, near, _)| near).fold(f32::INFINITY, f32::min);
        let owner = run.iter().map(|&(.., at)| at).min().expect("a side");
        let painted = document.edge_style(u, v).is_some();
        for &(_, _, at) in run {
            let (t, k) = (at / 3, at % 3);
            let start = triangles[t][k];
            sides[t][k] = Side {
                near,
                outline: run.len() == 1,
                owns_edge: at == owner,
                owns_start: first_with[start] == t,
                painted,
            };
        }
    }
    sides
}

/// Of `visible` triangles (with these `colors`), the painted ones by
/// colour.
pub(super) fn painted_faces(
    document: &Document,
    visible: &[TriangleId],
    colors: &[Option<Color>],
) -> Vec<(Color, Vec<[Point; 3]>)> {
    let triangles = document.triangle_ids();
    let mut groups = ByColor::default();
    for &t in visible {
        if let Some(color) = colors[t] {
            groups.add(color, triangles[t].map(|v| document.vertex(v)));
        }
    }
    groups.0
}

/// Things grouped by colour, each colour once (in the order they came),
/// to draw each colour's in one go.
pub(super) struct ByColor<T>(pub Vec<(Color, Vec<T>)>, HashMap<[u32; 4], usize>);

impl<T> Default for ByColor<T> {
    fn default() -> Self {
        ByColor(Vec::new(), HashMap::new())
    }
}

impl<T> ByColor<T> {
    pub(super) fn add(&mut self, color: Color, thing: T) {
        let key = [color.r, color.g, color.b, color.a].map(f32::to_bits);
        let groups = &mut self.0;
        let i = *self.1.entry(key).or_insert_with(|| {
            groups.push((color, Vec::new()));
            groups.len() - 1
        });
        groups[i].1.push(thing);
    }
}

/// The part of the world in view (with a margin): what's wholly outside it
/// needn't be drawn.
pub(super) struct View {
    min: Point,
    max: Point,
}

impl View {
    /// Whether something with these corners (world) may show.
    pub(super) fn shows(&self, corners: &[Point]) -> bool {
        let (mut min, mut max) = (corners[0], corners[0]);
        for p in &corners[1..] {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
        min.x <= self.max.x && self.min.x <= max.x && min.y <= self.max.y && self.min.y <= max.y
    }
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
        meshes: bool,
    ) -> Vec<Drawn> {
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
            others_view.draw_cached(renderer, size, *layer, others, &mut layers, meshes);
        }
        let mut above = Vec::new();
        for layer in &self.above {
            others_view.draw_cached(renderer, size, *layer, others, &mut above, meshes);
        }
        if look != Look::Painted {
            layers.append(&mut above);
        }
        if self.shown {
            match &pending {
                Some(pending) => {
                    let mut sink = meshes.then(LayerMeshes::default);
                    let mut frame = Frame::new(renderer, size);
                    self.draw_document(&mut frame, &pending.result, sink.as_mut());
                    push_layer(&mut layers, frame, sink, size);
                    let mut changes = Frame::new(renderer, size);
                    self.draw_changes(&mut changes, pending, source);
                    layers.push(Drawn::Geometry(changes.into_geometry()));
                }
                // Tweaking a painted edge (holding W or C): the layer as
                // it will be, that edge in its new style only, so a wide one
                // made thin doesn't show from under it.
                None if let Some(tweaked) = self.tweaked_edge(state) => {
                    let mut sink = meshes.then(LayerMeshes::default);
                    let mut frame = Frame::new(renderer, size);
                    self.draw_layer(
                        &mut frame,
                        &tweaked,
                        look,
                        self.crossfade,
                        self.show_edges,
                        self.mirrors,
                        sink.as_mut(),
                    );
                    push_layer(&mut layers, frame, sink, size);
                }
                None => {
                    let current = SceneLayer {
                        id: self.current,
                        document: self.document,
                        crossfade: self.crossfade,
                        show_edges: self.show_edges,
                        mirrors: self.mirrors,
                    };
                    self.draw_cached(renderer, size, current, look, &mut layers, meshes);
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
            for &layer in &self.lit {
                self.draw_cached(renderer, size, layer, look, &mut layers, meshes);
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
                content: "The current layer is out of view (hidden, or others isolated): show it to edit it".into(),
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
            self.draw_snap_grid(&mut overlay, &points);
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
                self.draw_snap_grid(&mut overlay, &[world]);
            }
            if state.selection.is_empty() && hover.is_none() {
                let (start, _, near) = self.creation_start(world, grid, shift);
                let reach = bounds.width + bounds.height;
                for &guide in near.iter().filter(|&&guide| self.runs_through(guide, start)) {
                    let shape = guide.shape(self.document, camera);
                    self.draw_guide_line(&mut overlay, guide, shape, true, reach);
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
    pub(super) fn draw_document(
        &self,
        frame: &mut Frame,
        document: &Document,
        meshes: Option<&mut LayerMeshes>,
    ) {
        self.draw_layer(
            frame,
            document,
            self.look(),
            Crossfade::default(),
            true,
            self.mirrors,
            meshes,
        );
    }

    /// Draws a layer; in the painted look crossfaded as `crossfade` says,
    /// and its edges only if `show_edges`. With `mirrors`, mirrored: in the
    /// painted look as it will be, otherwise its mirror images faint, so
    /// it's clear they're not there to edit. (Seen through one of them,
    /// that one's drawn as the drawing, and the drawing faint.)
    /// What of the world shows in `frame`, and as far out as `margin`
    /// (screen px) past its sides: a stroke's width, say.
    pub(super) fn view(&self, frame: &Frame, margin: f32) -> View {
        let size = frame.size();
        let corners = [
            Point::new(-margin, -margin),
            Point::new(size.width + margin, -margin),
            Point::new(-margin, size.height + margin),
            Point::new(size.width + margin, size.height + margin),
        ]
        .map(|p| self.camera.to_world(p));
        let (mut min, mut max) = (corners[0], corners[0]);
        for p in &corners[1..] {
            min = Point::new(min.x.min(p.x), min.y.min(p.y));
            max = Point::new(max.x.max(p.x), max.y.max(p.y));
        }
        View { min, max }
    }

    /// The outlines (world) of `document`'s painted edges `ends` (see
    /// [`joints`]), their widths as drawn at this zoom. Remembered per
    /// drawing and zoom, so panning needn't work them out again; those not
    /// yet are, with the edges they meet, so they join up as they should.
    fn edge_outlines(
        &self,
        document: &Document,
        ends: &[(VertexId, VertexId)],
    ) -> Vec<Vec<Point>> {
        let zoom = self.camera.zoom;
        let width = |a, b| {
            let style = document.edge_style(a, b).unwrap_or(doc::DEFAULT_EDGE);
            self.edge_width(style.width) / zoom
        };
        let mut memo = self.caches.outlines.borrow_mut();
        // Only a few drawings at a time (those drawn lately).
        if memo.len() > 64 && !memo.contains_key(&document.revision()) {
            memo.clear();
        }
        let known = memo.entry(document.revision()).or_default();
        if known.zoom != zoom {
            known.zoom = zoom;
            known.outlines.clear();
        }
        let missing: Vec<(VertexId, VertexId)> = ends
            .iter()
            .copied()
            .filter(|edge| !known.outlines.contains_key(edge))
            .collect();
        if !missing.is_empty() {
            if known.around.is_empty() {
                for (a, b, _) in document.edge_styles() {
                    known.around.entry(a).or_default().push((a, b));
                    known.around.entry(b).or_default().push((a, b));
                }
            }
            // With the painted edges they meet, each once.
            let mut list = missing.clone();
            let mut listed: std::collections::HashSet<_> = missing.iter().copied().collect();
            for &(a, b) in &missing {
                for v in [a, b] {
                    for &edge in known.around.get(&v).into_iter().flatten() {
                        if listed.insert(edge) {
                            list.push(edge);
                        }
                    }
                }
            }
            let widths: Vec<_> = list.iter().map(|&(a, b)| (a, b, width(a, b))).collect();
            let outlines = joints::outlines(&widths, |v| document.vertex(v));
            for (edge, outline) in list.into_iter().zip(outlines).take(missing.len()) {
                known.outlines.insert(edge, outline);
            }
        }
        ends.iter().map(|edge| known.outlines[edge].clone()).collect()
    }

    /// How far (screen px) `document`'s painted edges may reach past what
    /// they join: the widest of them, and some.
    fn edge_reach(&self, document: &Document) -> f32 {
        let widest = document
            .edge_styles()
            .map(|(_, _, style)| self.edge_width(style.width))
            .fold(0.0, f32::max);
        2.0 * widest + 4.0
    }

    pub(super) fn draw_layer(
        &self,
        frame: &mut Frame,
        document: &Document,
        look: Look,
        crossfade: Crossfade,
        show_edges: bool,
        mirrors: &[Mirror],
        mut meshes: Option<&mut LayerMeshes>,
    ) {
        if !mirrors.is_empty() {
            // Mapped so that, as this sees it, it's where it is.
            let seen = self.camera.image.unwrap_or(Affine::IDENTITY);
            let unseen = seen.inverse();
            if look == Look::Painted {
                let mirrored = document.with_mirrors(mirrors).transformed(unseen);
                self.draw_layer(frame, &mirrored, look, crossfade, show_edges, &[], meshes);
            } else {
                let strength = if look == Look::Faded { 0.35 } else { 1.0 };
                for image in doc::images(mirrors) {
                    if !image.near(seen) {
                        let ghost = document.transformed(image.then(unseen));
                        self.draw_hinted(
                            frame,
                            &ghost,
                            strength * 0.4,
                            1.0,
                            meshes.as_deref_mut(),
                        );
                    }
                }
                self.draw_layer(frame, document, look, crossfade, show_edges, &[], meshes);
            }
            return;
        }
        match look {
            Look::Painted => {}
            // Both with a hint of the colours, to still tell what's
            // painted; other layers fainter.
            Look::Plain => {
                self.draw_hinted(frame, document, 1.0, 1.5, meshes.as_deref_mut());
                if self.shows_vertices(look) {
                    self.draw_vertices(frame, document, meshes);
                }
                return;
            }
            Look::Faded => {
                self.draw_hinted(frame, document, 0.35, 1.0, meshes);
                return;
            }
        }

        // Only what may show: past the sides as far as a painted edge may
        // reach.
        let seen = self.view(frame, self.edge_reach(document));
        let shows = |a: VertexId, b: VertexId| seen.shows(&[document.vertex(a), document.vertex(b)]);

        // Unpainted faces hatched, as they're not exported: plainly not
        // painted (and not some colour). Then the painted ones by colour.
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let derived = self.derived(document);
        let visible = derived.visible(document, &seen);
        let colors = derived.colors(document);
        let corners = |t: TriangleId| document.triangle_ids()[t].map(|v| document.vertex(v));
        let unpainted: Vec<_> = visible
            .iter()
            .filter(|&&t| colors[t].is_none())
            .map(|&t| corners(t))
            .collect();
        let unpainted_tint = Color { a: 0.05, ..EDGE };
        match meshes.as_deref_mut() {
            Some(meshes) => {
                for &t in &unpainted {
                    meshes.under.triangle(t.map(|p| self.camera.to_screen(p)), unpainted_tint);
                }
                for &t in &visible {
                    if let Some(color) = colors[t] {
                        meshes.under.triangle(corners(t).map(|p| self.camera.to_screen(p)), color);
                    }
                }
            }
            None => {
                frame.fill(&self.mesh(unpainted.iter().copied()), unpainted_tint);
                for (color, triangles) in painted_faces(document, &visible, colors) {
                    frame.fill(&self.mesh(triangles.into_iter()), color);
                }
            }
        }
        frame.stroke(
            &self.hatch(&unpainted),
            stroke(Color { a: 0.3, ..EDGE }, 1.0),
        );
        if !show_edges {
            return;
        }

        // Unpainted edges dashed, thin: not exported either. Each edge's
        // dashes its own (see `dashes`).
        // Zoomed out, short ones fade away (see `detail`).
        let dashed = Color { a: 0.5, ..EDGE };
        let zoom = self.camera.zoom;
        let sides = derived.sides(document);
        let mut unpainted = Vec::new();
        for &t in &visible {
            let [a, b, c] = document.triangle_ids()[t];
            for ((u, v), side) in [(a, b), (b, c), (c, a)].into_iter().zip(sides[t]) {
                let shown = detail(side.near * zoom);
                if side.owns_edge && !side.painted && shown > 0.0 && shows(u, v) {
                    // From its lowest end, so its dashes stay put.
                    let (u, v) = (u.min(v), u.max(v));
                    unpainted.push((screen(u), screen(v), shown));
                }
            }
        }
        match meshes.as_deref_mut() {
            // Over the faces, under the painted edges.
            Some(meshes) => {
                for (a, b, shown) in unpainted {
                    let color = Color { a: dashed.a * shown, ..dashed };
                    for (from, to) in dashes(a, b, 4.0, 4.0) {
                        meshes.under.line(from, to, 1.0, color);
                    }
                }
            }
            // One colour for all: those mostly faded, left out.
            None => {
                let path = Path::new(|p| {
                    for (a, b, shown) in unpainted {
                        if shown >= 0.5 {
                            for (from, to) in dashes(a, b, 4.0, 4.0) {
                                p.move_to(from);
                                p.line_to(to);
                            }
                        }
                    }
                });
                frame.stroke(&path, stroke(dashed, 1.0));
            }
        }

        // The painted edges, each its own outline, so edges of different
        // widths meet without lying over each other.
        let edges: Vec<_> = document
            .edge_styles()
            .filter(|&(a, b, _)| shows(a, b))
            .map(|(a, b, style)| (a, b, self.edge_width(style.width), style.color))
            .collect();
        let ends: Vec<(VertexId, VertexId)> = edges.iter().map(|&(a, b, ..)| (a, b)).collect();
        let outlines: Vec<Vec<Point>> = self
            .edge_outlines(document, &ends)
            .into_iter()
            .map(|outline| outline.into_iter().map(|p| self.camera.to_screen(p)).collect())
            .collect();
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
        let mut by_color = ByColor::default();
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
            match meshes.as_deref_mut() {
                // A fan round the edge's middle, which sees all its outline.
                Some(meshes) => {
                    let middle = Point::new(
                        (screen(a).x + screen(b).x) / 2.0,
                        (screen(a).y + screen(b).y) / 2.0,
                    );
                    meshes.over.fan(middle, outline, color);
                }
                None => by_color.add(color, outline),
            }
        }
        for (color, outlines) in by_color.0 {
            frame.fill(&path(&outlines), color);
        }
    }

    /// Whether the vertices are drawn: in the shape mode, on the layer
    /// being shaped (they're what the shape hangs on).
    pub(super) fn shows_vertices(&self, look: Look) -> bool {
        self.tool == Tool::Shape && look == Look::Plain
    }

    /// A dot on each vertex (over everything else, with meshes). Zoomed
    /// out, those crowding their neighbours fade away (see `detail`): by
    /// their shortest edge, as long as two dots are wide.
    pub(super) fn draw_vertices(
        &self,
        frame: &mut Frame,
        document: &Document,
        meshes: Option<&mut LayerMeshes>,
    ) {
        let seen = self.view(frame, 4.0);
        let screen = |v| self.camera.to_screen(document.vertex(v));
        let derived = self.derived(document);
        let (sides, shortest) = (derived.sides(document), derived.shortest(document));
        let zoom = self.camera.zoom;
        // Each vertex once: by the first triangle with it.
        let dots = derived
            .visible(document, &seen)
            .into_iter()
            .flat_map(|t| {
                let corners = document.triangle_ids()[t];
                (0..3).filter(move |&k| sides[t][k].owns_start).map(move |k| corners[k])
            })
            .filter(|&v| seen.shows(&[document.vertex(v)]))
            .map(|v| (screen(v), detail(shortest[v] * zoom / 2.0)))
            .filter(|&(_, shown)| shown > 0.0);
        match meshes {
            Some(meshes) => {
                for (dot, shown) in dots {
                    meshes.over.dot(dot, 2.5, Color { a: shown, ..EDGE });
                }
            }
            None => {
                let path = Path::new(|p| {
                    for (dot, shown) in dots {
                        if shown >= 0.5 {
                            p.circle(dot, 2.5);
                        }
                    }
                });
                frame.fill(&path, EDGE);
            }
        }
    }

    /// What drawing `document` needs worked out, remembered per drawing.
    pub(super) fn derived(&self, document: &Document) -> Rc<Derived> {
        let mut memo = self.caches.derived.borrow_mut();
        // Only a few drawings at a time (those drawn lately).
        if memo.len() > 64 && !memo.contains_key(&document.revision()) {
            memo.clear();
        }
        let derived = memo
            .entry(document.revision())
            .or_insert_with(|| Rc::new(Derived::new()))
            .clone();
        derived.uses.set(derived.uses.get().saturating_add(1));
        derived
    }

    /// The plain look, `strength` strong (1: full), edges `width` wide, with
    /// a hint of the face colours and edge colours.
    pub(super) fn draw_hinted(
        &self,
        frame: &mut Frame,
        document: &Document,
        strength: f32,
        width: f32,
        mut meshes: Option<&mut LayerMeshes>,
    ) {
        let faint = |color: Color, alpha: f32| Color {
            a: color.a * alpha * strength,
            ..color
        };
        // Only what may show.
        let seen = self.view(frame, width + 2.0);
        let screen = |v| self.camera.to_screen(document.vertex(v));
        // Each visible triangle's edges, on screen, and how much of each to
        // draw: zoomed out, crowded ones fade away (see `Spacing`), but not
        // the outline, so the shape stays.
        let derived = self.derived(document);
        let visible = derived.visible(document, &seen);
        let colors = derived.colors(document);
        let corners = |t: TriangleId| document.triangle_ids()[t].map(|v| document.vertex(v));
        let wireframe = || {
            let sides = derived.sides(document);
            let zoom = self.camera.zoom;
            let mut lines = Vec::new();
            for &t in &visible {
                let [a, b, c] = document.triangle_ids()[t];
                for ((u, v), side) in [(a, b), (b, c), (c, a)].into_iter().zip(sides[t]) {
                    let shown = if side.outline {
                        1.0
                    } else {
                        detail(side.near * zoom)
                    };
                    if shown > 0.0 {
                        lines.push((screen(u), screen(v), shown));
                    }
                }
            }
            lines
        };
        match meshes.as_deref_mut() {
            Some(meshes) => {
                for &t in &visible {
                    let on_screen = corners(t).map(|p| self.camera.to_screen(p));
                    meshes.under.triangle(on_screen, faint(FILL, 1.0));
                    if let Some(color) = colors[t] {
                        meshes.under.triangle(on_screen, faint(color, 0.25));
                    }
                }
                // Each triangle outlined, over all the faces (shared edges
                // twice, as stroking the triangles does).
                for (from, to, shown) in wireframe() {
                    meshes.under.line(from, to, width, faint(EDGE, shown));
                }
            }
            None => {
                let mesh = self.mesh(visible.iter().map(|&t| corners(t)));
                frame.fill(&mesh, faint(FILL, 1.0));
                for (color, triangles) in painted_faces(document, &visible, colors) {
                    frame.fill(&self.mesh(triangles.into_iter()), faint(color, 0.25));
                }
                // One colour for all: those mostly faded, left out.
                let lines = Path::new(|p| {
                    for (from, to, shown) in wireframe() {
                        if shown >= 0.5 {
                            p.move_to(from);
                            p.line_to(to);
                        }
                    }
                });
                frame.stroke(&lines, stroke(faint(EDGE, 1.0), width));
            }
        }
        // Each colour's edges as one path.
        let mut by_color = ByColor::default();
        for (a, b, style) in document.edge_styles() {
            if seen.shows(&[document.vertex(a), document.vertex(b)]) {
                by_color.add(style.color, (a, b));
            }
        }
        for (color, edges) in by_color.0 {
            let color = faint(color, 0.5);
            match meshes.as_deref_mut() {
                // Over the outlines.
                Some(meshes) => {
                    for (a, b) in edges {
                        meshes.under.line(screen(a), screen(b), width, color);
                    }
                }
                None => {
                    let path = Path::new(|p| {
                        for (a, b) in edges {
                            p.move_to(screen(a));
                            p.line_to(screen(b));
                        }
                    });
                    frame.stroke(&path, stroke(color, width));
                }
            }
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
        layers: &mut Vec<Drawn>,
        meshes: bool,
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
            meshes,
        };
        let mut caches = self.caches.layers.borrow_mut();
        let cache = caches.entry(layer.id).or_insert_with(|| LayerCache {
            key: None,
            cache: canvas::Cache::new(),
            meshes: None,
        });
        if cache.key.as_ref() != Some(&key) {
            cache.key = Some(key);
            cache.cache.clear();
            cache.meshes = None;
        }
        // Drawn again (the canvas's cache was cleared): its meshes too.
        let mut built = None;
        let geometry = cache.cache.draw(renderer, size, |frame| {
            let mut sink = meshes.then(LayerMeshes::default);
            self.draw_layer(
                frame,
                layer.document,
                look,
                crossfade,
                show_edges,
                layer.mirrors,
                sink.as_mut(),
            );
            built = sink.map(|sink| sink.finish(size));
        });
        if built.is_some() {
            cache.meshes = built;
        }
        match &cache.meshes {
            Some((under, over)) => {
                layers.push(Drawn::Meshes(under.clone()));
                layers.push(Drawn::Geometry(geometry));
                layers.push(Drawn::Meshes(over.clone()));
            }
            None => layers.push(Drawn::Geometry(geometry)),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slivers_crowd_as_much_as_they_are_thin() {
        // A sliver 100 long and 2 thick, beside a fat triangle on its long
        // side (0 1).
        let points = [(0.0, 0.0), (100.0, 0.0), (50.0, 2.0), (50.0, -80.0)];
        let document = Document::from_parts(
            points.map(|(x, y)| Point::new(x, y)).to_vec(),
            vec![[0, 1, 2], [1, 0, 3]],
        )
        .unwrap();
        let sides = sides_of(&document);
        // Shared: as crowded as on the sliver's side, on both; drawn once.
        let shared = sides[0][0].near;
        assert!((shared - 2.0).abs() < 0.01, "{shared}");
        assert_eq!(sides[1][0].near, shared);
        assert!(sides[0][0].owns_edge && !sides[1][0].owns_edge);
        assert!(!sides[0][0].outline && sides[1][1].outline);
        // Each vertex once: 3 by the first triangle, only to the second.
        assert!(sides[0].iter().all(|side| side.owns_start));
        assert!(sides[1][2].owns_start && !sides[1][0].owns_start);
        // Thinned out zoomed out (2 px apart), not zoomed in (20 px).
        assert_eq!(detail(shared), 0.0);
        assert_eq!(detail(shared * 10.0), 1.0);
    }

    #[test]
    fn only_triangles_in_view_are_drawn() {
        // A row of 300 apart, 10 wide each.
        let mut points = Vec::new();
        let mut triangles = Vec::new();
        for i in 0..300 {
            let x = i as f32 * 20.0;
            let at = points.len();
            points.extend([Point::new(x, 0.0), Point::new(x + 10.0, 0.0), Point::new(x + 5.0, 10.0)]);
            triangles.push([at, at + 1, at + 2]);
        }
        let document = Document::from_parts(points, triangles).unwrap();
        let derived = Derived::new();
        let seen = View {
            min: Point::new(95.0, -1.0),
            max: Point::new(205.0, 1.0),
        };
        let all: Vec<_> = (0..300)
            .filter(|&t| seen.shows(&document.triangle_ids()[t].map(|v| document.vertex(v))))
            .collect();
        assert_eq!(all, (5..=10).collect::<Vec<_>>());
        // By going through them all, then (drawn again) by place: the same.
        derived.uses.set(1);
        assert_eq!(derived.visible(&document, &seen), all);
        derived.uses.set(2);
        assert_eq!(derived.visible(&document, &seen), all);
        assert!(derived.index.get().is_some_and(Option::is_some));
    }
}
