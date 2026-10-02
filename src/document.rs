//! The vector document: a triangle mesh with shared vertices.
//!
//! This module knows nothing about rendering or input. All mutations go
//! through [`Edit`] so they can later be recorded for undo/redo or saving.
//!
//! After every edit, the connectivity is made to follow the geometry (see
//! [`Document::normalize`]): vertices at the same spot are one vertex, a
//! vertex lying on an edge splits it, and flat triangles are dropped. So
//! edits only need to say where things go; joining up follows from that.
//!
//! Invariants, checked after every edit (violating edits are rejected):
//! - every triangle has positive winding; one whose winding flips has been
//!   folded over its neighbours,
//! - no two triangles overlap.
//!
//! Faces can be painted a colour, and edges a colour and width. A face
//! keeps its colour while it keeps its corners (even as they move); a face
//! an edit makes anew takes the colour of the face it lay in before, so
//! pieces of a split face keep its colour. Likewise an edge keeps its style
//! while it keeps its ends, and pieces of a styled edge (cut, say) keep it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iced::{Color, Point};

use crate::geometry::{Affine, area2, min_height, overlap};

mod fill;

/// Index of a vertex in [`Document::vertices`].
pub type VertexId = usize;

/// Index of a triangle in [`Document::triangle_ids`]. Not stable across
/// edits that remove triangles.
pub type TriangleId = usize;

#[derive(Debug, Clone)]
pub struct Document {
    vertices: Vec<Point>,
    triangles: Vec<[VertexId; 3]>,
    /// The painted faces' colours, by their corners (sorted, see [`key`]).
    colors: BTreeMap<[VertexId; 3], Color>,
    /// The painted edges' styles, by their ends (lowest first).
    edge_styles: BTreeMap<(VertexId, VertexId), EdgeStyle>,
    /// Changes with every edit, and differs between documents, so views
    /// can tell when to redo what they derived from it.
    revision: u64,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            vertices: Vec::new(),
            triangles: Vec::new(),
            colors: BTreeMap::new(),
            edge_styles: BTreeMap::new(),
            revision: next_revision(),
        }
    }
}

/// A revision number no document has had yet.
pub(crate) fn next_revision() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// How a painted edge is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeStyle {
    pub color: Color,
    /// In world units.
    pub width: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Add a triangle. Where it runs into existing geometry only the
    /// uncovered part is added, joined up with the mesh; corners on
    /// existing vertices use them (e.g. extending an edge). Within `snap`
    /// (world units) things count as touching, so no thinner pieces are
    /// made; see [`Document::fill`].
    AddTriangle { corners: [Point; 3], snap: f32 },
    /// Add a vertex at `at`, splitting the triangle it lies in into three,
    /// or the triangles on the edge it lies on into two each.
    InsertVertex { at: Point },
    /// Remove a triangle, leaving a hole (or a smaller outline).
    RemoveTriangle { triangle: TriangleId },
    /// Move a vertex (and thereby every triangle using it). Where the move
    /// would fold a triangle over, the mesh is rearranged around the vertex
    /// instead; see [`Document::move_vertex`]. Moved onto another vertex it
    /// welds with it; onto an edge, it splits that edge.
    MoveVertex { id: VertexId, to: Point },
    /// Move several vertices at once, each to where it's paired with (e.g.
    /// a selection moved or rotated together). Unlike moving them one by
    /// one, triangles between them don't fold over on the way; but nothing
    /// is rearranged either: a move that folds a triangle over is rejected.
    /// Landing on other vertices or edges joins them up as usual.
    MoveVertices { moves: Vec<(VertexId, Point)> },
    /// Add the drawing's mirror image across `mirror`, joined up with it
    /// where they meet (on the mirror line). Rejected if the two overlap.
    Mirror { mirror: Mirror },
    /// Add a piece of drawing as it is (pasted), joined up where its
    /// corners land on vertices. Rejected if it overlaps anything.
    Paste { piece: Piece },
    /// Paint a face this colour; `None` clears it.
    Paint {
        triangle: TriangleId,
        color: Option<Color>,
    },
    /// Paint edge `a`–`b` this style; `None` clears it.
    PaintEdge {
        a: VertexId,
        b: VertexId,
        style: Option<EdgeStyle>,
    },
    /// Paint every face this colour; `None` clears them all.
    PaintAll { color: Option<Color> },
    /// Paint every edge this style; `None` clears them all.
    PaintAllEdges { style: Option<EdgeStyle> },
}

/// A piece of a drawing, apart from it (copied): its triangles' corners
/// (wound positively), each face's colour, and its painted edges' styles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Piece {
    pub vertices: Vec<Point>,
    pub triangles: Vec<[usize; 3]>,
    pub colors: Vec<Option<Color>>,
    pub edge_styles: Vec<(usize, usize, EdgeStyle)>,
}

impl Piece {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    /// Moved by `by`.
    pub fn moved(&self, by: iced::Vector) -> Piece {
        Piece {
            vertices: self.vertices.iter().map(|&p| p + by).collect(),
            ..self.clone()
        }
    }

    /// The middle of its bounds.
    pub fn centre(&self) -> Point {
        let mut points = self.vertices.iter().copied();
        let Some(first) = points.next() else {
            return Point::ORIGIN;
        };
        let (min, max) = points.fold((first, first), |(min, max), p| {
            (
                Point::new(min.x.min(p.x), min.y.min(p.y)),
                Point::new(max.x.max(p.x), max.y.max(p.y)),
            )
        });
        Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0)
    }
}

/// A mirror: the line through `a` and `b`, reflecting what's on one side
/// onto the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mirror {
    pub a: Point,
    pub b: Point,
}

impl Mirror {
    /// `p` reflected across the line.
    pub fn reflect(self, p: Point) -> Point {
        self.affine().apply(p)
    }

    /// The reflection across the line.
    pub fn affine(self) -> Affine {
        Affine::reflection(self.a, self.b)
    }
}

/// Where a drawing shows with `mirrors` one after the other, each
/// mirroring all there is so far: the maps from the drawing to each of its
/// images, itself (the identity) first.
pub fn images(mirrors: &[Mirror]) -> Vec<Affine> {
    let mut images = vec![Affine::IDENTITY];
    for mirror in mirrors {
        let reflect = mirror.affine();
        let mirrored: Vec<_> = images.iter().map(|&image| image.then(reflect)).collect();
        images.extend(mirrored);
    }
    images
}

/// For drawings shown at `images`: the maps taking the drawing onto
/// where another of its images shows, as seen from one (`g` then back from
/// `h`). The images overlap each other iff the drawing overlaps itself
/// under one of these.
pub fn crossings(images: &[Affine]) -> Vec<Affine> {
    let mut maps: Vec<Affine> = Vec::new();
    for (i, &g) in images.iter().enumerate() {
        for (j, &h) in images.iter().enumerate() {
            let map = g.then(h.inverse());
            if i != j && !map.near(Affine::IDENTITY) && !maps.iter().any(|m| m.near(map)) {
                maps.push(map);
            }
        }
    }
    maps
}

/// What [`Document::normalize`] did to make the connectivity follow the
/// geometry. Vertex ids are as after the edit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    /// Vertices welded onto another at the same spot, as `(gone, kept)`.
    pub welded: Vec<(VertexId, VertexId)>,
    /// Edges `a`–`b` cut at a vertex that lies on them, as `(a, b, at)`.
    pub cut: Vec<(VertexId, VertexId, VertexId)>,
    /// Triangles dropped because they became flat.
    pub squashed: Vec<[VertexId; 3]>,
    /// Existing vertices that added triangles attach to.
    pub attached: Vec<VertexId>,
}

impl Changes {
    /// The vertex `v` ended up as, following welds.
    pub fn kept(&self, mut v: VertexId) -> VertexId {
        while let Some(&(_, kept)) = self.welded.iter().find(|&&(gone, _)| gone == v) {
            v = kept;
        }
        v
    }
}

