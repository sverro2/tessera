//! The `.tessera` file format: a zip of the drawing, as JSON, and the
//! background images it uses.
//!
//! - `drawing.json`: the layers (in groups), the view, the paint used, and
//!   the document size (if one's set).
//!   Colours and edge styles are kept once, in tables, and referred to by
//!   their place in them. Each layer's geometry is compact: its points as
//!   `[x0, y0, x1, y1, …]`, its faces as `[a, b, c, colour]` (`[a, b, c]` if
//!   erased), its painted edges as `[a, b, style]`. What's as it is by
//!   default is left out.
//! - `images/<hash>.<ext>`: each background image as it was picked (PNG,
//!   JPEG, …), stored as is (it's compressed already), once however many
//!   layers use it; named by its contents.
//!
//! The JSON says which `version` of the format it is. Additions with a
//! sensible default can go in as optional fields (`#[serde(default)]`)
//! without a new version: older files simply lack them, and fields not
//! known are ignored. A new version is only needed when existing fields
//! change meaning; then opening upgrades older versions to it.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use iced::{Point, Vector};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::background::Background;
use crate::camera::Camera;
use crate::document::{Document, EdgeStyle, Mirror};
use crate::layers::{Crossfade, Group, Layer, Layers, Node, NodeId};
use crate::page::Page;
use crate::paint;

/// The file name extension.
pub const EXTENSION: &str = "tessera";

/// Written into every file, to tell ours from other JSON.
const FORMAT: &str = "tessera";

/// The version `save` writes.
const VERSION: u32 = 1;

/// The drawing, in the zip.
const DRAWING: &str = "drawing.json";

/// What a file holds.
pub struct Contents {
    pub layers: Layers,
    pub camera: Camera,
    /// The layers' background images, by layer.
    pub backgrounds: HashMap<NodeId, Background>,
    /// The layer worked on when it was saved (if it's still there).
    pub current: Option<NodeId>,
}

/// `drawing.json`.
#[derive(Serialize, Deserialize)]
struct Drawing {
    format: String,
    version: u32,
    view: View,
    /// The faces' colours (`#rrggbb`), each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    colors: Vec<String>,
    /// The painted edges' styles, each once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    edge_styles: Vec<SavedEdgeStyle>,
    /// Front first.
    layers: Vec<SavedNode>,
    /// The document size, if one's set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page: Option<SavedPage>,
}

/// The document size: its centre and size (world units).
#[derive(Serialize, Deserialize)]
struct SavedPage {
    center: [f32; 2],
    size: [f32; 2],
}

#[derive(Serialize, Deserialize)]
struct View {
    pan: [f32; 2],
    zoom: f32,
    /// Clockwise, in radians.
    rotation: f32,
    /// The layer worked on: which, front first (as listed, groups open).
    #[serde(default)]
    layer: usize,
}

#[derive(Serialize, Deserialize, PartialEq)]
struct SavedEdgeStyle {
    /// `#rrggbb`.
    color: String,
    /// In world units.
    width: f32,
}

/// A layer or a group of them.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum SavedNode {
    Layer(SavedLayer),
    Group {
        name: String,
        #[serde(default, skip_serializing_if = "is_false")]
        hidden: bool,
        /// Front first.
        children: Vec<SavedNode>,
    },
}

#[derive(Serialize, Deserialize)]
struct SavedLayer {
    name: String,
    #[serde(default, skip_serializing_if = "is_false")]
    hidden: bool,
    /// Its edges hidden when painted and exported (their paint kept).
    #[serde(default, skip_serializing_if = "is_false")]
    edges_hidden: bool,
    /// How its painted edges blend into each other, if not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    crossfade: Option<SavedCrossfade>,
    /// The mirror it's mirrored across (until applied), as two points on
    /// it: `[ax, ay, bx, by]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mirror: Option<[f32; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    background: Option<SavedBackground>,
    /// `[x0, y0, x1, y1, …]`.
    #[serde(default)]
    points: Vec<f32>,
    /// `[a, b, c, colour]` (an index into `colors`), or `[a, b, c]` if
    /// erased; corners are indices into the points.
    #[serde(default)]
    faces: Vec<Vec<usize>>,
    /// The painted edges: `[a, b, style]` (an index into `edge_styles`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    edges: Vec<[usize; 3]>,
}

