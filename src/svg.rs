//! SVG export of a [`Document`].

use std::fmt::Write;

use crate::document::Document;

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
    let _ = writeln!(
        svg,
        r##"  <g fill="#7aa2f7" fill-opacity="0.35" stroke="#c0caf5" stroke-width="1.5" stroke-linejoin="round">"##
    );

    for [a, b, c] in document.triangles() {
        let _ = writeln!(
            svg,
            r#"    <polygon points="{},{} {},{} {},{}"/>"#,
            a.x, a.y, b.x, b.y, c.x, c.y
        );
    }

    svg.push_str("  </g>\n</svg>\n");
    svg
}
