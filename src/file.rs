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

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::background::Background;
use crate::camera::Camera;
use crate::document::{Document, EdgeStyle};
use crate::paint;

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
    pub background: Option<Background>,
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
    /// Each triangle's colour (`#rrggbb`), or `null` if unpainted; in the
    /// order of `triangles`. Added later; files without them are unpainted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    colors: Vec<Option<String>>,
    /// The painted edges. Added later; files without them have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    edges: Vec<EdgeV1>,
    /// Added after the first release; files without one have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<BackgroundV1>,
}

/// A painted edge.
#[derive(Serialize, Deserialize)]
struct EdgeV1 {
    /// Its ends, as indices into `vertices`.
    ends: [usize; 2],
    /// `#rrggbb`.
    color: String,
    /// In world units.
    width: f32,
}

/// A background image, embedded.
#[derive(Serialize, Deserialize)]
struct BackgroundV1 {
    /// The image file (PNG, JPEG, …), base64 encoded.
    image: String,
    /// Where its centre is (world).
    center: [f32; 2],
    /// World units per image pixel.
    scale: f32,
    /// Clockwise, in radians.
    rotation: f32,
    opacity: f32,
    #[serde(default = "shown")]
    visible: bool,
}

fn shown() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct View {
    pan: [f32; 2],
    zoom: f32,
    /// Clockwise, in radians.
    rotation: f32,
}

pub fn save(document: &Document, camera: Camera, background: Option<&Background>) -> String {
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
        colors: if document.colors().any(|c| c.is_some()) {
            document.colors().map(|c| c.map(paint::to_hex)).collect()
        } else {
            Vec::new()
        },
        edges: {
            let used = document.unique_vertices();
            let index = |v| used.binary_search(&v).expect("used vertex");
            document
                .edge_styles()
                .map(|(a, b, style)| EdgeV1 {
                    ends: [index(a), index(b)],
                    color: paint::to_hex(style.color),
                    width: style.width,
                })
                .collect()
        },
        background: background.map(|background| BackgroundV1 {
            image: BASE64.encode(background.bytes()),
            center: [background.center.x, background.center.y],
            scale: background.scale,
            rotation: background.rotation,
            opacity: background.opacity,
            visible: background.visible,
        }),
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
    let mut document =
        Document::from_parts(vertices, file.triangles).map_err(|e| format!("Damaged file: {e}"))?;
    let colors = file
        .colors
        .iter()
        .map(|hex| match hex {
            Some(hex) => paint::from_hex(hex)
                .map(Some)
                .ok_or_else(|| format!("Damaged file: colour {hex:?}")),
            None => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;
    document.set_colors(colors);
    let edges = file
        .edges
        .iter()
        .map(|edge| {
            let color = paint::from_hex(&edge.color)
                .ok_or_else(|| format!("Damaged file: colour {:?}", edge.color))?;
            let [a, b] = edge.ends;
            let valid = edge.width.is_finite() && edge.width > 0.0;
            Ok((a, b, EdgeStyle {
                color,
                width: if valid { edge.width } else { 1.0 },
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    document.set_edge_styles(edges);

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

    let background = file.background.map(open_background).transpose()?;

    Ok(Contents {
        document,
        camera,
        background,
    })
}

fn open_background(saved: BackgroundV1) -> Result<Background, String> {
    let damaged = |e: String| format!("Damaged background image: {e}");
    let bytes = BASE64
        .decode(saved.image)
        .map_err(|e| damaged(e.to_string()))?;
    let mut background = Background::load(bytes).map_err(damaged)?;

    let [x, y] = saved.center;
    if [x, y, saved.scale, saved.rotation, saved.opacity]
        .iter()
        .all(|n| n.is_finite())
    {
        background.center = Point::new(x, y);
        background.scale = saved
            .scale
            .clamp(Background::MIN_SCALE, Background::MAX_SCALE);
        background.rotation = saved.rotation;
        background.opacity = saved.opacity.clamp(0.0, 1.0);
    }
    background.visible = saved.visible;
    Ok(background)
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
        document.apply(Edit::Paint {
            triangle: 1,
            color: Some(paint::PALETTE[3]),
        });
        let (a, b) = document.unique_edges()[2];
        assert!(document.apply(Edit::PaintEdge {
            a,
            b,
            style: Some(EdgeStyle {
                color: paint::PALETTE[0],
                width: 2.5,
            }),
        }));
        let camera = Camera {
            pan: Vector::new(12.5, -3.0),
            zoom: 1.75,
            rotation: 0.5,
        };

        let opened = open(&save(&document, camera, None)).unwrap();

        assert_eq!(opened.document.to_parts(), document.to_parts());
        assert_eq!(
            opened.document.colors().collect::<Vec<_>>(),
            document.colors().collect::<Vec<_>>()
        );
        assert!(opened.document.color(1).is_some());
        assert_eq!(
            opened.document.edge_styles().collect::<Vec<_>>(),
            document.edge_styles().collect::<Vec<_>>()
        );
        assert_eq!(opened.camera.pan, camera.pan);
        assert_eq!(opened.camera.zoom, camera.zoom);
        assert_eq!(opened.camera.rotation, camera.rotation);
    }

    #[test]
    fn keeps_the_background_image() {
        let mut background = Background::load(crate::background::tests::pixel()).unwrap();
        background.center = Point::new(5.0, 6.0);
        background.scale = 2.5;
        background.rotation = -0.25;
        background.opacity = 0.3;
        background.visible = false;

        let text = save(&Document::default(), Camera::default(), Some(&background));
        let opened = open(&text).unwrap().background.unwrap();

        assert_eq!(opened.bytes(), background.bytes());
        assert_eq!(opened.center, background.center);
        assert_eq!(opened.scale, 2.5);
        assert_eq!(opened.rotation, -0.25);
        assert_eq!(opened.opacity, 0.3);
        assert!(!opened.visible);
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
