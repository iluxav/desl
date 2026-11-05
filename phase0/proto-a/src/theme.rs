//! One accent, one warning, one critical, three greys. The terminal's own palette; NO_COLOR honoured.

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
pub fn normal() -> Style {
    Style::default()
}
pub fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}
/// The `↓ n new` pill and the pending prefix: inverted accent.
pub fn pill() -> Style {
    if no_color() {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(Color::Black).bg(Color::Blue)
    }
}