#[derive(Serialize, Deserialize)]
struct SavedCrossfade {
    edges: bool,
    /// How far (world units) from their ends edges fade.
    width: f32,
}

/// A layer's background image, placed.
#[derive(Serialize, Deserialize)]
struct SavedBackground {
    /// The image, in the zip.
    image: String,
    /// Where its centre is (world).
    center: [f32; 2],
    /// World units per image pixel.
    scale: f32,
    /// Clockwise, in radians.
    rotation: f32,
    opacity: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    hidden: bool,
}

fn is_false(b: &bool) -> bool {
    !b
}

/// Everything, as a `.tessera` file; `current` the layer worked on.
pub fn save(
    layers: &Layers,
    camera: Camera,
    backgrounds: &HashMap<NodeId, Background>,
    current: NodeId,
) -> Vec<u8> {
    let mut saving = Saving {
        backgrounds,
        colors: Vec::new(),
        edge_styles: Vec::new(),
        images: Vec::new(),
    };
    let nodes = layers
        .nodes()
        .iter()
        .map(|node| saving.node(node))
        .collect();
    let drawing = Drawing {
        format: FORMAT.to_string(),
        version: VERSION,
        view: View {
            pan: [camera.pan.x, camera.pan.y],
            zoom: camera.zoom,
            rotation: camera.rotation,
            layer: layers
                .layers()
                .iter()
                .position(|layer| layer.id == current)
                .unwrap_or(0),
        },
        colors: saving.colors,
        edge_styles: saving.edge_styles,
        layers: nodes,
        page: layers.page().map(|page| SavedPage {
            center: [page.center.x, page.center.y],
            size: [page.size.width, page.size.height],
        }),
    };

    // In memory: nothing to go wrong but running out of it.
    let json = tidy(&serde_json::to_string_pretty(&drawing).expect("serializable"));
    let packed = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    // Image files are compressed already.
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut add = |name: &str, options, bytes: &[u8]| {
        zip.start_file(name, options)
            .map_err(std::io::Error::other)
            .and_then(|()| zip.write_all(bytes))
            .expect("writing to memory");
    };
    add(DRAWING, packed, json.as_bytes());
    for (name, background) in &saving.images {
        add(name, stored, background.bytes());
    }
    zip.finish().expect("writing to memory").into_inner()
}

/// Saving: the paint and images met so far, each once.
struct Saving<'a> {
    backgrounds: &'a HashMap<NodeId, Background>,
    colors: Vec<String>,
    edge_styles: Vec<SavedEdgeStyle>,
    /// Their names in the zip, and the images.
    images: Vec<(String, &'a Background)>,
}

impl<'a> Saving<'a> {
    fn node(&mut self, node: &Node) -> SavedNode {
        match node {
            Node::Layer(layer) => SavedNode::Layer(self.layer(layer)),
            Node::Group(group) => SavedNode::Group {
                name: group.name.clone(),
                hidden: !group.visible,
                children: group
                    .children
                    .iter()
                    .map(|child| self.node(child))
                    .collect(),
            },
        }
    }

