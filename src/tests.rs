use super::*;
use document::Edit;
use iced::Point;

/// Adds a triangle, `x` to the right.
fn edit(app: &mut Tessera, x: f32) {
    let corners = [
        Point::new(x + 100.0, 300.0),
        Point::new(x + 300.0, 300.0),
        Point::new(x + 200.0, 100.0),
    ];
    let revision = app.document().revision();
    let _ = app.update(Message::Editor(editor::Message::Edit {
        edits: vec![Edit::AddTriangle { corners, snap: 0.0 }],
        revision,
    }));
}

#[test]
fn a_document_is_placed_with_its_corner_on_the_grid() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let page = |app: &mut Tessera, message| {
        let _ = app.update(Message::Page(message));
    };
    // Odd sizes, around the triangle and then where it was: its corner
    // lands on the (10 px) grid either way.
    for (width, height) in [("1005", "703"), ("333", "777")] {
        page(&mut app, PageMessage::Show(true));
        page(&mut app, PageMessage::Width(width.into()));
        page(&mut app, PageMessage::Height(height.into()));
        page(&mut app, PageMessage::Apply);
        let corner = app.layers.page().unwrap().min();
        for side in [corner.x, corner.y] {
            assert!(
                (side / 10.0 - (side / 10.0).round()).abs() < 1e-4,
                "{corner:?}"
            );
        }
    }
}

#[test]
fn the_document_size_is_set_in_its_dialog_and_undone() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let page = |app: &mut Tessera, message| {
        let _ = app.update(Message::Page(message));
    };

    page(&mut app, PageMessage::Show(true));
    page(&mut app, PageMessage::Preset(page::Preset::FullHd));
    let dialog = app.page_dialog.as_ref().unwrap();
    assert_eq!((&*dialog.width, &*dialog.height), ("1920", "1080"));
    page(&mut app, PageMessage::Turn);
    page(&mut app, PageMessage::Apply);
    assert!(app.page_dialog.is_none());
    // Around the triangle, (100, 100)–(300, 300).
    let set = app.layers.page().expect("set");
    assert_eq!(set.size, iced::Size::new(1080.0, 1920.0));
    assert_eq!(set.center, Point::new(200.0, 200.0));
    // All of it in view.
    let canvas = Tessera::canvas_size(WINDOW_SIZE);
    for corner in set.corners() {
        let p = app.camera.to_screen(corner);
        assert!(
            (0.0..=canvas.width).contains(&p.x) && (0.0..=canvas.height).contains(&p.y),
            "{p:?}"
        );
    }

    // A custom size, where it was.
    page(&mut app, PageMessage::Show(true));
    assert!(!app.page_dialog.as_ref().unwrap().centre);
    page(&mut app, PageMessage::Width("1000".into()));
    page(&mut app, PageMessage::Height("700".into()));
    assert_eq!(
        app.page_dialog.as_ref().unwrap().preset,
        page::Preset::Custom
    );
    page(&mut app, PageMessage::Apply);
    let custom = app.layers.page().unwrap();
    assert_eq!(custom.size, iced::Size::new(1000.0, 700.0));
    assert_eq!(custom.center, set.center);

    // A size that won't do isn't applied.
    page(&mut app, PageMessage::Show(true));
    page(&mut app, PageMessage::Width("0".into()));
    page(&mut app, PageMessage::Apply);
    assert!(app.page_dialog.is_some());
    page(&mut app, PageMessage::Remove);
    assert_eq!(app.layers.page(), None);

    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.page(), Some(custom));
    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.page(), Some(set));
}

#[test]
fn layers_hold_their_own_geometry() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let back = app.current();

    let _ = app.update(Message::Layers(LayerAction::Add));
    let front = app.current();
    assert_ne!(front, back);
    assert!(app.document().is_empty());
    // The same triangle again: it would overlap on one layer, not on two.
    edit(&mut app, 0.0);
    assert_eq!(app.document().triangle_ids().len(), 1);
    edit(&mut app, 0.0);
    assert_eq!(app.document().triangle_ids().len(), 1);

    // Drawn around the current layer, back to front.
    let (below, above) = app.layers.around(front, &Default::default());
    assert_eq!((below.len(), above.len()), (1, 0));

    // Back to the first layer; its drawing is untouched.
    let _ = app.update(Message::Layers(LayerAction::Press(back)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    assert_eq!(app.current(), back);
    assert_eq!(app.document().triangle_ids().len(), 1);

    // Undo goes back over the edit on the front layer, then its adding.
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.layers().len(), 1);
    assert_eq!(app.current(), back);
    assert!(app.is_unsaved());
}

