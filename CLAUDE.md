# Tessera

A desktop app (Rust, [iced](https://github.com/iced-rs/iced) from git `master`)
for drawing low-poly pictures: triangles that share their corners, shaped in
the **shape mode**, their faces and edges coloured in the **paint mode**,
on layers. Saved as `.tessera` (a zip of JSON), exported as SVG or PNG.

## Checking work

```sh
cargo test                        # all tests; quick (dev builds are optimised a little)
cargo clippy --all-targets        # must stay free of warnings
cargo fmt                         # the code is rustfmt-clean; keep it so
```

Changes to drawing the canvas: compare pictures before and after (they
should match unless the look is meant to change):

```sh
TESSERA_BENCH=drawing.tessera TESSERA_SHOTS=dir cargo test --release shoot_canvas -- --ignored --nocapture
```

Changes for speed: measure with the benchmarks in `src/editor/bench.rs`
(ignored tests), before and after. Most take `TESSERA_BENCH` (a drawing),
`TESSERA_BENCH_LAYER` (the layer to work on, by name) and
`TESSERA_BENCH_MESHES` (only the GPU path), e.g.
`cargo test --release bench_frames -- --ignored --nocapture`. Large drawings
to try are in `testdata/` (`generate_test_drawings` writes them).

## How it fits together

The iced (Elm) architecture throughout: state, messages, `update`, `view`.

- `main.rs`: `Tessera`, the app's state; its `Message` (one variant per
  area); `update`/`handle`, `view`, and undo (a stack of whole `Layers`,
  drawings shared until changed).
- `update/<area>.rs`: each area's own message enum (`PaintMessage`,
  `LayerAction`, ...) and its handler `Tessera::update_<area>`. New app
  behaviour goes here: a message, and its arm in the handler.
- `panels.rs`, `dialogs.rs`: the `view` around the canvas (menus, toolbars,
  layers panel) and over it (dialogs).
- `editor.rs` + `editor/`: the canvas, a `canvas::Program` drawn by its own
  widget (`meshes::EditorView`). It gets a `Scene` (the layers) and
  `Settings` each view, keeps its own `State` (the interaction going on),
  and publishes `editor::Message`s (edits, panning, framing...) that
  `update/canvas.rs` carries out. It never changes the drawing itself.
  - `input.rs`: what keys, the mouse, the wheel and frames do.
  - `drag.rs`: following a drag to a valid edit (snapping via `guides.rs`
    and `grid.rs`, limiting moves; slow cases on other threads via
    `Worker`).
  - `draw.rs`: the scene: each layer from its cache, those changed drawn
    side by side (rayon); over them what a drag would do, highlights.
  - `painter.rs`: drawing one layer (`Painter`, which can go to other
    threads): only what's in view, thinning out what crowds when zoomed
    out; what it works out from a drawing is remembered by its revision
    (`Memos`, `Derived`).
  - `meshes.rs`: faces and lines as GPU triangle meshes. Without a GPU
    (the tiny-skia fallback) everything is drawn as canvas paths instead;
    both must keep working.
  - `selection.rs`, `extrude.rs`, `mirror.rs`, `painting.rs`: the tools.
- `document.rs` + `document/`: one layer's drawing: vertices, triangles,
  face colours, edge styles. Changed only through `Edit`s (`apply`,
  `apply_all`, `preview`), which keep it valid (no overlaps, no slivers);
  `fill.rs` fits new triangles round what's there, `cells.rs` is a
  spatial index.
- `layers.rs`: the layers and groups (front first), visibility, ordering.
- `keys.rs`: keyboard shortcuts: an `Action` per thing a key does, with its
  `context`, `label` and default keys; users can rebind them.
- `file.rs` (the format), `svg.rs`/`raster.rs` (export), `disk.rs`,
  `recent.rs`, `places.rs`, `snap.rs`: files and settings kept between runs.
- `camera.rs` (world <-> screen), `geometry.rs`, `joints.rs` (painted
  edges of different widths meeting), `fade.rs` (crossfaded edges).

## Things to keep true

- **A document's `revision` is unique to its contents**: every edit, and
  every preview, gets a new one. Caches and memos are keyed by it; a
  document that changed without a new revision draws stale.
- **The editor only asks**: it publishes messages; the app changes state.
  Edits carry the revision they were worked out for and are dropped if it
  changed meanwhile.
- **What's drawn is cached per layer** (`LayerKey`: drawing, style, view,
  size). Anything new that changes how a layer looks must be in the key.
- **Undo** is `push_undo(before)` with the layers as they were. View state
  kept outside `Layers` (camera, isolated layers) isn't undone; changes
  that are only a way of looking (expanding a group) push no step.

## Adding things

- A key: an `Action` in `keys.rs` (its context, label, default keys), then
  handle it in `editor/input.rs` (canvas) or `Tessera::key_pressed`
  (`main.rs`, app-wide).
- App behaviour: a message in the area's `update/*.rs` and its handler;
  UI in `panels.rs`/`dialogs.rs` sending it.
- Canvas behaviour: `editor/input.rs` (and `drag.rs` for drags), publishing
  an `editor::Message` if the app must act.
- Every change comes with tests: `src/tests.rs` (the app, message by
  message), `src/editor/tests/<area>.rs` (events through the canvas; the
  helpers to set them up in `src/editor/tests.rs`), `src/document/tests.rs`
  (edits), or a module's own.

## Style

- Comments and doc comments in plain (British) English, saying what
  something is or why, from the user's side: "The layers pointed at in the
  layers panel, lit up", not "This function iterates...". Short, often a
  phrase with a colon.
- Test names are sentences of the behaviour: `slash_frames_the_shape_pointed_at_else_the_layer`.
- Commit messages: a short title, then why, and measurements for speed-ups.
