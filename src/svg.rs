//! SVG export of the [`Layers`]: each layer and group an Inkscape layer,
//! each face a path of its own, painted edges paths over them.

use std::fmt::Write;

use crate::document::Document;
use crate::joints;
use crate::layers::{Layers, Node};
use crate::paint;

const PADDING: f32 = 10.0;
/// How wide unpainted edges are drawn.
const PLAIN_EDGE: f32 = 1.5;

pub fn export(layers: &Layers) -> String {
    let bounds = layers
        .layers()
        .iter()
        .filter_map(|layer| layer.document.bounds())
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
        Node::Layer(layer) => write_drawing(svg, &layer.document, &format!("l{n}-"), depth + 1),
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
fn write_drawing(svg: &mut String, document: &Document, prefix: &str, depth: usize) {
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
    for (i, ((.., color), outline)) in edges.iter().zip(&outlines).enumerate() {
        let mut d = String::new();
        for (j, p) in outline.iter().enumerate() {
            let _ = write!(d, "{} {},{} ", if j == 0 { "M" } else { "L" }, p.x, p.y);
        }
        d.push('Z');
        let _ = writeln!(
            svg,
            r#"{indent}<path id="{prefix}edge{i}" d="{d}" fill="{color}"/>"#
        );
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
    use crate::document::{EdgeStyle, Edit};
    use iced::{Color, Point};

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
