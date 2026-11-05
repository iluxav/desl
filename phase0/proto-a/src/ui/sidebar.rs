//! Projects, threads with status glyphs, and the WAITING group anchored at the bottom.

use super::text::truncate_end;
use super::chrome::ChromeView;
use crate::model::Status;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use ratatui::widgets::Paragraph;

pub fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::muted(),
        Status::Failed => theme::critical(),
    }
}

pub fn draw(f: &mut Frame, app: &ChromeView, area: Rect) {
    let w = area.width as usize;
    let mut top: Vec<Line<'static>> = Vec::new();
    top.push(Line::from(Span::styled(" PROJECTS", theme::muted().add_modifier(ratatui::style::Modifier::BOLD))));
    top.push(Line::from(vec![
        Span::styled(" ▾ ", theme::muted()),
        Span::styled(truncate_end(&app.project, w.saturating_sub(8)), theme::normal()),
        Span::styled(format!(" ({})", app.threads.len()), theme::muted()),
    ]));
    for (i, t) in app.threads.iter().enumerate() {
        let current = i == app.current;
        let focused = current && app.sidebar_focused;
        let name_w = w.saturating_sub(3 + 2 + t.id.len() + 2);
        let name = truncate_end(if t.name.is_empty() { "new" } else { &t.name }, name_w);
        let marker = if current { " ▸ " } else { "   " };
        let mut spans = vec![
            Span::styled(marker, if focused { theme::pill() } else { theme::accent() }),
            Span::styled(t.status.glyph().to_string(), if focused { theme::pill() } else { glyph_style(t.status) }),
            Span::styled(" ", if focused { theme::pill() } else { theme::normal() }),
            Span::styled(t.id.clone(), if focused { theme::pill() } else if current { theme::normal() } else { theme::muted() }),
            Span::styled("  ", if focused { theme::pill() } else { theme::normal() }),
            Span::styled(name.clone(), if focused { theme::pill() } else if current { theme::bold() } else { theme::dim() }),
        ];
        if focused {
            spans.push(Span::styled(" ".repeat(w.saturating_sub(3 + 2 + t.id.len() + 2 + name.width())), theme::pill()));
        }
        top.push(Line::from(spans));
    }

    let waiting: Vec<&crate::model::Thread> = app.threads.iter().filter(|t| t.status == Status::Waiting).collect();
    let mut bottom: Vec<Line<'static>> = Vec::new();
    let header_style = if waiting.is_empty() { theme::muted() } else { theme::warning().add_modifier(ratatui::style::Modifier::BOLD) };
    bottom.push(Line::from(Span::styled(format!(" WAITING ({})", waiting.len()), header_style)));
    for t in waiting.iter().rev() {
        bottom.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(t.status.glyph().to_string(), glyph_style(t.status)),
            Span::raw(" "),
            Span::styled(t.id.clone(), theme::normal()),
        ]));
    }

    let h = area.height as usize;
    let top_h = top.len().min(h);
    f.render_widget(Paragraph::new(top), Rect::new(area.x, area.y, area.width, top_h as u16));
    let bottom_h = bottom.len();
    let bottom_y = if top_h + 1 + bottom_h <= h { h - bottom_h } else { top_h + 1 };
    if bottom_y + bottom_h <= h {
        f.render_widget(Paragraph::new(bottom), Rect::new(area.x, area.y + bottom_y as u16, area.width, bottom_h as u16));
    }
}
