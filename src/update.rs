//! What the app does with each message, by area: each has messages of its
//! own and a handler for them.

mod background;
mod canvas;
mod export;
mod file;
mod keys;
mod layers;
mod menu;
mod paint;
mod shape;

pub use background::BackgroundMessage;
pub use export::ExportMessage;
pub use file::FileMessage;
pub use keys::KeysMessage;
pub use layers::LayerAction;
pub use menu::MenuMessage;
pub use paint::PaintMessage;
pub use shape::ShapeMessage;
