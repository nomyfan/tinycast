//! The user's scripts in `~/.config/tinycast/scripts/`: each executable
//! file is a launcher command, named and set up by comments at its top.
//!
//! ```sh
//! #!/bin/bash
//! # @name Toggle Dark Mode
//! # @icon 🌙
//! # @mode silent
//! ```

use std::{
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{fs::PermissionsExt, process::CommandExt, process::ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::{mpsc, oneshot};

use crate::icons;

/// How much of a file is read for its header.
const HEADER_BYTES: u64 = 4096;
/// How long the output is still read once the script has exited: what it
/// started in the background (`foo &`) can hold the pipe open for ever.
const DRAIN: Duration = Duration::from_millis(200);
/// How often reading stops waiting to check the script and its `Run`.
const POLL_MS: i32 = 50;
/// The bytes of a line kept; the rest, to the line's end, is dropped.
const LINE_BYTES: usize = 64 * 1024;
/// The characters of a line shown.
const LINE_CHARS: usize = 1000;

const README: &str = r#"Scripts here show in Tinycast's launcher. Each must be executable
(chmod +x my-script.sh) and start with a #! line naming what runs it, as
scripts run from a terminal do (without one, /bin/sh runs it). Comments at
its top set how it shows and runs:

    #!/bin/bash
    # @name Toggle Dark Mode
    # @icon 🌙
    # @mode silent

- @name: the name in the launcher; the file's name without its extension
  when left out.
- @icon: an emoji; an image file (.png, .svg, .jpg); or any other path,
  whose Finder icon shows: an app (/Applications/Safari.app) or a System
  Settings pane (/System/Library/ExtensionKit/Extensions/DisplaysExt.appex).
  Paths are relative to this folder; a script icon when left out.
- @mode: "silent" (the default) closes the launcher and shows the last
  line of output for a moment (nothing when a script that succeeded
  printed nothing; a failure always shows); "output" shows the whole
  output in the launcher.

Comments start with #, // or --. Blank lines may come between them; the
first line of code ends the header.

Scripts run in this folder, with the variables of ../.env (PATH among
them: add Homebrew's or Nix's folders there). Leaving the output view
stops a script, and a silent script's background processes (foo &) stop
when it ends: start long-running programs with `open` or `launchctl`.
"#;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Script {
    /// `script:<file name>`: the launcher row's and the recent choices' id.
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) icon: ScriptIcon,
    pub(crate) mode: OutputMode,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ScriptIcon {
    Default,
    /// Text drawn as the icon, an emoji most often.
    Emoji(String),
    /// What an `image` loads: a file's path or a `data:` URI.
    Image(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum OutputMode {
    /// Closes the popup; the last line of output shows in the HUD.
    Silent,
    /// The whole output shows in the popup.
    Output,
}

pub(crate) fn dir() -> PathBuf {
    crate::config::dir().join("scripts")
}

/// Makes the scripts folder, with its README, when there is none. Blocks.
pub(crate) fn make_dir() -> std::io::Result<()> {
    make_dir_at(&dir())
}

fn make_dir_at(dir: &Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("README.md"), README)
}

/// The scripts in the folder, by name: its executable files, symbolic
/// links followed, hidden ones left out. Makes the folder, with a README,
/// when there is none. Blocks: reads the files.
pub(crate) fn scan() -> Vec<Script> {
    scan_in(&dir())
}

fn scan_in(dir: &Path) -> Vec<Script> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Err(error) = make_dir_at(dir) {
                log::warn!("could not make {}: {error}", dir.display());
            }
            return Vec::new();
        }
        Err(error) => {
            log::warn!("could not list {}: {error}", dir.display());
            return Vec::new();
        }
    };
    let mut scripts: Vec<Script> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let file_name = path.file_name()?.to_str()?.to_owned();
            let metadata = std::fs::metadata(&path).ok()?;
            if file_name.starts_with('.') || !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return None;
            }
            let mut head = Vec::new();
            std::fs::File::open(&path).ok()?.take(HEADER_BYTES).read_to_end(&mut head).ok()?;
            Some(script(dir, path, &file_name, &String::from_utf8_lossy(&head)))
        })
        .collect();
    scripts.sort_by_key(|s| s.name.to_lowercase());
    scripts
}

