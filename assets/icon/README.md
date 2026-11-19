# Tinycast icon

A fishing bobber on its line ("cast"): white over `#F86044`, tilted a little, on graphite. The menu bar shows the
bobber alone.

| File | What it is |
| --- | --- |
| `app-icon.svg` | The full app icon, 1024 × 1024 on the macOS icon grid (824 square at 100, radius 185). |
| `layers/1-background.svg` | Background only, for Icon Composer. |
| `layers/2-line.svg` | The line on a transparent canvas. |
| `layers/3-bobber.svg` | The bobber on a transparent canvas. |
| `app-icon.png` | `app-icon.svg` rendered at 1024 × 1024, transparent outside the square: `nsh.json`'s macOS `icon`. |
| `tray-icon.svg` | 18 × 18 pt menu-bar icon, black on transparent, meant as a template image (macOS tints it). |

Colors can be edited directly in the SVGs, but regenerating overwrites them. To change the colors or the shape
(the bobber's size, position and tilt, the line's width, the tray icon's size and tilt), edit the constants at
the top of `generate.mjs` and run:

```sh
node assets/icon/generate.mjs
```

It needs no dependencies and rewrites every SVG above. Then render the PNG again, for example with librsvg
(`brew install librsvg`):

```sh
rsvg-convert -w 1024 -h 1024 assets/icon/app-icon.svg -o assets/icon/app-icon.png
```

The app uses them as they are: `src/icons.rs` embeds `tray-icon.svg` (`include_str!`), so a rebuild picks up a
change; `nsh package` turns `app-icon.png` into `AppIcon.icns`, and `nsh run` shows it in the Dock.
