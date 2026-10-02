//! Mapping between world (document) coordinates and screen coordinates.

use std::f32::consts::{PI, TAU};

use iced::{Point, Vector};

use crate::geometry::Affine;

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Screen position (relative to the canvas) of the world origin.
    pub pan: Vector,
    pub zoom: f32,
    /// Clockwise rotation of the world on screen, in radians, in (-π, π].
    pub rotation: f32,
    /// Seeing the world through this map first: to edit a drawing through
    /// one of its mirror images, where that image is.
    pub image: Option<Affine>,
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.05;
    pub const MAX_ZOOM: f32 = 50.0;

    pub fn to_screen(self, world: Point) -> Point {
        let world = self.image.map_or(world, |image| image.apply(world));
        let (sin, cos) = self.rotation.sin_cos();
        let (x, y) = (world.x * self.zoom, world.y * self.zoom);
        Point::new(x * cos - y * sin, x * sin + y * cos) + self.pan
    }

    /// This camera (turned as it is) moved and zoomed so that `points`
    /// (world) fill a `viewport` of this size but for `margin` (screen px)
    /// all round, in its middle. A single point (or none) keeps the zoom.
    pub fn framing(self, points: &[Point], viewport: iced::Size, margin: f32) -> Camera {
        let (sin, cos) = self.rotation.sin_cos();
        // As they're turned on screen, before zooming and panning.
        let turned = points
            .iter()
            .map(|p| Point::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos));
        let Some((min, max)) = turned.fold(None, |bounds: Option<(Point, Point)>, p| {
            Some(match bounds {
                None => (p, p),
                Some((min, max)) => (
                    Point::new(min.x.min(p.x), min.y.min(p.y)),
                    Point::new(max.x.max(p.x), max.y.max(p.y)),
                ),
            })
        }) else {
            return self;
        };
        let (width, height) = (max.x - min.x, max.y - min.y);
        let room = (
            (viewport.width - 2.0 * margin).max(1.0),
            (viewport.height - 2.0 * margin).max(1.0),
        );
        let zoom = if width.max(height) < 1e-6 {
            self.zoom
        } else {
            (room.0 / width.max(1e-6))
                .min(room.1 / height.max(1e-6))
                .clamp(Self::MIN_ZOOM, Self::MAX_ZOOM)
        };
        let middle = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        Camera {
            pan: Vector::new(
                viewport.width / 2.0 - middle.x * zoom,
                viewport.height / 2.0 - middle.y * zoom,
            ),
            zoom,
            ..self
        }
    }

    pub fn to_world(self, screen: Point) -> Point {
        let (sin, cos) = self.rotation.sin_cos();
        let p = screen - self.pan;
        let world = Point::new(
            (p.x * cos + p.y * sin) / self.zoom,
            (-p.x * sin + p.y * cos) / self.zoom,
        );
        self.image
            .map_or(world, |image| image.inverse().apply(world))
    }

    /// Zooms by `factor`, keeping the world point under `anchor` fixed.
    pub fn zoom_at(&mut self, anchor: Point, factor: f32) {
        let world = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.keep(world, anchor);
    }

    /// Rotates clockwise by `angle` radians, keeping the world point under
    /// `anchor` fixed.
    pub fn rotate_at(&mut self, anchor: Point, angle: f32) {
        let world = self.to_world(anchor);
        self.rotation = PI - (PI - self.rotation - angle).rem_euclid(TAU);
        self.keep(world, anchor);
    }

    /// Rotates to `rotation` (clockwise, radians), keeping the world point
    /// under `anchor` fixed.
    pub fn rotate_to(&mut self, anchor: Point, rotation: f32) {
        self.rotate_at(anchor, rotation - self.rotation);
    }

    /// Pans so that `world` is back at `anchor`.
    fn keep(&mut self, world: Point, anchor: Point) {
        let moved = self.to_screen(world);
        self.pan += anchor - moved;
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Vector::ZERO,
            zoom: 1.0,
            rotation: 0.0,
            image: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera {
            pan: Vector::new(30.0, -12.0),
            zoom: 1.5,
            rotation: 0.4,
            image: None,
        }
    }

    #[test]
    fn framing_fills_the_view_in_its_middle() {
        let points = [Point::new(10.0, 20.0), Point::new(110.0, 70.0)];
        let viewport = iced::Size::new(800.0, 600.0);
        let framed = camera().framing(&points, viewport, 50.0);
        assert_eq!(framed.rotation, camera().rotation);
        let screen: Vec<_> = points.iter().map(|&p| framed.to_screen(p)).collect();
        // Within the margin, and touching it one way.
        let (xs, ys): (Vec<_>, Vec<_>) = screen.iter().map(|p| (p.x, p.y)).unzip();
        let low = xs.iter().chain(&ys).copied().fold(f32::INFINITY, f32::min);
        assert!(low >= 50.0 - 1e-2, "{screen:?}");
        let fills = xs.iter().copied().fold(f32::INFINITY, f32::min) - 50.0 < 1e-2
            || ys.iter().copied().fold(f32::INFINITY, f32::min) - 50.0 < 1e-2;
        assert!(fills, "{screen:?}");
        // Centred.
        let middle = framed.to_screen(Point::new(60.0, 45.0));
        assert!(
            middle.distance(Point::new(400.0, 300.0)) < 1e-2,
            "{middle:?}"
        );
    }

    #[test]
    fn flipped_it_sees_the_world_mirrored() {
        let flip = Affine::reflection(Point::new(10.0, 0.0), Point::new(10.0, 5.0));
        let flipped = Camera {
            image: Some(flip),
            ..camera()
        };
        let p = Point::new(3.0, 7.0);
        assert!(
            flipped
                .to_screen(p)
                .distance(camera().to_screen(Point::new(17.0, 7.0)))
                < 1e-4
        );
        assert!(flipped.to_world(flipped.to_screen(p)).distance(p) < 1e-4);
    }

    #[test]
    fn to_world_inverts_to_screen() {
        let camera = camera();
        let p = Point::new(-70.0, 220.0);
        assert!(camera.to_world(camera.to_screen(p)).distance(p) < 1e-3);
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut camera = camera();
        let anchor = Point::new(200.0, 150.0);
        let world = camera.to_world(anchor);
        camera.zoom_at(anchor, 2.0);
        assert!(camera.to_screen(world).distance(anchor) < 1e-3);
    }

    #[test]
    fn rotation_keeps_anchor_fixed_and_wraps() {
        let mut camera = camera();
        let anchor = Point::new(200.0, 150.0);
        let world = camera.to_world(anchor);
        camera.rotate_at(anchor, 3.0);
        assert!(camera.to_screen(world).distance(anchor) < 1e-3);
        assert!((camera.rotation - (3.4 - TAU)).abs() < 1e-5);
    }

    #[test]
    fn rotate_to_sets_the_rotation() {
        let mut camera = camera();
        let anchor = Point::new(200.0, 150.0);
        let world = camera.to_world(anchor);
        camera.rotate_to(anchor, -1.0);
        assert!(camera.to_screen(world).distance(anchor) < 1e-3);
        assert!((camera.rotation + 1.0).abs() < 1e-5);
    }
}