/// The script at `path`, with the header `head` declares.
fn script(dir: &Path, path: PathBuf, file_name: &str, head: &str) -> Script {
    let mut name = None;
    let mut icon = ScriptIcon::Default;
    let mut mode = OutputMode::Silent;
    for (key, value) in header(head) {
        match key {
            "name" if !value.is_empty() => name = Some(value.to_owned()),
            "icon" if !value.is_empty() => icon = icon_of(dir, value),
            "mode" => match value {
                "silent" => mode = OutputMode::Silent,
                "output" => mode = OutputMode::Output,
                _ => log::warn!("{file_name}: @mode \"{value}\" is not silent or output"),
            },
            _ => {}
        }
    }
    let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    Script {
        id: format!("script:{file_name}"),
        name: name.unwrap_or_else(|| stem.to_owned()),
        path,
        icon,
        mode,
    }
}

/// The `@key value` lines of the comments at the top of `text`, after the
/// `#!` line; blank lines may come between them, code ends them.
fn header(text: &str) -> Vec<(&str, &str)> {
    let mut lines = text.lines().peekable();
    lines.next_if(|line| line.starts_with("#!"));
    let mut found = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(comment) = ["#", "//", "--"].iter().find_map(|marker| line.strip_prefix(marker)) else {
            break;
        };
        if let Some(declaration) = comment.trim_start().strip_prefix('@') {
            let (key, value) = declaration.split_once(char::is_whitespace).unwrap_or((declaration, ""));
            found.push((key, value.trim()));
        }
    }
    found
}

/// What `@icon value` shows: an image file itself; the icon Finder shows
/// for another path (an app, a System Settings pane's `.appex`); else the
/// text itself. Paths are relative to `dir`; a missing file shows the
/// default icon. Blocks: may make the Finder icon.
fn icon_of(dir: &Path, value: &str) -> ScriptIcon {
    let lower = value.to_lowercase();
    let is_image = [".png", ".svg", ".jpg", ".jpeg"].iter().any(|ext| lower.ends_with(ext));
    if !is_image && !value.contains('/') {
        return ScriptIcon::Emoji(value.to_owned());
    }
    let path = match value.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => dir.join(value),
    };
    if !is_image {
        // A missing path would get the generic document icon.
        let finder_icon = path.exists().then(|| icons::ensure(&path.to_string_lossy())).flatten();
        return finder_icon.map_or(ScriptIcon::Default, ScriptIcon::Image);
    }
    if lower.ends_with(".svg") {
        // `image` draws an SVG given as data, not one given by its path.
        return match std::fs::read_to_string(&path) {
            Ok(svg) => ScriptIcon::Image(icons::svg_uri(&svg)),
            Err(_) => ScriptIcon::Default,
        };
    }
    match path.is_file() {
        true => ScriptIcon::Image(path.to_string_lossy().into_owned()),
        false => ScriptIcon::Default,
    }
}

/// A script running. Dropping it kills the script and every process it
/// started.
///
/// Its fields are private: an `async move` block using a field alone takes
/// only that field, and would leave the rest, which kills the script when
/// dropped, behind.
pub(crate) struct Run {
    lines: mpsc::UnboundedReceiver<String>,
    done: oneshot::Receiver<Outcome>,
    _group: Group,
}

impl Run {
    /// The next line of output: stdout and stderr as they came, without
    /// ANSI escapes or control characters but tabs. `None` once the script
    /// has ended and every line is read.
    pub(crate) async fn line(&mut self) -> Option<String> {
        self.lines.recv().await
    }

    /// The next line if one has come, without waiting.
    pub(crate) fn line_now(&mut self) -> Option<String> {
        self.lines.try_recv().ok()
    }

    /// How it ended, once it has; then what it left running in the
    /// background is killed.
    pub(crate) async fn outcome(self) -> Outcome {
        // The rest of `self`, the group's killer, goes when this returns.
        self.done.await.unwrap_or_else(|_| Outcome::Failed("Stopped".into()))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    Succeeded,
    /// Why: "Exit code 1", "Killed by signal 9", "Not executable: …".
    Failed(String),
}

/// Kills the script's process group when dropped, and tells its reader to
/// stop.
struct Group {
    pgid: Option<i32>,
    dropped: Arc<AtomicBool>,
}

impl Drop for Group {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Relaxed);
        if let Some(pgid) = self.pgid {
            // SAFETY: `kill` takes any pid; a group already gone answers ESRCH.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
    }
}

