//! Lining a dragged point up with what's around it (holding Shift):
//! guides from the edges around it, the view, and other shapes.

use super::*;

/// Guides a point dragged with Shift lines up with.
pub(super) const GUIDE: Color = Color::from_rgb8(0x73, 0xda, 0xca);

/// Screen-space distance within which a dragged point lines up with a
/// guide; and with a point on guides (where two cross, say).
pub(super) const GUIDE_HIT: f32 = 8.0;

pub(super) const GUIDE_POINT_HIT: f32 = 10.0;

/// Screen-space distance within which guides are shown, as coming up; and
/// how many at most.
pub(super) const GUIDE_NEAR: f32 = 20.0;

pub(super) const GUIDES_SHOWN: usize = 3;

/// How many of the shapes' outline edges, and of the vertices, nearest
/// where a point's placed offer guides (angles, lengths, lining up): with
/// all of them, crowded drawings would offer too many.
pub(super) const NEAREST: usize = 10;

/// A line (or circle) a point dragged with Shift can line up with, from
/// the edges around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Guide {
    /// Straight on from edge `m`–`n`, past `n`: no bend there.
    Straight { m: VertexId, n: VertexId },
    /// From `n`, square to edge `m`–`n`: a right angle there.
    Square { m: VertexId, n: VertexId },
    /// From `n`, at the same angle as edge `u`–`v`.
    Parallel {
        n: VertexId,
        u: VertexId,
        v: VertexId,
    },
    /// Between `a` and `b`: the outline straight there.
    Between { a: VertexId, b: VertexId },
    /// Where edges to `a` and `b` meet at a right angle: the circle over
    /// `a`–`b` (Thales').
    Corner { a: VertexId, b: VertexId },
    /// From `n`, level (or `upright`) as seen in the view.
    Level { n: VertexId, upright: bool },
    /// Just as far from `a` as from `b`: edges to both as long.
    Middle { a: VertexId, b: VertexId },
    /// From a point that's no vertex yet (where a new triangle starts),
    /// level (or `upright`) as seen in the view.
    Axis { at: Point, upright: bool },
    /// In line with edge `u`–`v` (of another shape): on it, or on past it.
    Along { u: VertexId, v: VertexId },
    /// From a point that's no vertex yet (where a new triangle starts), at
    /// the same angle as edge `u`–`v`.
    ParallelFrom { at: Point, u: VertexId, v: VertexId },
    /// From a point that's no vertex yet (where an extrusion started),
    /// square to edge `a`–`b`: extruding it straight out.
    Normal { at: Point, a: VertexId, b: VertexId },
    /// The far side of a new triangle started at `at` (from its dragged
    /// corner to its third) running along `side`: level or upright in the
    /// view, or parallel to `edge`. For that, the dragged corner is on the
    /// line from `at` along `line` (where it runs, as the others).
    Across {
        at: Point,
        line: Vector,
        side: Vector,
        edge: Option<(VertexId, VertexId)>,
    },
}

impl Guide {
    /// Where it runs, seen through `camera`. Lines go from the end the new
    /// edge starts at, as far as the edge they follow is long.
    pub(super) fn shape(self, document: &Document, camera: Camera) -> Shape {
        let at = |v| document.vertex(v);
        match self {
            Guide::Level { n, upright } => level(at(n), upright, camera),
            Guide::Along { u, v } => Shape::Line(at(u), at(v) - at(u)),
            Guide::ParallelFrom { at: from, u, v } => Shape::Line(from, at(v) - at(u)),
            Guide::Across { at, line, .. } => Shape::Line(at, line),
            Guide::Normal { at: from, a, b } => {
                let d = at(b) - at(a);
                Shape::Line(from, Vector::new(-d.y, d.x))
            }
            Guide::Axis { at, upright } => level(at, upright, camera),
            Guide::Straight { m, n } => Shape::Line(at(n), at(n) - at(m)),
            Guide::Square { m, n } => {
                let d = at(n) - at(m);
                Shape::Line(at(n), Vector::new(-d.y, d.x))
            }
            Guide::Parallel { n, u, v } => Shape::Line(at(n), at(v) - at(u)),
            Guide::Between { a, b } => Shape::Line(at(a), at(b) - at(a)),
            Guide::Corner { a, b } => {
                let (a, b) = (at(a), at(b));
                Shape::Circle(a + (b - a) * 0.5, a.distance(b) / 2.0)
            }
            Guide::Middle { a, b } => {
                let (a, b) = (at(a), at(b));
                let d = b - a;
                Shape::Line(a + d * 0.5, Vector::new(-d.y, d.x))
            }
        }
    }

