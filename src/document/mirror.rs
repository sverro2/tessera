//! Mirrors: a line a drawing is shown (and exported) mirrored across, and
//! the drawing reflected or otherwise mapped.

use super::*;

/// A mirror: the line through `a` and `b`, reflecting what's on one side
/// onto the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mirror {
    pub a: Point,
    pub b: Point,
}

impl Mirror {
    /// `p` reflected across the line.
    pub fn reflect(self, p: Point) -> Point {
        self.affine().apply(p)
    }

    /// The reflection across the line.
    pub fn affine(self) -> Affine {
        Affine::reflection(self.a, self.b)
    }
}

/// Where a drawing shows with `mirrors` one after the other, each
/// mirroring all there is so far: the maps from the drawing to each of its
/// images, itself (the identity) first.
pub fn images(mirrors: &[Mirror]) -> Vec<Affine> {
    let mut images = vec![Affine::IDENTITY];
    for mirror in mirrors {
        let reflect = mirror.affine();
        let mirrored: Vec<_> = images.iter().map(|&image| image.then(reflect)).collect();
        images.extend(mirrored);
    }
    images
}

/// For drawings shown at `images`: the maps taking the drawing onto
/// where another of its images shows, as seen from one (`g` then back from
/// `h`). The images overlap each other iff the drawing overlaps itself
/// under one of these.
pub fn crossings(images: &[Affine]) -> Vec<Affine> {
    let mut maps: Vec<Affine> = Vec::new();
    for (i, &g) in images.iter().enumerate() {
        for (j, &h) in images.iter().enumerate() {
            let map = g.then(h.inverse());
            if i != j && !map.near(Affine::IDENTITY) && !maps.iter().any(|m| m.near(map)) {
                maps.push(map);
            }
        }
    }
    maps
}

impl Document {
    /// Just the drawing's mirror image across `mirror`, painted alike.
    pub fn reflection(&self, mirror: Mirror) -> Document {
        self.transformed(mirror.affine())
    }

    /// The drawing mapped by `map`, painted alike.
    pub fn transformed(&self, map: Affine) -> Document {
        let flips = map.flips();
        Document {
            vertices: self.vertices.iter().map(|&p| map.apply(p)).collect(),
            // Turned over, they wind the other way round: turned back.
            triangles: self
                .triangles
                .iter()
                .map(|&[a, b, c]| if flips { [a, c, b] } else { [a, b, c] })
                .collect(),
            colors: self.colors.clone(),
            edge_styles: self.edge_styles.clone(),
            revision: next_revision(),
        }
    }

    /// The drawing with `mirrors` applied, one after the other: see
    /// [`Self::with_mirror`].
    pub fn with_mirrors(&self, mirrors: &[Mirror]) -> Document {
        let mut drawing = self.clone();
        for &mirror in mirrors {
            drawing = drawing.with_mirror(mirror);
        }
        drawing
    }

    /// The drawing and, beside it, its mirror image across `mirror`: not
    /// joined up (see [`Self::with_mirror`]).
    pub(super) fn with_reflection(&self, mirror: Mirror) -> Document {
        let n = self.vertices.len();
        let image = self.reflection(mirror);
        let mut both = self.clone();
        both.vertices.extend(image.vertices);
        both.triangles
            .extend(image.triangles.into_iter().map(|t| t.map(|v| v + n)));
        both.colors.extend(
            image
                .colors
                .into_iter()
                .map(|(t, c)| (key(t.map(|v| v + n)), c)),
        );
        both.edge_styles.extend(
            image
                .edge_styles
                .into_iter()
                .map(|((a, b), style)| ((a + n, b + n), style)),
        );
        both.revision = next_revision();
        both
    }

    /// The drawing with its mirror image joined on, as applying the mirror
    /// would make it; for showing and exporting it. Unlike applying, it
    /// doesn't check the two don't overlap (they're kept from that while
    /// editing): it's quick enough to do on every change.
    pub fn with_mirror(&self, mirror: Mirror) -> Document {
        let mut changes = Changes::default();
        let mut joined = self.with_reflection(mirror);
        if !joined.normalize(&[], &mut changes, self) {
            return self.with_reflection(mirror);
        }
        joined.inherit_colors(&self.with_reflection(mirror));
        joined.inherit_edge_styles(&self.with_reflection(mirror));
        joined
    }

    /// Whether the drawing overlaps itself mapped by any of `maps`: those of
    /// its triangles new or moved since `before` (all of them, without it),
    /// mapped, against all of it. (With each map's inverse among them too,
    /// that's all that can have come to overlap.)
    pub fn overlaps_under(&self, before: Option<&Document>, maps: &[Affine]) -> bool {
        let changed = before.map(|before| Changed::since(self, before));
        let bounds = |t: [Point; 3]| {
            let (mut min, mut max) = (t[0], t[0]);
            for p in &t[1..] {
                min = Point::new(min.x.min(p.x), min.y.min(p.y));
                max = Point::new(max.x.max(p.x), max.y.max(p.y));
            }
            (min, max)
        };
        let all: Vec<_> = self.triangles().map(|t| (t, bounds(t))).collect();
        self.triangles
            .iter()
            .filter(|&&t| changed.as_ref().is_none_or(|changed| changed.triangle(t)))
            .any(|&[a, b, c]| {
                maps.iter().any(|&map| {
                    let corners = if map.flips() { [c, b, a] } else { [a, b, c] };
                    let image = corners.map(|v| map.apply(self.vertices[v]));
                    let (min, max) = bounds(image);
                    all.iter().any(|&(u, (umin, umax))| {
                        min.x <= umax.x
                            && umin.x <= max.x
                            && min.y <= umax.y
                            && umin.y <= max.y
                            && overlap(image, u)
                    })
                })
            })
    }
}
