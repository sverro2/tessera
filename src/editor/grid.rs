//! Snapping to a grid (holding Ctrl while dragging, shape mode): the point
//! dragged lands on the grid; moving several together (the selection, an
//! extrusion, a paste), the one nearest the cursor does: always onto a
//! point it shows. Zoomed far out, where its points would crowd, just every
//! tenth is shown (those of the dot grid), and snapped to; further out,
//! every hundredth. With Shift too, grid points on a guide come first (the
//! grid has the say).

use std::collections::HashMap;

use super::*;

/// Every this many steps of the grid, either way, a dot of the dot grid
/// on the canvas.
pub(super) const MAJOR: i64 = 10;

/// Where the snapping grid meets the dot grid: drawn lighter, and larger.
const MAJOR_DOT: Color = Color::from_rgb8(0xc0, 0xca, 0xf5);

/// How far apart (screen px) the grid points shown (and snapped to) are at
/// least: zoomed far out, just every tenth is, or hundredth.
const MIN_SPACING: f32 = 6.0;

/// The grid: its step (world units), from the world's origin, so it stays
/// put whatever else changes (a document placed on it lines up with it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grid {
    pub step: f32,
}

impl Default for Grid {
    fn default() -> Self {
        Grid { step: 10.0 }
    }
}

/// What snapping to the grid came to: where the point lined up, what that
/// does; the guides that would do and those near (with Shift).
pub(super) type Gridded = (Option<LinedUp>, Vec<Guide>, Vec<Guide>);

impl Grid {
    /// How many of its steps apart the points shown and snapped to are at
    /// `zoom`: 1, else 10, 100, … for them to be far enough apart on screen.
    pub fn stride_at(self, zoom: f32) -> i64 {
        let mut stride = 1;
        while self.step * stride as f32 * zoom < MIN_SPACING && stride < 1_000_000 {
            stride *= MAJOR;
        }
        stride
    }
}

