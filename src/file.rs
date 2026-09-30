//! The `.tessera` file format: the layers and the view, as JSON.
//!
//! Every file says which `version` of the format it is. Opening reads that
//! first and upgrades older versions step by step to the current one, so
//! files keep opening as the format grows. Additions with a sensible default
//! (e.g. face colour, edge width) can go in as optional fields
//! (`#[serde(default)]`) without a new version: older files simply lack
//! them, and unknown fields are ignored. A new version is only needed when
//! existing fields change meaning; then add a `V3`, make it what `save`
//! writes, and turn a `V2` into a `V3` in `open`.
//!
//! - Version 1: one drawing.
//! - Version 2: layers (in groups), each with its own drawing.

use std::sync::Arc;

use iced::{Point, Vector};
use serde::{Deserialize, Serialize};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::background::Background;
use crate::camera::Camera;
use crate::document::{Document, EdgeStyle};
use crate::layers::{Crossfade, Group, Layer, Layers, Node};
use crate::paint;

/// The file name extension.
pub const EXTENSION: &str = "tessera";

/// Written into every file, to tell ours from other JSON.
const FORMAT: &str = "tessera";

/// The version `save` writes.
const VERSION: u32 = 2;

/// What a file holds.
pub struct Contents {
    pub layers: Layers,
    pub camera: Camera,
    pub background: Option<Background>,
}

/// Just enough to tell which version a file is.
#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// Version 1: one drawing and the view.
#[derive(Deserialize)]
struct V1 {
    view: View,
    #[serde(flatten)]
    drawing: DrawingV1,
    /// Added after the first release; files without one have none.
    #[serde(default)]
    background: Option<BackgroundV1>,
}

/// Version 2: layers and the view.
#[derive(Serialize, Deserialize)]
struct V2 {
    format: String,
    version: u32,
    view: View,
    /// Front first.
    layers: Vec<NodeV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<BackgroundV1>,
}

/// A layer or a group of them.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum NodeV2 {
    Layer {
        name: String,
        #[serde(default = "shown")]
        visible: bool,
        /// Whether painted edges blend into their neighbours' colours at
        /// their ends. Added later; files without it don't.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        crossfade_edges: bool,
        /// How far edges fade at their ends (world units); if not given,
        /// the default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crossfade_edge_width: Option<f32>,
        /// Whether its edges are hidden when painted and exported (their
        /// paint kept). Added later; files without it show them.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        hide_edges: bool,
        #[serde(flatten)]
        drawing: DrawingV1,
    },
    Group {
        name: String,
        #[serde(default = "shown")]
        visible: bool,
        /// Front first.
        children: Vec<NodeV2>,
    },
}

