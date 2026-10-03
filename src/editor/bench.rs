use super::*;
use std::time::Instant as T;

/// The candidates tried on all cores give what trying them in turn
/// (on one thread) does.
#[test]
fn parallel_search_finds_what_a_sequential_one_does() {
    let doc = grid(6, 40.0);
    let cache = Caches::default();
    let editor = super::tests::editor(&doc, &cache);
    let one_thread = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut moved = 0;
    for v in doc.unique_vertices() {
        let from = doc.vertex(v);
        // Far across the grid, where most positions are blocked.
        for to in [
            Point::new(250.0, 250.0),
            Point::new(-50.0, 120.0),
            Point::new(400.0, 20.0),
        ] {
            let target = from + (to - from) * 0.8;
            let parallel = editor.limit_move(v, target).map(|(p, _)| p);
            let worker = editor.worker();
            let sequential = one_thread.install(|| {
                let caches = Caches::default();
                let editor = worker.editor(&caches);
                editor.limit_move(v, target).map(|(p, _)| p)
            });
            assert_eq!(parallel, sequential, "vertex {v} to {target:?}");
            moved += usize::from(parallel.is_some_and(|p| p != target));
        }
    }
    // Some moves were held back, so candidates were tried.
    assert!(moved > 0);
}

fn grid(n: usize, size: f32) -> Document {
    let mut doc = Document::default();
    let p = |i: usize, j: usize| Point::new(50.0 + i as f32 * size, 50.0 + j as f32 * size);
    for i in 0..n - 1 {
        for j in 0..n - 1 {
            for corners in [
                [p(i, j), p(i + 1, j), p(i, j + 1)],
                [p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)],
            ] {
                doc.apply(Edit::AddTriangle { corners, snap: 0.0 });
            }
        }
    }
    doc
}

fn time<R>(label: &str, runs: u32, mut f: impl FnMut() -> R) {
    let start = T::now();
    for _ in 0..runs {
        std::hint::black_box(f());
    }
    println!("BENCH {label}: {:?}", start.elapsed() / runs);
}

/// Drags vertices of a real drawing (60 of them, of the layer with most
/// triangles of the file in `TESSERA_BENCH`) across its shape, timing each mouse
/// move's work without the time budget. Run with
/// `TESSERA_BENCH=file.tessera cargo test bench_file --release -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow, and needs a drawing to time"]
fn bench_file() {
    let Ok(path) = std::env::var("TESSERA_BENCH") else {
        return;
    };
    let contents = crate::file::open(&std::fs::read(path).unwrap()).unwrap();
    let layers = contents.layers.layers();
    let doc = &layers
        .iter()
        .max_by_key(|layer| layer.document.triangle_ids().len())
        .unwrap()
        .document;
    let cache = Caches::default();
    let mut editor = super::tests::editor(doc, &cache);
    editor.camera = contents.camera;
    let (min, max) = doc.bounds().unwrap();
    let centre = Point::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
    println!(
        "BENCH file: {} vertices, {} triangles",
        doc.vertex_count(),
        doc.triangle_ids().len()
    );

    // An even sample of them: all would take long in a large drawing.
    let all_vertices = doc.unique_vertices();
    let step = (all_vertices.len() / 60).max(1);
    let sample: Vec<VertexId> = all_vertices.into_iter().step_by(step).collect();
    let mut all = Vec::new();
    for &v in &sample {
        let from = doc.vertex(v);
        // Across the shape: through its middle, and as far out again.
        let to = centre + (centre - from);
        for step in 1..=40 {
            let p = from + (to - from) * (step as f32 / 40.0);
            let start = std::time::Instant::now();
            editor.limit_move(v, p);
            all.push((start.elapsed(), v, step));
        }
    }
    use canvas::Program;
    // And as the canvas gets it: pressing on each vertex, dragging.
    let bounds = Rectangle::new(Point::ORIGIN, iced::Size::new(1000.0, 800.0));
    let mut updates = Vec::new();
    for &v in &sample {
        let mut state = State::default();
        let from = editor.camera.to_screen(doc.vertex(v));
        let to = editor.camera.to_screen(centre + (centre - doc.vertex(v)));
        // Each move and the frame after it.
        let mut send = |event: mouse::Event, p: Point| {
            let cursor = mouse::Cursor::Available(p);
            let frame = Event::Window(window::Event::RedrawRequested(Instant::now()));
            let start = std::time::Instant::now();
            editor.update(&mut state, &Event::Mouse(event), bounds, cursor);
            editor.update(&mut state, &frame, bounds, cursor);
            start.elapsed()
        };
        send(mouse::Event::CursorMoved { position: from }, from);
        send(mouse::Event::ButtonPressed(mouse::Button::Left), from);
        for step in 1..=40 {
            let p = from + (to - from) * (step as f32 / 40.0);
            updates.push(send(mouse::Event::CursorMoved { position: p }, p));
        }
        send(mouse::Event::ButtonReleased(mouse::Button::Left), to);
    }
    updates.sort();
    let total_updates: Duration = updates.iter().sum();
    println!(
        "BENCH file: update mean {:?}, p90 {:?}, max {:?}",
        total_updates / updates.len() as u32,
        updates[updates.len() * 9 / 10],
        updates.last().unwrap()
    );

    all.sort();
    let total: Duration = all.iter().map(|(d, ..)| *d).sum();
    println!(
        "BENCH file: {} moves, mean {:?}",
        all.len(),
        total / all.len() as u32
    );
    for q in [0.5, 0.9, 0.99] {
        println!(
            "BENCH file: p{} {:?}",
            q * 100.0,
            all[(all.len() as f32 * q) as usize].0
        );
    }
    for (d, v, step) in all.iter().rev().take(5) {
        println!("BENCH file: slowest {d:?} (vertex {v}, step {step})");
    }
}

