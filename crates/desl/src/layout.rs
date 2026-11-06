//! The workspace screen (SPEC.md "Interface design"): projects and their threads on the left,
//! the shown thread's agent on the right under a one-line header, and the common keys along the
//! bottom. The agent draws into desl's own terminal emulator, which is copied into the pane.

use crate::theme;
use crate::ui::text::truncate_end;
use desl_ir::{Project, SHELL, Status, Thread};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

/// What a key after the prefix does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Switch,
    Fork,
    New,
    Open,
    Rename,
    Info,
    Threads,
    Next,
    Prev,
    Jump(usize),
    Last,
    Sidebar,
    Close,
    Detach,
    Help,
}

/// The keys desl owns after the prefix, in the order the help shows them. The help is drawn
/// from this table and [`action`] reads it, so the two cannot disagree.
pub const KEYS: &[(&str, &str, &str)] = &[
    ("Agent", "s", "Switch this thread to the other agent"),
    ("Agent", "f", "Fork: this conversation in a new thread, on either agent"),
    ("Agent", "i", "Info: agent, session ids, model, directory"),
    ("Threads", "w", "Move to the thread list (j/k, enter, esc)"),
    ("Threads", "c", "New thread: pick Claude, Codex or a shell"),
    ("Threads", "o", "Open a directory: a shell to cd into it, then c"),
    ("Threads", "n", "Next thread"),
    ("Threads", "p", "Previous thread"),
    ("Threads", "1…9", "Jump to thread n"),
    ("Threads", "l", "Back to the thread shown before"),
    ("Threads", ",", "Rename this thread"),
    ("Threads", "x", "Close this thread: its agent or shell ends"),
    ("desl", "b", "Hide or show the sidebar"),
    ("desl", "d", "Detach; the agents keep working"),
    ("desl", "?", "This help"),
    ("desl", "ctrl+b", "Send ctrl+b to the agent"),
];

/// The action for the key pressed after the prefix.
pub fn action(c: char) -> Option<Action> {
    Some(match c {
        's' => Action::Switch,
        'w' => Action::Threads,
        'c' => Action::New,
        'f' => Action::Fork,
        ',' => Action::Rename,
        'i' => Action::Info,
        'o' => Action::Open,
        'n' => Action::Next,
        'p' => Action::Prev,
        'l' => Action::Last,
        'b' => Action::Sidebar,
        'x' => Action::Close,
        'd' => Action::Detach,
        '?' => Action::Help,
        '1'..='9' => Action::Jump(c as usize - '0' as usize),
        _ => return None,
    })
}

/// Every thread in a fixed order: by project, then oldest first. The sidebar, `prefix n`,
/// `prefix p` and `prefix 1`…`9` follow it, so a thread keeps its place while statuses change.
pub fn order<'a>(projects: &[Project], threads: &'a [Thread]) -> Vec<&'a Thread> {
    let rank = |t: &Thread| projects.iter().position(|p| p.id == t.project_id).unwrap_or(usize::MAX);
    let mut out: Vec<&Thread> = threads.iter().collect();
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.created_at.cmp(&b.created_at)).then(a.id.cmp(&b.id)));
    out
}

/// A path with the home directory written `~`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// The end of `s` in at most `width` columns, `…` standing for what was cut: a path keeps the
/// directories nearest the project.
pub fn truncate_start(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut w = 1;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > width {
            break;
        }
        w += cw;
        out.push(c);
    }
    out.push('…');
    out.iter().rev().collect()
}

/// Where everything goes on a terminal of this size: two rounded panels, the sidebar and the
/// agent's, over one line of keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    /// The sidebar's panel, borders included.
    pub sidebar: Option<Rect>,
    /// The agent's panel, borders included; its top border carries the header.
    pub main: Rect,
    /// The agent's screen inside the panel, a column of padding each side: its terminal is
    /// exactly this size.
    pub pane: Rect,
    pub footer: Rect,
}

/// The sidebar takes three tenths of the width, 32 to 52 columns, and steps aside below 80.
pub fn areas(cols: u16, rows: u16, sidebar: bool) -> Areas {
    let side_w = if sidebar && cols >= 80 { (cols * 3 / 10).clamp(32, 52) } else { 0 };
    let body_h = rows.saturating_sub(1).max(3);
    let main = Rect::new(side_w, 0, cols.saturating_sub(side_w).max(5), body_h);
    Areas {
        sidebar: (side_w > 0).then(|| Rect::new(0, 0, side_w, body_h)),
        main,
        pane: Rect::new(main.x + 2, main.y + 1, main.width.saturating_sub(4).max(1), main.height.saturating_sub(2).max(1)),
        footer: Rect::new(0, rows.saturating_sub(1), cols, 1),
    }
}

