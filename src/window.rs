//! Window management, as Raycast's window commands: moves and resizes the
//! focused window of the app in front through the Accessibility API.

use std::{
    sync::{Mutex, PoisonError},
    thread,
    time::{Duration, Instant},
};

use nsh_core::{element::desktop::displays, prelude::*};
use serde::{Deserialize, Deserializer, de::Error};

use crate::mac::AxWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    TopLeftQuarter,
    TopRightQuarter,
    BottomLeftQuarter,
    BottomRightQuarter,
    FirstThird,
    CenterThird,
    LastThird,
    FirstTwoThirds,
    LastTwoThirds,
    Maximize,
    AlmostMaximize,
    Center,
    ReasonableSize,
    Restore,
    NextDisplay,
    PreviousDisplay,
    ToggleFullscreen,
}

use Command::*;

impl Command {
    /// In the order the launcher lists them.
    pub const ALL: [Command; 21] = [
        LeftHalf,
        RightHalf,
        TopHalf,
        BottomHalf,
        TopLeftQuarter,
        TopRightQuarter,
        BottomLeftQuarter,
        BottomRightQuarter,
        FirstThird,
        CenterThird,
        LastThird,
        FirstTwoThirds,
        LastTwoThirds,
        Maximize,
        AlmostMaximize,
        Center,
        ReasonableSize,
        Restore,
        NextDisplay,
        PreviousDisplay,
        ToggleFullscreen,
    ];