impl Document {
    /// Changes with every applied edit; also differs between documents.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    pub fn vertex(&self, id: VertexId) -> Point {
        self.vertices[id]
    }

    /// The most recently added vertex.
    pub fn last_vertex(&self) -> VertexId {
        self.vertices.len() - 1
    }

    pub fn triangle_ids(&self) -> &[[VertexId; 3]] {
        &self.triangles
    }

    pub fn triangles(&self) -> impl Iterator<Item = [Point; 3]> + '_ {
        self.triangles.iter().map(|t| t.map(|id| self.vertices[id]))
    }

    /// The colour face `t` is painted, if any.
    pub fn color(&self, t: TriangleId) -> Option<Color> {
        self.colors.get(&key(self.triangles[t])).copied()
    }

    /// Each face's colour, in the order of [`Self::triangle_ids`].
    pub fn colors(&self) -> impl Iterator<Item = Option<Color>> + '_ {
        self.triangles
            .iter()
            .map(|&t| self.colors.get(&key(t)).copied())
    }

    /// Paints the faces, in the order of [`Self::triangle_ids`] (e.g. as
    /// read from a file); extra colours are ignored. Not an edit.
    pub fn set_colors(&mut self, colors: impl IntoIterator<Item = Option<Color>>) {
        self.colors = self
            .triangles
            .iter()
            .zip(colors)
            .filter_map(|(&t, color)| Some((key(t), color?)))
            .collect();
    }

    /// The style edge `a`–`b` is painted, if any.
    pub fn edge_style(&self, a: VertexId, b: VertexId) -> Option<EdgeStyle> {
        self.edge_styles.get(&(a.min(b), a.max(b))).copied()
    }

    /// The painted edges (lowest end first) and their styles.
    pub fn edge_styles(&self) -> impl Iterator<Item = (VertexId, VertexId, EdgeStyle)> + '_ {
        self.edge_styles
            .iter()
            .map(|(&(a, b), &style)| (a, b, style))
    }

    /// Paints edges (e.g. as read from a file); those that aren't edges
    /// are ignored. Not an edit.
    pub fn set_edge_styles(
        &mut self,
        styles: impl IntoIterator<Item = (VertexId, VertexId, EdgeStyle)>,
    ) {
        let edges = self.unique_edges();
        self.edge_styles = styles
            .into_iter()
            .map(|(a, b, style)| ((a.min(b), a.max(b)), style))
            .filter(|(edge, _)| edges.binary_search(edge).is_ok())
            .collect();
    }

    /// Vertices used by at least one triangle (with repeats).
    pub fn used_vertices(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.triangles.iter().flatten().copied()
    }

    /// The triangles of the shape `vertex` belongs to: all those reachable
    /// through shared vertices.
    pub fn connected(&self, vertex: VertexId) -> Vec<[VertexId; 3]> {
        let mut reached = vec![vertex];
        let mut shape: Vec<[VertexId; 3]> = Vec::new();
        let mut grew = true;

        while grew {
            grew = false;
            for t in &self.triangles {
                if !shape.contains(t) && t.iter().any(|v| reached.contains(v)) {
                    shape.push(*t);
                    reached.extend(t);
                    grew = true;
                }
            }
        }
        shape
    }

    /// All edges, as vertex id pairs. Shared edges appear once per triangle.
    pub fn edges(&self) -> impl Iterator<Item = (VertexId, VertexId)> + '_ {
        self.triangles
            .iter()
            .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
    }

    /// Each edge once, as a sorted list of `(low, high)` vertex id pairs.
    pub fn unique_edges(&self) -> Vec<(VertexId, VertexId)> {
        let mut edges: Vec<_> = self.edges().map(|(u, v)| (u.min(v), u.max(v))).collect();
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Vertices used by triangles, each once, sorted.
    pub fn unique_vertices(&self) -> Vec<VertexId> {
        let mut used: Vec<_> = self.used_vertices().collect();
        used.sort_unstable();
        used.dedup();
        used
    }

    /// The number of vertices used by triangles.
    pub fn vertex_count(&self) -> usize {
        self.unique_vertices().len()
    }

    /// Axis-aligned bounds of all vertices used by triangles.
    pub fn bounds(&self) -> Option<(Point, Point)> {
        let mut points = self.triangles.iter().flatten().map(|&id| self.vertices[id]);
        let first = points.next()?;

        Some(points.fold((first, first), |(min, max), p| {
            (
                Point::new(min.x.min(p.x), min.y.min(p.y)),
                Point::new(max.x.max(p.x), max.y.max(p.y)),
            )
        }))
    }

    /// The vertices triangles use (renumbered from 0, in order) and the
    /// triangles in terms of those: everything needed to rebuild the
    /// document with [`Self::from_parts`].
    pub fn to_parts(&self) -> (Vec<Point>, Vec<[usize; 3]>) {
        let used = self.unique_vertices();
        let index = |v| used.binary_search(&v).expect("used vertex");
        let vertices = used.iter().map(|&v| self.vertices[v]).collect();
        let triangles = self.triangles.iter().map(|t| t.map(index)).collect();
        (vertices, triangles)
    }

    /// Rebuilds a document from [`Self::to_parts`], checking that it holds
    /// together: finite points, corners that exist, positive winding (which
    /// it restores), no flat or overlapping triangles.
    pub fn from_parts(vertices: Vec<Point>, triangles: Vec<[usize; 3]>) -> Result<Self, String> {
        if let Some(p) = vertices
            .iter()
            .find(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(format!("vertex {p:?} is not a finite point"));
        }
        if let Some(t) = triangles
            .iter()
            .find(|t| t.iter().any(|&v| v >= vertices.len()))
        {
            return Err(format!("triangle {t:?} refers to a missing vertex"));
        }

        let mut document = Document {
            vertices,
            ..Document::default()
        };
        for t in triangles {
            if is_flat(t.map(|v| document.vertices[v])) {
                return Err(format!("triangle {t:?} is flat"));
            }
            document.push_triangle(t);
        }
        if !document.is_valid_after(&Document::default()) {
            return Err("triangles overlap".to_string());
        }
        Ok(document)
    }

    /// Applies an edit. Returns `false` (leaving the document unchanged) if
    /// the edit was rejected, e.g. because it would produce a degenerate,
    /// duplicate, folded-over or overlapping triangle.
    pub fn apply(&mut self, edit: Edit) -> bool {
        self.apply_all(&[edit])
    }

    /// Checks the invariants, assuming they held in `before`: only triangles
    /// that are new or have a moved corner need checking for overlaps.
    fn is_valid_after(&self, before: &Document) -> bool {
        if !self.triangles().all(|[a, b, c]| area2(a, b, c) > 0.0) {
            return false;
        }

        let existed: HashSet<[VertexId; 3]> = before.triangles.iter().map(|&t| key(t)).collect();
        let changed = |t: &[VertexId; 3]| {
            !existed.contains(&key(*t))
                || t.iter()
                    .any(|&v| before.vertices.get(v) != Some(&self.vertices[v]))
        };

        self.triangle_ids()
            .iter()
            .enumerate()
            .filter(|(_, t)| changed(t))
            .all(|(i, t)| {
                let points = t.map(|v| self.vertices[v]);
                self.triangle_ids()
                    .iter()
                    .enumerate()
                    .all(|(j, u)| i == j || !overlap(points, u.map(|v| self.vertices[v])))
            })
    }

    /// Applies several edits as one: all of them, or none if any is rejected.
    pub fn apply_all(&mut self, edits: &[Edit]) -> bool {
        match self.preview(edits) {
            Some((next, _)) => {
                *self = next;
                self.revision = next_revision();
                true
            }
            None => false,
        }
    }

    /// The document after `edits`, and what normalizing it changed; `None`
    /// if any of them is rejected.
    pub fn preview(&self, edits: &[Edit]) -> Option<(Document, Changes)> {
        self.preview_until(edits, None)
    }

    /// Like [`Self::preview`], but gives up (returning `None`) once past
    /// `deadline`: filling can take long in crowded geometry.
    pub fn preview_until(
        &self,
        edits: &[Edit],
        deadline: Option<Instant>,
    ) -> Option<(Document, Changes)> {
        self.preview_with(edits, deadline, true)
    }

    /// Like [`Self::preview_until`], but leaving the paint behind: for
    /// trying out edits that may yet be turned down (it's cheaper).
    pub fn preview_shape_until(
        &self,
        edits: &[Edit],
        deadline: Option<Instant>,
    ) -> Option<(Document, Changes)> {
        self.preview_with(edits, deadline, false)
    }

    fn preview_with(
        &self,
        edits: &[Edit],
        deadline: Option<Instant>,
        paint: bool,
    ) -> Option<(Document, Changes)> {
        let mut changes = Changes::default();
        let mut document = self.clone();

        for edit in edits {
            let mut next = document.clone();
            let moved: Vec<VertexId> = match edit {
                Edit::MoveVertex { id, .. } => vec![*id],
                Edit::MoveVertices { moves } => moves.iter().map(|&(id, _)| id).collect(),
                _ => Vec::new(),
            };
            let valid = next.apply_unchecked(edit.clone(), &mut changes, deadline)
                && next.normalize(&moved, &mut changes, &document)
                && next.is_valid_after(&document);
            if !valid {
                return None;
            }
            if paint {
                // A mirror image takes its paint from what it mirrors; a
                // pasted piece brings its own.
                let added;
                let source = match edit {
                    Edit::Mirror { mirror } => {
                        added = document.with_reflection(*mirror);
                        &added
                    }
                    Edit::Paste { piece } => {
                        added = document.with_piece(piece);
                        &added
                    }
                    _ => &document,
                };
                next.inherit_colors(source);
                next.inherit_edge_styles(source);
            }
            document = next;
        }

        Some((document, changes))
    }

    /// The faces with all their corners among `vertices`, as a piece
    /// apart, painted alike (edges both of whose ends are in it too).
    pub fn piece(&self, vertices: &[VertexId]) -> Piece {
        let inside: HashSet<VertexId> = vertices.iter().copied().collect();
        let mut index: BTreeMap<VertexId, usize> = BTreeMap::new();
        let mut piece = Piece::default();
        for &t in &self.triangles {
            if !t.iter().all(|v| inside.contains(v)) {
                continue;
            }
            let corners = t.map(|v| {
                *index.entry(v).or_insert_with(|| {
                    piece.vertices.push(self.vertices[v]);
                    piece.vertices.len() - 1
                })
            });
            piece.triangles.push(corners);
            piece.colors.push(self.colors.get(&key(t)).copied());
        }
        for (&(a, b), &style) in &self.edge_styles {
            if let (Some(&a), Some(&b)) = (index.get(&a), index.get(&b)) {
                piece.edge_styles.push((a, b, style));
            }
        }
        piece
    }

    /// The drawing and, beside it, `piece`: not joined up.
    fn with_piece(&self, piece: &Piece) -> Document {
        let n = self.vertices.len();
        let mut both = self.clone();
        both.vertices.extend(&piece.vertices);
        for (&t, &color) in piece.triangles.iter().zip(&piece.colors) {
            let t = t.map(|v| v + n);
            both.push_triangle(t);
            if let Some(color) = color {
                both.colors.insert(key(t), color);
            }
        }
        for &(a, b, style) in &piece.edge_styles {
            let (a, b) = (a + n, b + n);
            both.edge_styles.insert((a.min(b), a.max(b)), style);
        }
        both.revision = next_revision();
        both
    }

    /// How many vertices there are room for: those added next are numbered
    /// from here.
    pub fn vertex_slots(&self) -> usize {
        self.vertices.len()
    }

    /// Just the drawing's mirror image across `mirror`, painted alike.
    pub fn reflection(&self, mirror: Mirror) -> Document {
        self.transformed(mirror.affine())
    }

    /// The drawing mapped by `map`, painted alike.
    pub fn transformed(&self, map: Affine) -> Document {
        let flips = map.flips();
        Document {
            vertices: self.vertices.iter().map(|&p| map.apply(p)).collect(),
            // Turned over, they wind the other way round: turned back.
            triangles: self
                .triangles
                .iter()
                .map(|&[a, b, c]| if flips { [a, c, b] } else { [a, b, c] })
                .collect(),
            colors: self.colors.clone(),
            edge_styles: self.edge_styles.clone(),
            revision: next_revision(),
        }
    }

    /// The drawing with `mirrors` applied, one after the other: see
    /// [`Self::with_mirror`].
    pub fn with_mirrors(&self, mirrors: &[Mirror]) -> Document {
        let mut drawing = self.clone();
        for &mirror in mirrors {
            drawing = drawing.with_mirror(mirror);
        }
        drawing
    }

    /// The drawing and, beside it, its mirror image across `mirror`: not
    /// joined up (see [`Self::with_mirror`]).
    fn with_reflection(&self, mirror: Mirror) -> Document {
        let n = self.vertices.len();
        let image = self.reflection(mirror);
        let mut both = self.clone();
        both.vertices.extend(image.vertices);
        both.triangles
            .extend(image.triangles.into_iter().map(|t| t.map(|v| v + n)));
        both.colors.extend(
            image
                .colors
                .into_iter()
                .map(|(t, c)| (key(t.map(|v| v + n)), c)),
        );
        both.edge_styles.extend(
            image
                .edge_styles
                .into_iter()
                .map(|((a, b), style)| ((a + n, b + n), style)),
        );
        both.revision = next_revision();
        both
    }

    /// The drawing with its mirror image joined on, as applying the mirror
    /// would make it; for showing and exporting it. Unlike applying, it
    /// doesn't check the two don't overlap (they're kept from that while
    /// editing): it's quick enough to do on every change.
    pub fn with_mirror(&self, mirror: Mirror) -> Document {
        let mut changes = Changes::default();
        let mut joined = self.with_reflection(mirror);
        if !joined.normalize(&[], &mut changes, self) {
            return self.with_reflection(mirror);
        }
        joined.inherit_colors(&self.with_reflection(mirror));
        joined.inherit_edge_styles(&self.with_reflection(mirror));
        joined
    }

    /// Whether the drawing overlaps itself mapped by any of `maps`: those of
    /// its triangles new or moved since `before` (all of them, without it),
    /// mapped, against all of it. (With each map's inverse among them too,
    /// that's all that can have come to overlap.)
    pub fn overlaps_under(&self, before: Option<&Document>, maps: &[Affine]) -> bool {
        let changed = before.map(|before| Changed::since(self, before));
        let bounds = |t: [Point; 3]| {
            let (mut min, mut max) = (t[0], t[0]);
            for p in &t[1..] {
                min = Point::new(min.x.min(p.x), min.y.min(p.y));
                max = Point::new(max.x.max(p.x), max.y.max(p.y));
            }
            (min, max)
        };
        let all: Vec<_> = self.triangles().map(|t| (t, bounds(t))).collect();
        self.triangles
            .iter()
            .filter(|&&t| changed.as_ref().is_none_or(|changed| changed.triangle(t)))
            .any(|&[a, b, c]| {
                maps.iter().any(|&map| {
                    let corners = if map.flips() { [c, b, a] } else { [a, b, c] };
                    let image = corners.map(|v| map.apply(self.vertices[v]));
                    let (min, max) = bounds(image);
                    all.iter().any(|&(u, (umin, umax))| {
                        min.x <= umax.x
                            && umin.x <= max.x
                            && min.y <= umax.y
                            && umin.y <= max.y
                            && overlap(image, u)
                    })
                })
            })
    }

    fn apply_unchecked(
        &mut self,
        edit: Edit,
        changes: &mut Changes,
        deadline: Option<Instant>,
    ) -> bool {
        match edit {
            Edit::AddTriangle { corners, snap } => self.fill(corners, snap, changes, deadline),
            Edit::InsertVertex { at } => self.insert_vertex(at),
            Edit::RemoveTriangle { triangle } => {
                triangle < self.triangles.len() && {
                    self.triangles.remove(triangle);
                    true
                }
            }
            Edit::MoveVertex { id, to } => self.move_vertex(id, to),
            Edit::Mirror { mirror } => {
                *self = self.with_reflection(mirror);
                true
            }
            Edit::Paste { piece } => {
                let fine = piece
                    .vertices
                    .iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite())
                    && piece
                        .triangles
                        .iter()
                        .flatten()
                        .all(|&v| v < piece.vertices.len());
                // A face landing right on one there is would be welded onto
                // it, and dropped as the same: it lies over it all the same.
                let same = |p: Point, q: Point| p.distance(q) <= tolerance(0.0);
                let on_one = piece.triangles.iter().any(|t| {
                    let corners = t.map(|v| piece.vertices[v]);
                    self.triangles()
                        .any(|u| corners.iter().all(|&p| u.iter().any(|&q| same(p, q))))
                });
                fine && !on_one && {
                    *self = self.with_piece(&piece);
                    true
                }
            }
            Edit::MoveVertices { moves } => {
                let used = self.unique_vertices();
                moves.iter().all(|(id, to)| {
                    used.binary_search(id).is_ok() && to.x.is_finite() && to.y.is_finite()
                }) && {
                    for &(id, to) in &moves {
                        self.vertices[id] = to;
                    }
                    true
                }
            }
            Edit::Paint { triangle, color } => {
                let Some(&t) = self.triangles.get(triangle) else {
                    return false;
                };
                match color {
                    Some(color) => self.colors.insert(key(t), color),
                    None => self.colors.remove(&key(t)),
                };
                true
            }
            Edit::PaintEdge { a, b, style } => {
                let edge = (a.min(b), a.max(b));
                if self.unique_edges().binary_search(&edge).is_err() {
                    return false;
                }
                match style {
                    Some(style) => self.edge_styles.insert(edge, style),
                    None => self.edge_styles.remove(&edge),
                };
                true
            }
            Edit::PaintAll { color } => {
                self.colors = match color {
                    Some(color) => self.triangles.iter().map(|&t| (key(t), color)).collect(),
                    None => BTreeMap::new(),
                };
                true
            }
            Edit::PaintAllEdges { style } => {
                self.edge_styles = match style {
                    Some(style) => self
                        .unique_edges()
                        .into_iter()
                        .map(|edge| (edge, style))
                        .collect(),
                    None => BTreeMap::new(),
                };
                true
            }
        }
    }

    /// After an edit from `before`: edges that kept their ends keep their
    /// style; new edges lying along a styled edge from before (a piece of
    /// it) take its style. Styles of edges gone are dropped.
    fn inherit_edge_styles(&mut self, before: &Document) {
        if self.edge_styles.is_empty() && before.edge_styles.is_empty() {
            return;
        }
        let existed = before.unique_edges();
        let on = |p: Point, a: Point, b: Point| {
            crate::geometry::closest_on_segment(p, a, b).1 <= tolerance(a.distance(b))
        };

        self.edge_styles = self
            .unique_edges()
            .into_iter()
            .filter_map(|edge| {
                let style = if existed.binary_search(&edge).is_ok() {
                    self.edge_styles.get(&edge).copied()
                } else {
                    let (p, q) = (self.vertices[edge.0], self.vertices[edge.1]);
                    before.edge_styles().find_map(|(a, b, style)| {
                        let (a, b) = (before.vertices[a], before.vertices[b]);
                        (on(p, a, b) && on(q, a, b)).then_some(style)
                    })
                };
                Some((edge, style?))
            })
            .collect();
    }

    /// After an edit from `before`: faces that kept their corners keep
    /// their colour; new faces take that of the face they lay in before
    /// (by their centre), if painted. Colours of faces gone are dropped.
    fn inherit_colors(&mut self, before: &Document) {
        if self.colors.is_empty() && before.colors.is_empty() {
            return;
        }
        let existed: BTreeSet<_> = before.triangles.iter().map(|&t| key(t)).collect();

        self.colors = self
            .triangles
            .iter()
            .filter_map(|&t| {
                let k = key(t);
                let color = if existed.contains(&k) {
                    self.colors.get(&k).copied()
                } else {
                    let [a, b, c] = t.map(|v| self.vertices[v]);
                    let centre = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
                    before.triangle_at(centre).and_then(|u| before.color(u))
                };
                Some((k, color?))
            })
            .collect();
    }

    /// Splits the triangle containing `at` (on its boundary counts) into
    /// three at a new vertex there. On an edge, one of the three is flat;
    /// normalizing drops it and splits the triangle across the edge.
    fn insert_vertex(&mut self, at: Point) -> bool {
        let taken = self
            .used_vertices()
            .any(|v| self.vertices[v].distance(at) <= tolerance(0.0));
        let inside = |[a, b, c]: [Point; 3]| {
            [(a, b), (b, c), (c, a)]
                .into_iter()
                .all(|(u, v)| area2(u, v, at) >= -tolerance(u.distance(v)) * u.distance(v))
        };
        let Some(triangle) = self.triangles().position(inside).filter(|_| !taken) else {
            return false;
        };

        let [a, b, c] = self.triangles.remove(triangle);
        self.vertices.push(at);
        let d = self.last_vertex();
        self.triangles.extend([[a, b, d], [b, c, d], [c, a, d]]);
        true
    }

    /// Moves vertex `id` in a straight line to `to`, rearranging its
    /// triangles whenever one of them would fold over, i.e. when the vertex
    /// crosses the line through the opposite edge `u`–`w` of that triangle:
    ///
    /// - Through the edge itself, with a triangle on the other side: the edge
    ///   is flipped, so the vertex connects to that triangle's far corner.
    /// - Through the edge, into empty space: the triangle is dropped and the
    ///   vertex's other triangles stretch along with it. If it is the only
    ///   triangle holding the vertex, it turns over to the other side instead.
    /// - Past the end of the edge (say `u` ends up between the vertex and
    ///   `w`): the edge from the vertex to `w` runs through `u`, so it is
    ///   flipped the same way, or the (flat) triangle is dropped.
    ///
    /// A triangle that ends up flat, the vertex landing on the line through
    /// the opposite edge, isn't folded over: normalizing drops it and, if
    /// the vertex lands on an edge, splits that edge.
    ///
    /// Returns `false` if the rearranging doesn't settle.
    fn move_vertex(&mut self, id: VertexId, to: Point) -> bool {
        const MAX_EVENTS: usize = 64;

        let mut from = self.vertices[id];

        for _ in 0..MAX_EVENTS {
            // The first triangle around `id` the move would flatten, as the
            // fraction of the remaining way at which that happens.
            let event = self
                .triangles
                .iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    let (u, w) = opposite(*t, id)?;
                    let (pu, pw) = (self.vertices[u], self.vertices[w]);
                    let (now, then) = (area2(pu, pw, from), area2(pu, pw, to));
                    let lands_on_line = is_flat([pu, pw, to]);
                    (then <= 0.0 && !lands_on_line)
                        .then(|| (i, (now / (now - then)).clamp(0.0, 1.0)))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));

            let Some((i, fraction)) = event else {
                self.vertices[id] = to;
                return true;
            };

            from = from + (to - from) * fraction;
            self.vertices[id] = from;
            self.unfold(i, id);
        }

        false
    }

    /// Resolves triangle `i`, flattened because vertex `id` lies on the line
    /// through its opposite edge. See [`Self::move_vertex`].
    fn unfold(&mut self, i: TriangleId, id: VertexId) {
        let t = self.triangles[i];
        let (u, w) = opposite(t, id).expect("triangle contains the vertex");
        let (pu, pw, pv) = (self.vertices[u], self.vertices[w], self.vertices[id]);

        // Where the vertex is along u → w.
        let along = (pv - pu).x * (pw - pu).x + (pv - pu).y * (pw - pu).y;
        let along = along / (pw - pu).x.hypot((pw - pu).y).powi(2);

        // The edge running through the flattened triangle, and the corner
        // lying in the middle of it.
        let ((p, q), middle) = if along <= 0.0 {
            ((id, w), u)
        } else if along >= 1.0 {
            ((id, u), w)
        } else {
            ((u, w), id)
        };

        let across = self.triangles.iter().enumerate().find_map(|(j, other)| {
            (j != i && other.contains(&p) && other.contains(&q))
                .then(|| other.iter().copied().find(|&v| v != p && v != q))
                .flatten()
                .map(|x| (j, x))
        });

        let alone = !self.triangles.iter().enumerate().any(|(j, other)| {
            j != i && other.contains(&id) && (other.contains(&u) || other.contains(&w))
        });

        match across {
            // Flip: the two triangles on edge p–q become two triangles on
            // the edge from `middle` to the far corner `x`.
            Some((j, x)) => {
                self.triangles.remove(i.max(j));
                self.triangles.remove(i.min(j));
                self.push_triangle([p, middle, x]);
                self.push_triangle([middle, q, x]);
            }
            // Only this triangle holds the vertex: let it turn over. It is
            // flat right now, so its winding is set for the far side.
            None if alone => self.triangles[i] = [t[0], t[2], t[1]],
            None => {
                self.triangles.remove(i);
            }
        }
    }

    /// Makes the connectivity follow the geometry: welds vertices at the
    /// same spot (keeping one that wasn't `moved`), drops triangles that
    /// are flat, and splits edges that have a vertex lying on them (see
    /// [`Self::split_t_junctions`]). Records what it did in `changes`.
    ///
    /// Returns `false` if that doesn't settle.
    ///
    /// `before` is the (normalized) document the edit was made to: only what
    /// changed since needs checking against the rest.
    fn normalize(&mut self, moved: &[VertexId], changes: &mut Changes, before: &Document) -> bool {
        let used = self.unique_vertices();
        let changed = Changed::since(self, before);

        for &u in &changed.vertices {
            for &v in &used {
                if u == v || self.vertices[u].distance(self.vertices[v]) > tolerance(0.0) {
                    continue;
                }
                // The later one goes, unless only the earlier one was moved.
                let (u, v) = (u.min(v), u.max(v));
                let (gone, kept) = if moved.contains(&u) && !moved.contains(&v) {
                    (u, v)
                } else {
                    (v, u)
                };
                if changes.welded.iter().any(|&(g, _)| g == gone || g == kept) {
                    continue; // Already welded away.
                }
                self.vertices[gone] = self.vertices[kept];
                for t in &mut self.triangles {
                    for c in t.iter_mut().filter(|c| **c == gone) {
                        *c = kept;
                    }
                }
                changes.welded.push((gone, kept));
            }
        }

        // Welding squashes the triangles that had both, and may leave
        // duplicates (two triangles folded onto each other).
        let mut seen: HashSet<[VertexId; 3]> = HashSet::new();
        let mut squashed = Vec::new();
        self.triangles.retain(|&t| {
            let key = key(t);
            if key[0] == key[1] || key[1] == key[2] {
                squashed.push(t);
                return false;
            }
            seen.insert(key)
        });
        changes.squashed.extend(squashed);

        self.split_t_junctions(changes, Some(&changed))
    }

    /// Splits every triangle that has a vertex lying in the middle of one of
    /// its edges (a "T-junction") into two, towards its opposite corner, so
    /// that vertex becomes a proper corner on both sides of the edge.
    ///
    /// (Nearly) flat triangles are removed first: they cover nothing, and in
    /// one a vertex can lie on two edges at once, which would split forever.
    /// The edges such a triangle joined are then repaired like any other.
    ///
    /// With `changed` (since a document without T-junctions), only what
    /// changed is checked: its edges against every vertex, and its vertices
    /// against every edge.
    ///
    /// Returns `false` if that doesn't settle.
    pub(super) fn split_t_junctions(
        &mut self,
        changes: &mut Changes,
        changed: Option<&Changed>,
    ) -> bool {
        let limit = 4 * (self.triangles.len() + self.vertices.len());

        for _ in 0..limit {
            let vertices = &self.vertices;
            self.triangles.retain(|&t| {
                let flat = is_flat(t.map(|v| vertices[v]));
                if flat {
                    changes.squashed.push(t);
                }
                !flat
            });

            let used = self.unique_vertices();
            let vertices = &self.vertices;
            let junction = self.triangles.iter().enumerate().find_map(|(i, &t)| {
                // An unchanged triangle can only have gained a changed
                // vertex on its edges.
                let candidates: &[VertexId] = match changed {
                    Some(changed) if !changed.triangle(t) => &changed.vertices,
                    _ => &used,
                };
                (0..3).find_map(|k| {
                    let (u, v, x) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    candidates
                        .iter()
                        .copied()
                        .find(|&w| {
                            !t.contains(&w)
                                && is_inside_segment(vertices[w], vertices[u], vertices[v])
                        })
                        .map(|w| (i, (u, v, w), [u, w, x], [w, v, x]))
                })
            });

            let Some((i, cut, first, second)) = junction else {
                return true;
            };

            changes.cut.push(cut);
            self.triangles.remove(i);
            self.push_triangle(first);
            self.push_triangle(second);
        }

        false
    }

    /// Adds a new triangle, wound positively.
    fn push_triangle(&mut self, [a, b, c]: [VertexId; 3]) {
        let [pa, pb, pc] = [a, b, c].map(|id| self.vertices[id]);

        self.triangles.push(if area2(pa, pb, pc) < 0.0 {
            [a, c, b]
        } else {
            [a, b, c]
        });
    }

    /// The triangle containing `point` (on its boundary counts), if any.
    pub fn triangle_at(&self, point: Point) -> Option<TriangleId> {
        self.triangles().position(|[a, b, c]| {
            // Triangles are wound positively, so inside is left of each edge.
            [(a, b), (b, c), (c, a)]
                .into_iter()
                .all(|(u, v)| area2(u, v, point) >= 0.0)
        })
    }

    /// Finds a triangle with these corners, in any order.
    pub fn find_triangle(&self, mut ids: [VertexId; 3]) -> Option<TriangleId> {
        ids.sort_unstable();

        self.triangles.iter().position(|t| {
            let mut t = *t;
            t.sort_unstable();
            t == ids
        })
    }
}

