use super::*;

fn doc_with_triangle() -> Document {
    let mut doc = Document::default();
    assert!(doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(5.0, 10.0),
        ],
        snap: 0.0
    }));
    doc
}

#[test]
fn a_piece_is_copied_and_pasted_joined_on_but_never_over_anything() {
    let mut doc = Document::default();
    assert!(doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0)
        ],
        snap: 0.0
    }));
    let red = Color::from_rgb(1.0, 0.0, 0.0);
    assert!(doc.apply(Edit::Paint {
        triangle: 0,
        color: Some(red)
    }));
    let piece = doc.piece(&doc.unique_vertices());
    assert_eq!(piece.triangles.len(), 1);
    // Some of its corners only: no whole face.
    assert!(doc.piece(&[0, 1]).is_empty());

    // In place: over itself.
    assert!(!doc.clone().apply(Edit::Paste {
        piece: piece.clone()
    }));
    // Beside it, sharing an edge: joined on, painted alike.
    let beside = piece.moved(iced::Vector::new(10.0, 0.0));
    assert!(doc.apply(Edit::Paste { piece: beside }));
    assert_eq!(doc.triangle_ids().len(), 2);
    assert_eq!(doc.vertex_count(), 5);
    assert!(doc.colors().all(|color| color == Some(red)));
}

#[test]
fn mirroring_adds_the_mirror_image_joined_on() {
    // A triangle with its right side on the mirror line x = 10, its
    // left corner painted.
    let mut doc = Document::default();
    assert!(doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(0.0, 5.0),
            Point::new(10.0, 0.0),
            Point::new(10.0, 10.0)
        ],
        snap: 0.0
    }));
    let red = Color::from_rgb(1.0, 0.0, 0.0);
    assert!(doc.apply(Edit::Paint {
        triangle: 0,
        color: Some(red)
    }));
    let mirror = Mirror {
        a: Point::new(10.0, -5.0),
        b: Point::new(10.0, 20.0),
    };
    assert_eq!(mirror.reflect(Point::new(0.0, 5.0)), Point::new(20.0, 5.0));
    assert!(!doc.overlaps_under(None, &[mirror.affine()]));

    let shown = doc.with_mirror(mirror);
    assert!(doc.apply(Edit::Mirror { mirror }));
    // Joined along the line: four vertices, the image painted alike.
    assert_eq!(doc.vertex_count(), 4);
    assert_eq!(doc.triangle_ids().len(), 2);
    assert!(doc.colors().all(|color| color == Some(red)));
    assert!(
        doc.unique_vertices()
            .iter()
            .any(|&v| doc.vertex(v) == Point::new(20.0, 5.0))
    );
    // As shown before applying.
    assert_eq!(shown.vertex_count(), 4);
    assert_eq!(shown.triangle_ids().len(), 2);

    // A mirror through the drawing would mirror it over itself.
    let through = Mirror {
        a: Point::new(5.0, -5.0),
        b: Point::new(5.0, 20.0),
    };
    assert!(doc.overlaps_under(None, &[through.affine()]));
    assert!(!doc.apply(Edit::Mirror { mirror: through }));
}

#[test]
fn vertices_move_together_without_folding_on_the_way() {
    let mut doc = Document::default();
    assert!(doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(0.0, 0.0),
            Point::new(10.0, 0.0),
            Point::new(0.0, 10.0)
        ],
        snap: 0.0
    }));
    // The whole triangle, far past where any corner alone would fold it.
    let by = iced::Vector::new(100.0, 50.0);
    let moves: Vec<_> = (0..3).map(|v| (v, doc.vertex(v) + by)).collect();
    assert!(doc.apply(Edit::MoveVertices { moves }));
    assert_eq!(doc.vertex(0), Point::new(100.0, 50.0));
    assert_eq!(doc.triangle_ids().len(), 1);
    // Folding it over is rejected.
    let flipped = vec![(1, Point::new(90.0, 50.0))];
    assert!(!doc.apply(Edit::MoveVertices { moves: flipped }));
}