    /// Its name in the config's `[window]` table and in the launcher's ids.
    pub fn id(self) -> &'static str {
        match self {
            LeftHalf => "left-half",
            RightHalf => "right-half",
            TopHalf => "top-half",
            BottomHalf => "bottom-half",
            TopLeftQuarter => "top-left-quarter",
            TopRightQuarter => "top-right-quarter",
            BottomLeftQuarter => "bottom-left-quarter",
            BottomRightQuarter => "bottom-right-quarter",
            FirstThird => "first-third",
            CenterThird => "center-third",
            LastThird => "last-third",
            FirstTwoThirds => "first-two-thirds",
            LastTwoThirds => "last-two-thirds",
            Maximize => "maximize",
            AlmostMaximize => "almost-maximize",
            Center => "center",
            ReasonableSize => "reasonable-size",
            Restore => "restore",
            NextDisplay => "next-display",
            PreviousDisplay => "previous-display",
            ToggleFullscreen => "toggle-fullscreen",
        }
    }

    pub fn from_id(id: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.id() == id)
    }

    pub fn title(self) -> &'static str {
        match self {
            LeftHalf => "Left Half",
            RightHalf => "Right Half",
            TopHalf => "Top Half",
            BottomHalf => "Bottom Half",
            TopLeftQuarter => "Top Left Quarter",
            TopRightQuarter => "Top Right Quarter",
            BottomLeftQuarter => "Bottom Left Quarter",
            BottomRightQuarter => "Bottom Right Quarter",
            FirstThird => "First Third",
            CenterThird => "Center Third",
            LastThird => "Last Third",
            FirstTwoThirds => "First Two Thirds",
            LastTwoThirds => "Last Two Thirds",
            Maximize => "Maximize",
            AlmostMaximize => "Almost Maximize",
            Center => "Center",
            ReasonableSize => "Reasonable Size",
            Restore => "Restore",
            NextDisplay => "Next Display",
            PreviousDisplay => "Previous Display",
            ToggleFullscreen => "Toggle Fullscreen",
        }
    }

    /// The part of the display the window takes, as fractions of it, for
    /// the commands that give the window a fixed place.
    fn region(self) -> Option<Rect> {
        let r = |x, y, w, h| Some(Rect { x, y, w, h });
        let third = 1.0 / 3.0;
        match self {
            LeftHalf => r(0.0, 0.0, 0.5, 1.0),
            RightHalf => r(0.5, 0.0, 0.5, 1.0),
            TopHalf => r(0.0, 0.0, 1.0, 0.5),
            BottomHalf => r(0.0, 0.5, 1.0, 0.5),
            TopLeftQuarter => r(0.0, 0.0, 0.5, 0.5),
            TopRightQuarter => r(0.5, 0.0, 0.5, 0.5),
            BottomLeftQuarter => r(0.0, 0.5, 0.5, 0.5),
            BottomRightQuarter => r(0.5, 0.5, 0.5, 0.5),
            FirstThird => r(0.0, 0.0, third, 1.0),
            CenterThird => r(third, 0.0, third, 1.0),
            LastThird => r(2.0 * third, 0.0, third, 1.0),
            FirstTwoThirds => r(0.0, 0.0, 2.0 * third, 1.0),
            LastTwoThirds => r(third, 0.0, 2.0 * third, 1.0),
            Maximize => r(0.0, 0.0, 1.0, 1.0),
            AlmostMaximize => r(0.05, 0.05, 0.9, 0.9),
            ReasonableSize => r(0.2, 0.2, 0.6, 0.6),
            Center | Restore | NextDisplay | PreviousDisplay | ToggleFullscreen => None,
        }
    }

    fn is_third(self) -> bool {
        matches!(self, FirstThird | CenterThird | LastThird | FirstTwoThirds | LastTwoThirds)
    }

    /// Where it puts a window at `window`, given the displays' visible
    /// areas (not empty); `None` for Restore and Toggle Fullscreen, which
    /// do not compute a place.
    pub fn layout(self, window: Rect, displays: &[Rect]) -> Option<Rect> {
        let area = display_of(window, displays);
        if let Some(part) = self.region() {
            // On a portrait display thirds stack, as Raycast's do.
            let part = if self.is_third() && area.h > area.w { part.transposed() } else { part };
            return Some(area.part(part).rounded());
        }
        match self {
            Center => {
                let (w, h) = (window.w.min(area.w), window.h.min(area.h));
                Some(Rect { x: area.x + (area.w - w) / 2.0, y: area.y + (area.h - h) / 2.0, w, h }.rounded())
            }
            NextDisplay | PreviousDisplay => {
                let mut order = displays.to_vec();
                order.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
                let at = order.iter().position(|d| *d == area)?;
                let step = if self == NextDisplay { 1 } else { order.len() - 1 };
                let to = order[(at + step) % order.len()];
                // The same place and size relative to the display, kept on it.
                let w = (window.w / area.w).min(1.0);
                let h = (window.h / area.h).min(1.0);
                let x = ((window.x - area.x) / area.w).clamp(0.0, 1.0 - w);
                let y = ((window.y - area.y) / area.h).clamp(0.0, 1.0 - h);
                Some(to.part(Rect { x, y, w, h }).rounded())
            }
            _ => None,
        }
    }

    /// Its launcher icon: a screen on a blue tile, the part the window
    /// takes filled, as app icons are drawn.
    pub fn icon_svg(self) -> String {
        const SCREEN: Rect = Rect { x: 4.5, y: 6.0, w: 15.0, h: 12.0 };
        let mark = match self.region() {
            Some(part) => {
                let r = SCREEN.part(part);
                format!("<rect x='{}' y='{}' width='{}' height='{}' rx='1' fill='#FFFFFF'/>", r.x, r.y, r.w, r.h)
            }
            None => {
                let path = match self {
                    Center => "M10 10h4v4h-4z",
                    Restore => "M10.5 9.5 8.75 11.25 10.5 13M9 11.25h4.5a2 2 0 0 1 0 4h-2.5",
                    NextDisplay => "M8.5 12h7M13 9.5l2.5 2.5-2.5 2.5",
                    PreviousDisplay => "M15.5 12h-7M11 9.5 8.5 12l2.5 2.5",
                    _ => "M8 10.5V8.5h2.5M16 10.5V8.5h-2.5M8 13.5v2h2.5M16 13.5v2h-2.5",
                };
                format!(
                    "<path d='{path}' fill='none' stroke='#FFFFFF' stroke-width='1.5' \
                     stroke-linecap='round' stroke-linejoin='round'/>"
                )
            }
        };
        let s = SCREEN;
        format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24'>\
             <defs><linearGradient id='bg' x1='0' y1='0' x2='0' y2='1'>\
             <stop offset='0' stop-color='#5AC8FA'/><stop offset='1' stop-color='#1E6FF0'/>\
             </linearGradient></defs>\
             <rect x='1' y='1' width='22' height='22' rx='5.5' fill='url(#bg)'/>\
             <rect x='{}' y='{}' width='{}' height='{}' rx='1.5' fill='#FFFFFF' fill-opacity='0.25' \
             stroke='#FFFFFF' stroke-width='1.25'/>{mark}</svg>",
            s.x, s.y, s.w, s.h
        )
    }
}

impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = String::deserialize(deserializer)?;
        Command::from_id(&id).ok_or_else(|| D::Error::custom(format!("\"{id}\" is not a window command")))
    }
}

/// A frame in screen coordinates: points from the primary display's top
/// left corner, `y` down, as nsh and the Accessibility API have them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn from_bounds(b: Bounds<Points>) -> Rect {
        let (x, y) = (b.origin.x.0 as f64, b.origin.y.0 as f64);
        Rect { x, y, w: b.size.width.0 as f64, h: b.size.height.0 as f64 }
    }

    /// The part of `self` at `part`'s fractions of it.
    fn part(self, part: Rect) -> Rect {
        Rect {
            x: self.x + part.x * self.w,
            y: self.y + part.y * self.h,
            w: part.w * self.w,
            h: part.h * self.h,
        }
    }

    fn rounded(self) -> Rect {
        Rect { x: self.x.round(), y: self.y.round(), w: self.w.round(), h: self.h.round() }
    }

    fn transposed(self) -> Rect {
        Rect { x: self.y, y: self.x, w: self.h, h: self.w }
    }

    fn overlap(self, other: Rect) -> f64 {
        let w = (self.x + self.w).min(other.x + other.w) - self.x.max(other.x);
        let h = (self.y + self.h).min(other.y + other.h) - self.y.max(other.y);
        w.max(0.0) * h.max(0.0)
    }
}

/// The display most of `window` is on (the first one when it is on none).
fn display_of(window: Rect, displays: &[Rect]) -> Rect {
    displays
        .iter()
        .copied()
        .max_by(|a, b| window.overlap(*a).total_cmp(&window.overlap(*b)))
        .filter(|d| window.overlap(*d) > 0.0)
        .unwrap_or(displays[0])
}

/// A window Tinycast moved, for Restore.
struct Moved {
    window: AxWindow,
    /// Its frame before the first of the commands that moved it in a row.
    before: Rect,
    /// Where it took the last one.
    after: Rect,
}

/// The windows moved last, oldest first; windows closed since stay until
/// they are pushed out.
static MOVED: Mutex<Vec<Moved>> = Mutex::new(Vec::new());
const REMEMBERED: usize = 64;

/// Runs `command` on the focused window of the app in front, asking for
/// Accessibility access first when Tinycast has none.
pub fn run(command: Command) {
    if !crate::accessibility_granted() {
        return log::warn!("Tinycast needs Accessibility access to move windows");
    }
    let Some(pid) = crate::mac::frontmost_pid() else {
        return;
    };
    let areas: Vec<Rect> = displays().into_iter().map(|d| Rect::from_bounds(d.visible_bounds)).collect();
    if areas.is_empty() {
        return;
    }
    // Off the UI's thread: an app that does not answer holds each call for
    // seconds.
    blocking(move || apply(command, pid, &areas));
}

