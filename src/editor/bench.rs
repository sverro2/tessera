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

/// Drags each vertex of a real drawing (the layer with most triangles
/// of the file in `TESSERA_BENCH`) across its shape, timing each mouse
/// move's work without the time budget. Run with
/// `TESSERA_BENCH=file.tessera cargo test bench_file --release -- --ignored --nocapture`.
#[test]
#[ignore = "a benchmark: slow, and needs a drawing to time"]
fn bench_file() {
    let Ok(path) = std::env::var("TESSERA_BENCH") else {
        return;
    };
    let contents = crate::file::open(&std::fs::read_to_string(path).unwrap()).unwrap();
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

    let mut all = Vec::new();
    for v in doc.unique_vertices() {
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
    for v in doc.unique_vertices() {
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