#[test]
fn faces_keep_their_colour_through_edits() {
    let red = Color::from_rgb8(255, 0, 0);
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::Paint {
        triangle: 0,
        color: Some(red),
    }));
    assert_eq!(doc.color(0), Some(red));

    // Moving a corner keeps it; splitting gives every piece its colour.
    assert!(doc.apply(Edit::MoveVertex {
        id: 2,
        to: Point::new(5.0, 12.0),
    }));
    assert_eq!(doc.color(0), Some(red));
    assert!(doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 4.0),
    }));
    assert_eq!(doc.triangle_ids().len(), 3);
    assert!(doc.colors().all(|c| c == Some(red)));

    // A face added beside it (in empty space) gets the default.
    assert!(doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0
    }));
    assert_eq!(doc.colors().filter(|c| *c == Some(DEFAULT_FACE)).count(), 1);

    // Clearing.
    assert!(doc.apply(Edit::Paint {
        triangle: 0,
        color: None,
    }));
    assert_eq!(doc.color(0), None);
    assert_eq!(doc.colors().filter(Option::is_none).count(), 1);

    // Splitting an erased face: its pieces stay erased.
    let erased = doc
        .triangles()
        .position(|_| true)
        .filter(|&t| doc.color(t).is_none());
    let [a, b, c] = doc.triangles().nth(erased.unwrap()).unwrap();
    let centre = Point::new((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0);
    assert!(doc.apply(Edit::InsertVertex { at: centre }));
    assert_eq!(doc.colors().filter(Option::is_none).count(), 3);
}

#[test]
fn painting_everything() {
    let red = Color::from_rgb8(255, 0, 0);
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 4.0),
    }));
    assert!(doc.apply(Edit::PaintAll { color: Some(red) }));
    assert!(doc.colors().all(|c| c == Some(red)));
    let style = EdgeStyle {
        color: red,
        width: 2.0,
    };
    assert!(doc.apply(Edit::PaintAllEdges { style: Some(style) }));
    assert_eq!(doc.edge_styles().count(), doc.unique_edges().len());

    assert!(doc.apply(Edit::PaintAll { color: None }));
    assert!(doc.apply(Edit::PaintAllEdges { style: None }));
    assert!(doc.colors().all(|c| c.is_none()));
    assert_eq!(doc.edge_styles().count(), 0);
}

#[test]
fn edges_keep_their_style_through_edits() {
    let style = EdgeStyle {
        color: Color::from_rgb8(255, 0, 0),
        width: 3.0,
    };
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::PaintEdge {
        a: 1,
        b: 0,
        style: Some(style),
    }));
    assert_eq!(doc.edge_style(0, 1), Some(style));
    // Not an edge.
    assert!(!doc.apply(Edit::PaintEdge {
        a: 0,
        b: 7,
        style: Some(style),
    }));

    // Moving an end keeps it.
    assert!(doc.apply(Edit::MoveVertex {
        id: 1,
        to: Point::new(12.0, 0.0),
    }));
    assert_eq!(doc.edge_style(0, 1), Some(style));

    // Cut: both pieces keep it; the new edge inside gets the default.
    assert!(doc.apply(Edit::InsertVertex {
        at: Point::new(6.0, 0.0),
    }));
    let cut = doc.last_vertex();
    assert_eq!(doc.edge_style(0, 1), None);
    assert_eq!(doc.edge_style(0, cut), Some(style));
    assert_eq!(doc.edge_style(cut, 1), Some(style));
    assert_eq!(doc.edge_style(cut, 2), Some(DEFAULT_EDGE));
    // An erased edge, cut: its pieces stay erased.
    assert!(doc.apply(Edit::PaintEdge {
        a: 1,
        b: 2,
        style: None,
    }));
    let (p, q) = (doc.vertex(1), doc.vertex(2));
    assert!(doc.apply(Edit::InsertVertex {
        at: p + (q - p) * 0.5
    }));
    let half = doc.last_vertex();
    assert_eq!(doc.edge_style(1, half), None);
    assert_eq!(doc.edge_style(half, 2), None);

    // Clearing.
    assert!(doc.apply(Edit::PaintEdge {
        a: 0,
        b: cut,
        style: None,
    }));
    assert_eq!(doc.edge_style(0, cut), None);
    assert_eq!(doc.edge_style(cut, 1), Some(style));
}

#[test]
fn extending_shares_the_edge() {
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0
    }));
    assert_eq!(sorted(&doc), vec![[0, 1, 2], [0, 1, 3]]);

    // Moving a shared corner moves both triangles.
    doc.apply(Edit::MoveVertex {
        id: 0,
        to: Point::new(-1.0, 0.0),
    });
    assert!(doc.triangles().all(|t| t[0] == Point::new(-1.0, 0.0)));
}

#[test]
fn splitting_replaces_parent() {
    let mut doc = doc_with_triangle();
    let at = Point::new(5.0, 3.0);
    assert!(doc.apply(Edit::InsertVertex { at }));
    assert_eq!(doc.triangle_ids(), &[[0, 1, 3], [1, 2, 3], [2, 0, 3]]);
}

