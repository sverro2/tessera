//! Gradients: colours along a line (linear) or out from a point (radial),
//! laid over a layer's faces or edges. Each face (or edge) takes the one
//! colour where its middle lies, laid over the colour it had; the shapes
//! painted are those the gradient's line runs through. And the gradients
//! kept between runs as presets, in the user's config folder
//! (`~/.config/tessera/gradients.json`).

use std::collections::HashMap;
use std::path::PathBuf;

use iced::{Color, Point};
use serde::{Deserialize, Serialize};

use crate::document::{Document, EdgeStyle, Edit, TriangleId, VertexId};
use crate::geometry::area2;
use crate::paint::{self, Target};

/// How the colours run: from the line's start to its end, or out from its
/// start (the centre) as far as its end (the rim), all round.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Linear,
    Radial,
}

/// A colour at a place along the gradient: `at` from 0 (its start) to 1
/// (its end).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stop {
    pub at: f32,
    pub color: Color,
}

/// The colours, and how they run. Its stops in order along it, at least two.
#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    pub kind: Kind,
    stops: Vec<Stop>,
}

impl Default for Gradient {
    /// From the palette's blue to its pink, out to see-through: so it shows
    /// what laying it over something looks like.
    fn default() -> Self {
        Gradient {
            kind: Kind::Linear,
            stops: vec![
                Stop {
                    at: 0.0,
                    color: paint::PALETTE[6],
                },
                Stop {
                    at: 1.0,
                    color: Color {
                        a: 0.0,
                        ..paint::PALETTE[0]
                    },
                },
            ],
        }
    }
}

impl Gradient {
    /// One of `kind` with these stops (at least two; put in order, `at`
    /// kept within 0 to 1).
    pub fn new(kind: Kind, stops: impl IntoIterator<Item = Stop>) -> Option<Self> {
        let mut stops: Vec<Stop> = stops
            .into_iter()
            .filter(|stop| stop.at.is_finite())
            .map(|stop| Stop {
                at: stop.at.clamp(0.0, 1.0),
                ..stop
            })
            .collect();
        stops.sort_by(|a, b| a.at.total_cmp(&b.at));
        (stops.len() >= 2).then_some(Gradient { kind, stops })
    }

    /// Its stops, in order along it.
    pub fn stops(&self) -> &[Stop] {
        &self.stops
    }

    /// Moves stop `i` to `at` (within 0 to 1), keeping it in order among
    /// the others: where it lands, as its number now.
    pub fn move_stop(&mut self, i: usize, at: f32) -> usize {
        let Some(stop) = self.stops.get_mut(i) else {
            return i;
        };
        stop.at = at.clamp(0.0, 1.0);
        self.settle(i)
    }

    /// Recolours stop `i`.
    pub fn set_color(&mut self, i: usize, color: Color) {
        if let Some(stop) = self.stops.get_mut(i) {
            stop.color = color;
        }
    }

    /// A stop added at `at`, the colour the gradient has there (so it looks
    /// the same until it's changed): its number.
    pub fn add_stop(&mut self, at: f32) -> usize {
        let at = at.clamp(0.0, 1.0);
        let color = self.color_at(at);
        self.stops.push(Stop { at, color });
        self.settle(self.stops.len() - 1)
    }

    /// Takes stop `i` away, if that leaves two at least; whether it did.
    pub fn remove_stop(&mut self, i: usize) -> bool {
        let removes = self.stops.len() > 2 && i < self.stops.len();
        if removes {
            self.stops.remove(i);
        }
        removes
    }

    /// The other way round: the end's colour at the start.
    pub fn reverse(&mut self) {
        self.stops.reverse();
        for stop in &mut self.stops {
            stop.at = 1.0 - stop.at;
        }
    }

    /// Stop `i`, just moved, put back in order: where it went.
    fn settle(&mut self, i: usize) -> usize {
        let stop = self.stops.remove(i);
        // After any at the same place.
        let to = self
            .stops
            .iter()
            .position(|other| other.at > stop.at)
            .unwrap_or(self.stops.len());
        self.stops.insert(to, stop);
        to
    }

    /// The colour at `t` along it (0 its start, 1 its end; beyond, the
    /// colour at that end): mixed from the stops either side in OKLab, so
    /// colours run evenly into each other (blue into yellow without going
    /// grey in the middle), their opacity along with them.
    pub fn color_at(&self, t: f32) -> Color {
        let first = self.stops[0];
        let last = self.stops[self.stops.len() - 1];
        if t.is_nan() || t <= first.at {
            return first.color;
        }
        if t >= last.at {
            return last.color;
        }
        let next = self.stops.iter().position(|stop| stop.at > t).unwrap_or(1);
        let (a, b) = (self.stops[next - 1], self.stops[next]);
        let f = if b.at > a.at {
            (t - a.at) / (b.at - a.at)
        } else {
            1.0
        };
        mix(a.color, b.color, f)
    }