#[test]
fn framing_a_shape_on_another_layer_switches_to_it() {
    let mut app = Tessera::default();
    let back = app.current();
    let _ = app.update(Message::Layers(LayerAction::Add));
    let front = app.current();
    let points = vec![iced::Point::ORIGIN, iced::Point::new(100.0, 100.0)];
    let frame = |layer| {
        Message::Editor(editor::Message::Frame {
            points: points.clone(),
            layer,
        })
    };
    let _ = app.update(frame(None));
    assert_eq!(app.current(), front);
    let _ = app.update(frame(Some(back)));
    assert_eq!((app.current(), app.selected), (back, back));
}

#[test]
fn dragging_a_layer_moves_it() {
    let mut app = Tessera::default();
    let back = app.current();
    let _ = app.update(Message::Layers(LayerAction::Add));
    let front = app.current();
    let names = |app: &Tessera| {
        app.layers
            .rows()
            .iter()
            .map(|row| row.name.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&app), ["Layer 2", "Layer 1"]);

    // Onto itself: nothing; then onto the lower half of the other.
    let _ = app.update(Message::Layers(LayerAction::Press(front)));
    let _ = app.update(Message::Layers(LayerAction::Hover(front, 0.9)));
    assert_eq!(app.dragging, Some((front, None)));
    let _ = app.update(Message::Layers(LayerAction::Hover(back, 0.9)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    assert_eq!(names(&app), ["Layer 1", "Layer 2"]);
    assert_eq!(app.dragging, None);

    // Into a group; out again to the very back.
    let _ = app.update(Message::Layers(LayerAction::Press(back)));
    let _ = app.update(Message::Layers(LayerAction::Group));
    let group = app.selected;
    let _ = app.update(Message::Layers(LayerAction::Press(front)));
    let _ = app.update(Message::Layers(LayerAction::Hover(group, 0.5)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
    assert_eq!(depths, [0, 1, 1]);
    let _ = app.update(Message::Layers(LayerAction::Press(front)));
    let _ = app.update(Message::Layers(LayerAction::HoverEnd));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
    assert_eq!(depths, [0, 1, 0]);

    // Letting go outside the list: nothing.
    let _ = app.update(Message::Layers(LayerAction::Press(front)));
    let _ = app.update(Message::Layers(LayerAction::Hover(group, 0.1)));
    let _ = app.update(Message::Layers(LayerAction::Leave));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    assert_eq!(app.layers.rows().last().unwrap().id, front);

    // Each move is an undo step.
    let _ = app.update(Message::Undo);
    let depths: Vec<_> = app.layers.rows().iter().map(|row| row.depth).collect();
    assert_eq!(depths, [0, 1, 1]);
}

#[test]
fn painting_a_whole_layer_or_all_layers() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let back = app.current();
    let _ = app.update(Message::Layers(LayerAction::Add));
    edit(&mut app, 0.0);
    let red = Color::from_rgb8(255, 0, 0);
    let _ = app.update(Message::Paint(PaintMessage::PickBrush(Brush::Color(red))));
    let painted = |app: &Tessera| {
        app.layers
            .layers()
            .iter()
            .map(|layer| layer.document.colors().all(|c| c == Some(red)))
            .collect::<Vec<_>>()
    };

    let _ = app.update(Message::Paint(PaintMessage::PaintEverything(false)));
    assert_eq!(painted(&app), [true, false]);
    let _ = app.update(Message::Undo);
    assert_eq!(painted(&app), [false, false]);

    let _ = app.update(Message::Paint(PaintMessage::PaintEverything(true)));
    assert_eq!(painted(&app), [true, true]);
    assert_eq!(app.recent, vec![red]);
    // Edges too, the current layer only.
    let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
    let _ = app.update(Message::Paint(PaintMessage::PaintEverything(false)));
    let red_edges = |id| {
        let document = &app.layers.layer(id).unwrap().document;
        document
            .edge_styles()
            .filter(|(_, _, style)| style.color == red)
            .count()
    };
    assert_eq!((red_edges(app.current()), red_edges(back)), (3, 0));
}

#[test]
fn a_duplicate_is_worked_on_leaving_the_original_be() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let original = app.current();
    let faces = app.document().triangle_ids().len();

    let _ = app.update(Message::Layers(LayerAction::Duplicate));
    let copy = app.current();
    assert_ne!(copy, original);
    assert_eq!(app.layers.layers().len(), 2);
    // Going nuts with the copy: the original keeps its drawing.
    edit(&mut app, 400.0);
    assert_eq!(app.document().triangle_ids().len(), faces + 1);
    let kept = &app.layers.layer(original).unwrap().document;
    assert_eq!(kept.triangle_ids().len(), faces);

    // One undo step each.
    let _ = app.update(Message::Undo);
    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.layers().len(), 1);
}

#[test]
fn pointing_at_a_layer_lights_it_up_until_off_it() {
    let mut app = Tessera::default();
    let back = app.current();
    let _ = app.update(Message::Layers(LayerAction::Add));
    let front = app.current();
    let before = app.layers.signature();

    let _ = app.update(Message::Layers(LayerAction::Point(back)));
    assert_eq!(app.pointed_layer, Some(back));
    // Onto the next line before off the last: the next one stays.
    let _ = app.update(Message::Layers(LayerAction::Point(front)));
    let _ = app.update(Message::Layers(LayerAction::Unpoint(back)));
    assert_eq!(app.pointed_layer, Some(front));
    let _ = app.update(Message::Layers(LayerAction::Unpoint(front)));
    assert_eq!(app.pointed_layer, None);
    // Not a change (nothing to undo).
    assert_eq!(app.layers.signature(), before);
    assert_eq!(app.undo.len(), 1);
}

#[test]
fn opening_a_file_again_picks_up_where_it_was_left() {
    let mut app = Tessera::new();
    edit(&mut app, 0.0);
    let _ = app.update(Message::Layers(LayerAction::Add));
    let text = file::save(&app.layers, app.camera, &app.backgrounds, app.current());
    let path = PathBuf::from("/drawings/flower.tessera");
    let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
        path.clone(),
        text.clone(),
    ))))));

    // Working on the back layer, zoomed in; then leaving the file.
    let back_index = 1;
    let back = app.layers.layers()[back_index].id;
    let _ = app.update(Message::Layers(LayerAction::Press(back)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    app.camera.zoom = 3.0;
    app.camera.pan = iced::Vector::new(12.0, -4.0);
    let _ = app.update(Message::File(FileMessage::New));
    let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
    assert_eq!(app.camera.zoom, 1.0);

    let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((path, text))))));
    assert_eq!(app.camera.zoom, 3.0);
    assert_eq!(app.camera.pan, iced::Vector::new(12.0, -4.0));
    assert_eq!(app.current(), app.layers.layers()[back_index].id);
}