    fn layer(&mut self, layer: &Layer) -> SavedLayer {
        let document = &layer.document;
        let (points, triangles) = document.to_parts();
        let used = document.unique_vertices();
        let index = |v| used.binary_search(&v).expect("used vertex");
        let faces = triangles
            .iter()
            .zip(document.colors())
            .map(|(&[a, b, c], color)| {
                let mut face = vec![a, b, c];
                face.extend(color.map(|color| self.color(paint::to_hex(color))));
                face
            })
            .collect();
        let edges = document
            .edge_styles()
            .map(|(a, b, style)| {
                let style = SavedEdgeStyle {
                    color: paint::to_hex(style.color),
                    width: style.width,
                };
                [index(a), index(b), self.edge_style(style)]
            })
            .collect();
        SavedLayer {
            name: layer.name.clone(),
            hidden: !layer.visible,
            edges_hidden: !layer.show_edges,
            crossfade: (layer.crossfade != Crossfade::default()).then_some(SavedCrossfade {
                edges: layer.crossfade.edges,
                width: layer.crossfade.width,
            }),
            mirror: layer.mirror.map(|Mirror { a, b }| [a.x, a.y, b.x, b.y]),
            background: self
                .backgrounds
                .get(&layer.id)
                .map(|background| self.background(background)),
            points: points.iter().flat_map(|p| [p.x, p.y]).collect(),
            faces,
            edges,
        }
    }

    fn color(&mut self, color: String) -> usize {
        index_of(&mut self.colors, color)
    }

    fn edge_style(&mut self, style: SavedEdgeStyle) -> usize {
        index_of(&mut self.edge_styles, style)
    }

    fn background(&mut self, background: &'a Background) -> SavedBackground {
        // The same image (shared, or alike) is kept once.
        let kept = self
            .images
            .iter()
            .find(|(_, image)| image.same_image(background) || image.bytes() == background.bytes());
        let image = match kept {
            Some((name, _)) => name.clone(),
            None => {
                let name = image_name(background.bytes());
                self.images.push((name.clone(), background));
                name
            }
        };
        SavedBackground {
            image,
            center: [background.center.x, background.center.y],
            scale: background.scale,
            rotation: background.rotation,
            opacity: background.opacity,
            hidden: !background.visible,
        }
    }
}

