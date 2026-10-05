//! Adding a triangle where it may run into existing geometry: only the part
//! not covered yet is added, triangulated so it joins the mesh properly.

use std::time::Instant;

use iced::Point;

use super::{Changed, Changes, Document, VertexId, is_flat};
use crate::geometry::{area2, closest_on_segment, min_height};

/// Distance (world units) within which points count as coinciding, or as
/// lying on a line.
const EPSILON: f32 = 1e-3;

/// A point the fill may use: an existing vertex, or a new one.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    point: Point,
    vertex: Option<VertexId>,
}

impl Document {
    /// Covers the part of the triangle `corners` that no triangle covers yet.
    ///
    /// Things within `snap` of each other count as touching, so the fill
    /// leaves no pieces thinner than that: each corner snaps onto an
    /// existing vertex or edge nearby, and a side passing close to an
    /// existing vertex is bent through it.
    ///
    /// The corners of that outline, the existing vertices inside it and the
    /// points where existing edges cross its sides are joined greedily by
    /// the shortest edges that stay in the uncovered part and cross nothing,
    /// which divides that part into triangles. Returns `false` if nothing is
    /// left to cover.
    ///
    /// Records the existing vertices the new triangles attach to in
    /// `changes`.
    ///
    /// Gives up (returning `false`) once past `deadline`.
    pub(super) fn fill(
        &mut self,
        corners: [Point; 3],
        snap: f32,
        changes: &mut Changes,
        deadline: Option<Instant>,
    ) -> bool {
        /// How often to bend the outline again to get rid of thin pieces.
        const MAX_ROUNDS: usize = 8;

        let snap = snap.max(EPSILON);
        let used = self.unique_vertices();
        let corners = corners.map(|corner| self.snap_point(corner, snap, &used));
        let [a, b, c] = corners;
        if area2(a, b, c).abs() < EPSILON {
            return false;
        }
        let triangle = if area2(a, b, c) > 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        };

        // The vertices the outline bends through: at first those close to a
        // side, then any the fill would otherwise leave in a thin piece.
        let mut through: Vec<VertexId> = used
            .iter()
            .copied()
            .filter(|&id| {
                let p = self.vertices[id];
                let (off, along) = nearest_side(triangle, p);
                along > 0.0 && along < 1.0 && off > EPSILON && off <= snap
            })
            .collect();

        let mut filled = None;
        for _ in 0..MAX_ROUNDS {
            let outline = bent(triangle, &through, |id| self.vertices[id], snap);
            let mut next = self.clone();
            let Some(attached) = next.fill_outline(corners, &outline, deadline) else {
                break;
            };
            if late(deadline) {
                return false;
            }
            let thin = next.thin_piece(self, snap, &outline);
            filled = Some((next, attached));
            match thin {
                Some(id) => through.push(id),
                None => break,
            }
        }

