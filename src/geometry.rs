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

/// Closest point on the (infinite) line through `a` and `b` to `p`, and its
/// distance.
pub fn closest_on_line(p: Point, a: Point, b: Point) -> (Point, f32) {
    let ab = b - a;
    let len2 = ab.x * ab.x + ab.y * ab.y;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2
    };
    let closest = a + ab * t;
    (closest, closest.distance(p))
}

/// Whether `p` lies inside the polygon through `points` (closed back to
/// the first; crossing itself, by the even-odd rule).
pub fn inside_polygon(p: Point, points: &[Point]) -> bool {
    let mut inside = false;
    for (i, &a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// The convex hull of `points`, wound positively (as triangles are); empty
/// for fewer than three points not all on a line.
pub fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut points = points.to_vec();
    points.sort_by(|p, q| p.x.total_cmp(&q.x).then(p.y.total_cmp(&q.y)));
    points.dedup();
    if points.len() < 3 {
        return Vec::new();
    }
    // Andrew's monotone chain: the lower hull, then the upper.
    let mut hull: Vec<Point> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let ordered: Box<dyn Iterator<Item = &Point>> = if pass == 0 {
            Box::new(points.iter())
        } else {
            Box::new(points.iter().rev())
        };
        for &p in ordered {
            while hull.len() >= start + 2
                && area2(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    if hull.len() < 3 { Vec::new() } else { hull }
}

/// A map of the plane taking `p` to `m p + t` (`m` row by row): here
/// always an isometry, made of reflections.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    m: [f32; 4],
    t: iced::Vector,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        m: [1.0, 0.0, 0.0, 1.0],
        t: iced::Vector::ZERO,
    };

    /// The reflection across the line through `a` and `b`.
    pub fn reflection(a: Point, b: Point) -> Affine {
        let d = b - a;
        let len = d.x.hypot(d.y);
        if len == 0.0 {
            return Affine::IDENTITY;
        }
        let (x, y) = (d.x / len, d.y / len);
        let m = [x * x - y * y, 2.0 * x * y, 2.0 * x * y, y * y - x * x];
        // `a` stays put.
        let ma = Point::new(m[0] * a.x + m[1] * a.y, m[2] * a.x + m[3] * a.y);
        Affine { m, t: a - ma }
    }

    pub fn apply(self, p: Point) -> Point {
        let [a, b, c, d] = self.m;
        Point::new(a * p.x + b * p.y, c * p.x + d * p.y) + self.t
    }

    /// This, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        let [a, b, c, d] = self.m;
        let [e, f, g, h] = next.m;
        Affine {
            m: [e * a + f * c, e * b + f * d, g * a + h * c, g * b + h * d],
            t: next.apply(Point::ORIGIN + self.t) - Point::ORIGIN,
        }
    }

    pub fn inverse(self) -> Affine {
        let [a, b, c, d] = self.m;
        let det = a * d - b * c;
        let m = [d / det, -b / det, -c / det, a / det];
        let t = iced::Vector::new(
            -(m[0] * self.t.x + m[1] * self.t.y),
            -(m[2] * self.t.x + m[3] * self.t.y),
        );
        Affine { m, t }
    }

    /// Whether it turns the plane over (an odd number of reflections):
    /// what it maps winds the other way round.
    pub fn flips(self) -> bool {
        let [a, b, c, d] = self.m;
        a * d - b * c < 0.0
    }

    /// Whether it's (all but) the same map as `other`.
    pub fn near(self, other: Affine) -> bool {
        let close = |x: f32, y: f32| (x - y).abs() < 1e-4 * (1.0 + x.abs().max(y.abs()));
        self.m.iter().zip(&other.m).all(|(&x, &y)| close(x, y))
            && close(self.t.x, other.t.x)
            && close(self.t.y, other.t.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_convex_hull_leaves_out_points_inside() {
        let points = [
            (0.0, 0.0),
            (4.0, 0.0),
            (2.0, 1.0),
            (4.0, 4.0),
            (0.0, 4.0),
            (1.0, 2.0),
        ]
        .map(|(x, y)| Point::new(x, y));
        let hull = convex_hull(&points);
        assert_eq!(hull.len(), 4);
        assert!(!hull.contains(&Point::new(2.0, 1.0)));
        assert!(area2(hull[0], hull[1], hull[2]) > 0.0);
    }

    #[test]
    fn reflections_compose_and_invert() {
        let r = Affine::reflection(Point::new(1.0, 0.0), Point::new(1.0, 5.0));
        assert!(r.apply(Point::new(0.0, 2.0)).distance(Point::new(2.0, 2.0)) < 1e-5);
        assert!(r.flips());
        assert!(r.then(r).near(Affine::IDENTITY));
        let s = Affine::reflection(Point::new(0.0, 0.0), Point::new(1.0, 1.0));
        let rs = r.then(s);
        assert!(!rs.flips());
        let p = Point::new(3.0, -2.0);
        assert!(rs.inverse().apply(rs.apply(p)).distance(p) < 1e-4);
        assert!(rs.apply(p).distance(s.apply(r.apply(p))) < 1e-5);
    }
}