/// Times one mouse move's worth of work (as `update` does it) on a
/// 10×10 grid, at normal zoom and zoomed far out, without and with the
/// time budget. Run with `cargo test bench -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow"]
fn bench_editor() {
    let doc = grid(10, 40.0);
    let cache = Caches::default();
    let at = |x: f32, y: f32| {
        doc.unique_vertices()
            .into_iter()
            .find(|&v| doc.vertex(v) == Point::new(x, y))
            .unwrap()
    };
    let centre = at(210.0, 210.0);
    let corner = at(410.0, 410.0);
    let (a, b) = doc
        .unique_edges()
        .into_iter()
        .find(|&(u, v)| doc.vertex(u).x == 410.0 && doc.vertex(v).x == 410.0)
        .unwrap();
    let start = doc.vertex(a);

    for (label, zoom) in [("near", 1.0), ("far", 0.1)] {
        for budget in [None, Some(BUDGET)] {
            let editor = Editor {
                document: &doc,
                current: 0,
                crossfade: Crossfade::default(),
                show_edges: true,
                mirrors: &[],
                shown: true,
                camera: Camera {
                    zoom,
                    ..Camera::default()
                },
                below: Vec::new(),
                above: Vec::new(),
                caches: &cache,
                background: None,
                tool: Tool::Shape,
                proportional: None,
                clipboard: None,
                keys: Some(crate::keys::defaults()),
                lit: Vec::new(),
                grid: Grid::default(),
                deadline: Cell::new(None),
            };
            let run = |f: &dyn Fn(&Editor)| {
                editor
                    .deadline
                    .set(budget.map(|budget| std::time::Instant::now() + budget));
                f(&editor);
            };
            let label = format!(
                "{label} {}",
                if budget.is_some() {
                    "budget"
                } else {
                    "unbounded"
                }
            );
            let far = zoom < 1.0;
            let scale = if far { 10.0 } else { 1.0 };

            time(&format!("{label}: small move"), 5, || {
                run(&|e| {
                    e.limit_move(centre, Point::new(215.0, 205.0));
                })
            });
            time(&format!("{label}: blocked move"), 3, || {
                run(&|e| {
                    e.limit_move(centre, Point::new(600.0 * scale, 600.0 * scale));
                })
            });
            time(&format!("{label}: corner outward"), 5, || {
                run(&|e| {
                    e.limit_move(corner, Point::new(450.0, 450.0));
                })
            });
            time(&format!("{label}: extend over crowded"), 3, || {
                run(&|e| {
                    e.limit_extend(
                        a,
                        b,
                        start,
                        Point::new(470.0 * scale.sqrt(), 600.0 * scale.sqrt()),
                    );
                })
            });
        }
    }
}

