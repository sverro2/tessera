//! The vector document: a triangle mesh with shared vertices.
//!
//! This module knows nothing about rendering or input. All mutations go
//! through [`Edit`] so they can later be recorded for undo/redo or saving.

use iced::Point;

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
    /// The triangle squashed flat by this is removed, and a triangle on the
    /// other side of `a`–`b` is split at `id`, so the mesh stays conforming
    /// (no vertex ever sits in the middle of another triangle's edge).
    CollapseOntoEdge {
        id: VertexId,
        a: VertexId,
        b: VertexId,
        at: Point,
    },
    /// Weld vertex `id` onto its neighbour `into`. Triangles that had both
    /// are squashed and removed.
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

    /// Applies an edit. Returns `false` if the edit was rejected (e.g. it
    /// would produce a degenerate or duplicate triangle).
    pub fn apply(&mut self, edit: Edit) -> bool {
        match edit {
            Edit::AddTriangle(corners) => {
                if is_degenerate(corners) {
                    return false;
                }

                let base = self.vertices.len();
                self.vertices.extend(corners);
                self.triangles.push([base, base + 1, base + 2]);
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

                self.triangles.push([a, b, c]);
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
                self.triangles.extend([[a, b, d], [b, c, d], [c, a, d]]);
            }
            Edit::CollapseOntoEdge { id, a, b, at } => {
                let Some(squashed) = self.find_triangle([id, a, b]) else {
                    return false;
                };

                self.vertices[id] = at;

                // Remove the squashed triangle by identity: `at` is only
                // approximately on `a`–`b`, so a flatness test is unreliable.
                self.triangles.remove(squashed);

                // Split the neighbour across `a`–`b` so `id` becomes its corner.
                let mut split = Vec::new();
                self.triangles.retain(|t| {
                    let across = t.contains(&a) && t.contains(&b);
                    if across {
                        let x = t.iter().copied().find(|&v| v != a && v != b).unwrap();
                        split.extend([[a, id, x], [id, b, x]]);
                    }
                    !across
                });
                self.triangles.extend(split);
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
            }
            Edit::MoveVertex { id, to } => {
                self.vertices[id] = to;
            }
        }

        true
    }

    /// The topmost (most recently added) triangle containing `point`.
    pub fn triangle_at(&self, point: Point) -> Option<TriangleId> {
        self.triangle_ids()
            .iter()
            .rposition(|t| contains(t.map(|id| self.vertices[id]), point))
    }

    fn has_triangle(&self, ids: [VertexId; 3]) -> bool {
        self.find_triangle(ids).is_some()
    }

    /// Finds a triangle with these corners, in any order.
    fn find_triangle(&self, mut ids: [VertexId; 3]) -> Option<TriangleId> {
        ids.sort_unstable();

        self.triangles.iter().position(|t| {
            let mut t = *t;
            t.sort_unstable();
            t == ids
        })
    }
}

fn contains([a, b, c]: [Point; 3], p: Point) -> bool {
    let side = |u: Point, v: Point| (v.x - u.x) * (p.y - u.y) - (p.x - u.x) * (v.y - u.y);
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn is_degenerate([a, b, c]: [Point; 3]) -> bool {
    let area2 = (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y);
    area2.abs() < 1e-3
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
        assert_eq!(doc.triangle_ids(), &[[0, 1, 2], [0, 1, 3]]);

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
        assert_eq!(doc.triangle_at(at), Some(0));
        assert!(doc.apply(Edit::SplitTriangle { triangle: 0, at }));
        assert_eq!(doc.triangle_ids(), &[[0, 1, 3], [1, 2, 3], [2, 0, 3]]);
        assert_eq!(doc.triangle_at(Point::new(50.0, 50.0)), None);
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
