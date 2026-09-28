//! Mapping between world (document) coordinates and screen coordinates.

use iced::{Point, Vector};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Screen position (relative to the canvas) of the world origin.
    pub pan: Vector,
    pub zoom: f32,
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.05;
    pub const MAX_ZOOM: f32 = 50.0;

    pub fn to_screen(self, world: Point) -> Point {
        Point::new(world.x * self.zoom, world.y * self.zoom) + self.pan
    }

    pub fn to_world(self, screen: Point) -> Point {
        let p = screen - self.pan;
        Point::new(p.x / self.zoom, p.y / self.zoom)
    }

    /// Zooms by `factor`, keeping the world point under `anchor` fixed.
    pub fn zoom_at(&mut self, anchor: Point, factor: f32) {
        let world = self.to_world(anchor);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        let moved = self.to_screen(world);
        self.pan += anchor - moved;
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Vector::ZERO,
            zoom: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let mut camera = Camera {
            pan: Vector::new(30.0, -12.0),
            zoom: 1.5,
        };
        let anchor = Point::new(200.0, 150.0);
        let world = camera.to_world(anchor);
        camera.zoom_at(anchor, 2.0);
        assert!(camera.to_screen(world).distance(anchor) < 1e-3);
    }
}
