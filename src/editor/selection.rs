//! The selection (shape mode): moving, rotating and scaling it, with
//! proportional editing, snapped to join up; and drawing it.

use super::*;

/// What's selected (shape mode).
pub(super) const SELECTED: Color = Color::from_rgb8(0xff, 0x9e, 0x3b);

/// Moving the selection, the cursor now at `cursor` (world) after it was
/// away (adjusting how far proportional editing reaches, say): the move
/// goes on from there, as it was, rather than jumping to the cursor.
pub(super) fn rebase(interaction: &mut Interaction, cursor: Point) {
    match interaction {
        Interaction::MovingSelection { from, to, .. } => {
            *from += cursor - *to;
            *to = cursor;
        }
        Interaction::PivotingSelection {
            pivot,
            center,
            from,
            to,
        } => {
            let d = cursor - *center;
            *from = match pivot {
                Pivot::Rotate => {
                    let angle = |p: Point| (p.y - center.y).atan2(p.x - center.x);
                    let (sin, cos) = (angle(*from) - angle(*to)).sin_cos();
                    *center + Vector::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
                }
                Pivot::Scale => {
                    let scale = to.distance(*center) / from.distance(*center);
                    if scale > 1e-6 {
                        *center + d * (1.0 / scale)
                    } else {
                        *from
                    }
                }
            };
            *to = cursor;
        }
        _ => {}
    }
}

