//! The screen: outer frame, sidebar, transcript, approval strip, prompt, status bar, help overlay.
//! Layout follows the drawing in SPEC.md "Interface design".

pub mod chrome;
pub mod help;
pub mod markdown;
pub mod picker;
pub mod prompt;
pub mod sidebar;
pub mod status;
pub mod text;
pub mod transcript;

use crate::app::{App, Focus, Overlay};
use crate::keymap::Ctx;
use crate::theme;
use chrome::ChromeView;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

/// Sidebar content width, plus its divider column.
fn sidebar_width(total: u16) -> u16 {
    if total >= 120 { 24 } else { 20 }
}

/// The outer frame, the separator, the sidebar and the status bar. Returns the main area.
pub fn frame(f: &mut Frame, view: &ChromeView, sidebar_visible: bool) -> Option<Rect> {
    let area = f.area();
    let outer =
        Block::bordered().border_type(BorderType::Rounded).border_style(theme::muted()).title(Line::from(" desl ").style(theme::muted()));
    let inner = outer.inner(area);
    f.render_widget(outer, area);
    if inner.height < 4 || inner.width < 20 {
        return None;
    }

    let [body, sep, status_row] = Layout::vertical([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    // The separator between body and status bar, joined into the outer frame.
    let buf = f.buffer_mut();
    for x in inner.x..inner.right() {
        if let Some(c) = buf.cell_mut(Position::new(x, sep.y)) {
            c.set_symbol("─").set_style(theme::muted());
        }
    }
    if let Some(c) = buf.cell_mut(Position::new(area.x, sep.y)) {
        c.set_symbol("├").set_style(theme::muted());
    }
    if let Some(c) = buf.cell_mut(Position::new(area.right() - 1, sep.y)) {
        c.set_symbol("┤").set_style(theme::muted());
    }

    let main = if sidebar_visible && body.width > sidebar_width(area.width) + 30 {
        let w = sidebar_width(area.width);
        let [side, main] = Layout::horizontal([Constraint::Length(w), Constraint::Min(10)]).areas(body);
        let divider = Block::default().borders(Borders::RIGHT).border_style(theme::muted());
        let side_inner = divider.inner(side);
        f.render_widget(divider, side);
        let x = side.right() - 1;
        let buf = f.buffer_mut();
        if let Some(c) = buf.cell_mut(Position::new(x, area.y)) {
            c.set_symbol("┬").set_style(theme::muted());
        }
        if let Some(c) = buf.cell_mut(Position::new(x, sep.y)) {
            c.set_symbol("┴").set_style(theme::muted());
        }
        sidebar::draw(f, view, side_inner);
        main
    } else {
        body
    };
    status::draw(f, view, status_row);
    Some(main)
}

fn chrome<'a>(app: &'a App, hint: &'a str) -> ChromeView<'a> {
    ChromeView {
        projects: &app.projects,
        threads: &app.threads,
        current: app.current,
        sidebar_focused: app.focus == Focus::Sidebar,
        notice: app.notice.as_ref(),
        clock: &app.clock,
        prefix_pending: app.prefix_pending,
        focused_ctx: match app.focus {
            Focus::Input => Ctx::Prompt,
            Focus::Transcript => Ctx::Transcript,
            Focus::Sidebar => Ctx::Sidebar,
        },
        approval_pending: app.approval_pending(),
        hint,
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let hint = format!("{} ? help", crate::keymap::PREFIX_LABEL);
    let Some(main) = frame(f, &chrome(app, &hint), app.sidebar_visible) else { return };
    draw_main(f, app, main);

    let area = f.area();
    if matches!(app.overlay, Overlay::Help) {
        let mut scroll = app.help_scroll;
        help::draw(f, &chrome(app, &hint), &mut scroll, area);
        app.help_scroll = scroll;
    } else if let Overlay::Picker(p, _) = &app.overlay {
        picker::draw(f, p, area);
    }
}

fn draw_main(f: &mut Frame, app: &mut App, area: Rect) {
    let prompt_height = prompt::height(app, area.width);
    let approval_height = u16::from(app.strip_pending());
    let [title, transcript, approval, prompt_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(approval_height),
        Constraint::Length(prompt_height),
    ])
    .areas(area);

    let t = app.thread();
    let branch = t.meta.git.branch.clone().unwrap_or_else(|| "no git".into());
    let title_line = Line::from(format!(" {} · {} · {}", t.id(), t.agent(), branch)).style(theme::muted());
    f.render_widget(title_line, title);

    transcript::draw(f, app, transcript);
    if approval_height > 0 {
        prompt::draw_strip(f, app, approval);
    }
    prompt::draw(f, app, prompt_area);
}

#[cfg(test)]
mod tests;