#[test]
fn files_dropped_on_the_window_open_or_go_behind_the_layer() {
    use update::Dropped;
    assert_eq!(
        Dropped::of(Path::new("/a/flower.tessera")),
        Dropped::Drawing
    );
    assert_eq!(
        Dropped::of(Path::new("/a/FLOWER.TESSERA")),
        Dropped::Drawing
    );
    assert_eq!(Dropped::of(Path::new("/a/photo.JPG")), Dropped::Image);
    assert_eq!(Dropped::of(Path::new("/a/scan.png")), Dropped::Image);
    assert_eq!(Dropped::of(Path::new("/a/notes.txt")), Dropped::Other);

    let mut app = Tessera::default();
    // Dragged over, and off again: the hint comes and goes.
    let path = PathBuf::from("/a/flower.tessera");
    let _ = app.update(Message::File(FileMessage::Hovered(Some(path.clone()))));
    assert_eq!(app.dropping, Some(path.clone()));
    let _ = app.update(Message::File(FileMessage::Hovered(None)));
    assert_eq!(app.dropping, None);

    // A drawing dropped with changes unsaved: asked first.
    edit(&mut app, 0.0);
    let _ = app.update(Message::File(FileMessage::Dropped(path.clone())));
    assert!(matches!(&app.confirming, Some(Replace::OpenPath(p)) if *p == path));
    app.confirming = None;

    // Neither: said so.
    let _ = app.update(Message::File(FileMessage::Dropped("/a/notes.txt".into())));
    assert!(app.notice.as_ref().is_some_and(|notice| notice.error));
}

