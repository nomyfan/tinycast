//! The clipboard history: what was copied, newest first, kept on disk.
//! Text is kept in the history's file; images as PNGs beside it.

use std::{
    cell::Cell,
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    time::Duration,
};

use nsh_core::prelude::*;
use serde::{Deserialize, Serialize};

use crate::mac;

/// How many entries the history keeps.
const LIMIT: usize = 200;
/// How often the pasteboard is checked for a change.
const POLL: Duration = Duration::from_millis(400);
/// Larger images are left out of the history.
const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Something copied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipEntry {
    /// Unique in the history; the newest is the largest.
    pub id: u64,
    /// The text copied; empty for an image.
    pub text: String,
    /// The image copied, when it is one.
    #[serde(default)]
    pub image: Option<ClipImage>,
    /// The app in front when it was copied.
    pub source: Option<String>,
    /// Where that app is, for its icon.
    #[serde(default)]
    pub source_path: Option<String>,
    /// When it was copied, in Unix milliseconds.
    pub copied_at: u64,
}

/// An image in the history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipImage {
    /// Its PNG file.
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// The PNG's size in bytes.
    pub bytes: u64,
    /// The PNG's hash: the same image copied again is the same entry.
    pub hash: u64,
}

impl ClipEntry {
    /// What its row says.
    pub fn label(&self) -> String {
        match &self.image {
            Some(image) => format!("Image ({}×{})", image.width, image.height),
            None => {
                let line = self.text.trim().lines().next().unwrap_or_default().trim();
                let mut short: String = line.chars().take(80).collect();
                if short.len() < line.len() {
                    short.push('…');
                }
                short
            }
        }
    }

    fn same_content(&self, other: &ClipEntry) -> bool {
        match (&self.image, &other.image) {
            (Some(a), Some(b)) => a.hash == b.hash,
            (None, None) => self.text == other.text,
            _ => false,
        }
    }
}

fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("Library/Application Support/Tinycast")
}

fn history_file() -> PathBuf {
    data_dir().join("history.json")
}

fn images_dir() -> PathBuf {
    data_dir().join("images")
}

/// The history saved last time, empty when there is none.
pub fn load() -> Vec<ClipEntry> {
    std::fs::read(history_file())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(entries: Vec<ClipEntry>) {
    blocking(move || {
        let path = history_file();
        let result = std::fs::create_dir_all(data_dir())
            .and_then(|()| std::fs::write(&path, serde_json::to_vec(&entries)?));
        if let Err(error) = result {
            log::warn!("could not save the clipboard history: {error}");
        }
    });
}

/// Watches the pasteboard while the calling owner lives, putting each new
/// text or image at the top of `history` (moving it there when it was
/// copied before) and saving the history.
pub fn watch(history: Signal<Vec<ClipEntry>>) {
    let seen = Cell::new(-1_isize);
    set_interval(POLL, move || {
        let count = mac::change_count();
        if count == seen.replace(count) || mac::is_skipped() {
            return;
        }
        let (source, source_path) = mac::frontmost_app().map_or((None, None), |a| (a.name, a.path));
        if let Some(path) = source_path.clone() {
            blocking(move || crate::icons::ensure(&path));
        }
        let entry = move |text: String, image: Option<ClipImage>| ClipEntry {
            id: 0,
            text,
            image,
            source,
            source_path,
            copied_at: nsh_core::time::unix_ms() as u64,
        };
        if let Some(text) = mac::read_text() {
            add(history, entry(text, None));
            return;
        }
        // Converting and writing a large image takes a moment: off the UI's
        // thread.
        spawn(async move {
            if let Ok(Some(image)) = blocking(read_image).await {
                add(history, entry(String::new(), Some(image)));
            }
        });
    });
}

/// The pasteboard's image, written to the images folder.
fn read_image() -> Option<ClipImage> {
    let image = mac::read_image()?;
    if image.png.len() > MAX_IMAGE_BYTES {
        log::info!("an image of {} bytes is left out of the history", image.png.len());
        return None;
    }
    let mut hasher = DefaultHasher::new();
    image.png.hash(&mut hasher);
    let hash = hasher.finish();
    let file = images_dir().join(format!("{hash:016x}.png"));
    if !file.exists() {
        std::fs::create_dir_all(images_dir()).ok()?;
        std::fs::write(&file, &image.png).ok()?;
    }
    Some(ClipImage {
        file: file.to_str()?.to_owned(),
        width: image.width,
        height: image.height,
        bytes: image.png.len() as u64,
        hash,
    })
}

/// Puts `entry` at the top of `history`, in place of an entry with the
/// same content, and drops what goes past the limit.
fn add(history: Signal<Vec<ClipEntry>>, mut entry: ClipEntry) {
    let mut dropped = Vec::new();
    history.update(|entries| {
        if entries.first().is_some_and(|e| e.same_content(&entry)) {
            return;
        }
        entries.retain(|e| !e.same_content(&entry));
        entry.id = entries.iter().map(|e| e.id).max().map_or(1, |id| id + 1);
        entries.insert(0, entry);
        if entries.len() > LIMIT {
            dropped = entries.split_off(LIMIT);
        }
    });
    remove_images(dropped);
    save(history.get_untracked());
}

fn remove_images(entries: Vec<ClipEntry>) {
    let files: Vec<String> = entries.into_iter().filter_map(|e| e.image).map(|i| i.file).collect();
    if !files.is_empty() {
        blocking(move || {
            for file in files {
                let _ = std::fs::remove_file(file);
            }
        });
    }
}

/// Puts `entry` back on the clipboard; `false` when its image file is gone.
pub fn put(entry: &ClipEntry) -> bool {
    match &entry.image {
        Some(image) => mac::write_image(&image.file),
        None => {
            mac::write_text(&entry.text);
            true
        }
    }
}

/// Removes the entries `doomed` picks, on screen and on disk.
pub fn remove(history: Signal<Vec<ClipEntry>>, doomed: impl Fn(&ClipEntry) -> bool) {
    let mut removed = Vec::new();
    history.update(|entries| {
        let (gone, kept) = std::mem::take(entries).into_iter().partition(|e| doomed(e));
        removed = gone;
        *entries = kept;
    });
    remove_images(removed);
    save(history.get_untracked());
}

/// Removes every entry, on screen and on disk.
pub fn clear(history: Signal<Vec<ClipEntry>>) {
    history.set(Vec::new());
    save(Vec::new());
    blocking(|| {
        let _ = std::fs::remove_dir_all(images_dir());
    });
}
