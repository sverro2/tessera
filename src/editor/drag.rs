//! Following a drag: what it would do (creating, extending, moving,
//! transforming), snapped and kept valid, worked out on other threads
//! where that's slow.

use std::collections::HashMap;

use super::*;

/// An editor's view of the drawing, without what can't go to another
/// thread (its caches): to work out drags on several cores.
#[derive(Clone, Copy)]
pub(super) struct Worker<'a> {
    document: &'a Document,
    current: NodeId,
    show_edges: bool,
    mirrors: &'a [Mirror],
    shown: bool,
    camera: Camera,
    background: Option<&'a Background>,
    tool: Tool,
    proportional: Option<f32>,
    clipboard: Option<&'a Piece>,
    keys: Option<&'a Keymap>,
    grid: Grid,
    brush: crate::paint::Hsv,
    deadline: Option<std::time::Instant>,
}

/// What dragged points may snap onto, on screen (see
/// [`Editor::snap_targets`]).
pub(super) struct SnapTargets {
    vertices: Vec<(VertexId, Point)>,
    edges: Vec<(VertexId, VertexId, Point, Point)>,
    /// Each by the cells (squares as wide as grabbing reaches) it's in, to
    /// look only at those around a point: vertices, then edges.
    cells: HashMap<(i32, i32), (Vec<usize>, Vec<usize>)>,
    /// Edges to look at wherever: whole lines, and ones too long to sort.
    everywhere: Vec<usize>,
}

impl SnapTargets {
    /// How wide (screen px) a cell is: as far as anything snaps.
    const CELL: f32 = if VERTEX_HIT > EDGE_HIT {
        VERTEX_HIT
    } else {
        EDGE_HIT
    };
    /// How many cells an edge may cover before it's looked at wherever.
    const MAX_CELLS: i32 = 64;

    fn new(
        vertices: Vec<(VertexId, Point)>,
        edges: Vec<(VertexId, VertexId, Point, Point)>,
        lines: &[(VertexId, VertexId)],
    ) -> SnapTargets {
        let cell = |p: Point| {
            (
                (p.x / Self::CELL).floor() as i32,
                (p.y / Self::CELL).floor() as i32,
            )
        };
        let mut cells: HashMap<(i32, i32), (Vec<usize>, Vec<usize>)> = HashMap::new();
        let mut everywhere = Vec::new();
        for (i, &(_, p)) in vertices.iter().enumerate() {
            cells.entry(cell(p)).or_default().0.push(i);
        }
        for (i, &(u, v, p, q)) in edges.iter().enumerate() {
            let (x0, y0) = cell(Point::new(p.x.min(q.x), p.y.min(q.y)));
            let (x1, y1) = cell(Point::new(p.x.max(q.x), p.y.max(q.y)));
            if lines.contains(&(u, v)) || (x1 - x0 + 1) * (y1 - y0 + 1) > Self::MAX_CELLS {
                everywhere.push(i);
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    cells.entry((x, y)).or_default().1.push(i);
                }
            }
        }
        SnapTargets {
            vertices,
            edges,
            cells,
            everywhere,
        }
    }

    /// Those that may be in reach of `at` (screen): its vertices and its
    /// edges, each once.
    fn around(&self, at: Point) -> (Vec<usize>, Vec<usize>) {
        let (cx, cy) = (
            (at.x / Self::CELL).floor() as i32,
            (at.y / Self::CELL).floor() as i32,
        );
        let (mut vertices, mut edges) = (Vec::new(), self.everywhere.clone());
        for y in cy - 1..=cy + 1 {
            for x in cx - 1..=cx + 1 {
                if let Some((v, e)) = self.cells.get(&(x, y)) {
                    vertices.extend(v);
                    edges.extend(e);
                }
            }
        }
        vertices.sort_unstable();
        edges.sort_unstable();
        edges.dedup();
        (vertices, edges)
    }
}