#[test]
fn deleting_a_group_with_every_layer_in_it() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let _ = app.update(Message::Layers(LayerAction::Add));
    edit(&mut app, 0.0);
    // Both in one group, then that group in another.
    let _ = app.update(Message::Layers(LayerAction::Group));
    let inner = app.selected;
    let back = app.layers.layers()[1].id;
    let _ = app.update(Message::Layers(LayerAction::Press(back)));
    let _ = app.update(Message::Layers(LayerAction::Hover(inner, 0.5)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    let _ = app.update(Message::Layers(LayerAction::Press(inner)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    assert_eq!(app.layers.nodes().len(), 1);

    let _ = app.update(Message::Layers(LayerAction::Remove));
    assert_eq!(app.layers.layers().len(), 1);
    assert!(app.document().is_empty());
    assert_eq!(app.current(), app.layers.first_layer());
    assert_eq!(app.selected, app.current());

    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.layers().len(), 2);
    assert!(app.layers.contains(inner));
}

#[test]
fn the_pipette_picks_up_a_brush() {
    let mut app = Tessera::default();
    let _ = app.update(Message::Paint(PaintMessage::TogglePicking));
    assert_eq!(app.mode, Mode::Paint);
    assert!(app.picking);

    let red = Color::from_rgb8(255, 0, 0);
    let pick = |app: &mut Tessera, picked| {
        let _ = app.update(Message::Editor(editor::Message::Pick(picked)));
    };
    pick(&mut app, editor::Picked::Face(Some(red)));
    assert!(!app.picking);
    assert_eq!(app.brush, Brush::Color(red));

    pick(
        &mut app,
        editor::Picked::Edge(Some(document::EdgeStyle {
            color: red,
            width: 4.5,
        })),
    );
    assert_eq!(app.edge_width, 4.5);
    // Unpainted: the eraser, to paint it back like that.
    pick(&mut app, editor::Picked::Edge(None));
    assert_eq!(app.brush, Brush::Eraser);

    let _ = app.update(Message::Paint(PaintMessage::TogglePicking));
    let _ = app.update(Message::SetMode(Mode::Shape));
    assert!(!app.picking);
}

#[test]
fn crossfading_edges_is_per_layer_and_undoable() {
    let mut app = Tessera::default();
    let back = app.current();
    let _ = app.update(Message::Layers(LayerAction::Add));
    let front = app.current();
    let crossfade = |app: &Tessera, id| app.layers.layer(id).unwrap().crossfade;

    let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
    let _ = app.update(Message::Paint(PaintMessage::Crossfade(true)));
    assert!(crossfade(&app, front).edges);
    assert_eq!(crossfade(&app, back), layers::Crossfade::default());

    let _ = app.update(Message::Undo);
    assert!(!crossfade(&app, front).edges);
}

#[test]
fn the_fade_width_and_hiding_edges_are_undoable() {
    let mut app = Tessera::default();
    let layer = app.current();
    let look = |app: &Tessera| {
        let layer = app.layers.layer(layer).unwrap();
        (layer.crossfade.width, layer.show_edges)
    };
    let _ = app.update(Message::Paint(PaintMessage::SetTarget(Target::Edges)));
    let _ = app.update(Message::Paint(PaintMessage::Crossfade(true)));
    // Dragging the slider: one step.
    for width in [8.0, 12.0, 20.0] {
        let _ = app.update(Message::Paint(PaintMessage::FadeWidth(width)));
    }
    let _ = app.update(Message::Paint(PaintMessage::FadeWidthDone));
    assert_eq!(look(&app), (20.0, true));
    let _ = app.update(Message::Paint(PaintMessage::ShowEdges(false)));
    assert_eq!(look(&app), (20.0, false));

    let _ = app.update(Message::Undo);
    assert_eq!(look(&app), (20.0, true));
    let _ = app.update(Message::Undo);
    assert_eq!(look(&app), (layers::Crossfade::WIDTH, true));
}

#[test]
fn recent_files() {
    let mut app = Tessera::default();
    let text = file::save(&app.layers, app.camera, &app.backgrounds, app.current());
    let path = PathBuf::from("/drawings/flower.tessera");
    let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
        path.clone(),
        text,
    ))))));
    assert_eq!(app.recent_files.paths(), std::slice::from_ref(&path));

    // With unsaved changes, opening a recent file asks first.
    edit(&mut app, 0.0);
    let _ = app.update(Message::File(FileMessage::OpenRecent(path.clone())));
    assert!(matches!(&app.confirming, Some(Replace::OpenPath(p)) if *p == path));

    // Gone: said so, and forgotten.
    let _ = app.update(Message::File(FileMessage::RecentFailed(
        path,
        "No such file".into(),
    )));
    assert!(app.recent_files.paths().is_empty());
    assert!(app.notice.as_ref().unwrap().error);

    // Saving (as) remembers it too.
    let saved = PathBuf::from("/drawings/copy.tessera");
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(Some(saved.clone())),
        app.versions(),
        None,
    )));
    assert_eq!(app.recent_files.paths(), [saved]);
}

#[test]
fn alt_and_moving_tweaks_the_edge_width() {
    let mut app = Tessera {
        edge_width: 2.0,
        ..Tessera::default()
    };
    let start = app.edge_width;
    let _ = app.update(Message::Editor(editor::Message::TweakWidth(100.0)));
    assert!((app.edge_width - start * 1f32.exp()).abs() < 1e-4);
    let _ = app.update(Message::Editor(editor::Message::TweakWidth(-100.0)));
    assert!((app.edge_width - start).abs() < 1e-4);
    // Within the slider's range.
    let _ = app.update(Message::Editor(editor::Message::TweakWidth(10_000.0)));
    assert_eq!(app.edge_width, 12.0);
    let _ = app.update(Message::Editor(editor::Message::TweakWidth(-10_000.0)));
    assert_eq!(app.edge_width, 0.5);
}

