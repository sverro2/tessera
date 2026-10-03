//! Carrying out edits: what each does to the triangles, and settling the
//! result so the drawing stays valid (no folds, no T-junctions, nothing
//! overlapping), its faces and edges keeping their paint.

use super::*;

/// What changed in a document since `before`: the vertices that are new or
/// moved, and (to tell the triangles that are new) the triangles before.
pub(super) struct Changed {
    /// Sorted.
    vertices: Vec<VertexId>,
    /// The triangles before (sorted corners); `None` if they're the same as
    /// now (nothing new: only moved, say).
    before: Option<QuickSet<[VertexId; 3]>>,
}

impl Changed {
    pub(super) fn since(document: &Document, before: &Document) -> Self {
        let vertices = document
            .unique_vertices()
            .into_iter()
            .filter(|&v| before.vertices.get(v) != Some(&document.vertices[v]))
            .collect();
        let same = document.triangles == before.triangles;
        Changed {
            vertices,
            before: (!same).then(|| before.triangles.iter().map(|&t| key(t)).collect()),
        }
    }

    /// Whether triangle `t` is new: it wasn't there before.
    pub(super) fn is_new(&self, t: [VertexId; 3]) -> bool {
        self.before
            .as_ref()
            .is_some_and(|before| !before.contains(&key(t)))
    }

    /// Whether vertex `v` is new, or moved.
    pub(super) fn vertex(&self, v: VertexId) -> bool {
        self.vertices.binary_search(&v).is_ok()
    }

    /// Whether triangle `t` is new, or has a changed corner.
    pub(super) fn triangle(&self, t: [VertexId; 3]) -> bool {
        t.iter().any(|&v| self.vertex(v)) || self.is_new(t)
    }
}

/// How many changed vertices make sorting all of them into [`Cells`] worth
/// it, rather than going through them all for each.
const MANY: usize = 64;

/// How many new faces (or edges) make sorting those from before into
/// [`Cells`] worth it, to find the one each lay in (or along).
const FEW: usize = 16;

impl Document {
    /// Checks the invariants, assuming they held in `before`: only triangles
    /// that are new or have a moved corner need checking for overlaps.
    pub(super) fn is_valid_after(&self, before: &Document) -> bool {
        if !self.triangles().all(|[a, b, c]| area2(a, b, c) > 0.0) {
            return false;
        }

        // The same faces as before (moved, say): none new.
        let existed: Option<QuickSet<[VertexId; 3]>> = (self.triangles != before.triangles)
            .then(|| before.triangles.iter().map(|&t| key(t)).collect());
        let changed = |t: &[VertexId; 3]| {
            t.iter()
                .any(|&v| before.vertices.get(v) != Some(&self.vertices[v]))
                || existed
                    .as_ref()
                    .is_some_and(|existed| !existed.contains(&key(*t)))
        };

        let changed: Vec<usize> = (0..self.triangles.len())
            .filter(|&i| changed(&self.triangles[i]))
            .collect();
        let corners = |i: usize| self.triangles[i].map(|v| self.vertices[v]);
        // Each against those it may reach: with many, those in the cells it
        // touches; with a few, all of them.
        let faces = (changed.len() > FEW).then(|| Cells::of_shapes(self.triangles()));
        changed.into_iter().all(|i| {
            let points = corners(i);
            let (min, max) = cells::bounds(points.into_iter());
            // Quickly apart by their bounds, else not overlapping.
            let clear = |j: usize| {
                let other = corners(j);
                let (umin, umax) = cells::bounds(other.into_iter());
                i == j
                    || umax.x < min.x
                    || max.x < umin.x
                    || umax.y < min.y
                    || max.y < umin.y
                    || !overlap(points, other)
            };
            match &faces {
                Some(faces) => faces.within(min, max).all(clear),
                None => (0..self.triangles.len()).all(clear),
            }
        })
    }

    /// Paints `shaped`, what `edit` made of this document (shape only, as
    /// [`Self::preview_shape_until`] leaves it), as [`Self::preview`] would.
    pub fn paint_after(&self, edit: &Edit, shaped: &mut Document) {
        // The same faces (moved, say): the same paint, as it came along.
        if shaped.triangles == self.triangles {
            return;
        }
        // A mirror image takes its paint from what it mirrors; a pasted
        // piece brings its own.
        let added;
        let source = match edit {
            Edit::Mirror { mirror } => {
                added = self.with_reflection(*mirror);
                &added
            }
            Edit::Paste { piece } => {
                added = self.with_piece(piece);
                &added
            }
            _ => self,
        };
        shaped.inherit_colors(source);
        shaped.inherit_edge_styles(source);
    }

