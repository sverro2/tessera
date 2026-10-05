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

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use iced::{Color, Point};

use crate::geometry::{Affine, area2, min_height, overlap};

pub(crate) mod cells;
mod edit;
mod fill;
mod loops;
mod merge;
mod mirror;
mod quick;

use cells::Cells;
use edit::Changed;
pub use mirror::{Mirror, crossings, images};
use quick::QuickSet;

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

/// The colour of faces made in empty space (a muted blue): painted from the
/// start, until painted otherwise or erased.
pub const DEFAULT_FACE: Color = Color::from_rgb8(0x3d, 0x4a, 0x6e);

/// The style of edges made anew (but for pieces of edges there were):
/// fine and light.
pub const DEFAULT_EDGE: EdgeStyle = EdgeStyle {
    color: Color::from_rgb8(0xc0, 0xca, 0xf5),
    width: 1.0,
};

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
    /// Split each of `edges` (`a`–`b`, either way round) at its middle, and
    /// the triangles on them with it: one with all three split into four
    /// (its corners, and one in the middle), the same shape halved; one
    /// with two, into three; with one, into two.
    Subdivide { edges: Vec<(VertexId, VertexId)> },
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
    /// Carry out `edit` (moving vertices, or pasting), and where what it
    /// moved or pasted lands over the drawing, merge it in: it stays as it
    /// is, on top, and the faces under it are cut away round it, filled in
    /// again where they still show (joined up at new vertices where they
    /// cross it; within `snap`, world units, things count as touching, as
    /// for [`Edit::AddTriangle`]). Faces some of whose corners moved
    /// (stretched, or folded over), and those of the moved ones not all of
    /// whose corners are among `held` (if any: the selection, which the
    /// rest only goes along with, editing proportionally), are made again
    /// if need be, keeping their vertices. Rejected if what's carried
    /// whole folds over itself.
    Merging {
        edit: Box<Edit>,
        snap: f32,
        held: Vec<VertexId>,
    },
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

impl Edit {
    /// The edit carried out: what's merged, else itself.
    pub fn inner(&self) -> &Edit {
        match self {
            Edit::Merging { edit, .. } => edit.inner(),
            edit => edit,
        }
    }

    /// The vertices it moves.
    fn moved(&self) -> Vec<VertexId> {
        match self.inner() {
            Edit::MoveVertex { id, .. } => vec![*id],
            Edit::MoveVertices { moves } => moves.iter().map(|&(id, _)| id).collect(),
            _ => Vec::new(),
        }
    }
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
    /// Whether merging (see [`Edit::Merging`]) cleared anything away: if
    /// not, it was just the edit merged.
    pub merged: bool,
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
        // One there isn't (of another drawing, say) is in no shape.
        if vertex >= self.vertices.len() {
            return Vec::new();
        }
        // Each vertex's triangles, then out from `vertex` through them.
        let mut around: Vec<Vec<TriangleId>> = vec![Vec::new(); self.vertices.len()];
        for (i, t) in self.triangles.iter().enumerate() {
            for &v in t {
                around[v].push(i);
            }
        }
        let mut reached = vec![false; self.vertices.len()];
        let mut in_shape = vec![false; self.triangles.len()];
        let mut next = vec![vertex];
        reached[vertex] = true;
        while let Some(v) = next.pop() {
            for &i in &around[v] {
                if std::mem::replace(&mut in_shape[i], true) {
                    continue;
                }
                for &w in &self.triangles[i] {
                    if !std::mem::replace(&mut reached[w], true) {
                        next.push(w);
                    }
                }
            }
        }
        // In the order they're in.
        self.triangles
            .iter()
            .zip(in_shape)
            .filter_map(|(&t, inside)| inside.then_some(t))
            .collect()
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
        // Marked, then gathered in order: quicker than sorting them all.
        let mut marked = vec![false; self.vertices.len()];
        for v in self.used_vertices() {
            marked[v] = true;
        }
        marked
            .iter()
            .enumerate()
            .filter_map(|(v, &used)| used.then_some(v))
            .collect()
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
        if !document.is_valid_after(&Document::default(), &QuickSet::default()) {
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

    /// Applies several edits as one: all of them, or none if any is rejected.
    pub fn apply_all(&mut self, edits: &[Edit]) -> bool {
        match self.preview(edits) {
            Some((next, _)) => {
                *self = next;
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
            let moved = edit.moved();
            // A pasted piece's faces, as they were copied (see
            // `is_valid_after`).
            let pasted: QuickSet<[(u32, u32); 3]> = match edit.inner() {
                Edit::Paste { piece } => piece
                    .triangles
                    .iter()
                    .map(|t| corners_key(t.map(|v| piece.vertices[v])))
                    .collect(),
                _ => QuickSet::default(),
            };
            let valid = match edit {
                Edit::Merging { edit, snap, held } => next.merge(
                    edit,
                    *snap,
                    held,
                    &moved,
                    &pasted,
                    &mut changes,
                    &document,
                    deadline,
                ),
                _ => {
                    next.apply_unchecked(edit.clone(), &mut changes, deadline)
                        && next.normalize(&moved, &mut changes, &document)
                        && next.is_valid_after(&document, &pasted)
                }
            };
            if !valid {
                return None;
            }
            if paint {
                document.paint_after(edit, &mut next);
            }
            document = next;
        }

        // Another drawing than this one, for views to tell apart.
        document.revision = next_revision();
        Some((document, changes))
    }

    /// The faces with all their corners among `vertices`, as a piece
    /// apart, painted alike (edges both of whose ends are in it too).
    pub fn piece(&self, vertices: &[VertexId]) -> Piece {
        let inside: QuickSet<VertexId> = vertices.iter().copied().collect();
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

    /// The triangle containing `point` (on its boundary counts), if any.
    pub fn triangle_at(&self, point: Point) -> Option<TriangleId> {
        self.triangles().position(|t| contains(t, point))
    }

    /// Finds a triangle with these corners, in any order.
    #[cfg(test)]
    pub fn find_triangle(&self, mut ids: [VertexId; 3]) -> Option<TriangleId> {
        ids.sort_unstable();

        self.triangles.iter().position(|t| {
            let mut t = *t;
            t.sort_unstable();
            t == ids
        })
    }
}

/// Whether the positively wound triangle `t` contains `point` (on its
/// boundary counts): it's left of each edge.
fn contains([a, b, c]: [Point; 3], point: Point) -> bool {
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .all(|(u, v)| area2(u, v, point) >= 0.0)
}

/// A face's corners in a fixed order, to tell it by whichever corner comes
/// first.
fn key(mut t: [VertexId; 3]) -> [VertexId; 3] {
    t.sort_unstable();
    t
}

/// A triangle's corners, in a fixed order, to tell it by where it is
/// (exactly: a face moved or pasted as it was, not one near it).
pub fn corners_key(corners: [Point; 3]) -> [(u32, u32); 3] {
    let mut key = corners.map(|p| (p.x.to_bits(), p.y.to_bits()));
    key.sort_unstable();
    key
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
mod tests;

#[cfg(test)]
mod fuzz;