/// One line of the sidebar: a project, a thread that a click or `enter` opens, one of a
/// thread's subagents (a click opens its thread), or the space between two projects.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Row {
    Project(String),
    Thread(String),
    /// A thread's second line: its agent, status and age.
    Meta(String),
    /// A thread and the index of one of its subagents.
    Sub(String, usize),
    Gap,
}

/// A thread lists at most this many of its subagents, the newest.
pub const SUBAGENTS_SHOWN: usize = 4;

pub fn rows(projects: &[Project], threads: &[Thread]) -> Vec<Row> {
    let mut out = Vec::new();
    let mut last: Option<&str> = None;
    for t in order(projects, threads) {
        if last != Some(t.project_id.as_str()) {
            if last.is_some() {
                out.push(Row::Gap);
            }
            last = Some(t.project_id.as_str());
            out.push(Row::Project(t.project_id.clone()));
        }
        out.push(Row::Thread(t.id.clone()));
        out.push(Row::Meta(t.id.clone()));
        let n = t.subagents.len();
        out.extend((n.saturating_sub(SUBAGENTS_SHOWN)..n).map(|k| Row::Sub(t.id.clone(), k)));
    }
    out
}

/// Sidebar lines above the first row: the border and a line of padding.
const SIDE_TOP: u16 = 2;

/// The first row shown, so `keep` (an index into [`rows`]) stays in view.
pub fn side_skip(total: usize, keep: usize, area: Rect) -> usize {
    let room = area.height.saturating_sub(SIDE_TOP + 1) as usize;
    if total <= room { 0 } else { (keep + 1).saturating_sub(room).min(total - room) }
}

/// The sidebar row at line `y` (terminal row), if any.
pub fn row_at(projects: &[Project], threads: &[Thread], area: Rect, skip: usize, y: u16) -> Option<Row> {
    if y + 1 >= area.y + area.height {
        return None;
    }
    let i = (y.checked_sub(area.y + SIDE_TOP)? as usize) + skip;
    rows(projects, threads).get(i).cloned()
}

/// A subagent's conversation shown in the pane instead of the agent.
pub struct SubView<'a> {
    pub title: String,
    pub running: bool,
    pub lines: &'a [Line<'static>],
    /// Lines up from the bottom; 0 follows the end.
    pub scroll: usize,
}

/// One choice in a menu: what it starts, and why it cannot when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub note: String,
    pub enabled: bool,
}

pub struct Menu<'a> {
    pub title: &'a str,
    pub items: &'a [MenuItem],
    pub cursor: usize,
}

pub struct View<'a> {
    pub projects: &'a [Project],
    pub threads: &'a [Thread],
    pub current: &'a str,
    /// The highlighted sidebar row while the sidebar has the keys.
    pub cursor: Option<usize>,
    pub skip: usize,
    pub agent: &'a str,
    pub prefix_armed: bool,
    /// A sentence for the user, shown in the key line until the next key.
    pub notice: Option<&'a str>,
    pub help: bool,
    /// Lines scrolled back in the agent's screen, 0 when following it.
    pub scrolled: usize,
    pub menu: Option<Menu<'a>>,
    pub sub: Option<SubView<'a>>,
    /// Milliseconds since the epoch, for ages.
    pub now: i64,
    /// The subagent shown in the pane, (thread, subagent id): its row takes the tint.
    pub shown_sub: Option<(&'a str, &'a str)>,
    /// The new name being typed for the thread, in the key line.
    pub renaming: Option<&'a str>,
    /// `prefix i`: (label, value) rows shown in a box over the pane.
    pub info: Option<&'a [(String, String)]>,
}

fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::dim(),
        Status::Failed => theme::critical(),
    }
}

fn agent_title(agent: &str) -> String {
    let mut c = agent.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// A rounded panel, its border lit when it has the keys.
fn panel(focused: bool) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(if focused { theme::accent() } else { theme::muted() })
}