    pub(super) fn apply_unchecked(
        &mut self,
        edit: Edit,
        changes: &mut Changes,
        deadline: Option<Instant>,
    ) -> bool {
        match edit {
            Edit::AddTriangle { corners, snap } => self.fill(corners, snap, changes, deadline),
            Edit::InsertVertex { at } => self.insert_vertex(at),
            Edit::RemoveTriangle { triangle } => {
                triangle < self.triangles.len() && {
                    self.triangles.remove(triangle);
                    true
                }
            }
            Edit::MoveVertex { id, to } => self.move_vertex(id, to),
            Edit::Mirror { mirror } => {
                *self = self.with_reflection(mirror);
                true
            }
            Edit::Paste { piece } => {
                let fine = piece
                    .vertices
                    .iter()
                    .all(|p| p.x.is_finite() && p.y.is_finite())
                    && piece
                        .triangles
                        .iter()
                        .flatten()
                        .all(|&v| v < piece.vertices.len());
                // A face landing right on one there is would be welded onto
                // it, and dropped as the same: it lies over it all the same.
                let same = |p: Point, q: Point| p.distance(q) <= tolerance(0.0);
                let faces = Cells::of_shapes(self.triangles());
                let on_one = piece.triangles.iter().any(|t| {
                    let corners = t.map(|v| piece.vertices[v]);
                    faces.around(corners[0], tolerance(0.0)).any(|u| {
                        let u = self.triangles[u].map(|v| self.vertices[v]);
                        corners.iter().all(|&p| u.iter().any(|&q| same(p, q)))
                    })
                });
                fine && !on_one && {
                    *self = self.with_piece(&piece);
                    true
                }
            }
            Edit::MoveVertices { moves } => {
                let used = self.unique_vertices();
                moves.iter().all(|(id, to)| {
                    used.binary_search(id).is_ok() && to.x.is_finite() && to.y.is_finite()
                }) && {
                    for &(id, to) in &moves {
                        self.vertices[id] = to;
                    }
                    true
                }
            }
            Edit::Paint { triangle, color } => {
                let Some(&t) = self.triangles.get(triangle) else {
                    return false;
                };
                match color {
                    Some(color) => self.colors.insert(key(t), color),
                    None => self.colors.remove(&key(t)),
                };
                true
            }
            Edit::PaintEdge { a, b, style } => {
                let edge = (a.min(b), a.max(b));
                if self.unique_edges().binary_search(&edge).is_err() {
                    return false;
                }
                match style {
                    Some(style) => self.edge_styles.insert(edge, style),
                    None => self.edge_styles.remove(&edge),
                };
                true
            }
            Edit::PaintAll { color } => {
                self.colors = match color {
                    Some(color) => self.triangles.iter().map(|&t| (key(t), color)).collect(),
                    None => BTreeMap::new(),
                };
                true
            }
            Edit::PaintAllEdges { style } => {
                self.edge_styles = match style {
                    Some(style) => self
                        .unique_edges()
                        .into_iter()
                        .map(|edge| (edge, style))
                        .collect(),
                    None => BTreeMap::new(),
                };
                true
            }
        }
    }

    /// After an edit from `before`: edges that kept their ends keep their
    /// style; new edges lying along an edge from before (a piece of it) take
    /// its style (none, if erased); other new edges, the default. Styles of
    /// edges gone are dropped.
    pub(super) fn inherit_edge_styles(&mut self, before: &Document) {
        let existed = before.unique_edges();
        let on = |p: Point, a: Point, b: Point| {
            crate::geometry::closest_on_segment(p, a, b).1 <= tolerance(a.distance(b))
        };
        // To find those new edges lie along: with many new ones, sorted into
        // cells first.
        let edges = self.unique_edges();
        let new = edges
            .iter()
            .filter(|edge| existed.binary_search(edge).is_err())
            .count();
        let near = (new > FEW).then(|| {
            Cells::of_shapes(
                existed
                    .iter()
                    .map(|&(a, b)| [before.vertices[a], before.vertices[b]]),
            )
        });

        self.edge_styles = edges
            .into_iter()
            .filter_map(|edge| {
                let style = if existed.binary_search(&edge).is_ok() {
                    self.edge_styles.get(&edge).copied()
                } else {
                    let (p, q) = (self.vertices[edge.0], self.vertices[edge.1]);
                    let lies_along = |&(a, b): &(VertexId, VertexId)| {
                        let (a, b) = (before.vertices[a], before.vertices[b]);
                        on(p, a, b) && on(q, a, b)
                    };
                    let along = match &near {
                        Some(near) => near
                            .around(p, tolerance(p.distance(q)))
                            .map(|i| existed[i])
                            .find(lies_along),
                        None => existed.iter().copied().find(lies_along),
                    };
                    match along {
                        Some((a, b)) => before.edge_style(a, b),
                        None => Some(DEFAULT_EDGE),
                    }
                };
                Some((edge, style?))
            })
            .collect();
    }

