//! Random edits, checking the document stays valid through them all.

use super::*;
use crate::geometry::closest_on_segment;

#[test]
fn random_moves() {
    let mut seed = 12345u64;
    let mut rand = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32
    };

    let (mut ok, mut rejected) = (0, 0);
    for _ in 0..200 {
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
        doc.apply(Edit::AddTriangle {
            corners: [doc.vertex(1), doc.vertex(2), Point::new(350.0, 150.0)],
            snap: 0.0,
        });
        doc.apply(Edit::InsertVertex {
            at: Point::new(200.0, 230.0),
        });
        doc.apply(Edit::InsertVertex {
            at: Point::new(200.0, 370.0),
        });

        for _ in 0..20 {
            let used: Vec<_> = doc.used_vertices().collect();
            let id = used[(rand() * used.len() as f32) as usize % used.len()];
            let to = Point::new(rand() * 500.0, rand() * 600.0);
            let before = doc.clone();
            if doc.apply(Edit::MoveVertex { id, to }) {
                ok += 1;
                // Only exact T-junctions (as rearranging would create) count;
                // a random target merely touching an edge is fine.
                let used: Vec<_> = doc.used_vertices().collect();
                let t_junction = doc.triangle_ids().iter().any(|t| {
                    (0..3).any(|k| {
                        let (u, v) = (doc.vertex(t[k]), doc.vertex(t[(k + 1) % 3]));
                        used.iter().any(|&w| {
                            let p = doc.vertex(w);
                            !t.contains(&w)
                                && closest_on_segment(p, u, v).1 < 1e-4
                                && p.distance(u) > 1e-3
                                && p.distance(v) > 1e-3
                        })
                    })
                });
                assert!(!t_junction, "{before:?} {id} -> {to:?}");
            } else {
                rejected += 1;
            }
        }
    }
    println!("ok {ok}, rejected {rejected}");
}

#[test]
fn random_fills() {
    let mut seed = 777u64;
    let mut rand = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32
    };

    let (mut ok, mut empty, mut rejected) = (0, 0, 0);
    for _ in 0..30 {
        let mut doc = Document::default();
        doc.apply(Edit::AddTriangle {
            corners: [
                Point::new(200.0, 300.0),
                Point::new(300.0, 300.0),
                Point::new(250.0, 200.0),
            ],
            snap: 0.0,
        });

        for _ in 0..15 {
            let r: Vec<f32> = (0..8).map(|_| rand()).collect();
            let point = |i: usize| Point::new(r[i] * 500.0, r[i + 1] * 500.0);
            let edit = if r[0] < 0.5 {
                let edges: Vec<_> = doc.edges().collect();
                let (a, b) = edges[(r[1] * edges.len() as f32) as usize % edges.len()];
                Edit::AddTriangle {
                    corners: [doc.vertex(a), doc.vertex(b), point(2)],
                    snap: 0.0,
                }
            } else {
                Edit::AddTriangle {
                    corners: [point(2), point(4), point(6)],
                    snap: 0.0,
                }
            };

            let started = std::time::Instant::now();
            let mut unchecked = doc.clone();
            if !unchecked.apply_unchecked(edit.clone(), &mut Changes::default(), None) {
                empty += 1;
            } else if doc.apply(edit.clone()) {
                ok += 1;
                assert!(!super::tests::has_t_junction(&doc), "{edit:?}");
            } else {
                rejected += 1;
            }
            if std::env::var("FILL_TRACE").is_ok() {
                println!(
                    "{} tris, {} vertices, {:?}",
                    doc.triangle_ids().len(),
                    doc.vertices.len(),
                    started.elapsed()
                );
            }
        }
    }
    println!("fills: ok {ok}, nothing to add {empty}, rejected {rejected}");
}

#[test]
fn random_merges() {
    let mut seed = 4242u64;
    let mut rand = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32) / (1u64 << 31) as f32
    };

    let (mut ok, mut rejected) = (0, 0);
    for _ in 0..40 {
        // A few triangles here and there.
        let mut doc = Document::default();
        for _ in 0..6 {
            let (x, y) = (rand() * 400.0, rand() * 400.0);
            let corners = [
                (0.0, 0.0),
                (60.0 + rand() * 60.0, 0.0),
                (rand() * 60.0, 80.0),
            ]
            .map(|(dx, dy)| Point::new(x + dx, y + dy));
            doc.apply(Edit::AddTriangle { corners, snap: 0.5 });
        }

        for _ in 0..10 {
            // One face's corners moved over the rest, or pasted there.
            let triangles = doc.triangle_ids().to_vec();
            let t = triangles[(rand() * triangles.len() as f32) as usize % triangles.len()];
            let by = iced::Vector::new(rand() * 200.0 - 100.0, rand() * 200.0 - 100.0);
            let edit = if rand() < 0.5 {
                Edit::MoveVertices {
                    moves: t.iter().map(|&v| (v, doc.vertex(v) + by)).collect(),
                }
            } else {
                Edit::Paste {
                    piece: doc.piece(&t).moved(by),
                }
            };
            let edit = Edit::Merging {
                edit: Box::new(edit),
                snap: 0.5,
                held: Vec::new(),
            };
            let before = doc.clone();
            if doc.apply(edit.clone()) {
                ok += 1;
                let shapes: Vec<[Point; 3]> = doc.triangles().collect();
                for (i, &s) in shapes.iter().enumerate() {
                    assert!(area2(s[0], s[1], s[2]) > 0.0, "{before:?} {edit:?}");
                    for &u in &shapes[i + 1..] {
                        assert!(!overlap(s, u), "{before:?} {edit:?}");
                    }
                }
                assert!(!super::tests::has_t_junction(&doc), "{before:?} {edit:?}");
            } else {
                // Moved, a face joined on may fold over: that's turned down.
                rejected += 1;
            }
        }
    }
    println!("merges: ok {ok}, rejected {rejected}");
    assert!(ok > rejected);
}