/// Each agent's mark: with a Nerd Font, the Claude and OpenAI logos and a terminal; without,
/// a one-cell badge in the agent's colour.
pub fn agent_icon(agent: &str) -> (&'static str, Style) {
    use ratatui::style::Color;
    let badge =
        |fg: Color, bg: Color| if theme::nerd() { Style::default() } else { Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD) };
    match (agent, theme::nerd()) {
        ("claude", true) => ("\u{ec82}", theme::claude()),
        ("codex", true) => ("\u{ec81}", theme::normal()),
        (SHELL, true) => ("\u{ebca}", theme::dim()),
        ("claude", false) => ("✻", badge(Color::Black, Color::Rgb(217, 119, 87))),
        ("codex", false) => ("›", badge(Color::Black, Color::Rgb(220, 220, 220))),
        (SHELL, false) => ("$", badge(Color::White, Color::Rgb(70, 70, 70))),
        (_, true) => ("\u{ec20}", theme::dim()),
        (_, false) => ("◆", theme::dim()),
    }
}

/// How long ago `then` was, in one short word: `now`, `5m`, `3h`, `2d`. Both in milliseconds.
pub fn age(now: i64, then: i64) -> String {
    let s = (now - then).max(0) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn sidebar(f: &mut Frame, v: &View, area: Rect) {
    let waiting = v.threads.iter().filter(|t| t.status == Status::Waiting).count();
    let mut block = panel(v.cursor.is_some()).title(Line::from(Span::styled(" Projects ", theme::bold())));
    if waiting > 0 {
        block = block.title(Line::from(Span::styled(format!(" {waiting} waiting "), theme::warning())).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    // A line of padding at the top, a column each side.
    let body = Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), inner.height.saturating_sub(1));
    let w = body.width as usize;
    let rows = rows(v.projects, v.threads);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(v.skip) {
        // The cursor lights a thread's both lines.
        let lit = match row {
            Row::Thread(_) => v.cursor == Some(i),
            Row::Meta(_) => v.cursor == Some(i.wrapping_sub(1)),
            _ => false,
        };
        match row {
            Row::Gap => lines.push(Line::default()),
            Row::Project(id) => {
                // The folder's name, then dimmer where it lives: `desl  ~/Projects`.
                let path = v.projects.iter().find(|p| p.id == *id).map_or(id.clone(), |p| p.path.clone());
                let p = std::path::Path::new(&path);
                let name = p.file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
                let parent = p.parent().map(|d| tilde(&d.to_string_lossy())).unwrap_or_default();
                let name = truncate_end(&name, w);
                let room = w.saturating_sub(name.width() + 2);
                let mut spans = vec![Span::styled(name, theme::bold())];
                if room >= 6 && !parent.is_empty() {
                    spans.push(Span::styled(format!("  {}", truncate_start(&parent, room)), theme::muted()));
                }
                lines.push(Line::from(spans));
            }
            Row::Thread(id) => {
                let Some(t) = v.threads.iter().find(|t| t.id == *id) else { continue };
                let current = t.id == v.current && v.shown_sub.is_none();
                // The thread on screen is tinted across both its lines; the cursor lights them.
                let base = if lit {
                    theme::pill()
                } else if current {
                    theme::surface()
                } else {
                    Style::default()
                };
                let on = |s: Style| if lit { theme::pill() } else { base.patch(s) };
                let title = if t.name.is_empty() { "New thread" } else { t.name.as_str() };
                let title = truncate_end(title, w.saturating_sub(4));
                let pad = w.saturating_sub(3 + title.width());
                lines.push(Line::from(vec![
                    Span::styled(" ", base),
                    Span::styled(t.status.glyph().to_string(), on(glyph_style(t.status))),
                    Span::styled(" ", base),
                    Span::styled(title, on(if current { theme::bold() } else { theme::normal() })),
                    Span::styled(" ".repeat(pad), base),
                ]));
            }
            Row::Meta(id) => {
                let Some(t) = v.threads.iter().find(|t| t.id == *id) else { continue };
                let current = t.id == v.current && v.shown_sub.is_none();
                let base = if lit {
                    theme::pill()
                } else if current {
                    theme::surface()
                } else {
                    Style::default()
                };
                let on = |s: Style| if lit { theme::pill() } else { base.patch(s) };
                // `   ✻ claude · idle · 5m`
                let (icon, icon_style) = agent_icon(&t.active_agent);
                let mut meta = if t.active_agent == SHELL {
                    tilde(&t.cwd)
                } else {
                    format!("{} · {} · {}", t.active_agent, t.status.word(), age(v.now, t.updated_at))
                };
                if let Some(b) = t.git.branch.as_deref().filter(|b| *b != "main" && *b != "master") {
                    meta.push_str(&format!(" · ⎇ {b}"));
                }
                let meta = truncate_end(&meta, w.saturating_sub(6));
                let pad = w.saturating_sub(5 + meta.width());
                lines.push(Line::from(vec![
                    Span::styled("   ", base),
                    Span::styled(icon, if lit { theme::pill() } else { base.patch(icon_style) }),
                    Span::styled(" ", base),
                    Span::styled(meta, on(theme::muted())),
                    Span::styled(" ".repeat(pad), base),
                ]));
            }
            Row::Sub(id, k) => {
                let Some(t) = v.threads.iter().find(|t| t.id == *id) else { continue };
                let Some(s) = t.subagents.get(*k) else { continue };
                // Running only while its thread's turn is: one left unfinished by a stopped turn is not.
                let (glyph, style) = match (s.finished, t.status.busy()) {
                    (true, _) => ("✓", theme::muted()),
                    (false, true) => ("●", theme::accent()),
                    (false, false) => ("○", theme::muted()),
                };
                let last = !matches!(rows.get(i + 1), Some(Row::Sub(next, _)) if next == id);
                let what = if s.description.is_empty() { s.kind.clone() } else { s.description.clone() };
                let label = if s.kind == "general-purpose" || s.description.is_empty() { what } else { format!("{} · {what}", s.kind) };
                let lit = v.cursor == Some(i);
                let shown = v.shown_sub == Some((id.as_str(), s.id.as_str()));
                let label = truncate_end(&label, w.saturating_sub(9));
                let pad = w.saturating_sub(8 + label.width());
                // The subagent in the pane is tinted like a thread on screen, its text brightened.
                let base = if shown { theme::surface() } else { Style::default() };
                let pick = |st: Style| if lit { theme::pill() } else { base.patch(st) };
                let text = if shown {
                    theme::bold()
                } else if s.finished {
                    theme::muted()
                } else {
                    theme::dim()
                };
                lines.push(Line::from(vec![
                    Span::styled(if last { "   └─ " } else { "   ├─ " }, theme::muted()),
                    Span::styled(glyph, pick(style)),
                    Span::styled(" ", pick(Style::default())),
                    Span::styled(label, pick(text)),
                    Span::styled(" ".repeat(pad), pick(Style::default())),
                ]));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), body);
}

/// The agent's panel: its border, with the project, agent and model on top and the thread's
/// id and status on the right.
fn main_panel(f: &mut Frame, v: &View, area: Rect) {
    let mut block = panel(v.cursor.is_none() && !v.help);
    if let Some(s) = &v.sub {
        let state = if s.running { Span::styled(" ● running ", theme::accent()) } else { Span::styled(" ✓ done ", theme::dim()) };
        block = block
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled("Subagent", theme::accent().add_modifier(Modifier::BOLD)),
                Span::styled(" · ", theme::muted()),
                Span::styled(s.title.clone(), theme::normal()),
                Span::raw(" "),
            ]))
            .title(Line::from(state).right_aligned());
        f.render_widget(block, area);
        return;
    }
    if let Some(t) = v.threads.iter().find(|t| t.id == v.current) {
        // A shell's header names where it started; an agent's, its project.
        let project = v.projects.iter().find(|p| p.id == t.project_id).map(|p| p.path.clone());
        let mut path = tilde(&if v.agent == SHELL { t.cwd.clone() } else { project.unwrap_or_else(|| t.cwd.clone()) });
        // A thread in its own worktree says which branch it works on.
        if t.worktree.is_some()
            && let Some(b) = &t.git.branch
        {
            path.push_str(&format!(" ⎇ {b}"));
        }
        let binding = t.bindings.get(v.agent);
        let mut model = binding.and_then(|b| b.model_choice.clone().or(b.model.clone())).unwrap_or_default();
        if let Some(e) = binding.and_then(|b| b.effort_choice.as_deref()) {
            model = if model.is_empty() { format!("{e} effort") } else { format!("{model} · {e}") };
        }
        let right = if v.scrolled > 0 {
            Line::from(Span::styled(format!(" ↑ {} lines · wheel down to follow ", v.scrolled), theme::warning()))
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(t.status.glyph().to_string(), glyph_style(t.status)),
                Span::styled(format!(" {} ", t.status.word()), theme::dim()),
                Span::styled(format!("{} ", t.id), theme::muted()),
            ])
        };
        let agent = if v.agent == SHELL { t.name.clone() } else { agent_title(v.agent) };
        let (icon, icon_style) = agent_icon(v.agent);
        let tail = if model.is_empty() { 0 } else { model.width() + 3 };
        let room = (area.width as usize).saturating_sub(right.width() + agent.width() + tail + 10).max(8);
        let mut left = vec![
            Span::raw(" "),
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(agent, theme::accent().add_modifier(Modifier::BOLD)),
            Span::styled(" · ", theme::muted()),
            Span::styled(truncate_start(&path, room), theme::normal()),
        ];
        if !model.is_empty() {
            left.push(Span::styled(" · ", theme::muted()));
            left.push(Span::styled(model, theme::dim()));
        }
        left.push(Span::raw(" "));
        block = block.title(Line::from(left)).title(right.right_aligned());
    }
    f.render_widget(block, area);
}

