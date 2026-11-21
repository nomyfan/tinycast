//! Tinycast: an app launcher and clipboard history for macOS.
//!
//! The app lives in the menu bar with its main window hidden; global
//! shortcuts open a non-activating popup, so the app the user is in keeps
//! the focus and gets it back, with a paste, when the popup closes.

mod apps;
mod clipboard;
mod clipboard_view;
mod config;
mod dotenv;
mod hud;
mod icons;
mod mac;
mod recent;
mod script_view;
mod scripts;
mod search;
mod window;

use std::{collections::HashSet, time::Duration};

use nsh_base::{Command, CommandApi, CommandEntry, CommandItem, button, command, theme};
use nsh_core::{
    MacOsBackdrop, MacOsBackdropBlending, MacOsBackdropMaterial, MacOsBackdropState,
    desktop::{Display, Key, Shortcut, WindowAttributes, WindowKind, WindowPosition},
    element::ImageSrc,
    element::desktop::{
        MenuItem, WindowRef, current_window, displays, open_window, quit, register_global_shortcut,
        set_tray, tray, window,
    },
    prelude::*,
};

use crate::{
    apps::AppEntry,
    clipboard::ClipEntry,
    mac::shared_hotkey::SharedHotKey,
    scripts::{Outcome, OutputMode, Script, ScriptIcon},
};

/// The app's UI set (`nsh.json` `"rust"`).
#[native_module("tinycast/native")]
fn native(registry: &mut Registry) {
    registry.register::<App>();
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Apps,
    Clipboard,
    /// The output of `Launcher::script`, as it runs.
    Output,
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
    /// When each launcher item was chosen lately: they list first, and a
    /// search ranks them higher.
    recent: Signal<recent::Recent>,
    /// The user's settings (`config.toml`, `.env`).
    settings: Signal<config::Loaded>,
    /// The scripts folder's, as of the popup's opening.
    scripts: Signal<Vec<Script>>,
    /// The script `Mode::Output` runs.
    script: Stored<Option<Script>>,
    pub(crate) mode: Signal<Mode>,
    /// The clipboard or output view was entered from the launcher's list,
    /// so Escape goes back to it rather than closing the popup.
    pub(crate) nested: Stored<bool>,
    pub(crate) popup: Stored<Option<WindowRef>>,
    /// The palette the popup shows, whichever mode.
    pub(crate) search: ComponentRef<Command>,
    hud: hud::Hud,
    /// The app's root's: what must outlive the popup runs under it.
    owner: Owner,
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
        self.rescan_scripts();
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
            options = options.display(display.id);
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
            return log::warn!(
                "Tinycast needs Accessibility access to paste; the text is on the clipboard"
            );
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

    /// Lists the scripts folder again, off the engine thread.
    fn rescan_scripts(self) {
        self.owner.run(|| {
            spawn(async move {
                let Ok(found) = blocking(scripts::scan).await else {
                    return;
                };
                if self.scripts.with_untracked(|scripts| *scripts != found) {
                    self.scripts.set(found);
                }
            });
        });
    }

    /// Runs the script of launcher row `id`: in the output view, or with
    /// the popup closed and its last line of output in the HUD.
    fn run_script(self, id: &str) {
        let Some(script) = self
            .scripts
            .with_untracked(|s| s.iter().find(|s| s.id == id).cloned())
        else {
            return;
        };
        match script.mode {
            OutputMode::Output => {
                self.script.replace(Some(script));
                self.nested.replace(true);
                self.mode.set(Mode::Output);
            }
            OutputMode::Silent => {
                self.close();
                let env = self.settings.with_untracked(|s| s.env.clone());
                // Under the root: the popup's owner goes with the popup, and
                // the run with it.
                self.owner.run(|| {
                    spawn(async move {
                        let mut run = scripts::start(&script, &env);
                        let mut last_line = None;
                        while let Some(line) = run.line().await {
                            if !line.trim().is_empty() {
                                last_line = Some(line);
                            }
                        }
                        let outcome = run.outcome().await;
                        if let Some((message, failed)) =
                            hud_message(&script.name, last_line, outcome)
                        {
                            self.hud.show(&message, failed);
                        }
                    });
                });
            }
        }
    }
}

