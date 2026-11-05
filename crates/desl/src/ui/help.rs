//! The `?` overlay, generated from the keymap: the focused element's keys, approvals when one is
//! pending, then the workspace keys that work everywhere. Scrolls when the terminal is short.

use super::chrome::ChromeView;
use crate::keymap::{self, Ctx, KEYMAP};
use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, app: &ChromeView, scroll: &mut usize, area: Rect) {
    let focused = app.focused_ctx;
    let mut order = vec![focused];
    if app.approval_pending {
        order.push(Ctx::Approval);
    }
    order.push(Ctx::Workspace);

    let shown: Vec<&'static keymap::Binding> = KEYMAP.iter().filter(|b| order.contains(&b.ctx)).collect();
    let label_w = shown.iter().map(|b| keymap::binding_label(b).width()).max().unwrap_or(8) + 2;

    let mut lines: Vec<Line<'static>> = Vec::new();
    for ctx in order {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(format!(" {}", ctx.title()), theme::muted().add_modifier(Modifier::BOLD))));
        for b in shown.iter().filter(|b| b.ctx == ctx) {
            let label = keymap::binding_label(b);
            let pad = label_w.saturating_sub(label.width());
            let built = b.built;
            let (key_style, desc_style) = if built { (theme::accent(), theme::normal()) } else { (theme::muted(), theme::muted()) };
            let mut spans = vec![
                Span::raw(" "),
                Span::styled(label, key_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(b.desc.to_string(), desc_style),
            ];
            if !built {
                spans.push(Span::styled("  · not yet", theme::muted()));
            }
            lines.push(Line::from(spans));
        }
    }

    let width = 76u16.min(area.width.saturating_sub(4)).max(20);
    let max_rows = area.height.saturating_sub(4) as usize;
    let footer_rows = 2; // blank + hint
    let body_rows = lines.len().min(max_rows.saturating_sub(footer_rows)).max(1);
    let height = (body_rows + footer_rows + 2) as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);

    let max_scroll = lines.len().saturating_sub(body_rows);
    *scroll = (*scroll).min(max_scroll);
    let below = lines.len().saturating_sub(*scroll + body_rows);

    let mut visible: Vec<Line<'static>> = lines.into_iter().skip(*scroll).take(body_rows).collect();
    visible.push(Line::from(""));
    let hint = if max_scroll > 0 {
        if below > 0 { format!(" ↓ {below} more · j/k scroll · esc closes") } else { " j/k scroll · esc closes".to_string() }
    } else {
        " esc closes".to_string()
    };
    visible.push(Line::from(Span::styled(hint, theme::muted())));

    f.render_widget(Clear, popup);
    let block =
        Block::bordered().border_type(BorderType::Rounded).border_style(theme::muted()).title(Line::from(" keys ").style(theme::muted()));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    f.render_widget(Paragraph::new(visible), inner);
}
