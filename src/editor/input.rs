//! What the editor does with input: keys pressed and let go, the mouse
//! pressed, moved and released, the wheel, and frames while something
//! animates; and framing a shape (/).

use super::*;

impl Editor<'_> {
    /// The shape to frame (world, as seen: with its mirror images), the
    /// layer it's on if not this one, and whether it's one pointed at: the
    /// selection; else the frontmost shape under the cursor (at `inside`),
    /// on whichever layer in view, as they're drawn: painted, in their
    /// order (one on a layer in front over this layer's behind it);
    /// otherwise this layer over all; else (not pointed at) the one last
    /// highlighted; else all of the layer.
    pub(super) fn shape_to_frame(
        &self,
        state: &State,
        inside: Option<Point>,
    ) -> (Vec<Point>, Option<NodeId>, bool) {
        let document = self.document;
        let hover = inside
            .filter(|_| self.shown)
            .and_then(|pos| self.hit_test(pos));
        if state.selection.is_empty()
            && let Some(pos) = inside
        {
            let elsewhere = |layers: &[SceneLayer]| self.shape_elsewhere(layers, pos);
            let found = match hover {
                Some(_) if self.look() != Look::Painted => None,
                Some(_) => elsewhere(&self.above),
                None => elsewhere(&self.above).or_else(|| elsewhere(&self.below)),
            };
            if let Some((points, layer)) = found {
                return (points, Some(layer), true);
            }
        }
        let pointed = !state.selection.is_empty() || hover.is_some();
        let vertices: Vec<VertexId> = if !state.selection.is_empty() {
            state.selection.clone()
        } else if let Some(hover) = hover {
            document
                .connected(self.hover_vertex(hover))
                .into_iter()
                .flatten()
                .collect()
        } else if let Some(shape) = state
            .shape
            .as_ref()
            .filter(|shape| shape.revision == document.revision())
        {
            shape.triangles.iter().flatten().copied().collect()
        } else {
            document.unique_vertices()
        };
        (as_seen(document, &vertices, self.mirrors), None, pointed)
    }

    /// The shape of one of `layers` (others in view, back to front) under
    /// `screen`, frontmost first (as the pipette picks), as
    /// [`Self::shape_to_frame`] has it, and that layer.
    pub(super) fn shape_elsewhere(
        &self,
        layers: &[SceneLayer],
        screen: Point,
    ) -> Option<(Vec<Point>, NodeId)> {
        // The other layers as they are, even working through the current
        // layer's mirror image.
        let world = self.plain_camera().to_world(screen);
        layers.iter().rev().find_map(|layer| {
            let document = layer.document;
            let t = document.triangle_at(world)?;
            let shape = document.connected(document.triangle_ids()[t][0]);
            let vertices: Vec<VertexId> = shape.into_iter().flatten().collect();
            Some((as_seen(document, &vertices, layer.mirrors), layer.id))
        })
    }

    /// Keeping up with what changed since the last event: the shapes
    /// highlighted after an edit, what's moved after proportional editing
    /// changed, the selection (only the shape mode's, of vertices still
    /// there).
    pub(super) fn keep_up(&self, state: &mut State) {
        // After an edit, the highlighted shapes may have grown or shrunk;
        // gone, if what they were found from is (undone, say, or another
        // layer or drawing now).
        let revision = self.document.revision();
        let mut used = None;
        let previous = state.fading.as_mut().map(|(previous, _)| previous);
        for slot in std::iter::once(&mut state.shape).chain(previous) {
            if let Some(shape) = slot.as_ref()
                && shape.revision != revision
            {
                let used = used.get_or_insert_with(|| self.document.unique_vertices());
                *slot = used
                    .binary_search(&shape.start)
                    .is_ok()
                    .then(|| self.highlight(shape.start));
            }
        }
        // Proportional editing changed (reaching further, say, or zoomed):
        // what's being moved is worked out again.
        let proportional = self.proportional.map(|reach| (reach, self.camera.zoom));
        if state.proportional != proportional {
            state.proportional = proportional;
            if let Some(to) = state.interaction.selection_to() {
                state.aim.get_or_insert(to);
            }
        }
        // The selection is only the shape mode's, and only of vertices still
        // there (not welded away, say, or undone).
        if self.tool != Tool::Shape || !self.shown {
            state.selection.clear();
            state.lasso.clear();
            state.lasso_armed = false;
            if matches!(
                state.interaction,
                Interaction::Lassoing
                    | Interaction::MovingSelection { .. }
                    | Interaction::PivotingSelection { .. }
                    | Interaction::Extruding { .. }
            ) {
                state.interaction = Interaction::Idle;
                state.pending = None;
            }
        } else if state.selected_layer != self.current {
            // Another layer: its vertices are others.
            state.selection.clear();
            state.selected_layer = self.current;
        } else if state.selected_in != revision {
            let used = self.document.unique_vertices();
            state.selection.retain(|v| used.binary_search(v).is_ok());
            state.selected_in = revision;
        }
    }

    /// A key pressed or let go, or the modifiers held changing; `inside`
    /// and `screen` are where the cursor is (see `update`).
    pub(super) fn on_key(
        &self,
        state: &mut State,
        event: &keyboard::Event,
        inside: Option<Point>,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        match event {
            keyboard::Event::ModifiersChanged(modifiers) => {
                self.on_modifiers(state, *modifiers, screen)
            }
            keyboard::Event::KeyReleased {
                key, physical_key, ..
            } => self.on_key_released(state, key, *physical_key, screen),
            keyboard::Event::KeyPressed {
                key,
                physical_key,
                modifiers,
                repeat,
                ..
            } => {
                if *key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && let Some(action) = self.on_escape(state)
                {
                    return Some(action);
                }
                // What it does here, as the user has it.
                let action = self.pressed(key, *physical_key, *modifiers)?;
                self.on_action(state, action, *repeat, inside, screen)
            }
        }
    }

    /// The modifiers held changed: in the shape mode mid-drag, Shift or
    /// Ctrl down or up turns guides or the grid on or off; painting, Ctrl
    /// turns the brush into the pipette.
    fn on_modifiers(
        &self,
        state: &mut State,
        modifiers: keyboard::Modifiers,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        state.modifiers = modifiers;
        if self.tool == Tool::Shape {
            if matches!(
                state.interaction,
                Interaction::Creating { .. }
                    | Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
                    | Interaction::Extruding { .. }
                    | Interaction::MovingSelection { .. }
                    | Interaction::Pasting { .. }
            ) && state.tweaking.is_none()
                && let Some(pos) = screen
            {
                state.aim = Some(self.camera.to_world(pos));
            }
            return Some(canvas::Action::request_redraw());
        }
        matches!(self.tool, Tool::Paint { .. }).then(canvas::Action::request_redraw)
    }

    /// A key let go: the one held to tweak something (see `tweak`), done.
    fn on_key_released(
        &self,
        state: &mut State,
        key: &keyboard::Key,
        physical_key: keyboard::key::Physical,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        let tweak = state.tweaking?;
        let action = match tweak.what {
            Tweaking::Lightness => Action::Lightness,
            Tweaking::Opacity => Action::Opacity,
            Tweaking::Width => Action::Width,
            Tweaking::Reach => Action::Reach,
            Tweaking::LoopTurn => Action::EdgeLoop,
        };
        let chord = Chord::pressed(key, physical_key, keyboard::Modifiers::empty())?;
        if !self.keys?.releases(&chord, action) {
            return None;
        }
        state.tweaking = None;
        // Done reaching: what's being moved goes on from where the cursor
        // is.
        if tweak.what == Tweaking::Reach
            && let Some(pos) = screen
        {
            rebase(&mut state.interaction, self.camera.to_world(pos));
        }
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// Esc lets go of what's going on with the selection (grabbing,
    /// rotating... or pasting); else of the lasso made ready; else of the
    /// selection. Nothing to let go of: nothing.
    fn on_escape(&self, state: &mut State) -> Option<canvas::Action<Message>> {
        let pasting = matches!(state.interaction, Interaction::Pasting { .. });
        if state.selection.is_empty() && !state.lasso_armed && !pasting {
            return None;
        }
        match state.interaction {
            Interaction::Idle if state.lasso_armed => state.lasso_armed = false,
            Interaction::Idle => state.selection.clear(),
            _ => {
                state.interaction = Interaction::Idle;
                state.pending = None;
                state.aim = None;
            }
        }
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// What a key does here (`repeat`: held, coming again).
    fn on_action(
        &self,
        state: &mut State,
        action: Action,
        repeat: bool,
        inside: Option<Point>,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        match action {
            Action::Reach | Action::Lightness | Action::Opacity | Action::Width => {
                self.tweak(state, action, repeat, inside, screen)
            }
            Action::Copy | Action::CutFaces | Action::Paste if self.shaping(state) => {
                self.clipboard_key(state, action, inside)
            }
            Action::FrameShape if matches!(state.interaction, Interaction::Idle) => {
                let (points, layer, pointed) = self.shape_to_frame(state, inside);
                (!points.is_empty() && (self.shown || layer.is_some())).then(|| {
                    canvas::Action::publish(Message::Frame {
                        points,
                        layer,
                        pointed,
                    })
                    .and_capture()
                })
            }
            // Lasso (Q, or Shift+Q to take out of the selection): the next
            // drag draws one (the same again: not).
            Action::Lasso | Action::LassoRemove if !repeat && self.shaping(state) => {
                let removes = action == Action::LassoRemove;
                state.lasso_armed = !(state.lasso_armed && state.lasso_removes == removes);
                state.lasso_removes = removes;
                Some(canvas::Action::request_redraw().and_capture())
            }
            // Select all (Ctrl+A): every vertex of the layer.
            Action::SelectAll if self.shaping(state) => {
                state.selection = self.document.unique_vertices();
                state.selected_in = self.document.revision();
                Some(canvas::Action::request_redraw().and_capture())
            }
            Action::SelectShape if self.shaping(state) => self.select_shape(state, inside),
            Action::EdgeLoop if self.shaping(state) => self.edge_loop_key(state, repeat, inside),
            Action::Extrude if state.selection.is_empty() => {
                if !self.shaping(state) {
                    return None;
                }
                self.extrude_edge(state, inside?)
            }
            Action::Grab | Action::Rotate | Action::Scale | Action::Extrude
                if !state.selection.is_empty() && self.tool == Tool::Shape =>
            {
                self.transform_key(state, action, inside)
            }
            Action::CutEdge | Action::Delete if self.shaping(state) => {
                self.remove_key(state, action, inside?)
            }
            _ => None,
        }
    }

    /// Whether the shape mode's keys work now: shaping a layer that's
    /// shown, nothing going on.
    fn shaping(&self, state: &State) -> bool {
        self.tool == Tool::Shape && self.shown && matches!(state.interaction, Interaction::Idle)
    }

    /// Holding a key tweaks something by moving the mouse: editing
    /// proportionally, the reach key (Shift+O) how far that reaches (what's
    /// being moved stays meanwhile); painting with a colour, the lightness
    /// key (C) how light it is; painting edges, the width key (W) how wide
    /// (the face or edge previewed stays meanwhile).
    fn tweak(
        &self,
        state: &mut State,
        action: Action,
        repeat: bool,
        inside: Option<Point>,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        let (what, at) = match action {
            Action::Reach if self.tool == Tool::Shape && self.proportional.is_some() => {
                (Tweaking::Reach, screen)
            }
            Action::Lightness
                if matches!(
                    self.tool,
                    Tool::Paint {
                        brush: Brush::Color(_),
                        ..
                    }
                ) =>
            {
                (Tweaking::Lightness, inside)
            }
            Action::Opacity
                if matches!(
                    self.tool,
                    Tool::Paint {
                        brush: Brush::Color(_),
                        ..
                    }
                ) =>
            {
                (Tweaking::Opacity, inside)
            }
            Action::Width if self.paints_edges() => (Tweaking::Width, inside),
            _ => return None,
        };
        if !repeat && state.tweaking.is_none() {
            let at = at?;
            state.tweaking = Some(Tweak { what, at, last: at });
        }
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// Copy (Ctrl+C) copies the selected faces, cut (Ctrl+X) cuts them;
    /// paste (Ctrl+V) picks up a copy of what was copied, to put down with
    /// a click.
    fn clipboard_key(
        &self,
        state: &mut State,
        action: Action,
        inside: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        match action {
            Action::Copy => {
                let piece = self.document.piece(&state.selection);
                (!piece.is_empty())
                    .then(|| canvas::Action::publish(Message::Copy(piece)).and_capture())
            }
            Action::CutFaces => {
                let piece = self.document.piece(&state.selection);
                if piece.is_empty() {
                    return None;
                }
                // The faces copied: those with all corners selected (last
                // first, so the others keep their places).
                let edits = self
                    .document
                    .triangle_ids()
                    .iter()
                    .enumerate()
                    .rev()
                    .filter(|(_, t)| t.iter().all(|v| state.selection.contains(v)))
                    .map(|(triangle, _)| Edit::RemoveTriangle { triangle })
                    .collect();
                let revision = self.document.revision();
                Some(
                    canvas::Action::publish(Message::Cut {
                        piece,
                        edits,
                        revision,
                    })
                    .and_capture(),
                )
            }
            _ => self.paste(state, inside?),
        }
    }

    /// Picks up a copy of what was copied, the cursor at `inside`: right
    /// where it was copied from, if it fits there (on another layer, say);
    /// else in the middle of the cursor.
    fn paste(&self, state: &mut State, inside: Point) -> Option<canvas::Action<Message>> {
        let piece = self.clipboard.filter(|piece| !piece.is_empty())?;
        let at = self.camera.to_world(inside);
        let paste = |by: Vector| {
            self.check(
                vec![self.merging(Edit::Paste {
                    piece: piece.moved(by),
                })],
                at,
            )
        };
        let (base, pending) = match paste(Vector::ZERO) {
            Some(pending) => (Vector::ZERO, Some(pending)),
            None => {
                let base = at - piece.centre();
                (base, paste(base))
            }
        };
        state.selection.clear();
        state.interaction = Interaction::Pasting {
            base,
            from: at,
            to: at,
        };
        state.pending = pending;
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// Select shape (Ctrl+L) selects the whole shape under the cursor; off
    /// the drawing, the whole shapes the selection is in.
    fn select_shape(
        &self,
        state: &mut State,
        inside: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        let hovered = inside
            .and_then(|pos| self.hit_test(pos))
            .map(|hover| vec![self.hover_vertex(hover)]);
        let from = hovered.unwrap_or_else(|| state.selection.clone());
        let mut linked = Vec::new();
        for v in from {
            if !linked.contains(&v) {
                linked.extend(self.document.connected(v).into_iter().flatten());
                linked.sort_unstable();
                linked.dedup();
            }
        }
        for v in linked {
            if !state.selection.contains(&v) {
                state.selection.push(v);
            }
        }
        state.selected_in = self.document.revision();
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// Shift+Q over an edge selects its edge loop (in place of the
    /// selection); held (over an edge, or with a loop selected), moving
    /// across tweaks how far the loop may turn at a vertex, the loop found
    /// again from its edge as it goes.
    fn edge_loop_key(
        &self,
        state: &mut State,
        repeat: bool,
        inside: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        if repeat {
            return state
                .tweaking
                .map(|_| canvas::Action::request_redraw().and_capture());
        }
        let at = inside?;
        match self.hit_test(at) {
            Some(Hover::Edge { a, b, .. }) => {
                state.edge_loop = Some((a, b, self.document.revision()));
                self.select_edge_loop(state);
            }
            _ if self.edge_loop_selected(state) => {}
            _ => return None,
        }
        state.tweaking = Some(Tweak {
            what: Tweaking::LoopTurn,
            at,
            last: at,
        });
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// How far an edge loop may turn at a vertex (radians).
    pub(super) fn loop_turn(&self, state: &State) -> f32 {
        state.loop_turn.unwrap_or(LOOP_TURN)
    }

    /// The vertices of the edge loop last found (Shift+Q), as far as it
    /// may turn now; none if the drawing changed since.
    fn edge_loop_vertices(&self, state: &State) -> Option<Vec<VertexId>> {
        let (a, b, revision) = state.edge_loop?;
        if revision != self.document.revision() {
            return None;
        }
        let mut vertices: Vec<VertexId> = self
            .document
            .edge_loop(a, b, self.loop_turn(state))
            .into_iter()
            .flat_map(|(u, v)| [u, v])
            .collect();
        vertices.sort_unstable();
        vertices.dedup();
        Some(vertices)
    }

    /// Whether the selection is still the edge loop last found (not
    /// changed by hand since, nor the drawing).
    fn edge_loop_selected(&self, state: &State) -> bool {
        state.selected_in == self.document.revision()
            && self.edge_loop_vertices(state).is_some_and(|vertices| {
                let mut selection = state.selection.clone();
                selection.sort_unstable();
                selection == vertices
            })
    }

    /// Selects the edge loop from the edge it was found from, as far as it
    /// may turn now.
    fn select_edge_loop(&self, state: &mut State) {
        match self.edge_loop_vertices(state) {
            Some(vertices) => {
                state.selection = vertices;
                state.selected_in = self.document.revision();
            }
            None => state.edge_loop = None,
        }
    }

    /// Without a selection, extrude (E) over an outer edge extrudes it, as
    /// if its ends were selected.
    fn extrude_edge(&self, state: &mut State, inside: Point) -> Option<canvas::Action<Message>> {
        let Hover::Edge { a, b, .. } = self.hit_test(inside)? else {
            return None;
        };
        if self.extruded_ends(&[a, b]).is_empty() {
            return None;
        }
        let at = self.camera.to_world(inside);
        state.selection = vec![a, b];
        state.selected_in = self.document.revision();
        state.interaction = Interaction::Extruding { from: at, to: at };
        state.pending = None;
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// With a selection: grab (G), rotate (R), scale (T) or extrude (E) it,
    /// following the cursor from `inside`. Not while something else is
    /// going on.
    fn transform_key(
        &self,
        state: &mut State,
        action: Action,
        inside: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        if !matches!(state.interaction, Interaction::Idle) {
            return None;
        }
        let at = self.camera.to_world(inside?);
        let center = self.centre(&state.selection);
        let pivot = |pivot| Interaction::PivotingSelection {
            pivot,
            center,
            from: at,
            to: at,
        };
        state.interaction = match action {
            Action::Grab => Interaction::MovingSelection {
                from: at,
                to: at,
                held: false,
            },
            Action::Rotate => pivot(Pivot::Rotate),
            Action::Extrude => Interaction::Extruding { from: at, to: at },
            _ => pivot(Pivot::Scale),
        };
        state.pending = None;
        Some(canvas::Action::request_redraw().and_capture())
    }

    /// Cutting (C) subdivides the selection: every edge with both ends
    /// selected split at its middle, the faces on them with it, the new
    /// vertices selected too (to go on); else it cuts the edge under the
    /// cursor (at `inside`) where it is. Deleting (D) removes the selected
    /// vertices: every triangle using one; else the triangle under the
    /// cursor.
    fn remove_key(
        &self,
        state: &mut State,
        action: Action,
        inside: Point,
    ) -> Option<canvas::Action<Message>> {
        let edits = match action {
            Action::CutEdge if !state.selection.is_empty() => {
                let selected: std::collections::HashSet<VertexId> =
                    state.selection.iter().copied().collect();
                let edges: Vec<(VertexId, VertexId)> = self
                    .document
                    .unique_edges()
                    .into_iter()
                    .filter(|(a, b)| selected.contains(a) && selected.contains(b))
                    .collect();
                if edges.is_empty() {
                    return None;
                }
                let pending = self.check(
                    vec![Edit::Subdivide { edges }],
                    self.camera.to_world(inside),
                )?;
                let n = self.document.vertex_slots();
                state.selection.extend(
                    pending
                        .result
                        .unique_vertices()
                        .into_iter()
                        .filter(|&v| v >= n),
                );
                let revision = self.document.revision();
                return Some(
                    canvas::Action::publish(Message::Edit {
                        edits: pending.edits,
                        revision,
                    })
                    .and_capture(),
                );
            }
            Action::CutEdge => match self.hit_test(inside)? {
                Hover::Edge { at, .. } => vec![Edit::InsertVertex { at }],
                Hover::Vertex(_) | Hover::Face(_) => return None,
            },
            // Last first, so the others keep their places.
            _ if !state.selection.is_empty() => {
                let edits: Vec<_> = self
                    .document
                    .triangle_ids()
                    .iter()
                    .enumerate()
                    .rev()
                    .filter(|(_, t)| t.iter().any(|v| state.selection.contains(v)))
                    .map(|(triangle, _)| Edit::RemoveTriangle { triangle })
                    .collect();
                if edits.is_empty() {
                    return None;
                }
                edits
            }
            _ => vec![Edit::RemoveTriangle {
                triangle: self.document.triangle_at(self.camera.to_world(inside))?,
            }],
        };
        let edits = self.check(edits, self.camera.to_world(inside))?.edits;
        let revision = self.document.revision();
        Some(canvas::Action::publish(Message::Edit { edits, revision }).and_capture())
    }

    /// A mouse button pressed, the cursor at `pos` (screen).
    pub(super) fn on_press(
        &self,
        state: &mut State,
        button: mouse::Button,
        pos: Point,
    ) -> Option<canvas::Action<Message>> {
        // Guides are a drag's own: none from the one before.
        state.guides.clear();
        state.near_guides.clear();
        let world = self.camera.to_world(pos);
        if button == mouse::Button::Left && !self.editable() {
            return None;
        }
        // Mid-drag, a right click cancels it: all is as it was, and letting
        // go of the left button then does nothing.
        if button == mouse::Button::Right
            && matches!(
                state.interaction,
                Interaction::Creating { .. }
                    | Interaction::MovingVertex { .. }
                    | Interaction::Extending { .. }
                    | Interaction::LeavingFace { .. }
                    | Interaction::MovingSelection { held: true, .. }
                    | Interaction::Lassoing
                    | Interaction::PlacingMirror { .. }
                    | Interaction::Painting { .. }
            )
        {
            state.interaction = Interaction::Idle;
            state.pending = None;
            state.aim = None;
            state.lasso.clear();
            state.stroke.clear();
            state.edge_stroke.clear();
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // Grabbed, rotating or scaling, the selection follows the cursor: a
        // click applies that; a right click cancels it.
        if state.interaction.is_transforming() {
            if let Some(world) = state.aim.take() {
                self.follow(state, world);
            }
            let pasting = matches!(state.interaction, Interaction::Pasting { .. });
            let extruding = matches!(state.interaction, Interaction::Extruding { .. });
            // Pasting, a click where it doesn't fit does nothing.
            if pasting && button == mouse::Button::Left && state.pending.is_none() {
                return Some(canvas::Action::request_redraw().and_capture());
            }
            let pending = state.pending.take();
            state.interaction = Interaction::Idle;
            let action = match (button, pending) {
                (mouse::Button::Left, Some(pending)) => {
                    // What was pasted is selected, to go on with.
                    if pasting {
                        let n = self.document.vertex_slots();
                        let mut pasted: Vec<VertexId> = pending
                            .result
                            .unique_vertices()
                            .into_iter()
                            .filter(|&v| v >= n)
                            .collect();
                        pasted.extend(pending.changes.welded.iter().map(|&(_, kept)| kept));
                        pasted.sort_unstable();
                        pasted.dedup();
                        state.selection = pasted;
                        state.selected_in = self.document.revision();
                    }
                    // What was extruded: its far edges, to go on from.
                    if extruding {
                        state.selection = self.extruded(&state.selection, &pending);
                        state.selected_in = self.document.revision();
                    }
                    canvas::Action::publish(Message::Edit {
                        edits: pending.edits,
                        revision: self.document.revision(),
                    })
                }
                _ => canvas::Action::request_redraw(),
            };
            return Some(action.and_capture());
        }
        // The lasso Q made ready: a right click lets go of it.
        if button == mouse::Button::Right && state.lasso_armed {
            state.lasso_armed = false;
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // Shift+click adds a vertex to the selection, or removes it; with a
        // selection, it does nothing else. (Elsewhere, Shift+drag creates a
        // triangle that starts lined up.)
        if button == mouse::Button::Left
            && self.tool == Tool::Shape
            && state.modifiers.shift()
            && !state.modifiers.command()
            && !state.lasso_armed
            && matches!(state.interaction, Interaction::Idle)
        {
            let hovered = self.hit_test(pos);
            if let Some(Hover::Vertex(v)) = hovered {
                match state.selection.iter().position(|&s| s == v) {
                    Some(i) => {
                        state.selection.remove(i);
                    }
                    None => state.selection.push(v),
                }
                state.selected_in = self.document.revision();
            }
            if matches!(hovered, Some(Hover::Vertex(_))) || !state.selection.is_empty() {
                return Some(canvas::Action::request_redraw().and_capture());
            }
        }
        if button == mouse::Button::Left && self.picking(state) {
            return Some(
                match self.pick(pos) {
                    Some(picked) => canvas::Action::publish(Message::Pick(picked)),
                    None => canvas::Action::request_redraw(),
                }
                .and_capture(),
            );
        }

        state.interaction = match button {
            mouse::Button::Middle if state.modifiers.command() => Interaction::Zooming {
                anchor: pos,
                last: pos,
            },
            mouse::Button::Middle => Interaction::Panning { last: pos },
            mouse::Button::Left if matches!(self.tool, Tool::Background { .. }) => {
                Interaction::MovingBackground { last: pos }
            }
            mouse::Button::Left if self.tool == Tool::Mirror => {
                let snapped = self.mirror_end(pos);
                let start = snapped.unwrap_or(world);
                state.mirror_fits = false;
                state.mirror_through = None;
                Interaction::PlacingMirror {
                    start,
                    anchored: snapped.is_some(),
                    from: start,
                    to: start,
                }
            }
            // Adjusting the brush (holding W or C): a click applies it
            // to what's previewed, not what's under the mouse.
            mouse::Button::Left
                if state.tweaking.is_some() && matches!(self.tool, Tool::Paint { .. }) =>
            {
                let at = state.tweaking.map_or(pos, |tweak| tweak.at);
                state.stroke.clear();
                state.edge_stroke.clear();
                self.paint_at(state, at);
                let faces = std::mem::take(&mut state.stroke);
                let edges = std::mem::take(&mut state.edge_stroke);
                return Some(self.paint(faces, edges).and_capture());
            }
            mouse::Button::Left if matches!(self.tool, Tool::Paint { .. }) => {
                state.stroke.clear();
                state.edge_stroke.clear();
                self.paint_at(state, pos);
                Interaction::Painting { last: world }
            }
            // After Q: a lasso.
            mouse::Button::Left if state.lasso_armed => {
                state.lasso_armed = false;
                state.lasso = vec![world];
                Interaction::Lassoing
            }
            // With a selection, dragging moves it (and a click lets go
            // of it).
            mouse::Button::Left if !state.selection.is_empty() => Interaction::MovingSelection {
                from: world,
                to: world,
                held: true,
            },
            mouse::Button::Left => match self.hit_test(pos) {
                Some(Hover::Vertex(id)) => Interaction::MovingVertex {
                    id,
                    to: self.document.vertex(id),
                },
                Some(Hover::Edge { a, b, at }) => Interaction::Extending {
                    a,
                    b,
                    start: at,
                    apex: at,
                },
                Some(Hover::Face(face)) => Interaction::LeavingFace {
                    face,
                    last: world,
                    edge: None,
                    apex: world,
                },
                // With Ctrl, it starts on the grid; with Shift, lined up.
                None => {
                    let (from, fine, near) = self.creation_start(
                        world,
                        state.modifiers.command(),
                        state.modifiers.shift(),
                    );
                    state.start_guides = (fine, near);
                    Interaction::Creating { from, to: from }
                }
            },
            _ => return None,
        };
        self.deadline.set(Some(std::time::Instant::now() + BUDGET));
        state.pending = self.pending(state.interaction);
        self.deadline.set(None);

        Some(canvas::Action::request_redraw().and_capture())
    }

    /// The cursor moved to `pos` (screen; outside too, while dragging).
    pub(super) fn on_move(&self, state: &mut State, pos: Point) -> canvas::Action<Message> {
        let world = self.camera.to_world(pos);

        if let Some(tweak) = &mut state.tweaking
            && (tweak.what == Tweaking::Reach || matches!(state.interaction, Interaction::Idle))
        {
            let across = pos.x - tweak.last.x;
            tweak.last = pos;
            if tweak.what == Tweaking::LoopTurn {
                let turn = Dial::LOOP_TURN.moved(self.loop_turn(state), across);
                state.loop_turn = Some(turn);
                self.select_edge_loop(state);
                return canvas::Action::request_redraw().and_capture();
            }
            let message = match tweak.what {
                Tweaking::Width => Message::TweakWidth(across),
                Tweaking::Lightness => Message::TweakLightness(across),
                Tweaking::Opacity => Message::TweakOpacity(across),
                Tweaking::Reach => Message::TweakReach(across),
                Tweaking::LoopTurn => unreachable!("tweaked here"),
            };
            return canvas::Action::publish(message).and_capture();
        }

        match &mut state.interaction {
            Interaction::Idle if self.tool != Tool::Shape || !self.shown => {
                return canvas::Action::request_redraw();
            }
            Interaction::Painting { last } => {
                // Everything along the way, however fast the move.
                let from = self.camera.to_screen(*last);
                *last = world;
                let steps = (from.distance(pos) / 3.0).ceil().max(1.0) as usize;
                for i in 1..=steps {
                    self.paint_at(state, from + (pos - from) * (i as f32 / steps as f32));
                }
                return canvas::Action::request_redraw().and_capture();
            }
            Interaction::MovingBackground { last } => {
                let delta = self.camera.to_world(pos) - self.camera.to_world(*last);
                *last = pos;
                return canvas::Action::publish(Message::MoveBackground(delta)).and_capture();
            }
            Interaction::Idle => {
                if let Some(hover) = self.hit_test(pos) {
                    let vertex = self.hover_vertex(hover);
                    let same = state.shape.as_ref().is_some_and(|shape| {
                        shape.triangles.iter().flatten().any(|&v| v == vertex)
                    });
                    if !same {
                        let previous = state.shape.replace(self.highlight(vertex));
                        state.fading = Some((previous, Instant::now()));
                    }
                }
                return canvas::Action::request_redraw();
            }
            Interaction::Panning { last } => {
                let delta = pos - *last;
                *last = pos;
                return canvas::Action::publish(Message::Pan(delta)).and_capture();
            }
            Interaction::Zooming { anchor, last } => {
                // Up zooms in, as the wheel turned away does.
                let factor = ((last.y - pos.y) / ZOOM_DRAG).exp();
                *last = pos;
                return canvas::Action::publish(Message::Zoom {
                    anchor: *anchor,
                    factor,
                })
                .and_capture();
            }
            Interaction::Lassoing => {
                let far = state
                    .lasso
                    .last()
                    .is_none_or(|&p| self.camera.to_screen(p).distance(pos) >= 3.0);
                if far {
                    state.lasso.push(world);
                }
                return canvas::Action::request_redraw().and_capture();
            }
            // Pressed on the selection: not moving it until clearly dragged,
            // so a tap still lets go of it.
            Interaction::MovingSelection {
                from,
                to,
                held: true,
            } if from == to && self.camera.to_screen(*from).distance(pos) < CLICK_SLOP => {
                return canvas::Action::request_redraw().and_capture();
            }
            _ => {}
        }

        // Worked out when the frame is drawn (see `follow`).
        state.aim = Some(world);
        canvas::Action::request_redraw().and_capture()
    }

    /// The left or middle button let go, the cursor at `screen`.
    pub(super) fn on_release(
        &self,
        state: &mut State,
        screen: Option<Point>,
    ) -> Option<canvas::Action<Message>> {
        // Grabbed, rotating or scaling: only a click applies that.
        if state.interaction.is_transforming() {
            return None;
        }
        // Where it was let go, if not worked out yet.
        if let Some(world) = state.aim.take() {
            self.follow(state, world);
        }
        let finished = std::mem::take(&mut state.interaction);
        let pending = state.pending.take();
        state.guides.clear();
        state.near_guides.clear();

        // A mirror where it was let go, if it's long enough to tell
        // its direction and clear of the drawing.
        if let Interaction::PlacingMirror { from, to, .. } = finished {
            let long = self
                .camera
                .to_screen(from)
                .distance(self.camera.to_screen(to))
                >= 2.0 * VERTEX_HIT;
            let mirror = Mirror { a: from, b: to };
            return Some(
                if long && self.fits(mirror) {
                    canvas::Action::publish(Message::SetMirror(mirror))
                } else {
                    canvas::Action::request_redraw()
                }
                .and_capture(),
            );
        }
        if let Interaction::Lassoing = finished {
            let lasso = std::mem::take(&mut state.lasso);
            if lasso.len() >= 3 {
                let caught = self
                    .document
                    .unique_vertices()
                    .into_iter()
                    .filter(|&v| inside_polygon(self.document.vertex(v), &lasso));
                if state.lasso_removes {
                    let caught: Vec<VertexId> = caught.collect();
                    state.selection.retain(|v| !caught.contains(v));
                } else {
                    for v in caught {
                        if !state.selection.contains(&v) {
                            state.selection.push(v);
                        }
                    }
                }
            }
            state.selected_in = self.document.revision();
            return Some(canvas::Action::request_redraw().and_capture());
        }
        // A click (not a drag) on the selection lets go of it.
        if let (Interaction::MovingSelection { from, .. }, None) = (finished, &pending) {
            let still =
                screen.is_some_and(|pos| self.camera.to_screen(from).distance(pos) < CLICK_SLOP);
            if still {
                state.selection.clear();
            }
        }

        if let Interaction::Idle = finished {
            return None;
        }
        if let Interaction::Painting { .. } = finished {
            let faces = std::mem::take(&mut state.stroke);
            let edges = std::mem::take(&mut state.edge_stroke);
            return Some(self.paint(faces, edges).and_capture());
        }

        // Exactly what was shown.
        Some(
            match pending {
                Some(pending) => canvas::Action::publish(Message::Edit {
                    edits: pending.edits,
                    revision: self.document.revision(),
                }),
                None => canvas::Action::request_redraw(),
            }
            .and_capture(),
        )
    }

    /// The wheel turned, the cursor at `anchor` (screen).
    pub(super) fn on_wheel(
        &self,
        state: &mut State,
        delta: mouse::ScrollDelta,
        anchor: Point,
    ) -> canvas::Action<Message> {
        if state.modifiers.shift() {
            // Some platforms turn Shift+wheel into horizontal scrolling.
            let (x, y, per_notch) = match delta {
                mouse::ScrollDelta::Lines { x, y } => (x, y, 1.0),
                mouse::ScrollDelta::Pixels { x, y } => (x, y, 50.0),
            };
            let angle = if y != 0.0 { y } else { x } / per_notch * ROTATE_STEP;
            let message = if matches!(self.tool, Tool::Background { .. }) {
                let anchor = self.camera.to_world(anchor);
                Message::RotateBackground { anchor, angle }
            } else {
                Message::Rotate { anchor, angle }
            };
            return canvas::Action::publish(message).and_capture();
        }

        let factor = match delta {
            mouse::ScrollDelta::Lines { y, .. } => 1.15f32.powf(y),
            mouse::ScrollDelta::Pixels { y, .. } => (y / 200.0).exp(),
        };
        let message = if matches!(self.tool, Tool::Background { .. }) {
            let anchor = self.camera.to_world(anchor);
            Message::ScaleBackground { anchor, factor }
        } else {
            Message::Zoom { anchor, factor }
        };

        canvas::Action::publish(message).and_capture()
    }

    /// A frame: where a drag got to since the last one, and the shape
    /// highlight fading over.
    pub(super) fn on_frame(
        &self,
        state: &mut State,
        now: Instant,
    ) -> Option<canvas::Action<Message>> {
        // A frame: where the drag got to since the last one.
        if let Some(world) = state.aim.take() {
            self.follow(state, world);
        }
        // Keep drawing frames while the shape highlight moves over.
        let (_, since) = state.fading.as_ref()?;
        if now.duration_since(*since) >= SHAPE_FADE {
            state.fading = None;
        }
        Some(canvas::Action::request_redraw())
    }

    /// What a key pressed does on the canvas now (in its mode), if anything.
    pub(super) fn pressed(
        &self,
        key: &keyboard::Key,
        physical: keyboard::key::Physical,
        modifiers: keyboard::Modifiers,
    ) -> Option<Action> {
        let context = match self.tool {
            Tool::Shape => Context::Shape,
            Tool::Paint { .. } => Context::Paint,
            Tool::Background { .. } | Tool::Mirror => return None,
        };
        let chord = Chord::pressed(key, physical, modifiers)?;
        self.keys?.action(&chord, context)
    }
}

/// The points of `vertices` of `document` as seen: with their mirror
/// images across `mirrors`.
pub(super) fn as_seen(
    document: &Document,
    vertices: &[VertexId],
    mirrors: &[Mirror],
) -> Vec<Point> {
    let images = doc::images(mirrors);
    vertices
        .iter()
        .flat_map(|&v| {
            images
                .iter()
                .map(move |image| image.apply(document.vertex(v)))
        })
        .collect()
}
