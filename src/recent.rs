//! The files opened or saved lately, kept between runs in the user's config
//! folder (`~/.config/tessera/recent.json`).

use std::path::{Path, PathBuf};

/// How many files are kept.
const MAX: usize = 8;

/// The recent files, most recent first.
#[derive(Debug, Default)]
pub struct Recent {
    paths: Vec<PathBuf>,
    /// Where they're kept; `None` keeps them in memory only (as in tests).
    store: Option<PathBuf>,
}

impl Recent {
    /// The recent files as kept in the config folder (none if there are
    /// none yet, or they can't be read).
    pub fn load() -> Self {
        let store = config_file();
        let paths = store
            .as_deref()
            .and_then(|store| std::fs::read_to_string(store).ok())
            .and_then(|text| serde_json::from_str::<Vec<PathBuf>>(&text).ok())
            .unwrap_or_default();
        Recent { paths, store }
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Puts `path` first.
    pub fn add(&mut self, path: PathBuf) {
        self.paths.retain(|p| *p != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX);
        self.save();
    }

    /// Forgets `path` (e.g. when it's gone).
    pub fn remove(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.save();
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.save();
    }

    /// Keeps the list; not being able to is no reason to bother anyone.
    fn save(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if let Some(folder) = store.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.paths) {
            let _ = std::fs::write(store, text);
        }
    }
}

/// Where the list is kept: in `$XDG_CONFIG_HOME`, else `~/.config` (or,
/// on Windows, `%APPDATA%`).
fn config_file() -> Option<PathBuf> {
    let folder = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|folder| !folder.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(folder.join("tessera").join("recent.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_first_without_repeats() {
        let mut recent = Recent::default();
        for name in ["a", "b", "a"] {
            recent.add(PathBuf::from(name));
        }
        assert_eq!(recent.paths(), [PathBuf::from("a"), PathBuf::from("b")]);

        for i in 0..20 {
            recent.add(PathBuf::from(i.to_string()));
        }
        assert_eq!(recent.paths().len(), MAX);
        assert_eq!(recent.paths()[0], PathBuf::from("19"));

        recent.remove(Path::new("19"));
        assert_eq!(recent.paths()[0], PathBuf::from("18"));
        recent.clear();
        assert!(recent.paths().is_empty());
    }

    #[test]
    fn kept_between_runs() {
        let folder = std::env::temp_dir().join(format!("tessera-recent-{}", std::process::id()));
        let store = folder.join("recent.json");
        let mut recent = Recent {
            paths: Vec::new(),
            store: Some(store.clone()),
        };
        recent.add(PathBuf::from("/drawings/flower.tessera"));

        let text = std::fs::read_to_string(&store).unwrap();
        let paths: Vec<PathBuf> = serde_json::from_str(&text).unwrap();
        assert_eq!(paths, [PathBuf::from("/drawings/flower.tessera")]);
        let _ = std::fs::remove_dir_all(folder);
    }
}
