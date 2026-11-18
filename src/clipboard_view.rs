//! The clipboard history's view: the entries, the highlighted one's
//! preview, a footer, and an actions panel (⌘K) as Raycast's.

use nsh_base::{CommandEntry, CommandGroup, CommandItem, command};
use nsh_core::{element::ImageSrc, keymap::When, prelude::*};

use crate::{
    Launcher, back_button, clipboard, clipboard::ClipEntry, icons, mac, palette, reactive_src,
    row_icon, styled,
};

/// What the actions panel shows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Page {
    Main,
    /// Which of the latest entries to delete.
    DeleteRange,
    /// "Delete every entry?"
    ConfirmDeleteAll,
}

/// The ranges "Delete Entries…" offers, in minutes.
const RANGES: &[(u64, &str)] = &[
    (5, "Last 5 Minutes"),
    (15, "Last 15 Minutes"),
    (30, "Last 30 Minutes"),
    (60, "Last Hour"),
    (24 * 60, "Last 24 Hours"),
];

/// The view's state, shared by its parts.
#[derive(Clone, Copy)]
struct View {
    launcher: Launcher,
    highlighted: Signal<Option<u64>>,
    /// The main search field's query.
    query: Signal<SharedString>,
    /// The actions panel's page while it is open.
    actions: Signal<Option<Page>>,
    /// The actions panel's query.
    actions_query: Signal<SharedString>,
}

impl View {
    fn find(self, id: Option<u64>) -> Option<ClipEntry> {
        self.launcher
            .history
            .with(|entries| entries.iter().find(|e| Some(e.id) == id).cloned())
    }

    fn current(self) -> Option<ClipEntry> {
        self.find(self.highlighted.get_untracked())
    }

    fn open_actions(self, page: Page) {
        self.actions_query.set(SharedString::default());
        self.actions.set(Some(page));
    }

    fn close_actions(self) {
        self.actions.set(None);
        self.launcher.focus_search();
    }

    fn paste(self) {
        if let Some(entry) = self.current() {
            self.launcher.paste(&entry);
        }
    }

    fn copy(self) {
        if let Some(entry) = self.current() {
            self.launcher.copy(&entry);
        }
    }

    fn delete(self) {
        if let Some(id) = self.highlighted.get_untracked() {
            clipboard::remove(self.launcher.history, |e| e.id == id);
        }
        self.close_actions();
    }

    fn delete_last(self, minutes: u64) {
        let since = (nsh_core::time::unix_ms() as u64).saturating_sub(minutes * 60_000);
        clipboard::remove(self.launcher.history, |e| e.copied_at >= since);
        self.close_actions();
    }

    fn delete_all(self) {
        clipboard::clear(self.launcher.history);
        self.close_actions();
    }
}

/// The app that "Paste" pastes into: the one in front, which the
/// non-activating popup leaves in front.
struct Target {
    name: String,
    icon: Option<String>,
}

fn target() -> Target {
    let app = mac::frontmost_app();
    let name = app.as_ref().and_then(|a| a.name.clone()).unwrap_or_else(|| "Active App".into());
    let icon = app.and_then(|a| a.path).and_then(|path| {
        let icon = icons::cached_path(&path);
        icon.exists().then(|| icon.to_string_lossy().into_owned())
    });
    Target { name, icon }
}

pub fn clipboard_view(launcher: Launcher) -> impl IntoElement {
    let view_state = View {
        launcher,
        highlighted: signal(None),
        query: signal(SharedString::default()),
        actions: signal(None),
        actions_query: signal(SharedString::default()),
    };
    let v = view_state;
    let target_name = target().name;

    let always = KeyOptions { when: When::Always, ..KeyOptions::default() };
    on_key_with("mod+k", always, move |_| match v.actions.get_untracked() {
        Some(_) => v.close_actions(),
        None => v.open_actions(Page::Main),
    });
    on_key_with("mod+enter", always, move |_| v.copy());
    on_key_with("ctrl+x", always, move |_| v.delete());
    on_key_with("mod+shift+x", always, move |_| v.open_actions(Page::DeleteRange));
    on_key_with("ctrl+shift+x", always, move |_| v.open_actions(Page::ConfirmDeleteAll));

    view()
        .flex_1()
        // Backspace in an empty search field goes back, as in Raycast: from
        // the entries to the launcher, from an actions page to the first.
        // Bound only while the field is empty, so it deletes text otherwise.
        .child(when(
            move || v.actions.get().is_none() && v.query.with(|q| q.is_empty()),
            move || {
                on_key_with("backspace", always, move |_| v.launcher.back());
                view()
            },
        ))
        .child(when(
            move || {
                v.actions.get().is_some_and(|p| p != Page::Main)
                    && v.actions_query.with(|q| q.is_empty())
            },
            move || {
                on_key_with("backspace", always, move |_| v.open_actions(Page::Main));
                view()
            },
        ))
        .child(view().flex_1().flex_row().child(entries(v)).child(preview(v)))
        .child(footer(v, target_name))
        .child(when(move || v.actions.get().is_some(), move || actions_panel(v)))
}