/// Times the work behind each feature on grids of 162, 722 and 3042
/// triangles, to tell what's slow and how it grows. Run with
/// `cargo test --release bench_suite -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow"]
fn bench_suite() {
    for n in [10, 20, 40] {
        let doc = grid(n, 40.0);
        let cache = Caches::default();
        let editor = super::tests::editor(&doc, &cache);
        let t = doc.triangle_ids().len();
        let label = |what: &str| format!("{t:>5} tris: {what}");
        let first = doc.unique_vertices()[0];
        let side = 50.0 + (n - 1) as f32 * 40.0;
        let runs = if n == 40 { 3 } else { 20 };

        time(&label("connected (shape highlight)"), runs, || {
            doc.connected(first)
        });
        time(&label("hit test (hover), 100 points"), runs, || {
            for i in 0..100 {
                let x = 50.0 + (i as f32 * 37.0) % (side - 50.0);
                let y = 50.0 + (i as f32 * 53.0) % (side - 50.0);
                editor.hit_test(Point::new(x, y));
            }
        });
        time(&label("check: triangle on the outside"), runs, || {
            editor.check(
                vec![Edit::AddTriangle {
                    corners: [
                        Point::new(side, 50.0),
                        Point::new(side, 90.0),
                        Point::new(side + 35.0, 70.0),
                    ],
                    snap: editor.snap_distance(),
                }],
                Point::new(side + 35.0, 70.0),
            )
        });
        time(&label("preview: move a vertex a little"), runs, || {
            doc.preview(&[Edit::MoveVertex {
                id: first,
                to: doc.vertex(first) + Vector::new(-3.0, -2.0),
            }])
        });
        time(&label("outlines"), runs, || editor.outlines(|_| false));
        let all = doc.unique_vertices();
        time(&label("extrude all, upwards"), runs, || {
            editor.extrude(&all, Vector::new(0.0, -60.0), Point::new(100.0, -10.0))
        });
        let from = Point::new(side + 60.0, 100.0);
        time(&label("guided creation (Shift)"), runs, || {
            editor.guided_creation(from, Point::new(side + 120.0, 140.0))
        });
        time(&label("grid creation (Ctrl)"), runs, || {
            editor.grid_create(from, Point::new(side + 123.0, 137.0), false)
        });
        let mut layers = crate::layers::Layers::default();
        let id = layers.first_layer();
        layers.layer_mut(id).unwrap().document = std::sync::Arc::new(doc.clone());
        time(&label("svg export"), runs, || {
            crate::svg::export(&layers, None)
        });
        time(&label("save file"), runs, || {
            crate::file::save(&layers, Camera::default(), &HashMap::new(), id)
        });
        let saved = crate::file::save(&layers, Camera::default(), &HashMap::new(), id);
        time(&label("open file"), runs.min(5), || {
            crate::file::open(&saved).unwrap()
        });
        let mirror = doc::Mirror {
            a: Point::new(side + 20.0, 0.0),
            b: Point::new(side + 20.0, 10.0),
        };
        layers.set_mirror(id, Some(mirror));
        time(&label("mirrored layer drawing()"), runs, || {
            layers.layer(id).unwrap().drawing()
        });
        let mut proportional = super::tests::editor(&doc, &cache);
        proportional.proportional = Some(100.0);
        time(&label("proportional weights"), runs, || {
            proportional.weights(&[first])
        });
    }
}

/// Splits `check` up, to tell which part grows with the drawing. Run with
/// `cargo test --release bench_check_parts -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow"]
fn bench_check_parts() {
    for n in [20, 40] {
        let doc = grid(n, 40.0);
        let cache = Caches::default();
        let editor = super::tests::editor(&doc, &cache);
        let t = doc.triangle_ids().len();
        let side = 50.0 + (n - 1) as f32 * 40.0;
        let edits = vec![Edit::AddTriangle {
            corners: [
                Point::new(side, 50.0),
                Point::new(side, 90.0),
                Point::new(side + 35.0, 70.0),
            ],
            snap: editor.snap_distance(),
        }];
        let label = |what: &str| format!("{t:>5} tris: {what}");
        time(&label("preview, shape only"), 5, || {
            doc.preview_shape_until(&edits, None)
        });
        time(&label("preview, with paint"), 5, || {
            doc.preview_until(&edits, None)
        });
        let (result, changes) = doc.preview_until(&edits, None).unwrap();
        time(&label("new triangles"), 5, || {
            new_triangles(&doc, &result, &|v| changes.kept(v)).count()
        });
        time(&label("sliver test"), 5, || {
            result.triangle_ids().iter().any(|&t| {
                let h = min_height(t.map(|v| editor.camera.to_screen(result.vertex(v))));
                h < MIN_THICKNESS && doc.find_triangle(t).is_none()
            })
        });
        time(&label("clone"), 5, || doc.clone());
    }
}