/// Pretty JSON, but with each array of plain values (numbers, strings) on
/// one line, `[1, 2, 3]`, rather than a value a line: a face a line, the
/// points a line; readable, and short.
fn tidy(pretty: &str) -> String {
    let chars: Vec<char> = pretty.chars().collect();
    // Where the string starting at `i` ends (past its closing quote).
    let past_string = |mut i: usize| {
        i += 1;
        while i < chars.len() && chars[i] != '"' {
            i += if chars[i] == '\\' { 2 } else { 1 };
        }
        (i + 1).min(chars.len())
    };
    // Where the array starting at `i` ends, if nothing's nested in it.
    let flat_end = |i: usize| {
        let mut j = i + 1;
        while j < chars.len() {
            match chars[j] {
                '"' => j = past_string(j),
                '[' | '{' => return None,
                ']' => return Some(j),
                _ => j += 1,
            }
        }
        None
    };
    let mut out = String::with_capacity(pretty.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                let end = past_string(i);
                out.extend(&chars[i..end]);
                i = end;
            }
            '[' if let Some(end) = flat_end(i) => {
                out.push('[');
                let mut j = i + 1;
                while j < end {
                    match chars[j] {
                        '"' => {
                            let past = past_string(j);
                            out.extend(&chars[j..past]);
                            j = past;
                        }
                        ',' => {
                            out.push_str(", ");
                            j += 1;
                        }
                        c if c.is_whitespace() => j += 1,
                        c => {
                            out.push(c);
                            j += 1;
                        }
                    }
                }
                out.push(']');
                i = end + 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Where `item` is in `items`; put last if it isn't there yet.
fn index_of<T: PartialEq>(items: &mut Vec<T>, item: T) -> usize {
    match items.iter().position(|other| *other == item) {
        Some(i) => i,
        None => {
            items.push(item);
            items.len() - 1
        }
    }
}

/// An image file's name in the zip: by its contents (so the same image is
/// named alike), with the extension of its format.
fn image_name(bytes: &[u8]) -> String {
    // FNV-1a: plenty to tell a drawing's few images apart.
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    let extension = image::guess_format(bytes)
        .ok()
        .and_then(|format| format.extensions_str().first().copied())
        .unwrap_or("img");
    format!("images/{hash:016x}.{extension}")
}

/// A `.tessera` file's contents; if it can't be read, why.
pub fn open(bytes: &[u8]) -> Result<Contents, String> {
    let not_ours = || "Not a Tessera file".to_string();
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|_| not_ours())?;
    let mut json = Vec::new();
    zip.by_name(DRAWING)
        .map_err(|_| not_ours())?
        .read_to_end(&mut json)
        .map_err(|e| format!("Damaged file: {e}"))?;

    /// Just enough to tell which version it is.
    #[derive(Deserialize)]
    struct Header {
        format: String,
        version: u32,
    }
    let header: Header = serde_json::from_slice(&json).map_err(|_| not_ours())?;
    if header.format != FORMAT {
        return Err(not_ours());
    }
    if header.version > VERSION {
        return Err(format!(
            "Made with a newer Tessera (format version {}); please update",
            header.version
        ));
    }
    if header.version != VERSION {
        return Err(format!("Unknown format version {}", header.version));
    }
    let drawing: Drawing =
        serde_json::from_slice(&json).map_err(|e| format!("Damaged file: {e}"))?;

    let colors = drawing
        .colors
        .iter()
        .map(|hex| paint::from_hex(hex).ok_or_else(|| format!("Damaged file: colour {hex:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    let edge_styles = drawing
        .edge_styles
        .iter()
        .map(|style| {
            let color = paint::from_hex(&style.color)
                .ok_or_else(|| format!("Damaged file: colour {:?}", style.color))?;
            let width = if style.width.is_finite() && style.width > 0.0 {
                style.width
            } else {
                crate::document::DEFAULT_EDGE.width
            };
            Ok(EdgeStyle { color, width })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut opening = Opening {
        zip,
        colors,
        edge_styles,
        images: HashMap::new(),
        backgrounds: Vec::new(),
    };
    let nodes = drawing
        .layers
        .into_iter()
        .map(|node| opening.node(node))
        .collect::<Result<_, _>>()?;
    let mut layers = Layers::from_nodes(nodes);
    // One that makes no sense is left out.
    let page = drawing.page.and_then(|SavedPage { center, size }| {
        let page = Page {
            center: Point::new(center[0], center[1]),
            size: iced::Size::new(size[0], size[1]),
        };
        (center.iter().all(|n| n.is_finite()) && Page::fits(page.size)).then_some(page)
    });
    layers.set_page(page);
    // Laid out as the layers are: their ids now theirs.
    let ids: Vec<_> = layers.layers().iter().map(|layer| layer.id).collect();
    let backgrounds = ids
        .iter()
        .zip(opening.backgrounds)
        .filter_map(|(&id, background)| Some((id, background?)))
        .collect();

    let View {
        pan: [x, y],
        zoom,
        rotation,
        layer,
    } = drawing.view;
    let finite = [x, y, zoom, rotation].iter().all(|n| n.is_finite());
    let camera = if finite && zoom > 0.0 {
        Camera {
            pan: Vector::new(x, y),
            zoom: zoom.clamp(Camera::MIN_ZOOM, Camera::MAX_ZOOM),
            rotation,
            image: None,
        }
    } else {
        Camera::default()
    };

    Ok(Contents {
        current: ids.get(layer).copied(),
        layers,
        camera,
        backgrounds,
    })
}

/// Opening: the paint tables, and the images read so far (each once, for
/// the layers placing it to share).
struct Opening<'a> {
    zip: ZipArchive<Cursor<&'a [u8]>>,
    colors: Vec<iced::Color>,
    edge_styles: Vec<EdgeStyle>,
    images: HashMap<String, Background>,
    /// Each layer's, in the order of [`Layers::layers`].
    backgrounds: Vec<Option<Background>>,
}

impl Opening<'_> {
    fn node(&mut self, saved: SavedNode) -> Result<Node, String> {
        Ok(match saved {
            SavedNode::Layer(layer) => Node::Layer(self.layer(layer)?),
            SavedNode::Group {
                name,
                hidden,
                children,
            } => Node::Group(Group {
                // Made unique by `Layers::from_nodes`.
                id: 0,
                name,
                visible: !hidden,
                expanded: true,
                children: children
                    .into_iter()
                    .map(|child| self.node(child))
                    .collect::<Result<_, _>>()?,
            }),
        })
    }

    fn layer(&mut self, saved: SavedLayer) -> Result<Layer, String> {
        let damaged = |what: String| format!("Damaged file: {what}");
        if !saved.points.len().is_multiple_of(2) {
            return Err(damaged("points come in pairs".into()));
        }
        let points = saved
            .points
            .chunks(2)
            .map(|p| Point::new(p[0], p[1]))
            .collect();
        let mut triangles = Vec::with_capacity(saved.faces.len());
        let mut colors = Vec::with_capacity(saved.faces.len());
        for face in &saved.faces {
            let (corners, color) = match face[..] {
                [a, b, c] => ([a, b, c], None),
                [a, b, c, color] => {
                    let color = self.colors.get(color).copied();
                    let color = color.ok_or_else(|| damaged(format!("colour of {face:?}")))?;
                    ([a, b, c], Some(color))
                }
                _ => return Err(damaged(format!("face {face:?}"))),
            };
            triangles.push(corners);
            colors.push(color);
        }
        let mut document = Document::from_parts(points, triangles).map_err(damaged)?;
        document.set_colors(colors);
        let edges = saved
            .edges
            .iter()
            .map(|&[a, b, style]| {
                let found = self.edge_styles.get(style).copied();
                let found = found.ok_or_else(|| damaged(format!("edge style {style}")))?;
                Ok((a, b, found))
            })
            .collect::<Result<Vec<_>, String>>()?;
        document.set_edge_styles(edges);

        let background = saved
            .background
            .map(|background| self.background(background))
            .transpose()?;
        self.backgrounds.push(background);

        let crossfade = match saved.crossfade {
            Some(SavedCrossfade { edges, width }) if width.is_finite() && width > 0.0 => {
                Crossfade { edges, width }
            }
            Some(SavedCrossfade { edges, .. }) => Crossfade {
                edges,
                ..Crossfade::default()
            },
            None => Crossfade::default(),
        };
        Ok(Layer {
            // Made unique by `Layers::from_nodes`.
            id: 0,
            name: saved.name,
            visible: !saved.hidden,
            crossfade,
            show_edges: !saved.edges_hidden,
            document: Arc::new(document),
            mirror: saved
                .mirror
                .filter(|m| m.iter().all(|c| c.is_finite()))
                .map(|[ax, ay, bx, by]| Mirror {
                    a: Point::new(ax, ay),
                    b: Point::new(bx, by),
                })
                .filter(|m| m.a != m.b),
        })
    }

    /// A background image, read from the zip (once), placed as saved.
    fn background(&mut self, saved: SavedBackground) -> Result<Background, String> {
        let damaged = |e: String| format!("Damaged background image: {e}");
        let image = match self.images.get(&saved.image) {
            Some(image) => image.clone(),
            None => {
                let mut bytes = Vec::new();
                self.zip
                    .by_name(&saved.image)
                    .map_err(|e| damaged(e.to_string()))?
                    .read_to_end(&mut bytes)
                    .map_err(|e| damaged(e.to_string()))?;
                let image = Background::load(bytes).map_err(damaged)?;
                self.images.insert(saved.image.clone(), image.clone());
                image
            }
        };
        Ok(place(image, &saved))
    }
}

/// `background` placed as saved (where that's usable).
fn place(mut background: Background, saved: &SavedBackground) -> Background {
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
    background.visible = !saved.hidden;
    background
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Edit;
    use iced::Color;

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
        // One face erased, one edge too.
        document.apply(Edit::Paint {
            triangle: 2,
            color: None,
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
        let (a, b) = document.unique_edges()[0];
        assert!(document.apply(Edit::PaintEdge { a, b, style: None }));
        document
    }

    /// The drawing, as written.
    fn json(file: &[u8]) -> String {
        let mut zip = ZipArchive::new(Cursor::new(file)).unwrap();
        let mut text = String::new();
        zip.by_name(DRAWING)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    /// `text` as `drawing.json`, zipped.
    fn zipped(text: &str) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(DRAWING, SimpleFileOptions::default())
            .unwrap();
        zip.write_all(text.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
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
        let mirror = Mirror {
            a: Point::new(1.0, 2.0),
            b: Point::new(3.0, 4.0),
        };
        layers.set_mirror(first, Some(mirror));
        let group = layers.group(first).unwrap();
        layers.rename(group, "Back".into());
        let page = Page {
            center: Point::new(40.0, -25.5),
            size: iced::Size::new(2480.0, 3508.0),
        };
        layers.set_page(Some(page));
        let camera = Camera {
            pan: Vector::new(12.5, -3.0),
            zoom: 1.75,
            rotation: 0.5,
            image: None,
        };

        let opened = open(&save(&layers, camera, &HashMap::new(), first)).unwrap();

        let rows = |layers: &Layers| {
            layers
                .rows()
                .iter()
                .map(|row| (row.depth, row.name.to_string(), row.visible))
                .collect::<Vec<_>>()
        };
        assert_eq!(rows(&opened.layers), rows(&layers));
        assert_eq!(opened.layers.page(), Some(page));
        let settings = |layers: &Layers| {
            layers
                .layers()
                .iter()
                .map(|layer| (layer.crossfade, layer.show_edges, layer.mirror))
                .collect::<Vec<_>>()
        };
        assert_eq!(settings(&opened.layers), settings(&layers));
        let (document, saved) = (
            &opened.layers.layers()[1].document,
            &layers.layers()[1].document,
        );
        assert_eq!(document.to_parts(), saved.to_parts());
        assert_eq!(
            document.colors().collect::<Vec<_>>(),
            saved.colors().collect::<Vec<_>>()
        );
        assert_eq!(document.color(2), None);
        assert_eq!(
            document.edge_styles().collect::<Vec<_>>(),
            saved.edge_styles().collect::<Vec<_>>()
        );
        assert_eq!(opened.camera.pan, camera.pan);
        assert_eq!(opened.camera.zoom, camera.zoom);
        assert_eq!(opened.camera.rotation, camera.rotation);
        // The layer worked on.
        assert_eq!(opened.current, Some(opened.layers.layers()[1].id));
    }

    #[test]
    fn opacity_is_kept_and_left_out_when_opaque() {
        let mut document = drawing();
        let see_through = Color::from_rgba8(0x12, 0x34, 0x56, 0.25);
        assert!(document.apply(Edit::Paint {
            triangle: 0,
            color: Some(see_through),
        }));
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = Arc::new(document);
        let saved = save(&layers, Camera::default(), &HashMap::new(), first);
        let opened = open(&saved).unwrap();
        let colour = opened.layers.layers()[0].document.color(0).unwrap();
        assert_eq!(paint::to_hex(colour), paint::to_hex(see_through));
        // Opaque ones stay so (written without an opacity: see `to_hex`).
        let opaque = opened.layers.layers()[0].document.color(1);
        assert_eq!(opaque, Some(paint::PALETTE[3]));
    }

    #[test]
    fn paint_is_kept_once_and_defaults_left_out() {
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = Arc::new(drawing());
        let text = json(&save(&layers, Camera::default(), &HashMap::new(), first));
        let drawing: serde_json::Value = serde_json::from_str(&text).unwrap();
        // The default colour and the one painted; the default edge style
        // and the one painted.
        assert_eq!(drawing["colors"].as_array().unwrap().len(), 2, "{text}");
        assert_eq!(
            drawing["edge_styles"].as_array().unwrap().len(),
            2,
            "{text}"
        );
        let layer = &drawing["layers"][0];
        // An erased face: no colour.
        let faces = layer["faces"].as_array().unwrap();
        let erased = faces
            .iter()
            .filter(|face| face.as_array().unwrap().len() == 3)
            .count();
        assert_eq!(erased, 1, "{text}");
        for unset in [
            "hidden",
            "edges_hidden",
            "crossfade",
            "mirror",
            "background",
        ] {
            assert!(layer.get(unset).is_none(), "{unset}: {text}");
        }
    }

    #[test]
    fn keeps_each_layers_background_image_storing_each_image_once() {
        let mut background = Background::load(crate::background::tests::pixel()).unwrap();
        background.center = Point::new(5.0, 6.0);
        background.scale = 2.5;
        background.rotation = -0.25;
        background.opacity = 0.3;
        background.visible = false;
        // Two layers with it, placed apart (as a duplicate's is); one
        // without.
        let mut layers = Layers::default();
        let first = layers.first_layer();
        let second = layers.add_layer(first);
        layers.add_layer(second);
        let mut moved = background.clone();
        moved.center = Point::new(-1.0, 0.0);
        let backgrounds = HashMap::from([(first, background.clone()), (second, moved)]);

        let file = save(&layers, Camera::default(), &backgrounds, first);
        // The image once, as it is.
        let zip = ZipArchive::new(Cursor::new(&file[..])).unwrap();
        let images: Vec<_> = zip
            .file_names()
            .filter(|name| name.starts_with("images/"))
            .collect();
        assert_eq!(images.len(), 1, "{images:?}");
        assert!(images[0].ends_with(".png"), "{images:?}");

        let opened = open(&file).unwrap();
        let ids: Vec<_> = opened.layers.layers().iter().map(|l| l.id).collect();
        // Front first: the third, the second, the first.
        assert!(!opened.backgrounds.contains_key(&ids[0]));
        let (two, one) = (&opened.backgrounds[&ids[1]], &opened.backgrounds[&ids[2]]);
        assert_eq!(one.bytes(), background.bytes());
        assert!(one.same_image(two));
        assert_eq!(one.center, background.center);
        assert_eq!(two.center, Point::new(-1.0, 0.0));
        assert_eq!(one.scale, 2.5);
        assert_eq!(one.rotation, -0.25);
        assert_eq!(one.opacity, 0.3);
        assert!(!one.visible);
    }

    #[test]
    fn plain_arrays_go_on_one_line() {
        let pretty = "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": [\n    [\n      3,\n      \"x, ]\"\n    ]\n  ]\n}";
        assert_eq!(
            tidy(pretty),
            "{\n  \"a\": [1, 2],\n  \"b\": [\n    [3, \"x, ]\"]\n  ]\n}"
        );
        // Still the same JSON.
        let same: serde_json::Value = serde_json::from_str(&tidy(pretty)).unwrap();
        assert_eq!(
            same,
            serde_json::from_str::<serde_json::Value>(pretty).unwrap()
        );
    }

    #[test]
    fn ignores_fields_it_does_not_know() {
        let file = zipped(
            r#"{"format":"tessera","version":1,"style":{"fill":"red"},
            "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
            "layers":[{"kind":"layer","name":"A","x":1}]}"#,
        );
        assert!(open(&file).is_ok());
    }

    #[test]
    fn refuses_what_it_cannot_open() {
        let error = |file: &[u8]| open(file).err().unwrap();

        assert_eq!(error(b"<svg/>"), "Not a Tessera file");
        assert_eq!(
            error(&zipped(r#"{"format":"other","version":1}"#)),
            "Not a Tessera file"
        );
        assert!(error(&zipped(r#"{"format":"tessera","version":99}"#)).contains("newer Tessera"));
        assert!(
            error(&zipped(
                r#"{"format":"tessera","version":1,
                "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
                "layers":[{"kind":"layer","name":"A","points":[0.0,0.0],"faces":[[0,1,2]]}]}"#
            ))
            .contains("missing vertex")
        );
        assert!(
            error(&zipped(
                r#"{"format":"tessera","version":1,
                "view":{"pan":[0.0,0.0],"zoom":1.0,"rotation":0.0},
                "layers":[{"kind":"layer","name":"A",
                    "points":[0,0,1,0,0,1],"faces":[[0,1,2,7]]}]}"#
            ))
            .contains("colour")
        );
    }
}
