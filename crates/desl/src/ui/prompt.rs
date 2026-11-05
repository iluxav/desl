//! The bordered prompt box and the one-line approval strip above it.

use super::text::{truncate_end, truncate_middle};
use crate::app::{App, Focus};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::UnicodeWidthStr;

const MAX_LINES: usize = 5;

/// One column of air on each side of the box, as in the spec drawing.
const MARGIN: u16 = 1;

fn inner_width(width: u16) -> usize {
    (width as usize).saturating_sub(2 * MARGIN as usize + 2 + 1 + 2 + 1).max(1)
}

pub fn height(app: &App, width: u16) -> u16 {
    let (lines, _) = app.input.layout(inner_width(width));
    (lines.len().clamp(1, MAX_LINES) + 2) as u16
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let area = Rect::new(area.x + MARGIN, area.y, area.width.saturating_sub(2 * MARGIN), area.height);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(theme::muted());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let width = inner_width(area.width);
    let (lines, (crow, ccol)) = app.input.layout(width);
    let visible_rows = inner.height as usize;
    // Keep the cursor's row in view when the prompt is taller than five lines.
    let first = crow.saturating_sub(visible_rows.saturating_sub(1));
    let focused = app.focus == Focus::Input;
    let caret_style = if focused { theme::accent() } else { theme::muted() };
    let rows: Vec<Line<'static>> = lines
        .iter()
        .enumerate()
        .skip(first)
        .take(visible_rows)
        .map(|(i, l)| {
            Line::from(vec![Span::styled(if i == 0 { " › " } else { "   " }, caret_style), Span::styled(l.clone(), theme::normal())])
        })
        .collect();
    f.render_widget(Paragraph::new(rows), inner);
    if focused {
        let x = inner.x + 3 + ccol as u16;
        let y = inner.y + (crow - first) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

/// The one-line strip above the prompt: an approval request, or a yes/no question such as a switch.
pub fn draw_strip(f: &mut Frame, app: &App, area: Rect) {
    let area = Rect::new(area.x + MARGIN, area.y, area.width.saturating_sub(2 * MARGIN), area.height);
    let t = app.thread();
    if let Some(a) = &t.meta.pending_approval {
        let keys: Vec<Span<'static>> = vec![
            Span::styled("[y]", theme::accent()),
            Span::styled("es ", theme::dim()),
            Span::styled("[n]", theme::accent()),
            Span::styled("o ", theme::dim()),
            Span::styled("[a]", theme::accent()),
            Span::styled("lways", theme::dim()),
        ];
        let keys_w: usize = keys.iter().map(|s| s.content.width()).sum();
        let head = format!(" Allow {}: ", t.agent());
        // A shell command reads as itself; other tools read as their name and target.
        let what = a.summary.clone();
        let avail = (area.width as usize).saturating_sub(head.width() + keys_w + 2);
        let what = truncate_middle(&what, avail.max(4));
        let pad = (area.width as usize).saturating_sub(head.width() + what.width() + keys_w);
        let mut spans = vec![Span::styled(head, theme::warning()), Span::styled(what, theme::normal()), Span::raw(" ".repeat(pad))];
        spans.extend(keys);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    } else if let Some(ask) = &t.ask {
        let keys: Vec<Span<'static>> = vec![
            Span::styled("[y]", theme::accent()),
            Span::styled("es ", theme::dim()),
            Span::styled("[n]", theme::accent()),
            Span::styled("o", theme::dim()),
        ];
        let keys_w: usize = keys.iter().map(|s| s.content.width()).sum();
        let avail = (area.width as usize).saturating_sub(keys_w + 3);
        let text = truncate_end(&ask.text, avail.max(8));
        let pad = (area.width as usize).saturating_sub(1 + text.width() + keys_w);
        let mut spans = vec![Span::raw(" "), Span::styled(text, theme::normal()), Span::raw(" ".repeat(pad))];
        spans.extend(keys);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}
