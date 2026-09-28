//! Small 2D helpers shared by the document and the editor.

use iced::Point;

/// Twice the signed area of `a`, `b`, `c`. Its sign gives the winding.
pub fn area2(a: Point, b: Point, c: Point) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)
}

/// Whether two positively wound triangles overlap by more than a hair.
/// Sharing corners or edges doesn't count.
pub fn overlap(t: [Point; 3], u: [Point; 3]) -> bool {
    const TOLERANCE: f32 = 1e-3;

    // Separating axis test: disjoint iff some edge has the whole other
    // triangle on (or within a hair of) its outer side.
    let separates = |[a, b, c]: [Point; 3], other: [Point; 3]| {
        [(a, b), (b, c), (c, a)].into_iter().any(|(p, q)| {
            let len = p.distance(q);
            other.into_iter().all(|v| area2(p, q, v) <= TOLERANCE * len)
        })
    };

    !separates(t, u) && !separates(u, t)
}

/// The smallest height of a triangle (twice its area over its longest side).
pub fn min_height([a, b, c]: [Point; 3]) -> f32 {
    let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
    if longest == 0.0 {
        0.0
    } else {
        area2(a, b, c).abs() / longest
    }
}

/// Closest point on segment `a`–`b` to `p`, and its distance.
pub fn closest_on_segment(p: Point, a: Point, b: Point) -> (Point, f32) {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0)
    };
    let closest = a + ab * t;
    (closest, closest.distance(p))
}
