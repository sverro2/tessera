//! Keyboard shortcuts: which keys do what, as the user likes them, kept
//! between runs in the user's config folder (`~/.config/tessera/keys.json`).
//!
//! The file maps each action (by name) to its keys, e.g.
//! `"grab": ["G"]`, `"save_as": ["Ctrl+Shift+S"]`. Actions it leaves out
//! keep their defaults (so ones added later turn up); keys it can't read
//! are left out.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use iced::keyboard::{self, Key, Modifiers};
use serde::{Deserialize, Serialize};

/// Where an action works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    /// Anywhere.
    Global,
    /// In the shape mode.
    Shape,
    /// In the paint mode.
    Paint,
}

impl Context {
    pub const ALL: [Context; 3] = [Context::Global, Context::Shape, Context::Paint];

    pub fn label(self) -> &'static str {
        match self {
            Context::Global => "Everywhere",
            Context::Shape => "Shape mode",
            Context::Paint => "Paint mode",
        }
    }

    /// Whether a key can't do something in both: one would hide the other.
    fn overlaps(self, other: Context) -> bool {
        self == other || self == Context::Global || other == Context::Global
    }
}

/// Something a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    // Everywhere.
    ToggleMode,
    New,
    Open,
    Save,
    SaveAs,
    Undo,
    Redo,
    Mirror,
    DuplicateLayer,
    FrameShape,
    ShowKeys,
    // Shape mode.
    CutEdge,
    Delete,
    Grab,
    Rotate,
    Scale,
    Extrude,
    Lasso,
    LassoRemove,
    EdgeLoop,
    SelectShape,
    SelectAll,
    Copy,
    CutFaces,
    Paste,
    Proportional,
    Reach,
    // Paint mode.
    PaintFaces,
    PaintEdges,
    Pipette,
    Lightness,
    Opacity,
    Width,
}

impl Action {
    pub const ALL: [Action; 33] = [
        Action::ToggleMode,
        Action::New,
        Action::Open,
        Action::Save,
        Action::SaveAs,
        Action::Undo,
        Action::Redo,
        Action::Mirror,
        Action::DuplicateLayer,
        Action::FrameShape,
        Action::ShowKeys,
        Action::CutEdge,
        Action::Delete,
        Action::Grab,
        Action::Rotate,
        Action::Scale,
        Action::Extrude,
        Action::Lasso,
        Action::LassoRemove,
        Action::EdgeLoop,
        Action::SelectShape,
        Action::SelectAll,
        Action::Copy,
        Action::CutFaces,
        Action::Paste,
        Action::Proportional,
        Action::Reach,
        Action::PaintFaces,
        Action::PaintEdges,
        Action::Pipette,
        Action::Lightness,
        Action::Opacity,
        Action::Width,
    ];

    pub fn context(self) -> Context {
        use Action::*;
        match self {
            ToggleMode | New | Open | Save | SaveAs | Undo | Redo | Mirror | DuplicateLayer
            | FrameShape | ShowKeys => Context::Global,
            CutEdge | Delete | Grab | Rotate | Scale | Extrude | Lasso | LassoRemove | EdgeLoop
            | SelectShape | SelectAll | Copy | CutFaces | Paste | Proportional | Reach => {
                Context::Shape
            }
            PaintFaces | PaintEdges | Pipette | Lightness | Opacity | Width => Context::Paint,
        }
    }

