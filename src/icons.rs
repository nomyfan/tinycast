//! Icons of apps and files as PNGs in the user's cache folder, which the
//! `image` element loads by path.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
};

/// The icons' size in points: AppKit renders them at the main display's
/// scale (64 pixels on a Retina display).
const POINTS: f64 = 32.0;

/// A clipboard on an orange-to-pink tile, drawn as app icons are: for the
/// Clipboard History command and entries without an app of their own.
pub const CLIPBOARD_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'>\
    <defs><linearGradient id='bg' x1='0' y1='0' x2='0' y2='1'>\
    <stop offset='0' stop-color='#FF9A3D'/><stop offset='1' stop-color='#F23B63'/>\
    </linearGradient></defs>\
    <rect x='1' y='1' width='22' height='22' rx='5.5' fill='url(#bg)'/>\
    <rect x='6.25' y='5.75' width='11.5' height='13.5' rx='2' fill='#FFFFFF'/>\
    <rect x='9' y='4.25' width='6' height='3.25' rx='1.2' fill='#FFFFFF' stroke='#F5525A' stroke-width='1'/>\
    <path d='M8.75 11h6.5M8.75 13.5h6.5M8.75 16h4' stroke='#F5525A' stroke-width='1.25' stroke-linecap='round'/>\
    </svg>";

/// The menu bar icon, a tilted fishing bobber: a template, which macOS
/// draws in the menu bar's text color. Its source and the app icon's are in
/// `assets/icon/`.
pub const TRAY_SVG: &str = include_str!("../assets/icon/tray-icon.svg");

/// A left arrow, drawn in the color the image's tint gives it.
pub const BACK_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' \
    fill='none' stroke='#000' stroke-width='2.2' stroke-linecap='round' stroke-linejoin='round'>\
    <path d='M19 12H5M11 6l-6 6 6 6'/></svg>";

/// Two overlapping sheets, drawn in the color the image's tint gives it.
pub const COPY_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' \
    fill='none' stroke='#000' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'>\
    <rect x='9' y='9' width='11' height='11' rx='2'/>\
    <path d='M5 15V6a2 2 0 0 1 2-2h8'/></svg>";

/// A push pin with rounded corners, drawn in the color the image's tint
/// gives it.
pub const PIN_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' \
    fill='none' stroke='#000' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'>\
    <rect x='8' y='2.5' width='8' height='3' rx='1.5'/>\
    <path d='M10 5.5V8.6C10 9.3 9.7 9.9 9.2 10.4L7.4 12.2C6.6 13 7.1 14 8.1 14H15.9\
    C16.9 14 17.4 13 16.6 12.2L14.8 10.4C14.3 9.9 14 9.3 14 8.6V5.5M12 14v6.5'/></svg>";

/// A trash can, red as destructive actions are.
pub const TRASH_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' \
    fill='none' stroke='#E5484D' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'>\
    <path d='M4 7h16M10 11v6M14 11v6M5 7l1 12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2l1-12M9 7V4h6v3'/></svg>";

/// `svg` as a `data:` URI.
pub fn svg_uri(svg: &str) -> String {
    let mut uri = String::from("data:image/svg+xml,");
    for byte in svg.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'/' | b'=' | b':' => {
                uri.push(byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Caches/Tinycast/icons")
}

/// Where the icon of `path` is cached, whether or not it is yet.
pub fn cached_path(path: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    cache_dir().join(format!("{:016x}.png", hasher.finish()))
}

/// The cached icon of `path`, made first when it is missing or older than
/// the bundle (an app updated since). Blocks: call it off the UI's thread.
pub fn ensure(path: &str) -> Option<String> {
    let icon = cached_path(path);
    if !is_fresh(&icon, Path::new(path)) {
        let png = crate::mac::icon_png(path, POINTS)?;
        std::fs::create_dir_all(cache_dir()).ok()?;
        std::fs::write(&icon, png).ok()?;
    }
    icon.to_str().map(str::to_owned)
}

fn is_fresh(icon: &Path, source: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (modified(icon), modified(source)) {
        (Some(icon), Some(source)) => icon >= source,
        (Some(_), None) => true,
        _ => false,
    }
}