fn footer(f: &mut Frame, v: &View, area: Rect) {
    let line = if let Some(n) = v.notice {
        Line::from(vec![Span::styled("  ● ", theme::warning()), Span::styled(n.to_string(), theme::normal())])
    } else if let Some(name) = v.renaming {
        Line::from(vec![
            Span::styled("  Rename ", theme::bold()),
            Span::styled(name.to_string(), theme::normal()),
            Span::styled("▏", theme::accent()),
            Span::styled("   enter save   esc cancel", theme::dim()),
        ])
    } else if v.sub.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in [("esc", "back to the agent"), ("j/k", "scroll"), ("g/G", "top/bottom"), ("wheel", "scroll")] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else if v.menu.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in [("j/k", "move"), ("enter", "start"), ("1-9", "pick"), ("w", "own worktree"), ("esc", "cancel")] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else if v.cursor.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in [("j/k", "move"), ("enter", "open"), ("1-9", "jump"), ("esc", "back to the agent")] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else {
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(" ctrl+b ", if v.prefix_armed { theme::pill() } else { theme::accent().add_modifier(Modifier::BOLD) }),
            Span::raw("  "),
        ];
        for (k, what) in [
            ("s", "switch agent"),
            ("c", "new"),
            ("o", "open dir"),
            ("w", "threads"),
            ("n p", "next prev"),
            ("f", "fork"),
            ("x", "close"),
            ("?", "help"),
            ("d", "detach"),
        ] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
}

