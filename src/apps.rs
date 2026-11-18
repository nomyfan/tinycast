//! The installed applications.

use std::path::Path;

/// An application bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct AppEntry {
    /// Where the `.app` is.
    pub path: String,
    /// Its name as Finder shows it, in the user's language.
    pub name: String,
    /// Its bundle's file name without `.app`, matched as well, so the
    /// English name finds a localized app.
    pub file_name: String,
    /// Its icon's PNG, once [`with_icons`] made it.
    pub icon: Option<String>,
}

/// The `.app` bundles in the usual folders and the folders inside them
/// (`/Applications/Utilities`, a vendor's folder), sorted by name.
pub fn scan() -> Vec<AppEntry> {
    let home = std::env::var("HOME").unwrap_or_default();
    let roots = [
        "/Applications".to_owned(),
        "/System/Applications".to_owned(),
        "/System/Library/CoreServices/Applications".to_owned(),
        format!("{home}/Applications"),
    ];
    let mut apps = Vec::new();
    for root in &roots {
        collect(Path::new(root), 2, &mut apps);
    }
    // Finder is not in any of them.
    collect_app(Path::new("/System/Library/CoreServices/Finder.app"), &mut apps);
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.path == b.path);
    apps
}

/// `apps` with their icons, made the first time and cached after.
pub fn with_icons(mut apps: Vec<AppEntry>) -> Vec<AppEntry> {
    for app in &mut apps {
        app.icon = crate::icons::ensure(&app.path);
    }
    apps
}

fn collect(dir: &Path, depth: u32, apps: &mut Vec<AppEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Hidden ones (`.Karabiner-VirtualHIDDevice-Manager.app`) are not
        // meant to be opened by hand.
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "app") {
            collect_app(&path, apps);
        } else if depth > 1 && entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect(&path, depth - 1, apps);
        }
    }
}

fn collect_app(path: &Path, apps: &mut Vec<AppEntry>) {
    let Some(path_str) = path.to_str() else {
        return;
    };
    let file_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned();
    apps.push(AppEntry {
        name: crate::mac::display_name(path_str),
        path: path_str.to_owned(),
        file_name,
        icon: None,
    });
}
