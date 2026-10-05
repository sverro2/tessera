//! Extruding the selection (E): its outer edges swept along as far as the
//! cursor moved, each into a quad joined on behind it; a click puts it
//! down, and the new edges are selected, to go on extruding from.
//!
//! Only outer edges (with a face on one side only) with both ends selected
//! are swept, and only those whose open side the cursor went to: an edge
//! facing the other way, or whose quad would come out flat, is left out.
//! A quad running into what's there (or into another edge's) is fitted in
//! round it, the sweeps merging; if that can't be done, it's left out.

use std::collections::{HashMap, HashSet};

use super::*;
use crate::geometry::overlap;

/// A quad swept from an edge that runs into something, to fit in round it:
/// the face on the edge, the edge (`a` → `b`), and its two triangles.
type Merging = (TriangleId, VertexId, VertexId, [[Point; 3]; 2]);

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

    /// `selection` extruded by `by`: the quads clear of everything, as a
    /// piece to join on (a quad, two triangles, for each outer edge that
    /// can be swept that way, painted like the face on it, its edges
    /// styled like the edge swept); and those running into what's
    /// there or into each other, to fit in round that (see [`Self::merged`]),
    /// each with the face on its edge and that edge.
    pub(super) fn extrusion_parts(
        &self,
        selection: &[VertexId],
        by: Vector,
    ) -> (Piece, Vec<Merging>) {
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
        let height = |t: &[Point; 3]| min_height(t.map(|p| self.camera.to_screen(p)));

        let mut quads: Vec<(VertexId, VertexId, TriangleId, [[Point; 3]; 2])> = Vec::new();
        let mut merging = Vec::new();
        for (a, b, face) in self.outer_edges(selection) {
            let (pa, pb) = (document.vertex(a), document.vertex(b));
            // The face is to the left of `a` → `b`: open to the right.
            if area2(pa, pb, pa + by) >= 0.0 {
                continue;
            }
            let quad = [[pb, pa, pa + by], [pb, pa + by, pb + by]];
            // Thin is fine (a hole would be worse), as long as it isn't flat.
            if quad.iter().any(|t| height(t) < FLAT) {
                continue;
            }
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
            if quad.iter().all(clear) {
                quads.push((a, b, face, quad));
            } else if quad.iter().all(|t| height(t) >= MIN_THICKNESS) {
                // Fitted in, it mustn't leave slivers: thick enough to.
                merging.push((face, a, b, quad));
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
        (piece, merging)
    }

    /// The edits putting down `piece`, then fitting each of the `merging`
    /// quads in round what's there by then (only what's still uncovered,
    /// joined up: where sweeps run into each other, they merge), painted
    /// like the face on its edge and styled like that edge. `None` if
    /// none of them fits.
    fn merged(&self, piece: Piece, merging: &[Merging]) -> Option<Vec<Edit>> {
        let mut edits = Vec::new();
        let mut result = self.document.clone();
        if !piece.is_empty() {
            let paste = Edit::Paste { piece };
            if !result.apply(paste.clone()) {
                return None;
            }
            edits.push(paste);
        }
        let before = edits.len();
        let snap = self.snap_distance();
        let mut filled = Vec::new();
        for &(face, a, b, quad) in merging {
            for corners in quad {
                let fill = Edit::AddTriangle { corners, snap };
                if result.apply(fill.clone()) {
                    edits.push(fill);
                    filled.push((face, a, b, corners));
                }
            }
        }
        if edits.len() == before {
            return None;
        }

        // Each new face painted like the face on the edge swept over it.
        let existed: HashSet<[VertexId; 3]> = {
            let mut after = self.document.clone();
            for edit in &edits[..before] {
                after.apply(edit.clone());
            }
            after.triangle_ids().iter().map(|&t| sorted(t)).collect()
        };
        let swept = |p: Point| {
            filled
                .iter()
                .find(|(.., corners)| inside_polygon(p, corners))
        };
        let mut styled = HashSet::new();
        for (t, &ids) in result.triangle_ids().iter().enumerate() {
            if existed.contains(&sorted(ids)) {
                continue;
            }
            let [p, q, r] = ids.map(|v| result.vertex(v));
            let centre = Point::new((p.x + q.x + r.x) / 3.0, (p.y + q.y + r.y) / 3.0);
            let Some(&(face, a, b, _)) = swept(centre) else {
                continue;
            };
            if let Some(color) = self.document.color(face) {
                edits.push(Edit::Paint {
                    triangle: t,
                    color: Some(color),
                });
            }
            if let Some(style) = self.document.edge_style(a, b) {
                for (u, v) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
                    if styled.insert((u.min(v), u.max(v))) && result.edge_style(u, v).is_none() {
                        edits.push(Edit::PaintEdge {
                            a: u,
                            b: v,
                            style: Some(style),
                        });
                    }
                }
            }
        }
        Some(edits)
    }

    /// Extruding `selection` by `by` (the cursor at `at`); `None` if no
    /// edge can be swept that way, or it doesn't work out. Sweeps running
    /// into something are fitted in round it if they can be, else left out.
    pub(super) fn extrude(&self, selection: &[VertexId], by: Vector, at: Point) -> Option<Pending> {
        let (piece, merging) = self.extrusion_parts(selection, by);
        if !merging.is_empty()
            && let Some(edits) = self.merged(piece.clone(), &merging)
            && let Some(pending) = self.check(edits, at)
        {
            return Some(pending);
        }
        if piece.is_empty() {
            return None;
        }
        self.check(vec![Edit::Paste { piece }], at)
    }

    /// The vertices at the far ends of what `pending` extruded `selection`
    /// into: to go on extruding from.
    pub(super) fn extruded(&self, selection: &[VertexId], pending: &Pending) -> Vec<VertexId> {
        // Where the far ends were put: the piece's, and the fitted quads'.
        let put: Vec<Point> = pending
            .edits
            .iter()
            .flat_map(|edit| match edit {
                Edit::Paste { piece } => piece.vertices.clone(),
                Edit::AddTriangle { corners, .. } => corners.to_vec(),
                _ => Vec::new(),
            })
            .collect();
        let near = |p: Point, q: Point| p.distance(q) < 1e-3 * (1.0 + p.x.abs().max(p.y.abs()));
        let result = &pending.result;
        let used = result.unique_vertices();
        let mut far: Vec<VertexId> = put
            .iter()
            .filter(|&&p| !selection.iter().any(|&v| near(self.document.vertex(v), p)))
            .filter_map(|&p| used.iter().copied().find(|&v| near(result.vertex(v), p)))
            .collect();
        far.sort_unstable();
        far.dedup();
        far
    }
}

/// A triangle's corners in order: the same for each way round.
fn sorted(mut t: [VertexId; 3]) -> [VertexId; 3] {
    t.sort_unstable();
    t
}