fn entries(v: View) -> impl IntoElement {
    let parse_id = |id: &str| id.strip_prefix("clip:").and_then(|n| n.parse::<u64>().ok());
    let items = move || {
        v.launcher.history.with(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let keywords = match entry.image {
                        Some(_) => "image picture screenshot 图片".to_owned(),
                        None => entry.text.chars().take(2000).collect(),
                    };
                    CommandItem::new(format!("clip:{}", entry.id), entry.label()).keywords([keywords])
                })
                .collect::<Vec<_>>()
        })
    };
    palette(v.launcher)
        .items(items)
        .placeholder("Search clipboard history…")
        .search_prefix(back_button(v.launcher))
        .on_select(move |id: &SharedString| {
            if let Some(entry) = v.find(parse_id(id)) {
                v.launcher.paste(&entry);
            }
        })
        .on_highlight_change(move |id: &Option<SharedString>| {
            v.highlighted.set(id.as_deref().and_then(parse_id));
        })
        .on_query_change(move |q: &SharedString| v.query.set(q.clone()))
        .on_cancel(move || {
            if v.launcher.nested.get() {
                v.launcher.back();
            } else {
                v.launcher.close();
            }
        })
        .render_item(move |entry: &CommandEntry| {
            // An image shows itself; text, the app it came from.
            let id = parse_id(&entry.id);
            let src = reactive_src(move || {
                let entry = v.find(id)?;
                if let Some(image) = entry.image {
                    return Some(image.file);
                }
                let icon = icons::cached_path(&entry.source_path?);
                icon.exists().then(|| icon.to_string_lossy().into_owned())
            });
            view()
                .flex_1()
                .flex_row()
                .items_center()
                .gap_2p5()
                .child(row_icon(src))
                .child(text(entry.label.clone()).flex_1().app_class("text-sm").number_of_lines(1))
        })
        .empty(text("Nothing copied yet").app_class("py-6 text-center text-sm text-muted-foreground"))
        .w(px(320.0))
        .app_class("border-r border-border")
}

fn preview(v: View) -> impl IntoElement {
    let highlighted = move || v.find(v.highlighted.get());
    view()
        .flex_1()
        .p_4()
        .gap_3()
        .child(
            when(
                move || highlighted().is_some_and(|e| e.image.is_some()),
                move || {
                    let file = move || highlighted().and_then(|e| e.image).map(|i| i.file);
                    image(move || ImageSrc::from(file().unwrap_or_default()))
                        .flex_1()
                        .object_fit(ResizeMode::Contain)
                },
            )
            .otherwise(move || {
                text(move || highlighted().map(|e| e.text).unwrap_or_default())
                    .flex_1()
                    .app_class("text-sm font-mono")
                    .number_of_lines(24)
            }),
        )
        .child(
            text(move || highlighted().map(|e| details(&e)).unwrap_or_default())
                .app_class("text-xs text-muted-foreground"),
        )
}

/// The bar under the view: what it is, what Return does, and the actions.
fn footer(v: View, target_name: String) -> impl IntoElement {
    view()
        .h(px(40.0))
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .app_class("border-t border-border/60 bg-foreground/[0.03]")
        .child(image(icons::svg_uri(icons::CLIPBOARD_SVG)).w(px(18.0)).h(px(18.0)))
        .child(text("Clipboard History").flex_1().app_class("text-sm text-muted-foreground"))
        .child(text(format!("Paste to {target_name}")).app_class("text-sm"))
        .child(keycaps("↩"))
        .child(view().w(px(1.0)).h(px(16.0)).mx_1().app_class("bg-border"))
        .child(
            nsh_base::button()
                .flex_row()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_1()
                .rounded(px(6.0))
                .app_class("hover:bg-foreground/10 active:opacity-70")
                .on_press(move || match v.actions.get_untracked() {
                    Some(_) => v.close_actions(),
                    None => v.open_actions(Page::Main),
                })
                .child(text("Actions").app_class("text-sm"))
                .child(keycaps("⌘K")),
        )
}

/// A shortcut's keys, one cap each ("⇧⌘X").
fn keycaps(keys: &str) -> impl IntoElement {
    view().flex_row().gap_1().child(
        keys.chars()
            .map(|key| {
                view()
                    .min_w(px(20.0))
                    .h(px(20.0))
                    .px_1()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.0))
                    .app_class("bg-foreground/[0.06]")
                    .child(text(key.to_string()).app_class("text-xs text-muted-foreground"))
            })
            .collect::<Vec<_>>(),
    )
}

/// The panel ⌘K opens over the view's bottom right.
fn actions_panel(v: View) -> impl IntoElement {
    view()
        .app_class(
            "absolute right-3 bottom-12 w-80 overflow-hidden rounded-xl border border-border \
             bg-popover shadow-xl",
        )
        .child(dynamic(move || match v.actions.get() {
            Some(page) => actions_page(v, page).into_element(),
            None => view().into_element(),
        }))
}