#[test]
fn the_about_box_opens_and_closes() {
    let mut app = Tessera::default();
    let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::Help)));
    let _ = app.update(Message::Menu(MenuMessage::About(true)));
    assert!(app.about);
    assert_eq!(app.menu, None);
    let _ = app.update(Message::Key(keyboard::Event::KeyPressed {
        key: Key::Named(keyboard::key::Named::Escape),
        modified_key: Key::Named(keyboard::key::Named::Escape),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    }));
    assert!(!app.about);
}

#[test]
fn proportional_editing_toggles_and_reaches_as_far_as_set() {
    let mut app = Tessera::new();
    assert!(!app.proportional);
    let _ = app.update(Message::Shape(ShapeMessage::ToggleProportional));
    assert!(app.proportional);
    let start = app.reach;
    let _ = app.update(Message::Editor(editor::Message::TweakReach(100.0)));
    assert!((app.reach - start * 1f32.exp()).abs() < 1e-3);
    let _ = app.update(Message::Editor(editor::Message::TweakReach(-10_000.0)));
    assert_eq!(app.reach, REACH.0);
    let _ = app.update(Message::Shape(ShapeMessage::Reach(1e6)));
    assert_eq!(app.reach, REACH.1);
}

#[test]
fn a_mirror_is_placed_moved_applied_and_undone() {
    let mut app = Tessera::new();
    edit(&mut app, 0.0);
    let faces = app.document().triangle_ids().len();
    let _ = app.update(Message::Shape(ShapeMessage::PlaceMirror(true)));
    assert_eq!(app.tool(), Tool::Mirror);
    let (_, max) = app.document().bounds().unwrap();
    let at = |x: f32| document::Mirror {
        a: iced::Point::new(x, 0.0),
        b: iced::Point::new(x, 10.0),
    };
    let _ = app.update(Message::Editor(editor::Message::SetMirror(
        at(max.x + 50.0),
    )));
    assert!(!app.placing_mirror);
    // Placed again: instead.
    let mirror = at(max.x + 20.0);
    let _ = app.update(Message::Editor(editor::Message::SetMirror(mirror)));
    let current = app.current();
    assert_eq!(app.layers.layer(current).unwrap().mirror, Some(mirror));

    let _ = app.update(Message::Shape(ShapeMessage::ApplyMirror));
    assert_eq!(app.document().triangle_ids().len(), 2 * faces);
    assert_eq!(app.layers.layer(current).unwrap().mirror, None);

    // One undo step back to mirrored, but not applied.
    let _ = app.update(Message::Undo);
    assert_eq!(app.document().triangle_ids().len(), faces);
    assert_eq!(app.layers.layer(current).unwrap().mirror, Some(mirror));
}

#[test]
fn keys_are_looked_up_and_set() {
    let mut app = Tessera::new();
    let press = |app: &mut Tessera, key: Key, modifiers| {
        let _ = app.update(Message::Key(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }));
    };
    let none = keyboard::Modifiers::empty();
    let letter = |c: &str| Key::Character(c.into());

    press(&mut app, Key::Named(keyboard::key::Named::F1), none);
    assert!(app.keys_open);
    // While they're open, keys don't do what they do.
    press(&mut app, Key::Named(keyboard::key::Named::Tab), none);
    assert_eq!(app.mode, Mode::Shape);

    // Switching modes on Q instead of Tab.
    let _ = app.update(Message::Keys(KeysMessage::Rebind(Some((
        Action::ToggleMode,
        Some(0),
    )))));
    press(&mut app, letter("q"), none);
    assert_eq!(app.rebinding, None);
    assert_eq!(app.keys.first(Action::ToggleMode), "Q");
    // Esc while setting one: never mind; then closes.
    let _ = app.update(Message::Keys(KeysMessage::Rebind(Some((
        Action::ToggleMode,
        None,
    )))));
    press(&mut app, Key::Named(keyboard::key::Named::Escape), none);
    assert!(app.keys_open);
    assert_eq!(app.keys.keys(Action::ToggleMode).len(), 1);
    press(&mut app, Key::Named(keyboard::key::Named::Escape), none);
    assert!(!app.keys_open);

    // Now Q switches, Tab doesn't.
    press(&mut app, Key::Named(keyboard::key::Named::Tab), none);
    assert_eq!(app.mode, Mode::Shape);
    press(&mut app, letter("q"), none);
    assert_eq!(app.mode, Mode::Paint);

    let _ = app.update(Message::Keys(KeysMessage::ResetKeys));
    assert_eq!(app.keys, Keymap::default());
}

