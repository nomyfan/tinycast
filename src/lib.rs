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
mod window;

use std::{collections::HashSet, time::Duration};

use nsh_base::{Command, CommandApi, CommandEntry, CommandItem, button, command, theme};
use nsh_core::{
    MacOsBackdrop, MacOsBackdropBlending, MacOsBackdropMaterial, MacOsBackdropState,
    desktop::{DisplayId, Key, Shortcut, WindowAttributes, WindowKind, WindowPosition},
    element::{AdaptiveColor, ImageSrc},
    element::desktop::{
        MenuItem, WindowRef, current_window, displays, open_window, quit,
        register_global_shortcut, set_tray, tray, window,
    },
    prelude::*,
};

use crate::{apps::AppEntry, clipboard::ClipEntry, mac::shared_hotkey::SharedHotKey};


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

/// What a global shortcut does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Show(Mode),
    Window(window::Command),
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
        // nothing. No system shadow: it rims the popup with a dark line.
        attributes.transparent = Some(true);
        attributes.shadow = Some(false);
        // The palette answers Escape: it clears the query, then goes back
        // or closes the popup (`on_cancel`).
        attributes.close_on_escape = Some(false);
        let mut options = window()
            .id("launcher")
            .kind(WindowKind::Popup)
            .default_size(size(px(760.0), px(460.0)))
            .default_position(WindowPosition::Center)
            .attributes(attributes)
            .on_open_change(move |_| {
                self.popup.replace(None);
            })
            .content(move || popup_content(self));
        // Without a display the popup opens on the hidden main window's,
        // always the same one.
        if let Some(display) = mouse_display() {
            options = options.display(display);
        }
        let popup = open_window(options);
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
        if !accessibility_granted() {
            return log::warn!("Tinycast needs Accessibility access to paste; the text is on the clipboard");
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

    fn launch(self, path: &str) {
        self.close();
        mac::launch_app(path);
    }

    fn run(self, action: Action) {
        match action {
            Action::Show(mode) => self.toggle(mode),
            Action::Window(command) => window::run(command),
        }
    }
}

/// Whether Tinycast has Accessibility access, opening its pane in System
/// Settings when it has not.
pub(crate) fn accessibility_granted() -> bool {
    if mac::has_accessibility_access() {
        return true;
    }
    drop(open_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    ));
    false
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
    let hotkeys = computed(move || {
        launcher.settings.with(|s| {
            let mut hotkeys = vec![
                (s.config.hotkeys.launcher.clone(), Action::Show(Mode::Apps)),
                (s.config.hotkeys.clipboard.clone(), Action::Show(Mode::Clipboard)),
            ];
            hotkeys.extend(s.config.window.iter().map(|(&command, keys)| (keys.clone(), Action::Window(command))));
            hotkeys
        })
    });
    // Why the system would not give a keystroke: Spotlight or another app
    // holds it.
    let held = signal(Vec::<String>::new());
    effect(move || {
        held.set(Vec::new());
        for (key, action) in hotkeys.get() {
            let taken = register_global_shortcut(&key, move || launcher.run(action));
            // Ends with the effect's run, giving a shared hot key back.
            spawn(async move {
                let report = |problem: String| {
                    log::warn!("{problem}");
                    held.update(|h| h.push(problem));
                };
                // With its shortcut on, Spotlight gets ⌘Space whatever is
                // registered.
                if is_cmd_space(&key)
                    && blocking(mac::spotlight_takes_cmd_space).await.unwrap_or(false)
                {
                    return report(format!(
                        "{key} is held by Spotlight: turn its shortcut off in System Settings"
                    ));
                }
                if taken.await {
                    return;
                }
                // Another app has it (macOS 27's Spotlight keeps ⌘Space with
                // its shortcut off): share it, as Raycast does.
                let (pressed, mut presses) = tokio::sync::mpsc::unbounded_channel();
                let shared = Shortcut::parse(&key, true)
                    .ok()
                    .and_then(|shortcut| SharedHotKey::register(&shortcut, pressed));
                let hot_key = match shared {
                    Some((hot_key, registered)) => registered.await.unwrap_or(false).then_some(hot_key),
                    None => None,
                };
                // Kept until the task ends.
                let Some(_hot_key) = hot_key else {
                    return report(format!("{key} is held by another app"));
                };
                log::info!("{key} is shared with another app");
                while presses.recv().await.is_some() {
                    launcher.run(action);
                }
            });
        }
    });

    // The tray, with what went wrong with the settings above its items.
    effect(move || {
        let mut problems = launcher.settings.with(|s| s.problems.clone());
        problems.extend(held.get());
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
                .icon(icons::svg_uri(icons::TRAY_SVG))
                .tooltip("Tinycast")
                .menu(items),
        );
    });

    view()
}

/// Whether `key` is ⌘Space, Spotlight's shortcut, however it is written.
fn is_cmd_space(key: &str) -> bool {
    Shortcut::parse(key, true).is_ok_and(|s| {
        s.key == Key::Space && s.meta && !s.alt && !s.ctrl && !s.shift
    })
}

/// The display the mouse pointer is on, where the popup opens.
fn mouse_display() -> Option<DisplayId> {
    let displays = displays();
    let primary = displays.iter().find(|d| d.primary)?;
    let (x, y) = mac::mouse_location();
    // Cocoa's `y` goes up from the primary display's bottom, nsh's down
    // from its top. The pointer can sit on a display's far edge, so the
    // edges count.
    let (x, y) = (px(x as f32), px(primary.bounds.size.height.0 - y as f32));
    displays
        .iter()
        .find(|d| {
            let b = &d.bounds;
            b.left() <= x && x <= b.right() && b.top() <= y && y <= b.bottom()
        })
        .map(|d| d.id)
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
        // The edge is all that outlines the popup: the window has no shadow.
        .app_class("border border-foreground/10 bg-popover/55 text-popover-foreground")
        .child(dynamic(move || match launcher.mode.get() {
            Mode::Apps => app_palette(launcher).into_element(),
            Mode::Clipboard => clipboard_view::clipboard_view(launcher).into_element(),
        }))
}

const CLIPBOARD_COMMAND: &str = "cmd:clipboard";
const WINDOW_PREFIX: &str = "window:";

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
        launcher.settings.with(|s| {
            items.extend(window::Command::ALL.map(|command| {
                let item = CommandItem::new(format!("{WINDOW_PREFIX}{}", command.id()), command.title())
                    .group("Window Management")
                    .keywords(["window", "窗口"]);
                match s.config.window.get(&command) {
                    Some(keys) => item.shortcut(keys.clone()),
                    None => item,
                }
            }));
        });
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
                launcher.launch(path);
            } else if let Some(command) = id.strip_prefix(WINDOW_PREFIX).and_then(window::Command::from_id) {
                launcher.close();
                window::run(command);
            }
        })
        .on_cancel(move || launcher.close())
        .empty(text("No matches").app_class("py-6 text-center text-sm text-muted-foreground"))
        .render_item(move |entry: &CommandEntry| {
            // The window commands' rows show their layout, the other
            // commands' rows (no `app:` id) the clipboard glyph.
            let path = entry.id.strip_prefix("app:").map(str::to_owned);
            let layout_icon = entry
                .id
                .strip_prefix(WINDOW_PREFIX)
                .and_then(window::Command::from_id)
                .map(|command| icons::svg_uri(&command.icon_svg()));
            let running = {
                let path = path.clone();
                move || path.as_ref().is_some_and(|p| launcher.running.with(|r| r.contains(p)))
            };
            let src = reactive_src(move || {
                if layout_icon.is_some() {
                    return layout_icon.clone();
                }
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