impl<'a> Worker<'a> {
    /// An editor to work with, on this thread, drawing into `caches` (none
    /// is drawn).
    pub(super) fn editor(self, caches: &'a Caches) -> Editor<'a> {
        Editor {
            document: self.document,
            current: self.current,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            lit: Vec::new(),
            page: None,
            grid: self.grid,
            brush: self.brush,
            deadline: Cell::new(self.deadline),
        }
    }
}

impl Editor<'_> {
    /// Where a drag to `world` gets to, and what releasing there does. If
    /// nothing works out (in time), it stays where it was.
    pub(super) fn follow(&self, state: &mut State, world: Point) {
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        // With Shift, a dragged point lines up with what's around it; with
        // Ctrl, it lands on the grid (that first, with both).
        let shift = state.modifiers.shift();
        let grid = state.modifiers.command();
        let gridded = |state: &mut State, (found, fine, near): grid::Gridded| {
            (state.guides, state.near_guides) = (fine, near);
            found
        };
        state.guides.clear();
        state.near_guides.clear();
        let guided = |dragged: Dragged, guides: &mut Vec<Guide>, near: &mut Vec<Guide>| {
            if !shift {
                return None;
            }
            let found;
            (*guides, *near, found) = self.guided(dragged, world)?;
            found
        };
        match &mut state.interaction {
            Interaction::Creating { from, .. } => {
                let from = *from;
                // The corner dragged: with Ctrl on the grid; with Shift level
                // or upright with where it starts, or with the vertices there
                // are, and so on; else snapped.
                let (found, mut fine, mut near) = if grid {
                    self.grid_create(from, world, shift)
                } else if shift {
                    let (fine, near, found) = self.guided_creation(from, world);
                    (
                        found.or_else(|| self.create_snapped(from, world)),
                        fine,
                        near,
                    )
                } else {
                    (self.create_snapped(from, world), Vec::new(), Vec::new())
                };
                // And those its start lined up with.
                fine.extend(state.start_guides.0.iter().copied());
                near.extend(state.start_guides.1.iter().copied());
                (state.guides, state.near_guides) = (fine, near);
                let to = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
                state.interaction = Interaction::Creating { from, to };
            }
            Interaction::MovingVertex { id, .. } if grid => {
                let id = *id;
                if let Some((at, pending)) = gridded(state, self.grid_move(id, world, shift)) {
                    state.interaction = Interaction::MovingVertex { id, to: at };
                    state.pending = Some(pending);
                }
            }
            Interaction::MovingVertex { id, to } => {
                if let Some((at, pending)) = guided(
                    Dragged::Vertex(*id),
                    &mut state.guides,
                    &mut state.near_guides,
                ) {
                    *to = at;
                    state.pending = Some(pending);
                    self.deadline.set(None);
                    return;
                }
                let moved = match self.proportional {
                    Some(_) => self.move_proportionally(*id, world),
                    None => self.limit_move(*id, world),
                };
                if let Some((p, pending)) = moved {
                    *to = p;
                    state.pending = Some(pending);
                }
            }
            Interaction::Extending { a, b, start, .. } if grid => {
                let (a, b, start) = (*a, *b, *start);
                let found = gridded(state, self.grid_extend(a, b, start, world, shift));
                let apex = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
                state.interaction = Interaction::Extending { a, b, start, apex };
            }
            Interaction::Extending { a, b, start, apex } => {
                let dragged = Dragged::Apex {
                    a: *a,
                    b: *b,
                    start: *start,
                };
                if let Some((at, pending)) =
                    guided(dragged, &mut state.guides, &mut state.near_guides)
                {
                    *apex = at;
                    state.pending = Some(pending);
                    self.deadline.set(None);
                    return;
                }
                if let Some((p, pending)) = self.limit_extend(*a, *b, *start, world) {
                    *apex = p;
                    state.pending = pending;
                }
            }
            Interaction::LeavingFace {
                face,
                last,
                edge,
                apex,
            } => {
                let crossed = self.last_crossed(*last, world);
                *last = world;
                if crossed.is_some() {
                    *edge = crossed;
                    // What the previous edge would do is moot.
                    state.pending = None;
                }
                if self.document.triangle_at(world) == Some(*face) {
                    *edge = None;
                }
                match *edge {
                    Some((a, b, start)) if grid => {
                        let (found, fine, near) = self.grid_extend(a, b, start, world, shift);
                        (state.guides, state.near_guides) = (fine, near);
                        *apex = found.as_ref().map_or(world, |(at, _)| *at);
                        state.pending = found.map(|(_, pending)| pending);
                    }
                    Some((a, b, start)) => {
                        let dragged = Dragged::Apex { a, b, start };
                        if let Some((at, pending)) =
                            guided(dragged, &mut state.guides, &mut state.near_guides)
                        {
                            *apex = at;
                            state.pending = Some(pending);
                        } else if let Some((p, pending)) = self.limit_extend(a, b, start, world) {
                            *apex = p;
                            state.pending = pending;
                        }
                    }
                    None => {
                        *apex = world;
                        state.pending = None;
                    }
                }
            }
            Interaction::Pasting { base, from, to } if grid => {
                *to = world;
                let by = *base + (world - *from);
                state.pending = self
                    .clipboard
                    .and_then(|piece| self.grid_paste(piece, by, world));
            }
            Interaction::Pasting { base, from, to } => {
                *to = world;
                state.pending = self.clipboard.and_then(|piece| {
                    let by = *base + (world - *from);
                    // Snapped, so it joins up where it lands.
                    let moved = piece.moved(by);
                    self.snap_offsets(&moved.vertices, &|_| false)
                        .into_iter()
                        .chain([Vector::ZERO])
                        .find_map(|snap| {
                            let piece = piece.moved(by + snap);
                            self.check(vec![self.merging(Edit::Paste { piece })], world)
                        })
                });
            }
            Interaction::MovingSelection { from, to, .. } if grid => {
                let by = world - *from;
                if let Some(pending) = self.grid_move_selection(&state.selection, by, world) {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::MovingSelection { from, to, .. } => {
                let by = world - *from;
                // Snapped, so it joins up where it lands; not onto what
                // moves along.
                let moving: std::collections::HashSet<VertexId> = self
                    .weights(&state.selection)
                    .into_iter()
                    .map(|(v, _)| v)
                    .collect();
                let moved: Vec<Point> = state
                    .selection
                    .iter()
                    .map(|&v| self.document.vertex(v) + by)
                    .collect();
                let found = self
                    .snap_offsets(&moved, &|v| moving.contains(&v))
                    .into_iter()
                    .chain([Vector::ZERO])
                    .find_map(|snap| {
                        let by = by + snap;
                        self.transform(&state.selection, world, |p, w| p + by * w)
                    });
                if let Some(pending) = found {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::Extruding { from, to } if grid => {
                let (found, fine, near) = self.grid_extrude(&state.selection, *from, world, shift);
                (state.guides, state.near_guides) = (fine, near);
                *to = found.as_ref().map_or(world, |(at, _)| *at);
                state.pending = found.map(|(_, pending)| pending);
            }
            Interaction::Extruding { from, to } => {
                // With Shift, it lines up with what's around it.
                if shift {
                    let (fine, near, found) = self.guided_extrusion(&state.selection, *from, world);
                    (state.guides, state.near_guides) = (fine, near);
                    if let Some((at, pending)) = found {
                        *to = at;
                        state.pending = Some(pending);
                        self.deadline.set(None);
                        return;
                    }
                }
                let by = world - *from;
                // Snapped, so its far edges join up where they land; not
                // onto what it's extruded from.
                let selected: std::collections::HashSet<VertexId> =
                    state.selection.iter().copied().collect();
                let ends: Vec<Point> = self
                    .extruded_ends(&state.selection)
                    .into_iter()
                    .map(|v| self.document.vertex(v) + by)
                    .collect();
                *to = world;
                state.pending = self
                    .snap_offsets(&ends, &|v| selected.contains(&v))
                    .into_iter()
                    .chain([Vector::ZERO])
                    .find_map(|snap| self.extrude(&state.selection, by + snap, world));
            }
            Interaction::PivotingSelection {
                pivot,
                center,
                from,
                to,
            } => {
                let center = *center;
                // Turned by `turn`, scaled by `scale`, around the centre.
                let (turn, scale) = match pivot {
                    Pivot::Rotate => {
                        let angle = |p: Point| (p.y - center.y).atan2(p.x - center.x);
                        (angle(world) - angle(*from), 1.0)
                    }
                    Pivot::Scale => {
                        // Started right on the centre: nothing to scale by.
                        let start = from.distance(center);
                        if start < 1e-3 {
                            self.deadline.set(None);
                            return;
                        }
                        (0.0, world.distance(center) / start)
                    }
                };
                // Those proportionally edited, partly.
                let transform = |p: Point, w: f32| {
                    let (sin, cos) = (turn * w).sin_cos();
                    let d = (p - center) * (1.0 + (scale - 1.0) * w);
                    center + Vector::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
                };
                if let Some(pending) = self.transform(&state.selection, world, transform) {
                    *to = world;
                    state.pending = Some(pending);
                }
            }
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::Zooming { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. }
            | Interaction::Lassoing => {}
            Interaction::PlacingMirror {
                start,
                anchored,
                from,
                to,
            } => {
                let shift = state.modifiers.shift();
                // Shift: at a multiple of 15°; else onto a vertex in reach.
                let snapped = if shift {
                    None
                } else {
                    self.mirror_end(self.camera.to_screen(world))
                };
                let mut end = snapped.unwrap_or(world);
                if shift {
                    let d = world - *start;
                    let step = std::f32::consts::PI / 12.0;
                    let angle = (d.y.atan2(d.x) / step).round() * step;
                    let length = d.x.hypot(d.y);
                    end = *start + Vector::new(angle.cos(), angle.sin()) * length;
                }
                (*from, *to) = (*start, end);
                state.mirror_through = snapped;
                // Not on a vertex at its end: running right through one it
                // passes close by, so its image joins up there.
                if snapped.is_none()
                    && let Some((a, b, through)) =
                        self.snap_mirror(*start, end, *anchored && !shift)
                {
                    (*from, *to) = (a, b);
                    state.mirror_through = Some(through);
                }
                state.mirror_fits = self.fits(Mirror { a: *from, b: *to });
            }
        }
        self.deadline.set(None);
    }

    /// Where a point dragged to `screen` may snap, best first: vertices in
    /// grab range, then where edges in range cross the mirror line (if
    /// any), the closest points on edges in range, and on the mirror line. `skip_vertex`
    /// and `skip_edge` leave some out; the edges in `lines` count as the
    /// whole line through them.
    pub(super) fn snaps(
        &self,
        screen: Point,
        skip_vertex: impl Fn(VertexId) -> bool,
        skip_edge: impl Fn(VertexId, VertexId) -> bool,
        lines: &[(VertexId, VertexId)],
    ) -> Vec<(Target, Point)> {
        let targets = self.snap_targets((screen, screen), skip_vertex, skip_edge, lines);
        self.snaps_among(&targets, screen, lines)
    }

    /// What points within `min`–`max` (screen) may snap onto: the
    /// drawing's vertices and edges in reach of there (and the edges in
    /// `lines`, which count as whole lines), on screen, but those
    /// `skip_vertex` and `skip_edge` leave out. Gathered once for many
    /// points (a whole shape moved, say).
    pub(super) fn snap_targets(
        &self,
        (min, max): (Point, Point),
        skip_vertex: impl Fn(VertexId) -> bool,
        skip_edge: impl Fn(VertexId, VertexId) -> bool,
        lines: &[(VertexId, VertexId)],
    ) -> SnapTargets {
        let reach = VERTEX_HIT.max(EDGE_HIT);
        let (min, max) = (
            Point::new(min.x - reach, min.y - reach),
            Point::new(max.x + reach, max.y + reach),
        );
        let near = |p: Point, q: Point| {
            p.x.max(q.x) >= min.x
                && p.x.min(q.x) <= max.x
                && p.y.max(q.y) >= min.y
                && p.y.min(q.y) <= max.y
        };
        let at = |v| self.camera.to_screen(self.document.vertex(v));
        let vertices = self
            .document
            .unique_vertices()
            .into_iter()
            .filter(|&v| !skip_vertex(v))
            .map(|v| (v, at(v)))
            .filter(|&(_, p)| near(p, p))
            .collect();
        let edges = self
            .document
            .unique_edges()
            .into_iter()
            .filter(|&(u, v)| !skip_edge(u, v))
            .map(|(u, v)| (u, v, at(u), at(v)))
            .filter(|&(u, v, p, q)| near(p, q) || lines.contains(&(u, v)))
            .collect();
        SnapTargets::new(vertices, edges, lines)
    }

    /// As [`Self::snaps`], among `targets` (see [`Self::snap_targets`]).
    pub(super) fn snaps_among(
        &self,
        targets: &SnapTargets,
        screen: Point,
        lines: &[(VertexId, VertexId)],
    ) -> Vec<(Target, Point)> {
        let camera = self.camera;
        let at = |v| camera.to_screen(self.document.vertex(v));
        let (near_vertices, near_edges) = targets.around(screen);

        let mut vertices: Vec<_> = near_vertices
            .into_iter()
            .map(|i| targets.vertices[i])
            .map(|(v, p)| {
                (
                    Target::Vertex(v),
                    self.document.vertex(v),
                    p.distance(screen),
                )
            })
            .filter(|&(_, _, d)| d <= VERTEX_HIT)
            .collect();

        let mut edges: Vec<_> = near_edges
            .into_iter()
            .map(|i| targets.edges[i])
            .filter_map(|(u, v, pu, pv)| {
                let (closest, d) = if lines.contains(&(u, v)) {
                    closest_on_line(screen, pu, pv)
                } else {
                    closest_on_segment(screen, pu, pv)
                };
                (d <= EDGE_HIT).then(|| (Target::Edge(u, v), camera.to_world(closest), d))
            })
            .collect();

        // On the mirror line, and where edges cross it: what lands there
        // joins up with its mirror image.
        let mut crossings = Vec::new();
        let mut mirror = Vec::new();
        // The lines are where they are, not seen through a mirror image.
        let plain = self.plain_camera();
        for &line in self.mirrors {
            let (a, b) = (plain.to_screen(line.a), plain.to_screen(line.b));
            let (closest, d) = closest_on_line(screen, a, b);
            if d <= EDGE_HIT {
                mirror.push((Target::Mirror, camera.to_world(closest), d));
            }
            for &(target, _, _) in &edges {
                let Target::Edge(u, v) = target else {
                    continue;
                };
                let (pu, pv) = (at(u), at(v));
                let (su, sv) = (area2(a, b, pu), area2(a, b, pv));
                if su * sv >= 0.0 {
                    continue;
                }
                let crossing = pu + (pv - pu) * (su / (su - sv));
                let d = crossing.distance(screen);
                if d <= VERTEX_HIT {
                    crossings.push((target, camera.to_world(crossing), d));
                }
            }
        }

        vertices.sort_by(|x, y| x.2.total_cmp(&y.2));
        crossings.sort_by(|x, y| x.2.total_cmp(&y.2));
        edges.sort_by(|x, y| x.2.total_cmp(&y.2));
        vertices
            .into_iter()
            .chain(crossings)
            .chain(edges)
            .chain(mirror)
            .map(|(target, point, _)| (target, point))
            .collect()
    }

    /// What releasing `interaction` now would do. `None` if nothing.
    pub(super) fn pending(&self, interaction: Interaction) -> Option<Pending> {
        match interaction {
            Interaction::Idle
            | Interaction::Panning { .. }
            | Interaction::Zooming { .. }
            | Interaction::MovingBackground { .. }
            | Interaction::Painting { .. }
            | Interaction::Lassoing
            | Interaction::PlacingMirror { .. } => None,
            // Worked out as they move (see `follow`); not moved yet, nothing.
            Interaction::MovingSelection { .. }
            | Interaction::PivotingSelection { .. }
            | Interaction::Extruding { .. }
            | Interaction::Pasting { .. } => None,
            Interaction::Creating { from, to, .. } => self.create(from, to, to),
            Interaction::MovingVertex { id, to } => self.move_to(id, to),
            Interaction::LeavingFace { edge, apex, .. } => {
                let (a, b, start) = edge?;
                self.pending(Interaction::Extending { a, b, start, apex })
            }
            Interaction::Extending { a, b, start, apex } => {
                // Back on the source edge, the drag reads as cancelled.
                if self.is_near_edge(apex, a, b) {
                    return None;
                }
                self.extend(a, b, start, apex)
            }
        }
    }

    /// Applies `edits` (placing a point `at`) to a copy of the document, if
    /// the document accepts them (and they overlap no mirror image).
    pub(super) fn check(&self, edits: Vec<Edit>, at: Point) -> Option<Pending> {
        if self.out_of_time() {
            return None;
        }

        // The shape first: the paint only once it's taken (see below).
        let (result, changes) = self
            .document
            .preview_shape_until(&edits, self.deadline.get())?;
        // Merging nothing away, it's just the edit merged.
        let edits = match &edits[..] {
            [Edit::Merging { edit, .. }] if !changes.merged => vec![(**edit).clone()],
            _ => edits,
        };

        // Over a mirror line: it would overlap a mirror image.
        if !self.mirrors.is_empty() {
            let maps = doc::crossings(&doc::images(self.mirrors));
            if result.overlaps_under(Some(self.document), &maps) {
                return None;
            }
        }
        // Taken: now with its paint. One edit's is worked out on its shape;
        // several are made again, each painted after the one before.
        let (result, changes) = match &edits[..] {
            [edit] => {
                let mut result = result;
                self.document.paint_after(edit, &mut result);
                (result, changes)
            }
            _ => self.document.preview_until(&edits, None)?,
        };
        let added = new_triangles(self.document, &result, &|v| changes.kept(v)).collect();
        Some(Pending {
            edits,
            result,
            changes,
            added,
            at,
        })
    }

    /// `edit` (moving the selection, or pasting) merged into what it lands
    /// over, rather than turned down: what's moved comes in front, and
    /// what's under it is cut away round it (see [`Edit::Merging`]).
    pub(super) fn merging(&self, edit: Edit) -> Edit {
        self.merging_held(edit, &[])
    }

    /// As [`Self::merging`], moving `held` (the selection), which what's
    /// around only goes along with (editing proportionally): only faces of
    /// the selection's are carried whole.
    pub(super) fn merging_held(&self, edit: Edit, held: &[VertexId]) -> Edit {
        Edit::Merging {
            edit: Box::new(edit),
            snap: self.snap_distance(),
            held: held.to_vec(),
        }
    }

    /// How close (world units) a new triangle's corners and sides may come
    /// to existing geometry before counting as touching it: what a fill
    /// needs to leave no slivers.
    pub(super) fn snap_distance(&self) -> f32 {
        MIN_THICKNESS / self.camera.zoom
    }

    pub(super) fn out_of_time(&self) -> bool {
        self.deadline
            .get()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }

    pub(super) fn is_near_edge(&self, point: Point, a: VertexId, b: VertexId) -> bool {
        let screen = |v| self.camera.to_screen(self.document.vertex(v));
        closest_on_segment(self.camera.to_screen(point), screen(a), screen(b)).1 <= EDGE_HIT
    }

    /// Dragging vertex `id` to `to`: onto the best snap that works out (a
    /// vertex to weld with, an edge to join, or the line through one of its
    /// opposite edges), else to `to` itself, unless a vertex or edge is in
    /// reach: it joins up with that, or doesn't go there (rather than
    /// landing a hair off it).
    pub(super) fn move_to(&self, id: VertexId, to: Point) -> Option<Pending> {
        let lines: Vec<_> = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|&t| opposite(t, id))
            .map(|(u, v)| (u.min(v), u.max(v)))
            .collect();
        let snaps = self.snaps(
            self.camera.to_screen(to),
            |v| v == id,
            |u, v| u == id || v == id,
            &lines,
        );

        let joins = must_join(&snaps, &lines);
        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain((!joins).then_some(to))
            .find_map(|p| self.check(vec![Edit::MoveVertex { id, to: p }], p))
    }

    /// Creating a triangle dragged from `from` to `to` (placing a point
    /// `at`): equilateral, its third corner to the left as seen.
    pub(super) fn create(&self, from: Point, to: Point, at: Point) -> Option<Pending> {
        // Seen mirrored, the other way round, so it's still to the left as
        // seen.
        let flips = self.camera.image.is_some_and(Affine::flips);
        let corners = if flips {
            equilateral(to, from)
        } else {
            equilateral(from, to)
        };
        self.check(
            vec![Edit::AddTriangle {
                corners,
                snap: self.snap_distance(),
            }],
            at,
        )
    }

    /// Creating a triangle dragged from `from` to `to`, its dragged corner
    /// or its third one snapped onto the best snap that works out (a
    /// vertex, an edge), else as it is: where the drag ends up, and what
    /// that adds. `None` if nothing works out.
    pub(super) fn create_snapped(&self, from: Point, to: Point) -> Option<(Point, Pending)> {
        let third = self.third_corner(from, to);

        // Each snap as (onto a vertex, how far on screen, where the drag
        // ends up, where the point snapped lands).
        let mut found: Vec<(bool, f32, Point, Point)> = Vec::new();
        for (corner, dragged) in [(to, true), (third, false)] {
            let at = self.camera.to_screen(corner);
            for (target, p) in self.snaps(at, |_| false, |_, _| false, &[]) {
                let end = if dragged {
                    p
                } else {
                    self.dragged_corner(from, p)
                };
                let distance = self.camera.to_screen(p).distance(at);
                found.push((matches!(target, Target::Vertex(_)), distance, end, p));
            }
        }
        found.sort_by(|x, y| y.0.cmp(&x.0).then(x.1.total_cmp(&y.1)));

        found
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, _, end, p)| (end, p))
            .chain([(to, to)])
            .find_map(|(end, p)| Some((end, self.create(from, end, p)?)))
    }

    /// A new triangle's sixty degrees, as seen: turning its dragged corner
    /// by minus this around where it started gives its third corner (the
    /// other way round seen mirrored, so it's still to the left as seen).
    pub(super) fn sixty(&self) -> f32 {
        let flips = self.camera.image.is_some_and(Affine::flips);
        let sign = if flips { -1.0 } else { 1.0 };
        sign * std::f32::consts::FRAC_PI_3
    }

    /// The third corner of a new triangle dragged from `from` to `to`.
    pub(super) fn third_corner(&self, from: Point, to: Point) -> Point {
        from + turn(to - from, -self.sixty())
    }

    /// Where a new triangle started at `from` is dragged to for its third
    /// corner to be at `third`.
    pub(super) fn dragged_corner(&self, from: Point, third: Point) -> Point {
        from + turn(third - from, self.sixty())
    }

    /// Dragging from edge `a`–`b` (grabbed at `start`) to `apex`. On a side of
    /// the edge that has a triangle, split that triangle and then move the
    /// new vertex to `apex` like any dragged vertex, so it can go on to
    /// squash a child or rearrange the neighbours. On an empty side, add a
    /// triangle on the edge, its third corner snapped like a dragged vertex.
    pub(super) fn extend(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        apex: Point,
    ) -> Option<Pending> {
        if let Some(triangle) = self.triangle_beside(a, b, apex) {
            let Some((insert, document, id)) = self.split_from_edge(a, b, start, triangle) else {
                return self.check(vec![Edit::InsertVertex { at: apex }], apex);
            };
            let moved = self.with_document(&document).move_to(id, apex)?;
            let edits = [insert].into_iter().chain(moved.edits).collect();
            return self.check(edits, moved.at);
        }

        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        let source = (a.min(b), a.max(b));
        let snaps = self.snaps(
            self.camera.to_screen(apex),
            |v| v == a || v == b,
            |u, v| (u, v) == source,
            &[],
        );

        snaps
            .into_iter()
            .take(MAX_SNAPS)
            .map(|(_, p)| p)
            .chain([apex])
            .find_map(|p| {
                self.check(
                    vec![Edit::AddTriangle {
                        corners: [pa, pb, p],
                        snap: self.snap_distance(),
                    }],
                    p,
                )
            })
    }

    /// Dragging from edge `a`–`b` (grabbed at `start`) to exactly `apex` (on
    /// the grid, say): as [`Self::extend`], but snapping onto nothing.
    pub(super) fn extend_exact(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        apex: Point,
    ) -> Option<Pending> {
        if let Some(triangle) = self.triangle_beside(a, b, apex) {
            let Some((insert, document, id)) = self.split_from_edge(a, b, start, triangle) else {
                return self.check(vec![Edit::InsertVertex { at: apex }], apex);
            };
            let moved = self
                .with_document(&document)
                .check(vec![Edit::MoveVertex { id, to: apex }], apex)?;
            let edits = [insert].into_iter().chain(moved.edits).collect();
            return self.check(edits, apex);
        }
        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        self.check(
            vec![Edit::AddTriangle {
                corners: [pa, pb, apex],
                snap: self.snap_distance(),
            }],
            apex,
        )
    }

    /// Splits `triangle` (on edge `a`–`b`) at a point just inside it from
    /// `start` on the edge: where a drag into it begins. Returns the edit,
    /// the resulting document and the new vertex.
    pub(super) fn split_from_edge(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        triangle: TriangleId,
    ) -> Option<(Edit, Document, VertexId)> {
        let t = self.document.triangle_ids()[triangle];
        let far = self
            .document
            .vertex(*t.iter().find(|&&v| v != a && v != b)?);

        let inset = (MIN_THICKNESS / self.camera.zoom).min(start.distance(far) / 2.0);
        let at = start + (far - start) * (inset / start.distance(far));
        let insert = Edit::InsertVertex { at };

        let mut document = self.document.clone();
        document.apply(insert.clone()).then(|| {
            let id = document.last_vertex();
            (insert, document, id)
        })
    }

    /// This editor, looking at another document (e.g. a step ahead).
    pub(super) fn with_document<'b>(&'b self, document: &'b Document) -> Editor<'b> {
        Editor {
            document,
            current: self.current,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: Vec::new(),
            above: Vec::new(),
            camera: self.camera,
            caches: self.caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            lit: self.lit.clone(),
            page: self.page,
            grid: self.grid,
            brush: self.brush,
            deadline: self.deadline.clone(),
        }
    }

    /// The last edge the cursor crossed going from `from` to `to` (world),
    /// and where: its ends and the crossing point. `None` if it crossed
    /// none.
    pub(super) fn last_crossed(
        &self,
        from: Point,
        to: Point,
    ) -> Option<(VertexId, VertexId, Point)> {
        let d = to - from;
        self.document
            .unique_edges()
            .into_iter()
            .filter_map(|(a, b)| {
                let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
                let e = pb - pa;
                let den = cross(d, e);
                if den.abs() < 1e-9 {
                    return None;
                }
                let t = cross(pa - from, e) / den;
                let u = cross(pa - from, d) / den;
                ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u))
                    .then(|| (t, (a, b, pa + e * u)))
            })
            .max_by(|(t1, _), (t2, _)| t1.total_cmp(t2))
            .map(|(_, crossed)| crossed)
    }

    /// The triangle on edge `a`–`b` lying on the same side as `point`.
    pub(super) fn triangle_beside(
        &self,
        a: VertexId,
        b: VertexId,
        point: Point,
    ) -> Option<TriangleId> {
        let (pa, pb) = (self.document.vertex(a), self.document.vertex(b));
        let side = area2(pa, pb, point);

        self.document.triangle_ids().iter().position(|t| {
            let third = t.iter().find(|&&v| v != a && v != b);
            t.contains(&a)
                && t.contains(&b)
                && third.is_some_and(|&v| area2(pa, pb, self.document.vertex(v)) * side > 0.0)
        })
    }

    /// Where vertex `id` should be when the cursor is at `to`: the cursor itself if the move is allowed (the mesh
    /// rearranges itself around the vertex where needed), otherwise the
    /// closest allowed point, sliding along the edges around the vertex.
    /// That's the last resort, e.g. when a triangle would be swept over an
    /// unconnected part of the drawing.
    ///
    /// Returns the position and what releasing there would do; `None` if no
    /// valid position was found (in time).
    pub(super) fn limit_move(&self, id: VertexId, to: Point) -> Option<(Point, Pending)> {
        let camera = self.camera;
        let origin = camera.to_screen(self.document.vertex(id));

        // The vertex must stay on its own side of the opposite edge of each
        // of its triangles.
        let lines: Vec<Line> = self
            .document
            .triangle_ids()
            .iter()
            .filter_map(|t| {
                let i = t.iter().position(|&v| v == id)?;
                let a = camera.to_screen(self.document.vertex(t[(i + 1) % 3]));
                let b = camera.to_screen(self.document.vertex(t[(i + 2) % 3]));
                Some(offset_line(a, b, origin, SLIDE_INSET))
            })
            .collect();

        self.closest_valid(&lines, to, move |p| Interaction::MovingVertex { id, to: p })
    }

    /// Where the new corner of an extension from edge `a`–`b` should be when
    /// the cursor is at `to`. Into a triangle,
    /// the new vertex is limited like any dragged vertex; on an empty side it
    /// slides along the geometry a new triangle would overlap.
    pub(super) fn limit_extend(
        &self,
        a: VertexId,
        b: VertexId,
        start: Point,
        to: Point,
    ) -> Option<(Point, Option<Pending>)> {
        let camera = self.camera;
        let (pa, pb) = (
            camera.to_screen(self.document.vertex(a)),
            camera.to_screen(self.document.vertex(b)),
        );
        let cursor = camera.to_screen(to);

        // On the source edge the drag reads as cancelled, so don't hold the
        // corner back there.
        if closest_on_segment(cursor, pa, pb).1 <= EDGE_HIT {
            return Some((to, None));
        }

        let lines: Vec<Line> = match self.triangle_beside(a, b, to) {
            // Splitting, then moving the new vertex.
            Some(triangle) => {
                let apex = match self.split_from_edge(a, b, start, triangle) {
                    Some((_, document, id)) => self.with_document(&document).limit_move(id, to)?.0,
                    None => to,
                };
                let pending = self.pending(Interaction::Extending { a, b, start, apex })?;
                return Some((apex, Some(pending)));
            }
            // New triangle: the fill bends around other geometry rather than
            // leaving slivers, so just stay clear of the source edge.
            None => vec![offset_line(pa, pb, cursor, SLIDE_INSET)],
        };

        self.closest_valid(&lines, to, move |p| Interaction::Extending {
            a,
            b,
            start,
            apex: p,
        })
        .map(|(p, pending)| (p, Some(pending)))
    }

    /// The valid position closest to the cursor at `to` (world): the cursor
    /// itself, its projection onto one of `lines` (sliding along a limit) or
    /// where two of them cross (stuck in a corner), with what releasing the
    /// drag (`interaction` there) does. `None` if there's none (in time).
    ///
    /// The other positions are tried on all cores at once (nearest wins, as
    /// if tried in turn).
    pub(super) fn closest_valid(
        &self,
        lines: &[Line],
        to: Point,
        interaction: impl Fn(Point) -> Interaction + Sync,
    ) -> Option<(Point, Pending)> {
        if let Some(pending) = self.pending(interaction(to)) {
            return Some((to, pending));
        }

        let camera = self.camera;
        let cursor = camera.to_screen(to);

        let projections = lines.iter().map(|&(p, d)| p + d * dot(cursor - p, d));
        let corners = lines.iter().enumerate().flat_map(|(i, &(p1, d1))| {
            lines[i + 1..].iter().filter_map(move |&(p2, d2)| {
                let den = cross(d1, d2);
                (den.abs() > 1e-6).then(|| p1 + d1 * (cross(p2 - p1, d2) / den))
            })
        });

        let mut candidates: Vec<_> = projections
            .chain(corners)
            .map(|p| (camera.to_world(p), p.distance(cursor)))
            .collect();
        candidates.sort_by(|x, y| x.1.total_cmp(&y.1));

        let worker = self.worker();
        candidates.par_iter().find_map_first(|&(p, _)| {
            let caches = Caches::default();
            let editor = worker.editor(&caches);
            if editor.out_of_time() {
                return None;
            }
            editor.pending(interaction(p)).map(|pending| (p, pending))
        })
    }

    /// What working out drags on another thread needs of this editor.
    pub(super) fn worker(&self) -> Worker<'_> {
        Worker {
            document: self.document,
            current: self.current,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            camera: self.camera,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            grid: self.grid,
            brush: self.brush,
            deadline: self.deadline.get(),
        }
    }
}