    /// What it does, for the lookup.
    pub fn label(self) -> &'static str {
        use Action::*;
        match self {
            ToggleMode => "Switch between shape and paint",
            New => "New drawing",
            Open => "Open…",
            Save => "Save",
            SaveAs => "Save as…",
            Undo => "Undo",
            Redo => "Redo",
            Mirror => "Place a mirror on the layer",
            DuplicateLayer => "Duplicate the selected layer (or group)",
            FrameShape => {
                "Focus on a shape: fit it in view, its layer isolated (the selection, else the \
                 one pointed at, frontmost as drawn); again, back (but over another shape: on that)"
            }
            ShowKeys => "Show these shortcuts",
            CutEdge => "Subdivide the selection, or cut the hovered edge",
            Delete => "Delete the hovered face (or the selection)",
            Grab => "Grab the selection (click to put down)",
            Rotate => "Rotate the selection",
            Scale => "Scale the selection",
            Extrude => {
                "Extrude the selection's outer edges, or the hovered one (click to put down)"
            }
            Lasso => "Lasso: then drag around vertices to add them to the selection",
            LassoRemove => "Lasso: then drag around vertices to take them out of the selection",
            EdgeLoop => {
                "Select the edge loop pointed at; hold and move: how far it may turn at a vertex"
            }
            SelectShape => "Select the whole shape",
            SelectAll => "Select every vertex of the layer",
            Copy => "Copy the selected faces",
            CutFaces => "Cut the selected faces",
            Paste => "Paste (click to put down)",
            Proportional => "Proportional editing on/off",
            Reach => "Hold and move: how far proportional editing reaches",
            PaintFaces => "Paint faces",
            PaintEdges => "Paint edges",
            Pipette => "Pipette on/off",
            Lightness => "Hold and move: lighter or darker",
            Opacity => "Hold and move: more or less opaque",
            Width => "Hold and move: edge width",
        }
    }

    /// Its keys out of the box.
    fn defaults(self) -> &'static [&'static str] {
        use Action::*;
        match self {
            ToggleMode => &["Tab"],
            New => &["Ctrl+N"],
            Open => &["Ctrl+O"],
            Save => &["Ctrl+S"],
            SaveAs => &["Ctrl+Shift+S"],
            Undo => &["Ctrl+Z"],
            Redo => &["Ctrl+Shift+Z", "Ctrl+Y"],
            Mirror => &["Ctrl+M"],
            DuplicateLayer => &["Ctrl+Shift+D"],
            FrameShape => &["/"],
            ShowKeys => &["F1"],
            CutEdge => &["C"],
            Delete => &["D"],
            Grab => &["G"],
            Rotate => &["R"],
            Scale => &["T"],
            Extrude => &["E"],
            Lasso => &["Q"],
            LassoRemove => &["Alt+Q"],
            EdgeLoop => &["Shift+Q"],
            SelectShape => &["Ctrl+L"],
            SelectAll => &["Ctrl+A"],
            Copy => &["Ctrl+C"],
            CutFaces => &["Ctrl+X"],
            Paste => &["Ctrl+V"],
            Proportional => &["O"],
            Reach => &["Shift+O"],
            PaintFaces => &["F"],
            PaintEdges => &["E"],
            Pipette => &["I"],
            Lightness => &["C"],
            Opacity => &["Shift+C"],
            Width => &["W"],
        }
    }

    fn default_chords(self) -> Vec<Chord> {
        self.defaults()
            .iter()
            .map(|text| text.parse().expect("a default key"))
            .collect()
    }
}

/// What the mouse does with keys held: not changed here, but shown in the
/// lookup, by where it works.
pub const GESTURES: [(Context, &str, &str); 12] = [
    (
        Context::Global,
        "Esc",
        "Cancel; close; let go of the selection",
    ),
    (Context::Global, "Middle-drag", "Pan"),
    (Context::Global, "Wheel", "Zoom"),
    (
        Context::Global,
        "Ctrl+middle-drag",
        "Zoom (up: in), as on a drawing tablet",
    ),
    (Context::Global, "Shift+wheel", "Rotate the view"),
    (
        Context::Shape,
        "Drag on blank canvas, then click",
        "New triangle: its first side, then its third corner",
    ),
    (
        Context::Shape,
        "Shift+click",
        "Add or remove a vertex from the selection",
    ),
    (
        Context::Shape,
        "Shift while dragging",
        "Line up with guides",
    ),
    (
        Context::Shape,
        "Ctrl while dragging",
        "Snap to the grid, from where it starts (View > Snap settings)",
    ),
    (
        Context::Shape,
        "Right-click",
        "Cancel a drag, a new triangle, or grabbing, rotating, scaling, extruding or pasting",
    ),
    (Context::Paint, "Ctrl+click", "Pick up a colour"),
    (Context::Paint, "Click or drag", "Paint"),
];

/// A key with the modifiers held for it, e.g. Ctrl+Shift+S.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    /// A character key lowercase ("s"), else the key's name ("Tab", "F1").
    key: String,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