    /// Where `p` is along it, laid from `start` to `end` (0 to 1; beyond
    /// either end, past them).
    pub fn along(&self, start: Point, end: Point, p: Point) -> f32 {
        let d = end - start;
        let length2 = d.x * d.x + d.y * d.y;
        if length2 <= 0.0 {
            return 0.0;
        }
        match self.kind {
            Kind::Linear => ((p.x - start.x) * d.x + (p.y - start.y) * d.y) / length2,
            Kind::Radial => p.distance(start) / length2.sqrt(),
        }
    }
}

/// `a` mixed with `b`, `f` of the way to it, in OKLab; premultiplied by
/// opacity, so a see-through end doesn't bring its hidden colour along.
pub fn mix(a: Color, b: Color, f: f32) -> Color {
    let (la, lb) = (oklab(a), oklab(b));
    let alpha = a.a + (b.a - a.a) * f;
    let lab: [f32; 3] = std::array::from_fn(|i| {
        let (pa, pb) = (la[i] * a.a, lb[i] * b.a);
        if alpha > 1e-6 {
            (pa + (pb - pa) * f) / alpha
        } else {
            la[i] + (lb[i] - la[i]) * f
        }
    });
    from_oklab(lab, alpha)
}

/// `new` laid over `old` (as it's drawn: over nothing, if unpainted): what
/// shows. Opacity adds up, so see-through over opaque is opaque; nothing
/// at all over nothing stays unpainted.
pub fn over(new: Color, old: Option<Color>) -> Option<Color> {
    let Some(old) = old else {
        return (new.a > 0.0).then_some(new);
    };
    let a = new.a + old.a * (1.0 - new.a);
    if a <= 0.0 {
        return Some(old);
    }
    let channel = |n: f32, o: f32| (n * new.a + o * old.a * (1.0 - new.a)) / a;
    Some(Color::from_rgba(
        channel(new.r, old.r),
        channel(new.g, old.g),
        channel(new.b, old.b),
        a,
    ))
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// `color` (its opacity left out) in OKLab: lightness, then green–red and
/// blue–yellow.
fn oklab(color: Color) -> [f32; 3] {
    let [r, g, b] = [color.r, color.g, color.b].map(to_linear);
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// Back from OKLab, with opacity `alpha`; kept to the colours there are.
fn from_oklab([lightness, a, b]: [f32; 3], alpha: f32) -> Color {
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    let srgb = |c: f32| to_srgb(c.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    Color::from_rgba(
        srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
        alpha.clamp(0.0, 1.0),
    )
}

/// The faces of the shapes (parts hanging together by their corners) the
/// line `start`–`end` runs through, or starts or ends in; in order.
pub fn reached(document: &Document, start: Point, end: Point) -> Vec<TriangleId> {
    let triangles = document.triangle_ids();
    let crossed: Vec<VertexId> = document
        .triangles()
        .zip(triangles)
        .filter(|(corners, _)| meets(*corners, start, end))
        .map(|(_, t)| t[0])
        .collect();
    if crossed.is_empty() {
        return Vec::new();
    }
    // Out from those through shared corners.
    let mut around: HashMap<VertexId, Vec<TriangleId>> = HashMap::new();
    for (i, t) in triangles.iter().enumerate() {
        for &v in t {
            around.entry(v).or_default().push(i);
        }
    }
    let mut reached = vec![false; triangles.len()];
    let mut seen: std::collections::HashSet<VertexId> = crossed.iter().copied().collect();
    let mut next = crossed;
    while let Some(v) = next.pop() {
        for &i in around.get(&v).into_iter().flatten() {
            if std::mem::replace(&mut reached[i], true) {
                continue;
            }
            for &w in &triangles[i] {
                if seen.insert(w) {
                    next.push(w);
                }
            }
        }
    }
    (0..triangles.len()).filter(|&i| reached[i]).collect()
}

/// Whether the segment `p`–`q` touches the (positively wound) triangle.
fn meets([a, b, c]: [Point; 3], p: Point, q: Point) -> bool {
    let inside = |x: Point| area2(a, b, x) >= 0.0 && area2(b, c, x) >= 0.0 && area2(c, a, x) >= 0.0;
    let crosses = |u: Point, v: Point| {
        area2(p, q, u) * area2(p, q, v) <= 0.0 && area2(u, v, p) * area2(u, v, q) <= 0.0
    };
    inside(p) || inside(q) || crosses(a, b) || crosses(b, c) || crosses(c, a)
}

/// The edit laying `gradient` from `start` to `end` over the faces (or
/// edges) of the shapes its line runs through: each the colour where its
/// middle lies, over what it had. Edges not painted yet come out `width`
/// wide; painted ones keep theirs. `None` if nothing would change.
pub fn edit(
    document: &Document,
    gradient: &Gradient,
    (start, end): (Point, Point),
    target: Target,
    width: f32,
) -> Option<Edit> {
    let faces = reached(document, start, end);
    let color_at = |p: Point| gradient.color_at(gradient.along(start, end, p));
    match target {
        Target::Faces => {
            let corners: Vec<[Point; 3]> = document.triangles().collect();
            let faces: Vec<(TriangleId, Option<Color>)> = faces
                .into_iter()
                .filter_map(|t| {
                    let [a, b, c] = corners[t];
                    let middle = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
                    let old = document.color(t);
                    let new = over(color_at(middle), old);
                    (new != old).then_some((t, new))
                })
                .collect();
            (!faces.is_empty()).then_some(Edit::PaintFaces { faces })
        }
        Target::Edges => {
            let ids = document.triangle_ids();
            let mut ends: Vec<(VertexId, VertexId)> = faces
                .into_iter()
                .flat_map(|t| {
                    let [a, b, c] = ids[t];
                    [(a, b), (b, c), (c, a)]
                })
                .map(|(u, v)| (u.min(v), u.max(v)))
                .collect();
            ends.sort_unstable();
            ends.dedup();
            let edges: Vec<(VertexId, VertexId, Option<EdgeStyle>)> = ends
                .into_iter()
                .filter_map(|(a, b)| {
                    let (p, q) = (document.vertex(a), document.vertex(b));
                    let middle = Point::new((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
                    let old = document.edge_style(a, b);
                    let color = over(color_at(middle), old.map(|style| style.color));
                    let new = color.map(|color| EdgeStyle {
                        color,
                        width: old.map_or(width, |style| style.width),
                    });
                    (new != old).then_some((a, b, new))
                })
                .collect();
            (!edges.is_empty()).then_some(Edit::PaintEdges { edges })
        }
    }
}

/// A gradient kept under a name, to come back to.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub gradient: Gradient,
}

/// The presets, kept between runs.
#[derive(Debug, Default)]
pub struct Presets {
    list: Vec<Preset>,
    /// Where they're kept; `None` keeps them in memory only (as in tests).
    store: Option<PathBuf>,
}

/// How a gradient is kept: its stops' colours as `#rrggbbaa`.
#[derive(Serialize, Deserialize)]
struct Kept {
    name: String,
    #[serde(default)]
    kind: Kind,
    stops: Vec<(f32, String)>,
}

impl Presets {
    /// As kept in the config folder; none if there are none (or they can't
    /// be read).
    pub fn load() -> Self {
        // Tests start with none, and leave the user's file be.
        if cfg!(test) {
            return Presets::default();
        }
        let store = crate::recent::config_folder().map(|folder| folder.join("gradients.json"));
        let kept: Vec<Kept> = store
            .as_deref()
            .and_then(|store| std::fs::read_to_string(store).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let list = kept
            .into_iter()
            .filter_map(|kept| {
                let stops = kept.stops.iter().map(|(at, hex)| {
                    Some(Stop {
                        at: *at,
                        color: paint::from_hex(hex)?,
                    })
                });
                let gradient = Gradient::new(kept.kind, stops.collect::<Option<Vec<_>>>()?)?;
                Some(Preset {
                    name: kept.name,
                    gradient,
                })
            })
            .collect();
        Presets { list, store }
    }

    pub fn list(&self) -> &[Preset] {
        &self.list
    }

    /// Keeps `gradient` as `name`, in place of one of that name there was.
    pub fn save(&mut self, name: &str, gradient: &Gradient) {
        let preset = Preset {
            name: name.to_string(),
            gradient: gradient.clone(),
        };
        match self.list.iter_mut().find(|preset| preset.name == name) {
            Some(same) => *same = preset,
            None => self.list.push(preset),
        }
        self.keep();
    }

    /// Forgets preset `i`.
    pub fn remove(&mut self, i: usize) {
        if i < self.list.len() {
            self.list.remove(i);
            self.keep();
        }
    }

    /// Writes them down; not being able to is no reason to bother anyone.
    fn keep(&self) {
        let Some(store) = &self.store else {
            return;
        };
        let kept: Vec<Kept> = self
            .list
            .iter()
            .map(|preset| Kept {
                name: preset.name.clone(),
                kind: preset.gradient.kind,
                stops: preset
                    .gradient
                    .stops
                    .iter()
                    .map(|stop| (stop.at, paint::to_hex(stop.color)))
                    .collect(),
            })
            .collect();
        if let Some(folder) = store.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(text) = serde_json::to_string_pretty(&kept) {
            let _ = std::fs::write(store, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::from_rgb(1.0, 0.0, 0.0);
    const BLUE: Color = Color::from_rgb(0.0, 0.0, 1.0);

    fn red_to_blue(kind: Kind) -> Gradient {
        Gradient::new(
            kind,
            [
                Stop {
                    at: 0.0,
                    color: RED,
                },
                Stop {
                    at: 1.0,
                    color: BLUE,
                },
            ],
        )
        .unwrap()
    }

    fn close(a: Color, b: Color) -> bool {
        let [a, b] = [a, b].map(|c| [c.r, c.g, c.b, c.a]);
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3)
    }

    #[test]
    fn colours_run_from_stop_to_stop_and_stay_beyond_the_ends() {
        let gradient = red_to_blue(Kind::Linear);
        assert!(close(gradient.color_at(-1.0), RED));
        assert!(close(gradient.color_at(0.0), RED));
        assert!(close(gradient.color_at(1.0), BLUE));
        assert!(close(gradient.color_at(3.0), BLUE));
        // In between: some of each, mixed in OKLab (not through a dull
        // purple-grey: brighter than the plain average).
        let middle = gradient.color_at(0.5);
        assert!(middle.r > 0.5 && middle.b > 0.5, "{middle:?}");
    }

    #[test]
    fn mixing_with_see_through_keeps_the_colour_and_fades_it() {
        let clear = Color { a: 0.0, ..BLUE };
        let half = mix(RED, clear, 0.5);
        assert!(close(half, Color { a: 0.5, ..RED }), "{half:?}");
    }

    #[test]
    fn a_line_places_points_along_it_or_out_from_its_start() {
        let (start, end) = (Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        let linear = red_to_blue(Kind::Linear);
        assert_eq!(linear.along(start, end, Point::new(5.0, 7.0)), 0.5);
        let radial = red_to_blue(Kind::Radial);
        assert_eq!(radial.along(start, end, Point::new(0.0, -5.0)), 0.5);
    }

    #[test]
    fn opacity_adds_up_laid_over_what_was_there() {
        let veil = Color { a: 0.2, ..BLUE };
        // Over red, mostly red still, and as opaque.
        let over_red = over(veil, Some(RED)).unwrap();
        assert!(
            close(over_red, Color::from_rgb(0.8, 0.0, 0.2)),
            "{over_red:?}"
        );
        // Over nothing: as see-through as it is.
        assert_eq!(over(veil, None), Some(veil));
        // Over half see-through: more opaque than either.
        let half = Color { a: 0.5, ..RED };
        assert!((over(veil, Some(half)).unwrap().a - 0.6).abs() < 1e-6);
        // Nothing over nothing: still unpainted.
        assert_eq!(over(Color::TRANSPARENT, None), None);
    }

    #[test]
    fn stops_stay_in_order_as_they_are_moved_added_and_removed() {
        let mut gradient = red_to_blue(Kind::Linear);
        let added = gradient.add_stop(0.25);
        assert_eq!(added, 1);
        assert!(close(gradient.stops()[1].color, gradient.color_at(0.25)));
        // The first dragged past the others: last now.
        assert_eq!(gradient.move_stop(0, 2.0), 2);
        let at: Vec<f32> = gradient.stops().iter().map(|stop| stop.at).collect();
        assert_eq!(at, vec![0.25, 1.0, 1.0]);
        assert!(gradient.remove_stop(0));
        // Two at least.
        assert!(!gradient.remove_stop(0));
        gradient.reverse();
        assert_eq!(gradient.stops()[0].color, RED);
    }

    #[test]
    fn the_line_reaches_the_shapes_it_runs_through() {
        let mut doc = Document::default();
        // Two triangles joined at a corner, and one apart.
        for corners in [
            [(0.0, 0.0), (10.0, 0.0), (5.0, 10.0)],
            [(10.0, 0.0), (20.0, 0.0), (15.0, 10.0)],
            [(40.0, 0.0), (50.0, 0.0), (45.0, 10.0)],
        ] {
            assert!(doc.apply(Edit::AddTriangle {
                corners: corners.map(|(x, y)| Point::new(x, y)),
                snap: 0.0,
            }));
        }
        let shape_of = |t: TriangleId| doc.triangle_ids()[t];
        // Through the first only: it and the one it hangs together with.
        let reached = reached(&doc, Point::new(5.0, -5.0), Point::new(5.0, 3.0));
        assert_eq!(reached.len(), 2, "{reached:?}");
        assert!(reached.iter().all(|&t| shape_of(t)[0] < 6));
        // Starting in the one apart.
        assert_eq!(
            super::reached(&doc, Point::new(45.0, 3.0), Point::new(60.0, 3.0)).len(),
            1
        );
        // Through nothing.
        assert!(super::reached(&doc, Point::new(0.0, 50.0), Point::new(50.0, 50.0)).is_empty());
    }
}
