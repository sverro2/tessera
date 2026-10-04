//! Edges of different widths, drawn so they don't overlap: each edge as a
//! filled outline, cut off where it meets its neighbours around a vertex.
//!
//! Around a vertex, the edges are taken in order of direction. Between two
//! neighbouring edges, the sides facing each other meet in a point; the
//! line from the vertex to that point is where one edge ends and the other
//! begins, so their outlines share it instead of lying over each other.
//! Where the gap between two neighbours is half a turn or more, their sides
//! don't meet: each edge ends in a round cap up to halfway into the gap.
//!
//! Where an edge runs on into one other edge alone (along a line), the two
//! are as wide where they meet: halfway between their widths, each tapering
//! to it from its far end. So a line changes width smoothly, without a
//! step; where three or more meet, each keeps its own width.

use std::f32::consts::PI;

use iced::{Point, Vector};

/// The outline of each edge `(a, b, width)`, in the same order; vertex
/// positions from `at`. Widths are in the same units as the positions.
pub fn outlines(edges: &[(usize, usize, f32)], at: impl Fn(usize) -> Point) -> Vec<Vec<Point>> {
    // Around each vertex: its edges (index, and whether it starts there),
    // by direction.
    let mut around: std::collections::BTreeMap<usize, Vec<(f32, usize)>> = Default::default();
    for (i, &(a, b, _)) in edges.iter().enumerate() {
        let d = at(b) - at(a);
        around.entry(a).or_default().push((d.y.atan2(d.x), i));
        around.entry(b).or_default().push(((-d.y).atan2(-d.x), i));
    }
    for list in around.values_mut() {
        list.sort_by(|x, y| x.0.total_cmp(&y.0));
    }
    // Each edge's half width at either end: where it runs on into one
    // other edge alone, the two meet halfway between their widths (each
    // tapering to it), so a line thinning or thickening does so smoothly;
    // elsewhere, its own.
    let half = |i: usize, v: usize| {
        let own = edges[i].2 / 2.0;
        match around[&v][..] {
            [(_, j), (_, k)] => (edges[j].2 + edges[k].2) / 4.0,
            _ => own,
        }
    };

    edges
        .iter()
        .enumerate()
        .map(|(i, &(a, b, _))| {
            let mut outline = end(i, a, b, edges, &around, &at, &half);
            outline.extend(end(i, b, a, edges, &around, &at, &half));
            outline
        })
        .collect()
}

/// The part of edge `i`'s outline at vertex `v` (its other end `u`): from
/// its right side (looking from `v` to `u`) round through `v` to its left.
fn end(
    i: usize,
    v: usize,
    u: usize,
    edges: &[(usize, usize, f32)],
    around: &std::collections::BTreeMap<usize, Vec<(f32, usize)>>,
    at: &impl Fn(usize) -> Point,
    half: &impl Fn(usize, usize) -> f32,
) -> Vec<Point> {
    let p = at(v);
    let q = at(u);
    let length = p.distance(q);
    let h = half(i, v);
    let list = &around[&v];
    let k = list
        .iter()
        .position(|&(_, j)| j == i && other(edges[j], v) == u)
        .expect("edge at its vertex");
    let angle = list[k].0;
    let d = direction(angle);
    let n = Vector::new(-d.y, d.x);

    // The neighbours: the next edge turning towards the left side, and the
    // one before it, towards the right side.
    let next = list[(k + 1) % list.len()];
    let prev = list[(k + list.len() - 1) % list.len()];
    let gap = |from: f32, to: f32| {
        if list.len() == 1 {
            2.0 * PI
        } else {
            (to - from).rem_euclid(2.0 * PI)
        }
    };
    let left_gap = gap(angle, next.0);
    let right_gap = gap(prev.0, angle);

    // Where this edge's side meets the facing side of a neighbour, if they
    // meet; kept to within half of either edge, so a sharp corner can't
    // reach past a short edge.
    let meet = |side: f32, neighbour: (f32, usize)| -> Point {
        let (other_angle, j) = neighbour;
        let e = direction(other_angle);
        let m = Vector::new(-e.y, e.x);
        let w = other(edges[j], v);
        let other_length = p.distance(at(w));
        // This side: from p + n·h·side to its far end (tapering, maybe);
        // the neighbour's facing side likewise, on the other side of it.
        let from = p + n * (h * side);
        let along = q + n * (half(i, u) * side) - from;
        let other_from = p - m * (half(j, v) * side);
        let other_along = at(w) - m * (half(j, w) * side) - other_from;
        let t = cross(other_from - from, other_along) / cross(along, other_along);
        let corner = from + along * t;
        let limit = 0.5 * length.min(other_length);
        let reach = corner.distance(p);
        if reach > limit {
            p + (corner - p) * (limit / reach)
        } else {
            corner
        }
    };
    // A round cap from this side, round to halfway into the gap (from the
    // edge's direction), `turn` far (negative: clockwise).
    let cap = |from: Vector, turn: f32| -> Vec<Point> {
        let start = from.y.atan2(from.x);
        let steps = (turn.abs() / (PI / 12.0)).ceil().max(1.0) as usize;
        (0..=steps)
            .map(|s| p + direction(start + turn * s as f32 / steps as f32) * h)
            .collect()
    };
    const STRAIGHT: f32 = PI - 1e-3;

    let mut points = Vec::new();
    if right_gap < STRAIGHT {
        points.push(meet(-1.0, prev));
    } else {
        points.extend(cap(-n, -(right_gap / 2.0 - PI / 2.0)));
    }
    points.push(p);
    if left_gap < STRAIGHT {
        points.push(meet(1.0, next));
    } else {
        let mut arc = cap(n, left_gap / 2.0 - PI / 2.0);
        arc.reverse();
        points.extend(arc);
    }
    points
}

