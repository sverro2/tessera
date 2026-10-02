//! Dialogs over the window: exporting a PNG, the keyboard shortcuts, about
//! Tessera, and saving changes first.

use super::*;
// Named, as the standard library has a `column!` of its own.
use iced::widget::{column, row};

impl Tessera {
    /// Asks what to do with unsaved changes, over the dimmed window.
    /// What Tessera is, its version, and whose work it builds on (with
    /// their licences).
    /// Exporting a PNG: how large (with its size in pixels) and on what,
    /// then where.
    pub(super) fn png_dialog(&self) -> Element<'_, Message> {
        let small = |label: String| text(label).size(13);
        let (_, area) = svg::area(&self.layers);
        let (width, height) = raster::pixels(area.width, area.height, self.png_scale);
        let fits = width <= raster::MAX_SIDE && height <= raster::MAX_SIDE;
        let scales: Vec<_> = [1.0, 2.0, 4.0, 8.0]
            .into_iter()
            .map(|scale| {
                (
                    match scale {
                        1.0 => "1×",
                        2.0 => "2×",
                        4.0 => "4×",
                        _ => "8×",
                    },
                    self.png_scale == scale,
                    Message::Export(ExportMessage::PngScale(scale)),
                )
            })
            .collect();
        let backdrops: Vec<_> = raster::Backdrop::ALL
            .into_iter()
            .map(|backdrop| {
                (
                    backdrop.label(),
                    self.png_backdrop == backdrop,
                    Message::Export(ExportMessage::PngBackdrop(backdrop)),
                )
            })
            .collect();
        let size = if fits {
            text(format!("{width} × {height} pixels"))
                .size(12)
                .style(text::secondary)
        } else {
            text(format!("{width} × {height} pixels: too large"))
                .size(12)
                .style(text::danger)
        };

