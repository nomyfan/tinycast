# Tinycast

A small Raycast-style app launcher and clipboard history for macOS, written in Rust with
[Native Shell (nsh)](https://nsh.dev).

Tinycast lives in the menu bar. Its popup is a non-activating panel: the app you are in keeps the focus, so a
clipboard entry can be pasted straight back into it.

## Features

- **Launcher** (⌥Space): search the installed apps by their localized or file name and open them. Running apps
  show a dot under their icon.
- **Clipboard history** (⌥⇧V, or "Clipboard History" in the launcher): text and images, newest first, with a
  preview and the app each entry was copied from. Return pastes the entry into the app you were in.
  - Actions (⌘K): Copy to Clipboard (⌘↩), Delete Entry (⌃X), Delete Entries… by time range (⇧⌘X), Delete All
    Entries… (⌃⇧X, asks first).
  - Backspace in an empty search field, or Escape, goes back to the launcher.
  - Copies marked as concealed or transient (password managers) are left out.

Pasting posts ⌘V to the app in front, which needs Accessibility access: System Settings › Privacy & Security ›
Accessibility. Without it, the entry is put on the clipboard for you to paste.

## Development

Needs the `nsh` CLI ([installation](https://nsh.dev/docs/installation)) and Rust; `rust-toolchain.toml` pins the
version NSH's crates need.

```sh
nsh run                       # build and run, with logs in the terminal; Rust edits rebuild and restart it
nsh package --target macos    # .nsh/out/Tinycast.app, signed ad hoc for this Mac
```

Use `nsh run` rather than `cargo run`: only `nsh` applies `nsh.json`, which keeps the main window hidden and the app
out of the Dock.

## Code

| File | What it holds |
|---|---|
| `src/lib.rs` | The root: shortcuts, tray, the popup window and the launcher's list |
| `src/clipboard_view.rs` | The clipboard history's view, footer and actions panel |
| `src/clipboard.rs` | Watching the pasteboard, and the history on disk |
| `src/apps.rs` | Finding the installed apps |
| `src/icons.rs` | App icons as cached PNGs, and the built-in SVG icons |
| `src/mac.rs` | The AppKit and CoreGraphics calls nsh has no API for |

macOS has no clipboard-changed event, so the history polls `NSPasteboard.changeCount` every 400 ms.

## Data

| Path | Contents |
|---|---|
| `~/Library/Application Support/Tinycast/history.json` | The clipboard history (up to 200 entries) |
| `~/Library/Application Support/Tinycast/images/` | Copied images, as PNGs |
| `~/Library/Caches/Tinycast/icons/` | App icons; safe to delete |
