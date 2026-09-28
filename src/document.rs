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

use crate::geometry::{area2, closest_on_segment, min_height, overlap};

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
    /// Add a free-standing triangle with three new vertices.
    AddTriangle([Point; 3]),
    /// Add a triangle sharing the edge `a`–`b` with existing geometry.
    ExtendEdge {
        a: VertexId,
        b: VertexId,
        apex: Corner,
    },
    /// Replace a triangle with three triangles meeting at a new vertex `at`.
    SplitTriangle { triangle: TriangleId, at: Point },
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
    /// Move a vertex (and thereby every triangle using it).
    MoveVertex { id: VertexId, to: Point },
}

impl Document {
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    pub fn vertex(&self, id: VertexId) -> Point {
        self.vertices[id]
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

    fn apply_unchecked(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::AddTriangle(corners) => {
                if is_degenerate(corners) {
                    return false;
                }

                let base = self.vertices.len();
                self.vertices.extend(corners);
                self.push_triangle([base, base + 1, base + 2]);
            }
            Edit::ExtendEdge { a, b, apex } => {
                let c = match apex {
                    Corner::Existing(id) => {
                        if id == a || id == b || self.has_triangle([a, b, id]) {
                            return false;
                        }
                        id
                    }
                    Corner::New(point) => {
                        if is_degenerate([self.vertices[a], self.vertices[b], point]) {
                            return false;
                        }
                        self.vertices.push(point);
                        self.vertices.len() - 1
                    }
                };

                if is_degenerate([a, b, c].map(|id| self.vertices[id])) {
                    return false;
                }

                self.push_triangle([a, b, c]);
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
            Edit::CollapseOntoEdge { id, a, b, at } => {
                if !self.has_triangle([id, a, b]) {
                    return false;
                }

                self.vertices[id] = at;

                self.tidy_after_moving(id);
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

                self.tidy_after_moving(into);
            }
            Edit::MoveVertex { id, to } => {
                self.vertices[id] = to;
            }
        }

        true
    }

    /// After vertex `id` moved onto an edge or another vertex: removes the
    /// triangles around it that are now flat, then makes the mesh conforming
    /// again (see [`Self::split_t_junctions`]).
    fn tidy_after_moving(&mut self, id: VertexId) {
        let vertices = &self.vertices;
        self.triangles
            .retain(|t| !(t.contains(&id) && is_flat(t.map(|v| vertices[v]))));

        self.split_t_junctions();
    }

    /// Splits every triangle that has a vertex lying in the middle of one of
    /// its edges (a "T-junction") into two, towards its opposite corner, so
    /// that vertex becomes a proper corner on both sides of the edge.
    fn split_t_junctions(&mut self) {
        loop {
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
                break;
            };

            self.triangles.remove(i);
            self.push_triangle(first);
            self.push_triangle(second);
        }
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

/// Relative tolerance for treating points as lying on a line.
const FLAT: f32 = 1e-3;

/// Whether a triangle is flat, relative to its size.
fn is_flat(t: [Point; 3]) -> bool {
    let [a, b, c] = t;
    let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
    min_height(t) <= FLAT * longest
}

/// Whether `p` lies on segment `a`–`b`, away from its ends.
fn is_inside_segment(p: Point, a: Point, b: Point) -> bool {
    let tolerance = FLAT * a.distance(b);
    closest_on_segment(p, a, b).1 <= tolerance
        && p.distance(a) > tolerance
        && p.distance(b) > tolerance
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

    #[test]
    fn folding_over_is_rejected() {
        let mut doc = doc_with_triangle();
        doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        });

        // Pulling the top corner across the shared edge folds its triangle.
        let before = sorted(&doc);
        assert!(!doc.apply(Edit::MoveVertex {
            id: 2,
            to: Point::new(5.0, -5.0),
        }));
        assert_eq!(sorted(&doc), before);
        assert_eq!(doc.vertex(2), Point::new(5.0, 10.0));
    }

    #[test]
    fn overlapping_is_rejected() {
        let mut doc = doc_with_triangle();

        // Extending edge 0–1 towards the side the triangle is already on.
        assert!(!doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, 20.0)),
        }));

        // A separate triangle on top of the existing one.
        assert!(!doc.apply(Edit::AddTriangle([
            Point::new(2.0, 2.0),
            Point::new(8.0, 2.0),
            Point::new(5.0, 20.0),
        ])));

        // Touching along an edge is fine.
        assert!(doc.apply(Edit::ExtendEdge {
            a: 0,
            b: 1,
            apex: Corner::New(Point::new(5.0, -10.0)),
        }));
        assert_eq!(doc.triangle_ids().len(), 2);
    }

    /// Total area and whether any vertex sits in the middle of an edge.
    fn area_and_t_junctions(doc: &Document) -> (f32, bool) {
        let area = doc.triangles().map(|[a, b, c]| area2(a, b, c) / 2.0).sum();
        let used: Vec<_> = doc.used_vertices().collect();
        let t_junction = doc.edges().any(|(u, v)| {
            used.iter()
                .any(|&w| is_inside_segment(doc.vertex(w), doc.vertex(u), doc.vertex(v)))
        });
        (area, t_junction)
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