impl Chord {
    /// The chord for a key pressed (as `key`, at `physical`) with
    /// `modifiers`; `None` for keys that can't be bound (modifiers on their
    /// own, say).
    pub fn pressed(
        key: &Key,
        physical: keyboard::key::Physical,
        modifiers: Modifiers,
    ) -> Option<Chord> {
        let name = match key.to_latin(physical) {
            Some(c) => c.to_ascii_lowercase().to_string(),
            None => match key {
                Key::Named(named) => {
                    use keyboard::key::Named::*;
                    if matches!(
                        named,
                        Shift | Control | Alt | Super | Meta | Hyper | AltGraph
                    ) {
                        return None;
                    }
                    format!("{named:?}")
                }
                Key::Character(c) => c.to_lowercase(),
                Key::Unidentified => return None,
            },
        };
        Some(Chord {
            key: name,
            ctrl: modifiers.command(),
            shift: modifiers.shift(),
            alt: modifiers.alt(),
        })
    }

    /// The key alone, whatever modifiers: to tell its release.
    fn same_key(&self, other: &Chord) -> bool {
        self.key.eq_ignore_ascii_case(&other.key)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
        ] {
            if held {
                f.write_str(name)?;
            }
        }
        if self.key.chars().count() == 1 {
            f.write_str(&self.key.to_uppercase())
        } else {
            f.write_str(&self.key)
        }
    }
}

impl std::str::FromStr for Chord {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let mut chord = Chord {
            key: String::new(),
            ctrl: false,
            shift: false,
            alt: false,
        };
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (key, modifiers) = parts.split_last().ok_or("no key")?;
        for modifier in modifiers {
            match modifier.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "option" => chord.alt = true,
                other => return Err(format!("unknown modifier {other:?}")),
            }
        }
        if key.is_empty() {
            return Err("no key".into());
        }
        chord.key = if key.chars().count() == 1 {
            key.to_lowercase()
        } else {
            key.to_string()
        };
        Ok(chord)
    }
}

/// Which keys do what.
#[derive(Debug, Clone, PartialEq)]
pub struct Keymap {
    keys: BTreeMap<Action, Vec<Chord>>,
    /// Where it's kept; `None` keeps it in memory only (as in tests).
    store: Option<PathBuf>,
}

impl Default for Keymap {
    /// Out of the box.
    fn default() -> Self {
        Keymap {
            keys: Action::ALL
                .into_iter()
                .map(|a| (a, a.default_chords()))
                .collect(),
            store: None,
        }
    }
}

/// The defaults, to lend out to an editor in a test.
#[cfg(test)]
pub fn defaults() -> &'static Keymap {
    static DEFAULTS: std::sync::OnceLock<Keymap> = std::sync::OnceLock::new();
    DEFAULTS.get_or_init(Keymap::default)
}

impl Keymap {
    /// The keys as kept in the config folder, the defaults for what isn't
    /// there. Written there if it wasn't yet (so it can be found to edit),
    /// or if it holds more than the keys changed (written by an older
    /// version, with all of them): so keys changed out of the box reach
    /// whoever didn't change them.
    pub fn load() -> Self {
        // Tests keep to the defaults, and leave the user's file be.
        if cfg!(test) {
            return Keymap::default();
        }
        let store = crate::recent::config_folder().map(|folder| folder.join("keys.json"));
        let text = store
            .as_deref()
            .and_then(|store| std::fs::read_to_string(store).ok());
        let mut keymap = text.as_deref().map(Keymap::from_json).unwrap_or_default();
        keymap.store = store;
        // (One that can't be read is left for its writer to put right.)
        let read = |text: &str| serde_json::from_str::<serde_json::Value>(text).ok();
        let slimmer = |text: &str| read(text).is_some_and(|v| Some(v) != read(&keymap.to_json()));
        if text.as_deref().is_none_or(slimmer) {
            keymap.save();
        }
        keymap
    }