#[test]
fn collapsing_a_corner_removes_the_triangle() {
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::MoveVertex {
        id: 2,
        to: Point::new(5.0, 0.0)
    }));
    assert!(doc.is_empty());
}

fn split_triangle() -> Document {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 3.0),
    });
    doc
}

fn sorted(doc: &Document) -> Vec<[VertexId; 3]> {
    let mut ids = doc.triangle_ids().to_vec();
    ids.iter_mut().for_each(|t| t.sort_unstable());
    ids.sort_unstable();
    ids
}

#[test]
fn removing_the_triangle_under_a_point() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0,
    });

    assert_eq!(doc.triangle_at(Point::new(50.0, 50.0)), None);
    let below = doc.triangle_at(Point::new(5.0, -3.0)).unwrap();
    assert!(doc.apply(Edit::RemoveTriangle { triangle: below }));
    assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
}

#[test]
fn cutting_an_edge_splits_both_sides() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0,
    });

    assert!(doc.apply(Edit::InsertVertex {
        at: Point::new(3.0, 0.0),
    }));
    assert_eq!(
        sorted(&doc),
        vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
    );
    assert_eq!(area_and_t_junctions(&doc), (100.0, false));

    // Right on a corner there's nothing to cut.
    assert!(!doc.apply(Edit::InsertVertex {
        at: Point::new(0.0, 0.0),
    }));
}

#[test]
fn collapsing_a_split_center_onto_edge_keeps_two() {
    let mut doc = split_triangle();
    assert!(doc.apply(Edit::MoveVertex {
        id: 3,
        to: Point::new(4.0, 0.0)
    }));
    assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
}

#[test]
fn merging_a_split_center_into_corner_restores_parent() {
    let mut doc = split_triangle();
    assert!(doc.apply(Edit::MoveVertex {
        id: 3,
        to: doc.vertex(0)
    }));
    assert_eq!(sorted(&doc), vec![[0, 1, 2]]);
}

#[test]
fn collapsing_onto_a_shared_edge_splits_the_other_side() {
    // Two triangles sharing edge 0–1, the upper one split at 4.
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0,
    });
    doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 3.0),
    });

    assert!(doc.apply(Edit::MoveVertex {
        id: 4,
        to: Point::new(4.0, 0.0)
    }));

    // Nothing uses the edge 0–1 any more; 4 is a corner on both sides.
    assert!(
        !doc.edges()
            .any(|(u, v)| (u, v) == (0, 1) || (u, v) == (1, 0))
    );
    assert_eq!(
        sorted(&doc),
        vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
    );
}

#[test]
fn collapsing_tolerates_rounding() {
    // A long, slanted shared edge: the snapped point is never exactly on it.
    let (a, b) = (Point::new(100.3, 300.7), Point::new(407.1, 290.2));
    let mut doc = Document::default();
    doc.apply(Edit::AddTriangle {
        corners: [a, b, Point::new(250.0, 100.0)],
        snap: 0.0,
    });
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(250.0, 500.0)],
        snap: 0.0,
    });
    doc.apply(Edit::InsertVertex {
        at: Point::new(250.0, 230.0),
    });

    for i in 1..40 {
        let t = i as f32 / 40.0;
        let at = a + (b - a) * t;
        let mut doc = doc.clone();
        assert!(doc.apply(Edit::MoveVertex { id: 4, to: at }));
        assert_eq!(doc.triangle_ids().len(), 4, "t = {t}");
        assert!(
            doc.triangle_ids()
                .iter()
                .all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
        );
    }
}

fn area(doc: &Document) -> f32 {
    doc.triangles().map(|[a, b, c]| area2(a, b, c) / 2.0).sum()
}

#[test]
fn moving_across_an_inner_edge_flips_it() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0,
    });
    doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 3.0),
    });

    // Pull the top centre 4 across the shared edge 0–1 into the bottom
    // triangle: that edge flips to 4–3; the outline stays the same.
    assert!(doc.apply(Edit::MoveVertex {
        id: 4,
        to: Point::new(5.0, -3.0),
    }));
    assert_eq!(
        sorted(&doc),
        vec![[0, 2, 4], [0, 3, 4], [1, 2, 4], [1, 3, 4]]
    );
    assert_eq!(area_and_t_junctions(&doc), (100.0, false));
}

#[test]
fn moving_an_outline_corner_across_an_edge_dents_the_outline() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, -10.0)],
        snap: 0.0,
    });

    // The top corner 2 pulled down into the bottom triangle: the peak
    // becomes a notch, with the shared edge flipped to 2–3.
    assert!(doc.apply(Edit::MoveVertex {
        id: 2,
        to: Point::new(5.0, -5.0),
    }));
    assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
    assert_eq!(area(&doc), 25.0);
}

