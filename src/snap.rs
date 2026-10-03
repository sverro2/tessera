//! How dragged points snap (View > Snap settings), kept between runs in the
//! user's config folder (`~/.config/tessera/snap.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The grid steps there are to pick from, and the default.
pub const MIN_GRID: f32 = 0.1;
pub const MAX_GRID: f32 = 10_000.0;
const DEFAULT_GRID: f32 = 10.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Snap {
    /// Holding Ctrl while dragging, points land on a grid this fine (world
    /// units, a pixel each), whatever the zoom.
    pub grid: f32,
    /// Where it's kept; `None` keeps it in memory only (as in tests).
    #[serde(skip)]
    store: Option<PathBuf>,
}

impl Default for Snap {
    fn default() -> Self {
        Snap {
            grid: DEFAULT_GRID,
            store: None,
        }
    }
}

impl Snap {
    /// As kept in the config folder; the defaults for what isn't there.
    pub fn load() -> Self {
        // Tests keep to the defaults, and leave the user's file be.
        if cfg!(test) {
            return Snap::default();
        }
        let store = crate::recent::config_folder().map(|folder| folder.join("snap.json"));
        let mut snap = store
            .as_deref()
            .and_then(|store| std::fs::read_to_string(store).ok())
            .and_then(|text| serde_json::from_str::<Snap>(&text).ok())
            .unwrap_or_default();
        if !(MIN_GRID..=MAX_GRID).contains(&snap.grid) {
            snap.grid = DEFAULT_GRID;
        }
        snap.store = store;
        snap
    }

    /// Sets the grid step (if it will do), and keeps it.
    pub fn set_grid(&mut self, grid: f32) -> bool {
        if !(MIN_GRID..=MAX_GRID).contains(&grid) {
            return false;
        }
        self.grid = grid;
        self.save();
        true
    }

    /// Keeps it; not being able to is no reason to bother anyone.
    fn save(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if let Some(folder) = store.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(store, text);
        }
    }
}
