//! SVG export of the [`Layers`]: each layer and group an Inkscape layer,
//! each face a path of its own, painted edges paths over them.

use std::collections::HashMap;
use std::fmt::Write;

use crate::fade;
use crate::joints;
use crate::layers::{Layer, Layers, Node};
use crate::paint;

const PADDING: f32 = 10.0;
/// How wide unpainted edges are drawn.
const PLAIN_EDGE: f32 = 1.5;

pub fn export(layers: &Layers) -> String {
    let bounds = layers
        .layers()
        .iter()
        .filter_map(|layer| layer.drawing().bounds())
        .reduce(|(min1, max1), (min2, max2)| {
            (
                iced::Point::new(min1.x.min(min2.x), min1.y.min(min2.y)),
                iced::Point::new(max1.x.max(max2.x), max1.y.max(max2.y)),
            )
        });
    let (min, max) = bounds.unwrap_or((iced::Point::ORIGIN, iced::Point::ORIGIN));

    let x = min.x - PADDING;
    let y = min.y - PADDING;
    let width = max.x - min.x + 2.0 * PADDING;
    let height = max.y - min.y + 2.0 * PADDING;

    let mut svg = String::new();

    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" viewBox="{x} {y} {width} {height}" width="{width}" height="{height}">"#
    );
    let mut count = 0;
    // Back to front.
    for node in layers.nodes().iter().rev() {
        write_node(&mut svg, node, 1, &mut count);
    }
    svg.push_str("</svg>\n");
    svg
}

/// A layer or group as an Inkscape layer (a group Inkscape treats as a
/// layer: what's in it is picked directly), `depth` deep.
fn write_node(svg: &mut String, node: &Node, depth: usize, count: &mut usize) {
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
        Node::Layer(layer) => write_drawing(svg, layer, &format!("l{n}-"), depth + 1),
        Node::Group(group) => {
            for child in group.children.iter().rev() {
                write_node(svg, child, depth + 1, count);
            }
        }
    }
    let _ = writeln!(svg, "{indent}</g>");
}

/// One path per face, outlined; painted edges over them. Every element
/// styled itself, so editors (e.g. Inkscape) can pick and node-edit each
/// directly. Ids start with `prefix`.
///
/// Crossfaded edges keep their own colour but at their ends, which fade into
/// the average colour of the painted edges meeting there (see [`fade`]): a
/// linear gradient, exactly as drawn.
fn write_drawing(svg: &mut String, layer: &Layer, prefix: &str, depth: usize) {
    // Mirrored, if it has a mirror.
    let document = &layer.drawing();
    let crossfade_edges = layer.crossfade.edges;
    let indent = "  ".repeat(depth);
    for (i, ([a, b, c], color)) in document.triangles().zip(document.colors()).enumerate() {
        // Painted faces are solid.
        let fill = match color {
            Some(color) => format!(r#"fill="{}""#, paint::to_hex(color)),
            None => r##"fill="#7aa2f7" fill-opacity="0.35""##.to_string(),
        };
        let _ = writeln!(
            svg,
            r#"{indent}<path id="{prefix}face{i}" d="M {},{} L {},{} L {},{} Z" {fill}/>"#,
            a.x, a.y, b.x, b.y, c.x, c.y
        );
    }

    if !layer.show_edges {
        return;
    }

    // The edges over them, each its own outline, so edges of different
    // widths meet without lying over each other.
    let edges: Vec<_> = document
        .unique_edges()
        .into_iter()
        .map(|(a, b)| match document.edge_style(a, b) {
            Some(style) => (a, b, style.width, paint::to_hex(style.color)),
            None => (a, b, PLAIN_EDGE, "#c0caf5".to_string()),
        })
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
                    r#"<stop offset="{}" stop-color="{}"/>"#,
                    at / length,
                    paint::to_hex(color)
                );
            }
            let _ = writeln!(svg, "{indent}{gradient}</linearGradient>");
            format!("url(#{id}-fade)")
        } else {
            color.clone()
        };
        let _ = writeln!(svg, r#"{indent}<path id="{id}" d="{d}" fill="{fill}"/>"#);
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
        let svg = export(&layers);
        assert_eq!(svg.matches("-face").count(), 2, "{svg}");
        // Its mirror image is in view too.
        assert!(svg.contains(r#"viewBox="-10 -10 60 30""#), "{svg}");
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
        let mut layers = Layers::default();
        let first = layers.first_layer();
        layers.layer_mut(first).unwrap().document = std::sync::Arc::new(document);
        assert_eq!(export(&layers).matches("-edge").count(), 3);
        layers.set_show_edges(first, false);
        let svg = export(&layers);
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
        let solid = export(&layers);
        assert!(!solid.contains("linearGradient"), "{solid}");

        layers.set_crossfade(
            first,
            crate::layers::Crossfade {
                edges: true,
                width: 2.0,
            },
        );
        let svg = export(&layers);
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

        let svg = export(&layers);
        // Three faces, and six edges (each its own outline, painted or not).
        assert_eq!(svg.matches("<path ").count(), 3 + 6, "{svg}");
        assert!(!svg.contains("stroke"), "{svg}");
        // Back to front; the hidden layer hidden.
        let back = svg.find(r#"inkscape:label="Layer 1""#).unwrap();
        let front = svg
            .find(r#"inkscape:label="A &quot;quote&quot;" style="display:none""#)
            .unwrap();
        assert!(back < front, "{svg}");
        // The red face and the red edge.
        assert_eq!(svg.matches(r##"fill="#ff0000""##).count(), 2, "{svg}");
        assert_eq!(svg.matches(r#"id="l1-edge"#).count(), 6, "{svg}");
        for other in ["<polygon", "<line"] {
            assert!(!svg.contains(other), "{svg}");
        }
    }
}