fn help(f: &mut Frame, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut group = "";
    for (g, key, what) in KEYS {
        if *g != group {
            if !group.is_empty() {
                lines.push(Line::default());
            }
            group = g;
            lines.push(Line::from(Span::styled(g.to_string(), theme::bold())));
        }
        lines.push(Line::from(vec![Span::styled(format!("  {key:<8}"), theme::accent()), Span::raw(what.to_string())]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Every other key goes to the agent. Any key closes this.", theme::dim())));
    // Borders, and a line of padding above.
    let h = (lines.len() as u16 + 3).min(area.height);
    let w = 64.min(area.width);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    let block = panel(true).title(Line::from(Span::styled(" Keys, after ctrl+b ", theme::bold())));
    let inner = block.inner(r);
    f.render_widget(Clear, r);
    f.render_widget(block, r);
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), inner.height.saturating_sub(1)),
    );
}

/// `prefix i`: the thread's facts in a box over the pane, labels dim, values plain.
fn info(f: &mut Frame, rows: &[(String, String)], area: Rect) {
    let label_w = rows.iter().map(|(l, _)| l.width()).max().unwrap_or(0);
    let value_w = rows.iter().map(|(_, v)| v.width()).max().unwrap_or(0);
    let w = ((label_w + value_w + 9) as u16).min(area.width);
    let h = (rows.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 3, w, h);
    let inner_w = (w as usize).saturating_sub(6 + label_w);
    let lines: Vec<Line<'static>> = rows
        .iter()
        .map(|(l, v)| Line::from(vec![Span::styled(format!("{l:<label_w$}  "), theme::dim()), Span::raw(truncate_start(v, inner_w))]))
        .collect();
    let block = panel(true).title(Line::from(Span::styled(" Thread ", theme::bold())));
    let inner = block.inner(r);
    f.render_widget(Clear, r);
    f.render_widget(block, r);
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), inner.height.saturating_sub(1)),
    );
}

/// A small box in the middle of the pane: numbered choices, the cursor's lit.
fn menu(f: &mut Frame, m: &Menu, area: Rect) {
    let label_w = m.items.iter().map(|i| i.label.width()).max().unwrap_or(0);
    let note_w = m.items.iter().map(|i| i.note.width()).max().unwrap_or(0);
    let w = ((label_w + note_w + 13) as u16).max(m.title.width() as u16 + 6).min(area.width);
    let h = (m.items.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 3, w, h);
    let inner_w = w.saturating_sub(4) as usize;
    let lines: Vec<Line<'static>> = m
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let lit = i == m.cursor;
            let text = format!(" {}  {:<label_w$}  {}", i + 1, item.label, item.note);
            let text = format!("{text:<inner_w$}");
            let style = match (lit, item.enabled) {
                (true, _) => theme::pill(),
                (false, true) => theme::normal(),
                (false, false) => theme::muted(),
            };
            Line::from(Span::styled(truncate_end(&text, inner_w), style))
        })
        .collect();
    let block = panel(true).title(Line::from(Span::styled(format!(" {} ", m.title), theme::bold())));
    let inner = block.inner(r);
    f.render_widget(Clear, r);
    f.render_widget(block, r);
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(inner.x + 1, inner.y + 1, inner.width.saturating_sub(2), inner.height.saturating_sub(1)),
    );
}