/// What changed in a document since `before`: the vertices that are new or
/// moved, and (to tell the triangles that are new) the triangles before.
pub(super) struct Changed {
    vertices: Vec<VertexId>,
    before: HashSet<[VertexId; 3]>,
}

impl Changed {
    fn since(document: &Document, before: &Document) -> Self {
        let vertices = document
            .unique_vertices()
            .into_iter()
            .filter(|&v| before.vertices.get(v) != Some(&document.vertices[v]))
            .collect();
        Changed {
            vertices,
            before: before.triangles.iter().map(|&t| key(t)).collect(),
        }
    }

    /// Whether triangle `t` is new, or has a changed corner.
    fn triangle(&self, t: [VertexId; 3]) -> bool {
        !self.before.contains(&key(t)) || t.iter().any(|v| self.vertices.contains(v))
    }
}

/// A face's corners in a fixed order, to tell it by whichever corner comes
/// first.
fn key(mut t: [VertexId; 3]) -> [VertexId; 3] {
    t.sort_unstable();
    t
}

/// The corners of `t` other than `id`, in winding order after it.
pub fn opposite(t: [VertexId; 3], id: VertexId) -> Option<(VertexId, VertexId)> {
    let i = t.iter().position(|&v| v == id)?;
    Some((t[(i + 1) % 3], t[(i + 2) % 3]))
}

