//! A spatial index: things (points, edges, triangles) sorted into square
//! cells by their bounds, to find those near a place without going through
//! them all.

use iced::{Point, Vector};

use super::quick::QuickMap;

/// How many cells a thing may cover before it's kept apart instead (a long
/// edge across the drawing, say): looked at for every query.
const MAX_CELLS: i64 = 64;

pub(super) struct Cells {
    /// How wide a cell is (world units).
    size: f32,
    cells: QuickMap<(i64, i64), Vec<usize>>,
    /// Those covering too many cells to put in each.
    large: Vec<usize>,
}

impl Cells {
    /// For `count` things within `bounds` (world): cells about as many as
    /// there are things.
    pub(super) fn new(bounds: (Point, Point), count: usize) -> Self {
        let (min, max) = bounds;
        let (w, h) = (max.x - min.x, max.y - min.y);
        let count = count.max(1) as f32;
        let area = (w * h).max(w.max(h).powi(2) / count);
        Cells {
            size: (area / count).sqrt().max(1e-2),
            cells: QuickMap::default(),
            large: Vec::new(),
        }
    }

    /// The points `ids` (at their place in `points`), each by its id.
    pub(super) fn of_points(points: &[Point], ids: &[usize]) -> Self {
        let bounds = bounds(ids.iter().map(|&v| points[v]));
        let mut cells = Cells::new(bounds, ids.len());
        for &v in ids {
            cells.insert(v, points[v], points[v]);
        }
        cells
    }

    /// Each of `shapes` (its corners) by its place among them.
    pub(super) fn of_shapes<const N: usize>(shapes: impl Iterator<Item = [Point; N]>) -> Self {
        let shapes: Vec<(Point, Point)> =
            shapes.map(|corners| bounds(corners.into_iter())).collect();
        let all = bounds(shapes.iter().flat_map(|&(min, max)| [min, max]));
        let mut cells = Cells::new(all, shapes.len());
        for (id, (min, max)) in shapes.into_iter().enumerate() {
            cells.insert(id, min, max);
        }
        cells
    }

    fn cell(&self, p: Point) -> (i64, i64) {
        (
            (p.x / self.size).floor() as i64,
            (p.y / self.size).floor() as i64,
        )
    }

    /// Adds thing `id`, within `min`–`max`.
    pub(super) fn insert(&mut self, id: usize, min: Point, max: Point) {
        let ((x0, y0), (x1, y1)) = (self.cell(min), self.cell(max));
        if (x1 - x0 + 1) * (y1 - y0 + 1) > MAX_CELLS {
            self.large.push(id);
            return;
        }
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    /// The things in the cells the rectangle `min`–`max` touches: all that
    /// may reach into it (and some that don't); a thing in several of
    /// those cells, more than once.
    pub(super) fn within(&self, min: Point, max: Point) -> impl Iterator<Item = usize> + '_ {
        let ((x0, y0), (x1, y1)) = (self.cell(min), self.cell(max));
        // A query wider than there are cells: just all of them.
        let wide = (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > self.cells.len() as i64;
        let cells: Box<dyn Iterator<Item = &Vec<usize>>> = if wide {
            Box::new(self.cells.values())
        } else {
            Box::new(
                (y0..=y1)
                    .flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
                    .filter_map(|cell| self.cells.get(&cell)),
            )
        };
        cells.flatten().chain(&self.large).copied()
    }

    /// As [`Self::within`], around `p` as far as `reach`.
    pub(super) fn around(&self, p: Point, reach: f32) -> impl Iterator<Item = usize> + '_ {
        let reach = Vector::new(reach, reach);
        self.within(p - reach, p + reach)
    }
}

/// The bounds of `points`: their least and greatest coordinates.
pub(super) fn bounds(mut points: impl Iterator<Item = Point>) -> (Point, Point) {
    let Some(first) = points.next() else {
        return (Point::ORIGIN, Point::ORIGIN);
    };
    points.fold((first, first), |(min, max), p| {
        (
            Point::new(min.x.min(p.x), min.y.min(p.y)),
            Point::new(max.x.max(p.x), max.y.max(p.y)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_reaches_into_a_place_is_found() {
        let points: Vec<Point> = (0..100)
            .map(|i| Point::new((i % 10) as f32 * 10.0, (i / 10) as f32 * 10.0))
            .collect();
        let ids: Vec<usize> = (0..100).collect();
        let cells = Cells::of_points(&points, &ids);
        let mut near: Vec<usize> = cells
            .around(Point::new(50.0, 50.0), 1.0)
            .filter(|&v| points[v].distance(Point::new(50.0, 50.0)) <= 1.0)
            .collect();
        near.dedup();
        assert_eq!(near, [55]);

        // A long edge across it all is found anywhere along it.
        let edges = [
            [Point::new(0.0, 0.0), Point::new(1000.0, 0.0)],
            [Point::new(0.0, 5.0), Point::new(1.0, 5.0)],
        ];
        let cells = Cells::of_shapes(edges.into_iter());
        assert!(cells.around(Point::new(900.0, 0.0), 1.0).any(|e| e == 0));
        assert!(cells.around(Point::new(0.5, 5.0), 0.1).any(|e| e == 1));
    }
}
