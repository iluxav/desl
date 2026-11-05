//! Paints a `vt100` screen into the ratatui buffer, cell for cell.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};

fn color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn render(screen: &vt100::Screen, area: Rect, buf: &mut Buffer) {
    let (rows, cols) = screen.size();
    let (rows, cols) = (rows.get(), cols.get());
    for row in 0..rows.min(area.height) {
        let mut col = 0u16;
        while col < cols.min(area.width) {
            let Some(cell) = screen.cell(row, col) else {
                col += 1;
                continue;
            };
            if cell.is_wide_continuation() {
                col += 1;
                continue;
            }
            let pos = Position::new(area.x + col, area.y + row);
            if let Some(target) = buf.cell_mut(pos) {
                let text = cell.contents();
                target.set_symbol(if text.is_empty() { " " } else { &text });
                let mut style = Style::default().fg(color(cell.fgcolor())).bg(color(cell.bgcolor()));
                if cell.bold() {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.italic() {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.underline() {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.inverse() {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                target.set_style(style);
            }
            if cell.is_wide() {
                // The continuation cell must stay blank so the wide glyph is not overdrawn.
                if let Some(next) = buf.cell_mut(Position::new(area.x + col + 1, area.y + row)) {
                    next.set_symbol("");
                }
                col += 2;
            } else {
                col += 1;
            }
        }
    }
}

/// Rows of text at the bottom of the screen, for status heuristics.
pub fn tail_text(screen: &vt100::Screen, rows: u16) -> String {
    let (h, w) = screen.size();
    let (h, w) = (h.get(), w.get());
    let from = h.saturating_sub(rows);
    (from..h).map(|r| screen.contents_between(r, 0, r, w)).collect::<Vec<_>>().join("\n")
}
