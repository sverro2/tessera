//! Drawing a layer: its faces, edges and vertices as it looks (painted,
//! plain or faded), only what's in view, and zoomed out without the detail
//! that would crowd. By a [`Painter`], which can go to other threads, so
//! layers are drawn side by side; what's worked out from a drawing to draw
//! it is remembered (see [`Memos`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::*;
use crate::document::cells::Cells;

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
    sides: OnceLock<Vec<[Side; 3]>>,
    /// Each vertex's shortest edge (by vertex id; world units).
    shortest: OnceLock<Vec<f32>>,
    /// Each triangle's colour, as [`Document::colors`] has them (looked up
    /// one by one, they take a while).
    colors: OnceLock<Vec<Option<Color>>>,
    /// The triangles by place, with the bounds of all of them, to find
    /// those in view without going through all of them; none if empty.
    index: OnceLock<Option<(Cells, Point, Point)>>,
    /// How often it was asked for: one drawn just once (a mirror image
    /// worked out anew each time) isn't worth indexing.
    uses: AtomicU32,
}

impl Derived {
    fn new() -> Self {
        Derived {
            sides: OnceLock::new(),
            shortest: OnceLock::new(),
            colors: OnceLock::new(),
            index: OnceLock::new(),
            uses: AtomicU32::new(0),
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
        let indexed = (self.uses.load(Ordering::Relaxed) > 1 && triangles.len() > 256)
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
        let near = run
            .iter()
            .map(|&(_, near, _)| near)
            .fold(f32::INFINITY, f32::min);
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

/// How a layer is drawn: its look and, in the painted look (the paint
/// mode), whether its edges show. The other looks show them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Style {
    pub(super) look: Look,
    pub(super) show_edges: bool,
}

impl Style {
    /// A layer's, showing its edges if `show_edges`, in `look`.
    pub(super) fn of(look: Look, show_edges: bool) -> Style {
        match look {
            Look::Painted => Style { look, show_edges },
            Look::Plain | Look::Faded => Style::plain(look),
        }
    }

    /// In `look`, with its edges.
    pub(super) fn plain(look: Look) -> Style {
        Style {
            look,
            show_edges: true,
        }
    }
}

/// What drawing a layer takes from the editor: how it's seen, the tool,
/// and what's remembered of the drawings. Unlike the editor (with its
/// caches of what's on the GPU), it can go to other threads, so that
/// layers can be drawn side by side.
#[derive(Clone, Copy)]
pub(super) struct Painter<'a> {
    pub(super) camera: Camera,
    tool: Tool,
    memos: &'a Memos,
}

/// What's remembered of the drawings drawn lately, by drawing (its
/// revision): shared by layers drawn side by side.
#[derive(Default)]
pub(super) struct Memos {
    /// Painted edges' outlines worked out; each drawing's apart.
    outlines: Mutex<HashMap<u64, Arc<Mutex<EdgeOutlines>>>>,
    /// What drawing them needs worked out (see [`Derived`]).
    derived: Mutex<HashMap<u64, Arc<Derived>>>,
}

/// A drawing's painted edges' outlines (world), as at `zoom`, and around
/// each vertex its painted edges (to work out more).
#[derive(Default)]
struct EdgeOutlines {
    zoom: f32,
    outlines: HashMap<(VertexId, VertexId), Vec<Point>>,
    around: HashMap<VertexId, Vec<(VertexId, VertexId)>>,
}

impl Editor<'_> {
    /// How this draws layers (see [`Painter`]).
    pub(super) fn painter(&self) -> Painter<'_> {
        Painter {
            camera: self.camera,
            tool: self.tool,
            memos: &self.caches.memos,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_layer(
        &self,
        frame: &mut Frame,
        document: &Document,
        style: Style,
        mirrors: &[Mirror],
        meshes: Option<&mut LayerMeshes>,
        beneath: &mut Vec<[Point; 3]>,
    ) {
        self.painter()
            .draw_layer(frame, document, style, mirrors, meshes, beneath);
    }

    pub(super) fn edge_width(&self, width: f32) -> f32 {
        self.painter().edge_width(width)
    }

    pub(super) fn mesh(&self, triangles: impl Iterator<Item = [Point; 3]>) -> Path {
        self.painter().mesh(triangles)
    }
}

impl Painter<'_> {
    /// Draws a layer; in the painted look its edges only if `show_edges`. With `mirrors`, mirrored: in the
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
    fn edge_outlines(&self, document: &Document, ends: &[(VertexId, VertexId)]) -> Vec<Vec<Point>> {
        let zoom = self.camera.zoom;
        let width = |a, b| {
            let style = document.edge_style(a, b).unwrap_or(doc::DEFAULT_EDGE);
            self.edge_width(style.width) / zoom
        };
        // Each drawing's apart, so layers drawn side by side don't wait for
        // each other.
        let entry = {
            let mut memo = self.memos.outlines.lock().expect("outlines");
            // Only a few drawings at a time (those drawn lately).
            if memo.len() > 64 && !memo.contains_key(&document.revision()) {
                memo.clear();
            }
            memo.entry(document.revision()).or_default().clone()
        };
        let mut known = entry.lock().expect("outlines");
        let known = &mut *known;
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
        ends.iter()
            .map(|edge| known.outlines[edge].clone())
            .collect()
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

    /// Draws a layer on `frame` (and, with them, into `meshes`); in the
    /// painted look, what's translucent of it goes into `beneath` (as it's
    /// seen, in triangles), for the checkerboard behind it (see
    /// `meshes::checker`).
    pub(super) fn draw_layer(
        &self,
        frame: &mut Frame,
        document: &Document,
        style: Style,
        mirrors: &[Mirror],
        mut meshes: Option<&mut LayerMeshes>,
        beneath: &mut Vec<[Point; 3]>,
    ) {
        let Style { look, show_edges } = style;
        if !mirrors.is_empty() {
            // Mapped so that, as this sees it, it's where it is.
            let seen = self.camera.image.unwrap_or(Affine::IDENTITY);
            let unseen = seen.inverse();
            if look == Look::Painted {
                let mirrored = document.with_mirrors(mirrors).transformed(unseen);
                self.draw_layer(frame, &mirrored, style, &[], meshes, beneath);
            } else {
                let strength = if look == Look::Faded { 0.35 } else { 1.0 };
                for image in doc::images(mirrors) {
                    if !image.near(seen) {
                        let ghost = document.transformed(image.then(unseen));
                        self.draw_hinted(frame, &ghost, strength * 0.4, 1.0, meshes.as_deref_mut());
                    }
                }
                self.draw_layer(frame, document, style, &[], meshes, beneath);
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
        let shows =
            |a: VertexId, b: VertexId| seen.shows(&[document.vertex(a), document.vertex(b)]);

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
        // What's see-through shows the checkerboard behind it.
        for &t in &visible {
            if colors[t].is_some_and(|color| color.a < 1.0) {
                beneath.push(corners(t).map(|p| self.camera.to_screen(p)));
            }
        }
        let unpainted_tint = Color { a: 0.05, ..EDGE };
        match meshes.as_deref_mut() {
            Some(meshes) => {
                for &t in &unpainted {
                    meshes
                        .under
                        .triangle(t.map(|p| self.camera.to_screen(p)), unpainted_tint);
                }
                for &t in &visible {
                    if let Some(color) = colors[t] {
                        meshes
                            .under
                            .triangle(corners(t).map(|p| self.camera.to_screen(p)), color);
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
                    let color = Color {
                        a: dashed.a * shown,
                        ..dashed
                    };
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
            .map(|outline| {
                outline
                    .into_iter()
                    .map(|p| self.camera.to_screen(p))
                    .collect()
            })
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
        let mut by_color = ByColor::default();
        for (&(a, b, _, color), outline) in edges.iter().zip(&outlines) {
            // The edge's middle, which sees all its outline.
            let middle = Point::new(
                (screen(a).x + screen(b).x) / 2.0,
                (screen(a).y + screen(b).y) / 2.0,
            );
            // See-through: the checkerboard behind.
            if color.a < 1.0 {
                fan_triangles(middle, outline, beneath);
            }
            match meshes.as_deref_mut() {
                // A fan round the edge's middle.
                Some(meshes) => meshes.over.fan(middle, outline, color),
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
                (0..3)
                    .filter(move |&k| sides[t][k].owns_start)
                    .map(move |k| corners[k])
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
    pub(super) fn derived(&self, document: &Document) -> Arc<Derived> {
        let mut memo = self.memos.derived.lock().expect("derived");
        // Only a few drawings at a time (those drawn lately).
        if memo.len() > 64 && !memo.contains_key(&document.revision()) {
            memo.clear();
        }
        let derived = memo
            .entry(document.revision())
            .or_insert_with(|| Arc::new(Derived::new()))
            .clone();
        derived.uses.fetch_add(1, Ordering::Relaxed);
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

/// The polygon through `outline` (screen) as a fan of triangles round
/// `centre` (from where all of it is seen), into `out`.
fn fan_triangles(centre: Point, outline: &[Point], out: &mut Vec<[Point; 3]>) {
    for (i, &p) in outline.iter().enumerate() {
        out.push([centre, p, outline[(i + 1) % outline.len()]]);
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
            points.extend([
                Point::new(x, 0.0),
                Point::new(x + 10.0, 0.0),
                Point::new(x + 5.0, 10.0),
            ]);
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
        derived.uses.store(1, Ordering::Relaxed);
        assert_eq!(derived.visible(&document, &seen), all);
        derived.uses.store(2, Ordering::Relaxed);
        assert_eq!(derived.visible(&document, &seen), all);
        assert!(derived.index.get().is_some_and(Option::is_some));
    }
}
