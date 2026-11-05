//! The picker overlay: a query line, then matches with the selected one inverted. Used for
//! threads, commands, files, prompt history, agents and models.

use crate::picker::Picker;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

/// Rows of matches on screen at once; a longer list scrolls with the selection.
const MAX_ROWS: usize = 10;

pub fn draw(f: &mut Frame, p: &Picker, area: Rect) {
    let matches = p.matches();
    let rows = matches.len().clamp(1, MAX_ROWS);
    let sel = p.selected.min(matches.len().saturating_sub(1));
    // The window follows the selection, so it is never off screen.
    let first = (sel + 1).saturating_sub(rows);
    let shown = &matches[first.min(matches.len())..(first + rows).min(matches.len())];
    let width = 76u16.min(area.width.saturating_sub(4)).max(24);
    // Descriptions line up in a column after the longest visible label.
    let label_w = shown.iter().map(|e| e.label.width()).max().unwrap_or(0).min((width as usize).saturating_sub(20));
    let more = matches.len().saturating_sub(first + rows);
    let footer = usize::from(p.note.is_some() || more > 0) * 2; // blank + note
    let height = (rows + 4 + footer) as u16; // borders + query + blank + rows
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 3;
    let popup = Rect::new(x, y, width, height.min(area.height));
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .title(Line::from(format!(" {} ", p.title)).style(theme::muted()));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(vec![Span::styled(" › ", theme::accent()), Span::raw(p.query.clone())]));
    lines.push(Line::from(""));
    let inner_w = inner.width as usize;
    for (i, e) in shown.iter().enumerate() {
        let selected = first + i == sel;
        let label = super::text::truncate_end(&e.label, label_w);
        let gap = label_w.saturating_sub(label.width()) + 2;
        let hint_w = inner_w.saturating_sub(3 + label_w + 2 + 1);
        let hint = super::text::truncate_end(&e.hint, hint_w);
        let pad = inner_w.saturating_sub(3 + label.width() + gap + hint.width());
        let (ls, hs) = if selected { (theme::pill(), theme::pill()) } else { (theme::normal(), theme::muted()) };
        let mut spans = vec![
            Span::styled(if selected { " ▸ " } else { "   " }, if selected { theme::pill() } else { theme::muted() }),
            Span::styled(label, ls),
            Span::styled(" ".repeat(gap), ls),
            Span::styled(hint, hs),
        ];
        if selected {
            spans.push(Span::styled(" ".repeat(pad), theme::pill()));
        }
        lines.push(Line::from(spans));
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled("   no match", theme::muted())));
    }
    if footer > 0 {
        let mut note = p.note.clone().unwrap_or_default();
        if more > 0 {
            let below = format!("↓ {more} more");
            note = if note.is_empty() { below } else { format!("{below} · {note}") };
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!(" {}", super::text::truncate_end(&note, inner_w.saturating_sub(2))), theme::muted())));
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position(Position::new(inner.x + 3 + p.query.width() as u16, inner.y));
}
