//! Tinycast: an app launcher and clipboard history for macOS.
//!
//! The app lives in the menu bar with its main window hidden; global
//! shortcuts open a non-activating popup, so the app the user is in keeps
//! the focus and gets it back, with a paste, when the popup closes.

mod apps;
mod clipboard;
mod clipboard_view;
mod config;
mod icons;
mod mac;

use std::{collections::HashSet, time::Duration};

use nsh_base::{Command, CommandApi, CommandEntry, CommandItem, button, command, theme};
use nsh_core::{
    MacOsBackdrop, MacOsBackdropBlending, MacOsBackdropMaterial, MacOsBackdropState,
    desktop::{WindowAttributes, WindowKind, WindowPosition},
    element::{AdaptiveColor, ImageSrc},
    element::desktop::{
        MenuItem, WindowRef, current_window, open_window, quit, register_global_shortcut, set_tray,
        tray, window,
    },
    prelude::*,
};

use crate::{apps::AppEntry, clipboard::ClipEntry};


/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("tinycast/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Apps,
    Clipboard,
}

/// What the popup shows, shared by the shortcuts, the tray and the popup.
#[derive(Clone, Copy)]
pub(crate) struct Launcher {
    pub(crate) apps: Signal<Vec<AppEntry>>,
    /// Where the running apps are, as of the popup's opening: their rows
    /// show a dot.
    running: Signal<HashSet<String>>,
    pub(crate) history: Signal<Vec<ClipEntry>>,
    /// The user's settings (`config.toml`).
    settings: Signal<config::Loaded>,
    pub(crate) mode: Signal<Mode>,
    /// The clipboard view was entered from the launcher's list, so Escape
    /// goes back to it rather than closing the popup.
    pub(crate) nested: Stored<bool>,
    pub(crate) popup: Stored<Option<WindowRef>>,
    /// The palette the popup shows, whichever mode.
    pub(crate) search: ComponentRef<Command>,
}

impl Launcher {
    /// Shows the popup in `mode`; a press of the shortcut of the mode it
    /// shows closes it.
    fn toggle(self, mode: Mode) {
        let open = self.popup.get();
        if open.is_some() && self.mode.get_untracked() == mode {
            self.close();
            return;
        }
        self.mode.set(mode);
        self.nested.replace(false);
        self.running.set(mac::running_app_paths());
        if let Some(popup) = open {
            popup.focus();
            return;
        }
        let mut attributes = WindowAttributes::default();
        // A non-activating panel: the app in front stays in front.
        attributes.activate = Some(false);
        // The content draws the rounded surface; the window around it shows
        // nothing but its shadow.
        attributes.transparent = Some(true);
        attributes.shadow = Some(true);
        // The palette answers Escape: it clears the query, then goes back
        // or closes the popup (`on_cancel`).
        attributes.close_on_escape = Some(false);
        let popup = open_window(
            window()
                .id("launcher")
                .kind(WindowKind::Popup)
                .default_size(size(px(760.0), px(460.0)))
                .default_position(WindowPosition::Center)
                .attributes(attributes)
                .on_open_change(move |_| {
                    self.popup.replace(None);
                })
                .content(move || popup_content(self)),
        );
        self.popup.replace(popup);
        // The window is made when this turn commits; a focus request sent
        // before that is lost, and the popup would show without the keyboard.
        request_animation_frame(move |_| {
            if let Some(popup) = self.popup.get() {
                popup.focus();
            }
        });
    }

    /// Puts the keyboard in the palette's search field. `auto_focus` alone
    /// can miss it: it runs before the popup's field is in a window.
    pub(crate) fn focus_search(self) {
        if let Some(api) = self.search.api::<CommandApi>() {
            api.focus();
        }
    }

    /// From the clipboard view back to the launcher's list.
    pub(crate) fn back(self) {
        self.nested.replace(false);
        self.mode.set(Mode::Apps);
    }

    pub(crate) fn close(self) {
        if let Some(popup) = self.popup.replace(None) {
            popup.close();
        }
    }

    /// Puts `entry` back on the clipboard, closes the popup and pastes it
    /// into the app in front.
    pub(crate) fn paste(self, entry: &ClipEntry) {
        if !clipboard::put(entry) {
            return log::warn!("the entry's image file is gone");
        }
        self.close();
        if !mac::can_post_keys() {
            log::warn!("Tinycast needs Accessibility access to paste; the text is on the clipboard");
            drop(open_url(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
            ));
            return;
        }
        // On a thread of its own: the popup's owner goes with the popup.
        blocking(|| {
            // Gives the app in front back its key window first.
            std::thread::sleep(Duration::from_millis(120));
            mac::post_paste();
        });
    }

    /// Puts `entry` back on the clipboard and closes the popup, without
    /// pasting.
    pub(crate) fn copy(self, entry: &ClipEntry) {
        if clipboard::put(entry) {
            self.close();
        }
    }