#[test]
fn c_and_moving_tweaks_the_lightness() {
    let mut app = Tessera::default();
    let _ = app.update(Message::Paint(PaintMessage::WheelPicked(Hsv {
        hue: 0.6,
        saturation: 0.8,
        value: 0.52,
    })));
    let _ = app.update(Message::Editor(editor::Message::TweakLightness(50.0)));
    assert!((app.hsv.value - 0.72).abs() < 1e-5);
    // Through black and back, and through white and back: the same
    // colour again.
    let before = app.hex.clone();
    let _ = app.update(Message::Editor(editor::Message::TweakLightness(-1000.0)));
    assert_eq!(app.hex, "#000000");
    let _ = app.update(Message::Editor(editor::Message::TweakLightness(180.0)));
    assert_eq!(app.hex, before);
    let _ = app.update(Message::Editor(editor::Message::TweakLightness(1000.0)));
    assert_eq!(app.hex, "#ffffff");
    let _ = app.update(Message::Editor(editor::Message::TweakLightness(-320.0)));
    assert_eq!(app.hex, before);
    assert_eq!(app.brush, Brush::Color(app.hsv.to_color()));
}

#[test]
fn renaming_is_one_undo_step() {
    let mut app = Tessera::default();
    let layer = app.current();
    let _ = app.update(Message::Layers(LayerAction::Press(layer)));
    let _ = app.update(Message::Layers(LayerAction::Drop));
    for name in ["S", "Sk", "Sketch"] {
        let _ = app.update(Message::Layers(LayerAction::Rename(name.into())));
    }
    assert_eq!(app.layers.rows()[0].name, "Sketch");
    let _ = app.update(Message::Undo);
    assert_eq!(app.layers.rows()[0].name, "Layer 1");
}

#[test]
fn another_menu_opens_straight_away() {
    let mut app = Tessera::default();

    // Hovering the menu bar opens nothing by itself.
    let _ = app.update(Message::Menu(MenuMessage::HoverMenu(Menu::View)));
    assert_eq!(app.menu, None);

    let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
    assert_eq!(app.menu, Some(Menu::File));
    let _ = app.update(Message::Menu(MenuMessage::HoverMenu(Menu::View)));
    assert_eq!(app.menu, Some(Menu::View));
    let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
    assert_eq!(app.menu, Some(Menu::File));
    let _ = app.update(Message::Menu(MenuMessage::ToggleMenu(Menu::File)));
    assert_eq!(app.menu, None);
}

#[test]
fn notices_fade_out() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let path = PathBuf::from("/tmp/drawing.tessera");
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(Some(path)),
        app.versions(),
        None,
    )));

    let since = app.notice.as_ref().unwrap().since;
    assert_eq!(app.notice.as_ref().unwrap().text, "Saved drawing.tessera");
    let at = |ms| Message::Frame(since + Duration::from_millis(ms));

    let _ = app.update(at(2_000));
    assert_eq!(app.notice.as_ref().unwrap().opacity(), 1.0);
    let _ = app.update(at(3_300));
    let opacity = app.notice.as_ref().unwrap().opacity();
    assert!(opacity > 0.0 && opacity < 1.0, "{opacity}");
    let _ = app.update(at(3_700));
    assert!(app.notice.is_none());

    // Errors stay longer.
    let _ = app.update(Message::Export(ExportMessage::Exported(Err(
        "disk full".into()
    ))));
    let since = app.notice.as_ref().unwrap().since;
    let _ = app.update(Message::Frame(since + Duration::from_secs(5)));
    assert_eq!(app.notice.as_ref().unwrap().opacity(), 1.0);
}

#[test]
fn undo_and_redo_step_through_edits() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let one = app.document().to_parts();
    edit(&mut app, 500.0);
    let two = app.document().to_parts();

    let _ = app.update(Message::Undo);
    assert_eq!(app.document().to_parts(), one);
    let _ = app.update(Message::Undo);
    assert!(app.document().is_empty());
    let _ = app.update(Message::Undo); // nothing left: no change
    assert!(app.document().is_empty());
    assert!(!app.is_unsaved(), "back where it started");

    let _ = app.update(Message::Redo);
    let _ = app.update(Message::Redo);
    assert_eq!(app.document().to_parts(), two);
    assert!(app.is_unsaved());

    // A new edit after undoing drops what could be redone.
    let _ = app.update(Message::Undo);
    edit(&mut app, 1000.0);
    assert!(app.redo.is_empty());
    let _ = app.update(Message::Redo);
    assert_eq!(app.document().triangle_ids().len(), 2);
}

