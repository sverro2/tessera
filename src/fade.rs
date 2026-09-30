//! Crossfaded edges: each painted edge its own colour, but near its ends,
//! where it fades into the mix of the painted edges meeting there. As its
//! colour only changes along it, it's drawn exactly by a linear gradient,
//! both on the canvas and in SVG.

use std::collections::HashMap;

use iced::{Color, Point};

use crate::document::{Document, VertexId};

/// How a crossfaded edge `length` long is coloured along it, as (distance
/// from its start, colour) stops: from `from` (the mix at its start) to
/// its `own` colour over `width` (at most half way), its own colour, and
/// over the last `width` to `to`. Beyond its ends, as at them.
pub fn edge_stops(
    own: Color,
    from: Color,
    to: Color,
    length: f32,
    width: f32,
) -> [(f32, Color); 4] {
    let width = width.min(length / 2.0);
    [
        (0.0, from),
        (width, own),
        (length - width, own),
        (length, to),
    ]
}

/// Each painted edge's fade (lowest end first; see [`edge_stops`]): its
/// ends fading into the mix of the painted edges meeting there, over
/// `width`. Positions from `at`, and `width` in the same units.
pub fn edge_fades(
    document: &Document,
    width: f32,
    at: impl Fn(VertexId) -> Point,
) -> HashMap<(VertexId, VertexId), [(f32, Color); 4]> {
    let mut ends: HashMap<VertexId, Vec<Color>> = HashMap::new();
    for (a, b, style) in document.edge_styles() {
        ends.entry(a).or_default().push(style.color);
        ends.entry(b).or_default().push(style.color);
    }
    document
        .edge_styles()
        .map(|(a, b, style)| {
            let length = at(a).distance(at(b)).max(1e-3);
            let stops = edge_stops(
                style.color,
                average(&ends[&a]),
                average(&ends[&b]),
                length,
                width,
            );
            ((a, b), stops)
        })
        .collect()
}

fn average(colors: &[Color]) -> Color {
    let n = colors.len() as f32;
    let sum = |f: fn(&Color) -> f32| colors.iter().map(f).sum::<f32>() / n;
    Color::from_rgba(sum(|c| c.r), sum(|c| c.g), sum(|c| c.b), sum(|c| c.a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_keep_their_colour_in_the_middle() {
        let (own, red, blue) = (
            Color::from_rgb(0.0, 1.0, 0.0),
            Color::from_rgb(1.0, 0.0, 0.0),
            Color::from_rgb(0.0, 0.0, 1.0),
        );
        assert_eq!(
            edge_stops(own, red, blue, 100.0, 10.0),
            [(0.0, red), (10.0, own), (90.0, own), (100.0, blue)]
        );
        // Short: half way each.
        assert_eq!(
            edge_stops(own, red, blue, 8.0, 10.0),
            [(0.0, red), (4.0, own), (4.0, own), (8.0, blue)]
        );
    }
}