/// Extruding a whole grid's top: for profiling. Run with
/// `cargo test --release bench_extrude -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow"]
fn bench_extrude() {
    let doc = grid(40, 40.0);
    let cache = Caches::default();
    let editor = super::tests::editor(&doc, &cache);
    let all = doc.unique_vertices();
    time("3042 tris: extrusion (piece)", 20, || {
        editor.extrusion(&all, Vector::new(0.0, -60.0))
    });
    time("3042 tris: extrude (with check)", 20, || {
        editor.extrude(&all, Vector::new(0.0, -60.0), Point::new(100.0, -10.0))
    });
}

/// Lining up a new triangle with Shift beside a large grid: for profiling.
/// Run with `cargo test --release bench_guided -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow"]
fn bench_guided() {
    let doc = grid(40, 40.0);
    let cache = Caches::default();
    let editor = super::tests::editor(&doc, &cache);
    let side = 50.0 + 39.0 * 40.0;
    let from = Point::new(side + 60.0, 100.0);
    time("3042 tris: guided creation", 400, || {
        editor.guided_creation(from, Point::new(side + 120.0, 140.0))
    });
}

/// Writes large drawings to `testdata/`, to open and try (and time) by
/// hand. Run with `cargo test --release generate_test_drawings -- --ignored`.
#[test]
#[ignore = "writes files"]
fn generate_test_drawings() {
    use crate::layers::Layers;
    use std::sync::Arc;

    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    std::fs::create_dir_all(&folder).unwrap();
    // A colour along a gradient, by `t` from 0 to 1.
    let gradient = |t: f32| {
        let t = t.clamp(0.0, 1.0);
        Color::from_rgb(0.2 + 0.7 * t, 0.35 + 0.3 * (1.0 - t), 0.85 - 0.5 * t)
    };
    // A triangle grid, `n` × `n` points `size` apart from `origin`, painted
    // along a gradient across it.
    let grid = |n: usize, size: f32, origin: Point| {
        let at = |i: usize, j: usize| i * n + j;
        let vertices = (0..n * n)
            .map(|k| {
                Point::new(
                    origin.x + (k / n) as f32 * size,
                    origin.y + (k % n) as f32 * size,
                )
            })
            .collect();
        let mut triangles = Vec::new();
        for i in 0..n - 1 {
            for j in 0..n - 1 {
                triangles.push([at(i, j), at(i + 1, j), at(i, j + 1)]);
                triangles.push([at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
        let count = triangles.len();
        let mut doc = Document::from_parts(vertices, triangles).unwrap();
        doc.set_colors((0..count).map(|k| Some(gradient(k as f32 / count as f32))));
        doc
    };
    // Hexagons (six triangles round a centre) `count` of them, scattered
    // (by a fixed sequence, so it's the same each time) without overlapping.
    let hexagons = |count: usize, seed: u64| {
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as f32 / (1u64 << 31) as f32
        };
        let (mut vertices, mut triangles, mut colors) = (Vec::new(), Vec::new(), Vec::new());
        let columns = (count as f32).sqrt().ceil() as usize;
        for k in 0..count {
            // In a jittered lattice, so they stay apart.
            let centre = Point::new(
                (k % columns) as f32 * 90.0 + next() * 30.0,
                (k / columns) as f32 * 90.0 + next() * 30.0,
            );
            let radius = 20.0 + next() * 15.0;
            let first = vertices.len();
            vertices.push(centre);
            for s in 0..6 {
                let angle = s as f32 * std::f32::consts::FRAC_PI_3 + next() * 0.2;
                vertices.push(centre + Vector::new(angle.cos(), angle.sin()) * radius);
            }
            let color = Some(gradient(next()));
            for s in 0..6 {
                triangles.push([first, first + 1 + s, first + 1 + (s + 1) % 6]);
                colors.push(color);
            }
        }
        let mut doc = Document::from_parts(vertices, triangles).unwrap();
        doc.set_colors(colors);
        doc
    };
    // Every edge painted as new ones are.
    let edged = |mut doc: Document| {
        let edges = doc.unique_edges();
        doc.set_edge_styles(edges.into_iter().map(|(a, b)| (a, b, doc::DEFAULT_EDGE)));
        doc
    };
    let save = |name: &str, layers: &Layers| {
        let points: Vec<Point> = layers
            .layers()
            .iter()
            .filter_map(|layer| layer.drawing().bounds())
            .flat_map(|(min, max)| [min, max])
            .collect();
        let camera = Camera::default().framing(&points, iced::Size::new(1200.0, 800.0), 60.0);
        let bytes = crate::file::save(layers, camera, &HashMap::new(), layers.first_layer());
        std::fs::write(folder.join(name), bytes).unwrap();
        println!("wrote testdata/{name}");
    };
    let one = |doc: Document| {
        let mut layers = Layers::default();
        let id = layers.first_layer();
        layers.layer_mut(id).unwrap().document = Arc::new(edged(doc));
        (layers, id)
    };

    let (layers, _) = one(grid(80, 30.0, Point::ORIGIN));
    save("grid-12k.tessera", &layers);

    let (mut layers, first) = one(hexagons(400, 1));
    let second = layers.add_layer(first);
    layers.layer_mut(second).unwrap().document = Arc::new(edged(hexagons(200, 2)));
    save("scatter-4k.tessera", &layers);

    let (mut layers, id) = one(grid(40, 30.0, Point::ORIGIN));
    let edge = 39.0 * 30.0 + 40.0;
    layers.set_mirror(
        id,
        Some(doc::Mirror {
            a: Point::new(edge, 0.0),
            b: Point::new(edge, 10.0),
        }),
    );
    save("mirrored-3k.tessera", &layers);

    let (mut layers, mut last) = one(grid(20, 30.0, Point::ORIGIN));
    for k in 1..8 {
        let id = layers.add_layer(last);
        let offset = Point::new(k as f32 * 70.0, k as f32 * 45.0);
        layers.layer_mut(id).unwrap().document = Arc::new(edged(grid(20, 30.0, offset)));
        last = id;
    }
    save("layers-8.tessera", &layers);
}

/// A few drags across the middle of `testdata/grid-12k.tessera` (see
/// [`generate_test_drawings`]), without the time budget: for profiling.
/// Run with `cargo test --release bench_big_drag -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow, and needs the generated drawings"]
fn bench_big_drag() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/grid-12k.tessera");
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let contents = crate::file::open(&bytes).unwrap();
    let doc = &contents.layers.layers()[0].document;
    let cache = Caches::default();
    let mut editor = super::tests::editor(doc, &cache);
    editor.camera = contents.camera;
    // A vertex in the middle, dragged a little, then far across.
    let middle = doc
        .unique_vertices()
        .into_iter()
        .min_by(|&u, &v| {
            let d = |v| doc.vertex(v).distance(Point::new(1185.0, 1185.0));
            d(u).total_cmp(&d(v))
        })
        .unwrap();
    let from = doc.vertex(middle);
    let small = from + Vector::new(4.0, 3.0);
    time("12482 tris: preview of a small move", 300, || {
        doc.preview(&[Edit::MoveVertex {
            id: middle,
            to: small,
        }])
    });
    time("12482 tris: move_to (with snaps)", 3, || {
        editor.move_to(middle, small)
    });
    println!(
        "BENCH-INFO zoom {}, small move works directly: {}",
        editor.camera.zoom,
        editor.move_to(middle, small).is_some()
    );
    if std::env::var("BENCH_PREVIEW_ONLY").is_ok() {
        return;
    }
    time("12482 tris: small move", 3, || {
        editor.limit_move(middle, from + Vector::new(4.0, 3.0))
    });
    time("12482 tris: move across", 1, || {
        editor.limit_move(middle, from + Vector::new(300.0, 200.0))
    });
}
