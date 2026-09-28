//! Adding a triangle where it may run into existing geometry: only the part
//! not covered yet is added, triangulated so it joins the mesh properly.

use iced::Point;

use super::{Corner, Document, VertexId, is_flat};
use crate::geometry::area2;

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
    /// Its corners, the existing vertices inside it and the points where
    /// existing edges cross its sides are joined greedily by the shortest
    /// edges that stay in the uncovered part and cross nothing, which
    /// divides that part into triangles. Returns `false` if nothing is left
    /// to cover.
    pub(super) fn fill(&mut self, corners: [Corner; 3]) -> bool {
        let points = corners.map(|corner| match corner {
            Corner::Existing(id) => self.vertices[id],
            Corner::New(point) => point,
        });
        let [a, b, c] = points;
        if area2(a, b, c).abs() < EPSILON {
            return false;
        }
        let outline = if area2(a, b, c) > 0.0 {
            [a, b, c]
        } else {
            [a, c, b]
        };

        let mesh: Vec<[Point; 3]> = self.triangles().collect();
        let mut edges: Vec<(Point, Point)> = self
            .edges()
            .map(|(u, v)| (self.vertices[u], self.vertices[v]))
            .collect();
        let sides = [(a, b), (b, c), (c, a)];

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

        for (corner, point) in corners.into_iter().zip(points) {
            let vertex = match corner {
                Corner::Existing(id) => Some(id),
                Corner::New(_) => None,
            };
            add(Candidate { point, vertex });
        }
        for id in self.used_vertices() {
            let point = self.vertices[id];
            if inside_or_on(outline, point) {
                add(Candidate {
                    point,
                    vertex: Some(id),
                });
            }
        }
        for &(u, v) in &edges {
            for (s, t) in sides {
                if let Some(point) = crossing(s, t, u, v) {
                    add(Candidate {
                        point,
                        vertex: None,
                    });
                }
            }
        }

        // Nothing may cross the existing edges or the triangle's sides.
        edges.extend(sides);
        let covered = |p: Point| mesh.iter().any(|&t| strictly_inside(t, p));
        let in_gap = |p: Point| inside_or_on(outline, p) && !covered(p);

        // All usable connections, shortest first.
        let mut connections: Vec<(f32, usize, usize)> = Vec::new();
        for i in 0..candidates.len() {
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
            return false;
        }

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

        // New corners in the middle of existing edges split those triangles.
        self.split_t_junctions()
    }
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
fn crossing(p: Point, q: Point, a: Point, b: Point) -> Option<Point> {
    let (sp, sq) = (side(a, b, p), side(a, b, q));
    let (sa, sb) = (side(p, q, a), side(p, q, b));
    let apart = |x: f32, y: f32| (x > EPSILON && y < -EPSILON) || (x < -EPSILON && y > EPSILON);

    (apart(sp, sq) && apart(sa, sb)).then(|| p + (q - p) * (sp / (sp - sq)))
}