#[test]
fn moving_out_through_the_outline_drops_the_crossed_triangle() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::InsertVertex {
        at: Point::new(5.0, 3.0),
    });

    // The centre 3 leaves through the bottom edge 0–1: the triangle on
    // that edge goes, the other two stretch down with the vertex.
    assert!(doc.apply(Edit::MoveVertex {
        id: 3,
        to: Point::new(5.0, -4.0),
    }));
    assert_eq!(sorted(&doc), vec![[0, 2, 3], [1, 2, 3]]);
    assert_eq!(area(&doc), 50.0 + 20.0);
    assert!(!area_and_t_junctions(&doc).1);
}

#[test]
fn a_lone_triangle_turns_over() {
    let mut doc = doc_with_triangle();
    assert!(doc.apply(Edit::MoveVertex {
        id: 2,
        to: Point::new(5.0, -10.0),
    }));
    assert_eq!(doc.triangle_ids().len(), 1);
    assert_eq!(area(&doc), 50.0);
}

#[test]
fn extending_into_geometry_fills_only_the_gap() {
    let mut doc = doc_with_triangle();

    // Extending edge 0–1 towards the side the triangle is already on:
    // only the dart-shaped gap around its top corner 2 gets filled.
    assert!(doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(5.0, 20.0)],
        snap: 0.0
    }));
    assert_eq!(sorted(&doc), vec![[0, 1, 2], [0, 2, 3], [1, 2, 3]]);
    assert_eq!(area_and_t_junctions(&doc), (100.0, false));
}

#[test]
fn adding_on_top_of_geometry_fills_only_the_gap() {
    let mut doc = doc_with_triangle();

    // A triangle poking out of the top of the existing one: the part
    // outside it is added, and the existing edges it crosses are split.
    assert!(doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(2.0, 2.0),
            Point::new(8.0, 2.0),
            Point::new(5.0, 20.0),
        ],
        snap: 0.0
    }));
    let (area, t_junctions) = area_and_t_junctions(&doc);
    assert!(!t_junctions);
    // The sides cross at y = 5. Above that the new triangle adds 37.5,
    // less the 12.5 of the existing top corner poking into it.
    assert!((area - (50.0 + 37.5 - 12.5)).abs() < 1e-3, "{area}");

    // Entirely inside existing geometry: nothing to add.
    assert!(!doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(4.0, 1.0),
            Point::new(6.0, 1.0),
            Point::new(5.0, 3.0),
        ],
        snap: 0.0
    }));
}

/// Whether a vertex sits in the middle of another triangle's edge.
pub(super) fn has_t_junction(doc: &Document) -> bool {
    let used: Vec<_> = doc.used_vertices().collect();
    doc.triangle_ids().iter().any(|t| {
        (0..3).any(|k| {
            let (u, v) = (t[k], t[(k + 1) % 3]);
            used.iter().any(|&w| {
                !t.contains(&w)
                    && is_inside_segment(doc.vertex(w), doc.vertex(u), doc.vertex(v))
            })
        })
    })
}

/// Total area and whether any vertex sits in the middle of an edge.
fn area_and_t_junctions(doc: &Document) -> (f32, bool) {
    let area = doc.triangles().map(|[a, b, c]| area2(a, b, c) / 2.0).sum();
    (area, has_t_junction(doc))
}

#[test]
fn collapsing_can_squash_several_triangles() {
    // Two triangles sharing the edge 0–1 (y = 300), each split.
    let mut doc = Document::default();
    doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(100.0, 300.0),
            Point::new(300.0, 300.0),
            Point::new(200.0, 100.0),
        ],
        snap: 0.0,
    });
    doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(200.0, 500.0)],
        snap: 0.0,
    });
    let split = |doc: &mut Document, at| assert!(doc.apply(Edit::InsertVertex { at }));
    split(&mut doc, Point::new(200.0, 230.0)); // centre 4
    split(&mut doc, Point::new(200.0, 370.0)); // centre 5

    // Collapse the top centre onto the shared edge: the bottom side now
    // has four triangles around 5, two of them along the shared line.
    assert!(doc.apply(Edit::MoveVertex {
        id: 4,
        to: Point::new(200.0, 300.0)
    }));
    assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));

    // Collapse the bottom centre onto the shared line between 0 and 4.
    // That flattens both (0, 4, 5) and (4, 1, 5); the bottom is re-split
    // with a new edge from 4 down to 3.
    assert!(doc.apply(Edit::MoveVertex {
        id: 5,
        to: Point::new(150.0, 300.0)
    }));
    assert_eq!(area_and_t_junctions(&doc), (40_000.0, false));
    assert!(doc.edges().any(|e| e == (4, 3) || e == (3, 4)));
}