        modal(
            column![
                text("Export PNG").size(20),
                row![small("Size".into()).width(80), joined(scales), size]
                    .spacing(10)
                    .align_y(Center),
                row![small("Background".into()).width(80), joined(backdrops)]
                    .spacing(10)
                    .align_y(Center),
                text("Hidden layers are left out, as in the SVG.")
                    .size(12)
                    .style(text::secondary),
                row![
                    space::horizontal(),
                    button("Cancel")
                        .style(button::secondary)
                        .on_press(Message::Export(ExportMessage::ShowExportPng(false))),
                    button("Export…")
                        .on_press_maybe(fits.then_some(Message::Export(ExportMessage::ExportPng))),
                ]
                .spacing(8),
            ]
            .spacing(14)
            .width(440),
            Message::Export(ExportMessage::ShowExportPng(false)),
        )
    }

    /// The keyboard shortcuts, by where they work, to look up and to set:
    /// click a key, then press the new one; add one; take one away; back
    /// to the defaults. What the mouse does with keys held is listed too,
    /// fixed.
    pub(super) fn shortcuts(&self) -> Element<'_, Message> {
        let small = |label: String| text(label).size(13);
        let waiting = || text("Press a key…").size(12);

        let mut sections = column![].spacing(16);
        for context in Context::ALL {
            let mut rows = column![text(context.label()).size(16)].spacing(6);
            for action in Action::ALL.into_iter().filter(|a| a.context() == context) {
                let mut keys = row![].spacing(6).align_y(Center);
                for (i, chord) in self.keys.keys(action).iter().enumerate() {
                    let setting = self.rebinding == Some((action, Some(i)));
                    let label = if setting {
                        waiting()
                    } else {
                        text(chord.to_string()).size(12)
                    };
                    keys = keys.push(
                        row![
                            button(label)
                                .padding([2, 8])
                                .style(if setting {
                                    button::primary
                                } else {
                                    button::secondary
                                })
                                .on_press(Message::Keys(KeysMessage::Rebind(Some((
                                    action,
                                    Some(i)
                                ))))),
                            button(text("×").size(12))
                                .padding([2, 6])
                                .style(button::text)
                                .on_press(Message::Keys(KeysMessage::Unbind(action, i))),
                        ]
                        .align_y(Center),
                    );
                }
                let adding = self.rebinding == Some((action, None));
                keys = keys.push(tooltip(
                    button(if adding {
                        waiting()
                    } else {
                        text("+").size(12)
                    })
                    .padding([2, 8])
                    .style(if adding {
                        button::primary
                    } else {
                        button::text
                    })
                    .on_press(Message::Keys(KeysMessage::Rebind(Some((action, None))))),
                    tip("Add a key"),
                    tooltip::Position::Bottom,
                ));
                if !self.keys.is_default(action) {
                    keys = keys.push(tooltip(
                        button(text("Default").size(11))
                            .padding([2, 6])
                            .style(button::text)
                            .on_press(Message::Keys(KeysMessage::ResetKey(action))),
                        tip(format!("Back to {}", action_defaults(action))),
                        tooltip::Position::Bottom,
                    ));
                }
                rows = rows.push(
                    row![small(action.label().into()).width(280), keys]
                        .spacing(8)
                        .align_y(Center),
                );
            }
            for (_, keys, what) in keys::GESTURES.iter().filter(|g| g.0 == context) {
                rows = rows.push(
                    row![
                        small((*what).into()).width(280),
                        text(*keys).size(12).style(text::secondary),
                    ]
                    .spacing(8)
                    .align_y(Center),
                );
            }
            sections = sections.push(rows);
        }

        modal(
            column![
                text("Keyboard shortcuts").size(22),
                text(
                    "Click a key to change it, then press the new one (Esc: never mind). \
                     The mouse with keys held (in grey) can't be changed. Kept in your \
                     config folder, in tessera/keys.json."
                )
                .size(12)
                .style(text::secondary),
                scrollable(sections).height(440),
                row![
                    button(text("Reset all to defaults").size(13))
                        .style(button::secondary)
                        .on_press(Message::Keys(KeysMessage::ResetKeys)),
                    space::horizontal(),
                    button("Close").on_press(Message::Keys(KeysMessage::ShowKeys(false))),
                ]
                .align_y(Center),
            ]
            .spacing(12)
            .width(620),
            Message::Keys(KeysMessage::ShowKeys(false)),
        )
    }

    pub(super) fn about_box(&self) -> Element<'_, Message> {
        modal(
            column![
                row![
                    text("Tessera").size(24),
                    text(format!("version {}", env!("CARGO_PKG_VERSION")))
                        .size(13)
                        .style(text::secondary),
                ]
                .spacing(10)
                .align_y(iced::Bottom),
                text(
                    "Draw with triangles: shape a mesh of them, over a background \
                     image if you like, paint its faces and edges, in layers, and \
                     export it to SVG."
                )
                .size(14),
                column![
                    text("Idea and UI/UX design by Bibi Brettschneider").size(13),
                    text(format!(
                        "Made by {}",
                        env!("CARGO_PKG_AUTHORS").replace(':', ", ")
                    ))
                    .size(13),
                ]
                .spacing(4),
                text("Built with iced (iced.rs).").size(13),
                text("Icons from Lucide (lucide.dev), under the ISC licence:").size(13),
                container(
                    scrollable(
                        text(include_str!("../THIRD_PARTY_LICENSES"))
                            .size(11)
                            .font(iced::Font::MONOSPACE),
                    )
                    .height(200),
                )
                .padding(8)
                .style(container::bordered_box),
                row![
                    space::horizontal(),
                    button("Close").on_press(Message::Menu(MenuMessage::About(false))),
                ],
            ]
            .spacing(12)
            .width(480),
            Message::Menu(MenuMessage::About(false)),
        )
    }

    pub(super) fn confirmation(&self) -> Element<'_, Message> {
        modal(
            column![
                text(format!("Save changes to “{}”?", self.name())).size(18),
                text("Your changes will be lost if you don't save them.").size(14),
                row![
                    button("Don't save")
                        .style(button::danger)
                        .on_press(Message::File(FileMessage::Confirm(Choice::Discard))),
                    space::horizontal(),
                    button("Cancel")
                        .style(button::secondary)
                        .on_press(Message::File(FileMessage::Confirm(Choice::Cancel))),
                    button("Save").on_press(Message::File(FileMessage::Confirm(Choice::Save))),
                ]
                .spacing(10),
            ]
            .spacing(16)
            .width(380),
            Message::File(FileMessage::Confirm(Choice::Cancel)),
        )
    }
}

/// An action's keys out of the box, written out.
pub(super) fn action_defaults(action: Action) -> String {
    let keys = Keymap::default();
    let chords: Vec<_> = keys.keys(action).iter().map(ToString::to_string).collect();
    if chords.is_empty() {
        "no key".into()
    } else {
        chords.join(", ")
    }
}

/// `dialog` over the window, the rest dimmed: clicking there (or Esc) is
/// `on_close`.
pub(super) fn modal<'a>(
    dialog: impl Into<Element<'a, Message>>,
    on_close: Message,
) -> Element<'a, Message> {
    let dialog = container(dialog).padding(20).style(container::bordered_box);
    let backdrop = center(opaque(dialog)).style(|_| {
        container::Style::default().background(Color {
            a: 0.5,
            ..Color::BLACK
        })
    });
    opaque(mouse_area(backdrop).on_press(on_close))
}
