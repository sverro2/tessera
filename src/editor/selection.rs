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

/// The lasso's cursor at `p` (screen): a loop of rope above and to the
/// right, crossing itself, its end trailing down to a dot where the cursor
/// is; taking out of the selection (`removes`), with a minus by it.
fn draw_lasso_cursor(frame: &mut Frame, p: Point, removes: bool) {
    let at = |x: f32, y: f32| p + Vector::new(x, y);
    // Where the rope crosses itself.
    let (cx, cy) = (5.0, -6.0);
    let rope = Path::new(|path| {
        path.move_to(at(cx - 3.0, cy + 3.0));
        path.line_to(at(cx, cy));
        path.bezier_curve_to(
            at(cx + 2.0, cy - 3.0),
            at(cx - 4.0, cy - 14.0),
            at(cx + 6.0, cy - 17.0),
        );
        path.bezier_curve_to(
            at(cx + 16.0, cy - 20.0),
            at(cx + 22.0, cy - 10.0),
            at(cx + 14.0, cy - 4.0),
        );
        path.bezier_curve_to(
            at(cx + 9.0, cy - 1.0),
            at(cx + 3.0, cy - 1.0),
            at(cx - 2.0, cy - 2.0),
        );
        // The end, down to the cursor.
        path.move_to(at(cx - 3.0, cy + 3.0));
        path.bezier_curve_to(at(cx - 6.0, cy + 6.0), at(2.0, -1.0), p);
    });
    let outline = Stroke {
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..stroke(BACKGROUND, 3.5)
    };
    let line = Stroke {
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..stroke(HOVER, 1.5)
    };
    frame.stroke(&rope, outline);
    frame.stroke(&rope, line);
    frame.fill(&Path::circle(p, 2.5), BACKGROUND);
    frame.fill(&Path::circle(p, 1.5), HOVER);
    if removes {
        let minus = Path::line(at(-9.0, -16.0), at(-3.0, -16.0));
        frame.stroke(
            &minus,
            Stroke {
                line_cap: LineCap::Round,
                ..stroke(BACKGROUND, 4.0)
            },
        );
        frame.stroke(
            &minus,
            Stroke {
                line_cap: LineCap::Round,
                ..stroke(REMOVED, 2.0)
            },
        );
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
            // Taking out of the selection: in red.
            let color = if state.lasso_removes { REMOVED } else { HOVER };
            frame.fill(&lasso, Color { a: 0.08, ..color });
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[6.0, 4.0],
                    offset: 0,
                },
                ..stroke(color, 1.5)
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
        // Where it reaches from (screen): what's moved; else, tweaking with
        // nothing to move, where that started, to see how far it reaches.
        let mut centres: Vec<Point> = subjects.iter().map(|&v| camera.to_screen(at(v))).collect();
        let weights = match state.tweaking {
            Some(tweak) if tweaking && subjects.is_empty() => {
                centres.push(tweak.at);
                self.weights_around(tweak.at)
            }
            _ => self.weights(&subjects),
        };
        if let Some(reach) = self.proportional.filter(|_| !centres.is_empty()) {
            let around = Path::new(|p| {
                for &centre in &centres {
                    p.circle(centre, reach);
                }
            });
            frame.fill(
                &around,
                Color {
                    a: if tweaking { 0.12 } else { 0.06 },
                    ..HOVER
                },
            );
            if let [centre] = centres[..] {
                let dashed = Stroke {
                    line_dash: LineDash {
                        segments: &[6.0, 4.0],
                        offset: 0,
                    },
                    ..stroke(Color { a: 0.5, ..HOVER }, 1.0)
                };
                frame.stroke(&Path::circle(centre, reach), dashed);
            }
            for (v, w) in weights {
                if w < 1.0 {
                    let p = camera.to_screen(at(v));
                    frame.fill(&Path::circle(p, 3.5), Color { a: w, ..SELECTED });
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

        // The cursor: a lasso for the lasso, arrows each way with a
        // selection to move.
        let Some(p) = cursor else {
            return;
        };
        match state.interaction {
            // Drawing a lasso, or about to (after Q).
            Interaction::Lassoing => draw_lasso_cursor(frame, p, state.lasso_removes),
            Interaction::Idle if state.lasso_armed => {
                draw_lasso_cursor(frame, p, state.lasso_removes);
            }
            Interaction::Idle
            | Interaction::MovingSelection { .. }
            | Interaction::PivotingSelection { .. }
            | Interaction::Extruding { .. }
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
        let screen: Vec<Point> = points.iter().map(|&p| self.camera.to_screen(p)).collect();
        let Some(&first) = screen.first() else {
            return Vec::new();
        };
        let area = screen.iter().fold((first, first), |(min, max), p| {
            (
                Point::new(min.x.min(p.x), min.y.min(p.y)),
                Point::new(max.x.max(p.x), max.y.max(p.y)),
            )
        });
        let targets = self.snap_targets(area, moving, |u, v| moving(u) || moving(v), &[]);
        for (&p, &at) in points.iter().zip(&screen) {
            for (target, q) in self.snaps_among(&targets, at, &[]) {
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
        self.check(
            vec![self.merging_held(Edit::MoveVertices { moves }, selection)],
            at,
        )
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
        let joins = must_join(&snaps, &[]);
        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain((!joins).then_some(to))
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

    /// How much each vertex within reach of `centre` (screen) would go
    /// along with something moved there, as [`Self::weights`] has it.
    pub(super) fn weights_around(&self, centre: Point) -> Vec<(VertexId, f32)> {
        let Some(reach) = self.proportional else {
            return Vec::new();
        };
        self.document
            .unique_vertices()
            .into_iter()
            .filter_map(|v| {
                let near = self
                    .camera
                    .to_screen(self.document.vertex(v))
                    .distance(centre);
                (near < reach).then(|| {
                    let t = 1.0 - near / reach;
                    (v, t * t * (3.0 - 2.0 * t))
                })
            })
            .collect()
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