/// What the HUD says when a silent script ends, and whether it failed: its
/// last line of output, and why when it failed. Nothing for a script that
/// succeeded quietly (one that opens a window has nothing to add).
fn hud_message(name: &str, last_line: Option<String>, outcome: Outcome) -> Option<(String, bool)> {
    match (outcome, last_line) {
        (Outcome::Succeeded, Some(line)) => Some((line, false)),
        (Outcome::Succeeded, None) => None,
        (Outcome::Failed(why), Some(line)) => Some((format!("{why}: {line}"), true)),
        (Outcome::Failed(why), None) => Some((format!("{name}: {why}"), true)),
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
        recent: signal(recent::load()),
        settings: signal(config::load()),
        scripts: signal(Vec::new()),
        script: stored(None),
        mode: signal(Mode::Apps),
        nested: stored(false),
        popup: stored(None),
        search: component_ref(),
        hud: hud::Hud::new(),
        owner: Owner::current(),
    };
    launcher.rescan_scripts();

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
                (
                    s.config.hotkeys.clipboard.clone(),
                    Action::Show(Mode::Clipboard),
                ),
            ];
            hotkeys.extend(
                s.config
                    .window
                    .iter()
                    .map(|(&command, keys)| (keys.clone(), Action::Window(command))),
            );
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
                    && blocking(mac::spotlight_takes_cmd_space)
                        .await
                        .unwrap_or(false)
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
                    Some((hot_key, registered)) => {
                        registered.await.unwrap_or(false).then_some(hot_key)
                    }
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
            MenuItem::new("Open Scripts Folder…").on_select(|| {
                spawn(async move {
                    // Made again, README and all, if it was deleted since the
                    // launch.
                    if let Ok(Err(error)) = blocking(scripts::make_dir).await {
                        return log::warn!("could not make the scripts folder: {error}");
                    }
                    if let Err(error) = open_path(scripts::dir()).await {
                        log::warn!("could not open the scripts folder: {error}");
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
    Shortcut::parse(key, true)
        .is_ok_and(|s| s.key == Key::Space && s.meta && !s.alt && !s.ctrl && !s.shift)
}

/// The display the mouse pointer is on, where the popup and the HUD open.
pub(crate) fn mouse_display() -> Option<Display> {
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
        .cloned()
}

/// Frosted glass, as Raycast and Spotlight: the popover material blurs what
/// is behind the (transparent) window. Always active: Tinycast is never the
/// active app, and an inactive material is flat gray. The popup and the HUD
/// wear it, with `FROSTED_GLASS_CLASS`.
pub(crate) fn frosted_glass() -> MacOsBackdrop {
    let mut backdrop = MacOsBackdrop::default();
    backdrop.material = MacOsBackdropMaterial::Popover;
    backdrop.blending = MacOsBackdropBlending::BehindWindow;
    backdrop.state = MacOsBackdropState::Active;
    backdrop
}

/// Over `frosted_glass`: a thin wash of the theme's surface, which keeps
/// text legible on busy backgrounds, and an edge, all that outlines a
/// window without a shadow.
pub(crate) const FROSTED_GLASS_CLASS: &str =
    "border border-foreground/10 bg-popover/55 text-popover-foreground";

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
        .macos_backdrop(frosted_glass())
        // The edge is all that outlines the popup: the window has no shadow.
        .app_class(FROSTED_GLASS_CLASS)
        .child(dynamic(move || match launcher.mode.get() {
            Mode::Apps => app_palette(launcher).into_element(),
            Mode::Clipboard => clipboard_view::clipboard_view(launcher).into_element(),
            Mode::Output => script_view::script_view(launcher).into_element(),
        }))
}

const CLIPBOARD_COMMAND: &str = "cmd:clipboard";
const WINDOW_PREFIX: &str = "window:";
const SCRIPT_PREFIX: &str = "script:";

fn app_palette(launcher: Launcher) -> impl IntoElement {
    // The query is the app's: `Command` only matches substrings, so the
    // items come ranked by `search::rank` and it shows them unfiltered.
    let query = signal(SharedString::default());
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
                .shortcut(
                    launcher
                        .settings
                        .with(|s| s.config.hotkeys.clipboard.clone()),
                ),
        );
        launcher.settings.with(|s| {
            items.extend(window::Command::ALL.map(|command| {
                let item =
                    CommandItem::new(format!("{WINDOW_PREFIX}{}", command.id()), command.title())
                        .group("Window Management")
                        .keywords(["window", "窗口"]);
                match s.config.window.get(&command) {
                    Some(keys) => item.shortcut(keys.clone()),
                    None => item,
                }
            }));
        });
        launcher.scripts.with(|scripts| {
            items.extend(scripts.iter().map(|script| {
                let file_stem = script
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                CommandItem::new(script.id.clone(), script.name.clone())
                    .group("Scripts")
                    .keywords([file_stem, "script".into(), "脚本".into()])
            }));
        });
        let now = recent::now();
        query.with(|q| {
            launcher.recent.with(|recent| {
                if q.trim().is_empty() {
                    return recent::recent_first(items, recent, now);
                }
                search::rank(items, q, |id| {
                    recent.get(id).map_or(0, |times| recent::bonus(times, now))
                })
            })
        })
    };
    palette(launcher)
        .items(items)
        .filterable(false)
        .query(move || Some(query.get()))
        .on_query_change(move |q: &SharedString| query.set(q.clone()))
        .placeholder("Search apps, commands and scripts…")
        .on_select(move |id: &SharedString| {
            recent::record(launcher.recent, id);
            if id.as_ref() == CLIPBOARD_COMMAND {
                launcher.nested.replace(true);
                launcher.mode.set(Mode::Clipboard);
            } else if let Some(path) = id.strip_prefix("app:") {
                launcher.launch(path);
            } else if let Some(command) = id
                .strip_prefix(WINDOW_PREFIX)
                .and_then(window::Command::from_id)
            {
                launcher.close();
                window::run(command);
            } else if id.starts_with(SCRIPT_PREFIX) {
                launcher.run_script(id);
            }
        })
        .on_cancel(move || launcher.close())
        .empty(text("No matches").app_class("py-6 text-center text-sm text-muted-foreground"))
        .render_item(move |entry: &CommandEntry| {
            // The window commands' rows show their layout, the scripts' their
            // own icon, the other commands' rows (no `app:` id) the clipboard
            // glyph.
            let script_id = entry
                .id
                .starts_with(SCRIPT_PREFIX)
                .then(|| entry.id.to_string());
            let path = entry.id.strip_prefix("app:").map(str::to_owned);
            let layout_icon = entry
                .id
                .strip_prefix(WINDOW_PREFIX)
                .and_then(window::Command::from_id)
                .map(|command| icons::svg_uri(&command.icon_svg()));
            let running = {
                let path = path.clone();
                move || {
                    path.as_ref()
                        .is_some_and(|p| launcher.running.with(|r| r.contains(p)))
                }
            };
            let src = reactive_src(move || {
                if layout_icon.is_some() {
                    return layout_icon.clone();
                }
                let path = path.as_deref()?;
                launcher.apps.with(|apps| {
                    apps.iter()
                        .find(|a| a.path == path)
                        .and_then(|a| a.icon.clone())
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
                        .child(match script_id {
                            Some(id) => dynamic(move || {
                                let icon = launcher.scripts.with(|scripts| {
                                    scripts
                                        .iter()
                                        .find(|s| s.id == id)
                                        .map_or(ScriptIcon::Default, |s| s.icon.clone())
                                });
                                script_view::script_icon(icon)
                            })
                            .into_element(),
                            None => row_icon(src).into_element(),
                        })
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
pub(crate) fn reactive_src(
    icon: impl Fn() -> Option<String> + 'static,
) -> impl Fn() -> ImageSrc + 'static {
    move || ImageSrc::from(icon().unwrap_or_else(|| icons::svg_uri(icons::CLIPBOARD_SVG)))
}

pub(crate) fn row_icon(src: impl Fn() -> ImageSrc + 'static) -> impl IntoElement {
    image(src)
        .w(px(22.0))
        .h(px(22.0))
        .object_fit(ResizeMode::Contain)
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
                .tint_color(move || theme().foreground),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hud_says_the_last_line_and_why_it_failed_or_nothing() {
        let line = || Some("Dark mode on".to_owned());
        assert_eq!(
            hud_message("Dark", line(), Outcome::Succeeded),
            Some(("Dark mode on".into(), false))
        );
        assert_eq!(hud_message("Dark", None, Outcome::Succeeded), None);
        let failed = || Outcome::Failed("Exit code 1".into());
        assert_eq!(
            hud_message("Dark", line(), failed()),
            Some(("Exit code 1: Dark mode on".into(), true))
        );
        assert_eq!(
            hud_message("Dark", None, failed()),
            Some(("Dark: Exit code 1".into(), true))
        );
    }
}
