//! The user's settings: `~/.config/tinycast/config.toml`, read at launch
//! and again whenever it is saved.

use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    time::{Duration, SystemTime},
};

use nsh_core::{desktop::Shortcut, prelude::*};
use serde::Deserialize;

use crate::{dotenv, window};

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

[window]
# Global shortcuts of the window commands, none by default. The commands:
# left-half, right-half, top-half, bottom-half, top-left-quarter,
# top-right-quarter, bottom-left-quarter, bottom-right-quarter, first-third,
# center-third, last-third, first-two-thirds, last-two-thirds, maximize,
# almost-maximize, center, reasonable-size, restore, next-display,
# previous-display, toggle-fullscreen.
#
# left-half = "ctrl+alt+left"
# right-half = "ctrl+alt+right"
# maximize = "ctrl+alt+enter"
"#;

/// What a new `.env` says: nothing but how to use it.
const ENV_TEMPLATE: &str = r#"# Variables the scripts in ./scripts/ run with. Saved changes apply to the
# next run.
#
# Tinycast starts scripts with /usr/local/bin and ~/.local/bin in front of
# the PATH it was given, which has little more than /usr/bin and /bin. Add
# where your package managers put their commands, in the order you want:
#
# PATH=/opt/homebrew/bin:/opt/homebrew/sbin:$PATH
# PATH=$HOME/.nix-profile/bin:/etc/profiles/per-user/$USER/bin:/run/current-system/sw/bin:/nix/var/nix/profiles/default/bin:$PATH
#
# $NAME and ${NAME} take the value of an earlier line or of Tinycast's own
# environment; values in single quotes are kept as they are.
"#;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub hotkeys: Hotkeys,
    /// The window commands' global shortcuts.
    pub window: BTreeMap<window::Command, String>,
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
        Config { hotkeys: Hotkeys::default(), window: BTreeMap::new() }
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
    /// The variables scripts run with, over Tinycast's own: always `PATH`,
    /// then the `.env` file's.
    pub env: Vec<(String, String)>,
    pub problems: Vec<String>,
}

/// The folder of the config and the scripts.
pub fn dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/tinycast")
}

pub fn path() -> PathBuf {
    dir().join("config.toml")
}

pub fn env_path() -> PathBuf {
    dir().join(".env")
}

/// The config file's settings and the `.env` file's variables; writes each
/// file first when there is none, so there is one to edit.
pub fn load() -> Loaded {
    let mut loaded = match read_or_create(&path(), TEMPLATE) {
        Ok(text) => parse(&text),
        Err(error) => Loaded {
            problems: vec![format!("Could not read the config: {error}")],
            ..Loaded::default()
        },
    };
    let path = script_path();
    let text = read_or_create(&env_path(), ENV_TEMPLATE).unwrap_or_else(|error| {
        loaded.problems.push(format!("Could not read .env: {error}"));
        String::new()
    });
    let (vars, problems) =
        dotenv::parse(&text, |name| if name == "PATH" { Some(path.clone()) } else { std::env::var(name).ok() });
    loaded.env = vec![("PATH".to_owned(), path)];
    loaded.env.extend(vars);
    loaded.problems.extend(problems);
    loaded
}

/// The text of `path`, written with `template` first when there is none.
fn read_or_create(path: &std::path::Path, template: &str) -> std::io::Result<String> {
    match std::fs::read_to_string(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let written = std::fs::create_dir_all(dir()).and_then(|()| std::fs::write(path, template));
            if let Err(error) = written {
                log::warn!("could not write {}: {error}", path.display());
            }
            Ok(template.to_owned())
        }
        read => read,
    }
}

/// Scripts' `PATH` before the `.env` file's: Tinycast's own, which an app
/// opened from the Finder gets short, after the folders where commands
/// installed by hand go. Absolute: `exec` does not expand `~`.
fn script_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let inherited = std::env::var("PATH").unwrap_or_default();
    format!("/usr/local/bin:{home}/.local/bin:{inherited}")
}

fn parse(text: &str) -> Loaded {
    let mut config: Config = match toml::from_str(text) {
        Ok(config) => config,
        Err(error) => {
            let message = error.message().to_owned();
            return Loaded {
                problems: vec![format!("The config is not valid TOML: {message}")],
                ..Loaded::default()
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
    let mut taken: HashSet<Shortcut> = [&config.hotkeys.launcher, &config.hotkeys.clipboard]
        .into_iter()
        .filter_map(|keys| Shortcut::parse(keys, true).ok())
        .collect();
    config.window.retain(|command, keys| {
        let name = command.id();
        match Shortcut::parse(keys, true) {
            Err(error) => problems.push(format!("window.{name} \"{keys}\" is not a keystroke ({error})")),
            Ok(shortcut) if !taken.insert(shortcut) => {
                problems.push(format!("window.{name} \"{keys}\" is already another shortcut"))
            }
            Ok(_) => return true,
        }
        false
    });
    Loaded { config, env: Vec::new(), problems }
}

/// Reloads `loaded` whenever the config or the `.env` file is saved, while
/// the calling owner lives.
pub fn watch(loaded: Signal<Loaded>) {
    let modified = |path: PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let both = move || -> [Option<SystemTime>; 2] { [modified(path()), modified(env_path())] };
    let seen = std::cell::Cell::new(both());
    set_interval(Duration::from_secs(1), move || {
        let now = both();
        if now != seen.get() {
            seen.set(now);
            let reloaded = load();
            log::info!("reloaded {} and .env", path().display());
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
    fn the_template_names_every_window_command() {
        for command in window::Command::ALL {
            assert!(TEMPLATE.contains(command.id()), "{}", command.id());
        }
    }

    #[test]
    fn window_shortcuts_are_read() {
        let loaded = parse("[window]\nleft-half = \"ctrl+alt+left\"\n");
        assert_eq!(loaded.config.window[&window::Command::LeftHalf], "ctrl+alt+left");
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn a_window_shortcut_taken_twice_is_dropped() {
        let loaded = parse(
            "[window]\nleft-half = \"alt+space\"\nright-half = \"ctrl+alt+l\"\n\
             maximize = \"ctrl+alt+L\"\n",
        );
        assert_eq!(loaded.config.window.len(), 1);
        assert_eq!(loaded.problems.len(), 2);
    }

    #[test]
    fn an_unknown_window_command_is_reported() {
        assert_eq!(parse("[window]\nleft-halve = \"ctrl+alt+left\"\n").problems.len(), 1);
    }

    #[test]
    fn unknown_settings_are_reported() {
        assert_eq!(parse("[hotkey]\nlauncher = \"cmd+space\"\n").problems.len(), 1);
    }
}
