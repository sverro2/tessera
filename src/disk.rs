//! Reading and writing files, asking the user where first: off the
//! window's thread, as tasks.

use super::*;

/// Reads the file at `path`; if it can't, says why (with the path).
pub(super) async fn read_file(path: PathBuf) -> Result<(PathBuf, String), (PathBuf, String)> {
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok((path, text)),
        Err(error) => Err((path, error.to_string())),
    }
}

pub(super) async fn open_file() -> Result<Option<(PathBuf, String)>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Tessera", &[file::EXTENSION])
        .pick_file()
        .await
    else {
        return Ok(None);
    };

    let path = file.path().to_path_buf();
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;

    Ok(Some((path, text)))
}

/// Writes `text` to `path`, or to a file picked first (suggesting `name`).
pub(super) async fn save_file(
    path: Option<PathBuf>,
    name: String,
    text: String,
) -> Result<Option<PathBuf>, String> {
    let path = match path {
        Some(path) => path,
        None => {
            let Some(file) = rfd::AsyncFileDialog::new()
                .add_filter("Tessera", &[file::EXTENSION])
                .set_file_name(name)
                .save_file()
                .await
            else {
                return Ok(None);
            };
            let mut path = file.path().to_path_buf();
            if path.extension().is_none() {
                path.set_extension(file::EXTENSION);
            }
            path
        }
    };

    std::fs::write(&path, text).map_err(|e| e.to_string())?;

    Ok(Some(path))
}

pub(super) async fn pick_image() -> Result<Option<Vec<u8>>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("Images", &["png", "jpg", "jpeg", "webp", "gif", "bmp"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };

    std::fs::read(file.path())
        .map(Some)
        .map_err(|e| e.to_string())
}

pub(super) async fn save_svg(svg: String) -> Result<Option<PathBuf>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("SVG", &["svg"])
        .set_file_name("tessera.svg")
        .save_file()
        .await
    else {
        return Ok(None);
    };

    let path = file.path().to_path_buf();
    std::fs::write(&path, svg).map_err(|e| e.to_string())?;

    Ok(Some(path))
}

/// Asks where to save a PNG of `svg` (`scale` pixels to its unit, on
/// `backdrop`), and saves it there; rasterised on a thread of its own, so a
/// large one doesn't hold the window up.
pub(super) async fn save_png(
    svg: String,
    scale: f32,
    backdrop: raster::Backdrop,
) -> Result<Option<PathBuf>, String> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .add_filter("PNG", &["png"])
        .set_file_name("tessera.png")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    let path = file.path().to_path_buf();

    let (done, png) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = done.send(raster::png(&svg, scale, backdrop));
    });
    let png = png.await.map_err(|_| "rasterising stopped".to_string())??;
    std::fs::write(&path, png).map_err(|e| e.to_string())?;

    Ok(Some(path))
}