fn actions_page(v: View, page: Page) -> impl IntoElement {
    let target = target();
    let count = v.launcher.history.with(Vec::len);
    let (header, items): (Option<String>, Vec<CommandItem>) = match page {
        Page::Main => (
            None,
            vec![
                CommandItem::new("paste", format!("Paste to {}", target.name))
                    .group("Use")
                    .shortcut("enter"),
                CommandItem::new("copy", "Copy to Clipboard").group("Use").shortcut("mod+enter"),
                CommandItem::new("delete", "Delete Entry").group("Delete").shortcut("ctrl+x"),
                CommandItem::new("delete-range", "Delete Entries…")
                    .group("Delete")
                    .shortcut("mod+shift+x"),
                CommandItem::new("delete-all", "Delete All Entries…")
                    .group("Delete")
                    .shortcut("ctrl+shift+x"),
            ],
        ),
        Page::DeleteRange => (
            Some("Delete Entries…".into()),
            RANGES
                .iter()
                .map(|(minutes, label)| CommandItem::new(format!("last:{minutes}"), *label))
                .collect(),
        ),
        Page::ConfirmDeleteAll => (
            Some(format!("Delete all {count} entries? This cannot be undone.")),
            vec![
                CommandItem::new("confirm-delete-all", "Delete All Entries"),
                CommandItem::new("cancel", "Cancel"),
            ],
        ),
    };
    let paste_icon = target.icon;
    // Search at the bottom, as Raycast's panel has it: the command's parts
    // (header, search, list, empty, footer) laid out bottom up, so its
    // heading goes above it instead.
    let panel = styled(command())
        .app_class("flex-col-reverse")
        .items(items)
        .auto_focus(true)
        .placeholder("Search…")
        .list_class("max-h-72 p-1.5")
        .search_bar_class("flex-row items-center border-t border-border px-3")
        .search_class("h-10 flex-1 text-sm text-foreground placeholder:text-muted-foreground")
        // Sections without headings: the separator between them is enough.
        .render_group(|_: &CommandGroup| view())
        .group_class("h-0 p-0")
        .on_query_change(move |q: &SharedString| v.actions_query.set(q.clone()))
        .on_cancel(move || match page {
            Page::Main => v.close_actions(),
            _ => v.open_actions(Page::Main),
        })
        .on_select(move |id: &SharedString| match id.as_ref() {
            "paste" => v.paste(),
            "copy" => v.copy(),
            "delete" => v.delete(),
            "delete-range" => v.open_actions(Page::DeleteRange),
            "delete-all" => v.open_actions(Page::ConfirmDeleteAll),
            "confirm-delete-all" => v.delete_all(),
            "cancel" => v.open_actions(Page::Main),
            other => {
                if let Some(minutes) = other.strip_prefix("last:").and_then(|m| m.parse().ok()) {
                    v.delete_last(minutes);
                }
            }
        })
        .render_item(move |entry: &CommandEntry| {
            let destructive = entry.id.starts_with("delete") || entry.id == "confirm-delete-all";
            let icon = match entry.id.as_ref() {
                "paste" => Some(image(
                    paste_icon.clone().unwrap_or_else(|| icons::svg_uri(icons::CLIPBOARD_SVG)),
                )),
                "copy" => Some(
                    image(icons::svg_uri(icons::COPY_SVG))
                        .tint_color(move || nsh_core::element::AdaptiveColor::from(
                            nsh_base::theme().foreground,
                        )),
                ),
                id if id.starts_with("delete") => Some(image(icons::svg_uri(icons::TRASH_SVG))),
                _ => None,
            };
            view()
                .flex_1()
                .flex_row()
                .items_center()
                .gap_2p5()
                .child(icon.map(|i| i.w(px(18.0)).h(px(18.0))))
                .child(
                    text(entry.label.clone())
                        .flex_1()
                        .app_class(if destructive { "text-sm text-destructive" } else { "text-sm" }),
                )
                .child(keycaps(entry.shortcut_label.as_deref().unwrap_or_default()))
        });
    view()
        .child(header.map(|header| {
            text(header).app_class("px-4 pt-3 pb-1 text-xs font-medium text-muted-foreground")
        }))
        .child(panel)
}

fn details(entry: &ClipEntry) -> String {
    let what = match &entry.image {
        Some(image) => format!(
            "Image · {}×{} · {}",
            image.width,
            image.height,
            match image.bytes {
                0..1024 => format!("{} B", image.bytes),
                1024..1_048_576 => format!("{:.0} KB", image.bytes as f64 / 1024.0),
                _ => format!("{:.1} MB", image.bytes as f64 / 1_048_576.0),
            }
        ),
        None => format!(
            "{} characters · {} lines",
            entry.text.chars().count(),
            entry.text.lines().count()
        ),
    };
    let ago = (nsh_core::time::unix_ms() as u64).saturating_sub(entry.copied_at) / 1000;
    let ago = match ago {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", ago / 60),
        3600..86400 => format!("{} h ago", ago / 3600),
        _ => format!("{} d ago", ago / 86400),
    };
    let source = entry.source.as_deref().map(|s| format!(" · from {s}")).unwrap_or_default();
    format!("{what} · {ago}{source}")
}
