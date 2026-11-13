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
    Restart,
    SplitBeside,
    SplitStacked,
    Zoom,
    /// `alt+h` / `alt+l`: the keys move to the other pane.
    OtherPane,
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

/// A key desl owns after the prefix: its group in the list, the key as written, what it does and
/// the action it runs; `None` for the two rows that are not one action (`1…9`, the prefix itself).
pub struct Bind {
    pub group: &'static str,
    pub key: &'static str,
    pub what: &'static str,
    pub action: Option<Action>,
}

const fn bind(group: &'static str, key: &'static str, what: &'static str, action: Option<Action>) -> Bind {
    Bind { group, key, what, action }
}

/// The keys desl owns after the prefix, in the order the list shows them. The list is drawn
/// from this table and [`action`] reads it, so the two cannot disagree.
pub const KEYS: &[Bind] = &[
    bind("Agent", "s", "Switch this thread to the other agent", Some(Action::Switch)),
    bind("Agent", "f", "Fork: this conversation in a new thread, on either agent", Some(Action::Fork)),
    bind("Agent", "i", "Info: agent, session ids, model, directory", Some(Action::Info)),
    bind("Agent", "r", "Restart this agent, keeping the conversation", Some(Action::Restart)),
    bind("Threads", "w", "Move to the thread list (j/k, enter, esc)", Some(Action::Threads)),
    bind("Threads", "c", "New thread: pick Claude, Codex or a shell", Some(Action::New)),
    bind("Threads", "o", "Open a directory: type to find it; a shell opens there", Some(Action::Open)),
    bind("Threads", "n", "Next thread", Some(Action::Next)),
    bind("Threads", "p", "Previous thread", Some(Action::Prev)),
    bind("Threads", "1…9", "Jump to thread n", None),
    bind("Threads", "l", "Back to the thread shown before", Some(Action::Last)),
    bind("Threads", ",", "Rename this thread", Some(Action::Rename)),
    bind("Threads", "x", "Close this thread: its agent or shell ends", Some(Action::Close)),
    bind("Panes", "%", "Split: another thread beside this one", Some(Action::SplitBeside)),
    bind("Panes", "\"", "Split: another thread under this one", Some(Action::SplitStacked)),
    bind("Panes", "z", "Zoom: back to one pane, this one", Some(Action::Zoom)),
    bind("desl", "b", "Hide or show the sidebar", Some(Action::Sidebar)),
    bind("desl", "d", "Detach; the agents keep working", Some(Action::Detach)),
    bind("desl", "?", "This list; typing filters it, enter runs the row", Some(Action::Help)),
    bind("desl", "prefix", "Send the prefix key itself to the agent", None),
];

/// The keys desl owns without the prefix: the only ones that never reach the agent.
pub const DIRECT: &[(&str, &str)] =
    &[("alt+j k", "next / previous thread, without the prefix"), ("alt+h l", "the other pane, while two are open")];

