//! The `.tessera` file format: the document and the view, as JSON.
//!
//! Every file says which `version` of the format it is. Opening reads that
//! first and upgrades older versions step by step to the current one, so
//! files keep opening as the format grows. Additions with a sensible default
//! (e.g. face colour, edge width) can go in as optional fields
//! (`#[serde(default)]`) without a new version: older files simply lack
//! them, and unknown fields are ignored. A new version is only needed when
//! existing fields change meaning; then add a `V2`, make it what `save`
//! writes, and turn a `V1` into a `V2` in `open`.

use iced::{Point, Vector};
use serde::{Deserialize, Serialize};

use crate::camera::Camera;
use crate::document::Document;

/// The file name extension.
pub const EXTENSION: &str = "tessera";

/// Written into every file, to tell ours from other JSON.
const FORMAT: &str = "tessera";

/// The version `save` writes.
const VERSION: u32 = 1;

/// What a file holds.
pub struct Contents {
    pub document: Document,
    pub camera: Camera,
}

/// Just enough to tell which version a file is.
#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// Version 1: the triangles and the view.
#[derive(Serialize, Deserialize)]
struct V1 {
    format: String,
    version: u32,
    view: View,
    vertices: Vec<[f32; 2]>,
    /// Corners, as indices into `vertices`.
    triangles: Vec<[usize; 3]>,
}

#[derive(Serialize, Deserialize)]
struct View {
    pan: [f32; 2],
    zoom: f32,
    /// Clockwise, in radians.
    rotation: f32,
}

pub fn save(document: &Document, camera: Camera) -> String {
    let (vertices, triangles) = document.to_parts();
    let file = V1 {
        format: FORMAT.to_string(),
        version: VERSION,
        view: View {
            pan: [camera.pan.x, camera.pan.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
        },
        vertices: vertices.iter().map(|p| [p.x, p.y]).collect(),
        triangles,
    };

    serde_json::to_string(&file).expect("serializable")
}

pub fn open(text: &str) -> Result<Contents, String> {
    let header: Header =
        serde_json::from_str(text).map_err(|_| "Not a Tessera file".to_string())?;
    if header.format != FORMAT {
        return Err("Not a Tessera file".to_string());
    }

    let file: V1 = match header.version {
        1 => serde_json::from_str(text).map_err(|e| format!("Damaged file: {e}"))?,
        version if version > VERSION => {
            return Err(format!(
                "Made with a newer Tessera (format version {version}); please update"
            ));
        }
        version => return Err(format!("Unknown format version {version}")),
    };

    let vertices = file
        .vertices
        .iter()
        .map(|&[x, y]| Point::new(x, y))
        .collect();
    let document =
        Document::from_parts(vertices, file.triangles).map_err(|e| format!("Damaged file: {e}"))?;

    let View {
        pan: [x, y],
        zoom,
        rotation,
    } = file.view;
    let finite = [x, y, zoom, rotation].iter().all(|n| n.is_finite());
    let camera = if finite && zoom > 0.0 {
        Camera {
            pan: Vector::new(x, y),
            zoom: zoom.clamp(Camera::MIN_ZOOM, Camera::MAX_ZOOM),
            rotation,
        }
    } else {
        Camera::default()
    };

    Ok(Contents { document, camera })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Edit;

    #[test]
    fn saving_and_opening_gives_back_the_same() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(100.0, 300.0),
                Point::new(300.0, 300.0),
                Point::new(200.0, 100.0),
            ],
            snap: 0.0,
        });
        document.apply(Edit::InsertVertex {
            at: Point::new(200.0, 230.0),
        });
        let camera = Camera {
            pan: Vector::new(12.5, -3.0),
            zoom: 1.75,
            rotation: 0.5,
        };

        let opened = open(&save(&document, camera)).unwrap();

        assert_eq!(opened.document.to_parts(), document.to_parts());
        assert_eq!(opened.camera.pan, camera.pan);
        assert_eq!(opened.camera.zoom, camera.zoom);
        assert_eq!(opened.camera.rotation, camera.rotation);
    }

    #[test]
    fn opens_version_1() {
        // A version 1 file as written by the first release of the format.
        // Keep this test as it is: it guards that old files keep opening.
        let text = r#"{"format":"tessera","version":1,
            "view":{"pan":[10.0,20.0],"zoom":2.0,"rotation":0.0},
            "vertices":[[0.0,0.0],[10.0,0.0],[5.0,10.0]],
            "triangles":[[0,1,2]]}"#;

        let opened = open(text).unwrap();
        assert_eq!(opened.document.triangle_ids().len(), 1);
        assert_eq!(opened.camera.zoom, 2.0);
    }

    #[test]
    fn ignores_fields_it_does_not_know() {
        let text = r#"{"format":"tessera","version":1,"style":{"fill":"red"},
            "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
            "vertices":[],"triangles":[]}"#;
        assert!(open(text).is_ok());
    }

    #[test]
    fn refuses_what_it_cannot_open() {
        let error = |text: &str| open(text).err().unwrap();

        assert_eq!(error("<svg/>"), "Not a Tessera file");
        assert_eq!(
            error(r#"{"format":"other","version":1}"#),
            "Not a Tessera file"
        );
        assert!(error(r#"{"format":"tessera","version":99}"#).contains("newer Tessera"));
        assert!(
            error(
                r#"{"format":"tessera","version":1,
                "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
                "vertices":[[0.0,0.0]],"triangles":[[0,1,2]]}"#
            )
            .contains("missing vertex")
        );
    }
}