    fn launch(self, path: String) {
        self.close();
        let opened = open_path(path);
        spawn(async move {
            if let Err(error) = opened.await {
                log::warn!("could not open the app: {error}");
            }
        });
    }
}

/// The application's root: the hidden main window's content, which keeps
/// the state, the shortcuts, the tray and the clipboard watch.
#[component]
pub fn App() -> impl IntoElement {
    let launcher = Launcher {
        apps: signal(Vec::new()),
        running: signal(HashSet::new()),
        history: signal(clipboard::load()),
        settings: signal(config::load()),
        mode: signal(Mode::Apps),
        nested: stored(false),
        popup: stored(None),
        search: component_ref(),
    };

    // The names first, then the icons, which take a while the first time.
    spawn(async move {
        let Ok(found) = blocking(apps::scan).await else {
            return log::warn!("could not list the apps");
        };
        launcher.apps.set(found.clone());
        if let Ok(with_icons) = blocking(move || apps::with_icons(found)).await {
            launcher.apps.set(with_icons);
        }
    });
    clipboard::watch(launcher.history);

    config::watch(launcher.settings);
    for problem in launcher.settings.with_untracked(|s| s.problems.clone()) {
        log::warn!("{problem}");
    }

    // The global shortcuts, registered again when the config changes them:
    // the effect's cleanup gives the previous ones back first.
    let hotkeys = computed(move || launcher.settings.with(|s| s.config.hotkeys.clone()));
    // Keystrokes the system would not give: another app holds them.
    let held = signal(Vec::<String>::new());
    effect(move || {
        let hotkeys = hotkeys.get();
        held.set(Vec::new());
        for (key, mode) in [(hotkeys.launcher, Mode::Apps), (hotkeys.clipboard, Mode::Clipboard)] {
            let taken = register_global_shortcut(&key, move || launcher.toggle(mode));
            spawn(async move {
                // Spotlight's ⌘Space is the system's: registering it
                // succeeds, and Spotlight still gets the key.
                let spotlight = is_cmd_space(&key)
                    && blocking(mac::spotlight_takes_cmd_space).await.unwrap_or(false);
                if !taken.await || spotlight {
                    log::warn!("{key} is held by {}", if spotlight { "Spotlight" } else { "another app" });
                    held.update(|h| h.push(key));
                }
            });
        }
    });

    // The tray, with what went wrong with the settings above its items.
    effect(move || {
        let mut problems = launcher.settings.with(|s| s.problems.clone());
        problems.extend(held.get().into_iter().map(|key| {
            if is_cmd_space(&key) {
                format!("{key} is held by Spotlight: turn its shortcut off in System Settings")
            } else {
                format!("{key} is held by another app")
            }
        }));
        let mut items: Vec<MenuItem> = problems
            .into_iter()
            .map(|problem| MenuItem::new(format!("⚠︎ {problem}")).disabled(true))
            .collect();
        if !items.is_empty() {
            items.push(MenuItem::separator());
        }
        items.extend([
            MenuItem::new("Open Launcher").on_select(move || launcher.toggle(Mode::Apps)),
            MenuItem::new("Clipboard History").on_select(move || launcher.toggle(Mode::Clipboard)),
            MenuItem::separator(),
            MenuItem::new("Open Config File…").on_select(|| {
                let opened = open_path(config::path());
                spawn(async move {
                    if let Err(error) = opened.await {
                        log::warn!("could not open the config: {error}");
                    }
                });
            }),
            MenuItem::separator(),
            MenuItem::new("Quit Tinycast").on_select(quit),
        ]);
        set_tray(
            tray()
                .title([nsh_core::desktop::TrayTitleRun::new("TC")])
                .tooltip("Tinycast")
                .menu(items),
        );
    });

    view()
}

/// Whether `key` is ⌘Space, Spotlight's shortcut, however it is written.
fn is_cmd_space(key: &str) -> bool {
    use nsh_core::desktop::{Key, Shortcut};
    Shortcut::parse(key, true).is_ok_and(|s| {
        s.key == Key::Space && s.meta && !s.alt && !s.ctrl && !s.shift
    })
}

fn popup_content(launcher: Launcher) -> impl IntoElement {
    current_window().on_focus_change(move |focused| {
        if focused {
            launcher.focus_search();
        }
    });
    view()
        .flex_1()
        .rounded(px(14.0))
        .overflow_hidden()
        // Frosted glass, as Raycast and Spotlight: the popover material blurs
        // what is behind the (transparent) window, and a thin wash of the
        // theme's surface keeps text legible on busy backgrounds. Always
        // active: the popup's app is never the active one, and an inactive
        // material is flat gray.
        .macos_backdrop({
            let mut backdrop = MacOsBackdrop::default();
            backdrop.material = MacOsBackdropMaterial::Popover;
            backdrop.blending = MacOsBackdropBlending::BehindWindow;
            backdrop.state = MacOsBackdropState::Active;
            backdrop
        })
        .app_class("border border-foreground/10 bg-popover/55 text-popover-foreground")
        .child(dynamic(move || match launcher.mode.get() {
            Mode::Apps => app_palette(launcher).into_element(),
            Mode::Clipboard => clipboard_view::clipboard_view(launcher).into_element(),
        }))
}