#[test]
fn undoing_to_what_was_saved_is_saved() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let path = PathBuf::from("/tmp/drawing.tessera");
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(Some(path)),
        app.versions(),
        None,
    )));

    edit(&mut app, 500.0);
    assert!(app.is_unsaved());
    let _ = app.update(Message::Undo);
    assert!(!app.is_unsaved());
}

#[test]
fn edits_for_another_revision_are_ignored() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let stale = app.document().revision();
    let _ = app.update(Message::Undo);

    // Worked out before the undo (say, mid-drag): no longer applies.
    let corners = [
        Point::new(0.0, 0.0),
        Point::new(10.0, 0.0),
        Point::new(5.0, 10.0),
    ];
    let _ = app.update(Message::Editor(editor::Message::Edit {
        edits: vec![Edit::AddTriangle { corners, snap: 0.0 }],
        revision: stale,
    }));
    assert!(app.document().is_empty());
}

#[test]
fn new_starts_a_fresh_history() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
    assert!(app.undo.is_empty() && app.redo.is_empty());
}

#[test]
fn painting_remembers_the_colour_and_undoes() {
    let mut app = Tessera::default();
    let _ = app.update(Message::Editor(editor::Message::Edit {
        edits: vec![Edit::AddTriangle {
            corners: [
                Point::new(0.0, 0.0),
                Point::new(10.0, 0.0),
                Point::new(5.0, 10.0),
            ],
            snap: 0.0,
        }],
        revision: app.document().revision(),
    }));
    let _ = app.update(Message::SetMode(Mode::Paint));
    let _ = app.update(Message::Paint(PaintMessage::HexTyped("#ff0000".into())));
    let red = Color::from_rgb8(255, 0, 0);
    assert_eq!(app.brush, Brush::Color(red));

    let _ = app.update(Message::Editor(editor::Message::Edit {
        edits: vec![Edit::Paint {
            triangle: 0,
            color: Some(red),
        }],
        revision: app.document().revision(),
    }));
    assert_eq!(app.document().color(0), Some(red));
    assert_eq!(app.recent, vec![red]);

    // Back to the colour it had: the default, as made.
    let _ = app.update(Message::Undo);
    assert_eq!(app.document().color(0), Some(document::DEFAULT_FACE));
    assert_eq!(app.document().triangle_ids().len(), 1);
}

fn with_background() -> Tessera {
    let mut app = Tessera::default();
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundPicked(
        Ok(Some(background::tests::pixel())),
    )));
    app
}

#[test]
fn each_layer_has_its_own_background_image() {
    let mut app = with_background();
    let first = app.current();
    assert!(app.background().is_some());

    // A new layer: none of its own.
    let _ = app.update(Message::Layers(LayerAction::Add));
    assert_ne!(app.current(), first);
    assert!(app.background().is_none());
    let _ = app.update(Message::Layers(LayerAction::Press(first)));
    assert!(app.background().is_some());

    // A duplicate: its own, of the same image (not a copy of it).
    let _ = app.update(Message::Layers(LayerAction::Duplicate));
    let copy = app.current();
    assert_ne!(copy, first);
    let (a, b) = (&app.backgrounds[&first], &app.backgrounds[&copy]);
    assert!(a.same_image(b));
    // Moving the copy's leaves the original's be.
    let _ = app.update(Message::Editor(editor::Message::MoveBackground(
        iced::Vector::new(5.0, 0.0),
    )));
    assert_ne!(
        app.backgrounds[&first].center,
        app.backgrounds[&copy].center
    );
}

#[test]
fn a_background_image_fills_the_view() {
    let mut app = with_background();
    assert!(app.background().is_some());
    assert!(app.is_unsaved());

    // The window, less the menu bar: 800×600 for a 4×2 image.
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundFitted(
        iced::Size::new(800.0 + LAYERS_WIDTH, 600.0 + MENU_BAR_HEIGHT),
    )));
    let background = app.background().unwrap();
    assert!((background.scale - 200.0).abs() < 1e-3);
    assert_eq!(background.center, Point::new(400.0, 300.0));
}

