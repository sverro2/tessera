//! Extruding the selection (E): its outer edges swept along as far as the
//! cursor moved, each into a quad joined on behind it; a click puts it
//! down, and the new edges are selected, to go on extruding from.
//!
//! Only outer edges (with a face on one side only) with both ends selected
//! are swept, and only those whose open side the cursor went to: an edge
//! facing the other way, or whose quad would run into what's there (or
//! into another edge's), or come out too thin to see, is left out.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::geometry::overlap;

impl Editor<'_> {
    /// The outer edges with both ends in `selection`, each wound as its
    /// face winds it (`a` → `b`, the face to the left), with that face.
    pub(super) fn outer_edges(
        &self,
        selection: &[VertexId],
    ) -> Vec<(VertexId, VertexId, TriangleId)> {
        let selected: HashSet<VertexId> = selection.iter().copied().collect();
        let triangles = self.document.triangle_ids();
        let wound: HashSet<(VertexId, VertexId)> = triangles
            .iter()
            .flat_map(|&[a, b, c]| [(a, b), (b, c), (c, a)])
            .collect();
        triangles
            .iter()
            .enumerate()
            .flat_map(|(t, &[a, b, c])| [(a, b, t), (b, c, t), (c, a, t)])
            .filter(|&(a, b, _)| {
                selected.contains(&a) && selected.contains(&b) && !wound.contains(&(b, a))
            })
            .collect()
    }

    /// The vertices extruding `selection` may sweep along: the ends of its
    /// outer edges.
    pub(super) fn extruded_ends(&self, selection: &[VertexId]) -> Vec<VertexId> {
        let mut ends: Vec<VertexId> = self
            .outer_edges(selection)
            .into_iter()
            .flat_map(|(a, b, _)| [a, b])
            .collect();
        ends.sort_unstable();
        ends.dedup();
        ends
    }

    /// `selection` extruded by `by`, as a piece to join on: a quad (two
    /// triangles) for each outer edge that can be swept that way, painted
    /// like the face on it, its edges styled like the edge swept.
    pub(super) fn extrusion(&self, selection: &[VertexId], by: Vector) -> Piece {
        let document = self.document;
        let bounds = |t: &[Point; 3]| {
            let (mut min, mut max) = (t[0], t[0]);
            for p in &t[1..] {
                min = Point::new(min.x.min(p.x), min.y.min(p.y));
                max = Point::new(max.x.max(p.x), max.y.max(p.y));
            }
            (min, max)
        };
        let apart = |(min, max): (Point, Point), (umin, umax): (Point, Point)| {
            max.x < umin.x || umax.x < min.x || max.y < umin.y || umax.y < min.y
        };
        let there: Vec<([Point; 3], (Point, Point))> =
            document.triangles().map(|t| (t, bounds(&t))).collect();
        let thick =
            |t: &[Point; 3]| min_height(t.map(|p| self.camera.to_screen(p))) >= MIN_THICKNESS;

        let mut quads: Vec<(VertexId, VertexId, TriangleId, [[Point; 3]; 2])> = Vec::new();
        for (a, b, face) in self.outer_edges(selection) {
            let (pa, pb) = (document.vertex(a), document.vertex(b));
            // The face is to the left of `a` → `b`: open to the right.
            if area2(pa, pb, pa + by) >= 0.0 {
                continue;
            }
            let quad = [[pb, pa, pa + by], [pb, pa + by, pb + by]];
            let clear = |t: &[Point; 3]| {
                let b = bounds(t);
                there
                    .iter()
                    .all(|(u, ub)| apart(b, *ub) || !overlap(*t, *u))
                    && quads
                        .iter()
                        .flat_map(|(.., other)| other)
                        .all(|u| !overlap(*t, *u))
            };
            if quad.iter().all(|t| thick(t) && clear(t)) {
                quads.push((a, b, face, quad));
            }
        }

        // Each vertex once, near (as it is) or far (swept along).
        let mut piece = Piece::default();
        let mut index: HashMap<(VertexId, bool), usize> = HashMap::new();
        let mut corner = |piece: &mut Piece, v: VertexId, far: bool| {
            *index.entry((v, far)).or_insert_with(|| {
                let p = document.vertex(v);
                piece.vertices.push(if far { p + by } else { p });
                piece.vertices.len() - 1
            })
        };
        for (a, b, face, _) in quads {
            let (na, nb) = (corner(&mut piece, a, false), corner(&mut piece, b, false));
            let (fa, fb) = (corner(&mut piece, a, true), corner(&mut piece, b, true));
            let color = document.color(face);
            piece.triangles.extend([[nb, na, fa], [nb, fa, fb]]);
            piece.colors.extend([color, color]);
            if let Some(style) = document.edge_style(a, b) {
                for (u, v) in [(na, nb), (na, fa), (fa, fb), (fb, nb), (nb, fa)] {
                    piece.edge_styles.push((u, v, style));
                }
            }
        }
        piece
    }

    /// Extruding `selection` by `by` (the cursor at `at`); `None` if no
    /// edge can be swept that way, or it doesn't work out.
    pub(super) fn extrude(&self, selection: &[VertexId], by: Vector, at: Point) -> Option<Pending> {
        let piece = self.extrusion(selection, by);
        if piece.is_empty() {
            return None;
        }
        self.check(vec![Edit::Paste { piece }], at)
    }

    /// The vertices at the far ends of what `pending` extruded `selection`
    /// into: to go on extruding from.
    pub(super) fn extruded(&self, selection: &[VertexId], pending: &Pending) -> Vec<VertexId> {
        let Some(Edit::Paste { piece }) = pending.edits.first() else {
            return Vec::new();
        };
        let near = |p: Point, q: Point| p.distance(q) < 1e-3 * (1.0 + p.x.abs().max(p.y.abs()));
        let result = &pending.result;
        let used = result.unique_vertices();
        let mut far: Vec<VertexId> = piece
            .vertices
            .iter()
            .filter(|&&p| !selection.iter().any(|&v| near(self.document.vertex(v), p)))
            .filter_map(|&p| used.iter().copied().find(|&v| near(result.vertex(v), p)))
            .collect();
        far.sort_unstable();
        far.dedup();
        far
    }
}
