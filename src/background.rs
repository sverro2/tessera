//! A background image to draw over: a guide only, not part of the drawing.
//! It lives in world coordinates, so it pans, zooms and turns with the view.

use std::io::Cursor;
use std::sync::Arc;

use iced::widget::canvas::{self, Frame};
use iced::widget::image::Handle;
use iced::{Point, Radians, Rectangle, Size, Vector};

use crate::camera::Camera;

#[derive(Debug, Clone)]
pub struct Background {
    /// The image file as picked (PNG, JPEG, …), kept as is for saving.
    bytes: Arc<Vec<u8>>,
    handle: Handle,
    /// In pixels.
    size: Size,
    /// Where the image's centre is (world).
    pub center: Point,
    /// World units per image pixel.
    pub scale: f32,
    /// Clockwise, in radians, on top of the view's rotation.
    pub rotation: f32,
    /// 0 (invisible) to 1.
    pub opacity: f32,
    /// Hidden, it's kept (and saved) but not drawn.
    pub visible: bool,
}

impl Background {
    /// Smallest and largest scale, so the image can't vanish or explode.
    pub const MIN_SCALE: f32 = 1e-3;
    pub const MAX_SCALE: f32 = 1e3;

    /// An image file's contents, at the origin, one world unit a pixel.
    pub fn load(bytes: Vec<u8>) -> Result<Self, String> {
        let (width, height) = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())?
            .into_dimensions()
            .map_err(|_| "not an image Tessera can read".to_string())?;
        if width == 0 || height == 0 {
            return Err("the image is empty".to_string());
        }

        let bytes = Arc::new(bytes);
        Ok(Background {
            handle: Handle::from_bytes(bytes.as_ref().clone()),
            bytes,
            size: Size::new(width as f32, height as f32),
            center: Point::ORIGIN,
            scale: 1.0,
            rotation: 0.0,
            opacity: 0.5,
            visible: true,
        })
    }

    /// `other`'s image in this one's place: the same centre, rotation and
    /// opacity, scaled to fit where this image was.
    pub fn replaced_by(&self, other: Background) -> Background {
        let fit = (self.size.width * self.scale / other.size.width)
            .min(self.size.height * self.scale / other.size.height);
        Background {
            center: self.center,
            scale: fit.clamp(Self::MIN_SCALE, Self::MAX_SCALE),
            rotation: self.rotation,
            opacity: self.opacity,
            ..other
        }
    }

    /// The image file's contents.
    /// Whether `other` is the same image (placed alike or not): the same
    /// bytes, shared.
    pub fn same_image(&self, other: &Background) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Fits the whole image in a `viewport` (screen size) through `camera`,
    /// upright on screen.
    pub fn fit(&mut self, camera: Camera, viewport: Size) {
        let on_screen = (viewport.width / self.size.width).min(viewport.height / self.size.height);
        self.center = camera.to_world(Point::new(viewport.width / 2.0, viewport.height / 2.0));
        self.scale = (on_screen / camera.zoom).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        self.rotation = -camera.rotation;
    }

    pub fn draw(&self, frame: &mut Frame, camera: Camera) {
        if !self.visible {
            return;
        }
        let center = camera.to_screen(self.center);
        let size = self.size * (self.scale * camera.zoom);
        let bounds = Rectangle::new(center - Vector::new(size.width, size.height) * 0.5, size);

        frame.draw_image(
            bounds,
            canvas::Image::new(self.handle.clone())
                .rotation(Radians(self.rotation + camera.rotation))
                .opacity(self.opacity),
        );
    }

    /// The image's corners (world), in order around it.
    pub fn corners(&self) -> [Point; 4] {
        let (sin, cos) = self.rotation.sin_cos();
        let (w, h) = (
            self.size.width * self.scale / 2.0,
            self.size.height * self.scale / 2.0,
        );
        [(-w, -h), (w, -h), (w, h), (-w, h)]
            .map(|(x, y)| self.center + Vector::new(x * cos - y * sin, x * sin + y * cos))
    }

    /// Scales by `factor`, keeping the image point at `anchor` (world) in
    /// place.
    pub fn scale_at(&mut self, anchor: Point, factor: f32) {
        let scale = (self.scale * factor).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        self.center = anchor + (self.center - anchor) * (scale / self.scale);
        self.scale = scale;
    }

    /// Rotates clockwise by `angle` radians around `anchor` (world).
    pub fn rotate_at(&mut self, anchor: Point, angle: f32) {
        let (sin, cos) = angle.sin_cos();
        let d = self.center - anchor;
        self.center = anchor + Vector::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos);
        self.rotation = normalize(self.rotation + angle);
    }
}

/// An angle in (-π, π].
pub fn normalize(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    PI - (PI - angle).rem_euclid(TAU)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 4×2 PNG.
    pub(crate) fn pixel() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::DynamicImage::new_rgba8(4, 2)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn loading_reads_the_size() {
        let background = Background::load(pixel()).unwrap();
        assert_eq!(background.size, Size::new(4.0, 2.0));
        assert!(Background::load(b"not an image".to_vec()).is_err());
    }

    #[test]
    fn fitting_shows_all_of_it_upright() {
        let mut background = Background::load(pixel()).unwrap();
        let camera = Camera {
            zoom: 2.0,
            rotation: 0.3,
            ..Camera::default()
        };
        background.fit(camera, Size::new(800.0, 600.0));

        // 4×2 pixels in 800×600: 200 screen px a pixel, so 100 world units.
        assert!((background.scale - 100.0).abs() < 1e-3);
        assert!((background.rotation + 0.3).abs() < 1e-6);
        let centre = camera.to_screen(background.center);
        assert!(centre.distance(Point::new(400.0, 300.0)) < 1e-2);
        for corner in background.corners().map(|p| camera.to_screen(p)) {
            assert!((-0.01..=800.01).contains(&corner.x), "{corner:?}");
            assert!((-0.01..=600.01).contains(&corner.y), "{corner:?}");
        }
    }

    #[test]
    fn a_replacement_takes_the_old_place() {
        let mut old = Background::load(pixel()).unwrap();
        old.center = Point::new(10.0, 20.0);
        old.scale = 5.0; // 20×10 world units
        old.rotation = 0.5;
        old.opacity = 0.8;

        // A square image: 10 wide at most, to fit the old 10 high.
        let mut square = Vec::new();
        image::DynamicImage::new_rgba8(3, 3)
            .write_to(&mut Cursor::new(&mut square), image::ImageFormat::Png)
            .unwrap();
        let new = old.replaced_by(Background::load(square).unwrap());

        assert_eq!(new.center, old.center);
        assert_eq!(new.rotation, 0.5);
        assert_eq!(new.opacity, 0.8);
        assert!((new.scale * 3.0 - 10.0).abs() < 1e-4);
    }

    #[test]
    fn scaling_and_rotating_keep_the_anchor() {
        let mut background = Background::load(pixel()).unwrap();
        let anchor = background.corners()[2];

        background.scale_at(anchor, 3.0);
        assert!(background.corners()[2].distance(anchor) < 1e-4);
        background.rotate_at(anchor, 1.0);
        assert!(background.corners()[2].distance(anchor) < 1e-4);
    }
}