/// Starts `script` in its folder, with Tinycast's environment and `env`
/// over it. A script that cannot start (not executable) ends at once,
/// failed, with why.
pub(crate) fn start(script: &Script, env: &[(String, String)]) -> Run {
    let (line_sender, lines) = mpsc::unbounded_channel();
    let (done_sender, done) = oneshot::channel();
    let dropped = Arc::new(AtomicBool::new(false));
    let pgid = match spawn(script, env) {
        Ok((child, output)) => {
            let pgid = child.id() as i32;
            let dropped = dropped.clone();
            std::thread::spawn(move || {
                let outcome = pump(child, output, &line_sender, &dropped);
                drop(line_sender);
                let _ = done_sender.send(outcome);
            });
            Some(pgid)
        }
        Err(error) => {
            let file_name = script.path.file_name().unwrap_or_default().to_string_lossy();
            let why = match error.raw_os_error() {
                Some(libc::EACCES) => format!("Not executable: chmod +x {file_name}"),
                _ => error.to_string(),
            };
            let _ = done_sender.send(Outcome::Failed(why));
            None
        }
    };
    Run { lines, done, _group: Group { pgid, dropped } }
}

/// The script running in a process group of its own, its stdout and stderr
/// on one pipe so their lines keep their order.
fn spawn(script: &Script, env: &[(String, String)]) -> std::io::Result<(Child, std::io::PipeReader)> {
    let (output, writer) = std::io::pipe()?;
    let child = std::process::Command::new(&script.path)
        .current_dir(script.path.parent().unwrap_or(Path::new("/")))
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0)
        .spawn()?;
    // The `Command` and its copies of the writer are gone: the pipe ends
    // when the script and what it started have closed theirs.
    Ok((child, output))
}

/// Sends the script's lines until the pipe ends, its `Run` is dropped, or
/// a little after the script has exited; then its outcome.
fn pump(mut child: Child, mut output: std::io::PipeReader, lines: &mpsc::UnboundedSender<String>, dropped: &AtomicBool) -> Outcome {
    let mut splitter = Lines::default();
    let mut buffer = [0u8; 8192];
    let mut exited_at: Option<Instant> = None;
    loop {
        if dropped.load(Ordering::Relaxed) || exited_at.is_some_and(|at| at.elapsed() > DRAIN) {
            break;
        }
        let mut fd = libc::pollfd { fd: output.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd for the pipe, which outlives the call.
        let ready = unsafe { libc::poll(&mut fd, 1, POLL_MS) };
        if ready > 0 {
            match output.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => splitter.push(&buffer[..read], |line| drop(lines.send(line))),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        } else if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            break;
        }
        if exited_at.is_none() && matches!(child.try_wait(), Ok(Some(_))) {
            exited_at = Some(Instant::now());
        }
    }
    splitter.finish(|line| drop(lines.send(line)));
    match child.wait() {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(0), _) => Outcome::Succeeded,
            (Some(code), _) => Outcome::Failed(format!("Exit code {code}")),
            (None, signal) => Outcome::Failed(format!("Killed by signal {}", signal.unwrap_or(0))),
        },
        Err(error) => Outcome::Failed(error.to_string()),
    }
}

/// Bytes into lines, as `pump` reads them: a character can be split
/// between two reads, so lines are made of bytes and decoded whole.
#[derive(Default)]
struct Lines {
    /// The line so far; its bytes past `LINE_BYTES` are dropped.
    line: Vec<u8>,
}

impl Lines {
    fn push(&mut self, bytes: &[u8], mut send: impl FnMut(String)) {
        for &byte in bytes {
            if byte == b'\n' {
                send(clean(&self.line));
                self.line.clear();
            } else if self.line.len() < LINE_BYTES {
                self.line.push(byte);
            }
        }
    }

    /// Sends the last line when the output did not end with a newline.
    fn finish(&mut self, mut send: impl FnMut(String)) {
        if !self.line.is_empty() {
            send(clean(&self.line));
            self.line.clear();
        }
    }
}