/// The action for the key pressed after the prefix.
pub fn action(c: char) -> Option<Action> {
    if let Some(n) = c.to_digit(10).filter(|n| *n >= 1) {
        return Some(Action::Jump(n as usize));
    }
    KEYS.iter().find(|b| b.key.chars().eq([c])).and_then(|b| b.action)
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Areas {
    /// The sidebar's panel, borders included.
    pub sidebar: Option<Rect>,
    /// Everything right of the sidebar: the panes' panels together, where overlays go.
    pub body: Rect,
    /// Each pane's panel, borders included, left to right or top to bottom; its top border
    /// carries the header.
    pub mains: Vec<Rect>,
    /// Each agent's screen inside its panel, a column of padding each side: its terminal is
    /// exactly this size.
    pub panes: Vec<Rect>,
    pub footer: Rect,
}

/// The sidebar takes three tenths of the width, 32 to 52 columns, and steps aside below 80.
/// `split` is `Some(stacked)` for two panes: beside each other, or one above the other.
pub fn areas(cols: u16, rows: u16, sidebar: bool, split: Option<bool>) -> Areas {
    let side_w = if sidebar && cols >= 80 { (cols * 3 / 10).clamp(32, 52) } else { 0 };
    let body_h = rows.saturating_sub(1).max(3);
    let body = Rect::new(side_w, 0, cols.saturating_sub(side_w).max(5), body_h);
    let mains = match split {
        None => vec![body],
        Some(false) => {
            let left = body.width / 2;
            vec![Rect::new(body.x, body.y, left, body.height), Rect::new(body.x + left, body.y, body.width - left, body.height)]
        }
        Some(true) => {
            let top = body.height / 2;
            vec![Rect::new(body.x, body.y, body.width, top), Rect::new(body.x, body.y + top, body.width, body.height - top)]
        }
    };
    let panes =
        mains.iter().map(|m| Rect::new(m.x + 2, m.y + 1, m.width.saturating_sub(4).max(1), m.height.saturating_sub(2).max(1))).collect();
    Areas {
        sidebar: (side_w > 0).then(|| Rect::new(0, 0, side_w, body_h)),
        body,
        mains,
        panes,
        footer: Rect::new(0, rows.saturating_sub(1), cols, 1),
    }
}

/// One line of the sidebar: a project, a thread that a click or `enter` opens, one of a
/// thread's subagents (a click opens its thread), or the space between two projects.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Row {
    Project(String),
    Thread(String),
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
            // A blank row between projects; threads sit row on row.
            if last.is_some() {
                out.push(Row::Gap);
            }
            last = Some(t.project_id.as_str());
            out.push(Row::Project(t.project_id.clone()));
        }
        out.push(Row::Thread(t.id.clone()));
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

/// `prefix o`: the query, and the directories that match it, as `~/x` with where each is from.
/// The key list (`prefix ?`) while it is open: the typed filter and the row under the cursor.
pub struct PaletteView<'a> {
    pub query: &'a str,
    pub cursor: usize,
}

/// The rows of the key list that match `query`, best first; every runnable key when it is empty.
pub fn palette_rows(query: &str) -> Vec<&'static Bind> {
    let mut rows: Vec<(i64, usize, &'static Bind)> = KEYS
        .iter()
        .enumerate()
        .filter(|(_, b)| b.action.is_some())
        .filter_map(|(i, b)| crate::finder::score(&format!("{} {} {}", b.key, b.what, b.group), query).map(|s| (s, i, b)))
        .collect();
    if !query.is_empty() {
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    rows.into_iter().map(|(_, _, b)| b).collect()
}

pub struct FinderView<'a> {
    pub query: &'a str,
    pub rows: &'a [(String, &'static str)],
    pub cursor: usize,
    /// Still looking for repositories under the home directory.
    pub scanning: bool,
}

/// One pane: the thread it shows, that thread's agent, and how far back its screen is scrolled.
pub struct PaneView<'a> {
    pub thread_id: &'a str,
    pub agent: &'a str,
    /// Lines scrolled back in the agent's screen, 0 when following it.
    pub scrolled: usize,
}

pub struct View<'a> {
    pub projects: &'a [Project],
    pub threads: &'a [Thread],
    /// The panes, left to right or top to bottom; `focus` is the one with the keys.
    pub panes: Vec<PaneView<'a>>,
    pub focus: usize,
    /// The highlighted sidebar row while the sidebar has the keys.
    pub cursor: Option<usize>,
    pub skip: usize,
    pub prefix_armed: bool,
    /// A sentence for the user, shown in the key line until the next key.
    pub notice: Option<&'a str>,
    pub help: Option<PaletteView<'a>>,
    pub menu: Option<Menu<'a>>,
    pub finder: Option<FinderView<'a>>,
    pub sub: Option<SubView<'a>>,
    /// Milliseconds since the epoch, for ages.
    pub now: i64,
    /// The subagent shown in the pane, (thread, subagent id): its row takes the tint.
    pub shown_sub: Option<(&'a str, &'a str)>,
    /// The new name being typed for the thread, in the key line.
    pub renaming: Option<&'a str>,
    /// `prefix i`: (label, value) rows shown in a box over the pane.
    pub info: Option<&'a [(String, String)]>,
    /// Where a working thread's logo is in its slow pulse: 0 faint to 1 full.
    pub pulse: f32,
    /// The prefix as the interface writes it: `ctrl+space`.
    pub prefix: &'a str,
}

