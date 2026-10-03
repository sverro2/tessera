//! What the app does with each message, by area: each has messages of its
//! own and a handler for them.

mod background;
mod canvas;
mod export;
mod file;
mod keys;
mod layers;
mod menu;
mod page;
mod paint;
mod shape;
mod snap;

pub use background::{BackgroundMessage, Backgrounds, Field, Fields};
pub use export::{ExportMessage, ExportSettings, Format};
pub use file::Dropped;
pub use file::{FileMessage, Files};
pub use keys::KeysMessage;
pub use layers::{LayerAction, LayersPanel};
pub use menu::MenuMessage;
pub use page::{PageDialog, PageMessage};
pub use paint::{PaintMessage, Painting};
pub use shape::ShapeMessage;
pub use snap::SnapMessage;