    /// The edge it follows, if one.
    pub(super) fn followed(self) -> Option<(VertexId, VertexId)> {
        match self {
            Guide::Straight { m, n } | Guide::Square { m, n } => Some((m, n)),
            Guide::Parallel { u, v, .. }
            | Guide::Along { u, v }
            | Guide::ParallelFrom { u, v, .. } => Some((u, v)),
            Guide::Across { edge, .. } => edge,
            Guide::Normal { a, b, .. } => Some((a, b)),
            Guide::Between { .. }
            | Guide::Corner { .. }
            | Guide::Level { .. }
            | Guide::Middle { .. }
            | Guide::Axis { .. } => None,
        }
    }

    /// The end the new edge starts at, for those from one.
    pub(super) fn from(self) -> Option<VertexId> {
        match self {
            Guide::Straight { n, .. }
            | Guide::Square { n, .. }
            | Guide::Parallel { n, .. }
            | Guide::Level { n, .. } => Some(n),
            Guide::Between { .. }
            | Guide::Corner { .. }
            | Guide::Middle { .. }
            | Guide::Axis { .. }
            | Guide::Along { .. }
            | Guide::ParallelFrom { .. }
            | Guide::Across { .. }
            | Guide::Normal { .. } => None,
        }
    }

    /// Where the new edge it lines up starts (world), for those from one.
    pub(super) fn start(self, document: &Document) -> Option<Point> {
        match self {
            Guide::Axis { at, .. } | Guide::ParallelFrom { at, .. } | Guide::Normal { at, .. } => {
                Some(at)
            }
            _ => self.from().map(|n| document.vertex(n)),
        }
    }
}

/// Where a guide runs (world).
#[derive(Debug, Clone, Copy)]
pub(super) enum Shape {
    /// Through a point, along a direction.
    Line(Point, Vector),
    /// Around a centre, this far.
    Circle(Point, f32),
}

/// The line through `at` (world) level (or `upright`) as seen through
/// `camera`.
pub(super) fn level(at: Point, upright: bool, camera: Camera) -> Shape {
    let along = if upright {
        Vector::new(0.0, 1.0)
    } else {
        Vector::new(1.0, 0.0)
    };
    let origin = camera.to_screen(at);
    let d = camera.to_world(origin + along) - at;
    let length = d.x.hypot(d.y).max(1e-9);
    Shape::Line(at, d * (1.0 / length))
}

/// Where a dragged point lined up, and what releasing there does.
pub(super) type LinedUp = (Point, Pending);

/// The guides for a point dragged with Shift: those that would do, all
/// those near, and where it lined up (see [`Editor::guided`]).
pub(super) type Guided = (Vec<Guide>, Vec<Guide>, Option<LinedUp>);

/// What's being dragged, to line up with what's around it.
#[derive(Debug, Clone, Copy)]
pub(super) enum Dragged {
    Vertex(VertexId),
    /// The new corner of a triangle on edge `a`–`b`, grabbed at `start`.
    Apex {
        a: VertexId,
        b: VertexId,
        start: Point,
    },
}

/// Where two guides cross (world): lines once (unless parallel), a line
/// and a circle up to twice; circles aren't crossed with each other.
pub(super) fn crossings(g: Shape, h: Shape) -> Vec<Point> {
    match (g, h) {
        (Shape::Line(o, d), Shape::Line(p, e)) => {
            let den = cross(d, e);
            if den.abs() <= 1e-4 * d.x.hypot(d.y) * e.x.hypot(e.y) {
                return Vec::new();
            }
            vec![o + d * (cross(p - o, e) / den)]
        }
        (Shape::Line(o, d), Shape::Circle(c, r)) | (Shape::Circle(c, r), Shape::Line(o, d)) => {
            // o + t d at distance r from c.
            let f = o - c;
            let a = d.x * d.x + d.y * d.y;
            let b = 2.0 * (f.x * d.x + f.y * d.y);
            let k = f.x * f.x + f.y * f.y - r * r;
            let disc = b * b - 4.0 * a * k;
            if a == 0.0 || disc < 0.0 {
                return Vec::new();
            }
            let root = disc.sqrt();
            [(-b - root) / (2.0 * a), (-b + root) / (2.0 * a)]
                .into_iter()
                .map(|t| o + d * t)
                .collect()
        }
        (Shape::Circle(..), Shape::Circle(..)) => Vec::new(),
    }
}

