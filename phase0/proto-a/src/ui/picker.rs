//! The picker overlay: a query line, then matches with the selected one inverted.

use crate::picker::Picker;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, p: &Picker, area: Rect) {
    let matches = p.matches();
    let rows = matches.len().clamp(1, 10);
    let width = 76u16.min(area.width.saturating_sub(4)).max(24);
    // Descriptions line up in a column after the longest visible label.
    let label_w = matches.iter().take(rows).map(|e| e.label.width()).max().unwrap_or(0).min((width as usize).saturating_sub(20));
    let height = (rows + 4) as u16; // borders + query + blank + rows
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
    let sel = p.selected.min(matches.len().saturating_sub(1));
    let inner_w = inner.width as usize;
    for (i, e) in matches.iter().take(rows).enumerate() {
        let label = super::text::truncate_end(&e.label, label_w);
        let gap = label_w.saturating_sub(label.width()) + 2;
        let hint_w = inner_w.saturating_sub(3 + label_w + 2 + 1);
        let hint = super::text::truncate_end(&e.hint, hint_w);
        let pad = inner_w.saturating_sub(3 + label.width() + gap + hint.width());
        let (ls, hs) = if i == sel { (theme::pill(), theme::pill()) } else { (theme::normal(), theme::muted()) };
        let mut spans = vec![
            Span::styled(if i == sel { " ▸ " } else { "   " }, if i == sel { theme::pill() } else { theme::muted() }),
            Span::styled(label, ls),
            Span::styled(" ".repeat(gap), ls),
            Span::styled(hint, hs),
        ];
        if i == sel {
            spans.push(Span::styled(" ".repeat(pad), theme::pill()));
        }
        lines.push(Line::from(spans));
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled("   no match", theme::muted())));
    }
    f.render_widget(Paragraph::new(lines), inner);
    f.set_cursor_position(Position::new(inner.x + 3 + p.query.width() as u16, inner.y));
}
