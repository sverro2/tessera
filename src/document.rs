//! The vector document: a triangle mesh with shared vertices.
//!
//! This module knows nothing about rendering or input. All mutations go
//! through [`Edit`] so they can later be recorded for undo/redo or saving.
//!
//! Invariants, checked after every edit (violating edits are rejected):
//! - every triangle has positive winding; one whose winding flips has been
//!   folded over its neighbours,
//! - no two triangles overlap.

use iced::Point;

use crate::geometry::{area2, min_height, overlap};

mod fill;

/// Index of a vertex in [`Document::vertices`].
pub type VertexId = usize;

/// Index of a triangle in [`Document::triangle_ids`]. Not stable across
/// edits that remove triangles.
pub type TriangleId = usize;

#[derive(Debug, Clone, Default)]
pub struct Document {
    vertices: Vec<Point>,
    triangles: Vec<[VertexId; 3]>,
}

/// Where the third corner of a triangle comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Corner {
    New(Point),
    Existing(VertexId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edit {
    /// Add a triangle with three new corners. Where it runs into existing
    /// geometry only the uncovered part is added, joined up with the mesh.
    AddTriangle([Point; 3]),
    /// Add a triangle on the edge `a`–`b`, covering (like
    /// [`Edit::AddTriangle`]) only what isn't covered yet.
    ExtendEdge {
        a: VertexId,
        b: VertexId,
        apex: Corner,
    },
    /// Replace a triangle with three triangles meeting at a new vertex `at`.
    SplitTriangle { triangle: TriangleId, at: Point },
    /// Cut the edge `a`–`b` at a new vertex `at` on it: each triangle on
    /// the edge is split in two, towards its opposite corner.
    SplitEdge { a: VertexId, b: VertexId, at: Point },
    /// Remove a triangle, leaving a hole (or a smaller outline).
    RemoveTriangle { triangle: TriangleId },
    /// Move vertex `id` onto the edge `a`–`b` (to `at`, which lies on it).
    /// Triangles squashed flat by this are removed, and triangles that now
    /// have a vertex in the middle of an edge are split at it, so the mesh
    /// stays conforming (no vertex sits in the middle of another triangle's
    /// edge).
    CollapseOntoEdge {
        id: VertexId,
        a: VertexId,
        b: VertexId,
        at: Point,
    },
    /// Weld vertex `id` onto its neighbour `into`. Triangles squashed by
    /// this are removed, and the mesh is kept conforming as for
    /// [`Edit::CollapseOntoEdge`].
    MergeVertex { id: VertexId, into: VertexId },
    /// Move a vertex (and thereby every triangle using it). Where the move
    /// would fold a triangle over, the mesh is rearranged around the vertex
    /// instead; see [`Document::move_vertex`].
    MoveVertex { id: VertexId, to: Point },
}

impl Document {
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

    /// Vertices used by at least one triangle (with repeats).
    pub fn used_vertices(&self) -> impl Iterator<Item = VertexId> + '_ {
        self.triangles.iter().flatten().copied()
    }

    /// Vertices sharing a triangle with `id` (with repeats).
    pub fn neighbours(&self, id: VertexId) -> impl Iterator<Item = VertexId> + '_ {
        self.triangles
            .iter()
            .filter(move |t| t.contains(&id))
            .flatten()
            .copied()
            .filter(move |&v| v != id)
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

    /// The number of vertices used by triangles.
    pub fn vertex_count(&self) -> usize {
        let mut used: Vec<_> = self.used_vertices().collect();
        used.sort_unstable();
        used.dedup();
        used.len()
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

    /// Applies an edit. Returns `false` (leaving the document unchanged) if
    /// the edit was rejected, e.g. because it would produce a degenerate,
    /// duplicate, folded-over or overlapping triangle.
    pub fn apply(&mut self, edit: Edit) -> bool {
        let mut next = self.clone();

        if next.apply_unchecked(edit) && next.is_valid_after(self) {
            *self = next;
            true
        } else {
            false
        }
    }

    /// Checks the invariants, assuming they held in `before`: only triangles
    /// that are new or have a moved corner need checking for overlaps.
    fn is_valid_after(&self, before: &Document) -> bool {
        if !self.triangles().all(|[a, b, c]| area2(a, b, c) > 0.0) {
            return false;
        }

        let changed = |t: &[VertexId; 3]| {
            before.find_triangle(*t).is_none()
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
        let mut next = self.clone();

        if edits.iter().all(|&edit| next.apply(edit)) {
            *self = next;
            true
        } else {
            false
        }
    }

    fn apply_unchecked(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::AddTriangle(corners) => return self.fill(corners.map(Corner::New)),
            Edit::ExtendEdge { a, b, apex } => {
                if apex == Corner::Existing(a) || apex == Corner::Existing(b) {
                    return false;
                }
                return self.fill([Corner::Existing(a), Corner::Existing(b), apex]);
            }
            Edit::SplitTriangle { triangle, at } => {
                let [a, b, c] = self.triangles[triangle];
                let [pa, pb, pc] = [a, b, c].map(|id| self.vertices[id]);

                if [[pa, pb, at], [pb, pc, at], [pc, pa, at]]
                    .into_iter()
                    .any(is_degenerate)
                {
                    return false;
                }

                self.vertices.push(at);
                let d = self.vertices.len() - 1;

                self.triangles.remove(triangle);
                for t in [[a, b, d], [b, c, d], [c, a, d]] {
                    self.push_triangle(t);
                }
            }
            Edit::RemoveTriangle { triangle } => {
                if triangle >= self.triangles.len() {
                    return false;
                }
                self.triangles.remove(triangle);
            }
            Edit::SplitEdge { a, b, at } => {
                let (pa, pb) = (self.vertices[a], self.vertices[b]);
                if at.distance(pa) < 1e-3 || at.distance(pb) < 1e-3 {
                    return false;
                }

                self.vertices.push(at);
                let d = self.vertices.len() - 1;

                let mut split = Vec::new();
                self.triangles.retain(|t| {
                    let on_edge = t.contains(&a) && t.contains(&b);
                    if on_edge {
                        let x = t.iter().copied().find(|&v| v != a && v != b).unwrap();
                        split.extend([[a, d, x], [d, b, x]]);
                    }
                    !on_edge
                });

                if split.is_empty() {
                    return false;
                }
                for t in split {
                    self.push_triangle(t);
                }
            }
            Edit::CollapseOntoEdge { id, a, b, at } => {
                if !self.has_triangle([id, a, b]) {
                    return false;
                }

                self.vertices[id] = at;

                return self.tidy_after_moving(id);
            }
            Edit::MergeVertex { id, into } => {
                if id == into || !self.neighbours(id).any(|v| v == into) {
                    return false;
                }

                self.vertices[id] = self.vertices[into];

                for t in &mut self.triangles {
                    for v in t.iter_mut().filter(|v| **v == id) {
                        *v = into;
                    }
                }

                // Drop squashed triangles and any duplicates the weld created.
                let mut seen = Vec::new();
                self.triangles.retain(|t| {
                    let mut key = *t;
                    key.sort_unstable();
                    let keep = key[0] != key[1] && key[1] != key[2] && !seen.contains(&key);
                    seen.push(key);
                    keep
                });

                return self.tidy_after_moving(into);
            }
            Edit::MoveVertex { id, to } => return self.move_vertex(id, to),
        }

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
                    (then <= 0.0).then(|| (i, (now / (now - then)).clamp(0.0, 1.0)))
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

    /// After vertex `id` moved onto an edge or another vertex: removes the
    /// triangles around it that are now flat, then makes the mesh conforming
    /// again (see [`Self::split_t_junctions`]).
    fn tidy_after_moving(&mut self, id: VertexId) -> bool {
        let vertices = &self.vertices;
        self.triangles
            .retain(|t| !(t.contains(&id) && is_flat(t.map(|v| vertices[v]))));

        self.split_t_junctions()
    }

    /// Splits every triangle that has a vertex lying in the middle of one of
    /// its edges (a "T-junction") into two, towards its opposite corner, so
    /// that vertex becomes a proper corner on both sides of the edge.
    ///
    /// (Nearly) flat triangles are removed first: they cover nothing, and in
    /// one a vertex can lie on two edges at once, which would split forever.
    /// The edges such a triangle joined are then repaired like any other.
    ///
    /// Returns `false` if that doesn't settle.
    fn split_t_junctions(&mut self) -> bool {
        let limit = 4 * (self.triangles.len() + self.vertices.len());

        for _ in 0..limit {
            let vertices = &self.vertices;
            self.triangles.retain(|t| !is_flat(t.map(|v| vertices[v])));

            let mut used: Vec<VertexId> = self.used_vertices().collect();
            used.sort_unstable();
            used.dedup();

            let vertices = &self.vertices;
            let junction = self.triangles.iter().enumerate().find_map(|(i, &t)| {
                (0..3).find_map(|k| {
                    let (u, v, x) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    used.iter()
                        .copied()
                        .find(|&w| {
                            !t.contains(&w)
                                && is_inside_segment(vertices[w], vertices[u], vertices[v])
                        })
                        .map(|w| (i, [u, w, x], [w, v, x]))
                })
            });

            let Some((i, first, second)) = junction else {
                return true;
            };

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

    fn has_triangle(&self, ids: [VertexId; 3]) -> bool {
        self.find_triangle(ids).is_some()
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

/// The corners of `t` other than `id`, in winding order after it.
fn opposite(t: [VertexId; 3], id: VertexId) -> Option<(VertexId, VertexId)> {
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

fn is_degenerate([a, b, c]: [Point; 3]) -> bool {
    area2(a, b, c).abs() < 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with_triangle() -> Document {
        let mut doc = Document::default();
        assert!(doc.apply(Edit::AddTriangle([
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(5.0, 10.0),
        ])));
        doc
    }

    #[test]
    fn extending_shares_the_edge() {
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0))
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
        assert!(doc.apply(Edit::SplitTriangle { triangle: 0, at }));
        assert_eq!(doc.triangle_ids(), &[[0, 1, 3], [1, 2, 3], [2, 0, 3]]);
    }

    #[test]
    fn collapsing_a_corner_removes_the_triangle() {
        let mut doc = doc_with_triangle();
        assert!(doc.apply(Edit::CollapseOntoEdge {
            id: 2,
            a: 0,
            b: 1,
            at: Point::new(5.0, 0.0),
        }));
        assert!(doc.is_empty());
    }

    fn split_triangle() -> Document {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
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
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        });

        assert_eq!(doc.triangle_at(Point::new(50.0, 50.0)), None);
        let below = doc.triangle_at(Point::new(5.0, -3.0)).unwrap();
        assert!(doc.apply(Edit::RemoveTriangle { triangle: below }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
    }

    #[test]
    fn cutting_an_edge_splits_both_sides() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        });

        assert!(doc.apply(Edit::SplitEdge {
            a: 0,
            b: 1,
            at: Point::new(3.0, 0.0),
        }));
        assert_eq!(
            sorted(&doc),
            vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
        );
        assert_eq!(area_and_t_junctions(&doc), (100.0, false));

        // Right on a corner there's nothing to cut.
        assert!(!doc.apply(Edit::SplitEdge {
            a: 0,
            b: 4,
            at: Point::new(0.0, 0.0),
        }));
    }

    #[test]
    fn collapsing_a_split_center_onto_edge_keeps_two() {
        let mut doc = split_triangle();
        assert!(doc.apply(Edit::CollapseOntoEdge {
            id: 3,
            a: 0,
            b: 1,
            at: Point::new(4.0, 0.0),
        }));
        assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
    }

    #[test]
    fn merging_a_split_center_into_corner_restores_parent() {
        let mut doc = split_triangle();
        assert!(doc.apply(Edit::MergeVertex { id: 3, into: 0 }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
    }

    #[test]
    fn collapsing_onto_a_shared_edge_splits_the_other_side() {
        // Two triangles sharing edge 0–1, the upper one split at 4.
        let mut doc = doc_with_triangle();
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        });
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
            at: Point::new(5.0, 3.0),
        });

        assert!(doc.apply(Edit::CollapseOntoEdge {
            id: 4,
            a: 0,
            b: 1,
            at: Point::new(4.0, 0.0),
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
        doc.apply(Edit::AddTriangle([a, b, Point::new(250.0, 100.0)]));
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(250.0, 500.0)),
        });
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
            at: Point::new(250.0, 230.0),
        });

        for i in 1..40 {
            let t = i as f32 / 40.0;
            let at = a + (b - a) * t;
            let mut doc = doc.clone();
            assert!(doc.apply(Edit::CollapseOntoEdge {
                id: 4,
                a: 0,
                b: 1,
                at
            }));
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
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        });
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
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
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
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
        doc.apply(Edit::SplitTriangle {
            triangle: 0,
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
        assert!(doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, 20.0)),
        }));
        assert_eq!(sorted(&doc), vec![[0, 1, 2], [0, 2, 3], [1, 2, 3]]);
        assert_eq!(area_and_t_junctions(&doc), (100.0, false));
    }

    #[test]
    fn adding_on_top_of_geometry_fills_only_the_gap() {
        let mut doc = doc_with_triangle();

        // A triangle poking out of the top of the existing one: the part
        // outside it is added, and the existing edges it crosses are split.
        assert!(doc.apply(Edit::AddTriangle([
            Point::new(2.0, 2.0),
            Point::new(8.0, 2.0),
            Point::new(5.0, 20.0),
        ])));
        let (area, t_junctions) = area_and_t_junctions(&doc);
        assert!(!t_junctions);
        // The sides cross at y = 5. Above that the new triangle adds 37.5,
        // less the 12.5 of the existing top corner poking into it.
        assert!((area - (50.0 + 37.5 - 12.5)).abs() < 1e-3, "{area}");

        // Entirely inside existing geometry: nothing to add.
        assert!(!doc.apply(Edit::AddTriangle([
            Point::new(4.0, 1.0),
            Point::new(6.0, 1.0),
            Point::new(5.0, 3.0),
        ])));
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
        doc.apply(Edit::AddTriangle([
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ]));
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(200.0, 500.0)),
        });
        let split = |doc: &mut Document, corners: [VertexId; 3], at| {
            let triangle = doc.find_triangle(corners).unwrap();
            assert!(doc.apply(Edit::SplitTriangle { triangle, at }));
        };
        split(&mut doc, [0, 1, 2], Point::new(200.0, 230.0)); // centre 4
        split(&mut doc, [0, 1, 3], Point::new(200.0, 370.0)); // centre 5

        // Collapse the top centre onto the shared edge: the bottom side now
        // has four triangles around 5, two of them along the shared line.
        assert!(doc.apply(Edit::CollapseOntoEdge {
            id: 4,
            a: 0,
            b: 1,
            at: Point::new(200.0, 300.0),
        }));
        assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));

        // Collapse the bottom centre onto the shared line between 0 and 4.
        // That flattens both (0, 4, 5) and (4, 1, 5); the bottom is re-split
        // with a new edge from 4 down to 3.
        assert!(doc.apply(Edit::CollapseOntoEdge {
            id: 5,
            a: 0,
            b: 4,
            at: Point::new(150.0, 300.0),
        }));
        assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));
        assert!(doc.edges().any(|e| e == (4, 3) || e == (3, 4)));
    }

    #[test]
    fn merging_requires_neighbours() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::AddTriangle([
            Point::new(20.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(25.0, 10.0),
        ]));
        assert!(!doc.apply(Edit::MergeVertex { id: 1, into: 3 }));
    }

    #[test]
    fn rejects_degenerate_and_duplicate() {
        let mut doc = doc_with_triangle();
        assert!(!doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(20.0, 0.0))
        }));
        assert!(!doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::Existing(2)
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
            doc.apply(Edit::AddTriangle([
                Point::new(100.0, 300.0),
                Point::new(300.0, 300.0),
                Point::new(200.0, 100.0),
            ]));
            doc.apply(Edit::ExtendEdge {
                a: 0,
                b: 1,
                apex: Corner::New(Point::new(200.0, 500.0)),
            });
            doc.apply(Edit::ExtendEdge {
                a: 1,
                b: 2,
                apex: Corner::New(Point::new(350.0, 150.0)),
            });
            doc.apply(Edit::SplitTriangle {
                triangle: 0,
                at: Point::new(200.0, 230.0),
            });
            doc.apply(Edit::SplitTriangle {
                triangle: 0,
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
            doc.apply(Edit::AddTriangle([
                Point::new(200.0, 300.0),
                Point::new(300.0, 300.0),
                Point::new(250.0, 200.0),
            ]));

            for _ in 0..15 {
                let r: Vec<f32> = (0..8).map(|_| rand()).collect();
                let point = |i: usize| Point::new(r[i] * 500.0, r[i + 1] * 500.0);
                let edit = if r[0] < 0.5 {
                    let edges: Vec<_> = doc.edges().collect();
                    let (a, b) = edges[(r[1] * edges.len() as f32) as usize % edges.len()];
                    Edit::ExtendEdge {
                        a,
                        b,
                        apex: Corner::New(point(2)),
                    }
                } else {
                    Edit::AddTriangle([point(2), point(4), point(6)])
                };

                let started = std::time::Instant::now();
                let mut unchecked = doc.clone();
                if !unchecked.apply_unchecked(edit) {
                    empty += 1;
                } else if doc.apply(edit) {
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
