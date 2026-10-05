//! Merging: what an edit moved or pasted landing over the drawing. What
//! moved whole (every corner of a face) stays as it is, on top. What it
//! lands on, or comes so close to it would make slivers, is cleared away,
//! and so are the faces only some of whose corners moved (stretched, maybe
//! folded over); then what's left open is filled in again at once: a
//! constrained Delaunay triangulation, which makes faces as fat as they
//! can be, keeping the outlines (and the vertices that were there, clear
//! of what landed).

use std::time::Instant;

use spade::handles::FixedVertexHandle;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use super::*;
use crate::geometry::closest_on_segment;
use edit::Changed;
use quick::QuickMap;

impl Document {
    /// Carries out `edit` (with the vertices it `moved`, and the faces it
    /// `pasted`), merging what it changed into what it lands over (see
    /// [`Edit::Merging`]). Returns `false` if that doesn't work out: what
    /// moved whole overlapping itself (folded over), say.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn merge(
        &mut self,
        edit: &Edit,
        snap: f32,
        held: &[VertexId],
        moved: &[VertexId],
        pasted: &QuickSet<[(u32, u32); 3]>,
        changes: &mut Changes,
        before: &Document,
        deadline: Option<Instant>,
    ) -> bool {
        // Where things land, before joining up splits that.
        if !self.apply_unchecked(edit.clone(), changes, deadline) {
            return false;
        }
        // Cleared further than things count as touching, so what fills the
        // gap has room to be no thinner than that.
        let room = 2.0 * snap;
        let Some((top, cleared)) = self.to_merge(before, held, room) else {
            // Nothing over anything: just the edit.
            return self.normalize(moved, changes, before) && self.is_valid_after(before, pasted);
        };
        changes.merged = true;
        let top: Vec<[VertexId; 3]> = top.iter().map(|&t| self.triangles[t]).collect();
        let gap: Vec<[VertexId; 3]> = cleared.iter().map(|&t| self.triangles[t]).collect();
        for &t in cleared.iter().rev() {
            self.triangles.remove(t);
        }

        // Worked out among the faces around only (all that can change),
        // apart from the rest: quicker in a big drawing.
        let (min, max) = cells::bounds(gap.iter().chain(&top).flatten().map(|&v| self.vertices[v]));
        let margin = 2.0 * room + 1.0;
        let around = |t: &[VertexId; 3]| {
            let (lo, hi) = cells::bounds(t.iter().map(|&v| self.vertices[v]));
            hi.x >= min.x - margin
                && lo.x <= max.x + margin
                && hi.y >= min.y - margin
                && lo.y <= max.y + margin
        };
        let (near, far): (Vec<[VertexId; 3]>, Vec<[VertexId; 3]>) =
            self.triangles.iter().partition(|t| around(t));
        self.triangles = near;
        // The paint is done after (see `paint_merged`): not carried along.
        let colors = std::mem::take(&mut self.colors);
        let edge_styles = std::mem::take(&mut self.edge_styles);
        if deadline.is_some_and(|deadline| Instant::now() > deadline)
            || !self.fill_gap(&top, &gap, room)
            || !self.normalize(moved, changes, before)
        {
            return false;
        }
        self.triangles.extend(far);
        self.colors = colors;
        self.edge_styles = edge_styles;
        self.is_valid_after(before, pasted)
    }

    /// What to merge, if the edit made anything overlap (else `None`):
    /// the faces carried whole (all their corners moved, and `held` if
    /// any), to stay on top; and those to clear away (ids, ascending):
    /// the other faces that changed, and those from `before` that anything
    /// changed lies over, or that come within `room` of what's carried
    /// whole. `None` too if what's carried whole folds over itself: there's
    /// no merging that (see `merge`).
    #[allow(clippy::type_complexity)]
    fn to_merge(
        &self,
        before: &Document,
        held: &[VertexId],
        room: f32,
    ) -> Option<(Vec<TriangleId>, Vec<TriangleId>)> {
        let changed = Changed::since(self, before);
        let held: QuickSet<VertexId> = held.iter().copied().collect();
        let carried = |v: VertexId| {
            changed.vertex(v)
                && (held.is_empty() || held.contains(&v) || v >= before.vertices.len())
        };
        let corners = |t: TriangleId| self.triangles[t].map(|v| self.vertices[v]);
        let mut whole = Vec::new();
        let mut stretched = Vec::new();
        let mut rest = Vec::new();
        for (t, ids) in self.triangles.iter().enumerate() {
            if ids.iter().all(|&v| carried(v)) {
                whole.push(t);
            } else if ids.iter().any(|&v| changed.vertex(v)) || changed.is_new(*ids) {
                stretched.push(t);
            } else {
                rest.push(t);
            }
        }
        if whole.is_empty() && stretched.is_empty() {
            return None;
        }

        // Whether faces among `faces` overlap one in `others`, by `cells`
        // (of `faces`), or lie within `reach` of one.
        let shapes = |faces: &[TriangleId]| Cells::of_shapes(faces.iter().map(|&t| corners(t)));
        let near = |cells: &Cells, faces: &[TriangleId], t: TriangleId, reach: f32| {
            let shape = corners(t);
            let (min, max) = cells::bounds(shape.into_iter());
            let r = iced::Vector::new(reach, reach);
            let (min, max) = (min - r, max + r);
            cells.within(min, max).any(|k| {
                let other = corners(faces[k]);
                let (lo, hi) = cells::bounds(other.into_iter());
                faces[k] != t
                    && !(hi.x < min.x || max.x < lo.x || hi.y < min.y || max.y < lo.y)
                    && (overlap(shape, other) || (reach > 0.0 && apart(shape, other) < reach))
            })
        };
        let flipped = |t: TriangleId| {
            let [a, b, c] = corners(t);
            area2(a, b, c) <= 0.0
        };
        let changed_faces: Vec<TriangleId> = whole.iter().chain(&stretched).copied().collect();
        let changed_cells = shapes(&changed_faces);
        let overlapping = changed_faces.iter().any(|&t| flipped(t))
            || changed_faces
                .iter()
                .any(|&t| near(&changed_cells, &changed_faces, t, 0.0))
            || rest
                .iter()
                .any(|&t| near(&changed_cells, &changed_faces, t, 0.0));
        if !overlapping {
            return None;
        }
        // What moved whole can't be merged folded over itself.
        let whole_cells = shapes(&whole);
        if whole
            .iter()
            .any(|&t| flipped(t) || near(&whole_cells, &whole, t, 0.0))
        {
            return None;
        }
        let stretched_cells = shapes(&stretched);
        let mut cleared: Vec<TriangleId> = rest
            .into_iter()
            .filter(|&t| {
                near(&whole_cells, &whole, t, room) || near(&stretched_cells, &stretched, t, 0.0)
            })
            .chain(stretched.iter().copied())
            .collect();
        cleared.sort_unstable();
        Some((whole, cleared))
    }

    /// Fills what's left open once the faces of `gap` are cleared away,
    /// round the faces of `top` that stay: inside the outline of both
    /// together (as they are now), where `top` doesn't cover. All at once,
    /// as fat as can be (see the module's doc), the gap's vertices within
    /// `room` of `top` left out. `false` if that can't be done.
    fn fill_gap(&mut self, top: &[[VertexId; 3]], gap: &[[VertexId; 3]], room: f32) -> bool {
        let shape = |t: &[VertexId; 3]| t.map(|v| self.vertices[v]);

        // The outlines: of what stays on top, and of it and the gap
        // together. Each side a face has that no other of them has.
        let outline = |faces: &mut dyn Iterator<Item = &[VertexId; 3]>| {
            let mut sides: QuickMap<(VertexId, VertexId), u32> = QuickMap::default();
            for t in faces {
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    *sides.entry((a.min(b), a.max(b))).or_default() += 1;
                }
            }
            let mut once: Vec<(VertexId, VertexId)> = sides
                .into_iter()
                .filter(|&(_, n)| n == 1)
                .map(|(side, _)| side)
                .collect();
            once.sort_unstable();
            once
        };
        let top_outline = outline(&mut top.iter());
        let all_outline = outline(&mut top.iter().chain(gap));

        // The gap's vertices inside (not on an outline) come along, unless
        // covered by what stays on top, or too close to it.
        let on_outline: QuickSet<VertexId> = top_outline
            .iter()
            .chain(&all_outline)
            .flat_map(|&(a, b)| [a, b])
            .collect();
        let edges = Cells::of_shapes(
            top_outline
                .iter()
                .map(|&(a, b)| [self.vertices[a], self.vertices[b]]),
        );
        let covers = Cells::of_shapes(top.iter().map(shape));
        let covered = |p: Point| covers.around(p, 0.0).any(|k| contains(shape(&top[k]), p));
        // Whatever stays (what's on top, and the rest around): not filled
        // over, wherever the outline (folded, say) runs.
        let staying = Cells::of_shapes(self.triangles.iter().map(shape));
        let taken = |p: Point| {
            staying
                .around(p, 0.0)
                .any(|k| contains(shape(&self.triangles[k]), p))
        };
        let clear = |p: Point| {
            !covered(p)
                && edges.around(p, room).all(|k| {
                    let (a, b) = top_outline[k];
                    closest_on_segment(p, self.vertices[a], self.vertices[b]).1 >= room
                })
        };
        // The sides of what stays, where the filling goes: kept too, so
        // nothing filled runs across one (wherever the outline runs).
        let (lo, hi) = cells::bounds(
            all_outline
                .iter()
                .flat_map(|&(a, b)| [a, b])
                .map(|v| self.vertices[v]),
        );
        let mut staying_sides: Vec<(VertexId, VertexId)> = self
            .triangles
            .iter()
            .filter(|t| {
                let (min, max) = cells::bounds(shape(t).into_iter());
                max.x >= lo.x && min.x <= hi.x && max.y >= lo.y && min.y <= hi.y
            })
            .flat_map(|t| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])])
            .map(|(a, b)| (a.min(b), a.max(b)))
            .collect();
        staying_sides.sort_unstable();
        staying_sides.dedup();
        let mut points: Vec<VertexId> = on_outline
            .iter()
            .copied()
            .chain(staying_sides.iter().flat_map(|&(a, b)| [a, b]))
            .collect();
        points.sort_unstable();
        points.dedup();
        // Each of those inside only if clear of all the others so far (by
        // `room` / 2): two (all but) on the same spot would never join up.
        let spacing = room / 2.0;
        let cell = |p: Point| {
            (
                (p.x / spacing).floor() as i64,
                (p.y / spacing).floor() as i64,
            )
        };
        let mut grid: QuickMap<(i64, i64), Vec<Point>> = QuickMap::default();
        for &v in &points {
            let p = self.vertices[v];
            grid.entry(cell(p)).or_default().push(p);
        }
        let mut inside_points: Vec<VertexId> = gap
            .iter()
            .flatten()
            .copied()
            .filter(|v| !on_outline.contains(v) && clear(self.vertices[*v]))
            .collect();
        inside_points.sort_unstable();
        inside_points.dedup();
        for v in inside_points {
            let p = self.vertices[v];
            let (x, y) = cell(p);
            let crowded = (x - 1..=x + 1)
                .flat_map(|x| (y - 1..=y + 1).map(move |y| (x, y)))
                .filter_map(|c| grid.get(&c))
                .flatten()
                .any(|q| q.distance(p) < spacing);
            if !crowded && points.binary_search(&v).is_err() {
                grid.entry(cell(p)).or_default().push(p);
                points.push(v);
            }
        }
        points.sort_unstable();
        points.dedup();

        // Where the outlines cross: new vertices there, splitting both, so
        // the triangulation needn't (it's fragile at that).
        let n = self.vertices.len();
        let mut sides: Vec<(VertexId, VertexId)> = top_outline
            .iter()
            .chain(&all_outline)
            .chain(&staying_sides)
            .copied()
            .collect();
        sides.sort_unstable();
        sides.dedup();
        let mut crossings: Vec<Point> = Vec::new();
        let mut cuts: Vec<Vec<(f32, VertexId)>> = vec![Vec::new(); sides.len()];
        let ends = |&(a, b): &(VertexId, VertexId)| [self.vertices[a], self.vertices[b]];
        let near = Cells::of_shapes(sides.iter().map(ends));
        for i in 0..sides.len() {
            let [p, q] = ends(&sides[i]);
            let (min, max) = cells::bounds([p, q].into_iter());
            let mut others: Vec<usize> = near.within(min, max).filter(|&j| j > i).collect();
            others.sort_unstable();
            others.dedup();
            for j in others {
                let (u, v) = sides[j];
                if [u, v].iter().any(|w| *w == sides[i].0 || *w == sides[i].1) {
                    continue;
                }
                let [a, b] = ends(&sides[j]);
                let Some(x) = fill::crossing(p, q, a, b) else {
                    continue;
                };
                let id = n + crossings.len();
                crossings.push(x);
                cuts[i].push((p.distance(x), id));
                cuts[j].push((a.distance(x), id));
            }
        }
        let at = |v: VertexId| {
            if v < n {
                self.vertices[v]
            } else {
                crossings[v - n]
            }
        };

        // Inside the outline of both together: crossing it an odd number
        // of times going right.
        let rows = Cells::of_shapes(all_outline.iter().map(ends));
        let (_, far_right) = cells::bounds(all_outline.iter().flat_map(ends));
        let inside = |p: Point| {
            let mut seen: Vec<usize> = rows.within(p, Point::new(far_right.x + 1.0, p.y)).collect();
            seen.sort_unstable();
            seen.dedup();
            seen.into_iter()
                .filter(|&k| {
                    let [a, b] = ends(&all_outline[k]);
                    (a.y > p.y) != (b.y > p.y)
                        && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x)
                })
                .count()
                % 2
                == 1
        };

        // Each side's cuts in order along it, once.
        let chains: Vec<Vec<VertexId>> = sides
            .iter()
            .zip(&mut cuts)
            .map(|(&(a, b), cuts)| {
                cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
                let mut along: Vec<VertexId> = cuts.iter().map(|&(_, v)| v).collect();
                along.dedup();
                along.retain(|&v| v != a && v != b);
                along
            })
            .collect();

        // Triangulated, the outlines kept. (A degenerate case the
        // triangulation can't take is turned down, rather than crashing.)
        let triangulated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut cdt: ConstrainedDelaunayTriangulation<Point2<f64>> =
                ConstrainedDelaunayTriangulation::new();
            let mut ids: QuickMap<usize, VertexId> = QuickMap::default();
            let mut handles: QuickMap<VertexId, FixedVertexHandle> = QuickMap::default();
            for v in points.iter().copied().chain(n..n + crossings.len()) {
                let p = at(v);
                let handle = cdt.insert(Point2::new(p.x as f64, p.y as f64)).ok()?;
                ids.entry(handle.index()).or_insert(v);
                handles.insert(v, handle);
            }
            for (&(a, b), along) in sides.iter().zip(&chains) {
                let chain: Vec<FixedVertexHandle> = [a]
                    .into_iter()
                    .chain(along.iter().copied())
                    .chain([b])
                    .map(|v| handles[&v])
                    .collect();
                for pair in chain.windows(2) {
                    let (from, to) = (pair[0], pair[1]);
                    // Crossing a kept side all the same (all but touching
                    // it, say): split there after all.
                    if from != to
                        && !cdt.exists_constraint(from, to)
                        && cdt.try_add_constraint(from, to).is_empty()
                    {
                        cdt.add_constraint_and_split(from, to, |p| p);
                    }
                }
            }

            // Its faces inside, not under what stays on top.
            let faces = cdt
                .inner_faces()
                .filter(|face| {
                    let [a, b, c] = face.vertices().map(|v| {
                        let p = v.position();
                        Point::new(p.x as f32, p.y as f32)
                    });
                    let centre = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
                    inside(centre) && !taken(centre)
                })
                .map(|face| {
                    face.vertices().map(|v| {
                        let p = v.position();
                        // Those it split at itself: new vertices too.
                        ids.get(&v.fix().index())
                            .copied()
                            .ok_or(Point::new(p.x as f32, p.y as f32))
                    })
                })
                .collect::<Vec<_>>();
            Some(faces)
        }));
        let Ok(Some(faces)) = triangulated else {
            return false;
        };
        self.vertices.extend(crossings);
        let staying_count = self.triangles.len();
        let mut made: Vec<(Point, VertexId)> = Vec::new();
        for face in faces {
            let t = face.map(|corner| match corner {
                Ok(v) => v,
                Err(p) => match made.iter().find(|(q, _)| *q == p) {
                    Some(&(_, v)) => v,
                    None => {
                        self.vertices.push(p);
                        made.push((p, self.vertices.len() - 1));
                        self.vertices.len() - 1
                    }
                },
            });
            self.push_triangle(t);
        }
        // Over what stays all the same (a hair off, rounded): turned down
        // now, rather than left to join up (which wouldn't settle).
        let shape = |t: &[VertexId; 3]| t.map(|v| self.vertices[v]);
        let staying = Cells::of_shapes(self.triangles[..staying_count].iter().map(shape));
        !self.triangles[staying_count..].iter().any(|t| {
            let s = shape(t);
            let (min, max) = cells::bounds(s.into_iter());
            staying
                .within(min, max)
                .any(|k| overlap(s, shape(&self.triangles[k])))
        })
    }

    /// Paints `merged`, what [`Self::merge`] made of this document,
    /// from `landed` (what the edit made of it, painted, before anything
    /// was joined up or cut away):
    /// faces kept as they were keep their colour; new ones take the colour
    /// of the one they lie in, what landed first (it's on top).
    pub(super) fn paint_merged(&self, landed: &Document, merged: &mut Document) {
        let changed = Changed::since(landed, self);
        let corners = |t: [VertexId; 3]| t.map(|v| landed.vertices[v]);
        let (top, rest): (Vec<[VertexId; 3]>, Vec<[VertexId; 3]>) =
            landed.triangles.iter().partition(|&&t| changed.triangle(t));
        let existed: QuickSet<[VertexId; 3]> = landed.triangles.iter().map(|&t| key(t)).collect();
        let (top_cells, rest_cells) = (
            Cells::of_shapes(top.iter().map(|&t| corners(t))),
            Cells::of_shapes(rest.iter().map(|&t| corners(t))),
        );
        let colors: Vec<Option<Color>> = merged
            .triangles
            .iter()
            .map(|&t| {
                if let Some(&color) = landed.colors.get(&key(t)) {
                    return Some(color);
                }
                if existed.contains(&key(t)) {
                    return None;
                }
                let [a, b, c] = t.map(|v| merged.vertices[v]);
                let centre = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
                let find = |cells: &Cells, faces: &[[VertexId; 3]]| {
                    cells
                        .around(centre, 0.0)
                        .filter(|&k| contains(corners(faces[k]), centre))
                        .min()
                        .map(|k| faces[k])
                };
                let lay_in = find(&top_cells, &top).or_else(|| find(&rest_cells, &rest))?;
                landed.colors.get(&key(lay_in)).copied()
            })
            .collect();
        merged.set_colors(colors);
        merged.inherit_edge_styles(landed);
    }
}

/// How far apart two triangles are, if they don't overlap: the least
/// distance from a corner of one to a side of the other.
fn apart(t: [Point; 3], u: [Point; 3]) -> f32 {
    let sides = |s: [Point; 3]| [(s[0], s[1]), (s[1], s[2]), (s[2], s[0])];
    let from = |points: [Point; 3], other: [Point; 3]| {
        points
            .into_iter()
            .flat_map(|p| sides(other).map(|(a, b)| closest_on_segment(p, a, b).1))
            .fold(f32::INFINITY, f32::min)
    };
    from(t, u).min(from(u, t))
}