fn colour(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Copies the agent's screen into `area`.
pub fn pane(buf: &mut Buffer, area: Rect, screen: &vt100::Screen) {
    for r in 0..area.height {
        for c in 0..area.width {
            let out = &mut buf[(area.x + c, area.y + r)];
            let Some(cell) = screen.cell(r, c) else {
                out.reset();
                continue;
            };
            if cell.is_wide_continuation() {
                out.reset();
                continue;
            }
            let s = cell.contents();
            out.set_symbol(if s.is_empty() { " " } else { s });
            let mut m = Modifier::empty();
            for (on, flag) in [
                (cell.bold(), Modifier::BOLD),
                (cell.dim(), Modifier::DIM),
                (cell.italic(), Modifier::ITALIC),
                (cell.underline(), Modifier::UNDERLINED),
                (cell.inverse(), Modifier::REVERSED),
            ] {
                if on {
                    m |= flag;
                }
            }
            out.fg = colour(cell.fgcolor());
            out.bg = colour(cell.bgcolor());
            out.modifier = m;
        }
    }
}

/// Draws the screen: the panels, then the agent's screen copied into its pane.
pub fn draw(f: &mut Frame, v: &View, a: &Areas, screen: &vt100::Screen) {
    if let Some(s) = a.sidebar {
        sidebar(f, v, s);
    }
    main_panel(f, v, a.main);
    match &v.sub {
        Some(s) => {
            let h = a.pane.height as usize;
            let top = s.lines.len().saturating_sub(h + s.scroll);
            let shown: Vec<Line<'static>> = s.lines.iter().skip(top).take(h).cloned().collect();
            f.render_widget(Clear, a.pane);
            f.render_widget(Paragraph::new(shown), a.pane);
        }
        None => pane(f.buffer_mut(), a.pane, screen),
    }
    footer(f, v, a.footer);
    if let Some(m) = &v.menu {
        menu(f, m, a.pane);
    }
    if let Some(rows) = v.info {
        info(f, rows, a.pane);
    }
    if v.help {
        help(f, a.pane);
    }
}

fn sgr(fg: Color, bg: Color, m: Modifier) -> String {
    let mut codes = vec!["0".to_string()];
    for (flag, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::REVERSED, "7"),
        (Modifier::CROSSED_OUT, "9"),
    ] {
        if m.contains(flag) {
            codes.push(code.into());
        }
    }
    let colour = |c: Color, base: u8| -> Option<String> {
        let named = |n: u8| Some(format!("{}", base + n));
        let bright = |n: u8| Some(format!("{}", base + 60 + n));
        match c {
            Color::Reset => None,
            Color::Black => named(0),
            Color::Red => named(1),
            Color::Green => named(2),
            Color::Yellow => named(3),
            Color::Blue => named(4),
            Color::Magenta => named(5),
            Color::Cyan => named(6),
            Color::Gray => named(7),
            Color::DarkGray => bright(0),
            Color::LightRed => bright(1),
            Color::LightGreen => bright(2),
            Color::LightYellow => bright(3),
            Color::LightBlue => bright(4),
            Color::LightMagenta => bright(5),
            Color::LightCyan => bright(6),
            Color::White => bright(7),
            Color::Indexed(i) => Some(format!("{};5;{i}", base + 8)),
            Color::Rgb(r, g, b) => Some(format!("{};2;{r};{g};{b}", base + 8)),
        }
    };
    codes.extend(colour(fg, 30));
    codes.extend(colour(bg, 40));
    format!("\x1b[{}m", codes.join(";"))
}