    /// From the file's text: the defaults, but for the actions it names.
    fn from_json(text: &str) -> Keymap {
        let mut keymap = Keymap::default();
        let Ok(saved) = serde_json::from_str::<BTreeMap<String, Vec<String>>>(text) else {
            return keymap;
        };
        for (name, keys) in saved {
            let Ok(action) = serde_json::from_value::<Action>(serde_json::Value::String(name))
            else {
                continue;
            };
            let chords = keys.iter().filter_map(|key| key.parse().ok()).collect();
            keymap.keys.insert(action, chords);
        }
        keymap
    }

    /// Only the keys changed: the rest follow the defaults, even as those
    /// change.
    fn to_json(&self) -> String {
        let saved: BTreeMap<String, Vec<String>> = self
            .keys
            .iter()
            .filter(|&(&action, _)| !self.is_default(action))
            .map(|(action, chords)| {
                let name = serde_json::to_value(action)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_default();
                (name, chords.iter().map(Chord::to_string).collect())
            })
            .collect();
        serde_json::to_string_pretty(&saved).unwrap_or_default()
    }

    /// Keeps it; not being able to is no reason to bother anyone.
    fn save(&self) {
        let Some(store) = &self.store else {
            return;
        };
        if let Some(folder) = store.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let _ = std::fs::write(store, self.to_json());
    }

    /// The keys for `action`.
    pub fn keys(&self, action: Action) -> &[Chord] {
        self.keys.get(&action).map_or(&[], Vec::as_slice)
    }

    /// Its first key, written out ("—" if none), for hints.
    pub fn first(&self, action: Action) -> String {
        self.keys(action)
            .first()
            .map_or_else(|| "—".to_string(), Chord::to_string)
    }

    /// What a key pressed does in `context` (or anywhere): one of the
    /// actions working there.
    pub fn action(&self, chord: &Chord, context: Context) -> Option<Action> {
        self.keys
            .iter()
            .filter(|(action, _)| {
                let c = action.context();
                c == context || c == Context::Global
            })
            .find(|(_, chords)| chords.contains(chord))
            .map(|(&action, _)| action)
    }

    /// Whether a key released (whatever modifiers) is one of `action`'s:
    /// to tell when a held key lets go.
    pub fn releases(&self, chord: &Chord, action: Action) -> bool {
        self.keys(action).iter().any(|key| key.same_key(chord))
    }

    /// Whether `action` has its keys out of the box.
    pub fn is_default(&self, action: Action) -> bool {
        self.keys(action) == action.default_chords()
    }

    /// Binds `chord` to `action` (instead of its key at `index`, else as
    /// another); taken from where it did something it now can't (the same
    /// or an overlapping context). Kept. Returns those it was taken from.
    pub fn bind(&mut self, action: Action, index: Option<usize>, chord: Chord) -> Vec<Action> {
        let mut taken = Vec::new();
        for (&other, chords) in self.keys.iter_mut() {
            if other != action
                && other.context().overlaps(action.context())
                && chords.contains(&chord)
            {
                chords.retain(|c| *c != chord);
                taken.push(other);
            }
        }
        let chords = self.keys.entry(action).or_default();
        match index.filter(|&i| i < chords.len()) {
            Some(i) => chords[i] = chord,
            None if !chords.contains(&chord) => chords.push(chord),
            None => {}
        }
        // Not twice.
        let mut seen = Vec::new();
        chords.retain(|c| {
            let new = !seen.contains(c);
            seen.push(c.clone());
            new
        });
        self.save();
        taken
    }

    /// Unbinds `action`'s key at `index`. Kept.
    pub fn unbind(&mut self, action: Action, index: usize) {
        if let Some(chords) = self.keys.get_mut(&action)
            && index < chords.len()
        {
            chords.remove(index);
            self.save();
        }
    }

    /// Back to its keys out of the box (and, if those are taken now, taken
    /// back). Kept.
    pub fn reset(&mut self, action: Action) {
        self.keys.insert(action, Vec::new());
        for chord in action.default_chords() {
            self.bind(action, None, chord);
        }
        self.save();
    }

