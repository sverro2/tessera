//! The window's parts around the canvas: the menu bar and its menus, the
//! mode switch, and the panels for shaping, painting, the mirror, the
//! background and the layers.

use super::*;
// Named, as the standard library has a `column!` of its own.
use iced::widget::{column, row, stack};

impl Tessera {
    /// The label of the button showing the keyboard shortcuts: with its
    /// key, if it has one.
    pub(super) fn hint(&self) -> String {
        match self.keys.keys(Action::ShowKeys).first() {
            Some(key) => format!("Keyboard shortcuts ({key})"),
            None => "Keyboard shortcuts".to_string(),
        }
    }

    /// Shape and paint, side by side as one control.
    pub(super) fn mode_switch(&self) -> Element<'_, Message> {
        joined(vec![
            (
                "Shape",
                self.mode == Mode::Shape,
                Message::SetMode(Mode::Shape),
            ),
            (
                "Paint",
                self.mode == Mode::Paint,
                Message::SetMode(Mode::Paint),
            ),
        ])
    }

    /// Over the canvas, at the top: placing a mirror, a way out; the current
    /// layer mirrored, ways to apply the mirror, move it or take it away.
    pub(super) fn mirror_bar(&self) -> Element<'_, Message> {
        let small = |label| text(label).size(13);
        let mirrored = self
            .layers
            .layer(self.current())
            .is_some_and(|layer| layer.mirror.is_some());
        let bar = if self.editing_background {
            return space().into();
        } else if self.placing_mirror {
            row![
                small(if mirrored {
                    "Placing the mirror again"
                } else {
                    "Placing a mirror"
                }),
                button(small("Cancel"))
                    .padding([3, 10])
                    .style(button::secondary)
                    .on_press(Message::Shape(ShapeMessage::PlaceMirror(false))),
            ]
        } else if mirrored {
            row![
                small("Mirrored"),
                tooltip(
                    button(small("Apply"))
                        .padding([3, 10])
                        .on_press(Message::Shape(ShapeMessage::ApplyMirror)),
                    tip("Make the mirror image actual geometry"),
                    tooltip::Position::Bottom,
                ),
                tooltip(
                    button(small("Move…"))
                        .padding([3, 10])
                        .style(button::secondary)
                        .on_press(Message::Shape(ShapeMessage::PlaceMirror(true))),
                    tip("Place the mirror elsewhere (Ctrl+M)"),
                    tooltip::Position::Bottom,
                ),
                button(small("Remove"))
                    .padding([3, 10])
                    .style(button::secondary)
                    .on_press(Message::Shape(ShapeMessage::RemoveMirror)),
            ]
        } else {
            return space().into();
        };
        container(opaque(
            container(bar.spacing(8).align_y(Center))
                .padding([6, 10])
                .style(container::bordered_box),
        ))
        .center_x(Fill)
        .padding([36, 12])
        .into()
    }

    /// In the shape mode: proportional editing, on or off, and how far it
    /// reaches.
    pub(super) fn shape_panel(&self) -> Element<'_, Message> {
        if self.mode != Mode::Shape || self.editing_background {
            return space().into();
        }
        let mut body =
            column![tooltip(
            checkbox(self.proportional)
                .label("Proportional (O)")
                .size(14)
                .text_size(13)
                .on_toggle(|_| Message::Shape(ShapeMessage::ToggleProportional)),
            tip("Moving vertices takes those around them along, the less the further; how far is on screen, so zoomed out it reaches further"),
            tooltip::Position::Bottom,
        )]
            .spacing(8);
        if self.proportional {
            // On a log scale: a small reach is as easy to set as a large one.
            body = body.push(
                row![
                    text("Reach").size(13).width(44),
                    slider(REACH.0.ln()..=REACH.1.ln(), self.reach.ln(), |reach| {
                        Message::Shape(ShapeMessage::Reach(reach.exp()))
                    })
                    .step(0.01),
                    text(format!("{:.0} px", self.reach)).size(13).width(48),
                ]
                .spacing(8)
                .align_y(Center),
            );
            body = body.push(text("Alt+move: reach").size(11).style(text::secondary));
        }
        container(opaque(
            container(body)
                .padding(10)
                .width(220)
                .style(container::bordered_box),
        ))
        .padding(12)
        .into()
    }

    /// In the paint mode: what to paint (faces or edges) and the brush to
    /// paint with: a colour (from the wheel, the palette, recently used ones,
    /// or typed) or the eraser; for edges also how wide.
    pub(super) fn paint_panel(&self) -> Element<'_, Message> {
        if self.mode != Mode::Paint {
            return space().into();
        }
        /// The panel's width inside its padding.
        const INNER: f32 = 180.0;
        const SWATCH: f32 = 24.0;
        let gap = (INNER - 6.0 * SWATCH) / 5.0;
        let small = |label| text(label).size(13);
        let swatch = |color: Color| {
            let picked = self.brush == Brush::Color(color);
            button(space().width(SWATCH).height(SWATCH))
                .padding(0)
                .style(move |theme: &Theme, status| button::Style {
                    background: Some(color.into()),
                    border: iced::Border {
                        color: if picked {
                            theme.palette().primary.base.color
                        } else if status == button::Status::Hovered {
                            theme.palette().background.base.text
                        } else {
                            Color::TRANSPARENT
                        },
                        width: 2.0,
                        radius: 3.0.into(),
                    },
                    ..button::Style::default()
                })
                .on_press(Message::Paint(PaintMessage::PickBrush(Brush::Color(color))))
        };
        let swatches =
            |colors: &[Color]| row(colors.iter().map(|&color| swatch(color).into())).spacing(gap);

        let current: Element<'_, Message> = match self.brush {
            Brush::Color(color) => container(space().width(24).height(24))
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    border: iced::Border {
                        radius: 3.0.into(),
                        ..iced::Border::default()
                    },
                    ..container::Style::default()
                })
                .into(),
            Brush::Eraser => container(icon(icons::ERASER, button::secondary))
                .center_x(24)
                .into(),
        };
        let eraser_style = if self.brush == Brush::Eraser {
            button::primary
        } else {
            button::secondary
        };
        let eraser = button(icon(icons::ERASER, eraser_style))
            .padding([4, 6])
            .style(eraser_style)
            .on_press(Message::Paint(PaintMessage::PickBrush(Brush::Eraser)));
        let pipette_style = if self.picking {
            button::primary
        } else {
            button::secondary
        };
        let pipette = tooltip(
            button(icon(icons::PIPETTE, pipette_style))
                .padding([4, 6])
                .style(pipette_style)
                .on_press(Message::Paint(PaintMessage::TogglePicking)),
            tip("Pick up a colour (I, or Ctrl+click)"),
            tooltip::Position::Bottom,
        );

        let hsv = self.hsv;
        let mut body = column![
            joined(vec![
                (
                    "Faces",
                    self.target == Target::Faces,
                    Message::Paint(PaintMessage::SetTarget(Target::Faces))
                ),
                (
                    "Edges",
                    self.target == Target::Edges,
                    Message::Paint(PaintMessage::SetTarget(Target::Edges))
                ),
            ]),
            {
                let layer = self.layers.layer(self.current()).expect("current layer");
                // Showing the edges first: it matters more than how they fade.
                let mut options = column![tooltip(
                    checkbox(layer.show_edges)
                        .label("Show edges")
                        .size(14)
                        .text_size(13)
                        .on_toggle(|value| Message::Paint(PaintMessage::ShowEdges(value))),
                    tip("Hide this layer's edges when painting and exporting; their paint is kept"),
                    tooltip::Position::Bottom,
                )]
                .spacing(6);
                // Painting edges: whether they fade into each other at their
                // ends, and how far.
                let crossfade = layer.crossfade;
                if self.target == Target::Edges {
                    options = options.push(tooltip(
                        checkbox(crossfade.edges)
                            .label("Crossfade")
                            .size(14)
                            .text_size(13)
                            .on_toggle(|value| Message::Paint(PaintMessage::Crossfade(value))),
                        tip("Fade the ends of this layer's painted edges into the edges they meet"),
                        tooltip::Position::Bottom,
                    ));
                    if crossfade.edges {
                        options = options.push(
                            row![
                                small("Fade").width(44),
                                slider(1.0..=40.0, crossfade.width, |value| Message::Paint(
                                    PaintMessage::FadeWidth(value)
                                ))
                                .step(0.5)
                                .on_release(Message::Paint(PaintMessage::FadeWidthDone)),
                                text(format!("{:.1}", crossfade.width)).size(13).width(28),
                            ]
                            .spacing(8)
                            .align_y(Center),
                        );
                    }
                }
                options
            },
            row![
                current,
                text_input("#rrggbb", &self.hex)
                    .size(13)
                    .padding([3, 6])
                    .on_input(|value| Message::Paint(PaintMessage::HexTyped(value))),
                eraser,
                pipette,
            ]
            .spacing(8)
            .align_y(Center),
            container(
                wheel::view(hsv, INNER - 20.0)
                    .map(|value| Message::Paint(PaintMessage::WheelPicked(value)))
            )
            .center_x(Fill),
            row![
                small("Light").width(44),
                // Black, the colour itself (in the middle), white.
                slider(0.0..=2.0, hsv.value, move |value| {
                    Message::Paint(PaintMessage::WheelPicked(Hsv { value, ..hsv }))
                })
                .step(0.01),
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(8);
        if self.target == Target::Edges {
            body = body.push(
                row![
                    small("Width").width(44),
                    slider(0.5..=12.0, self.edge_width, |value| Message::Paint(
                        PaintMessage::EdgeWidth(value)
                    ))
                    .step(0.5),
                    text(format!("{:.1}", self.edge_width)).size(13).width(28),
                ]
                .spacing(8)
                .align_y(Center),
            );
        }
        // Everything at once; holding Shift, on every layer.
        let all_layers = self.modifiers.shift();
        let everything = match (self.target, all_layers) {
            (Target::Faces, false) => "Paint all faces of the layer",
            (Target::Edges, false) => "Paint all edges of the layer",
            (Target::Faces, true) => "Paint all faces, all layers",
            (Target::Edges, true) => "Paint all edges, all layers",
        };
        body = body.push(tooltip(
            button(text(everything).size(12).width(Fill).center())
                .width(Fill)
                .padding([4, 8])
                .style(button::secondary)
                .on_press(Message::Paint(PaintMessage::PaintEverything(all_layers))),
            tip("Hold Shift to paint every layer"),
            tooltip::Position::Bottom,
        ));
        body = body
            .push(swatches(&paint::PALETTE[..6]))
            .push(swatches(&paint::PALETTE[6..]));
        if !self.recent.is_empty() {
            body = body.push(text("Recent").size(11).style(text::secondary));
            for chunk in self.recent.chunks(6) {
                body = body.push(swatches(chunk));
            }
        }

        container(opaque(
            container(body)
                .padding(10)
                .width(INNER + 20.0)
                .style(container::bordered_box),
        ))
        .padding(12)
        .into()
    }

    /// The layers, front first, beside the canvas: select one to edit it,
    /// show or hide them, drag them to order them and to put them in and out
    /// of groups, double-click one to rename it.
    pub(super) fn layers_panel(&self) -> Element<'_, Message> {
        use LayerAction::*;
        let small = |label| text(label).size(12);
        let action = |label, action: LayerAction| {
            button(small(label))
                .padding([2, 6])
                .style(button::secondary)
                .on_press(Message::Layers(action))
        };
        let current = self.current();
        let rows = self.layers.rows();
        let group_selected = rows
            .iter()
            .any(|row| row.id == self.selected && row.group.is_some());
        let (dragged, place) = match self.dragging {
            Some((id, place)) => (Some(id), place),
            None => (None, None),
        };

        let lines = rows.into_iter().map(|row| {
            let id = row.id;
            let selected = id == self.selected;
            let expander: Element<'_, Message> = match row.group {
                Some(expanded) => button(small(if expanded { "▾" } else { "▸" }))
                    .padding([0, 4])
                    .style(button::text)
                    .on_press(Message::Layers(ToggleExpanded(id)))
                    .into(),
                None => space().width(17).into(),
            };
            let eye = button(icon(
                if row.visible {
                    icons::EYE
                } else {
                    icons::EYE_OFF
                },
                button::text,
            ))
            .padding([2, 4])
            .style(button::text)
            .on_press(Message::Layers(ToggleVisible(id)));
            let name: Element<'_, Message> = if self.naming == Some(id) {
                text_input("Name", row.name)
                    .id(NAME_FIELD)
                    .size(12)
                    .padding([2, 4])
                    .on_input(|name| Message::Layers(Rename(name)))
                    .on_submit(Message::Layers(EndRename))
                    .into()
            } else {
                // Hidden (itself or by its group), or being dragged: dimmed.
                let dim = !row.shown || dragged == Some(id);
                text(row.name)
                    .size(12)
                    .style(move |theme: &Theme| {
                        if dim {
                            text::secondary(theme)
                        } else {
                            text::default(theme)
                        }
                    })
                    .into()
            };
            let editing = id == current;
            let into = place == Some(Place::Into(id));
            // Nothing drawn on it (or in it, a group): said so.
            let empty = self.layers.layers_of(id).iter().all(|&layer| {
                self.layers
                    .layer(layer)
                    .is_none_or(|layer| layer.document.is_empty())
            });
            let mut line = row![space().width(row.depth as f32 * 14.0), expander, eye, name]
                .spacing(2)
                .align_y(Center);
            if empty && self.naming != Some(id) {
                line = line
                    .push(space::horizontal())
                    .push(text("empty").size(11).style(text::secondary));
            }
            let content = container(line)
                .width(Fill)
                .height(LAYER_ROW - 4.0)
                .padding([0, 4])
                .align_y(Center)
                .style(move |theme: &Theme| {
                    let palette = theme.palette();
                    // Dropping in, selected, or else the layer being edited.
                    let (background, text) = if into || selected {
                        (palette.primary.weak.color, palette.primary.weak.text)
                    } else if editing {
                        (palette.background.weak.color, palette.background.weak.text)
                    } else {
                        return container::Style::default();
                    };
                    container::Style {
                        background: Some(background.into()),
                        text_color: Some(text),
                        border: iced::Border {
                            radius: 3.0.into(),
                            color: palette.primary.base.color,
                            width: if into { 1.5 } else { 0.0 },
                        },
                        ..container::Style::default()
                    }
                });
            // Where dropping would put it: a line above or below.
            let marker = |shown: bool| {
                container(space().height(2))
                    .width(Fill)
                    .style(move |theme: &Theme| container::Style {
                        background: shown.then(|| theme.palette().primary.base.color.into()),
                        ..container::Style::default()
                    })
            };
            let line = column![
                marker(place == Some(Place::Before(id))),
                content,
                marker(place == Some(Place::After(id))),
            ];
            let mut area = mouse_area(line)
                .on_press(Message::Layers(Press(id)))
                .on_double_click(Message::Layers(StartRename(id)))
                .on_enter(Message::Layers(Point(id)))
                .on_exit(Message::Layers(Unpoint(id)));
            if dragged.is_some() {
                area = area.on_move(move |p| Message::Layers(Hover(id, p.y / LAYER_ROW)));
            }
            area.into()
        });

        // Below the lines: dropping there puts it at the very back.
        let mut end = mouse_area(
            container(space().height(LAYER_ROW * 2.0))
                .width(Fill)
                .style(move |theme: &Theme| container::Style {
                    background: (place == Some(Place::Last))
                        .then(|| theme.palette().primary.weak.color.into()),
                    ..container::Style::default()
                }),
        );
        if dragged.is_some() {
            end = end.on_move(|_| Message::Layers(HoverEnd));
        }

        let list = mouse_area(scrollable(column(lines).push(end)).height(Fill).width(Fill))
            .on_exit(Message::Layers(Leave));

        let mut buttons = row![
            action("+ Layer", Add),
            action("Duplicate", Duplicate),
            action("+ Group", Group),
            action("Delete", Remove),
        ]
        .spacing(4);
        if group_selected {
            buttons = buttons.push(action("Ungroup", Ungroup));
        }

        container(
            column![
                text("Layers").size(13),
                // Onto a second line if they don't fit.
                buttons.wrap().vertical_spacing(4),
                list,
                text("Drag to order, onto a group to put it in · Double-click: rename")
                    .size(11)
                    .style(text::secondary),
            ]
            .spacing(6),
        )
        .padding(8)
        .width(LAYERS_WIDTH)
        .height(Fill)
        .style(container::dark)
        .into()
    }

    /// The compass and, below it, the background button and (in the
    /// background mode) the panel to set up the image with.
    pub(super) fn background_panel(&self) -> Element<'_, Message> {
        let small = |label| text(label).size(13);
        // The background button, joined on its left by the eye to show or
        // hide the image without opening the panel: one control, split in
        // two, styled alike.
        let base = if self.editing_background {
            button::primary
        } else {
            button::secondary
        };
        let visible = self.background().map(|background| background.visible);
        let toggle = button(small("Background"))
            .padding([4, 10])
            .style(move |theme: &Theme, status| {
                let mut style = base(theme, status);
                if visible.is_some() {
                    let radius = style.border.radius;
                    style.border.radius = radius.top_left(0).bottom_left(0);
                }
                style
            })
            .on_press(Message::Background(BackgroundMessage::ToggleBackgroundMode));

        let mut buttons = row![].spacing(1);
        if let Some(visible) = visible {
            let icon = if visible { icons::EYE } else { icons::EYE_OFF };
            let eye = iced::widget::svg(iced::widget::svg::Handle::from_memory(icons::svg(icon)))
                .width(16)
                .height(Fill)
                // The same colour as the button's label.
                .style(move |theme: &Theme, _| iced::widget::svg::Style {
                    color: Some(base(theme, button::Status::Active).text_color),
                });
            buttons = buttons.push(
                button(eye)
                    .padding([4, 8])
                    .height(Fill)
                    .style(move |theme: &Theme, status| {
                        let mut style = base(theme, status);
                        let radius = style.border.radius;
                        style.border.radius = radius.top_right(0).bottom_right(0);
                        style
                    })
                    .on_press(Message::Background(
                        BackgroundMessage::ToggleBackgroundVisible,
                    )),
            );
        }
        let buttons = buttons.push(toggle).height(iced::Shrink);

        let mut panel = column![
            compass::view(self.camera.rotation).map(Message::Compass),
            buttons,
        ]
        .spacing(6)
        .align_x(iced::Right);

        if self.editing_background {
            let body: Element<'_, Message> = match self.background() {
                None => column![
                    small("This layer has no background image yet."),
                    row![
                        button(small("Choose image…"))
                            .on_press(Message::Background(BackgroundMessage::PickBackground)),
                        space::horizontal(),
                        button(small("Done"))
                            .style(button::secondary)
                            .on_press(Message::Background(BackgroundMessage::ToggleBackgroundMode)),
                    ],
                ]
                .spacing(10)
                .into(),
                Some(background) => {
                    let field = |label, field| {
                        let value = match field {
                            Field::X => &self.fields.x,
                            Field::Y => &self.fields.y,
                            Field::Scale => &self.fields.scale,
                            Field::Rotation => &self.fields.rotation,
                        };
                        row![
                            small(label).width(80),
                            text_input("", value)
                                .size(13)
                                .padding([3, 6])
                                .on_input(move |typed| Message::Background(
                                    BackgroundMessage::BackgroundField(field, typed)
                                )),
                        ]
                        .spacing(8)
                        .align_y(Center)
                    };
                    column![
                        row![
                            button(small("Change…"))
                                .on_press(Message::Background(BackgroundMessage::PickBackground)),
                            space::horizontal(),
                            button(small("Remove"))
                                .style(button::danger)
                                .on_press(Message::Background(BackgroundMessage::RemoveBackground)),
                        ]
                        .spacing(6),
                        field("X", Field::X),
                        field("Y", Field::Y),
                        field("Scale %", Field::Scale),
                        field("Rotation °", Field::Rotation),
                        row![
                            small("Opacity").width(80),
                            slider(0.0..=1.0, background.opacity, |value| Message::Background(
                                BackgroundMessage::BackgroundOpacity(value)
                            ))
                            .step(0.01),
                        ]
                        .spacing(8)
                        .align_y(Center),
                        text("Drag: move · Wheel: scale · Shift+wheel: rotate")
                            .size(11)
                            .style(text::secondary),
                        row![
                            button(small("Fit to view"))
                                .style(button::secondary)
                                .on_press(Message::Background(BackgroundMessage::FitBackground)),
                            space::horizontal(),
                            button(small("Done")).on_press(Message::Background(
                                BackgroundMessage::ToggleBackgroundMode
                            )),
                        ],
                    ]
                    .spacing(8)
                    .into()
                }
            };
            panel = panel.push(
                container(body)
                    .padding(10)
                    .width(280)
                    .style(container::bordered_box),
            );
        }

        // In the top right corner.
        container(opaque(panel))
            .align_right(Fill)
            .padding(12)
            .into()
    }

    /// The current notice, if any, fading as it goes.
    pub(super) fn notice(&self) -> Element<'_, Message> {
        let Some(notice) = &self.notice else {
            return space().into();
        };
        let opacity = notice.opacity();
        let error = notice.error;

        container(text(&notice.text).size(13))
            .padding([6, 10])
            .style(move |theme: &Theme| {
                let palette = theme.palette();
                let (background, color) = if error {
                    (palette.danger.base.color, palette.danger.base.text)
                } else {
                    (
                        palette.background.strong.color,
                        palette.background.strong.text,
                    )
                };
                let fade = |color: Color| Color {
                    a: color.a * opacity,
                    ..color
                };
                container::Style {
                    background: Some(fade(background).into()),
                    text_color: Some(fade(color)),
                    border: iced::border::rounded(6),
                    ..container::Style::default()
                }
            })
            .into()
    }

    /// The open menu's items, below its button; clicking anywhere else
    /// (but another menu's button) closes it.
    pub(super) fn dropdown(&self, menu: Menu) -> Element<'_, Message> {
        let item = |label, shortcut: String, message: Option<Message>| {
            button(
                row![
                    text(label).size(14),
                    space::horizontal(),
                    text(shortcut).size(12).style(text::secondary),
                ]
                .spacing(20),
            )
            .width(Fill)
            .padding([6, 12])
            .style(button::text)
            .on_press_maybe(message)
        };
        let has_content = !self.layers.is_empty();

        let (offset, items) = match menu {
            Menu::File => {
                let mut items = column![
                    item(
                        "New",
                        self.keys.first(Action::New),
                        has_content.then_some(Message::File(FileMessage::New))
                    ),
                    item(
                        "Open…",
                        self.keys.first(Action::Open),
                        Some(Message::File(FileMessage::Open))
                    ),
                ];
                // The recent files: their names, their folders dimmed.
                let recent = self.recent_files.paths();
                if !recent.is_empty() {
                    items = items.push(
                        container(text("Recent").size(11).style(text::secondary)).padding(
                            Padding {
                                top: 6.0,
                                left: 12.0,
                                ..Padding::ZERO
                            },
                        ),
                    );
                    for path in recent {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        let folder = path
                            .parent()
                            .and_then(Path::file_name)
                            .unwrap_or_default()
                            .to_string_lossy();
                        items = items.push(
                            button(
                                row![
                                    text(name).size(14).wrapping(text::Wrapping::None),
                                    space::horizontal(),
                                    text(folder)
                                        .size(12)
                                        .style(text::secondary)
                                        .wrapping(text::Wrapping::None),
                                ]
                                .spacing(12),
                            )
                            .width(Fill)
                            .padding([6, 12])
                            .style(button::text)
                            .on_press(Message::File(FileMessage::OpenRecent(path.clone()))),
                        );
                    }
                    items = items.push(item(
                        "Clear recent",
                        String::new(),
                        Some(Message::File(FileMessage::ClearRecent)),
                    ));
                    items = items.push(iced::widget::rule::horizontal(1));
                }
                (
                    0.0,
                    items
                        .push(item(
                            "Save",
                            self.keys.first(Action::Save),
                            Some(Message::File(FileMessage::Save)),
                        ))
                        .push(item(
                            "Save As…",
                            self.keys.first(Action::SaveAs),
                            Some(Message::File(FileMessage::SaveAs)),
                        ))
                        .push(item(
                            "Export SVG…",
                            String::new(),
                            has_content.then_some(Message::Export(ExportMessage::ExportSvg)),
                        ))
                        .push(item(
                            "Export PNG…",
                            String::new(),
                            has_content
                                .then_some(Message::Export(ExportMessage::ShowExportPng(true))),
                        )),
                )
            }
            Menu::Edit => (
                MENU_WIDTH + 10.0,
                column![
                    item(
                        "Undo",
                        self.keys.first(Action::Undo),
                        (!self.undo.is_empty()).then_some(Message::Undo)
                    ),
                    item(
                        "Redo",
                        self.keys.first(Action::Redo),
                        (!self.redo.is_empty()).then_some(Message::Redo)
                    ),
                ],
            ),
            Menu::View => (
                2.0 * (MENU_WIDTH + 10.0),
                column![item("Reset view", String::new(), Some(Message::ResetView))],
            ),
            Menu::Help => (
                3.0 * (MENU_WIDTH + 10.0),
                column![
                    item(
                        "Keyboard shortcuts…",
                        self.keys.first(Action::ShowKeys),
                        Some(Message::Keys(KeysMessage::ShowKeys(true)))
                    ),
                    item(
                        "About Tessera…",
                        String::new(),
                        Some(Message::Menu(MenuMessage::About(true)))
                    ),
                ],
            ),
        };

        let panel = container(items.width(260))
            .padding(4)
            .style(container::bordered_box);

        // Below the menu bar: its buttons stay usable, to switch menus.
        let outside = column![
            space().height(MENU_BAR_HEIGHT),
            mouse_area(space().width(Fill).height(Fill))
                .on_press(Message::Menu(MenuMessage::CloseMenu)),
        ];

        stack![
            outside,
            container(opaque(panel)).padding(Padding {
                top: MENU_BAR_HEIGHT,
                left: 8.0 + offset,
                ..Padding::ZERO
            }),
        ]
        .into()
    }
}