/// A line in screen space: a point on it and its unit direction.
pub(super) type Line = (Point, Vector);

/// The line through `a` and `b`, shifted `offset` px towards `towards`
/// (away from it if negative).
pub(super) fn offset_line(a: Point, b: Point, towards: Point, offset: f32) -> Line {
    let dir = (b - a) * (1.0 / a.distance(b));
    let mut normal = Vector::new(-dir.y, dir.x);
    if dot(normal, towards - a) < 0.0 {
        normal *= -1.0;
    }
    (a + normal * offset, dir)
}

pub(super) fn longest_edge([a, b, c]: [Point; 3]) -> (Point, Point) {
    [(a, b), (b, c), (c, a)]
        .into_iter()
        .max_by(|x, y| x.0.distance(x.1).total_cmp(&y.0.distance(y.1)))
        .unwrap()
}

/// `v` turned by `angle` (radians).
pub(super) fn turn(v: Vector, angle: f32) -> Vector {
    let (sin, cos) = angle.sin_cos();
    Vector::new(v.x * cos - v.y * sin, v.x * sin + v.y * cos)
}

pub(super) fn dot(a: Vector, b: Vector) -> f32 {
    a.x * b.x + a.y * b.y
}

pub(super) fn cross(a: Vector, b: Vector) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Triangles in `after` that `before` doesn't have (compared by corner ids,
/// through `same` for vertices that were welded).
pub(super) fn new_triangles<'a>(
    before: &'a Document,
    after: &'a Document,
    same: &'a impl Fn(VertexId) -> VertexId,
) -> impl Iterator<Item = [VertexId; 3]> + 'a {
    let key = move |t: &[VertexId; 3]| {
        let mut t = t.map(same);
        t.sort_unstable();
        t
    };
    let existing: std::collections::HashSet<_> = before.triangle_ids().iter().map(key).collect();

    after
        .triangle_ids()
        .iter()
        .filter(move |t| !existing.contains(&key(t)))
        .copied()
}

/// Equilateral triangle with base `from`–`to`, apex to the left of the drag.
pub(super) fn equilateral(from: Point, to: Point) -> [Point; 3] {
    let d = to - from;
    let mid = Point::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let h = 3f32.sqrt() / 2.0;
    [from, to, mid + Vector::new(d.y * h, -d.x * h)]
}

/// Whether among `snaps` there's something a dragged point must join up
/// with, rather than land a hair off: a vertex, or an edge (not one of
/// `lines`, which only line it up).
pub(super) fn must_join(snaps: &[(Target, Point)], lines: &[(VertexId, VertexId)]) -> bool {
    snaps.iter().any(|(target, _)| match *target {
        Target::Vertex(_) => true,
        Target::Edge(u, v) => !lines.contains(&(u, v)),
        Target::Mirror => false,
    })
}
