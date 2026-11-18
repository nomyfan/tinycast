//! The user's settings: `~/.config/tinycast/config.toml`, read at launch
//! and again whenever it is saved.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use nsh_core::{desktop::Shortcut, prelude::*};
use serde::Deserialize;

/// What a new config file says: every setting, at its default.
const TEMPLATE: &str = r#"# Tinycast settings. Saved changes apply at once.

[hotkeys]
# Keystrokes are modifiers and a key joined by "+": cmd (command), alt
# (option), ctrl, shift, and a letter, a digit, "space", "f1", ...
#
# To use "cmd+space", first turn off Spotlight's shortcut: System Settings ›
# Keyboard › Keyboard Shortcuts › Spotlight › "Show Spotlight search".
launcher = "alt+space"
clipboard = "alt+shift+v"
"#;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub hotkeys: Hotkeys,
}

/// The global shortcuts, as `register_global_shortcut` takes them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Hotkeys {
    /// Opens the launcher.
    pub launcher: String,
    /// Opens the clipboard history.
    pub clipboard: String,
}

impl Default for Config {
    fn default() -> Self {
        Config { hotkeys: Hotkeys::default() }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys { launcher: "alt+space".into(), clipboard: "alt+shift+v".into() }
    }
}

/// The config, and what was wrong with the file (each wrong setting is
/// at its default).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Loaded {
    pub config: Config,
    pub problems: Vec<String>,
}

pub fn path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/tinycast/config.toml")
}

/// The config file's settings; writes the file with the defaults first
/// when there is none, so there is one to edit.
pub fn load() -> Loaded {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let written = std::fs::create_dir_all(path.parent().unwrap())
                .and_then(|()| std::fs::write(&path, TEMPLATE));
            if let Err(error) = written {
                log::warn!("could not write {}: {error}", path.display());
            }
            TEMPLATE.to_owned()
        }
        Err(error) => {
            return Loaded {
                config: Config::default(),
                problems: vec![format!("Could not read the config: {error}")],
            };
        }
    };
    parse(&text)
}

fn parse(text: &str) -> Loaded {
    let mut config: Config = match toml::from_str(text) {
        Ok(config) => config,
        Err(error) => {
            let message = error.message().to_owned();
            return Loaded {
                config: Config::default(),
                problems: vec![format!("The config is not valid TOML: {message}")],
            };
        }
    };
    let mut problems = Vec::new();
    let defaults = Hotkeys::default();
    for (name, keys, default) in [
        ("launcher", &mut config.hotkeys.launcher, defaults.launcher.clone()),
        ("clipboard", &mut config.hotkeys.clipboard, defaults.clipboard.clone()),
    ] {
        // `register_global_shortcut` panics on a keystroke that is not one.
        if let Err(error) = Shortcut::parse(keys, true) {
            problems.push(format!("hotkeys.{name} \"{keys}\" is not a keystroke ({error})"));
            *keys = default;
        }
    }
    if config.hotkeys.launcher == config.hotkeys.clipboard {
        problems.push("hotkeys.launcher and hotkeys.clipboard are the same keystroke".into());
        config.hotkeys.clipboard = defaults.clipboard;
    }
    Loaded { config, problems }
}

/// Reloads `loaded` whenever the file is saved, while the calling owner
/// lives.
pub fn watch(loaded: Signal<Loaded>) {
    let modified = || std::fs::metadata(path()).and_then(|m| m.modified()).ok();
    let seen = std::cell::Cell::new(modified());
    set_interval(Duration::from_secs(1), move || {
        let now: Option<SystemTime> = modified();
        if now != seen.get() {
            seen.set(now);
            let reloaded = load();
            log::info!("reloaded {}", path().display());
            loaded.set(reloaded);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_the_defaults() {
        assert_eq!(parse(TEMPLATE), Loaded::default());
    }

    #[test]
    fn a_wrong_keystroke_falls_back_to_its_default() {
        let loaded = parse("[hotkeys]\nlauncher = \"cmd+spaec\"\n");
        assert_eq!(loaded.config.hotkeys.launcher, "alt+space");
        assert_eq!(loaded.problems.len(), 1);
    }

    #[test]
    fn cmd_space_is_a_keystroke() {
        let loaded = parse("[hotkeys]\nlauncher = \"cmd+space\"\n");
        assert_eq!(loaded.config.hotkeys.launcher, "cmd+space");
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn unknown_settings_are_reported() {
        assert_eq!(parse("[hotkey]\nlauncher = \"cmd+space\"\n").problems.len(), 1);
    }
}
