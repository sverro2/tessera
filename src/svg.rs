//! SVG export of a [`Document`].

use std::fmt::Write;

use crate::document::Document;
use crate::paint;

const PADDING: f32 = 10.0;

pub fn export(document: &Document) -> String {
    let (min, max) = document
        .bounds()
        .unwrap_or((iced::Point::ORIGIN, iced::Point::ORIGIN));

    let x = min.x - PADDING;
    let y = min.y - PADDING;
    let width = max.x - min.x + 2.0 * PADDING;
    let height = max.y - min.y + 2.0 * PADDING;

    let mut svg = String::new();

    let _ = writeln!(
        svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x} {y} {width} {height}" width="{width}" height="{height}">"#
    );
    // One path per face, outlined; painted edges over them. No groups,
    // and every element styled itself, so editors (e.g. Inkscape) can pick
    // and node-edit each directly.
    for (i, ([a, b, c], color)) in document.triangles().zip(document.colors()).enumerate() {
        // Painted faces are solid.
        let fill = match color {
            Some(color) => format!(r#"fill="{}""#, paint::to_hex(color)),
            None => r##"fill="#7aa2f7" fill-opacity="0.35""##.to_string(),
        };
        let _ = writeln!(
            svg,
            r##"  <path id="face{i}" d="M {},{} L {},{} L {},{} Z" {fill} stroke="#c0caf5" stroke-width="1.5" stroke-linejoin="round"/>"##,
            a.x, a.y, b.x, b.y, c.x, c.y
        );
    }
    for (i, (a, b, style)) in document.edge_styles().enumerate() {
        let (p, q) = (document.vertex(a), document.vertex(b));
        let _ = writeln!(
            svg,
            r#"  <path id="edge{i}" d="M {},{} L {},{}" fill="none" stroke="{}" stroke-width="{}" stroke-linecap="round"/>"#,
            p.x,
            p.y,
            q.x,
            q.y,
            paint::to_hex(style.color),
            style.width
        );
    }

    svg.push_str("</svg>\n");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{EdgeStyle, Edit};
    use iced::{Color, Point};

    #[test]
    fn faces_and_painted_edges_are_plain_paths() {
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

        let svg = export(&document);
        assert_eq!(svg.matches("<path ").count(), 3 + 1, "{svg}");
        assert_eq!(svg.matches(r##"fill="#ff0000""##).count(), 1, "{svg}");
        assert_eq!(svg.matches(r#"stroke-width="3""#).count(), 1, "{svg}");
        for other in ["<g", "<polygon", "<line"] {
            assert!(!svg.contains(other), "{svg}");
        }
    }
}
