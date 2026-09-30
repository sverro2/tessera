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
    // Faces, then the edges over them: plain ones, then painted ones.
    let _ = writeln!(
        svg,
        r##"  <g fill="#7aa2f7" fill-opacity="0.35" stroke="none">"##
    );

    for ([a, b, c], color) in document.triangles().zip(document.colors()) {
        // Painted faces are solid; the rest keep the group's fill.
        let fill = match color {
            Some(color) => format!(r#" fill="{}" fill-opacity="1""#, paint::to_hex(color)),
            None => String::new(),
        };
        let _ = writeln!(
            svg,
            r#"    <polygon points="{},{} {},{} {},{}"{fill}/>"#,
            a.x, a.y, b.x, b.y, c.x, c.y
        );
    }
    svg.push_str("  </g>\n");

    let _ = writeln!(
        svg,
        r##"  <g stroke="#c0caf5" stroke-width="1.5" stroke-linecap="round">"##
    );
    let line = |svg: &mut String, a: usize, b: usize, style: &str| {
        let (p, q) = (document.vertex(a), document.vertex(b));
        let _ = writeln!(
            svg,
            r#"    <line x1="{}" y1="{}" x2="{}" y2="{}"{style}/>"#,
            p.x, p.y, q.x, q.y
        );
    };
    for (a, b) in document.unique_edges() {
        if document.edge_style(a, b).is_none() {
            line(&mut svg, a, b, "");
        }
    }
    for (a, b, style) in document.edge_styles() {
        let style = format!(
            r#" stroke="{}" stroke-width="{}""#,
            paint::to_hex(style.color),
            style.width
        );
        line(&mut svg, a, b, &style);
    }

    svg.push_str("  </g>\n</svg>\n");
    svg
}
