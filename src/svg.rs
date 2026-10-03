//! SVG export of the [`Layers`]: each layer and group an Inkscape layer,
//! each painted face a path of its own, painted edges paths over them;
//! what isn't painted isn't there.
//!
//! Faces drawn one by one, each smoothed (anti-aliased) on its own, don't
//! quite cover the edges they share: half covered by each, a pixel there
//! is only three quarters covered by both, so what's behind shows through
//! in fine lines. Each layer's shared edges are sealed: a band along each,
//! under the faces, in a sublayer of their own ("Seals"), to delete if
//! they're not wanted. Under them, a seal only shows where the faces don't
//! quite cover; and as it keeps to the two faces it lies between, all the
//! way to the edge's ends, nothing of it shows past what's painted.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

use iced::Color;

use crate::document::{Document, VertexId};
use crate::fade;
use crate::geometry::area2;
use crate::joints;
use crate::layers::{Layer, Layers, Node};
use crate::page::Page;
use crate::paint;

const PADDING: f32 = 10.0;

/// How wide (pixels) seals are, where their faces have room: thinner,
/// smoothed as they are, they only part cover the line they're to seal.
pub const SEAL_PIXELS: f32 = 2.0;

/// How wide (world units: a pixel at 1×) seals are in an SVG on its own.
pub const SEAL: f32 = SEAL_PIXELS;

/// The area exported: `page` if given (what's past it cut off), else
/// around everything shown (mirrored too), with some room; its top left
/// corner and size (world units, a pixel each). Hidden layers are in the
/// SVG, but don't show, so they don't count.
pub fn area(layers: &Layers, page: Option<Page>) -> (iced::Point, iced::Size) {
    if let Some(page) = page {
        return (page.min(), page.size);
    }
    let bounds = layers
        .layers()
        .iter()
        .filter(|layer| layers.shown(layer.id))
        .filter_map(|layer| layer.drawing().bounds())
        .reduce(|(min1, max1), (min2, max2)| {
            (
                iced::Point::new(min1.x.min(min2.x), min1.y.min(min2.y)),
                iced::Point::new(max1.x.max(max2.x), max1.y.max(max2.y)),
            )
        });
    let (min, max) = bounds.unwrap_or((iced::Point::ORIGIN, iced::Point::ORIGIN));
    (
        iced::Point::new(min.x - PADDING, min.y - PADDING),
        iced::Size::new(max.x - min.x + 2.0 * PADDING, max.y - min.y + 2.0 * PADDING),
    )
}

/// The layers as an SVG of [`area`]: clipped to `page`, if given, by its
/// view box (so faces running past it are cut off cleanly where they
/// cross it, and are whole in the file).
pub fn export(layers: &Layers, page: Option<Page>) -> String {
    export_sealed(layers, page, SEAL)
}

/// As [`export`], with seals `seal` wide (world units): [`SEAL_PIXELS`]
/// of the image it's to become.
pub fn export_sealed(layers: &Layers, page: Option<Page>, seal: f32) -> String {
    let (corner, size) = area(layers, page);
    let (x, y) = (corner.x, corner.y);
    let (width, height) = (size.width, size.height);

    let mut svg = String::new();

    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" viewBox="{x} {y} {width} {height}" width="{width}" height="{height}">"#
    );
    let mut count = 0;
    // Back to front.
    for node in layers.nodes().iter().rev() {
        write_node(&mut svg, node, 1, &mut count, seal);
    }
    svg.push_str("</svg>\n");
    svg
}

/// A layer or group as an Inkscape layer (a group Inkscape treats as a
/// layer: what's in it is picked directly), `depth` deep.
fn write_node(svg: &mut String, node: &Node, depth: usize, count: &mut usize, seal: f32) {
    *count += 1;
    let n = *count;
    let indent = "  ".repeat(depth);
    let (name, visible) = match node {
        Node::Layer(layer) => (&layer.name, layer.visible),
        Node::Group(group) => (&group.name, group.visible),
    };
    let hidden = if visible {
        ""
    } else {
        r#" style="display:none""#
    };
    let _ = writeln!(
        svg,
        r#"{indent}<g id="layer{n}" inkscape:groupmode="layer" inkscape:label="{}"{hidden}>"#,
        escape(name)
    );
    match node {
        Node::Layer(layer) => write_drawing(svg, layer, &format!("l{n}-"), depth + 1, seal),
        Node::Group(group) => {
            for child in group.children.iter().rev() {
                write_node(svg, child, depth + 1, count, seal);
            }
        }
    }
    let _ = writeln!(svg, "{indent}</g>");
}