        match filled {
            Some((next, attached)) => {
                *self = next;
                changes.attached.extend(attached);
                true
            }
            None => false,
        }
    }

    /// An existing vertex of a new triangle thinner than `snap` (once edges
    /// with new vertices on them are split, as normalizing will), which the
    /// `outline` doesn't pass through yet: bending it through there avoids
    /// that piece.
    fn thin_piece(&self, before: &Document, snap: f32, outline: &[Point]) -> Option<VertexId> {
        // Only what the fill changed can have T-junctions (`before` has
        // none), and only its new triangles can be thin.
        let changed = Changed::since(self, before);
        let mut after = self.clone();
        after.split_t_junctions(&mut Default::default(), Some(&changed));
        let used = before.unique_vertices();

        after
            .triangle_ids()
            .iter()
            .filter(|&&t| changed.is_new(t))
            .filter(|t| min_height(t.map(|v| after.vertices[v])) < snap)
            .flat_map(|t| t.iter().copied())
            .find(|v| used.binary_search(v).is_ok() && !outline.contains(&after.vertices[*v]))
    }

    /// Covers the part of the polygon `outline` that no triangle covers yet;
    /// see [`Self::fill`]. New vertices at `corners` get the lowest ids.
    ///
    /// Returns the existing vertices the new triangles attach to, or `None`
    /// if nothing is left to cover.
    fn fill_outline(
        &mut self,
        corners: [Point; 3],
        outline: &[Point],
        deadline: Option<Instant>,
    ) -> Option<Vec<VertexId>> {
        let used = self.unique_vertices();
        let sides: Vec<(Point, Point)> = (0..outline.len())
            .map(|i| (outline[i], outline[(i + 1) % outline.len()]))
            .collect();

        // Only what reaches into the outline's bounds can matter.
        let (min, max) = super::cells::bounds(outline.iter().copied());
        let reaches = |points: &[Point]| {
            let (lo, hi) = super::cells::bounds(points.iter().copied());
            hi.x >= min.x - EPSILON
                && lo.x <= max.x + EPSILON
                && hi.y >= min.y - EPSILON
                && lo.y <= max.y + EPSILON
        };
        let mesh: Vec<[Point; 3]> = self.triangles().filter(|t| reaches(t)).collect();
        let mut edges: Vec<(Point, Point)> = self
            .edges()
            .map(|(u, v)| (self.vertices[u], self.vertices[v]))
            .filter(|&(p, q)| reaches(&[p, q]))
            .collect();

        // Where the fill may put corners.
        let mut candidates: Vec<Candidate> = Vec::new();
        let mut add = |candidate: Candidate| {
            if let Some(existing) = candidates
                .iter_mut()
                .find(|other| other.point.distance(candidate.point) < EPSILON)
            {
                existing.vertex = existing.vertex.or(candidate.vertex);
            } else {
                candidates.push(candidate);
            }
        };

        // Corners on existing vertices pick those up below. New vertices
        // are numbered in the order of `corners`.
        for &point in corners.iter().chain(outline) {
            add(Candidate {
                point,
                vertex: None,
            });
        }
        for &id in &used {
            let point = self.vertices[id];
            if in_polygon(outline, point) {
                add(Candidate {
                    point,
                    vertex: Some(id),
                });
            }
        }
        for &(u, v) in &edges {
            for &(s, t) in &sides {
                if let Some(point) = crossing(s, t, u, v) {
                    add(Candidate {
                        point,
                        vertex: None,
                    });
                }
            }
        }

        // Nothing may cross the existing edges or the outline.
        edges.extend(sides);
        let covered = |p: Point| mesh.iter().any(|&t| strictly_inside(t, p));
        let in_gap = |p: Point| in_polygon(outline, p) && !covered(p);

        // All usable connections, shortest first.
        let mut connections: Vec<(f32, usize, usize)> = Vec::new();
        for i in 0..candidates.len() {
            if late(deadline) {
                return None;
            }
            for j in i + 1..candidates.len() {
                let (p, q) = (candidates[i].point, candidates[j].point);
                let through_other = candidates
                    .iter()
                    .enumerate()
                    .any(|(k, other)| k != i && k != j && strictly_between(other.point, p, q));
                let crosses_edge = edges.iter().any(|&(u, v)| crossing(p, q, u, v).is_some());
                let midpoint = Point::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);

                if !through_other && !crosses_edge && in_gap(midpoint) {
                    connections.push((p.distance(q), i, j));
                }
            }
        }
        connections.sort_by(|x, y| x.0.total_cmp(&y.0));

        // Greedily keep those that don't cross one kept before. What remains
        // divides the gap into triangles.
        let mut kept: Vec<(usize, usize)> = Vec::new();
        for (_, i, j) in connections {
            if late(deadline) {
                return None;
            }
            let (p, q) = (candidates[i].point, candidates[j].point);
            let crosses_kept = kept
                .iter()
                .any(|&(k, l)| crossing(p, q, candidates[k].point, candidates[l].point).is_some());
            if !crosses_kept {
                kept.push((i, j));
            }
        }

        let connected = |i: usize, j: usize| kept.contains(&(i.min(j), i.max(j)));
        let mut faces: Vec<[usize; 3]> = Vec::new();
        for &(i, j) in &kept {
            if late(deadline) {
                return None;
            }
            for k in j + 1..candidates.len() {
                if !connected(i, k) || !connected(j, k) {
                    continue;
                }
                let t = [i, j, k].map(|n| candidates[n].point);
                let [p, q, r] = t;
                let centroid = Point::new((p.x + q.x + r.x) / 3.0, (p.y + q.y + r.y) / 3.0);
                let empty = !candidates.iter().enumerate().any(|(n, other)| {
                    n != i && n != j && n != k && inside_or_on(oriented(t), other.point)
                });

                if !is_flat(t) && empty && in_gap(centroid) {
                    faces.push([i, j, k]);
                }
            }
        }

        if faces.is_empty() {
            return None;
        }
        // New corners in the middle of existing edges are joined up when
        // the document is normalized.

        let mut attached: Vec<VertexId> = faces
            .iter()
            .flatten()
            .filter_map(|&n| candidates[n].vertex)
            .collect();
        attached.sort_unstable();
        attached.dedup();

        let mut ids: Vec<Option<VertexId>> = candidates.iter().map(|c| c.vertex).collect();
        for face in faces {
            let face = face.map(|n| {
                *ids[n].get_or_insert_with(|| {
                    self.vertices.push(candidates[n].point);
                    self.vertices.len() - 1
                })
            });
            self.push_triangle(face);
        }
        Some(attached)
    }
}

impl Document {
    /// `point`, snapped onto the closest of the `used` vertices within
    /// `snap`, else onto the closest edge within `snap`.
    fn snap_point(&self, point: Point, snap: f32, used: &[VertexId]) -> Point {
        let closest = |candidates: &mut dyn Iterator<Item = Point>| {
            candidates
                .map(|p| (p, p.distance(point)))
                .filter(|&(_, d)| d <= snap)
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .map(|(p, _)| p)
        };

        closest(&mut used.iter().map(|&v| self.vertices[v]))
            .or_else(|| {
                closest(
                    &mut self.edges().map(|(u, v)| {
                        closest_on_segment(point, self.vertices[u], self.vertices[v]).0
                    }),
                )
            })
            .unwrap_or(point)
    }
}