/// How far (world units) a point may be off a line of the given length and
/// still count as on it. Just enough to absorb rounding (e.g. of a point
/// snapped onto an edge); anything further off is a real distance, and
/// treating it as zero lets near-collinear points split each other forever.
fn tolerance(length: f32) -> f32 {
    1e-3 + 1e-5 * length
}

/// Whether a triangle is flat: a corner lies on the line through the other two.
pub(super) fn is_flat(t: [Point; 3]) -> bool {
    let [a, b, c] = t;
    let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
    min_height(t) <= tolerance(longest)
}

/// Whether `p` lies on segment `a`–`b`, away from its ends.
fn is_inside_segment(p: Point, a: Point, b: Point) -> bool {
    let length = a.distance(b);
    let tolerance = tolerance(length);
    if length == 0.0 {
        return false;
    }

    // How far along a → b the point is, and how far off the line.
    let along = ((p - a).x * (b - a).x + (p - a).y * (b - a).y) / length;
    let off = area2(a, b, p).abs() / length;

    off <= tolerance && along > tolerance && along < length - tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_triangle() -> Document {
        let mut doc = Document::default();
        assert!(doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0
        }));
        doc
    }

    #[test]
    fn a_piece_is_copied_and_pasted_joined_on_but_never_over_anything() {
        let mut doc = Document::default();
        assert!(doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(0.0, 10.0)
            ],
            snap: 0.0
        }));
        let red = Color::from_rgb(1.0, 0.0, 0.0);
        assert!(doc.apply(Edit::Paint {
            triangle: 0,
            color: Some(red)
        }));
        let piece = doc.piece(&doc.unique_vertices());
        assert_eq!(piece.triangles.len(), 1);
        // Some of its corners only: no whole face.
        assert!(doc.piece(&[0, 1]).is_empty());

        // In place: over itself.
        assert!(!doc.clone().apply(Edit::Paste {
            piece: piece.clone()
        }));
        // Beside it, sharing an edge: joined on, painted alike.
        let beside = piece.moved(iced::Vector::new(10.0, 0.0));
        assert!(doc.apply(Edit::Paste { piece: beside }));
        assert_eq!(doc.triangle_ids().len(), 2);
        assert_eq!(doc.vertex_count(), 5);
        assert!(doc.colors().all(|color| color == Some(red)));
    }

    #[test]
    fn mirroring_adds_the_mirror_image_joined_on() {
        // A triangle with its right side on the mirror line x = 10, its
        // left corner painted.
        let mut doc = Document::default();
        assert!(doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 5.0),
                Point::new(10.0, 0.0),
                Point::new(10.0, 10.0)
            ],
            snap: 0.0
        }));
        let red = Color::from_rgb(1.0, 0.0, 0.0);
        assert!(doc.apply(Edit::Paint {
            triangle: 0,
            color: Some(red)
        }));
        let mirror = Mirror {
            a: Point::new(10.0, -5.0),
            b: Point::new(10.0, 20.0),
        };
        assert_eq!(mirror.reflect(Point::new(0.0, 5.0)), Point::new(20.0, 5.0));
        assert!(!doc.overlaps_under(None, &[mirror.affine()]));

        let shown = doc.with_mirror(mirror);
        assert!(doc.apply(Edit::Mirror { mirror }));
        // Joined along the line: four vertices, the image painted alike.
        assert_eq!(doc.vertex_count(), 4);
        assert_eq!(doc.triangle_ids().len(), 2);
        assert!(doc.colors().all(|color| color == Some(red)));
        assert!(
            doc.unique_vertices()
                .iter()
                .any(|&v| doc.vertex(v) == Point::new(20.0, 5.0))
        );
        // As shown before applying.
        assert_eq!(shown.vertex_count(), 4);
        assert_eq!(shown.triangle_ids().len(), 2);

        // A mirror through the drawing would mirror it over itself.
        let through = Mirror {
            a: Point::new(5.0, -5.0),
            b: Point::new(5.0, 20.0),
        };
        assert!(doc.overlaps_under(None, &[through.affine()]));
        assert!(!doc.apply(Edit::Mirror { mirror: through }));
    }

    #[test]
    fn vertices_move_together_without_folding_on_the_way() {
        let mut doc = Document::default();
        assert!(doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(0.0, 10.0)
            ],
            snap: 0.0
        }));
        // The whole triangle, far past where any corner alone would fold it.
        let by = iced::Vector::new(100.0, 50.0);
        let moves: Vec<_> = (0..3).map(|v| (v, doc.vertex(v) + by)).collect();
        assert!(doc.apply(Edit::MoveVertices { moves }));
        assert_eq!(doc.vertex(0), Point::new(100.0, 50.0));
        assert_eq!(doc.triangle_ids().len(), 1);
        // Folding it over is rejected.
        let flipped = vec![(1, Point::new(90.0, 50.0))];
        assert!(!doc.apply(Edit::MoveVertices { moves: flipped }));
    }

    #[test]
    fn faces_keep_their_colour_through_edits() {
        let red = Color::from_rgb8(255, 0, 0);
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::Paint {
            triangle: 0,
            color: Some(red),
        }));
        assert_eq!(doc.color(0), Some(red));

        // Moving a corner keeps it; splitting gives every piece its colour.
        assert!(doc.apply(Edit::MoveVertex {
            id: 2,
            to: Point::new(5.0, 12.0),
        }));
        assert_eq!(doc.color(0), Some(red));
        assert!(doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 4.0),
        }));
        assert_eq!(doc.triangle_ids().len(), 3);
        assert!(doc.colors().all(|c| c == Some(red)));

        // A face added beside it is unpainted.
        assert!(doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0
        }));
        assert_eq!(doc.colors().filter(|c| c.is_none()).count(), 1);

        // Clearing.
        assert!(doc.apply(Edit::Paint {
            triangle: 0,
            color: None,
        }));
        assert_eq!(doc.color(0), None);
        assert_eq!(doc.colors().filter(|c| c.is_none()).count(), 2);
    }

    #[test]
    fn painting_everything() {
        let red = Color::from_rgb8(255, 0, 0);
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 4.0),
        }));
        assert!(doc.apply(Edit::PaintAll { color: Some(red) }));
        assert!(doc.colors().all(|c| c == Some(red)));
        let style = EdgeStyle {
            color: red,
            width: 2.0,
        };
        assert!(doc.apply(Edit::PaintAllEdges { style: Some(style) }));
        assert_eq!(doc.edge_styles().count(), doc.unique_edges().len());

        assert!(doc.apply(Edit::PaintAll { color: None }));
        assert!(doc.apply(Edit::PaintAllEdges { style: None }));
        assert!(doc.colors().all(|c| c.is_none()));
        assert_eq!(doc.edge_styles().count(), 0);
    }

    #[test]
    fn edges_keep_their_style_through_edits() {
        let style = EdgeStyle {
            color: Color::from_rgb8(255, 0, 0),
            width: 3.0,
        };
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::PaintEdge {
            a: 1,
            b: 0,
            style: Some(style),
        }));
        assert_eq!(doc.edge_style(0, 1), Some(style));
        // Not an edge.
        assert!(!doc.apply(Edit::PaintEdge {
            a: 0,
            b: 7,
            style: Some(style),
        }));

        // Moving an end keeps it.
        assert!(doc.apply(Edit::MoveVertex {
            id: 1,
            to: Point::new(12.0, 0.0),
        }));
        assert_eq!(doc.edge_style(0, 1), Some(style));

        // Cut: both pieces keep it; the new edge inside doesn't get it.
        assert!(doc.apply(Edit::InsertVertex {
            at: Point::new(6.0, 0.0),
        }));
        let cut = doc.last_vertex();
        assert_eq!(doc.edge_style(0, 1), None);
        assert_eq!(doc.edge_style(0, cut), Some(style));
        assert_eq!(doc.edge_style(cut, 1), Some(style));
        assert_eq!(doc.edge_style(cut, 2), None);
        assert_eq!(doc.edge_styles().count(), 2);

        // Clearing.
        assert!(doc.apply(Edit::PaintEdge {
            a: 0,
            b: cut,
            style: None,
        }));
        assert_eq!(doc.edge_styles().count(), 1);
    }

    #[test]
    fn extending_shares_the_edge() {
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0
        }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2], [0, 1, 3]]);

        // Moving a shared corner moves both triangles.
        doc.apply(Edit::MoveVertex {
            id: 0,
            to: Point::new(-1.0, 0.0),
        });
        assert!(doc.triangles().all(|t| t[0] == Point::new(-1.0, 0.0)));
    }

    #[test]
    fn splitting_replaces_parent() {
        let mut doc = doc_with_triangle();
        let at = Point::new(5.0, 3.0);
        assert!(doc.apply(Edit::InsertVertex { at }));
        assert_eq!(doc.triangle_ids(), &[[0, 1, 3], [1, 2, 3], [2, 0, 3]]);
    }

    #[test]
    fn collapsing_a_corner_removes_the_triangle() {
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::MoveVertex {
            id: 2,
            to: Point::new(5.0, 0.0)
        }));
        assert!(doc.is_empty());
    }

    fn split_triangle() -> Document {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 3.0),
        });
        doc
    }

    fn sorted(doc: &Document) -> Vec<[VertexId; 3]> {
        let mut ids = doc.triangle_ids().to_vec();
        ids.iter_mut().for_each(|t| t.sort_unstable());
        ids.sort_unstable();
        ids
    }

    #[test]
    fn removing_the_triangle_under_a_point() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0,
        });

        assert_eq!(doc.triangle_at(Point::new(50.0, 50.0)), None);
        let below = doc.triangle_at(Point::new(5.0, -3.0)).unwrap();
        assert!(doc.apply(Edit::RemoveTriangle { triangle: below }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
    }

    #[test]
    fn cutting_an_edge_splits_both_sides() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0,
        });

        assert!(doc.apply(Edit::InsertVertex {
            at: Point::new(3.0, 0.0),
        }));
        assert_eq!(
            sorted(&doc),
            vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
        );
        assert_eq!(area_and_t_junctions(&doc), (100.0, false));

        // Right on a corner there's nothing to cut.
        assert!(!doc.apply(Edit::InsertVertex {
            at: Point::new(0.0, 0.0),
        }));
    }

    #[test]
    fn collapsing_a_split_center_onto_edge_keeps_two() {
        let mut doc = split_triangle();
        assert!(doc.apply(Edit::MoveVertex {
            id: 3,
            to: Point::new(4.0, 0.0)
        }));
        assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
    }

    #[test]
    fn merging_a_split_center_into_corner_restores_parent() {
        let mut doc = split_triangle();
        assert!(doc.apply(Edit::MoveVertex {
            id: 3,
            to: doc.vertex(0)
        }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
    }

    #[test]
    fn collapsing_onto_a_shared_edge_splits_the_other_side() {
        // Two triangles sharing edge 0–1, the upper one split at 4.
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0,
        });
        doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 3.0),
        });

        assert!(doc.apply(Edit::MoveVertex {
            id: 4,
            to: Point::new(4.0, 0.0)
        }));

        // Nothing uses the edge 0–1 any more; 4 is a corner on both sides.
        assert!(
            !doc.edges()
                .any(|(u, v)| (u, v) == (0, 1) || (u, v) == (1, 0))
        );
        assert_eq!(
            sorted(&doc),
            vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
        );
    }

    #[test]
    fn collapsing_tolerates_rounding() {
        // A long, slanted shared edge: the snapped point is never exactly on it.
        let (a, b) = (Point::new(100.3, 300.7), Point::new(407.1, 290.2));
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle {
            corners: [a, b, Point::new(250.0, 100.0)],
            snap: 0.0,
        });
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(250.0, 500.0)],
            snap: 0.0,
        });
        doc.apply(Edit::InsertVertex {
            at: Point::new(250.0, 230.0),
        });

        for i in 1..40 {
            let t = i as f32 / 40.0;
            let at = a + (b - a) * t;
            let mut doc = doc.clone();
            assert!(doc.apply(Edit::MoveVertex { id: 4, to: at }));
            assert_eq!(doc.triangle_ids().len(), 4, "t = {t}");
            assert!(
                doc.triangle_ids()
                    .iter()
                    .all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            );
        }
    }

    fn area(doc: &Document) -> f32 {
        doc.triangles().map(|[a, b, c]| area2(a, b, c) / 2.0).sum()
    }

    #[test]
    fn moving_across_an_inner_edge_flips_it() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0,
        });
        doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 3.0),
        });

        // Pull the top centre 4 across the shared edge 0–1 into the bottom
        // triangle: that edge flips to 4–3; the outline stays the same.
        assert!(doc.apply(Edit::MoveVertex {
            id: 4,
            to: Point::new(5.0, -3.0),
        }));
        assert_eq!(
            sorted(&doc),
            vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
        );
        assert_eq!(area_and_t_junctions(&doc), (100.0, false));
    }

    #[test]
    fn moving_an_outline_corner_across_an_edge_dents_the_outline() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
            snap: 0.0,
        });

        // The top corner 2 pulled down into the bottom triangle: the peak
        // becomes a notch, with the shared edge flipped to 2–3.
        assert!(doc.apply(Edit::MoveVertex {
            id: 2,
            to: Point::new(5.0, -5.0),
        }));
        assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
        assert_eq!(area(&doc), 25.0);
    }

    #[test]
    fn moving_out_through_the_outline_drops_the_crossed_triangle() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::InsertVertex {
            at: Point::new(5.0, 3.0),
        });

        // The centre 3 leaves through the bottom edge 0–1: the triangle on
        // that edge goes, the other two stretch down with the vertex.
        assert!(doc.apply(Edit::MoveVertex {
            id: 3,
            to: Point::new(5.0, -4.0),
        }));
        assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
        assert_eq!(area(&doc), 50.0 + 20.0);
        assert!(!area_and_t_junctions(&doc).1);
    }

    #[test]
    fn a_lone_triangle_turns_over() {
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::MoveVertex {
            id: 2,
            to: Point::new(5.0, -10.0),
        }));
        assert_eq!(doc.triangle_ids().len(), 1);
        assert_eq!(area(&doc), 50.0);
    }

    #[test]
    fn extending_into_geometry_fills_only_the_gap() {
        let mut doc = doc_with_triangle();

        // Extending edge 0–1 towards the side the triangle is already on:
        // only the dart-shaped gap around its top corner 2 gets filled.
        assert!(doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, 20.0)],
            snap: 0.0
        }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2], [0, 2, 3], [1, 2, 3]]);
        assert_eq!(area_and_t_junctions(&doc), (100.0, false));
    }

    #[test]
    fn adding_on_top_of_geometry_fills_only_the_gap() {
        let mut doc = doc_with_triangle();

        // A triangle poking out of the top of the existing one: the part
        // outside it is added, and the existing edges it crosses are split.
        assert!(doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(2.0, 2.0),
                Point::new(8.0, 2.0),
                Point::new(5.0, 20.0),
            ],
            snap: 0.0
        }));
        let (area, t_junctions) = area_and_t_junctions(&doc);
        assert!(!t_junctions);
        // The sides cross at y = 5. Above that the new triangle adds 37.5,
        // less the 12.5 of the existing top corner poking into it.
        assert!((area - (50.0 + 37.5 - 12.5)).abs() < 1e-3, "{area}");

        // Entirely inside existing geometry: nothing to add.
        assert!(!doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(4.0, 1.0),
                Point::new(6.0, 1.0),
                Point::new(5.0, 3.0),
            ],
            snap: 0.0
        }));
    }

    /// Whether a vertex sits in the middle of another triangle's edge.
    pub(super) fn has_t_junction(doc: &Document) -> bool {
        let used: Vec<_> = doc.used_vertices().collect();
        doc.triangle_ids().iter().any(|t| {
            (0..3).any(|k| {
                let (u, v) = (t[k], t[(k + 1) % 3]);
                used.iter().any(|&w| {
                    !t.contains(&w)
                        && is_inside_segment(doc.vertex(w), doc.vertex(u), doc.vertex(v))
                })
            })
        })
    }

    /// Total area and whether any vertex sits in the middle of an edge.
    fn area_and_t_junctions(doc: &Document) -> (f32, bool) {
        let area = doc.triangles().map(|[a, b, c]| area2(a, b, c) / 2.0).sum();
        (area, has_t_junction(doc))
    }

    #[test]
    fn collapsing_can_squash_several_triangles() {
        // Two triangles sharing the edge 0–1 (y = 300), each split.
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(100.0, 300.0),
                Point::new(300.0, 300.0),
                Point::new(200.0, 100.0),
            ],
            snap: 0.0,
        });
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(200.0, 500.0)],
            snap: 0.0,
        });
        let split = |doc: &mut Document, at| assert!(doc.apply(Edit::InsertVertex { at }));
        split(&mut doc, Point::new(200.0, 230.0)); // centre 4
        split(&mut doc, Point::new(200.0, 370.0)); // centre 5

        // Collapse the top centre onto the shared edge: the bottom side now
        // has four triangles around 5, two of them along the shared line.
        assert!(doc.apply(Edit::MoveVertex {
            id: 4,
            to: Point::new(200.0, 300.0)
        }));
        assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));

        // Collapse the bottom centre onto the shared line between 0 and 4.
        // That flattens both (0, 4, 5) and (4, 1, 5); the bottom is re-split
        // with a new edge from 4 down to 3.
        assert!(doc.apply(Edit::MoveVertex {
            id: 5,
            to: Point::new(150.0, 300.0)
        }));
        assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));
        assert!(doc.edges().any(|e| e == (4, 3) || e == (3, 4)));
    }

    #[test]
    fn merging_joins_separate_triangles() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(20.0, 0.0),
                Point::new(30.0, 0.0),
                Point::new(25.0, 10.0),
            ],
            snap: 0.0,
        });
        // Corner to corner, then the second corner: they share an edge.
        assert!(doc.apply(Edit::MoveVertex {
            id: 1,
            to: doc.vertex(3)
        }));
        assert!(doc.apply(Edit::MoveVertex {
            id: 2,
            to: doc.vertex(5)
        }));
        assert_eq!(doc.triangle_ids().len(), 2);
        assert_eq!(
            doc.edges()
                .filter(|&(u, v)| u.min(v) == 3 && u.max(v) == 5)
                .count(),
            2
        );
    }

    #[test]
    fn merging_rejects_overlap() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(20.0, 0.0),
                Point::new(30.0, 0.0),
                Point::new(25.0, 10.0),
            ],
            snap: 0.0,
        });
        // The first triangle would be stretched right across the second.
        assert!(!doc.apply(Edit::MoveVertex {
            id: 0,
            to: doc.vertex(4)
        }));
    }

    /// The new triangles in `after` (not in `before`) thinner than `min`.
    fn thin_new(before: &Document, after: &Document, min: f32) -> Vec<[Point; 3]> {
        after
            .triangle_ids()
            .iter()
            .filter(|&&t| before.find_triangle(t).is_none())
            .map(|t| t.map(|v| after.vertex(v)))
            .filter(|&t| min_height(t) < min)
            .collect()
    }

    #[test]
    fn a_fill_bends_around_a_vertex_close_to_its_side() {
        // A triangle, and one below it whose right corner (250, 340) ends
        // up just under 6 from the left side of the extension below.
        let mut doc = Document::default();
        for corners in [
            [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)],
            [(150.0, 330.0), (250.0, 340.0), (200.0, 420.0)],
        ] {
            let corners = corners.map(|(x, y)| Point::new(x, y));
            assert!(doc.apply(Edit::AddTriangle { corners, snap: 0.0 }));
        }

        let corners = [doc.vertex(0), doc.vertex(1), Point::new(360.0, 380.0)];
        let (plain, _) = doc
            .preview(&[Edit::AddTriangle { corners, snap: 0.0 }])
            .unwrap();
        assert!(!thin_new(&doc, &plain, 5.0).is_empty());

        let (snapped, _) = doc
            .preview(&[Edit::AddTriangle { corners, snap: 5.0 }])
            .unwrap();
        assert_eq!(thin_new(&doc, &snapped, 5.0), Vec::<[Point; 3]>::new());
    }

    #[test]
    fn a_fill_bends_rather_than_splitting_off_a_sliver() {
        // Extending the right edge of a triangle across a separate one, its
        // side crossing that one's edge close to a corner: splitting there
        // would leave a sliver of the existing triangle.
        let mut doc = Document::default();
        for corners in [
            [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)],
            [(330.0, 280.0), (450.0, 300.0), (400.0, 150.0)],
        ] {
            let corners = corners.map(|(x, y)| Point::new(x, y));
            assert!(doc.apply(Edit::AddTriangle { corners, snap: 0.0 }));
        }

        let corners = [doc.vertex(1), doc.vertex(2), Point::new(550.0, 260.0)];
        let (plain, _) = doc
            .preview(&[Edit::AddTriangle { corners, snap: 0.0 }])
            .unwrap();
        assert!(!thin_new(&doc, &plain, 5.0).is_empty());

        let (snapped, _) = doc
            .preview(&[Edit::AddTriangle { corners, snap: 5.0 }])
            .unwrap();
        assert_eq!(thin_new(&doc, &snapped, 5.0), Vec::<[Point; 3]>::new());
    }

    #[test]
    fn rejects_degenerate_and_duplicate() {
        let mut doc = doc_with_triangle();
        assert!(!doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), Point::new(20.0, 0.0)],
            snap: 0.0
        }));
        assert!(!doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(0), doc.vertex(1), doc.vertex(2)],
            snap: 0.0
        }));
        assert_eq!(doc.triangle_ids().len(), 1);
    }
}

