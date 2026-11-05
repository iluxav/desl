//! Projects, threads with status glyphs, and the WAITING group anchored at the bottom.

use super::chrome::ChromeView;
use super::text::truncate_end;
use crate::model::{Status, Thread};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

pub fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::muted(),
        Status::Failed => theme::critical(),
    }
}

fn thread_row(t: &Thread, current: bool, focused: bool, w: usize) -> Line<'static> {
    let id = t.id().to_string();
    let name_w = w.saturating_sub(3 + 2 + id.width() + 2);
    let name = truncate_end(if t.name().is_empty() { "new" } else { t.name() }, name_w);
    let marker = if current { " ▸ " } else { "   " };
    let pill = theme::pill();
    let mut spans = vec![
        Span::styled(marker, if focused { pill } else { theme::accent() }),
        Span::styled(t.status().glyph().to_string(), if focused { pill } else { glyph_style(t.status()) }),
        Span::styled(" ", if focused { pill } else { theme::normal() }),
        Span::styled(
            id.clone(),
            if focused {
                pill
            } else if current {
                theme::normal()
            } else {
                theme::muted()
            },
        ),
        Span::styled("  ", if focused { pill } else { theme::normal() }),
        Span::styled(
            name.clone(),
            if focused {
                pill
            } else if current {
                theme::bold()
            } else {
                theme::dim()
            },
        ),
    ];
    if focused {
        spans.push(Span::styled(" ".repeat(w.saturating_sub(3 + 2 + id.width() + 2 + name.width())), pill));
    }
    Line::from(spans)
}

pub fn draw(f: &mut Frame, app: &ChromeView, area: Rect) {
    let w = area.width as usize;
    let mut top: Vec<Line<'static>> = Vec::new();
    top.push(Line::from(Span::styled(" PROJECTS", theme::muted().add_modifier(Modifier::BOLD))));
    // The row of the current thread, so a long list keeps it in view.
    let mut current_row = 0;
    for p in app.projects {
        let count = app.threads.iter().filter(|t| t.meta.project_id == p.id).count();
        top.push(Line::from(vec![
            Span::styled(" ▾ ", theme::muted()),
            Span::styled(truncate_end(&p.id, w.saturating_sub(8)), theme::normal()),
            Span::styled(format!(" ({count})"), theme::muted()),
        ]));
        for (i, t) in app.threads.iter().enumerate().filter(|(_, t)| t.meta.project_id == p.id) {
            let current = i == app.current;
            if current {
                current_row = top.len();
            }
            top.push(thread_row(t, current, current && app.sidebar_focused, w));
        }
    }

    let waiting: Vec<&Thread> = app.threads.iter().filter(|t| t.status() == Status::Waiting).collect();
    let mut bottom: Vec<Line<'static>> = Vec::new();
    let header_style = if waiting.is_empty() { theme::muted() } else { theme::warning().add_modifier(Modifier::BOLD) };
    bottom.push(Line::from(Span::styled(format!(" WAITING ({})", waiting.len()), header_style)));
    for t in waiting.iter().rev() {
        bottom.push(Line::from(vec![
            Span::raw("   "),
            Span::styled(t.status().glyph().to_string(), glyph_style(t.status())),
            Span::raw(" "),
            Span::styled(t.id().to_string(), theme::normal()),
        ]));
    }

    let h = area.height as usize;
    let bottom_h = bottom.len().min(h);
    // The thread list gets what the WAITING group leaves, and scrolls to keep the current thread on screen.
    let top_room = h.saturating_sub(bottom_h + 1).max(1).min(h);
    let skip = if current_row >= top_room { current_row + 1 - top_room } else { 0 };
    let top: Vec<Line<'static>> = top.into_iter().skip(skip).take(top_room).collect();
    let top_h = top.len();
    f.render_widget(Paragraph::new(top), Rect::new(area.x, area.y, area.width, top_h as u16));
    let bottom_y = if top_h + 1 + bottom_h <= h { h - bottom_h } else { top_h + 1 };
    if bottom_y + bottom_h <= h {
        f.render_widget(Paragraph::new(bottom), Rect::new(area.x, area.y + bottom_y as u16, area.width, bottom_h as u16));
    }
}