const CLIPBOARD_COMMAND: &str = "cmd:clipboard";

fn app_palette(launcher: Launcher) -> impl IntoElement {
    let items = move || {
        let mut items: Vec<CommandItem> = launcher.apps.with(|apps| {
            apps.iter()
                .map(|app| {
                    CommandItem::new(format!("app:{}", app.path), app.name.clone())
                        .group("Applications")
                        .keywords([app.file_name.clone()])
                })
                .collect()
        });
        items.push(
            CommandItem::new(CLIPBOARD_COMMAND, "Clipboard History")
                .group("Commands")
                .keywords(["paste", "剪贴板"])
                .shortcut(launcher.settings.with(|s| s.config.hotkeys.clipboard.clone())),
        );
        items
    };
    palette(launcher)
        .items(items)
        .placeholder("Search apps and commands…")
        .on_select(move |id: &SharedString| {
            if id.as_ref() == CLIPBOARD_COMMAND {
                launcher.nested.replace(true);
                launcher.mode.set(Mode::Clipboard);
            } else if let Some(path) = id.strip_prefix("app:") {
                launcher.launch(path.to_owned());
            }
        })
        .on_cancel(move || launcher.close())
        .empty(text("No matches").app_class("py-6 text-center text-sm text-muted-foreground"))
        .render_item(move |entry: &CommandEntry| {
            // The commands' rows (no `app:` id) show the clipboard glyph.
            let path = entry.id.strip_prefix("app:").map(str::to_owned);
            let running = {
                let path = path.clone();
                move || path.as_ref().is_some_and(|p| launcher.running.with(|r| r.contains(p)))
            };
            let src = reactive_src(move || {
                let path = path.as_deref()?;
                launcher.apps.with(|apps| {
                    apps.iter().find(|a| a.path == path).and_then(|a| a.icon.clone())
                })
            });
            view()
                .flex_1()
                .flex_row()
                .items_center()
                .gap_2p5()
                .child(
                    // A dot under the icon of an app that is running, as
                    // Raycast and the Dock show it.
                    view()
                        .child(row_icon(src))
                        .child(when(running, || {
                            view().app_class(
                                "absolute left-[9.5px] -bottom-1.5 size-[3px] rounded-full \
                                 bg-muted-foreground",
                            )
                        })),
                )
                .child(text(entry.label.clone()).flex_1().app_class("text-sm"))
                .child(
                    text(entry.shortcut_label.clone().unwrap_or_default())
                        .app_class("text-xs text-muted-foreground"),
                )
        })
        .flex_1()
}

/// An image source that follows `icon`: the PNG it names, or the
/// clipboard glyph while there is none (an app whose icon is not made yet,
/// an entry copied from no known app).
pub(crate) fn reactive_src(icon: impl Fn() -> Option<String> + 'static) -> impl Fn() -> ImageSrc + 'static {
    move || ImageSrc::from(icon().unwrap_or_else(|| icons::svg_uri(icons::CLIPBOARD_SVG)))
}

pub(crate) fn row_icon(src: impl Fn() -> ImageSrc + 'static) -> impl IntoElement {
    image(src).w(px(22.0)).h(px(22.0)).object_fit(ResizeMode::Contain)
}

/// The search bar's way back to the launcher's list, as Raycast's views
/// have it.
pub(crate) fn back_button(launcher: Launcher) -> impl IntoElement {
    button()
        .accessibility_label("Back")
        .w(px(28.0))
        .h(px(28.0))
        .mr(px(10.0))
        .items_center()
        .justify_center()
        .rounded(px(7.0))
        .app_class("bg-foreground/5 hover:bg-foreground/10 active:opacity-70")
        .on_press(move || launcher.back())
        .child(
            image(icons::svg_uri(icons::BACK_SVG))
                .w(px(16.0))
                .h(px(16.0))
                .tint_color(move || AdaptiveColor::from(theme().foreground)),
        )
}

/// The popup's main palette, which `Launcher::focus_search` focuses.
pub(crate) fn palette(launcher: Launcher) -> nsh_base::CommandElement {
    styled(command().component_ref(&launcher.search).auto_focus(true))
}

/// `command` styled for the popup.
pub(crate) fn styled(command: nsh_base::CommandElement) -> nsh_base::CommandElement {
    command
        .search_bar_class("flex-row items-center border-b border-foreground/10 px-4")
        .search_class("h-12 flex-1 text-base text-foreground placeholder:text-muted-foreground")
        .list_class("flex-1 p-1.5")
        .group_class("px-2 pt-2 pb-1 text-xs font-medium text-muted-foreground")
        .separator_class("my-1 h-px bg-foreground/10")
        .item_class("group h-10 flex-row items-center rounded-md px-2.5 focus:bg-foreground/10")
}