    /// Everything back to out of the box. Kept.
    pub fn reset_all(&mut self) {
        self.keys = Keymap::default().keys;
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(text: &str) -> Chord {
        text.parse().unwrap()
    }

    #[test]
    fn chords_read_and_write_alike() {
        for text in ["G", "Ctrl+Shift+S", "Tab", "F1", "Alt+Delete"] {
            assert_eq!(chord(text).to_string(), text);
        }
        assert_eq!(chord("ctrl+shift+s"), chord("Ctrl+Shift+S"));
        assert!("Hyper+G".parse::<Chord>().is_err());
        assert!("Ctrl+".parse::<Chord>().is_err());
    }

    #[test]
    fn a_key_pressed_is_found_where_it_works() {
        let keys = Keymap::default();
        let g = chord("G");
        assert_eq!(keys.action(&g, Context::Shape), Some(Action::Grab));
        assert_eq!(keys.action(&g, Context::Paint), None);
        // C cuts edges in the shape mode, lightens in the paint mode.
        assert_eq!(
            keys.action(&chord("C"), Context::Shape),
            Some(Action::CutEdge)
        );
        assert_eq!(
            keys.action(&chord("C"), Context::Paint),
            Some(Action::Lightness)
        );
        assert_eq!(
            keys.action(&chord("Shift+C"), Context::Paint),
            Some(Action::Opacity)
        );
        // Everywhere.
        assert_eq!(
            keys.action(&chord("Ctrl+Z"), Context::Paint),
            Some(Action::Undo)
        );
        assert_eq!(
            keys.action(&chord("Ctrl+Y"), Context::Shape),
            Some(Action::Redo)
        );
        // Modifiers count.
        assert_eq!(keys.action(&chord("Ctrl+G"), Context::Shape), None);
    }

    #[test]
    fn binding_takes_a_key_from_where_it_clashes_only() {
        let mut keys = Keymap::default();
        // G was Grab's (shape); now Pipette's (paint): no clash.
        assert!(keys.bind(Action::Pipette, None, chord("G")).is_empty());
        assert_eq!(
            keys.action(&chord("G"), Context::Paint),
            Some(Action::Pipette)
        );
        assert_eq!(keys.action(&chord("G"), Context::Shape), Some(Action::Grab));
        // Instead of Grab's own: R taken from Rotate.
        assert_eq!(
            keys.bind(Action::Grab, Some(0), chord("R")),
            [Action::Rotate]
        );
        assert_eq!(keys.keys(Action::Grab), [chord("R")]);
        assert!(keys.keys(Action::Rotate).is_empty());
        // Everywhere clashes with anywhere.
        assert_eq!(keys.bind(Action::Undo, None, chord("D")), [Action::Delete]);

        keys.reset(Action::Rotate);
        assert_eq!(keys.keys(Action::Rotate), [chord("R")]);
        assert!(keys.keys(Action::Grab).is_empty());
        keys.unbind(Action::Redo, 1);
        assert_eq!(keys.keys(Action::Redo), [chord("Ctrl+Shift+Z")]);
        assert!(!keys.is_default(Action::Redo));
        keys.reset_all();
        assert_eq!(keys, Keymap::default());
    }

    #[test]
    fn kept_between_runs_leaving_out_what_it_cannot_read() {
        let mut keys = Keymap::default();
        keys.bind(Action::Grab, Some(0), chord("Shift+G"));
        let text = keys.to_json();
        assert!(
            text.contains(
                r#""grab": [
    "Shift+G"
  ]"#
            ),
            "{text}"
        );
        let read = Keymap::from_json(&text);
        assert_eq!(read.keys(Action::Grab), [chord("Shift+G")]);
        // Only what's changed: not the rest.
        assert!(!text.contains("rotate"), "{text}");
        assert_eq!(Keymap::default().to_json(), "{}");
        // A key taken from another action changes that one too.
        keys.bind(Action::Grab, None, chord("R"));
        let read = Keymap::from_json(&keys.to_json());
        assert_eq!(read.keys(Action::Rotate), []);

        // Unknown actions and keys left out; actions not there, defaults.
        let read = Keymap::from_json(r#"{"grab": ["Hyper+X", "Q"], "teleport": ["P"]}"#);
        assert_eq!(read.keys(Action::Grab), [chord("Q")]);
        assert_eq!(read.keys(Action::Rotate), [chord("R")]);
        assert_eq!(Keymap::from_json("not json"), Keymap::default());
    }
}