    /// After an edit from `before`: faces that kept their corners keep
    /// their colour; new faces take that of the face they lay in before (by
    /// their centre; none, if erased); new faces in empty space, the
    /// default. Colours of faces gone are dropped.
    pub(super) fn inherit_colors(&mut self, before: &Document) {
        let existed: QuickSet<_> = before.triangles.iter().map(|&t| key(t)).collect();
        // To find the face a new one lay in: with many new ones, sorted into
        // cells first.
        let new = self
            .triangles
            .iter()
            .filter(|&&t| !existed.contains(&key(t)))
            .count();
        let faces = (new > FEW).then(|| Cells::of_shapes(before.triangles()));

        self.colors = self
            .triangles
            .iter()
            .filter_map(|&t| {
                let k = key(t);
                let color = if existed.contains(&k) {
                    self.colors.get(&k).copied()
                } else {
                    let [a, b, c] = t.map(|v| self.vertices[v]);
                    let centre = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
                    // The first containing it, as `triangle_at` finds.
                    let lay_in = match &faces {
                        Some(faces) => faces
                            .around(centre, 0.0)
                            .filter(|&u| {
                                contains(before.triangles[u].map(|v| before.vertices[v]), centre)
                            })
                            .min(),
                        None => before.triangle_at(centre),
                    };
                    match lay_in {
                        Some(u) => before.color(u),
                        None => Some(DEFAULT_FACE),
                    }
                };
                Some((k, color?))
            })
            .collect();
    }

    /// Splits the triangle containing `at` (on its boundary counts) into
    /// three at a new vertex there. On an edge, one of the three is flat;
    /// normalizing drops it and splits the triangle across the edge.
    pub(super) fn insert_vertex(&mut self, at: Point) -> bool {
        let taken = self
            .used_vertices()
            .any(|v| self.vertices[v].distance(at) <= tolerance(0.0));
        let inside = |[a, b, c]: [Point; 3]| {
            [(a, b), (b, c), (c, a)]
                .into_iter()
                .all(|(u, v)| area2(u, v, at) >= -tolerance(u.distance(v)) * u.distance(v))
        };
        let Some(triangle) = self.triangles().position(inside).filter(|_| !taken) else {
            return false;
        };

        let [a, b, c] = self.triangles.remove(triangle);
        self.vertices.push(at);
        let d = self.last_vertex();
        self.triangles.extend([[a, b, d], [b, c, d], [c, a, d]]);
        true
    }

    /// Moves vertex `id` in a straight line to `to`, rearranging its
    /// triangles whenever one of them would fold over, i.e. when the vertex
    /// crosses the line through the opposite edge `u`–`w` of that triangle:
    ///
    /// - Through the edge itself, with a triangle on the other side: the edge
    ///   is flipped, so the vertex connects to that triangle's far corner.
    /// - Through the edge, into empty space: the triangle is dropped and the
    ///   vertex's other triangles stretch along with it. If it is the only
    ///   triangle holding the vertex, it turns over to the other side instead.
    /// - Past the end of the edge (say `u` ends up between the vertex and
    ///   `w`): the edge from the vertex to `w` runs through `u`, so it is
    ///   flipped the same way, or the (flat) triangle is dropped.
    ///
    /// A triangle that ends up flat, the vertex landing on the line through
    /// the opposite edge, isn't folded over: normalizing drops it and, if
    /// the vertex lands on an edge, splits that edge.
    ///
    /// Returns `false` if the rearranging doesn't settle.
    pub(super) fn move_vertex(&mut self, id: VertexId, to: Point) -> bool {
        const MAX_EVENTS: usize = 64;

        let mut from = self.vertices[id];

        for _ in 0..MAX_EVENTS {
            // The first triangle around `id` the move would flatten, as the
            // fraction of the remaining way at which that happens.
            let event = self
                .triangles
                .iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    let (u, w) = opposite(*t, id)?;
                    let (pu, pw) = (self.vertices[u], self.vertices[w]);
                    let (now, then) = (area2(pu, pw, from), area2(pu, pw, to));
                    let lands_on_line = is_flat([pu, pw, to]);
                    (then <= 0.0 && !lands_on_line)
                        .then(|| (i, (now / (now - then)).clamp(0.0, 1.0)))
                })
                .min_by(|x, y| x.1.total_cmp(&y.1));