/// Seals under (see the module docs), one path per face, and the painted
/// edges over them. Every element
/// styled itself, so editors (e.g. Inkscape) can pick and node-edit each
/// directly. Ids start with `prefix`.
///
/// Crossfaded edges keep their own colour but at their ends, which fade into
/// the average colour of the painted edges meeting there (see [`fade`]): a
/// linear gradient, exactly as drawn.
fn write_drawing(svg: &mut String, layer: &Layer, prefix: &str, depth: usize, seal: f32) {
    // Mirrored, if it has a mirror.
    let document = &layer.drawing();
    let crossfade_edges = layer.crossfade.edges;
    let indent = "  ".repeat(depth);
    write_seals(svg, document, layer.show_edges, prefix, depth, seal);
    // The painted faces, solid; those not painted aren't there.
    let painted = document
        .triangles()
        .zip(document.colors())
        .filter_map(|(t, color)| Some((t, color?)));
    for (i, ([a, b, c], color)) in painted.enumerate() {
        let fill = paint_attributes("fill", "fill-opacity", color);
        let _ = writeln!(
            svg,
            r#"{indent}<path id="{prefix}face{i}" d="M {},{} L {},{} L {},{} Z" {fill}/>"#,
            a.x, a.y, b.x, b.y, c.x, c.y
        );
    }

    if !layer.show_edges {
        return;
    }

    // The painted edges over them (those not painted aren't there), each
    // its own outline, so edges of different widths meet without lying over
    // each other.
    let edges: Vec<_> = document
        .edge_styles()
        .map(|(a, b, style)| (a, b, style.width, style.color))
        .collect();
    let outlines = joints::outlines(
        &edges
            .iter()
            .map(|(a, b, width, _)| (*a, *b, *width))
            .collect::<Vec<_>>(),
        |v| document.vertex(v),
    );
    let fades = if crossfade_edges {
        fade::edge_fades(document, layer.crossfade.width, |v| document.vertex(v))
    } else {
        HashMap::new()
    };

    for (i, ((a, b, _, color), outline)) in edges.iter().zip(&outlines).enumerate() {
        let mut d = String::new();
        for (j, p) in outline.iter().enumerate() {
            let _ = write!(d, "{} {},{} ", if j == 0 { "M" } else { "L" }, p.x, p.y);
        }
        d.push('Z');
        let id = format!("{prefix}edge{i}");
        let fill = if let Some(stops) = fades.get(&(*a, *b)) {
            // Its own colour, fading at the ends into the mix there.
            let (p, q) = (document.vertex(*a), document.vertex(*b));
            let length = stops[3].0;
            let mut gradient = format!(
                r#"<linearGradient id="{id}-fade" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}">"#,
                p.x, p.y, q.x, q.y
            );
            for &(at, color) in stops {
                let _ = write!(
                    gradient,
                    r#"<stop offset="{}" {}/>"#,
                    at / length,
                    paint_attributes("stop-color", "stop-opacity", color),
                );
            }
            let _ = writeln!(svg, "{indent}{gradient}</linearGradient>");
            format!(r#"fill="url(#{id}-fade)""#)
        } else {
            paint_attributes("fill", "fill-opacity", *color)
        };
        let _ = writeln!(svg, r#"{indent}<path id="{id}" d="{d}" {fill}/>"#);
    }
}

/// The seals of `document`'s shared edges, `seal` wide, as a sublayer:
/// each edge two painted faces share, but for those painted (and shown,
/// `show_edges`), which cover it themselves. Each is a band along the edge,
/// `seal` wide but cut to its two faces (a trapezoid of each), so it goes
/// right to the edge's ends without ever sticking out of them; in the two
/// faces' colours mixed, as the line it fills would be. One path a colour.
fn write_seals(
    svg: &mut String,
    document: &Document,
    show_edges: bool,
    prefix: &str,
    depth: usize,
    seal: f32,
) {
    // Each edge's painted faces: their colours, and corners across the edge.
    // (Ordered, so the same drawing is always written the same.)
    let mut faces: BTreeMap<(VertexId, VertexId), Vec<(Color, VertexId)>> = BTreeMap::new();
    for (&[a, b, c], color) in document.triangle_ids().iter().zip(document.colors()) {
        let Some(color) = color else { continue };
        for (u, v, across) in [(a, b, c), (b, c, a), (c, a, b)] {
            faces
                .entry((u.min(v), u.max(v)))
                .or_default()
                .push((color, across));
        }
    }
    let mut paths: BTreeMap<String, String> = BTreeMap::new();
    for (&(a, b), sides) in &faces {
        let &[(one, c), (other, d)] = &sides[..] else {
            continue; // On the outline: nothing to seal.
        };
        // Translucent, it would show through, a line darker than the faces.
        if one.a < 1.0 || other.a < 1.0 {
            continue;
        }
        if show_edges && document.edge_style(a, b).is_some() {
            continue;
        }
        let [p, q, c, d] = [a, b, c, d].map(|v| document.vertex(v));
        let length = p.distance(q);
        if length == 0.0 {
            continue;
        }
        // In the face with corner `r`: the band's far side, `seal` / 2 from
        // the edge (or `r` itself, if that's nearer), from side to side.
        let across = |r: iced::Point| {
            let height = area2(p, q, r).abs() / length;
            let k = (seal / 2.0 / height).min(1.0);
            (p + (r - p) * k, q + (r - q) * k)
        };
        let ((p1, q1), (p2, q2)) = (across(c), across(d));
        let mut band = [p, p1, q1, q, q2, p2];
        // All one way round, so where they overlap, they add up (rather
        // than cancel out, as the nonzero rule has it).
        let winding: f32 = (0..band.len())
            .map(|i| area2(p, band[i], band[(i + 1) % band.len()]))
            .sum();
        if winding < 0.0 {
            band.reverse();
        }
        let d = paths
            .entry(paint::to_hex(fade::average(&[one, other])))
            .or_default();
        for (i, corner) in band.iter().enumerate() {
            let _ = write!(
                d,
                "{} {},{} ",
                if i == 0 { "M" } else { "L" },
                corner.x,
                corner.y
            );
        }
        d.push_str("Z ");
    }
    if paths.is_empty() {
        return;
    }
    let indent = "  ".repeat(depth);
    let _ = writeln!(
        svg,
        r#"{indent}<g id="{prefix}seals" inkscape:groupmode="layer" inkscape:label="Seals">"#
    );
    for (i, (color, d)) in paths.iter().enumerate() {
        let _ = writeln!(
            svg,
            r#"{indent}  <path id="{prefix}seal{i}" d="{}" fill="{color}"/>"#,
            d.trim_end()
        );
    }
    let _ = writeln!(svg, "{indent}</g>");
}

/// `color` as the attributes `name` (`fill`, say) and, translucent, its
/// opacity apart as `opacity` (`fill-opacity`): what SVG readers all
/// understand.
fn paint_attributes(name: &str, opacity: &str, color: Color) -> String {
    let hex = paint::to_opaque_hex(color);
    if color.a >= 1.0 {
        format!(r#"{name}="{hex}""#)
    } else {
        format!(r#"{name}="{hex}" {opacity}="{}""#, color.a)
    }
}

/// `text` as XML attribute content.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use crate::document::{EdgeStyle, Edit};
    use iced::{Color, Point};

    #[test]
    fn a_mirror_is_exported_mirrored() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        let color = Color::from_rgb8(255, 0, 0);
        document.apply(Edit::PaintAll { color: Some(color) });
        document.apply(Edit::PaintAllEdges {
            style: Some(EdgeStyle { color, width: 1.0 }),
        });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        layers.set_mirror(
            first,
            Some(crate::document::Mirror {
                a: Point::new(20.0, 0.0),
                b: Point::new(20.0, 10.0),
            }),
        );
        let svg = export(&layers, None);
        assert_eq!(svg.matches("-face").count(), 2, "{svg}");
        // Its mirror image is in view too.
        assert!(svg.contains(r#"viewBox="-10 -10 60 30""#), "{svg}");
    }

    #[test]
    fn clipped_to_the_document_it_shows_just_that() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(500.0, 0.0),
                Point::new(250.0, 400.0),
            ],
            snap: 0.0,
        });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let page = Page {
            center: Point::new(100.0, 50.0),
            size: iced::Size::new(200.0, 100.0),
        };
        let svg = export(&layers, Some(page));
        assert!(
            svg.contains(r#"viewBox="0 0 200 100" width="200" height="100""#),
            "{svg}"
        );
        // The face running past it is whole in the file.
        assert_eq!(svg.matches("-face").count(), 1, "{svg}");
    }

    #[test]
    fn hidden_edges_are_left_out() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        let color = Color::from_rgb8(255, 0, 0);
        document.apply(Edit::PaintAll { color: Some(color) });
        document.apply(Edit::PaintAllEdges {
            style: Some(EdgeStyle { color, width: 1.0 }),
        });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        assert_eq!(export(&layers, None).matches("-edge").count(), 3);
        layers.set_show_edges(first, false);
        let svg = export(&layers, None);
        assert_eq!(svg.matches("-edge").count(), 0, "{svg}");
        assert_eq!(svg.matches("-face").count(), 1, "{svg}");
    }

    #[test]
    fn crossfaded_edges_fade_along_them() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        // Two edges painted, the third erased.
        document.apply(Edit::PaintAllEdges { style: None });
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        for ((a, b), color) in [((0, 1), red), ((1, 2), blue)] {
            assert!(document.apply(Edit::PaintEdge {
                a,
                b,
                style: Some(EdgeStyle { color, width: 2.0 }),
            }));
        }
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let solid = export(&layers, None);
        assert!(!solid.contains("linearGradient"), "{solid}");

        layers.set_crossfade(
            first,
            crate::layers::Crossfade {
                edges: true,
                width: 2.0,
            },
        );
        let svg = export(&layers, None);
        // Two painted edges, 10 long: their own colour but for the last 2
        // at an end where they meet, fading into the blend there.
        assert_eq!(svg.matches("<linearGradient").count(), 2, "{svg}");
        assert!(
            svg.contains(r##"<stop offset="0" stop-color="#ff0000"/><stop offset="0.2" stop-color="#ff0000"/><stop offset="0.8" stop-color="#ff0000"/><stop offset="1" stop-color="#800080"/>"##),
            "{svg}"
        );
        assert_eq!(svg.matches("url(#l1-edge").count(), 2, "{svg}");
    }

    #[test]
    fn translucent_faces_have_their_opacity_and_no_seals() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        document.apply(Edit::InsertVertex {
            at: Point::new(5.0, 4.0),
        });
        let see_through = Color::from_rgba8(255, 0, 0, 0.5);
        document.apply(Edit::PaintAll {
            color: Some(see_through),
        });
        document.apply(Edit::PaintAllEdges { style: None });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let svg = export(&layers, None);
        assert!(
            svg.contains(r##"fill="#ff0000" fill-opacity="0.5""##),
            "{svg}"
        );
        // A seal would show through, a line darker than the faces.
        assert!(!svg.contains("Seals"), "{svg}");
    }

    #[test]
    fn shared_edges_are_sealed_but_for_painted_ones() {
        // Three faces around a middle vertex: three edges shared.
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        document.apply(Edit::InsertVertex {
            at: Point::new(5.0, 4.0),
        });
        let red = Color::from_rgb8(255, 0, 0);
        document.apply(Edit::PaintAll { color: Some(red) });
        let mut layers = Layers::default();
        let first = layers.first_layer();
        let mut set = |document: &Document| {
            layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document.clone());
            export_sealed(&layers, None, 0.5)
        };
        // Segments in the seals' paths.
        let seals = |svg: &str| {
            svg.lines()
                .filter(|line| line.contains("-seal"))
                .map(|line| line.matches("M ").count())
                .sum::<usize>()
        };

        // Edges painted (as they start out): they cover it themselves.
        let svg = set(&document);
        assert!(!svg.contains("Seals"), "{svg}");

        // Not painted: sealed, the one colour in one path, in a sublayer.
        document.apply(Edit::PaintAllEdges { style: None });
        let svg = set(&document);
        assert_eq!(seals(&svg), 3, "{svg}");
        assert!(
            svg.contains(r#"<g id="l1-seals" inkscape:groupmode="layer" inkscape:label="Seals">"#),
            "{svg}"
        );
        assert!(svg.contains(r##"<path id="l1-seal0" d="M "##), "{svg}");
        assert!(svg.contains(r##"Z" fill="#ff0000"/>"##), "{svg}");
        // Bands kept to their faces: inside the triangle they make up.
        let seal = svg.lines().find(|line| line.contains("l1-seal0")).unwrap();
        let d = seal
            .split(r#" d=""#)
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let numbers: Vec<f32> = d
            .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .filter_map(|n| n.parse().ok())
            .collect();
        assert_eq!(numbers.len(), 3 * 6 * 2, "{seal}");
        let outline = [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(5.0, 10.0),
        ];
        for xy in numbers.chunks(2) {
            let p = Point::new(xy[0], xy[1]);
            let inside = (0..3)
                .all(|i| crate::geometry::area2(outline[i], outline[(i + 1) % 3], p) >= -1e-4);
            assert!(inside, "{p:?} in {seal}");
        }
        // One of them painted: covered by the edge, not sealed. (Edges to
        // the middle vertex, 3, are the shared ones.)
        let shared = document
            .unique_edges()
            .into_iter()
            .find(|&(a, b)| a == 3 || b == 3)
            .unwrap();
        document.apply(Edit::PaintEdge {
            a: shared.0,
            b: shared.1,
            style: Some(EdgeStyle {
                color: red,
                width: 1.0,
            }),
        });
        let svg = set(&document);
        assert_eq!(seals(&svg), 2, "{svg}");
        // Under the faces (and so the painted edges).
        assert!(svg.find("l1-seals").unwrap() < svg.find("l1-face0").unwrap());

        // With the edges hidden, the painted one is sealed too.
        layers.set_show_edges(first, false);
        let svg = export_sealed(&layers, None, 0.5);
        assert_eq!(seals(&svg), 3, "{svg}");
        // And plain `export` as wide as at 1×.
        assert_eq!(export(&layers, None), export_sealed(&layers, None, SEAL));
    }

    #[test]
    fn a_seal_is_its_faces_colours_mixed() {
        let mut document = Document::default();
        for corners in [
            [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            [
                Point::new(10.0, 0.0),
                Point::new(10.0, 9.0),
                Point::new(5.0, 10.0),
            ],
        ] {
            assert!(document.apply(Edit::AddTriangle { corners, snap: 0.0 }));
        }
        assert_eq!(document.vertex_count(), 4, "sharing an edge");
        document.apply(Edit::PaintAllEdges { style: None });
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        for (triangle, color) in [(0, red), (1, blue)] {
            document.apply(Edit::Paint {
                triangle,
                color: Some(color),
            });
        }
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let svg = export(&layers, None);
        let seals: Vec<_> = svg.lines().filter(|line| line.contains("-seal")).collect();
        assert_eq!(seals.len(), 2, "the group, and one path: {svg}");
        assert!(seals[1].ends_with(r##"Z" fill="#800080"/>"##), "{svg}");
    }

    #[test]
    fn layers_hold_plain_paths() {
        let mut document = Document::default();
        document.apply(Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        });
        document.apply(Edit::InsertVertex {
            at: Point::new(5.0, 4.0),
        });
        // All erased, but for a face and an edge.
        document.apply(Edit::PaintAll { color: None });
        document.apply(Edit::PaintAllEdges { style: None });
        let red = Color::from_rgb8(255, 0, 0);
        document.apply(Edit::Paint {
            triangle: 0,
            color: Some(red),
        });
        let (a, b) = document.unique_edges()[0];
        document.apply(Edit::PaintEdge {
            a,
            b,
            style: Some(EdgeStyle {
                color: red,
                width: 3.0,
            }),
        });

        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        let hidden = layers.add_layer(first);
        layers.rename(hidden, "A \"quote\"".into());
        layers.set_visible(hidden, false);

        let svg = export(&layers, None);
        // The painted face and edge only (of three and six): what isn't
        // painted isn't there.
        assert_eq!(svg.matches("<path ").count(), 1 + 1, "{svg}");
        assert!(!svg.contains("stroke"), "{svg}");
        // Back to front; the hidden layer hidden.
        let back = svg.find(r#"inkscape:label="Layer 1""#).unwrap();
        let front = svg
            .find(r#"inkscape:label="A &quot;quote&quot;" style="display:none""#)
            .unwrap();
        assert!(back < front, "{svg}");
        // The red face and the red edge.
        assert_eq!(svg.matches(r##"fill="#ff0000""##).count(), 2, "{svg}");
        assert_eq!(svg.matches(r#"id="l1-edge"#).count(), 1, "{svg}");
        for other in ["<polygon", "<line"] {
            assert!(!svg.contains(other), "{svg}");
        }
    }
}
