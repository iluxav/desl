//! One accent, one warning, one critical, three greys, and two hues for git. The terminal's own
//! palette; NO_COLOR honoured.

use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;

fn no_color() -> bool {
    static NC: OnceLock<bool> = OnceLock::new();
    *NC.get_or_init(|| std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()))
}

pub fn accent() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Blue) }
}
pub fn warning() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Yellow) }
}
pub fn critical() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { Style::default().fg(Color::Red) }
}
/// The quietest grey: borders, ids, hints.
pub fn muted() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::DIM) } else { Style::default().fg(Color::DarkGray) }
}
/// A readable grey: secondary text.
pub fn dim() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::DIM) } else { Style::default().fg(Color::Gray) }
}
/// Strings in code, and added lines in a diff. The one extra hue the transcript allows itself.
pub fn code_string() -> Style {
    if no_color() { Style::default() } else { Style::default().fg(Color::Green) }
}
/// Diff gutters and lines: green for added, red for removed. The sign carries it without colour.
pub fn added() -> Style {
    if no_color() { Style::default() } else { Style::default().fg(Color::Green) }
}
pub fn removed() -> Style {
    if no_color() { Style::default() } else { Style::default().fg(Color::Red) }
}
/// Claude's own orange, for its mark.
pub fn claude() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Rgb(217, 119, 87)) }
}
/// A branch name: the terminal's magenta, rose in most palettes.
pub fn branch() -> Style {
    if no_color() { Style::default() } else { Style::default().fg(Color::Magenta) }
}
/// Uncommitted paths: the terminal's green, as added lines in a diff.
pub fn changes() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::BOLD) } else { Style::default().fg(Color::Green) }
}
pub fn normal() -> Style {
    Style::default()
}
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
static SURFACE: OnceLock<Color> = OnceLock::new();
/// The terminal's background and foreground, as it reported them.
static BG: OnceLock<(u8, u8, u8)> = OnceLock::new();
static FG: OnceLock<(u8, u8, u8)> = OnceLock::new();
static NERD: OnceLock<bool> = OnceLock::new();

/// A shade a little lighter (or darker) than the terminal's background, mixed from what the
/// terminal said its colours are: `rgb:1a1a/1b1b/2626` style answers to OSC 10 and 11.
pub fn set_surface_from(bg: Option<&str>, fg: Option<&str>) {
    fn rgb(answer: &str) -> Option<(f32, f32, f32)> {
        let body = answer.split("rgb:").nth(1)?;
        let mut it = body.split(['/', '\x1b', '\x07']).take(3).map(|h| u8::from_str_radix(h.get(..2)?, 16).ok());
        Some((f32::from(it.next()??), f32::from(it.next()??), f32::from(it.next()??)))
    }
    let Some(b) = bg.and_then(rgb) else { return };
    let f = fg.and_then(rgb).unwrap_or((255.0, 255.0, 255.0));
    let _ = BG.set((b.0 as u8, b.1 as u8, b.2 as u8));
    let _ = FG.set((f.0 as u8, f.1 as u8, f.2 as u8));
    let mix = |b: f32, f: f32| (b + (f - b) * 0.09).round().clamp(0.0, 255.0) as u8;
    let _ = SURFACE.set(Color::Rgb(mix(b.0, f.0), mix(b.1, f.1), mix(b.2, f.2)));
}

/// The terminal's own text colour, when it said.
pub fn fg_rgb() -> (u8, u8, u8) {
    *FG.get().unwrap_or(&(225, 225, 225))
}

/// `c` at `strength` (0 to 1) over the terminal's background: how a terminal shows
/// opacity. Without a known background, a dim `c` below half strength.
pub fn faded(c: (u8, u8, u8), strength: f32) -> Style {
    if no_color() {
        return if strength < 0.75 { Style::default().add_modifier(Modifier::DIM) } else { Style::default() };
    }
    match BG.get() {
        Some(b) => {
            let k = strength.clamp(0.0, 1.0);
            let mix = |b: u8, c: u8| (f32::from(b) + (f32::from(c) - f32::from(b)) * k).round() as u8;
            Style::default().fg(Color::Rgb(mix(b.0, c.0), mix(b.1, c.1), mix(b.2, c.2)))
        }
        None => {
            let s = Style::default().fg(Color::Rgb(c.0, c.1, c.2));
            if strength < 0.75 { s.add_modifier(Modifier::DIM) } else { s }
        }
    }
}

/// The row of the thread on screen: a quiet tint across the sidebar, no bar.
pub fn surface() -> Style {
    if no_color() {
        return Style::default().add_modifier(Modifier::BOLD);
    }
    Style::default().bg(*SURFACE.get().unwrap_or(&Color::Indexed(236)))
}

/// Nerd Font icons (the Claude and OpenAI marks) instead of lettered badges.
pub fn set_nerd(on: bool) {
    let _ = NERD.set(on);
}
pub fn nerd() -> bool {
    *NERD.get().unwrap_or(&false)
}

/// The `↓ n new` pill and the pending prefix: inverted accent.
pub fn pill() -> Style {
    if no_color() { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default().fg(Color::Black).bg(Color::Blue) }
}
