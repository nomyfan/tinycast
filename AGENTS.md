# AGENTS.md

This file provides guidance to coding agents working with code in this repository.

Tinycast is a Raycast-style app launcher, clipboard history and script runner for macOS only, written in Rust on
[Native Shell (nsh)](https://nsh.dev) 0.6. It lives in the menu bar; global shortcuts open a popup with the
launcher (⌥Space by default) or the clipboard history (⌥⇧V).

## Commands

```sh
nsh run                       # build and run with logs; Rust edits rebuild and restart the app
cargo build                   # compile only (fast feedback)
cargo test --lib              # unit tests (src/config.rs has them)
cargo test --lib config::tests::cmd_space_is_a_keystroke   # one test
nsh package --target macos    # release build: .nsh/out/Tinycast.app, signed ad hoc
scripts/build.sh              # the same, re-signed with a Keychain certificate (MACOS_SIGNING_IDENTITY overrides)
```

- Use `nsh run`, not `cargo run`: only `nsh` applies `nsh.json` (main window hidden, `activationPolicy: accessory`
  so no Dock icon). `src/main.rs` is the template's `cargo run` entry and ignores it.
- `rust-toolchain.toml` pins Rust 1.100.0: the nsh 0.6 crates need rustc ≥ 1.97.
- `nsh package` signs macOS apps ad hoc only (`--sign` is iOS-only), and an ad-hoc signature loses the
  Accessibility permission pasting needs at every rebuild. `scripts/build.sh` re-signs with a Developer ID
  Application or Apple Development certificate (hardened runtime), picked as `../gatelock` picks it.
- Running the app (or packaging with `--run`) opens windows and registers global shortcuts on the user's screen:
  ask before doing it, and don't post keystrokes or overwrite the user's clipboard to test.

## Architecture

- **The hidden main window is the app's host.** `App` in `src/lib.rs` never shows; it owns the state (`Launcher`,
  a `Copy` struct of signals: apps, running apps, clipboard history, settings, mode), the global shortcuts, the
  tray menu, the clipboard watch and the config watch.
- **The popup** is `open_window(window().kind(WindowKind::Popup))` with `activate: false`, which the macOS backend
  makes an `NSPanel` with `NonactivatingPanel`: it takes the keyboard while the app the user was in stays
  frontmost. That is what lets "paste" close the popup and post ⌘V into that app (`mac::post_paste`, which needs
  Accessibility access). Its content switches between the launcher list and `clipboard_view` with
  `dynamic(move || match launcher.mode.get() …)`; `nested` records whether the clipboard view was entered from the
  launcher (Escape/Backspace go back rather than close).
- **Lists are nsh_base's `Command`** (filtering, highlight, keyboard), styled by `styled()`/`palette()` in
  `src/lib.rs`. The clipboard view's ⌘K actions panel is a second `Command` with pages (`Page` in
  `src/clipboard_view.rs`); destructive confirmation is a page of it, not a dialog.
- **`src/mac.rs` holds every AppKit/CoreGraphics call nsh has no API for** (objc2 0.6 / objc2-app-kit 0.3, the
  versions nsh-platform-macos uses): pasteboard change count, text and images, concealed-type check, frontmost and
  running apps, localized app names, icons as PNG, launching apps (`open_path` blocks the main thread until the app
  has started), posting ⌘V, moving other apps' windows (`AxWindow`), Spotlight's ⌘Space setting, global shortcuts
  shared with another app (`src/mac/shared_hotkey.rs`).
- **Clipboard history** (`src/clipboard.rs`): macOS has no clipboard-changed event, so `watch` polls
  `NSPasteboard.changeCount` every 400 ms. Text goes in `history.json`; images are PNGs named by content hash in
  `images/`; unpinned entries past 200 or deleted take their image files with them. Pinned entries (⇧⌘P) list
  first and are spared by the limit and by "Delete Entries…"/"Delete All Entries…".