impl Editor<'_> {
    /// The distance between the grid points shown and snapped to (world).
    fn snap_step(&self) -> f32 {
        self.grid.step * self.grid.stride_at(self.camera.zoom) as f32
    }

    /// The grid points around `p` (world) to try, best first: with `prefer`,
    /// those close by it says (on a guide, say), then the nearest.
    pub(super) fn grid_points(&self, p: Point, prefer: &dyn Fn(Point) -> bool) -> Vec<Point> {
        let step = self.snap_step();
        let o = Point::ORIGIN;
        let (i, j) = (((p.x - o.x) / step).round(), ((p.y - o.y) / step).round());
        let mut points: Vec<(bool, f32, Point)> = Vec::new();
        for di in -2..=2 {
            for dj in -2..=2 {
                let q = Point::new(o.x + (i + di as f32) * step, o.y + (j + dj as f32) * step);
                let d = q.distance(p);
                points.push((!(d <= 1.5 * step && prefer(q)), d, q));
            }
        }
        points.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        points
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(.., q)| q)
            .collect()
    }

    /// For points moved together (world, moved by as much as the cursor),
    /// how much further to move them for the one nearest the cursor
    /// (`at`) to land on the grid, best first (`prefer` taking that).
    fn grid_offsets(
        &self,
        points: &[Point],
        at: Point,
        prefer: &dyn Fn(Vector) -> bool,
    ) -> Vec<Vector> {
        let Some(&nearest) = points
            .iter()
            .min_by(|p, q| p.distance(at).total_cmp(&q.distance(at)))
        else {
            return Vec::new();
        };
        self.grid_points(nearest, &|q| prefer(q - nearest))
            .into_iter()
            .map(|q| q - nearest)
            .collect()
    }

    /// Where what `pending` places ends up (world): the selection moved,
    /// a piece pasted, an extrusion's far ends; else the point dragged.
    pub(super) fn placed(&self, state: &State, pending: &Pending) -> Vec<Point> {
        let pasted = match pending.edits.first().map(Edit::inner) {
            Some(Edit::Paste { piece }) => Some(piece),
            _ => None,
        };
        match (state.interaction, pasted) {
            (Interaction::MovingSelection { .. }, _) => state
                .selection
                .iter()
                .map(|&v| pending.result.vertex(pending.changes.kept(v)))
                .collect(),
            (Interaction::Pasting { .. }, Some(piece)) => piece.vertices.clone(),
            // Not the ends extruded from: they stay.
            (Interaction::Extruding { .. }, Some(piece)) => piece
                .vertices
                .iter()
                .copied()
                .filter(|&p| {
                    !state
                        .selection
                        .iter()
                        .any(|&v| self.document.vertex(v).distance(p) < 1e-3)
                })
                .collect(),
            _ => vec![pending.at],
        }
    }

    /// Whether any of `guides` runs through any of `points`.
    pub(super) fn on_guide(&self, guides: &[Guide], points: &[Point]) -> bool {
        guides
            .iter()
            .any(|&guide| points.iter().any(|&p| self.runs_through(guide, p)))
    }

    /// A new triangle started at `from`, the cursor at `world`: its dragged
    /// corner on the grid (with `shift`, one where it or its third corner is
    /// on a guide first).
    pub(super) fn grid_create(&self, from: Point, world: Point, shift: bool) -> Gridded {
        let (fine, near) = if shift {
            let (fine, near, _) = self.guided_creation(from, world);
            (fine, near)
        } else {
            Default::default()
        };
        let prefer = |p: Point| self.on_guide(&near, &[p, self.third_corner(from, p)]);
        let found = self
            .grid_points(world, &prefer)
            .into_iter()
            .find_map(|p| Some((p, self.create(from, p, p)?)));
        (found, fine, near)
    }

    /// Dragging vertex `id`, the cursor at `world`: onto the grid (editing
    /// proportionally, those around going along).
    pub(super) fn grid_move(&self, id: VertexId, world: Point, shift: bool) -> Gridded {
        let (fine, near) = match shift {
            true => self
                .guided(Dragged::Vertex(id), world)
                .map(|(fine, near, _)| (fine, near))
                .unwrap_or_default(),
            false => Default::default(),
        };
        let from = self.document.vertex(id);
        let found = self
            .grid_points(world, &|p| self.on_guide(&near, &[p]))
            .into_iter()
            .find_map(|p| {
                let pending = match self.proportional {
                    Some(_) => self.transform(&[id], p, |q, w| q + (p - from) * w),
                    None => self.check(vec![Edit::MoveVertex { id, to: p }], p),
                }?;
                Some((p, pending))
            });
        (found, fine, near)
    }

    /// Extending from edge `a`–`b` (grabbed at `start`), the cursor at
    /// `world`: the new corner on the grid. Back on the edge, nothing.
    pub(super) fn grid_extend(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        world: Point,
        shift: bool,
    ) -> Gridded {
        let (fine, near) = match shift {
            true => self
                .guided(Dragged::Apex { a, b, start }, world)
                .map(|(fine, near, _)| (fine, near))
                .unwrap_or_default(),
            false => Default::default(),
        };
        let found = self
            .grid_points(world, &|p| self.on_guide(&near, &[p]))
            .into_iter()
            .filter(|&p| !self.is_near_edge(p, a, b))
            .find_map(|p| Some((p, self.extend_exact(a, b, start, p)?)));
        (found, fine, near)
    }

    /// Moving the selection by as much as the cursor moved (`by`, the
    /// cursor at `world`): the vertex nearest the cursor onto the grid.
    pub(super) fn grid_move_selection(
        &self,
        selection: &[VertexId],
        by: Vector,
        world: Point,
    ) -> Option<Pending> {
        let moved: Vec<Point> = selection
            .iter()
            .map(|&v| self.document.vertex(v) + by)
            .collect();
        self.grid_offsets(&moved, world, &|_| false)
            .into_iter()
            .find_map(|snap| self.transform(selection, world, |p, w| p + (by + snap) * w))
    }

    /// Pasting `piece` moved by `by` (the cursor at `world`): its vertex
    /// nearest the cursor onto the grid.
    pub(super) fn grid_paste(&self, piece: &Piece, by: Vector, world: Point) -> Option<Pending> {
        let moved = piece.moved(by);
        self.grid_offsets(&moved.vertices, world, &|_| false)
            .into_iter()
            .find_map(|snap| {
                let piece = piece.moved(by + snap);
                self.check(vec![self.merging(Edit::Paste { piece })], world)
            })
    }

    /// Extruding `selection` from `from`, the cursor at `world`: the far end
    /// nearest the cursor on the grid (with `shift`, one where that or the
    /// way it goes is on a guide first). Lined up at where the cursor goes
    /// for that.
    pub(super) fn grid_extrude(
        &self,
        selection: &[VertexId],
        from: Point,
        world: Point,
        shift: bool,
    ) -> Gridded {
        let (fine, near) = if shift {
            let (fine, near, _) = self.guided_extrusion(selection, from, world);
            (fine, near)
        } else {
            Default::default()
        };
        let by = world - from;
        let ends: Vec<Point> = self
            .extruded_ends(selection)
            .into_iter()
            .map(|v| self.document.vertex(v) + by)
            .collect();
        let prefer = |snap: Vector| {
            let mut points: Vec<Point> = ends.iter().map(|&e| e + snap).collect();
            points.push(world + snap);
            self.on_guide(&near, &points)
        };
        let found = self
            .grid_offsets(&ends, world, &prefer)
            .into_iter()
            .find_map(|snap| {
                let at = world + snap;
                Some((at, self.extrude(selection, by + snap, at)?))
            });
        (found, fine, near)
    }

    /// The grid snapped to, around `points` (world: what's placed on it):
    /// dots, fading further off, those of the dot grid lighter. Zoomed far
    /// out, just those it snaps to (see [`Grid::stride_at`]). Around many,
    /// not as far around each (and around the first few hundred only).
    pub(super) fn draw_snap_grid(&self, frame: &mut Frame, points: &[Point]) {
        let step = self.grid.step;
        let stride = self.grid.stride_at(self.camera.zoom);
        let spacing = step * stride as f32;
        let reach = if points.len() > 1 { 2.5 } else { 3.5 } * spacing;
        // Each dot once, as strong as it is near the nearest point.
        let mut dots: HashMap<(i64, i64), f32> = HashMap::new();
        for &at in points.iter().take(256) {
            let index = |p: f32| (p / spacing).round() as i64;
            let (i, j) = (index(at.x), index(at.y));
            for di in -4..=4 {
                for dj in -4..=4 {
                    let (gi, gj) = ((i + di) * stride, (j + dj) * stride);
                    let q = Point::new(gi as f32 * step, gj as f32 * step);
                    let d = q.distance(at);
                    if d <= reach {
                        let alpha = 0.8 * (1.0 - d / reach);
                        let dot = dots.entry((gi, gj)).or_default();
                        *dot = dot.max(alpha);
                    }
                }
            }
        }
        for ((gi, gj), alpha) in dots {
            let q = Point::new(gi as f32 * step, gj as f32 * step);
            let major = gi.rem_euclid(MAJOR) == 0 && gj.rem_euclid(MAJOR) == 0;
            let (color, radius) = if major {
                (MAJOR_DOT, 2.5)
            } else {
                (GUIDE, 1.75)
            };
            frame.fill(
                &Path::circle(self.camera.to_screen(q), radius),
                Color {
                    a: alpha.max(if major { 0.5 } else { 0.0 }),
                    ..color
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoomed_far_out_it_snaps_to_every_tenth_point_or_hundredth() {
        let grid = Grid::default();
        assert_eq!(grid.stride_at(1.0), 1);
        assert_eq!(grid.stride_at(0.6), 1);
        assert_eq!(grid.stride_at(0.5), 10);
        assert_eq!(grid.stride_at(0.1), 10);
        assert_eq!(grid.stride_at(0.05), 100);
        assert_eq!(grid.stride_at(0.01), 100);
    }
}