/// The positively wound `triangle` as a polygon whose sides are bent
/// through the vertices `through` (at `position`), each on the side it's
/// nearest to. Vertices within `snap` of a corner are left out: the corner
/// snapped onto them already.
fn bent(
    triangle: [Point; 3],
    through: &[VertexId],
    position: impl Fn(VertexId) -> Point,
    snap: f32,
) -> Vec<Point> {
    let mut bends: [Vec<(f32, Point)>; 3] = Default::default();
    for &id in through {
        let p = position(id);
        if triangle.iter().any(|corner| corner.distance(p) <= snap) {
            continue;
        }
        let (i, along) = (0..3)
            .map(|i| {
                let (s, t) = (triangle[i], triangle[(i + 1) % 3]);
                (i, closest_on_segment(p, s, t).1, along_and_off(p, s, t).0)
            })
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(i, _, along)| (i, along))
            .unwrap();
        bends[i].push((along, p));
    }

    let mut outline = Vec::new();
    for (i, mut bent) in bends.into_iter().enumerate() {
        outline.push(triangle[i]);
        bent.sort_by(|x, y| x.0.total_cmp(&y.0));
        outline.extend(bent.into_iter().map(|(_, p)| p));
    }
    outline
}

/// The distance from `p` to the nearest side of `triangle`, and how far
/// along that side (as a fraction) it is.
fn nearest_side(triangle: [Point; 3], p: Point) -> (f32, f32) {
    (0..3)
        .map(|i| {
            let (s, t) = (triangle[i], triangle[(i + 1) % 3]);
            let (along, off) = along_and_off(p, s, t);
            (off, along)
        })
        .filter(|&(_, along)| along > 0.0 && along < 1.0)
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .unwrap_or((f32::INFINITY, 0.0))
}

/// How far along `s` → `t` (as a fraction) the projection of `p` is, and
/// how far `p` is off that line.
fn along_and_off(p: Point, s: Point, t: Point) -> (f32, f32) {
    let length = s.distance(t).max(f32::MIN_POSITIVE);
    let along = ((p - s).x * (t - s).x + (p - s).y * (t - s).y) / (length * length);
    (along, side(s, t, p).abs())
}

/// Whether `p` is inside, or on the boundary of, a polygon (any winding).
fn in_polygon(polygon: &[Point], p: Point) -> bool {
    let n = polygon.len();
    let edges = (0..n).map(|i| (polygon[i], polygon[(i + 1) % n]));
    if edges
        .clone()
        .any(|(a, b)| closest_on_segment(p, a, b).1 <= EPSILON)
    {
        return true;
    }

    // Even-odd rule: count the edges a ray to the right crosses.
    edges
        .filter(|&(a, b)| (a.y > p.y) != (b.y > p.y))
        .filter(|&(a, b)| p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x))
        .count()
        % 2
        == 1
}

fn late(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

fn oriented([a, b, c]: [Point; 3]) -> [Point; 3] {
    if area2(a, b, c) > 0.0 {
        [a, b, c]
    } else {
        [a, c, b]
    }
}

/// Signed distance of `p` from the line `a`→`b`; positive on its left
/// (the inside of a positively wound triangle).
fn side(a: Point, b: Point, p: Point) -> f32 {
    area2(a, b, p) / a.distance(b).max(f32::MIN_POSITIVE)
}

/// Whether `p` is inside, or on the boundary of, a positively wound triangle.
fn inside_or_on([a, b, c]: [Point; 3], p: Point) -> bool {
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .all(|(u, v)| side(u, v, p) >= -EPSILON)
}

/// Whether `p` is inside a positively wound triangle, clear of its boundary.
fn strictly_inside([a, b, c]: [Point; 3], p: Point) -> bool {
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .all(|(u, v)| side(u, v, p) > EPSILON)
}

/// Whether `p` lies on segment `a`–`b`, away from its ends.
fn strictly_between(p: Point, a: Point, b: Point) -> bool {
    side(a, b, p).abs() < EPSILON
        && p.distance(a) > EPSILON
        && p.distance(b) > EPSILON
        && (p - a).x * (b - a).x + (p - a).y * (b - a).y > 0.0
        && (p - b).x * (a - b).x + (p - b).y * (a - b).y > 0.0
}

/// Where segments `p`–`q` and `a`–`b` properly cross (not just touch).
pub(super) fn crossing(p: Point, q: Point, a: Point, b: Point) -> Option<Point> {
    let (sp, sq) = (side(a, b, p), side(a, b, q));
    let (sa, sb) = (side(p, q, a), side(p, q, b));
    let apart = |x: f32, y: f32| (x > EPSILON && y < -EPSILON) || (x < -EPSILON && y > EPSILON);

    (apart(sp, sq) && apart(sa, sb)).then(|| p + (q - p) * (sp / (sp - sq)))
}