            let Some((i, fraction)) = event else {
                self.vertices[id] = to;
                return true;
            };

            from = from + (to - from) * fraction;
            self.vertices[id] = from;
            self.unfold(i, id);
        }

        false
    }

    /// Resolves triangle `i`, flattened because vertex `id` lies on the line
    /// through its opposite edge. See [`Self::move_vertex`].
    pub(super) fn unfold(&mut self, i: TriangleId, id: VertexId) {
        let t = self.triangles[i];
        let (u, w) = opposite(t, id).expect("triangle contains the vertex");
        let (pu, pw, pv) = (self.vertices[u], self.vertices[w], self.vertices[id]);

        // Where the vertex is along u → w.
        let along = (pv - pu).x * (pw - pu).x + (pv - pu).y * (pw - pu).y;
        let along = along / (pw - pu).x.hypot((pw - pu).y).powi(2);

        // The edge running through the flattened triangle, and the corner
        // lying in the middle of it.
        let ((p, q), middle) = if along <= 0.0 {
            ((id, w), u)
        } else if along >= 1.0 {
            ((id, u), w)
        } else {
            ((u, w), id)
        };

        let across = self.triangles.iter().enumerate().find_map(|(j, other)| {
            (j != i && other.contains(&p) && other.contains(&q))
                .then(|| other.iter().copied().find(|&v| v != p && v != q))
                .flatten()
                .map(|x| (j, x))
        });

        let alone = !self.triangles.iter().enumerate().any(|(j, other)| {
            j != i && other.contains(&id) && (other.contains(&u) || other.contains(&w))
        });

        match across {
            // Flip: the two triangles on edge p–q become two triangles on
            // the edge from `middle` to the far corner `x`.
            Some((j, x)) => {
                self.triangles.remove(i.max(j));
                self.triangles.remove(i.min(j));
                self.push_triangle([p, middle, x]);
                self.push_triangle([middle, q, x]);
            }
            // Only this triangle holds the vertex: let it turn over. It is
            // flat right now, so its winding is set for the far side.
            None if alone => self.triangles[i] = [t[0], t[2], t[1]],
            None => {
                self.triangles.remove(i);
            }
        }
    }

    /// Makes the connectivity follow the geometry: welds vertices at the
    /// same spot (keeping one that wasn't `moved`), drops triangles that
    /// are flat, and splits edges that have a vertex lying on them (see
    /// [`Self::split_t_junctions`]). Records what it did in `changes`.
    ///
    /// Returns `false` if that doesn't settle.
    ///
    /// `before` is the (normalized) document the edit was made to: only what
    /// changed since needs checking against the rest.
    pub(super) fn normalize(
        &mut self,
        moved: &[VertexId],
        changes: &mut Changes,
        before: &Document,
    ) -> bool {
        let used = self.unique_vertices();
        let changed = Changed::since(self, before);
        // Sorted into cells only when many changed: for a few, going
        // through them all is quicker.
        let near = (changed.vertices.len() > MANY).then(|| Cells::of_points(&self.vertices, &used));

        for &u in &changed.vertices {
            let candidates: Vec<VertexId> = match &near {
                Some(near) => near.around(self.vertices[u], tolerance(0.0)).collect(),
                None => used.clone(),
            };
            for v in candidates {
                if u == v || self.vertices[u].distance(self.vertices[v]) > tolerance(0.0) {
                    continue;
                }
                // The later one goes, unless only the earlier one was moved.
                let (u, v) = (u.min(v), u.max(v));
                let (gone, kept) = if moved.contains(&u) && !moved.contains(&v) {
                    (u, v)
                } else {
                    (v, u)
                };
                if changes.welded.iter().any(|&(g, _)| g == gone || g == kept) {
                    continue; // Already welded away.
                }
                self.vertices[gone] = self.vertices[kept];
                for t in &mut self.triangles {
                    for c in t.iter_mut().filter(|c| **c == gone) {
                        *c = kept;
                    }
                }
                changes.welded.push((gone, kept));
            }
        }

        // Welding squashes the triangles that had both, and may leave
        // duplicates (two triangles folded onto each other).
        let mut seen: QuickSet<[VertexId; 3]> = QuickSet::default();
        let mut squashed = Vec::new();
        self.triangles.retain(|&t| {
            let key = key(t);
            if key[0] == key[1] || key[1] == key[2] {
                squashed.push(t);
                return false;
            }
            seen.insert(key)
        });
        changes.squashed.extend(squashed);

        self.split_t_junctions(changes, Some(&changed))
    }

    /// Splits every triangle that has a vertex lying in the middle of one of
    /// its edges (a "T-junction") into two, towards its opposite corner, so
    /// that vertex becomes a proper corner on both sides of the edge.
    ///
    /// (Nearly) flat triangles are removed first: they cover nothing, and in
    /// one a vertex can lie on two edges at once, which would split forever.
    /// The edges such a triangle joined are then repaired like any other.
    ///
    /// With `changed` (since a document without T-junctions), only what
    /// changed is checked: its edges against every vertex, and its vertices
    /// against every edge.
    ///
    /// Returns `false` if that doesn't settle.
    pub(super) fn split_t_junctions(
        &mut self,
        changes: &mut Changes,
        changed: Option<&Changed>,
    ) -> bool {
        let limit = 4 * (self.triangles.len() + self.vertices.len());

        // Splitting adds no vertices: what's used (and where) only changes
        // when flat triangles go.
        let mut used = Vec::new();
        let mut near = None;
        let mut stale = true;
        for _ in 0..limit {
            let vertices = &self.vertices;
            let count = self.triangles.len();
            self.triangles.retain(|&t| {
                let flat = is_flat(t.map(|v| vertices[v]));
                if flat {
                    changes.squashed.push(t);
                }
                !flat
            });
            if stale || self.triangles.len() != count {
                used = self.unique_vertices();
                // Sorted into cells only when many changed (or all count):
                // for a few, going through them is quicker.
                near = changed
                    .is_none_or(|changed| changed.vertices.len() > MANY)
                    .then(|| Cells::of_points(&self.vertices, &used));
                stale = false;
            }
            let vertices = &self.vertices;
            let junction = self.triangles.iter().enumerate().find_map(|(i, &t)| {
                // An unchanged triangle can only have gained a changed
                // vertex on its edges.
                let only_changed = changed.filter(|changed| !changed.triangle(t));
                (0..3).find_map(|k| {
                    let (u, v, x) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                    let (a, b) = (vertices[u], vertices[v]);
                    let candidates: Box<dyn Iterator<Item = VertexId>> = match (&near, only_changed)
                    {
                        (Some(near), _) => {
                            let reach = tolerance(a.distance(b));
                            let reach = Vector::new(reach, reach);
                            let min = Point::new(a.x.min(b.x), a.y.min(b.y)) - reach;
                            let max = Point::new(a.x.max(b.x), a.y.max(b.y)) + reach;
                            Box::new(near.within(min, max).filter(move |&w| {
                                only_changed.is_none_or(|changed| changed.vertex(w))
                            }))
                        }
                        (None, Some(changed)) => Box::new(changed.vertices.iter().copied()),
                        (None, None) => Box::new(used.iter().copied()),
                    };
                    // Quickly out of its bounds, then really on it.
                    let reach = tolerance(a.distance(b));
                    let (min, max) = (
                        Point::new(a.x.min(b.x) - reach, a.y.min(b.y) - reach),
                        Point::new(a.x.max(b.x) + reach, a.y.max(b.y) + reach),
                    );
                    candidates
                        .filter(|&w| {
                            let p = vertices[w];
                            min.x <= p.x && p.x <= max.x && min.y <= p.y && p.y <= max.y
                        })
                        .filter(|&w| !t.contains(&w) && is_inside_segment(vertices[w], a, b))
                        // The lowest, as going through them in order would.
                        .min()
                        .map(|w| (i, (u, v, w), [u, w, x], [w, v, x]))
                })
            });

            let Some((i, cut, first, second)) = junction else {
                return true;
            };

            changes.cut.push(cut);
            self.triangles.remove(i);
            self.push_triangle(first);
            self.push_triangle(second);
        }

        false
    }

    /// Adds a new triangle, wound positively.
    pub(super) fn push_triangle(&mut self, [a, b, c]: [VertexId; 3]) {
        let [pa, pb, pc] = [a, b, c].map(|id| self.vertices[id]);

        self.triangles.push(if area2(pa, pb, pc) < 0.0 {
            [a, c, b]
        } else {
            [a, b, c]
        });
    }
}
