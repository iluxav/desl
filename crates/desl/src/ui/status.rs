//! One line: status glyph and word, agent, `⚠ yolo` while it is on, branch, help hint, clock.
//! Errors replace the left half.

use super::chrome::ChromeView;
use super::sidebar::glyph_style;
use crate::app::Level;
use crate::keymap;
use crate::theme;
use desl_ir::PermissionMode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, app: &ChromeView, area: Rect) {
    let t = app.thread();
    let mut right: Vec<Span<'static>> = Vec::new();
    if app.prefix_pending {
        right.push(Span::styled(format!(" {} ", keymap::PREFIX_LABEL), theme::pill()));
        right.push(Span::styled(
            format!(" {}", app.hint.strip_prefix(keymap::PREFIX_LABEL).unwrap_or(app.hint).trim_start()),
            theme::muted(),
        ));
    } else {
        right.push(Span::styled(app.hint.to_string(), theme::muted()));
    }
    right.push(Span::styled("  │ ", theme::muted()));
    right.push(Span::styled(app.clock.to_string(), theme::dim()));
    right.push(Span::raw(" "));
    let right_w: usize = right.iter().map(|s| s.content.width()).sum();

    let mut left: Vec<Span<'static>> = Vec::new();
    if let Some(n) = app.notice {
        let style = match n.level {
            Level::Error => theme::critical(),
            Level::Info => theme::dim(),
        };
        let avail = (area.width as usize).saturating_sub(right_w + 2);
        left.push(Span::raw(" "));
        left.push(Span::styled(super::text::truncate_end(&n.text, avail), style));
    } else {
        left.push(Span::raw(" "));
        left.push(Span::styled(t.status().glyph().to_string(), glyph_style(t.status())));
        left.push(Span::styled(format!(" {}", t.status().word()), theme::dim()));
        left.push(Span::styled(format!("  {}", t.agent()), theme::normal()));
        // Shown for as long as nothing asks (SPEC.md "Permissions and safety").
        if t.meta.permission_mode == PermissionMode::Yolo {
            left.push(Span::styled("  ⚠ yolo", theme::warning()));
        }
        let git = &t.meta.git;
        match &git.branch {
            Some(b) => {
                left.push(Span::styled(format!("  {b}"), theme::normal()));
                if git.ahead > 0 {
                    left.push(Span::styled(format!(" ↑{}", git.ahead), theme::dim()));
                }
                if git.behind > 0 {
                    left.push(Span::styled(format!(" ↓{}", git.behind), theme::dim()));
                }
            }
            None => left.push(Span::styled("  no git", theme::muted())),
        }
    }
    let left_w: usize = left.iter().map(|s| s.content.width()).sum();
    let pad = (area.width as usize).saturating_sub(left_w + right_w);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(right);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