#[test]
fn merging_joins_separate_triangles() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(20.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(25.0, 10.0),
        ],
        snap: 0.0,
    });
    // Corner to corner, then the second corner: they share an edge.
    assert!(doc.apply(Edit::MoveVertex {
        id: 1,
        to: doc.vertex(3)
    }));
    assert!(doc.apply(Edit::MoveVertex {
        id: 2,
        to: doc.vertex(5)
    }));
    assert_eq!(doc.triangle_ids().len(), 2);
    assert_eq!(
        doc.edges()
            .filter(|&(u, v)| u.min(v) == 3 && u.max(v) == 5)
            .count(),
        2
    );
}

#[test]
fn merging_rejects_overlap() {
    let mut doc = doc_with_triangle();
    doc.apply(Edit::AddTriangle {
        corners: [
            Point::new(20.0, 0.0),
            Point::new(30.0, 0.0),
            Point::new(25.0, 10.0),
        ],
        snap: 0.0,
    });
    // The first triangle would be stretched right across the second.
    assert!(!doc.apply(Edit::MoveVertex {
        id: 0,
        to: doc.vertex(4)
    }));
}

/// The new triangles in `after` (not in `before`) thinner than `min`.
fn thin_new(before: &Document, after: &Document, min: f32) -> Vec<[Point; 3]> {
    after
        .triangle_ids()
        .iter()
        .filter(|&&t| before.find_triangle(t).is_none())
        .map(|t| t.map(|v| after.vertex(v)))
        .filter(|&t| min_height(t) < min)
        .collect()
}

#[test]
fn a_fill_bends_around_a_vertex_close_to_its_side() {
    // A triangle, and one below it whose right corner (250, 340) ends
    // up just under 6 from the left side of the extension below.
    let mut doc = Document::default();
    for corners in [
        [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)],
        [(150.0, 330.0), (250.0, 340.0), (200.0, 420.0)],
    ] {
        let corners = corners.map(|(x, y)| Point::new(x, y));
        assert!(doc.apply(Edit::AddTriangle { corners, snap: 0.0 }));
    }

    let corners = [doc.vertex(0), doc.vertex(1), Point::new(360.0, 380.0)];
    let (plain, _) = doc
        .preview(&[Edit::AddTriangle { corners, snap: 0.0 }])
        .unwrap();
    assert!(!thin_new(&doc, &plain, 5.0).is_empty());

    let (snapped, _) = doc
        .preview(&[Edit::AddTriangle { corners, snap: 5.0 }])
        .unwrap();
    assert_eq!(thin_new(&doc, &snapped, 5.0), Vec::<[Point; 3]>::new());
}

#[test]
fn a_fill_bends_rather_than_splitting_off_a_sliver() {
    // Extending the right edge of a triangle across a separate one, its
    // side crossing that one's edge close to a corner: splitting there
    // would leave a sliver of the existing triangle.
    let mut doc = Document::default();
    for corners in [
        [(100.0, 300.0), (300.0, 300.0), (200.0, 100.0)],
        [(330.0, 280.0), (450.0, 300.0), (400.0, 150.0)],
    ] {
        let corners = corners.map(|(x, y)| Point::new(x, y));
        assert!(doc.apply(Edit::AddTriangle { corners, snap: 0.0 }));
    }

    let corners = [doc.vertex(1), doc.vertex(2), Point::new(550.0, 260.0)];
    let (plain, _) = doc
        .preview(&[Edit::AddTriangle { corners, snap: 0.0 }])
        .unwrap();
    assert!(!thin_new(&doc, &plain, 5.0).is_empty());

    let (snapped, _) = doc
        .preview(&[Edit::AddTriangle { corners, snap: 5.0 }])
        .unwrap();
    assert_eq!(thin_new(&doc, &snapped, 5.0), Vec::<[Point; 3]>::new());
}

#[test]
fn rejects_degenerate_and_duplicate() {
    let mut doc = doc_with_triangle();
    assert!(!doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), Point::new(20.0, 0.0)],
        snap: 0.0
    }));
    assert!(!doc.apply(Edit::AddTriangle {
        corners: [doc.vertex(0), doc.vertex(1), doc.vertex(2)],
        snap: 0.0
    }));
    assert_eq!(doc.triangle_ids().len(), 1);
}