/// The end of `edge` other than `v`.
fn other((a, b, _): (usize, usize, f32), v: usize) -> usize {
    if a == v { b } else { a }
}

fn direction(angle: f32) -> Vector {
    Vector::new(angle.cos(), angle.sin())
}

fn cross(a: Vector, b: Vector) -> f32 {
    a.x * b.y - a.y * b.x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(polygon: &[Point]) -> f32 {
        let n = polygon.len();
        (0..n)
            .map(|i| {
                let (p, q) = (polygon[i], polygon[(i + 1) % n]);
                p.x * q.y - q.x * p.y
            })
            .sum::<f32>()
            .abs()
            / 2.0
    }

    /// Whether `p` is inside `polygon` (not on its boundary, roughly).
    fn inside(polygon: &[Point], p: Point) -> bool {
        let n = polygon.len();
        let mut crossings = 0;
        for i in 0..n {
            let (a, b) = (polygon[i], polygon[(i + 1) % n]);
            if (a.y > p.y) != (b.y > p.y) {
                let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if x > p.x {
                    crossings += 1;
                }
            }
        }
        crossings % 2 == 1
    }

    #[test]
    fn a_lone_edge_has_round_ends() {
        let points = [Point::new(0.0, 0.0), Point::new(10.0, 0.0)];
        let outline = &outlines(&[(0, 1, 2.0)], |v| points[v])[0];
        let expected = 10.0 * 2.0 + PI;
        assert!((area(outline) - expected).abs() < 0.1, "{}", area(outline));
    }

    #[test]
    fn a_line_changes_width_smoothly_where_two_edges_meet() {
        // A thin edge running on into a thick one, a little turned.
        let points = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(20.0, 1.0),
        ];
        let outlines = outlines(&[(0, 1, 1.0), (1, 2, 3.0)], |v| points[v]);
        // Where they meet, both as wide (halfway between): no step.
        let across = |outline: &[Point], x: f32| {
            let ys: Vec<f32> = (-30..=30)
                .map(|k| k as f32 * 0.05)
                .filter(|&y| inside(outline, Point::new(x, y)))
                .collect();
            ys.last().unwrap() - ys.first().unwrap()
        };
        let (thin, thick) = (across(&outlines[0], 9.8), across(&outlines[1], 10.2));
        assert!(
            (thin - 2.0).abs() < 0.2 && (thick - 2.0).abs() < 0.2,
            "{thin} {thick}"
        );
        // Each its own width at its far end.
        assert!((across(&outlines[0], 0.2) - 1.0).abs() < 0.2);
        // Still not over each other.
        for x in 80..=120 {
            for y in -30..=30 {
                let p = Point::new(x as f32 * 0.1 + 0.013, y as f32 * 0.1 + 0.017);
                assert!(
                    outlines.iter().filter(|o| inside(o, p)).count() <= 1,
                    "{p:?}"
                );
            }
        }
    }

    #[test]
    fn meeting_edges_share_the_joint_instead_of_overlapping() {
        // A thick edge and a thin one at a right angle, and a third, thin,
        // straight on from the thick one.
        let points = [
            Point::new(0.0, 0.0),
            Point::new(20.0, 0.0),
            Point::new(0.0, 20.0),
            Point::new(-20.0, 0.0),
        ];
        let edges = [(0, 1, 6.0), (0, 2, 1.0), (3, 0, 1.0)];
        let outlines = outlines(&edges, |v| points[v]);

        // Sample the joint: no point is in two outlines.
        for x in -40..=40 {
            for y in -40..=40 {
                let p = Point::new(x as f32 * 0.1 + 0.013, y as f32 * 0.1 + 0.017);
                let count = outlines.iter().filter(|o| inside(o, p)).count();
                assert!(count <= 1, "{p:?} in {count} outlines");
            }
        }
        // And the thick edge still covers its full width by the joint.
        assert!(inside(&outlines[0], Point::new(1.0, 2.9)));
        assert!(inside(&outlines[0], Point::new(1.0, -2.9)));
    }
}