#[test]
fn the_background_panel_adjusts_the_image() {
    let mut app = with_background();
    let _ = app.update(Message::Background(BackgroundMessage::ToggleBackgroundMode));
    assert!(app.editing_background);
    assert_eq!(app.fields.scale, "100.00");

    let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
        Field::X,
        "12.5".into(),
    )));
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
        Field::Scale,
        "50".into(),
    )));
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
        Field::Rotation,
        "90".into(),
    )));
    // Half typed: kept as typed, the image left alone.
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
        Field::Y,
        "-".into(),
    )));
    let background = app.background().unwrap();
    assert_eq!(background.center, Point::new(12.5, 0.0));
    assert_eq!(background.scale, 0.5);
    assert!((background.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert_eq!(app.fields.y, "-");

    // Dragging on the canvas updates the fields.
    let _ = app.update(Message::Editor(editor::Message::MoveBackground(
        iced::Vector::new(1.0, 2.0),
    )));
    assert_eq!(app.fields.x, "13.5");
    assert_eq!(app.fields.y, "2.0");

    let _ = app.update(Message::Background(
        BackgroundMessage::ToggleBackgroundVisible,
    ));
    assert!(!app.background().unwrap().visible);
    let _ = app.update(Message::Background(BackgroundMessage::RemoveBackground));
    assert!(app.background().is_none());
}

#[test]
fn changing_the_image_keeps_its_place() {
    let mut app = with_background();
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundField(
        Field::X,
        "40".into(),
    )));
    let _ = app.update(Message::Background(
        BackgroundMessage::ToggleBackgroundVisible,
    ));
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundPicked(
        Ok(Some(background::tests::pixel())),
    )));
    assert_eq!(app.background().unwrap().center.x, 40.0);
    // A new image is always shown.
    assert!(app.background().unwrap().visible);
}

#[test]
fn the_background_counts_as_content() {
    let mut app = with_background();
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(Some(PathBuf::from("/tmp/traced.tessera"))),
        app.versions(),
        None,
    )));
    assert!(!app.is_unsaved());
    let _ = app.update(Message::Background(BackgroundMessage::BackgroundOpacity(
        0.9,
    )));
    assert!(app.is_unsaved());

    // New asks, and clears it.
    let _ = app.update(Message::File(FileMessage::New));
    assert!(matches!(app.confirming, Some(Replace::New)));
    let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
    assert!(app.background().is_none());
    assert!(!app.is_unsaved());
}

#[test]
fn new_on_a_blank_canvas_does_nothing() {
    let mut app = Tessera::default();
    app.camera.zoom = 2.0;
    let _ = app.update(Message::File(FileMessage::New));
    assert!(app.confirming.is_none());
    assert_eq!(app.camera.zoom, 2.0);
}

#[test]
fn new_asks_before_losing_changes() {
    let mut app = Tessera::default();
    assert!(!app.is_unsaved());
    edit(&mut app, 0.0);
    assert!(app.is_unsaved());
    assert_eq!(app.title(), "Untitled • — Tessera");

    let _ = app.update(Message::File(FileMessage::New));
    assert!(matches!(app.confirming, Some(Replace::New)));
    let _ = app.update(Message::File(FileMessage::Confirm(Choice::Cancel)));
    assert!(app.confirming.is_none());
    assert!(!app.document().is_empty());

    // "Don't save" goes on with it: a blank document and view.
    app.camera.zoom = 2.0;
    let _ = app.update(Message::File(FileMessage::Replace(Replace::New)));
    assert!(app.document().is_empty());
    assert_eq!(app.camera.zoom, 1.0);
    assert!(!app.is_unsaved());
}

#[test]
fn saved_changes_need_no_asking() {
    let mut app = Tessera::default();
    edit(&mut app, 0.0);
    let path = PathBuf::from("/tmp/drawing.tessera");
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(Some(path)),
        app.versions(),
        None,
    )));

    assert!(!app.is_unsaved());
    assert_eq!(app.title(), "drawing.tessera — Tessera");
    let _ = app.update(Message::File(FileMessage::New));
    assert!(app.confirming.is_none());

    // A cancelled save leaves new changes unsaved.
    edit(&mut app, 500.0);
    let _ = app.update(Message::File(FileMessage::Saved(
        Ok(None),
        app.versions(),
        None,
    )));
    assert!(app.is_unsaved());
}

#[test]
fn opening_restores_the_document_and_view() {
    let mut source = Tessera::default();
    edit(&mut source, 0.0);
    source.camera.zoom = 3.0;
    let text = file::save(
        &source.layers,
        source.camera,
        &source.backgrounds,
        source.current(),
    );

    let mut app = Tessera::default();
    let path = PathBuf::from("/tmp/drawing.tessera");
    let _ = app.update(Message::File(FileMessage::Opened(Ok(Some((
        path.clone(),
        text,
    ))))));

    assert_eq!(app.document().to_parts(), source.document().to_parts());
    assert_eq!(app.camera.zoom, 3.0);
    assert_eq!(app.path, Some(path));
    assert!(!app.is_unsaved());
}