/// Buttons side by side as one control, split in parts, the active ones
/// highlighted.
pub(super) fn joined<'a>(parts: Vec<(&'a str, bool, Message)>) -> Element<'a, Message> {
    let last = parts.len().saturating_sub(1);
    row(parts
        .into_iter()
        .enumerate()
        .map(|(i, (label, active, message))| {
            button(text(label).size(13))
                .padding([4, 12])
                .style(move |theme: &Theme, status| {
                    let mut style = if active {
                        button::primary(theme, status)
                    } else {
                        button::secondary(theme, status)
                    };
                    let mut radius = style.border.radius;
                    if i > 0 {
                        radius = radius.top_left(0).bottom_left(0);
                    }
                    if i < last {
                        radius = radius.top_right(0).bottom_right(0);
                    }
                    style.border.radius = radius;
                    style
                })
                .on_press(message)
                .into()
        }))
    .spacing(1)
    .into()
}

/// A 16 px icon, coloured as the label of a button in `style`.
pub(super) fn icon<'a>(
    shapes: &str,
    style: fn(&Theme, button::Status) -> button::Style,
) -> iced::widget::Svg<'a> {
    iced::widget::svg(iced::widget::svg::Handle::from_memory(icons::svg(shapes)))
        .width(16)
        .height(16)
        .style(move |theme: &Theme, _| iced::widget::svg::Style {
            color: Some(style(theme, button::Status::Active).text_color),
        })
}

/// What a tooltip says, in a dark box.
pub(super) fn tip<'a>(label: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(text(label).size(12))
        .padding([4, 8])
        .style(container::dark)
        .into()
}