/// Terminal output that turns `prev` into `next`: only the cells that changed, or every cell
/// when there is no `prev` of the same size.
pub fn paint(prev: Option<&Buffer>, next: &Buffer) -> Vec<u8> {
    let mut out = String::from("\x1b[0m");
    let mut style = String::new();
    let mut at: Option<(u16, u16)> = None;
    let mut put = |out: &mut String, x: u16, y: u16, cell: &ratatui::buffer::Cell| {
        if at != Some((x, y)) {
            out.push_str(&format!("\x1b[{};{}H", y + 1, x + 1));
        }
        let s = sgr(cell.fg, cell.bg, cell.modifier);
        if s != style {
            out.push_str(&s);
            style = s;
        }
        let sym = cell.symbol();
        out.push_str(if sym.is_empty() { " " } else { sym });
        at = Some((x + sym.width().max(1) as u16, y));
    };
    match prev {
        Some(p) if p.area == next.area => {
            for (x, y, cell) in p.diff(next) {
                put(&mut out, x, y, cell);
            }
        }
        _ => {
            out.push_str("\x1b[2J");
            let area = next.area;
            for y in 0..area.height {
                let mut skip = 0;
                for x in 0..area.width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let cell = &next[(x, y)];
                    put(&mut out, x, y, cell);
                    skip = cell.symbol().width().saturating_sub(1);
                }
            }
        }
    }
    out.push_str("\x1b[0m");
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn thread(id: &str, project: &str, created: i64, status: Status, name: &str) -> Thread {
        serde_json::from_value(serde_json::json!({
            "id": id, "project_id": project, "name": name, "cwd": "/w", "worktree": null,
            "parent_id": null, "fork_point": null, "active_agent": "claude", "status": status,
            "permission_mode": "edits", "created_at": created, "updated_at": 0, "head": 0,
            "bindings": { "claude": { "agent": "claude", "session_id": null, "seen_up_to": 0,
                "model": "opus", "model_choice": null, "effort_choice": "high", "last_used_at": 0 } },
            "git": { "branch": "main", "ahead": 0, "behind": 0 }
        }))
        .expect("a thread")
    }

    fn project(id: &str) -> Project {
        Project { id: id.into(), path: format!("/src/{id}"), default_agent: None, created_at: 0 }
    }

    fn screen(t: &Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_documented_key_has_an_action_and_digits_jump() {
        for (_, key, _) in KEYS {
            if *key != "ctrl+b" {
                assert!(action(key.chars().next().unwrap()).is_some(), "{key} does nothing");
            }
        }
        assert_eq!(action('3'), Some(Action::Jump(3)));
        assert_eq!(action('z'), None);
    }

    #[test]
    fn threads_keep_their_place_by_project_then_age() {
        let projects = [project("a"), project("b")];
        let threads = [
            thread("t-3", "b", 1, Status::Idle, "third"),
            thread("t-2", "a", 2, Status::Waiting, "second"),
            thread("t-1", "a", 1, Status::Working, "first"),
        ];
        let ids: Vec<&str> = order(&projects, &threads).iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t-1", "t-2", "t-3"]);
        let mut threads = threads;
        threads[2].subagents = (0..6)
            .map(|k| desl_ir::Subagent {
                id: format!("s{k}"),
                kind: "Explore".into(),
                description: "look".into(),
                finished: k < 5,
                started_at: k,
                ..Default::default()
            })
            .collect();
        let r = rows(&projects, &threads);
        assert_eq!(r[1], Row::Thread("t-1".into()));
        assert_eq!(r[2], Row::Meta("t-1".into()));
        assert_eq!(r[3], Row::Sub("t-1".into(), 2), "the newest four only");
        assert_eq!(r[7], Row::Thread("t-2".into()));
        let a = areas(120, 30, true);
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP + 3), Some(Row::Sub("t-1".into(), 2)));
        threads[2].subagents.clear();
        let r = rows(&projects, &threads);
        assert_eq!(r[0], Row::Project("a".into()));
        assert_eq!(r[5], Row::Gap);
        assert_eq!(r[6], Row::Project("b".into()));
        let a = areas(120, 30, true);
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP + 1), Some(Row::Thread("t-1".into())));
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP), Some(Row::Project("a".into())));
    }

    #[test]
    fn the_layout_gives_the_agent_what_the_chrome_leaves() {
        let a = areas(150, 40, true);
        assert_eq!(a.sidebar, Some(Rect::new(0, 0, 45, 39)));
        assert_eq!(a.main, Rect::new(45, 0, 105, 39));
        assert_eq!(a.pane, Rect::new(47, 1, 101, 37));
        assert_eq!(areas(300, 80, true).sidebar.map(|r| r.width), Some(52));
        assert_eq!(areas(90, 24, true).sidebar.map(|r| r.width), Some(32));
        assert_eq!(a.footer, Rect::new(0, 39, 150, 1));
        assert_eq!(areas(79, 24, true).sidebar, None);
        assert_eq!(areas(100, 30, false).pane.width, 96);
    }

    #[test]
    fn the_screen_reads_at_100_by_30_and_paints_only_what_changed() {
        let projects = [project("desl")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "desl", 2, Status::Waiting, "")];
        let a = areas(120, 30, true);
        let mut vt = vt100::Parser::new(a.pane.height.try_into().unwrap(), a.pane.width.try_into().unwrap(), 0);
        vt.process(b"\x1b[1mhello\x1b[0m from the agent");
        let mut v = View {
            projects: &projects,
            threads: &threads,
            current: "t-1a",
            cursor: None,
            skip: 0,
            agent: "claude",
            prefix_armed: false,
            notice: None,
            help: false,
            scrolled: 0,
            menu: None,
            sub: None,
            now: 0,
            shown_sub: None,
            renaming: None,
            info: None,
        };
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| draw(f, &v, &a, vt.screen())).unwrap();
        let s = screen(&t);
        assert_eq!(truncate_start("/tmp/a/b/demo", 8), "…/b/demo");
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("╭ Projects ") && lines[0].contains("1 waiting"), "{s}");
        assert!(lines[0].contains("╭ ✻ Claude · /src/desl · opus · high ") && lines[0].contains("○ idle t-1a ╮"), "{s}");
        assert!(lines[1].contains("││ hello from the agent"), "{s}");
        assert!(lines[2].contains("│ desl  /src "), "{s}");
        assert!(lines[3].contains("│  ○ fix the login "), "{s}");
        assert!(lines[4].contains("│    ✻ claude · idle · now "), "{s}");
        assert!(lines[5].contains("│  ◐ New thread "), "{s}");
        assert!(lines[6].contains("│    ✻ claude · waiting · now "), "{s}");
        // The thread on screen is tinted across its row; the others are not.
        let b = t.backend().buffer();
        assert_ne!(b[(20, 3)].bg, Color::Reset, "{s}");
        assert_eq!(b[(20, 5)].bg, Color::Reset, "{s}");
        // A subagent in the pane takes the tint from its thread.
        let mut with_sub = threads.clone();
        with_sub[0].subagents = vec![desl_ir::Subagent {
            id: "s1".into(),
            kind: "Explore".into(),
            description: "look".into(),
            finished: true,
            started_at: 0,
            ..Default::default()
        }];
        let mut t2 = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let v2 = View {
            projects: &projects,
            threads: &with_sub,
            current: "t-1a",
            cursor: None,
            skip: 0,
            agent: "claude",
            prefix_armed: false,
            notice: None,
            help: false,
            scrolled: 0,
            menu: None,
            sub: None,
            now: 0,
            shown_sub: Some(("t-1a", "s1")),
            renaming: None,
            info: None,
        };
        t2.draw(|f| draw(f, &v2, &a, vt.screen())).unwrap();
        let b2 = t2.backend().buffer();
        assert_eq!(b2[(20, 3)].bg, Color::Reset, "the thread gives up the tint");
        assert_ne!(b2[(20, 5)].bg, Color::Reset, "the subagent's row has it");
        assert!(lines[28].starts_with("╰"), "{s}");
        assert!(lines[29].contains(" ctrl+b   s switch agent"), "{s}");
        let before = t.backend().buffer().clone();
        let full = String::from_utf8(paint(None, &before)).unwrap();
        assert!(full.contains("\x1b[2J") && full.contains("hello"), "{full:?}");
        let items = [
            MenuItem { label: "Claude".into(), note: "agent".into(), enabled: true },
            MenuItem { label: "bash".into(), note: "shell".into(), enabled: true },
        ];
        v.menu = Some(Menu { title: "New in desl", items: &items, cursor: 1 });
        t.draw(|f| draw(f, &v, &a, vt.screen())).unwrap();
        let s = screen(&t);
        assert!(s.contains("╭ New in desl ") && s.contains(" 1  Claude  agent") && s.contains(" 2  bash    shell"), "{s}");
        assert!(s.lines().last().unwrap().contains("enter start"), "{s}");
        v.menu = None;
        v.help = true;
        t.draw(|f| draw(f, &v, &a, vt.screen())).unwrap();
        assert!(screen(&t).contains("Every other key goes to the agent."));
        let diff = String::from_utf8(paint(Some(&before), t.backend().buffer())).unwrap();
        assert!(!diff.contains("\x1b[2J") && diff.contains("Switch") && diff.contains("agent") && !diff.contains("projects"), "{diff:?}");
    }
}