- **Window management** (`src/window.rs`), Raycast's window commands: launcher rows (`window:<id>`) and optional
  global shortcuts (config `[window]`). The layouts are pure functions over nsh's `displays()` `visible_bounds`
  (top-left origin, as the Accessibility API's). `mac::AxWindow` moves the app in front's focused window through
  `AXUIElement` on a `blocking` thread, since an app that doesn't answer stalls each call. Restore remembers the frame
  from before the first command in a row, per window. A window just moved to another display is still held to the old
  display's size for about 60 ms, so `place` keeps setting the frame until it takes. When checking display areas from
  a script, note that `NSScreen.visibleFrame` in a process with no GUI connection (plain `osascript`) leaves out the
  menu bars of displays other than the main one; call `NSApplication.sharedApplication` first.
- **Script commands** (`src/scripts.rs`): the executable files of `~/.config/tinycast/scripts/` (made with a README
  on first launch) are launcher rows (`script:<file name>`), named by `# @name` / `@icon` / `@mode` comments at their
  top. The folder is listed again each time the popup opens. `scripts::start` runs one in its own process group,
  stdout and stderr on one pipe, read with `poll` so a background `foo &` holding the pipe cannot hang it; dropping
  the `Run` kills the group. `silent` scripts run under the root's owner (`Launcher::owner`) and end in the HUD
  (`src/hud.rs`, a non-focusable `Popup` window reused while shown: `open_window` with the id of an open window only
  focuses it); `output` scripts show in `src/script_view.rs` (`Mode::Output`), which stops them when it goes.
  `WindowKind::Overlay` would suit the HUD better, but it needs `capabilities` in `nsh.json`, and declaring any
  revokes every capability not listed.
- **Settings** (`src/config.rs`): `~/.config/tinycast/config.toml` (written with defaults on first launch), polled
  for changes every second, with `~/.config/tinycast/.env` (`src/dotenv.rs`): the variables scripts run with, over a
  `PATH` of `/usr/local/bin:$HOME/.local/bin:` and Tinycast's own. Shortcuts are registered in an `effect` whose
  `cleanup` releases the previous ones on reload. Problems (bad TOML, bad keystroke, held shortcut, bad `.env` line)
  become disabled "⚠︎" items in the tray menu, and the setting keeps its default.

Icons: `assets/icon/` (see its README). The tray icon is `tray-icon.svg`, embedded by `src/icons.rs` as a template;
the app icon is `app-icon.png`, named by `nsh.json`'s `icon`. `assets/icon/generate.mjs` regenerates the SVGs.

Data: `~/Library/Application Support/Tinycast/` (`history.json`, `images/`), icon cache in
`~/Library/Caches/Tinycast/icons/`.

## nsh pitfalls found here

- `WindowRef::focus()` right after `open_window()` is dropped (the native window is made at the commit); defer it
  with `request_animation_frame`. `auto_focus` can miss for the same reason, so the popup also focuses the search
  field when the window gains focus (`Launcher::focus_search`).
- With a non-activating panel key, `NSApp.isActive` reports `true` although the other app stays frontmost; check
  `NSWorkspace.frontmostApplication` instead.
- Tasks, timers and key bindings belong to the owner they were made under, and the popup's owner goes when it
  closes: work that must outlive it (the delayed ⌘V) runs on a `blocking` thread.
- Native alert dialogs take the focus, and the popup closes on blur (`close_on_blur`), cancelling what awaited
  them. Keep confirmations inside the popup.
- The popup's `close_on_escape` is off so `Command`'s `on_cancel` decides what Escape does.
- `register_global_shortcut` panics on an invalid keystroke: validate with `nsh_core::desktop::Shortcut::parse`.
  With Spotlight's ⌘Space setting on, Spotlight gets the key whatever is registered, so
  `mac::spotlight_takes_cmd_space` checks that separately. `register_global_shortcut` always asks for an exclusive
  Carbon hot key, which another app's exclusive registration of the keystroke makes answer `false`; macOS 27's
  Spotlight (the Siri AI app, relaunched at once when killed) registers ⌘Space even with that setting off. Tinycast
  then registers it shared, as Raycast does (`mac::shared_hotkey`, Carbon on the main thread, presses sent back to
  the engine thread).
- Conditional key bindings: call `on_key_with` inside a `when(..)` branch; the binding goes when the branch does
  (Backspace-in-empty-field goes back). Rust's `when` takes a `bool` condition.
- `Command` always puts its search bar first; the actions panel flips it with `flex-col-reverse` and renders its
  heading outside the `Command`.
- The popup is frosted glass (`macos_backdrop` with `BehindWindow`, state `Active` since the app is never active,
  over `bg-popover/55`). Opaque theme colors (`accent`, `muted`, `border`) look like white slabs in light mode on
  it: use translucent foreground shades (`bg-foreground/10`, `border-foreground/10`).

## References

- nsh docs: <https://nsh.dev/llms.txt> (every page is also served as Markdown at its URL plus `.md`).
- The nsh crates' source in `~/.cargo/registry/src/*/nsh-{core,base,platform-macos}-0.6.1` is the most precise
  reference for the Rust API and macOS behavior.
- Project skills (`.agents/skills.toml`, linked from `.claude/skills`): `nsh` and `nsh-design-guides`.
