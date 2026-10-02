//! Mirrors: placing one, the drawing seen (and edited) through its
//! mirror images.

use super::*;

/// A mirror line.
pub(super) const MIRROR: Color = Color::from_rgb8(0xbb, 0x9a, 0xf7);

impl Editor<'_> {
    /// Which of the drawing's mirror images `world` is in: the map from
    /// the drawing to it, `None` for the drawing itself. Going back through
    /// the mirrors, last first, it's mirrored back across each it's on the
    /// far side of (from all there is before that mirror).
    pub(super) fn image_at(&self, world: Point) -> Option<Affine> {
        let used = self.document.unique_vertices();
        if self.mirrors.is_empty() || used.is_empty() {
            return None;
        }
        // The middle of the drawing; of all there is up to a mirror, where
        // its images take it, on average (the maps are affine).
        let sum = used
            .iter()
            .map(|&v| self.document.vertex(v))
            .fold(Vector::ZERO, |sum, p| sum + Vector::new(p.x, p.y));
        let middle = Point::ORIGIN + sum * (1.0 / used.len() as f32);
        let mut p = world;
        let mut back = Affine::IDENTITY;
        for (k, mirror) in self.mirrors.iter().enumerate().rev() {
            let images = doc::images(&self.mirrors[..k]);
            let sum = images
                .iter()
                .map(|image| image.apply(middle))
                .fold(Vector::ZERO, |sum, q| sum + Vector::new(q.x, q.y));
            let centre = Point::ORIGIN + sum * (1.0 / images.len() as f32);
            if area2(mirror.a, mirror.b, p) * area2(mirror.a, mirror.b, centre) < 0.0 {
                p = mirror.reflect(p);
                back = back.then(mirror.affine());
            }
        }
        let image = back.inverse();
        (!image.near(Affine::IDENTITY)).then_some(image)
    }

    /// Whether `mirror` can go on the current layer (instead of the one it
    /// has): the drawing doesn't overlap its image across it.
    pub(super) fn fits(&self, mirror: Mirror) -> bool {
        mirror.a != mirror.b && !self.document.overlaps_under(None, &[mirror.affine()])
    }

    /// This editor seeing the current layer through its mirror, if the
    /// cursor went over to its image (shaping or painting): there it's the
    /// image that's picked, dragged, painted and previewed, and the edits
    /// made to it go to the drawing, mirrored back.
    pub(super) fn flipped(&self, state: &State) -> Option<Editor<'_>> {
        let image = state.image?;
        let edits = matches!(self.tool, Tool::Shape | Tool::Paint { .. });
        if self.mirrors.is_empty() || !edits || self.camera.image.is_some() {
            return None;
        }
        Some(Editor {
            document: self.document,
            current: self.current,
            crossfade: self.crossfade,
            show_edges: self.show_edges,
            mirrors: self.mirrors,
            shown: self.shown,
            below: self.below.clone(),
            above: self.above.clone(),
            camera: Camera {
                image: Some(image),
                ..self.camera
            },
            caches: self.caches,
            background: self.background,
            tool: self.tool,
            proportional: self.proportional,
            clipboard: self.clipboard,
            keys: self.keys,
            deadline: self.deadline.clone(),
        })
    }

    /// Where the drawing's vertices are: where a mirror can run through it
    /// and its image join up.
    pub(super) fn mirror_points(&self) -> Vec<Point> {
        self.document
            .unique_vertices()
            .into_iter()
            .map(|v| self.document.vertex(v))
            .collect()
    }

    /// Where an end of a mirror being placed snaps, the cursor at `screen`:
    /// onto a vertex of the drawing in reach, so the mirror
    /// runs right through it, and its image joins up there.
    pub(super) fn mirror_end(&self, screen: Point) -> Option<Point> {
        self.mirror_points()
            .into_iter()
            .map(|p| (p, self.camera.to_screen(p).distance(screen)))
            .filter(|&(_, d)| d <= VERTEX_HIT)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(p, _)| p)
    }

    /// A mirror being placed along `from` → `to`, snapped to run right
    /// through a vertex of the drawing it passes close by:
    /// turned around `from` if `turning` (it's on a vertex itself), else
    /// moved across, keeping its angle. Only to where the mirror fits. The
    /// line, and the vertex it now runs through.
    pub(super) fn snap_mirror(
        &self,
        from: Point,
        to: Point,
        turning: bool,
    ) -> Option<(Point, Point, Point)> {
        let camera = self.camera;
        let (a, b) = (camera.to_screen(from), camera.to_screen(to));
        if a.distance(b) < 2.0 * VERTEX_HIT {
            return None;
        }
        let mut near: Vec<(Point, f32)> = self
            .mirror_points()
            .into_iter()
            .filter(|&p| camera.to_screen(p).distance(a) > 1.0)
            .map(|p| (p, closest_on_line(camera.to_screen(p), a, b).1))
            .filter(|&(_, d)| d <= VERTEX_HIT)
            .collect();
        near.sort_by(|x, y| x.1.total_cmp(&y.1));
        near.into_iter().find_map(|(p, _)| {
            let (from, to) = if turning {
                let d = p - from;
                let length = (to - from).x.hypot((to - from).y);
                (from, from + d * (length / d.x.hypot(d.y)))
            } else {
                let (foot, _) = closest_on_line(p, from, to);
                let across = p - foot;
                (from + across, to + across)
            };
            self.fits(Mirror { a: from, b: to })
                .then_some((from, to, p))
        })
    }

    /// The camera as it sees what's not seen through a mirror image: the
    /// mirror lines, other layers.
    pub(super) fn plain_camera(&self) -> Camera {
        Camera {
            image: None,
            ..self.camera
        }
    }

    /// The whole line through `mirror`, across the view, dashed.
    pub(super) fn draw_mirror_line(
        &self,
        frame: &mut Frame,
        mirror: Mirror,
        bounds: Rectangle,
        color: Color,
    ) {
        let camera = self.plain_camera();
        let (a, b) = (camera.to_screen(mirror.a), camera.to_screen(mirror.b));
        let d = b - a;
        let length = d.x.hypot(d.y);
        if length == 0.0 {
            return;
        }
        let far = d * ((bounds.width + bounds.height) * 2.0 / length);
        let dashed = Stroke {
            line_dash: LineDash {
                segments: &[10.0, 6.0],
                offset: 0,
            },
            ..stroke(color, 1.5)
        };
        frame.stroke(&Path::line(a - far, b + far), dashed);
    }

    /// Placing a mirror: where it can't go (over the drawing), the line
    /// being drawn (green if it does, red if it doesn't) and the mirror
    /// image it makes, and the cursor.
    pub(super) fn draw_placing_mirror(
        &self,
        frame: &mut Frame,
        state: &State,
        bounds: Rectangle,
        cursor: Option<Point>,
    ) {
        let camera = self.camera;
        frame.fill_text(canvas::Text {
            content: "Drag a line clear of the drawing to mirror it across · it snaps to run through vertices, to join up there · Shift: snap the angle · Esc: cancel".into(),
            position: Point::new(bounds.width / 2.0, 14.0),
            color: EDGE,
            size: 13.0.into(),
            align_x: iced::widget::text::Alignment::Center,
            ..canvas::Text::default()
        });

        // Lines crossing the drawing's outline (its convex hull) mirror it
        // over itself; any line clear of it will do.
        let points: Vec<Point> = self
            .mirror_points()
            .into_iter()
            .map(|p| camera.to_screen(p))
            .collect();
        let hull = crate::geometry::convex_hull(&points);
        if let Some((first, rest)) = hull.split_first() {
            let outline = Path::new(|p| {
                p.move_to(*first);
                for &point in rest {
                    p.line_to(point);
                }
                p.close();
            });
            frame.fill(&outline, Color { a: 0.10, ..REMOVED });
            let dashed = Stroke {
                line_dash: LineDash {
                    segments: &[4.0, 4.0],
                    offset: 0,
                },
                ..stroke(Color { a: 0.6, ..REMOVED }, 1.5)
            };
            frame.stroke(&outline, dashed);
        }

        // The mirror there is now, being replaced.
        for &mirror in self.mirrors {
            self.draw_mirror_line(frame, mirror, bounds, Color { a: 0.3, ..MIRROR });
        }

        if let Interaction::PlacingMirror { from, to, .. } = state.interaction
            && from != to
        {
            let mirror = Mirror { a: from, b: to };
            let color = if state.mirror_fits { ADDED } else { REMOVED };
            let image = self.document.reflection(mirror);
            let mesh = self.mesh(image.triangles());
            frame.fill(&mesh, Color { a: 0.2, ..color });
            frame.stroke(&mesh, stroke(Color { a: 0.6, ..color }, 1.0));
            self.draw_mirror_line(frame, mirror, bounds, Color { a: 0.7, ..color });
            let (a, b) = (camera.to_screen(from), camera.to_screen(to));
            frame.stroke(&Path::line(a, b), stroke(color, 2.5));
            // Where it runs through the drawing: there it joins up with its
            // image.
            if let Some(through) = state.mirror_through {
                let at = camera.to_screen(through);
                frame.stroke(&Path::circle(at, 11.0), stroke(MIRROR, 2.5));
                frame.fill(&Path::circle(at, 3.0), MIRROR);
            }
            handle(frame, a, color);
            handle(frame, b, color);
            return;
        }

        if let Some(p) = cursor {
            let snapped = self.mirror_end(p);
            let at = snapped.map_or(p, |q| camera.to_screen(q));
            if snapped.is_some() {
                frame.stroke(&Path::circle(at, 11.0), stroke(MIRROR, 2.5));
            }
            handle(frame, at, MIRROR);
        }
    }
}