impl Editor<'_> {
    /// The lasso being drawn, the selected vertices (where the pending move
    /// puts them), the centre they rotate or scale around, and the cursor.
    pub(super) fn draw_selection(
        &self,
        frame: &mut Frame,
        state: &State,
        pending: Option<&Pending>,
        cursor: Option<Point>,
    ) {
        let camera = self.camera;

        if let (Interaction::Lassoing, Some((first, rest))) =
            (state.interaction, state.lasso.split_first())
        {
            let lasso = Path::new(|p| {
                p.move_to(camera.to_screen(*first));
                for &point in rest {
                    p.line_to(camera.to_screen(point));
                }
                p.close();
            });
            frame.fill(&lasso, Color { a: 0.08, ..HOVER });
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(HOVER, 1.5)
            };
            frame.stroke(&lasso, dashed);
        }

        let at = |v: VertexId| match pending {
            Some(pending) => pending.result.vertex(pending.changes.kept(v)),
            None => self.document.vertex(v),
        };
        // Editing proportionally: how far that reaches around what's moved
        // (the selection, else the vertex dragged or hovered), and how much
        // the vertices there go along.
        let tweaking = state
            .tweaking
            .is_some_and(|tweak| tweak.what == Tweaking::Reach);
        let subjects = match state.interaction {
            _ if !state.selection.is_empty() => state.selection.clone(),
            Interaction::MovingVertex { id, .. } => vec![id],
            Interaction::Idle => {
                let near = match state.tweaking {
                    Some(tweak) if tweaking => Some(tweak.at),
                    _ => cursor,
                };
                match near.and_then(|p| self.hit_test(p)) {
                    Some(Hover::Vertex(v)) => vec![v],
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        if let Some(reach) = self.proportional.filter(|_| !subjects.is_empty()) {
            let around = Path::new(|p| {
                for &v in &subjects {
                    p.circle(camera.to_screen(at(v)), reach);
                }
            });
            frame.fill(
                &around,
                Color {
                    a: if tweaking { 0.12 } else { 0.06 },
                    ..HOVER
                },
            );
            if let [v] = subjects[..] {
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(Color { a: 0.5, ..HOVER }, 1.0)
                };
                frame.stroke(&Path::circle(camera.to_screen(at(v)), reach), dashed);
            }
            for (v, w) in self.weights(&subjects) {
                if w < 1.0 {
                    let p = camera.to_screen(at(v));
                    frame.fill(&Path::circle(p, 3.5), Color { a: w, ..SELECTED });
                }
            }
            if let (true, Some(p)) = (tweaking, cursor) {
                let label = Point::new(p.x + 14.0, p.y - 18.0);
                for (offset, color) in [(1.0, BACKGROUND), (0.0, EDGE)] {
                    frame.fill_text(canvas::Text {
                        content: format!("{:.0} px", self.proportional.unwrap_or_default()),
                        position: label + Vector::new(offset, offset),
                        color,
                        size: 13.0.into(),
                        ..canvas::Text::default()
                    });
                }
            }
        }

        // The selection: its faces (all corners selected) tinted, its
        // edges (both ends) lit, its vertices as dots of their own colour.
        if !state.selection.is_empty() {
            let selected: std::collections::HashSet<VertexId> =
                state.selection.iter().copied().collect();
            let faces = Path::new(|p| {
                for t in self.document.triangle_ids() {
                    if t.iter().all(|v| selected.contains(v)) {
                        let [a, b, c] = t.map(|v| camera.to_screen(at(v)));
                        p.move_to(a);
                        p.line_to(b);
                        p.line_to(c);
                        p.close();
                    }
                }
            });
            frame.fill(
                &faces,
                Color {
                    a: 0.18,
                    ..SELECTED
                },
            );
            let edges = Path::new(|p| {
                for (a, b) in self.document.unique_edges() {
                    if selected.contains(&a) && selected.contains(&b) {
                        p.move_to(camera.to_screen(at(a)));
                        p.line_to(camera.to_screen(at(b)));
                    }
                }
            });
            frame.stroke(&edges, stroke(SELECTED, 2.0));
        }
        for &v in &state.selection {
            let p = camera.to_screen(at(v));
            frame.fill(&Path::circle(p, 5.0), SELECTED);
            frame.stroke(&Path::circle(p, 5.0), stroke(BACKGROUND, 1.5));
        }

        if let Interaction::PivotingSelection { center, .. } = state.interaction {
            let c = camera.to_screen(center);
            let plus = Path::new(|p| {
                p.move_to(c - Vector::new(6.0, 0.0));
                p.line_to(c + Vector::new(6.0, 0.0));
                p.move_to(c - Vector::new(0.0, 6.0));
                p.line_to(c + Vector::new(0.0, 6.0));
            });
            frame.stroke(&plus, stroke(JOINED, 2.0));
            if let Some(cursor) = cursor {
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[4.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(Color { a: 0.6, ..JOINED }, 1.0)
                };
                frame.stroke(&Path::line(c, cursor), dashed);
            }
        }

        // The cursor: a cross-hair for the lasso, arrows each way with a
        // selection to move.
        let Some(p) = cursor else {
            return;
        };
        match state.interaction {
            Interaction::Lassoing => {
                frame.stroke(&Path::circle(p, 4.0), stroke(HOVER, 1.5));
            }
            Interaction::Idle
            | Interaction::MovingSelection { .. }
            | Interaction::PivotingSelection { .. }
                if !state.selection.is_empty() =>
            {
                let arrows = Path::new(|path| {
                    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                        let tip = p + Vector::new(dx * 9.0, dy * 9.0);
                        path.move_to(p);
                        path.line_to(tip);
                        path.move_to(tip + Vector::new(-dx * 3.0 - dy * 3.0, -dy * 3.0 - dx * 3.0));
                        path.line_to(tip);
                        path.line_to(tip + Vector::new(-dx * 3.0 + dy * 3.0, -dy * 3.0 + dx * 3.0));
                    }
                });
                frame.stroke(&arrows, stroke(BACKGROUND, 3.5));
                frame.stroke(&arrows, stroke(HOVER, 1.5));
            }
            _ => {}
        }
    }

    /// How far to move `points` (world, already moved, together) so that
    /// one of them lands on something to snap to (see [`Self::snaps`]),
    /// best first: onto vertices, nearest first, then onto the rest. What
    /// `moving` says moves along isn't snapped to.
    pub(super) fn snap_offsets(
        &self,
        points: &[Point],
        moving: &dyn Fn(VertexId) -> bool,
    ) -> Vec<Vector> {
        let mut found: Vec<(bool, f32, Vector)> = Vec::new();
        for &p in points {
            let at = self.camera.to_screen(p);
            for (target, q) in self.snaps(at, moving, |u, v| moving(u) || moving(v), &[]) {
                let distance = self.camera.to_screen(q).distance(at);
                found.push((!matches!(target, Target::Vertex(_)), distance, q - p));
            }
        }
        found.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let mut offsets: Vec<Vector> = Vec::new();
        for (_, _, offset) in found {
            let near = |o: &Vector| (o.x - offset.x).hypot(o.y - offset.y) < 1e-4;
            if !offsets.iter().any(near) {
                offsets.push(offset);
            }
            if offsets.len() == MAX_SNAPS {
                break;
            }
        }
        offsets
    }

    /// Moving each of `selection` (and, editing proportionally, those
    /// around it) to where `f` takes it, all at once (the cursor `at`). `f`
    /// is told how much to move each: 1 fully, less for those around.
    /// `None` if that doesn't work out: it folds something over, say.
    pub(super) fn transform(
        &self,
        selection: &[VertexId],
        at: Point,
        f: impl Fn(Point, f32) -> Point,
    ) -> Option<Pending> {
        let moves = self
            .weights(selection)
            .into_iter()
            .map(|(v, w)| (v, f(self.document.vertex(v), w)))
            .collect();
        self.check(vec![Edit::MoveVertices { moves }], at)
    }

    /// Dragging vertex `id` to `to`, editing proportionally: those around
    /// it go along. Onto the best snap that works out (a vertex to weld
    /// with, an edge to join), else `to` itself. `None` if nothing works
    /// out: it would fold something over, say.
    pub(super) fn move_proportionally(&self, id: VertexId, to: Point) -> Option<(Point, Pending)> {
        let from = self.document.vertex(id);
        let snaps = self.snaps(
            self.camera.to_screen(to),
            |v| v == id,
            |u, v| u == id || v == id,
            &[],
        );
        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain([to])
            .find_map(|p| {
                let by = p - from;
                Some((p, self.transform(&[id], p, |q, w| q + by * w)?))
            })
    }

    /// How much each vertex goes along with `selection`: those selected
    /// fully (1); editing proportionally, those within reach (on screen) of
    /// one the less the further, smoothly down to nothing.
    pub(super) fn weights(&self, selection: &[VertexId]) -> Vec<(VertexId, f32)> {
        let mut weights: Vec<_> = selection.iter().map(|&v| (v, 1.0)).collect();
        let Some(reach) = self.proportional.filter(|_| !selection.is_empty()) else {
            return weights;
        };
        let reach = reach / self.camera.zoom;
        let selected: Vec<Point> = selection.iter().map(|&v| self.document.vertex(v)).collect();
        for v in self.document.unique_vertices() {
            if selection.contains(&v) {
                continue;
            }
            let p = self.document.vertex(v);
            let near = selected
                .iter()
                .map(|q| p.distance(*q))
                .fold(f32::INFINITY, f32::min);
            if near < reach {
                let t = 1.0 - near / reach;
                weights.push((v, t * t * (3.0 - 2.0 * t)));
            }
        }
        weights
    }

    /// The centre of `selection`: the average of its vertices.
    pub(super) fn centre(&self, selection: &[VertexId]) -> Point {
        let sum = selection
            .iter()
            .map(|&v| self.document.vertex(v))
            .fold(Vector::ZERO, |sum, p| sum + Vector::new(p.x, p.y));
        let n = selection.len().max(1) as f32;
        Point::new(sum.x / n, sum.y / n)
    }
}
