//! Mapping between world (document) coordinates and screen coordinates.

use std::f32::consts::{PI, TAU};

use iced::{Point, Vector};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Screen position (relative to the canvas) of the world origin.
    pub pan: Vector,
    pub zoom: f32,
    /// Clockwise rotation of the world on screen, in radians, in (-π, π].
    pub rotation: f32,
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.05;
    pub const MAX_ZOOM: f32 = 50.0;

    pub fn to_screen(self, world: Point) -> Point {
        let (sin, cos) = self.rotation.sin_cos();
        let (x, y) = (world.x * self.zoom, world.y * self.zoom);
        Point::new(x * cos - y * sin, x * sin + y * cos) + self.pan
    }

    pub fn to_world(self, screen: Point) -> Point {
        let (sin, cos) = self.rotation.sin_cos();
        let p = screen - self.pan;
        Point::new(
            (p.x * cos + p.y * sin) / self.zoom,
            (-p.x * sin + p.y * cos) / self.zoom,
        )
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
        }
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
}