fn apply(command: Command, pid: i32, displays: &[Rect]) {
    let Some(window) = AxWindow::focused(pid) else {
        return log::info!("the app in front has no focused window");
    };
    let Some(frame) = window.frame() else {
        return log::info!("the focused window has no frame");
    };
    // Held throughout, so that presses in quick succession apply in turn.
    let mut moved = MOVED.lock().unwrap_or_else(PoisonError::into_inner);
    let known = moved.iter().position(|m| m.window.same_as(&window));
    match command {
        ToggleFullscreen => window.set_fullscreen(!window.is_fullscreen()),
        Restore => {
            if let Some(i) = known {
                place(&window, frame, moved.remove(i).before, displays);
            }
        }
        _ => {
            let Some(target) = command.layout(frame, displays) else {
                return;
            };
            place(&window, frame, target, displays);
            // Apps keep sizes to steps of their own (Terminal's character
            // grid): where it went is what the next command finds.
            let after = window.frame().unwrap_or(target);
            // Restore goes back to before the first command, unless the
            // window was moved by hand since the last one.
            let before = match known.map(|i| moved.remove(i)) {
                Some(last) if last.after == frame => last.before,
                _ => frame,
            };
            moved.push(Moved { window, before, after });
            if moved.len() > REMEMBERED {
                moved.remove(0);
            }
        }
    }
}

/// Moves `window` from `frame` to `target`.
fn place(window: &AxWindow, frame: Rect, target: Rect, displays: &[Rect]) {
    window.set_frame(target);
    if display_of(frame, displays) == display_of(target, displays) {
        return;
    }
    // Moved to another display, a window keeps to the old one's size for a
    // moment (some 60 ms): sized again until it takes the new one's.
    let start = Instant::now();
    while window.frame().is_some_and(|f| f != target) && start.elapsed() < Duration::from_millis(500) {
        thread::sleep(Duration::from_millis(20));
        window.set_frame(target);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Rect = Rect { x: 0.0, y: 25.0, w: 1440.0, h: 875.0 };
    const TALL: Rect = Rect { x: 1440.0, y: 0.0, w: 1080.0, h: 1920.0 };

    #[test]
    fn ids_name_the_commands() {
        for command in Command::ALL {
            assert_eq!(Command::from_id(command.id()), Some(command));
        }
    }

    #[test]
    fn halves_take_the_visible_area() {
        let window = Rect { x: 100.0, y: 100.0, w: 400.0, h: 300.0 };
        assert_eq!(
            RightHalf.layout(window, &[WIDE]),
            Some(Rect { x: 720.0, y: 25.0, w: 720.0, h: 875.0 })
        );
    }

    #[test]
    fn thirds_stack_on_a_portrait_display() {
        let window = Rect { x: 1500.0, y: 100.0, w: 400.0, h: 300.0 };
        assert_eq!(
            FirstThird.layout(window, &[WIDE, TALL]),
            Some(Rect { x: 1440.0, y: 0.0, w: 1080.0, h: 640.0 })
        );
    }

    #[test]
    fn center_keeps_the_size_that_fits() {
        let window = Rect { x: 0.0, y: 25.0, w: 400.0, h: 2000.0 };
        assert_eq!(
            Center.layout(window, &[WIDE]),
            Some(Rect { x: 520.0, y: 25.0, w: 400.0, h: 875.0 })
        );
    }

    #[test]
    fn next_display_keeps_the_relative_place_and_wraps() {
        let left_half = Rect { x: 0.0, y: 25.0, w: 720.0, h: 875.0 };
        assert_eq!(
            NextDisplay.layout(left_half, &[TALL, WIDE]),
            Some(Rect { x: 1440.0, y: 0.0, w: 540.0, h: 1920.0 })
        );
        let on_tall = Rect { x: 1440.0, y: 0.0, w: 540.0, h: 1920.0 };
        assert_eq!(NextDisplay.layout(on_tall, &[TALL, WIDE]), Some(left_half));
        assert_eq!(PreviousDisplay.layout(on_tall, &[TALL, WIDE]), Some(left_half));
    }

    #[test]
    fn a_window_off_every_display_goes_on_the_first() {
        let window = Rect { x: -5000.0, y: 0.0, w: 400.0, h: 300.0 };
        assert_eq!(Maximize.layout(window, &[WIDE, TALL]), Some(WIDE));
    }
}
