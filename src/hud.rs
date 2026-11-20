//! A one-line message shown for a moment near the bottom of the display the
//! mouse pointer is on, as Raycast's HUD: what a script run with the popup
//! closed has to say.

use std::time::Duration;

use nsh_core::{
    desktop::{Display, WindowAttributes, WindowFrame, WindowKind, WindowLevel, WindowPosition},
    element::desktop::{WindowRef, open_window, window},
    geometry::Point,
    prelude::*,
};

/// The window's size: the pill fills it, since the window takes the
/// pointer where it is, transparent or not.
const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 40.0;
/// How far above the bottom of the display's visible area it shows.
const MARGIN: f32 = 96.0;
const SHOWN_FOR: Duration = Duration::from_secs(2);

#[derive(Clone, PartialEq, Default)]
struct Message {
    text: String,
    failed: bool,
}

/// The HUD. Made once, by the app's root: its window and timer belong to
/// the root's owner, so they outlive the popup.
#[derive(Clone, Copy)]
pub(crate) struct Hud {
    owner: Owner,
    message: Signal<Message>,
    window: Stored<Option<WindowRef>>,
    /// Closes the window; replaced, so an earlier message's timer does not
    /// cut a later one short.
    timer: Stored<Option<Spawned<()>>>,
}

impl Hud {
    pub(crate) fn new() -> Self {
        Hud { owner: Owner::current(), message: signal(Message::default()), window: stored(None), timer: stored(None) }
    }

    /// Shows `text` (one line, cut short when too long) for two seconds,
    /// red when `failed`; replaces the message shown, and moves to the
    /// display the pointer is on now.
    pub(crate) fn show(self, text: &str, failed: bool) {
        self.owner.run(|| {
            self.message.set(Message { text: text.to_owned(), failed });
            let at = crate::mouse_display().map(|display| top_left(&display));
            match self.window.get() {
                // `open_window` with the id of an open window only focuses it.
                Some(open) => {
                    if let Some(at) = at {
                        open.set_bounds(WindowFrame { x: Some(at.x), y: Some(at.y), ..WindowFrame::default() });
                    }
                }
                None => self.open(at),
            }
            if let Some(timer) = self.timer.replace(None) {
                timer.cancel();
            }
            let timer = set_timeout(SHOWN_FOR, move || {
                if let Some(open) = self.window.replace(None) {
                    open.close();
                }
            });
            self.timer.replace(Some(timer));
        });
    }

    fn open(self, at: Option<Point<Points>>) {
        let mut attributes = WindowAttributes::default();
        // It never takes the keyboard from the app in front.
        attributes.focusable = Some(false);
        attributes.activate = Some(false);
        attributes.close_on_escape = Some(false);
        attributes.close_on_blur = Some(false);
        attributes.transparent = Some(true);
        attributes.shadow = Some(false);
        // Over full-screen windows and floating panels too.
        attributes.level = Some(WindowLevel::AboveDock);
        let message = self.message;
        let options = window()
            .id("hud")
            .kind(WindowKind::Popup)
            .default_size(size(px(WIDTH), px(HEIGHT)))
            .default_position(at.map_or(WindowPosition::Center, WindowPosition::At))
            .attributes(attributes)
            .on_open_change(move |_| {
                self.window.replace(None);
            })
            .content(move || {
                view()
                    .flex_1()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .px_4()
                    .rounded_full()
                    .overflow_hidden()
                    .macos_backdrop(crate::frosted_glass())
                    .app_class(crate::FROSTED_GLASS_CLASS)
                    .child(
                        text(move || message.with(|m| m.text.clone()))
                            .number_of_lines(1)
                            .app_class(move || {
                                let color = if message.with(|m| m.failed) {
                                    "text-destructive"
                                } else {
                                    "text-popover-foreground"
                                };
                                Some(format!("text-sm font-medium {color}").into())
                            }),
                    )
            });
        self.window.replace(open_window(options));
    }
}

/// Where the window's top left corner goes on `display`: centered, near the
/// bottom of its visible area.
fn top_left(display: &Display) -> Point<Points> {
    let area = display.visible_bounds;
    Point {
        x: px(area.left().0 + (area.size.width.0 - WIDTH) / 2.0),
        y: px(area.bottom().0 - MARGIN - HEIGHT),
    }
}