/// A drawing: the triangles and how they're painted.
#[derive(Serialize, Deserialize)]
struct DrawingV1 {
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

pub fn save(layers: &Layers, camera: Camera, background: Option<&Background>) -> String {
    fn node(this: &Node) -> NodeV2 {
        match this {
            Node::Layer(layer) => NodeV2::Layer {
                name: layer.name.clone(),
                visible: layer.visible,
                crossfade_edges: layer.crossfade.edges,
                crossfade_edge_width: (layer.crossfade.width != Crossfade::WIDTH)
                    .then_some(layer.crossfade.width),
                hide_edges: !layer.show_edges,
                drawing: save_drawing(&layer.document),
            },
            Node::Group(group) => NodeV2::Group {
                name: group.name.clone(),
                visible: group.visible,
                children: group.children.iter().map(node).collect(),
            },
        }
    }

    let file = V2 {
        format: FORMAT.to_string(),
        version: VERSION,
        view: View {
            pan: [camera.pan.x, camera.pan.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
        },
        layers: layers.nodes().iter().map(node).collect(),
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

fn save_drawing(document: &Document) -> DrawingV1 {
    let (vertices, triangles) = document.to_parts();
    let used = document.unique_vertices();
    let index = |v| used.binary_search(&v).expect("used vertex");
    DrawingV1 {
        vertices: vertices.iter().map(|p| [p.x, p.y]).collect(),
        triangles,
        colors: if document.colors().any(|c| c.is_some()) {
            document.colors().map(|c| c.map(paint::to_hex)).collect()
        } else {
            Vec::new()
        },
        edges: document
            .edge_styles()
            .map(|(a, b, style)| EdgeV1 {
                ends: [index(a), index(b)],
                color: paint::to_hex(style.color),
                width: style.width,
            })
            .collect(),
    }
}

pub fn open(text: &str) -> Result<Contents, String> {
    let header: Header =
        serde_json::from_str(text).map_err(|_| "Not a Tessera file".to_string())?;
    if header.format != FORMAT {
        return Err("Not a Tessera file".to_string());
    }
    let damaged = |e: serde_json::Error| format!("Damaged file: {e}");

    let (view, layers, background) = match header.version {
        1 => {
            let file: V1 = serde_json::from_str(text).map_err(damaged)?;
            let layer = NodeV2::Layer {
                name: "Layer 1".to_string(),
                visible: true,
                crossfade_edges: false,
                crossfade_edge_width: None,
                hide_edges: false,
                drawing: file.drawing,
            };
            (file.view, vec![layer], file.background)
        }
        2 => {
            let file: V2 = serde_json::from_str(text).map_err(damaged)?;
            (file.view, file.layers, file.background)
        }
        version if version > VERSION => {
            return Err(format!(
                "Made with a newer Tessera (format version {version}); please update"
            ));
        }
        version => return Err(format!("Unknown format version {version}")),
    };

    fn node(saved: NodeV2) -> Result<Node, String> {
        Ok(match saved {
            NodeV2::Layer {
                name,
                visible,
                crossfade_edges,
                crossfade_edge_width,
                hide_edges,
                drawing,
            } => Node::Layer(Layer {
                // Made unique by `Layers::from_nodes`.
                id: 0,
                name,
                visible,
                crossfade: Crossfade {
                    edges: crossfade_edges,
                    width: fade_width(crossfade_edge_width),
                },
                show_edges: !hide_edges,
                document: Arc::new(open_drawing(drawing)?),
            }),
            NodeV2::Group {
                name,
                visible,
                children,
            } => Node::Group(Group {
                id: 0,
                name,
                visible,
                expanded: true,
                children: children.into_iter().map(node).collect::<Result<_, _>>()?,
            }),
        })
    }
    let layers = Layers::from_nodes(layers.into_iter().map(node).collect::<Result<_, _>>()?);

    let View {
        pan: [x, y],
        zoom,
        rotation,
    } = view;
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

    let background = background.map(open_background).transpose()?;

    Ok(Contents {
        layers,
        camera,
        background,
    })
}

/// A saved fade width, if it's a usable one; else the default.
fn fade_width(saved: Option<f32>) -> f32 {
    saved
        .filter(|width| width.is_finite() && *width > 0.0)
        .unwrap_or(Crossfade::WIDTH)
}

fn open_drawing(drawing: DrawingV1) -> Result<Document, String> {
    let vertices = drawing
        .vertices
        .iter()
        .map(|&[x, y]| Point::new(x, y))
        .collect();
    let mut document = Document::from_parts(vertices, drawing.triangles)
        .map_err(|e| format!("Damaged file: {e}"))?;
    let colors = drawing
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
    let edges = drawing
        .edges
        .iter()
        .map(|edge| {
            let color = paint::from_hex(&edge.color)
                .ok_or_else(|| format!("Damaged file: colour {:?}", edge.color))?;
            let [a, b] = edge.ends;
            let valid = edge.width.is_finite() && edge.width > 0.0;
            Ok((
                a,
                b,
                EdgeStyle {
                    color,
                    width: if valid { edge.width } else { 1.0 },
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    document.set_edge_styles(edges);
    Ok(document)
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

    fn drawing() -> Document {
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
        document
    }

    #[test]
    fn saving_and_opening_gives_back_the_same() {
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = Arc::new(drawing());
        let front = layers.add_layer(first);
        layers.rename(front, "Front <&>".into());
        layers.set_visible(front, false);
        layers.set_crossfade(
            first,
            Crossfade {
                edges: true,
                width: 3.0,
            },
        );
        layers.set_show_edges(first, false);
        let group = layers.group(first).unwrap();
        layers.rename(group, "Back".into());
        let camera = Camera {
            pan: Vector::new(12.5, -3.0),
            zoom: 1.75,
            rotation: 0.5,
        };

        let opened = open(&save(&layers, camera, None)).unwrap();

        let rows = |layers: &Layers| {
            layers
                .rows()
                .iter()
                .map(|row| (row.depth, row.name.to_string(), row.visible))
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&opened.layers), rows(&layers));
        let crossfades = |layers: &Layers| {
            layers
                .layers()
                .iter()
                .map(|layer| layer.crossfade)
                .collect::<Vec<_>>()
        };
        assert_eq!(crossfades(&opened.layers), crossfades(&layers));
        let edges_shown = |layers: &Layers| {
            layers
                .layers()
                .iter()
                .map(|layer| layer.show_edges)
                .collect::<Vec<_>>()
        };
        assert_eq!(edges_shown(&opened.layers), [true, false]);
        let (document, saved) = (
            &opened.layers.layers()[1].document,
            &layers.layers()[1].document,
        );
        assert_eq!(document.to_parts(), saved.to_parts());
        assert_eq!(
            document.colors().collect::<Vec<_>>(),
            saved.colors().collect::<Vec<_>>()
        );
        assert!(document.color(1).is_some());
        assert_eq!(
            document.edge_styles().collect::<Vec<_>>(),
            saved.edge_styles().collect::<Vec<_>>()
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

        let text = save(&Layers::default(), Camera::default(), Some(&background));
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
        // Keep this file text as it is: it guards that old files keep
        // opening.
        let text = r#"{"format":"tessera","version":1,
            "view":{"pan":[10.0,20.0],"zoom":2.0,"rotation":0.0},
            "vertices":[[0.0,0.0],[10.0,0.0],[5.0,10.0]],
            "triangles":[[0,1,2]]}"#;

        let opened = open(text).unwrap();
        let layers = opened.layers.layers();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].document.triangle_ids().len(), 1);
        assert_eq!(opened.camera.zoom, 2.0);
    }

    #[test]
    fn opens_version_1_with_colours_and_background() {
        let background = Background::load(crate::background::tests::pixel()).unwrap();
        let image = BASE64.encode(background.bytes());
        let text = format!(
            r##"{{"format":"tessera","version":1,
            "view":{{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0}},
            "vertices":[[0.0,0.0],[10.0,0.0],[5.0,10.0]],
            "triangles":[[0,1,2]],"colors":["#ff0000"],
            "edges":[{{"ends":[0,1],"color":"#00ff00","width":2.0}}],
            "background":{{"image":"{image}","center":[0.0,0.0],"scale":1.0,
                "rotation":0.0,"opacity":0.5}}}}"##
        );

        let opened = open(&text).unwrap();
        let document = &opened.layers.layers()[0].document;
        assert!(document.color(0).is_some());
        assert_eq!(document.edge_styles().count(), 1);
        assert_eq!(opened.background.unwrap().opacity, 0.5);
    }

    #[test]
    fn ignores_fields_it_does_not_know() {
        let text = r#"{"format":"tessera","version":2,"style":{"fill":"red"},
            "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
            "layers":[{"kind":"layer","name":"A","vertices":[],"triangles":[],"x":1}]}"#;
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