impl Editor<'_> {
    /// Lining up `dragged` (the cursor at `world`) with the guides around
    /// it: those coming up that would do, nearest first; all those near;
    /// and where it lines up (and what releasing there does), if anywhere
    /// that works out. `None` if a vertex is in reach, to
    /// snap onto instead.
    pub(super) fn guided(&self, dragged: Dragged, world: Point) -> Option<Guided> {
        let camera = self.camera;
        let cursor = camera.to_screen(world);
        let moving = match dragged {
            Dragged::Vertex(id) => Some(id),
            Dragged::Apex { .. } => None,
        };
        let vertex_near = self
            .snaps(cursor, |v| Some(v) == moving, |_, _| true, &[])
            .iter()
            .any(|(target, _)| matches!(target, Target::Vertex(_)));
        if vertex_near {
            return None;
        }
        let ends: Vec<VertexId> = match dragged {
            Dragged::Vertex(id) => {
                let mut ends: Vec<_> = self
                    .document
                    .unique_edges()
                    .into_iter()
                    .filter_map(|(u, v)| match (u == id, v == id) {
                        (true, _) => Some(v),
                        (_, true) => Some(u),
                        _ => None,
                    })
                    .collect();
                ends.sort_unstable();
                ends.dedup();
                ends
            }
            Dragged::Apex { a, b, .. } => vec![a, b],
        };
        let guides = self.guides(moving, &ends, world);
        let (near, all_near, candidates) = self.line_up(&guides, world);

        // Moving it to `at`: what that does, if it works out, lands there
        // (not snapped on elsewhere) and keeps the shape (nothing flattened
        // or dropped).
        let attempt = |at: Point| {
            let pending = match dragged {
                Dragged::Vertex(id) => match self.proportional {
                    Some(_) => self.move_proportionally(id, at).map(|(_, p)| p),
                    None => self.move_to(id, at),
                },
                Dragged::Apex { a, b, start } => self.extend(a, b, start, at),
            }?;
            let landed = camera.to_screen(pending.at).distance(camera.to_screen(at)) < 0.5;
            (landed && self.keeps_shape(&pending)).then_some(pending)
        };
        let found = candidates
            .into_iter()
            .take(2 * MAX_SNAPS)
            .find_map(|at| Some((at, attempt(at)?)));

        // Those that would do (where they come closest), nearest first.
        // Only as many as are shown: trying each takes a while.
        let fine: Vec<Guide> = near
            .iter()
            .filter(|&&(_, closest)| attempt(closest).is_some())
            .map(|&(guide, _)| guide)
            .take(GUIDES_SHOWN)
            .collect();
        Some((fine, all_near, found))
    }

    /// Lining up a new triangle started at `from` (the cursor at `world`):
    /// its dragged corner, and its third, level or upright with its start
    /// or with the vertices there are, or in line with the shapes'
    /// outlines; the sides from its start level, upright or at the angle of
    /// the outlines; and its far side likewise. As [`Self::guided`].
    pub(super) fn guided_creation(&self, from: Point, world: Point) -> Guided {
        let camera = self.camera;
        let outlines = self.nearest_outlines(self.outlines(|_| false), &[from, world]);
        let vertices = self.nearest_vertices(&[from, world]);
        // For either corner.
        let mut corner = Vec::new();
        for upright in [false, true] {
            corner.push(Guide::Axis { at: from, upright });
            for &n in &vertices {
                corner.push(Guide::Level { n, upright });
            }
        }
        for &(u, v) in &outlines {
            corner.push(Guide::ParallelFrom { at: from, u, v });
            corner.push(Guide::Along { u, v });
        }
        // The far side runs twice sixty degrees off the side dragged out.
        let across = |side: Vector, edge| Guide::Across {
            at: from,
            line: turn(side, 2.0 * self.sixty()),
            side,
            edge,
        };
        let mut far_side = Vec::new();
        for upright in [false, true] {
            if let Shape::Line(_, side) = level(from, upright, camera) {
                far_side.push(across(side, None));
            }
        }
        for &(u, v) in &outlines {
            let side = self.document.vertex(v) - self.document.vertex(u);
            far_side.push(across(side, Some((u, v))));
        }

        // The dragged corner first, as it's where the cursor is; then the
        // far side, then the third corner.
        let third = self.third_corner(from, world);
        let (mut near, mut all_near, lined) = self.line_up(&corner, world);
        let (near_far, all_far, lined_far) = self.line_up(&far_side, world);
        let (near_third, all_third, lined_third) = self.line_up(&corner, third);
        near.extend(near_far);
        all_near.extend(all_far);
        all_near.extend(all_third);

        let attempt = |at: Point| {
            let pending = self.pending(Interaction::Creating { from, to: at })?;
            self.keeps_shape(&pending).then_some(pending)
        };
        // Where the dragged corner goes for each to line up, best first.
        let candidates: Vec<Point> = lined
            .into_iter()
            .take(2 * MAX_SNAPS)
            .chain(lined_far.into_iter().take(2 * MAX_SNAPS))
            .chain(
                lined_third
                    .into_iter()
                    .take(2 * MAX_SNAPS)
                    .map(|p| self.dragged_corner(from, p)),
            )
            .collect();
        let found = candidates
            .into_iter()
            .find_map(|at| Some((at, attempt(at)?)));
        let fine = near
            .iter()
            .filter(|&&(_, closest)| attempt(closest).is_some())
            .chain(
                near_third
                    .iter()
                    .filter(|&&(_, closest)| attempt(self.dragged_corner(from, closest)).is_some()),
            )
            .map(|&(guide, _)| guide)
            // Only as many as are shown: trying each takes a while.
            .take(GUIDES_SHOWN)
            .collect();
        (fine, all_near, found)
    }

    /// Lining up an extrusion of `selection` started at `from` (the cursor
    /// at `world`): the way it goes square to an edge extruded (as far as
    /// that's long, say), level or upright in the view, or at the angle of
    /// the outlines (straight on from those at its ends, say); else one of
    /// its far ends level or upright with the vertices there are, or in
    /// line with the outlines. As [`Self::guided`], the cursor lining up.
    pub(super) fn guided_extrusion(
        &self,
        selection: &[VertexId],
        from: Point,
        world: Point,
    ) -> Guided {
        let camera = self.camera;
        let outlines = self.nearest_outlines(self.outlines(|_| false), &[from, world]);
        let vertices = self.nearest_vertices(&[from, world]);
        let mut way = Vec::new();
        for (a, b, _) in self.outer_edges(selection) {
            way.push(Guide::Normal { at: from, a, b });
        }
        for upright in [false, true] {
            way.push(Guide::Axis { at: from, upright });
        }
        for &(u, v) in &outlines {
            way.push(Guide::ParallelFrom { at: from, u, v });
        }
        let mut lands = Vec::new();
        for upright in [false, true] {
            for &n in &vertices {
                lands.push(Guide::Level { n, upright });
            }
        }
        for &(u, v) in &outlines {
            lands.push(Guide::Along { u, v });
        }

        // The way first; then the far ends nearest the cursor, a few.
        let by = world - from;
        let (near, mut all_near, lined) = self.line_up(&way, world);
        let mut candidates: Vec<Vector> = lined
            .into_iter()
            .take(2 * MAX_SNAPS)
            .map(|p| p - from)
            .collect();
        let mut near: Vec<(Guide, Vector)> =
            near.into_iter().map(|(g, at)| (g, at - from)).collect();
        let cursor = camera.to_screen(world);
        let mut ends: Vec<Point> = self
            .extruded_ends(selection)
            .into_iter()
            .map(|v| self.document.vertex(v))
            .collect();
        ends.sort_by(|p, q| {
            let off = |e: &Point| camera.to_screen(*e + by).distance(cursor);
            off(p).total_cmp(&off(q))
        });
        for end in ends.into_iter().take(3) {
            let (near_end, all_end, lined_end) = self.line_up(&lands, end + by);
            all_near.extend(all_end);
            candidates.extend(lined_end.into_iter().take(MAX_SNAPS).map(|p| p - end));
            near.extend(near_end.into_iter().map(|(g, at)| (g, at - end)));
        }

        let attempt = |by: Vector| self.extrude(selection, by, from + by);
        let found = candidates
            .into_iter()
            .find_map(|by| Some((from + by, attempt(by)?)));
        // Only as many as are shown: trying each takes a while.
        let fine = near
            .into_iter()
            .filter(|&(_, by)| attempt(by).is_some())
            .map(|(guide, _)| guide)
            .take(GUIDES_SHOWN)
            .collect();
        (fine, all_near, found)
    }

    /// Where a new triangle pressed at `pressed` starts: there; with `grid`
    /// on the nearest grid point; with `shift` lined up level or upright with
    /// the vertices there are, or in line with the outlines (with both, a
    /// grid point on such a guide first). And the guides that would do, and
    /// all those near.
    pub(super) fn creation_start(
        &self,
        pressed: Point,
        grid: bool,
        shift: bool,
    ) -> (Point, Vec<Guide>, Vec<Guide>) {
        if !grid && !shift {
            return (pressed, Vec::new(), Vec::new());
        }
        let mut guides = Vec::new();
        if shift {
            for upright in [false, true] {
                for n in self.nearest_vertices(&[pressed]) {
                    guides.push(Guide::Level { n, upright });
                }
            }
            for (u, v) in self.nearest_outlines(self.outlines(|_| false), &[pressed]) {
                guides.push(Guide::Along { u, v });
            }
        }
        let (near, all_near, lined) = self.line_up(&guides, pressed);
        let fine = near.into_iter().map(|(guide, _)| guide).collect();
        let start = if grid {
            self.grid_points(pressed, &|p| self.on_guide(&all_near, &[p]))
                .first()
                .copied()
        } else {
            lined.first().copied()
        };
        (start.unwrap_or(pressed), fine, all_near)
    }

    /// The guides a point dragged (`moving`, if it's a vertex already),
    /// with edges to `ends`, can line up with: straight on from the edges
    /// at its ends, or square to them; at the angle of the edges at its
    /// other ends; level or upright in the view from its ends; between two
    /// ends, as far from both, or square at the point itself; at the angle
    /// of, or in line with, the outlines of other shapes. Each once.
    pub(super) fn guides(
        &self,
        moving: Option<VertexId>,
        ends: &[VertexId],
        near: Point,
    ) -> Vec<Guide> {
        let document = self.document;
        let edges = document.unique_edges();
        let mut around: HashMap<VertexId, Vec<VertexId>> = HashMap::new();
        for &(u, v) in &edges {
            around.entry(u).or_default().push(v);
            around.entry(v).or_default().push(u);
        }
        let skip = |v| Some(v) == moving;

        let mut guides = Vec::new();
        for &n in ends {
            for &m in around.get(&n).into_iter().flatten() {
                if !skip(m) {
                    guides.push(Guide::Straight { m, n });
                    guides.push(Guide::Square { m, n });
                }
            }
            // Parallel to the edges at the other ends (completing a
            // parallelogram, say); not those further off: too many.
            for &(u, v) in &edges {
                if skip(u) || skip(v) || u == n || v == n {
                    continue;
                }
                if ends.contains(&u) || ends.contains(&v) {
                    guides.push(Guide::Parallel { n, u, v });
                }
            }
        }
        // Last: where one's the same line as one of the above, that one says
        // more.
        for &n in ends {
            for upright in [false, true] {
                guides.push(Guide::Level { n, upright });
            }
        }
        for (i, &a) in ends.iter().enumerate() {
            for &b in &ends[i + 1..] {
                if edges.binary_search(&(a.min(b), a.max(b))).is_err() {
                    guides.push(Guide::Between { a, b });
                }
                guides.push(Guide::Corner { a, b });
                guides.push(Guide::Middle { a, b });
            }
        }

        // From the other shapes there are (their outlines, those nearest):
        // at the same angle (keeping them parallel), and in line with them.
        let seed = moving.or(ends.first().copied());
        let own: std::collections::HashSet<VertexId> = seed
            .map(|v| document.connected(v).into_iter().flatten().collect())
            .unwrap_or_default();
        let others = self.outlines(|v| own.contains(&v));
        for (u, v) in self.nearest_outlines(others, &[near]) {
            for &n in ends {
                guides.push(Guide::Parallel { n, u, v });
            }
            guides.push(Guide::Along { u, v });
        }

        // Each kept, even where they run alike: each edge followed has a
        // length of its own to line up with (see `line_up`).
        guides
    }

    /// Of `outlines`, the [`NEAREST`] nearest any of `near` (world): with
    /// guides from all of them, crowded drawings would offer too many.
    pub(super) fn nearest_outlines(
        &self,
        mut outlines: Vec<(VertexId, VertexId)>,
        near: &[Point],
    ) -> Vec<(VertexId, VertexId)> {
        let at = |v| self.document.vertex(v);
        let off = |&(u, v): &(VertexId, VertexId)| {
            near.iter()
                .map(|&p| closest_on_segment(p, at(u), at(v)).1)
                .fold(f32::INFINITY, f32::min)
        };
        if outlines.len() > NEAREST {
            outlines.select_nth_unstable_by(NEAREST, |x, y| off(x).total_cmp(&off(y)));
            outlines.truncate(NEAREST);
        }
        outlines
    }

    /// The [`NEAREST`] vertices nearest any of `near` (world).
    pub(super) fn nearest_vertices(&self, near: &[Point]) -> Vec<VertexId> {
        let off = |&v: &VertexId| {
            let p = self.document.vertex(v);
            near.iter()
                .map(|&q| q.distance(p))
                .fold(f32::INFINITY, f32::min)
        };
        let mut vertices = self.document.unique_vertices();
        if vertices.len() > NEAREST {
            vertices.select_nth_unstable_by(NEAREST, |x, y| off(x).total_cmp(&off(y)));
            vertices.truncate(NEAREST);
        }
        vertices
    }

    /// The edges on the outlines of the shapes there are, but for those
    /// with a vertex that's `left_out`.
    pub(super) fn outlines(
        &self,
        left_out: impl Fn(VertexId) -> bool,
    ) -> Vec<(VertexId, VertexId)> {
        let mut count: HashMap<(VertexId, VertexId), usize> = HashMap::new();
        for (u, v) in self.document.edges() {
            *count.entry((u.min(v), u.max(v))).or_default() += 1;
        }
        let mut outlines: Vec<_> = count
            .into_iter()
            .filter(|&((u, v), n)| n == 1 && !left_out(u) && !left_out(v))
            .map(|(edge, _)| edge)
            .collect();
        outlines.sort_unstable();
        outlines
    }

    /// `guides`, each line (or circle) once: the first of those that run
    /// alike. (Told apart by where they run, rounded; quick, even with
    /// many.)
    pub(super) fn unique(&self, guides: Vec<Guide>) -> Vec<Guide> {
        let (document, camera) = (self.document, self.camera);
        let mut seen = std::collections::HashSet::new();
        guides
            .into_iter()
            .filter(|guide| {
                let key = match guide.shape(document, camera) {
                    Shape::Line(o, d) => {
                        let length = d.x.hypot(d.y);
                        if length == 0.0 {
                            return false;
                        }
                        // Its direction either way, and how far it runs from
                        // the origin.
                        let (mut x, mut y) = (d.x / length, d.y / length);
                        if x < 0.0 || (x == 0.0 && y < 0.0) {
                            (x, y) = (-x, -y);
                        }
                        let offset = o.y * x - o.x * y;
                        (
                            0,
                            (y.atan2(x) * 1e4).round() as i64,
                            (offset * 1e2).round() as i64,
                            0,
                        )
                    }
                    Shape::Circle(c, r) => {
                        if r == 0.0 {
                            return false;
                        }
                        let round = |n: f32| (n * 1e2).round() as i64;
                        (1, round(c.x), round(c.y), round(r))
                    }
                };
                seen.insert(key)
            })
            .collect()
    }

    /// Of `guides`, those coming up near the cursor (at `world`), nearest
    /// first: each line once, with where on it it comes closest; and all
    /// of them (several may run alike); and where it lines up with them,
    /// best first: points (where two cross, or as far as an edge is long,
    /// for every edge followed) before lines, nearest first.
    pub(super) fn line_up(
        &self,
        guides: &[Guide],
        world: Point,
    ) -> (Vec<(Guide, Point)>, Vec<Guide>, Vec<Point>) {
        let camera = self.camera;
        let document = self.document;
        let cursor = camera.to_screen(world);
        let off = |p: Point| camera.to_screen(p).distance(cursor);
        let mut near = Vec::new();
        let mut points = Vec::new();
        let mut lines = Vec::new();
        for &guide in guides {
            let shape = guide.shape(document, camera);
            // The closest point on it.
            let closest = match shape {
                Shape::Line(o, d) => {
                    let (q, _) =
                        closest_on_line(cursor, camera.to_screen(o), camera.to_screen(o + d));
                    camera.to_world(q)
                }
                Shape::Circle(c, r) => {
                    let away = world - c;
                    let length = away.x.hypot(away.y);
                    if length == 0.0 {
                        continue;
                    }
                    c + away * (r / length)
                }
            };
            let distance = off(closest);
            if distance > GUIDE_NEAR {
                continue;
            }
            near.push((distance, guide, closest));
            if distance <= GUIDE_HIT {
                lines.push((distance, closest));
            }
            // (Where several run alike, each edge they follow still has a
            // length of its own.)
            // As long as the edge it follows: straight on, past its end;
            // square or parallel, either way.
            let ways: &[f32] = match guide {
                Guide::Straight { .. } => &[1.0],
                Guide::Square { .. }
                | Guide::Parallel { .. }
                | Guide::ParallelFrom { .. }
                | Guide::Normal { .. } => &[1.0, -1.0],
                Guide::Between { .. }
                | Guide::Corner { .. }
                | Guide::Level { .. }
                | Guide::Middle { .. }
                | Guide::Axis { .. }
                | Guide::Along { .. }
                | Guide::Across { .. } => &[],
            };
            if let Shape::Line(o, d) = shape {
                for &way in ways {
                    let p = o + d * way;
                    if off(p) <= GUIDE_POINT_HIT {
                        points.push((off(p), p));
                    }
                }
            }
        }
        near.sort_by(|x, y| x.0.total_cmp(&y.0));
        let all_near: Vec<Guide> = near.iter().map(|&(_, guide, _)| guide).collect();
        // Each line (or circle) once, from here: to show, and to cross.
        let unique = self.unique(all_near.clone());
        let near: Vec<(Guide, Point)> = near
            .into_iter()
            .filter(|(_, guide, _)| unique.contains(guide))
            .map(|(_, g, at)| (g, at))
            .collect();
        for (i, &(g, _)) in near.iter().enumerate() {
            for &(h, _) in &near[i + 1..] {
                for at in crossings(g.shape(document, camera), h.shape(document, camera)) {
                    if off(at) <= GUIDE_POINT_HIT {
                        points.push((off(at), at));
                    }
                }
            }
        }
        points.sort_by(|x, y| x.0.total_cmp(&y.0));
        lines.sort_by(|x, y| x.0.total_cmp(&y.0));
        let lined = points.into_iter().chain(lines).map(|(_, l)| l).collect();
        (near, all_near, lined)
    }

    /// Whether `pending` keeps the shape: flattens or drops no triangle.
    pub(super) fn keeps_shape(&self, pending: &Pending) -> bool {
        pending.changes.squashed.is_empty()
            && pending.result.triangle_ids().len() >= self.document.triangle_ids().len()
    }

    /// The guides near a point dragged with Shift that it lines up with
    /// where `pending` puts it: those running through it, if that keeps the
    /// shape (else it's lined up with nothing worth showing).
    pub(super) fn lit(&self, state: &State, pending: &Pending) -> Vec<Guide> {
        if !self.keeps_shape(pending) {
            return Vec::new();
        }
        let also = self.also_lined(state);
        state
            .near_guides
            .iter()
            .copied()
            .filter(|&guide| {
                self.runs_through(guide, pending.at)
                    || also.iter().any(|&p| self.lines_up(guide, p))
            })
            .collect()
    }

    /// Whether `guide` lines `p` up: runs through it, not just from it (as
    /// those from a new triangle's start do).
    fn lines_up(&self, guide: Guide, p: Point) -> bool {
        let from_it = matches!(
            guide.shape(self.document, self.camera),
            Shape::Line(o, _) if o.distance(p) < 1e-3
        );
        !from_it && self.runs_through(guide, p)
    }

    /// The points (world) that line up besides the one dragged: a new
    /// triangle's third corner; an extrusion's far ends.
    fn also_lined(&self, state: &State) -> Vec<Point> {
        match state.interaction {
            // Its start too, lined up from where it was pressed.
            Interaction::Creating { from, to, .. } => vec![self.third_corner(from, to), from],
            Interaction::Extruding { from, to } => self
                .extruded_ends(&state.selection)
                .into_iter()
                .map(|v| self.document.vertex(v) + (to - from))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// How `guide` shows, the point dragged at `at` (world): where it runs,
    /// and the new edge it lines up (lit), from where (if it says) to where.
    fn guide_view(&self, state: &State, guide: Guide, at: Point) -> (Shape, Option<Point>, Point) {
        let document = self.document;
        let shape = guide.shape(document, self.camera);
        let also = self.also_lined(state);
        let through = || {
            if self.runs_through(guide, at) {
                at
            } else {
                also.iter()
                    .copied()
                    .find(|&p| self.lines_up(guide, p))
                    .unwrap_or(at)
            }
        };
        match state.interaction {
            // The far side runs through the far side, from the corner
            // dragged to the third.
            Interaction::Creating { to, .. } => match guide {
                Guide::Across { side, .. } => (Shape::Line(to, side), Some(to), also[0]),
                _ => (shape, guide.start(document), through()),
            },
            // The way an extrusion goes, shown from an end it extrudes: an
            // edge it's square to, else the nearest.
            Interaction::Extruding { from, to } if guide.start(document) == Some(from) => {
                let by = to - from;
                let end = match guide {
                    Guide::Normal { a, .. } => document.vertex(a),
                    _ => also
                        .iter()
                        .map(|&p| p - by)
                        .min_by(|p, q| p.distance(at).total_cmp(&q.distance(at)))
                        .unwrap_or(from),
                };
                let shape = match shape {
                    Shape::Line(_, d) => Shape::Line(end, d),
                    circle => circle,
                };
                (shape, Some(end), end + by)
            }
            _ => (shape, guide.start(document), through()),
        }
    }

    /// Whether `guide` runs through `at` (world), on screen to the pixel.
    pub(super) fn runs_through(&self, guide: Guide, at: Point) -> bool {
        let camera = self.camera;
        let p = camera.to_screen(at);
        let off = match guide.shape(self.document, camera) {
            Shape::Line(o, d) => closest_on_line(p, camera.to_screen(o), camera.to_screen(o + d)).1,
            Shape::Circle(c, r) => (camera.to_screen(c).distance(p) - r * camera.zoom).abs(),
        };
        off < 0.75
    }

    /// The guides for a point dragged with Shift, now at `at` (as it will
    /// be released): those it lines up with there bold, with the edges they
    /// follow, marked (parallel edges with an arrowhead each, edges as long
    /// with two ticks each, a right angle with a square in its corner,
    /// where two or more meet with a diamond); and the nearest few others
    /// that would do, faint.
    pub(super) fn draw_guides(
        &self,
        frame: &mut Frame,
        state: &State,
        pending: Option<&Pending>,
        bounds: Rectangle,
    ) {
        let at = pending.map(|pending| pending.at);
        let camera = self.camera;
        let document = self.document;
        let screen = |v| camera.to_screen(document.vertex(v));
        // What it lines up with is told from where it lands, so no more and
        // no less lights up than is so.
        let lit = pending.map_or_else(Vec::new, |pending| self.lit(state, pending));
        let mut shown = lit.clone();
        let others = state.guides.iter().filter(|guide| !lit.contains(guide));
        shown.extend(others.take(GUIDES_SHOWN.saturating_sub(lit.len())));

        let reach = bounds.width + bounds.height;
        for guide in &shown {
            let on = lit.contains(guide);
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(
                    Color {
                        a: if on { 0.9 } else { 0.3 },
                        ..GUIDE
                    },
                    if on { 1.5 } else { 1.0 },
                )
            };
            let (shape, ..) = self.guide_view(state, *guide, at.unwrap_or(Point::ORIGIN));
            match shape {
                Shape::Line(o, d) => {
                    let (a, b) = (camera.to_screen(o), camera.to_screen(o + d));
                    let length = a.distance(b).max(1e-6);
                    let far = (b - a) * (reach * 2.0 / length);
                    frame.stroke(&Path::line(a - far, b + far), dashed);
                }
                Shape::Circle(c, r) => {
                    frame.stroke(&Path::circle(camera.to_screen(c), r * camera.zoom), dashed);
                }
            }
            if let (true, Some((u, v))) = (on, guide.followed()) {
                frame.stroke(&Path::line(screen(u), screen(v)), stroke(GUIDE, 3.0));
            }
        }

        let Some(at) = at.filter(|_| !lit.is_empty()) else {
            return;
        };
        let p = camera.to_screen(at);
        // Marks halfway along `a`–`b` (moved `aside` along it, to make room
        // for another): an arrowhead pointing along it, or two ticks across.
        let mark = |frame: &mut Frame, a: Point, b: Point, arrow: bool, aside: f32| {
            let d = b - a;
            let length = d.x.hypot(d.y);
            if length < 1.0 {
                return;
            }
            let (x, y) = (d.x / length, d.y / length);
            let (along, across) = (Vector::new(x, y), Vector::new(-y, x));
            let mid = a + d * 0.5 + along * aside;
            let path = Path::new(|path| {
                if arrow {
                    path.move_to(mid - along * 4.0 + across * 4.0);
                    path.line_to(mid + along * 2.0);
                    path.line_to(mid - along * 4.0 - across * 4.0);
                } else {
                    for k in [-2.0, 2.0] {
                        path.move_to(mid + along * k - across * 5.0);
                        path.line_to(mid + along * k + across * 5.0);
                    }
                }
            });
            frame.stroke(&path, stroke(GUIDE, 2.0));
        };
        // A right angle at `corner`, between the ways to `a` and `b`.
        let square = |frame: &mut Frame, corner: Point, a: Point, b: Point| {
            let unit = |q: Point| {
                let d = q - corner;
                d * (1.0 / d.x.hypot(d.y).max(1e-6))
            };
            let (u, w) = (unit(a) * 9.0, unit(b) * 9.0);
            let path = Path::new(|path| {
                path.move_to(corner + u);
                path.line_to(corner + u + w);
                path.line_to(corner + w);
            });
            frame.stroke(&path, stroke(GUIDE, 2.0));
        };
        for guide in &lit {
            // The new edge lining up: from where it starts to the point that
            // lines up.
            let (_, start, end) = self.guide_view(state, *guide, at);
            let p = camera.to_screen(end);
            match *guide {
                Guide::Corner { a, b } => {
                    frame.stroke(&Path::line(screen(a), p), stroke(GUIDE, 2.0));
                    frame.stroke(&Path::line(screen(b), p), stroke(GUIDE, 2.0));
                    square(frame, p, screen(a), screen(b));
                    continue;
                }
                Guide::Middle { a, b } => {
                    // Both edges, as long.
                    for v in [a, b] {
                        frame.stroke(&Path::line(screen(v), p), stroke(GUIDE, 2.0));
                        mark(frame, screen(v), p, false, 0.0);
                    }
                    continue;
                }
                Guide::Between { .. } => continue,
                _ => {}
            }
            let Some(start) = start.map(|at| camera.to_screen(at)) else {
                continue;
            };
            // The new edge, from where it starts; marked alike with the one
            // it follows.
            frame.stroke(&Path::line(start, p), stroke(GUIDE, 2.0));
            let Some((u, v)) = guide.followed() else {
                continue;
            };
            // Just as long: only if it is.
            let equal = (start.distance(p) - screen(u).distance(screen(v))).abs() < 0.75;
            let parallel = matches!(
                guide,
                Guide::Parallel { .. } | Guide::ParallelFrom { .. } | Guide::Across { .. }
            );
            // Both: the arrowheads just before the middle, the ticks just
            // after, side by side rather than over each other.
            let aside = if equal && parallel { 7.0 } else { 0.0 };
            if parallel {
                // Pointing the same way.
                let (a, b) = (screen(u), screen(v));
                let ahead = p - start;
                let same_way = (b - a).x * ahead.x + (b - a).y * ahead.y >= 0.0;
                let (a, b) = if same_way { (a, b) } else { (b, a) };
                mark(frame, a, b, true, -aside);
                mark(frame, start, p, true, -aside);
            }
            if let Guide::Normal { a, b, .. } = *guide {
                square(frame, screen(a), screen(b), p);
            }
            if let Guide::Square { m, n } = *guide {
                square(frame, screen(n), screen(m), p);
            }
            if equal {
                // Along each the way it points, so ticks land past arrows.
                let (a, b) = (screen(u), screen(v));
                let ahead = p - start;
                let same_way = (b - a).x * ahead.x + (b - a).y * ahead.y >= 0.0;
                let (a, b) = if same_way { (a, b) } else { (b, a) };
                mark(frame, a, b, false, aside);
                mark(frame, start, p, false, aside);
            }
        }
        if lit.len() >= 2 {
            let diamond = Path::new(|path| {
                path.move_to(p + Vector::new(0.0, -7.0));
                path.line_to(p + Vector::new(7.0, 0.0));
                path.line_to(p + Vector::new(0.0, 7.0));
                path.line_to(p + Vector::new(-7.0, 0.0));
                path.close();
            });
            frame.stroke(&diamond, stroke(GUIDE, 2.0));
        }
    }
}