/// A line of output as text: ANSI escapes and control characters but tabs
/// taken out, at most `LINE_CHARS` characters.
fn clean(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                // CSI: parameters, then a final byte from @ to ~.
                Some('[') => {
                    while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
                }
                // OSC: to BEL or ESC \.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    if let Some((at, _)) = out.char_indices().nth(LINE_CHARS) {
        out.truncate(at);
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder of its own for a test.
    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tinycast-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_script(dir: &Path, file_name: &str, body: &str, mode: u32) -> Script {
        let path = dir.join(file_name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        script(dir, path, file_name, body)
    }

    /// Every line, then the outcome.
    fn run_to_end(script: &Script) -> (Vec<String>, Outcome) {
        let mut run = start(script, &[]);
        let mut lines = Vec::new();
        while let Some(line) = run.lines.blocking_recv() {
            lines.push(line);
        }
        let outcome = run.done.blocking_recv().unwrap();
        (lines, outcome)
    }

    #[test]
    fn the_header_names_the_script() {
        let dir = Path::new("/scripts");
        let s = script(dir, dir.join("dark.sh"), "dark.sh", "#!/bin/bash\n\n# @name Toggle Dark Mode\n#   @icon 🌙\n\n# @mode output\necho\n");
        assert_eq!(s.id, "script:dark.sh");
        assert_eq!(s.name, "Toggle Dark Mode");
        assert_eq!(s.icon, ScriptIcon::Emoji("🌙".into()));
        assert_eq!(s.mode, OutputMode::Output);
    }

    #[test]
    fn defaults_without_a_header() {
        let dir = Path::new("/scripts");
        let s = script(dir, dir.join("list-ports.sh"), "list-ports.sh", "#!/bin/sh\nlsof -i\n");
        assert_eq!(s.name, "list-ports");
        assert_eq!(s.icon, ScriptIcon::Default);
        assert_eq!(s.mode, OutputMode::Silent);
    }

    #[test]
    fn other_comment_styles() {
        assert_eq!(header("#!/usr/bin/swift\n// @name Swift\n-- @mode output\n"), [("name", "Swift"), ("mode", "output")]);
    }

    #[test]
    fn code_ends_the_header() {
        assert_eq!(header("#!/bin/sh\n# @name Kept\necho hi\n# @name Ignored\n"), [("name", "Kept")]);
    }

    #[test]
    fn an_unknown_mode_is_silent() {
        let dir = Path::new("/scripts");
        assert_eq!(script(dir, dir.join("a"), "a", "# @mode ouput\n").mode, OutputMode::Silent);
    }

    #[test]
    fn image_icons() {
        let dir = folder("icons");
        std::fs::write(dir.join("logo.png"), b"png").unwrap();
        std::fs::write(dir.join("logo.svg"), "<svg/>").unwrap();
        assert_eq!(icon_of(&dir, "logo.png"), ScriptIcon::Image(dir.join("logo.png").to_string_lossy().into()));
        assert_eq!(icon_of(&dir, "logo.svg"), ScriptIcon::Image(icons::svg_uri("<svg/>")));
        assert_eq!(icon_of(&dir, "missing.PNG"), ScriptIcon::Default);
    }

    #[test]
    fn other_paths_show_their_finder_icon() {
        let dir = folder("finder-icons");
        let ScriptIcon::Image(png) = icon_of(&dir, "/System/Library/ExtensionKit/Extensions/DisplaysExt.appex") else {
            panic!("no icon for the Displays pane");
        };
        assert!(png.ends_with(".png") && Path::new(&png).is_file(), "{png}");
        assert_eq!(icon_of(&dir, "/no/such/thing.appex"), ScriptIcon::Default);
    }

    #[test]
    fn only_executable_visible_files_are_scripts() {
        let dir = folder("scan");
        write_script(&dir, "b.sh", "#!/bin/sh\n# @name Beta\n", 0o755);
        write_script(&dir, "a.sh", "#!/bin/sh\n", 0o755);
        write_script(&dir, "plain.sh", "#!/bin/sh\n", 0o644);
        write_script(&dir, ".hidden.sh", "#!/bin/sh\n", 0o755);
        std::fs::create_dir(dir.join("sub")).unwrap();
        let elsewhere = folder("scan-elsewhere");
        write_script(&elsewhere, "linked.sh", "#!/bin/sh\n# @name Linked\n", 0o755);
        std::os::unix::fs::symlink(elsewhere.join("linked.sh"), dir.join("linked.sh")).unwrap();

        let names: Vec<String> = scan_in(&dir).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "Beta", "Linked"]);
    }

    #[test]
    fn output_keeps_its_order_across_stdout_and_stderr() {
        let dir = folder("order");
        let s = write_script(&dir, "o.sh", "#!/bin/sh\necho one\necho two >&2\nprintf 'three\\tcol'\n", 0o755);
        let (lines, outcome) = run_to_end(&s);
        assert_eq!(lines, ["one", "two", "three\tcol"]);
        assert_eq!(outcome, Outcome::Succeeded);
    }

    #[test]
    fn runs_in_its_folder_with_the_env() {
        let dir = folder("env");
        let s = write_script(&dir, "e.sh", "#!/bin/sh\npwd -P\necho \"$GREETING\"\n", 0o755);
        let mut run = start(&s, &[("GREETING".into(), "你好 🌙".into())]);
        let first = run.lines.blocking_recv().unwrap();
        assert_eq!(PathBuf::from(first), dir.canonicalize().unwrap());
        assert_eq!(run.lines.blocking_recv().unwrap(), "你好 🌙");
    }

    #[test]
    fn failures_say_why() {
        let dir = folder("fail");
        let exit = write_script(&dir, "x.sh", "#!/bin/sh\necho oops\nexit 3\n", 0o755);
        assert_eq!(run_to_end(&exit), (vec!["oops".into()], Outcome::Failed("Exit code 3".into())));
        let killed = write_script(&dir, "k.sh", "#!/bin/sh\nkill -9 $$\n", 0o755);
        assert_eq!(run_to_end(&killed).1, Outcome::Failed("Killed by signal 9".into()));
        // As from a terminal, a script without `#!` runs with /bin/sh.
        let no_shebang = write_script(&dir, "n.sh", "echo hi\n", 0o755);
        assert_eq!(run_to_end(&no_shebang), (vec!["hi".into()], Outcome::Succeeded));
        let plain = write_script(&dir, "p.sh", "#!/bin/sh\n", 0o644);
        assert_eq!(run_to_end(&plain).1, Outcome::Failed("Not executable: chmod +x p.sh".into()));
    }

    #[test]
    fn a_background_process_does_not_hold_the_run() {
        let dir = folder("background");
        let s = write_script(&dir, "b.sh", "#!/bin/sh\nsleep 30 &\necho started\n", 0o755);
        let begun = Instant::now();
        let (lines, outcome) = run_to_end(&s);
        assert_eq!((lines, outcome), (vec!["started".into()], Outcome::Succeeded));
        // Held by the background `sleep`, the run would last its 30 s.
        assert!(begun.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn dropping_the_run_kills_what_it_started() {
        let dir = folder("kill");
        let s = write_script(&dir, "s.sh", "#!/bin/sh\nsleep 30 &\necho $!\nwait\n", 0o755);
        let mut run = start(&s, &[]);
        let pid: i32 = run.lines.blocking_recv().unwrap().parse().unwrap();
        drop(run);
        let alive = || unsafe { libc::kill(pid, 0) } == 0;
        let begun = Instant::now();
        while alive() && begun.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive());
    }

    #[test]
    fn long_lines_are_cut() {
        let mut splitter = Lines::default();
        let mut lines = Vec::new();
        splitter.push(&vec![b'a'; LINE_BYTES * 2], |l| lines.push(l));
        splitter.push(b"\nnext\n", |l| lines.push(l));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), LINE_CHARS + 1);
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn characters_split_between_reads_stay_whole() {
        let mut splitter = Lines::default();
        let mut lines = Vec::new();
        let bytes = "月亮\n".as_bytes();
        splitter.push(&bytes[..2], |l| lines.push(l));
        splitter.push(&bytes[2..], |l| lines.push(l));
        assert_eq!(lines, ["月亮"]);
    }

    #[test]
    fn escapes_and_control_characters_go() {
        assert_eq!(clean(b"\x1b[1;31mred\x1b[0m\r \x1b]0;title\x07ok\tdone"), "red ok\tdone");
    }
}
