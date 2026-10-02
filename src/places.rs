//! Where the user left off in each file lately: the current layer and the
//! view. Kept between runs in the user's config folder
//! (`~/.config/tessera/places.json`), apart from the files themselves, so
//! looking around doesn't count as a change to save; opening a file again
//! picks up there.

use std::path::{Path, PathBuf};

use iced::Vector;
use serde::{Deserialize, Serialize};

use crate::camera::Camera;

/// How many files are remembered.
const MAX: usize = 64;

/// Where the user was in a file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// The current layer: which of [`crate::layers::Layers::layers`].
    pub layer: usize,
    pub pan: [f32; 2],
    pub zoom: f32,
    /// Clockwise, in radians.
    pub rotation: f32,
}

impl Place {
    pub fn new(layer: usize, camera: Camera) -> Self {
        Place {
            layer,
            pan: [camera.pan.x, camera.pan.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
        }
    }

    /// The view, if it's a usable one.
    pub fn camera(&self) -> Option<Camera> {
        let [x, y] = self.pan;
        let finite = [x, y, self.zoom, self.rotation]
            .iter()
            .all(|n| n.is_finite());
        (finite && self.zoom > 0.0).then(|| Camera {
            pan: Vector::new(x, y),
            zoom: self.zoom.clamp(Camera::MIN_ZOOM, Camera::MAX_ZOOM),
            rotation: self.rotation,
            image: None,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Saved {
    path: PathBuf,
    #[serde(flatten)]
    place: Place,
}

/// Where the user left off, by file, most recent first.
#[derive(Debug, Default)]
pub struct Places {
    places: Vec<(PathBuf, Place)>,
    /// Where they're kept; `None` keeps them in memory only (as in tests).
    store: Option<PathBuf>,
}

impl Places {
    /// As kept in the config folder (none if there are none yet, or they
    /// can't be read).
    pub fn load() -> Self {
        // Tests leave the user's file be.
        if cfg!(test) {
            return Places::default();
        }
        let store = crate::recent::config_folder().map(|folder| folder.join("places.json"));
        let places = store
            .as_deref()
            .and_then(|store| std::fs::read_to_string(store).ok())
            .and_then(|text| serde_json::from_str::<Vec<Saved>>(&text).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|saved| (saved.path, saved.place))
            .collect();
        Places { places, store }
    }

    /// Where the user left off in `path`, if remembered.
    pub fn get(&self, path: &Path) -> Option<Place> {
        self.places
            .iter()
            .find(|(p, _)| p == path)
            .map(|&(_, place)| place)
    }

    /// Remembers where the user is in `path` (first). Kept.
    pub fn remember(&mut self, path: PathBuf, place: Place) {
        self.places.retain(|(p, _)| *p != path);
        self.places.insert(0, (path, place));
        self.places.truncate(MAX);
        self.save();
    }

    /// Keeps them; not being able to is no reason to bother anyone.
    fn save(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if let Some(folder) = store.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let saved: Vec<_> = self
            .places
            .iter()
            .map(|(path, place)| Saved {
                path: path.clone(),
                place: *place,
            })
            .collect();
        if let Ok(text) = serde_json::to_string_pretty(&saved) {
            let _ = std::fs::write(store, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_place_per_file_first() {
        let mut places = Places::default();
        let at = |layer| Place::new(layer, Camera::default());
        places.remember(PathBuf::from("a"), at(1));
        places.remember(PathBuf::from("b"), at(2));
        places.remember(PathBuf::from("a"), at(3));
        assert_eq!(places.get(Path::new("a")), Some(at(3)));
        assert_eq!(places.places.len(), 2);
        assert_eq!(places.places[0].0, PathBuf::from("a"));
        for i in 0..100 {
            places.remember(PathBuf::from(i.to_string()), at(0));
        }
        assert_eq!(places.places.len(), MAX);
        assert_eq!(places.get(Path::new("b")), None);
    }

    #[test]
    fn a_view_that_makes_no_sense_isnt_used() {
        let mut place = Place::new(0, Camera::default());
        assert!(place.camera().is_some());
        place.zoom = f32::NAN;
        assert!(place.camera().is_none());
    }
}