impl View<'_> {
    /// The thread in the pane that has the keys.
    pub fn current(&self) -> &str {
        self.panes.get(self.focus).map_or("", |p| p.thread_id)
    }
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

/// An agent's own colour: Claude's orange, Codex in the terminal's text colour, a shell grey.
pub fn agent_rgb(agent: &str) -> (u8, u8, u8) {
    match agent {
        "claude" => (217, 119, 87),
        SHELL => (150, 150, 150),
        _ => theme::fg_rgb(),
    }
}

/// A thread's logo says its status: working pulses between faint and full, waiting is the
/// warning colour, failed the critical one, idle sits faded.
fn status_icon(t: &Thread, pulse: f32) -> (&'static str, Style) {
    let (icon, _) = agent_icon(&t.active_agent);
    let icon = if theme::nerd() { icon } else { text_icon(&t.active_agent) };
    let rgb = agent_rgb(&t.active_agent);
    let style = match t.status {
        Status::Working => theme::faded(rgb, 0.3 + 0.7 * pulse),
        Status::Waiting => theme::warning(),
        Status::Failed => theme::critical(),
        Status::Idle | Status::Detached => theme::faded(rgb, 0.45),
    };
    (icon, style)
}

/// Without a Nerd Font, a one-cell mark in the agent's colour that can fade like the logos.
fn text_icon(agent: &str) -> &'static str {
    match agent {
        "claude" => "✻",
        "codex" => "◎",
        SHELL => "$",
        _ => "◆",
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
        let lit = matches!(row, Row::Thread(_)) && v.cursor == Some(i);
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
                // A thread whose subagent is on screen hands its tint to the subagent's row.
                let shows_sub = v.shown_sub.is_some_and(|(id, _)| id == t.id);
                let focused = t.id == v.current() && !shows_sub;
                let current = !shows_sub && v.panes.iter().any(|p| p.thread_id == t.id);
                // One row: `<logo> title ……… ⎇ 21m`. The logo is the status; the thread on
                // screen is tinted; the cursor lights the row.
                let base = if lit {
                    theme::pill()
                } else if current {
                    theme::surface()
                } else {
                    Style::default()
                };
                let on = |s: Style| if lit { theme::pill() } else { base.patch(s) };
                let (icon, icon_style) = status_icon(t, v.pulse);
                let mut right = if t.active_agent == SHELL { String::new() } else { age(v.now, t.updated_at) };
                if t.worktree.is_some() {
                    right = format!("⎇ {right}").trim_end().to_string();
                }
                let title = if t.name.is_empty() { "New thread" } else { t.name.as_str() };
                let title = truncate_end(title, w.saturating_sub(5 + right.width()));
                let pad = w.saturating_sub(4 + title.width() + right.width());
                lines.push(Line::from(vec![
                    Span::styled(" ", base),
                    Span::styled(icon, on(icon_style)),
                    Span::styled(" ", base),
                    Span::styled(title, on(if focused { theme::bold() } else { theme::normal() })),
                    Span::styled(" ".repeat(pad), base),
                    Span::styled(right, on(theme::muted())),
                    Span::styled(" ", base),
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
                let label = truncate_end(&label, w.saturating_sub(7));
                let pad = w.saturating_sub(6 + label.width());
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
                    Span::styled(if last { " └─ " } else { " ├─ " }, theme::muted()),
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
fn main_panel(f: &mut Frame, v: &View, p: &PaneView, focused: bool, area: Rect) {
    let mut block = panel(focused && v.cursor.is_none() && v.help.is_none());
    if focused && let Some(s) = &v.sub {
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
    if let Some(t) = v.threads.iter().find(|t| t.id == p.thread_id) {
        // A shell's header names where it started; an agent's, its project.
        let project = v.projects.iter().find(|p| p.id == t.project_id).map(|p| p.path.clone());
        let mut path = tilde(&if p.agent == SHELL { t.cwd.clone() } else { project.unwrap_or_else(|| t.cwd.clone()) });
        // A thread in its own worktree says which branch it works on.
        if t.worktree.is_some()
            && let Some(b) = &t.git.branch
        {
            path.push_str(&format!(" ⎇ {b}"));
        }
        let binding = t.bindings.get(p.agent);
        let mut model = binding.and_then(|b| b.model_choice.clone().or(b.model.clone())).unwrap_or_default();
        if let Some(e) = binding.and_then(|b| b.effort_choice.as_deref()) {
            model = if model.is_empty() { format!("{e} effort") } else { format!("{model} · {e}") };
        }
        let right = if p.scrolled > 0 {
            Line::from(Span::styled(format!(" ↑ {} lines · wheel down to follow ", p.scrolled), theme::warning()))
        } else {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(t.status.glyph().to_string(), glyph_style(t.status)),
                Span::styled(format!(" {} ", t.status.word()), theme::dim()),
                Span::styled(format!("{} ", t.id), theme::muted()),
            ])
        };
        let agent = if p.agent == SHELL { t.name.clone() } else { agent_title(p.agent) };
        let (icon, icon_style) = agent_icon(p.agent);
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
    } else if v.help.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in [("type", "filter"), ("↑↓", "move"), ("enter", "run"), ("esc", "close")] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else if v.finder.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in
            [("type", "filter"), ("↑↓", "move"), ("tab", "complete a path"), ("enter", "open a shell there"), ("esc", "cancel")]
        {
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
        for (k, what) in [("j/k", "move, the pane follows"), ("enter esc", "back to the agent"), ("1-9", "jump")] {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else {
        let mut spans = vec![
            Span::raw("  "),
            Span::styled("alt+j k", theme::bold()),
            Span::styled(" threads   ", theme::dim()),
            Span::styled(
                format!(" {} ", v.prefix),
                if v.prefix_armed { theme::pill() } else { theme::accent().add_modifier(Modifier::BOLD) },
            ),
            Span::raw("  "),
        ];
        for (k, what) in
            [("s", "switch agent"), ("c", "new"), ("%", "split"), ("w", "sidebar"), ("x", "close"), ("?", "help"), ("d", "detach")]
        {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
}

/// `prefix ?`: every key desl owns, in a list that runs the one picked. Typing filters it as
/// the directory finder does, `↑`/`↓` move, `enter` runs, `esc` closes. The rows and the notes
/// under them come from [`KEYS`], never written by hand.
fn help(f: &mut Frame, p: &PaletteView, prefix: &str, area: Rect) {
    let rows = palette_rows(p.query);
    let notes: Vec<&Bind> = KEYS.iter().filter(|b| b.action.is_none()).collect();
    let tail = notes.len() + DIRECT.len() + 2;
    let w = 96.min(area.width);
    let h = ((rows.len().max(1) + tail + 4) as u16).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 4, w, h);
    let block = panel(true).title(Line::from(Span::styled(format!(" Keys, after {prefix} "), theme::bold())));
    let inner = block.inner(r);
    f.render_widget(Clear, r);
    f.render_widget(block, r);
    let inner = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height);
    let iw = inner.width as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("❯ ", theme::accent()),
            Span::styled(p.query.to_string(), theme::normal()),
            Span::styled("▏", theme::accent()),
            if p.query.is_empty() { Span::styled("type to filter", theme::muted()) } else { Span::raw("") },
        ]),
        Line::from(Span::styled("─".repeat(iw), theme::muted())),
    ];
    let room = (inner.height as usize).saturating_sub(2 + tail).max(1);
    if rows.is_empty() {
        lines.push(Line::from(Span::styled(" No key matches. esc closes this.", theme::dim())));
    }
    let skip = p.cursor.saturating_sub(room.saturating_sub(1));
    let what_w = iw.saturating_sub(20);
    let mut group = "";
    for (i, b) in rows.iter().enumerate().skip(skip).take(room) {
        let lit = i == p.cursor;
        let g = if b.group == group { "" } else { b.group };
        group = b.group;
        let what = truncate_end(b.what, what_w);
        let pad = what_w.saturating_sub(what.width());
        let (gs, ks, ws) =
            if lit { (theme::pill(), theme::pill(), theme::pill()) } else { (theme::dim(), theme::accent(), theme::normal()) };
        lines.push(Line::from(vec![
            Span::styled(format!(" {g:<8} "), gs),
            Span::styled(format!("{:<8} ", b.key), ks),
            Span::styled(format!("{what}{} ", " ".repeat(pad)), ws),
        ]));
    }
    lines.push(Line::default());
    for b in notes {
        lines.push(Line::from(vec![Span::styled(format!("          {:<8} ", b.key), theme::dim()), Span::styled(b.what, theme::dim())]));
    }
    for (key, what) in DIRECT {
        lines.push(Line::from(vec![Span::styled(format!("          {key:<8} "), theme::dim()), Span::styled(*what, theme::dim())]));
    }
    lines.push(Line::from(Span::styled("          Every other key goes to the agent.", theme::dim())));
    f.render_widget(Paragraph::new(lines), inner);
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

/// A box over the pane: the query on top, the matching directories under it, the cursor's lit.
fn finder(f: &mut Frame, v: &FinderView, area: Rect) {
    let w = 76.min(area.width);
    let h = 24.min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 4, w, h);
    let count = if v.scanning { format!(" {} · looking… ", v.rows.len()) } else { format!(" {} ", v.rows.len()) };
    let block = panel(true)
        .title(Line::from(Span::styled(" Open a directory ", theme::bold())))
        .title(Line::from(Span::styled(count, theme::dim())).right_aligned());
    let inner = block.inner(r);
    f.render_widget(Clear, r);
    f.render_widget(block, r);
    let inner = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height);
    let iw = inner.width as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled("❯ ", theme::accent()),
            Span::styled(v.query.to_string(), theme::normal()),
            Span::styled("▏", theme::accent()),
            if v.query.is_empty() { Span::styled("a name, or a path like ~/src/", theme::muted()) } else { Span::raw("") },
        ]),
        Line::from(Span::styled("─".repeat(iw), theme::muted())),
    ];
    let room = (inner.height as usize).saturating_sub(2);
    if v.rows.is_empty() {
        let why = if v.scanning { "Looking…" } else { "No directory matches. Type a path, like ~/src/app." };
        lines.push(Line::from(Span::styled(format!(" {why}"), theme::dim())));
    }
    let skip = v.cursor.saturating_sub(room.saturating_sub(1));
    for (i, (path, note)) in v.rows.iter().enumerate().skip(skip).take(room) {
        let lit = i == v.cursor;
        let note_w = note.width();
        let path = truncate_start(path, iw.saturating_sub(note_w + 4));
        let gap = iw.saturating_sub(path.width() + note_w + 2);
        let (ps, ns) = if lit { (theme::pill(), theme::pill()) } else { (theme::normal(), theme::dim()) };
        lines.push(Line::from(vec![Span::styled(format!(" {path}{}", " ".repeat(gap)), ps), Span::styled(format!("{note} "), ns)]));
    }
    f.render_widget(Paragraph::new(lines), inner);
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
/// `screens` are the panes' agents' screens, in the panes' order.
pub fn draw(f: &mut Frame, v: &View, a: &Areas, screens: &[&vt100::Screen]) {
    if let Some(s) = a.sidebar {
        sidebar(f, v, s);
    }
    for (i, (main, area)) in a.mains.iter().zip(&a.panes).enumerate() {
        let Some(p) = v.panes.get(i) else { continue };
        let focused = i == v.focus;
        main_panel(f, v, p, focused, *main);
        match &v.sub {
            Some(s) if focused => {
                let h = area.height as usize;
                let top = s.lines.len().saturating_sub(h + s.scroll);
                let shown: Vec<Line<'static>> = s.lines.iter().skip(top).take(h).cloned().collect();
                f.render_widget(Clear, *area);
                f.render_widget(Paragraph::new(shown), *area);
            }
            _ => {
                if let Some(screen) = screens.get(i) {
                    pane(f.buffer_mut(), *area, screen);
                }
            }
        }
    }
    footer(f, v, a.footer);
    if let Some(m) = &v.menu {
        menu(f, m, a.body);
    }
    if let Some(fv) = &v.finder {
        finder(f, fv, a.body);
    }
    if let Some(rows) = v.info {
        info(f, rows, a.body);
    }
    if let Some(p) = &v.help {
        help(f, p, v.prefix, a.body);
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
        for b in KEYS {
            match b.action {
                Some(a) => assert_eq!(action(b.key.chars().next().unwrap()), Some(a), "{} runs something else", b.key),
                None => assert!(matches!(b.key, "prefix" | "1…9"), "{} does nothing", b.key),
            }
        }
        assert_eq!(action('3'), Some(Action::Jump(3)));
        assert_eq!(action('q'), None);
    }

    #[test]
    fn the_key_list_shows_every_key_and_finds_the_one_typed_for() {
        let all = palette_rows("");
        assert_eq!(all.len(), KEYS.iter().filter(|b| b.action.is_some()).count());
        assert_eq!(all[0].key, "s");
        assert_eq!(palette_rows("restart")[0].action, Some(Action::Restart));
        assert_eq!(palette_rows("detach")[0].action, Some(Action::Detach));
        assert_eq!(palette_rows("rename")[0].action, Some(Action::Rename));
        assert!(palette_rows("qqqq").is_empty());
    }

    #[test]
    fn two_panes_share_the_body_beside_or_stacked() {
        let a = areas(150, 40, true, Some(false));
        assert_eq!(a.mains.len(), 2);
        assert_eq!(a.mains[0].x, 45);
        assert_eq!(a.mains[0].width + a.mains[1].width, a.body.width);
        assert_eq!(a.mains[1].x, a.mains[0].x + a.mains[0].width);
        assert_eq!(a.panes[1].width, a.mains[1].width - 4);
        let s = areas(150, 40, true, Some(true));
        assert_eq!(s.mains[0].height + s.mains[1].height, s.body.height);
        assert_eq!(s.mains[1].y, s.mains[0].height);
        assert_eq!(areas(150, 40, true, None).mains, vec![a.body]);
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
        assert_eq!(r[2], Row::Sub("t-1".into(), 2), "the newest four only");
        assert_eq!(r[6], Row::Thread("t-2".into()), "no blank row between threads");
        let a = areas(120, 30, true, None);
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP + 2), Some(Row::Sub("t-1".into(), 2)));
        threads[2].subagents.clear();
        let r = rows(&projects, &threads);
        assert_eq!(r[0], Row::Project("a".into()));
        assert_eq!(r[2], Row::Thread("t-2".into()));
        assert_eq!((&r[3], &r[4]), (&Row::Gap, &Row::Project("b".into())), "one between projects");
        let a = areas(120, 30, true, None);
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP + 1), Some(Row::Thread("t-1".into())));
        assert_eq!(row_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP), Some(Row::Project("a".into())));
    }

    #[test]
    fn the_layout_gives_the_agent_what_the_chrome_leaves() {
        let a = areas(150, 40, true, None);
        assert_eq!(a.sidebar, Some(Rect::new(0, 0, 45, 39)));
        assert_eq!(a.mains[0], Rect::new(45, 0, 105, 39));
        assert_eq!(a.panes[0], Rect::new(47, 1, 101, 37));
        assert_eq!(areas(300, 80, true, None).sidebar.map(|r| r.width), Some(52));
        assert_eq!(areas(90, 24, true, None).sidebar.map(|r| r.width), Some(32));
        assert_eq!(a.footer, Rect::new(0, 39, 150, 1));
        assert_eq!(areas(79, 24, true, None).sidebar, None);
        assert_eq!(areas(100, 30, false, None).panes[0].width, 96);
    }

    #[test]
    fn the_screen_reads_at_100_by_30_and_paints_only_what_changed() {
        let projects = [project("desl")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "desl", 2, Status::Waiting, "")];
        let a = areas(120, 30, true, None);
        let mut vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        vt.process(b"\x1b[1mhello\x1b[0m from the agent");
        let mut v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0 }],
            focus: 0,
            cursor: None,
            skip: 0,
            prefix_armed: false,
            notice: None,
            help: None,
            menu: None,
            finder: None,
            sub: None,
            now: 0,
            shown_sub: None,
            renaming: None,
            info: None,
            pulse: 1.0,
            prefix: "ctrl+space",
        };
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| draw(f, &v, &a, &[vt.screen()])).unwrap();
        let s = screen(&t);
        assert_eq!(truncate_start("/tmp/a/b/demo", 8), "…/b/demo");
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("╭ Projects ") && lines[0].contains("1 waiting"), "{s}");
        assert!(lines[0].contains("╭ ✻ Claude · /src/desl · opus · high ") && lines[0].contains("○ idle t-1a ╮"), "{s}");
        assert!(lines[1].contains("││ hello from the agent"), "{s}");
        assert!(lines[2].contains("│ desl  /src "), "{s}");
        assert!(lines[3].contains("│  ✻ fix the login ") && lines[3].contains("now  ││"), "{s}");
        assert!(lines[4].contains("│  ✻ New thread "), "{s}");
        assert!(!s.contains("claude ·"), "the logo says the agent: {s}");
        // The thread on screen is tinted across its row; the others are not.
        let b = t.backend().buffer();
        assert_ne!(b[(20, 3)].bg, Color::Reset, "{s}");
        assert_eq!(b[(20, 4)].bg, Color::Reset, "{s}");
        assert_eq!(b[(3, 4)].fg, Color::Yellow, "a waiting thread's logo is the warning colour");
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
            panes: vec![PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0 }],
            focus: 0,
            cursor: None,
            skip: 0,
            prefix_armed: false,
            notice: None,
            help: None,
            menu: None,
            finder: None,
            sub: None,
            now: 0,
            shown_sub: Some(("t-1a", "s1")),
            renaming: None,
            info: None,
            pulse: 1.0,
            prefix: "ctrl+space",
        };
        t2.draw(|f| draw(f, &v2, &a, &[vt.screen()])).unwrap();
        let b2 = t2.backend().buffer();
        assert_eq!(b2[(20, 3)].bg, Color::Reset, "the thread gives up the tint");
        assert_ne!(b2[(20, 4)].bg, Color::Reset, "the subagent's row has it");
        assert!(lines[28].starts_with("╰"), "{s}");
        assert!(lines[29].contains(" ctrl+space   s switch agent"), "{s}");
        let before = t.backend().buffer().clone();
        let full = String::from_utf8(paint(None, &before)).unwrap();
        assert!(full.contains("\x1b[2J") && full.contains("hello"), "{full:?}");
        let items = [
            MenuItem { label: "Claude".into(), note: "agent".into(), enabled: true },
            MenuItem { label: "bash".into(), note: "shell".into(), enabled: true },
        ];
        v.menu = Some(Menu { title: "New in desl", items: &items, cursor: 1 });
        t.draw(|f| draw(f, &v, &a, &[vt.screen()])).unwrap();
        let s = screen(&t);
        assert!(s.contains("╭ New in desl ") && s.contains(" 1  Claude  agent") && s.contains(" 2  bash    shell"), "{s}");
        assert!(s.lines().last().unwrap().contains("enter start"), "{s}");
        v.menu = None;
        v.help = Some(PaletteView { query: "", cursor: 0 });
        t.draw(|f| draw(f, &v, &a, &[vt.screen()])).unwrap();
        assert!(screen(&t).contains("Every other key goes to the agent."));
        let diff = String::from_utf8(paint(Some(&before), t.backend().buffer())).unwrap();
        assert!(!diff.contains("\x1b[2J") && diff.contains("Switch") && diff.contains("agent") && !diff.contains("projects"), "{diff:?}");
    }
}