#[cfg(test)]
mod fuzz {
    use super::*;
    use crate::geometry::closest_on_segment;

    #[test]
    fn random_moves() {
        let mut seed = 12345u64;
        let mut rand = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32) / (1u64 << 31) as f32
        };

        let (mut ok, mut rejected) = (0, 0);
        for _ in 0..200 {
            let mut doc = Document::default();
            doc.apply(Edit::AddTriangle {
                corners: [
                    Point::new(100.0, 300.0),
                    Point::new(300.0, 300.0),
                    Point::new(200.0, 100.0),
                ],
                snap: 0.0,
            });
            doc.apply(Edit::AddTriangle {
                corners: [doc.vertex(0), doc.vertex(1), Point::new(200.0, 500.0)],
                snap: 0.0,
            });
            doc.apply(Edit::AddTriangle {
                corners: [doc.vertex(1), doc.vertex(2), Point::new(350.0, 150.0)],
                snap: 0.0,
            });
            doc.apply(Edit::InsertVertex {
                at: Point::new(200.0, 230.0),
            });
            doc.apply(Edit::InsertVertex {
                at: Point::new(200.0, 370.0),
            });

            for _ in 0..20 {
                let used: Vec<_> = doc.used_vertices().collect();
                let id = used[(rand() * used.len() as f32) as usize % used.len()];
                let to = Point::new(rand() * 500.0, rand() * 600.0);
                let before = doc.clone();
                if doc.apply(Edit::MoveVertex { id, to }) {
                    ok += 1;
                    // Only exact T-junctions (as rearranging would create) count;
                    // a random target merely touching an edge is fine.
                    let used: Vec<_> = doc.used_vertices().collect();
                    let t_junction = doc.triangle_ids().iter().any(|t| {
                        (0..3).any(|k| {
                            let (u, v) = (doc.vertex(t[k]), doc.vertex(t[(k + 1) % 3]));
                            used.iter().any(|&w| {
                                let p = doc.vertex(w);
                                !t.contains(&w)
                                    && closest_on_segment(p, u, v).1 < 1e-4
                                    && p.distance(u) > 1e-3
                                    && p.distance(v) > 1e-3
                            })
                        })
                    });
                    assert!(!t_junction, "{before:?} {id} -> {to:?}");
                } else {
                    rejected += 1;
                }
            }
        }
        println!("ok {ok}, rejected {rejected}");
    }

    #[test]
    fn random_fills() {
        let mut seed = 777u64;
        let mut rand = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32) / (1u64 << 31) as f32
        };

        let (mut ok, mut empty, mut rejected) = (0, 0, 0);
        for _ in 0..30 {
            let mut doc = Document::default();
            doc.apply(Edit::AddTriangle {
                corners: [
                    Point::new(200.0, 300.0),
                    Point::new(300.0, 300.0),
                    Point::new(250.0, 200.0),
                ],
                snap: 0.0,
            });

            for _ in 0..15 {
                let r: Vec<f32> = (0..8).map(|_| rand()).collect();
                let point = |i: usize| Point::new(r[i] * 500.0, r[i + 1] * 500.0);
                let edit = if r[0] < 0.5 {
                    let edges: Vec<_> = doc.edges().collect();
                    let (a, b) = edges[(r[1] * edges.len() as f32) as usize % edges.len()];
                    Edit::AddTriangle {
                        corners: [doc.vertex(a), doc.vertex(b), point(2)],
                        snap: 0.0,
                    }
                } else {
                    Edit::AddTriangle {
                        corners: [point(2), point(4), point(6)],
                        snap: 0.0,
                    }
                };

                let started = std::time::Instant::now();
                let mut unchecked = doc.clone();
                if !unchecked.apply_unchecked(edit.clone(), &mut Changes::default(), None) {
                    empty += 1;
                } else if doc.apply(edit.clone()) {
                    ok += 1;
                    assert!(!super::tests::has_t_junction(&doc), "{edit:?}");
                } else {
                    rejected += 1;
                }
                if std::env::var("FILL_TRACE").is_ok() {
                    println!(
                        "{} tris, {} vertices, {:?}",
                        doc.triangle_ids().len(),
                        doc.vertices.len(),
                        started.elapsed()
                    );
                }
            }
        }
        println!("fills: ok {ok}, nothing to add {empty}, rejected {rejected}");
    }
}
