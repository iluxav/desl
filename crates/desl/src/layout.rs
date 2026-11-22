//! The workspace screen (SPEC.md "Interface design"): projects and their threads on the left,
//! the shown thread's agent on the right under a one-line header, and the common keys along the
//! bottom. The agent draws into desl's own terminal emulator, which is copied into the pane.

use crate::ui::text::truncate_end;
use crate::{icons, theme};
use desl_ir::{FILES, GitState, Project, SHELL, Status, TASKS, Thread, is_agent};
use desl_proto::view::{self, SubState};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

/// Shared with `desl web`, so both clients order and word the workspace alike.
pub use desl_proto::view::{age, order, tilde};

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
    History,
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
    Web,
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
    bind("Agent", "s", "Switch this thread to another agent", Some(Action::Switch)),
    bind("Agent", "f", "Fork: this conversation in a new thread, on either agent", Some(Action::Fork)),
    bind("Agent", "[", "History: the whole conversation, from its first prompt", Some(Action::History)),
    bind("Agent", "i", "Info: agent, session ids, model, directory", Some(Action::Info)),
    bind("Agent", "r", "Restart this agent, keeping the conversation", Some(Action::Restart)),
    bind("Threads", "w", "Move to the thread list (j/k, enter, esc)", Some(Action::Threads)),
    bind("Threads", "c", "New thread: an agent, a shell, a file manager or the task list", Some(Action::New)),
    bind("Threads", "o", "Open a directory: type to find it; a shell opens there", Some(Action::Open)),
    bind("Threads", "n", "Next thread", Some(Action::Next)),
    bind("Threads", "p", "Previous thread", Some(Action::Prev)),
    bind("Threads", "1…9", "Jump to thread n", None),
    bind("Threads", "l", "Back to the thread shown before", Some(Action::Last)),
    bind("Threads", ",", "Rename this thread", Some(Action::Rename)),
    bind("Threads", "x", "Close this thread: its agent, shell or file manager ends", Some(Action::Close)),
    bind("Panes", "%", "Split: another thread beside this one", Some(Action::SplitBeside)),
    bind("Panes", "\"", "Split: another thread under this one", Some(Action::SplitStacked)),
    bind("Panes", "z", "Zoom: back to one pane, this one", Some(Action::Zoom)),
    bind("desl", "b", "Hide or show the sidebar", Some(Action::Sidebar)),
    bind("desl", "d", "Detach; the agents keep working", Some(Action::Detach)),
    bind("desl", "v", "View the workspace in the browser (desl web)", Some(Action::Web)),
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
        out.extend(view::subagents_shown(t).map(|k| Row::Sub(t.id.clone(), k)));
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

/// A conversation shown in the pane instead of the agent's screen.
pub struct SubView<'a> {
    pub title: String,
    pub kind: SubKind,
    pub lines: &'a [Line<'static>],
    /// The line at the top of the pane.
    pub top: usize,
    /// The line the last search stopped on, drawn reversed.
    pub hit: Option<usize>,
    /// A search being typed: its text, and whether it goes down the page.
    pub typing: Option<(&'a str, bool)>,
    /// Tool calls are shown.
    pub tools: bool,
}

/// Whose conversation a [`SubView`] shows.
pub enum SubKind {
    /// One of the thread's subagents.
    Subagent { running: bool },
    /// The thread's own, from its first prompt (`prefix [`). `place` is where the reader is.
    History { place: String },
}

/// One choice in a menu: what it starts, and why it cannot when it cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub note: String,
    pub enabled: bool,
    /// What the row starts (an agent's id, a shell, files, tasks), for its mark; empty for none.
    pub kind: String,
}

pub struct Menu<'a> {
    pub title: &'a str,
    pub items: &'a [MenuItem],
    pub cursor: usize,
    /// What `enter` does to the row, for the key line: `start`, `switch`.
    pub verb: &'a str,
    /// The row can get a git worktree of its own (`w`): a new thread can, a switch cannot.
    pub worktree: bool,
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
    /// The text marked with the mouse, drawn reversed.
    pub selection: Option<Selection>,
}

/// Text marked in a pane: from one cell to another in reading order, both included, each a
/// (row, column) on the pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub from: (u16, u16),
    pub to: (u16, u16),
}

impl Selection {
    /// From the cell the drag began on to the one it is on now, whichever comes first.
    pub fn between(a: (u16, u16), b: (u16, u16)) -> Selection {
        if a <= b { Selection { from: a, to: b } } else { Selection { from: b, to: a } }
    }

    pub fn holds(&self, row: u16, col: u16) -> bool {
        (row, col) >= self.from && (row, col) <= self.to
    }

    /// The marked text as the screen holds it: a line that wrapped is one line again.
    pub fn text(&self, screen: &vt100::Screen) -> String {
        screen.contents_between(self.from.0, self.from.1, self.to.0, self.to.1 + 1)
    }

    /// The run of non-blank cells around (`row`, `col`), for a double click: a word, and a whole
    /// path or address. None on a blank cell.
    pub fn word(screen: &vt100::Screen, row: u16, col: u16) -> Option<Selection> {
        let blank = |c: u16| screen.cell(row, c).is_none_or(|cell| !cell.is_wide_continuation() && cell.contents().trim().is_empty());
        if blank(col) {
            return None;
        }
        let (_, cols) = screen.size();
        let mut from = col;
        while from > 0 && !blank(from - 1) {
            from -= 1;
        }
        let mut to = col;
        while to + 1 < cols.get() && !blank(to + 1) {
            to += 1;
        }
        Some(Selection { from: (row, from), to: (row, to) })
    }
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

fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::dim(),
        Status::Failed => theme::critical(),
    }
}

/// A rounded panel, its border lit when it has the keys.
fn panel(focused: bool) -> Block<'static> {
    Block::bordered().border_type(BorderType::Rounded).border_style(if focused { theme::accent() } else { theme::muted() })
}

/// Each agent's mark: with a Nerd Font, the Claude, OpenAI and Copilot logos, a terminal for a
/// shell, a folder for a file manager and a checklist for the task list; without, a one-cell
/// badge in the agent's colour.
pub fn agent_icon(agent: &str) -> (&'static str, Style) {
    use ratatui::style::Color;
    let badge =
        |fg: Color, bg: Color| if theme::nerd() { Style::default() } else { Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD) };
    match (agent, theme::nerd()) {
        ("claude", true) => ("\u{ec82}", theme::claude()),
        ("codex", true) => ("\u{ec81}", theme::normal()),
        ("copilot", true) => ("\u{ec1e}", theme::copilot()),
        (SHELL, true) => ("\u{ebca}", theme::dim()),
        (FILES, true) => ("\u{ea83}", theme::dim()),
        (TASKS, true) => ("\u{eab3}", theme::dim()),
        ("claude", false) => ("✻", badge(Color::Black, Color::Rgb(217, 119, 87))),
        ("codex", false) => ("›", badge(Color::Black, Color::Rgb(220, 220, 220))),
        ("copilot", false) => ("∞", badge(Color::Black, Color::Rgb(163, 113, 247))),
        (SHELL, false) => ("$", badge(Color::White, Color::Rgb(70, 70, 70))),
        (FILES, false) => ("/", badge(Color::Black, Color::Rgb(150, 150, 150))),
        (TASKS, false) => ("✓", badge(Color::Black, Color::Rgb(150, 150, 150))),
        (_, true) => ("\u{ec20}", theme::dim()),
        (_, false) => ("◆", theme::dim()),
    }
}

/// An agent's own colour: Claude's orange, Copilot's violet, Codex in the terminal's text
/// colour; a shell and a file manager grey.
pub fn agent_rgb(agent: &str) -> (u8, u8, u8) {
    match agent {
        "claude" => (217, 119, 87),
        "copilot" => (163, 113, 247),
        SHELL | FILES | TASKS => (150, 150, 150),
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
        "copilot" => "∞",
        SHELL => "$",
        FILES => "/",
        TASKS => "✓",
        _ => "◆",
    }
}

fn sidebar(f: &mut Frame, v: &View, area: Rect, stacked: Option<bool>) {
    let waiting = view::waiting(v.projects, v.threads).len();
    let mut block = panel(v.cursor.is_some()).title(Line::from(Span::styled(" Projects ", theme::bold())));
    if waiting > 0 {
        let text = match icons::waiting() {
            Some(bell) => format!(" {bell} {waiting} waiting "),
            None => format!(" {waiting} waiting "),
        };
        block = block.title(Line::from(Span::styled(text, theme::warning())).right_aligned());
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
                let p = v.projects.iter().find(|p| p.id == *id);
                let path = p.map_or(id.clone(), |p| p.path.clone());
                let mut line = project_row(&path, p.map(|p| &p.git), w);
                // The cursor lights the whole row, as it does a thread's.
                if v.cursor == Some(i) {
                    let used: usize = line.spans.iter().map(|s| s.content.width()).sum();
                    line.spans.push(Span::raw(" ".repeat(w.saturating_sub(used))));
                    for s in &mut line.spans {
                        s.style = theme::pill();
                    }
                }
                lines.push(line);
            }
            Row::Thread(id) => {
                let Some(t) = v.threads.iter().find(|t| t.id == *id) else { continue };
                // A thread whose subagent is on screen hands its tint to the subagent's row.
                let shows_sub = v.shown_sub.is_some_and(|(id, _)| id == t.id);
                // The pane it is in, if it is on screen: the one with the keys is lit in the
                // accent, as its pane's border is, the other tinted grey; each says which pane.
                let pane = v.panes.iter().position(|p| p.thread_id == t.id).filter(|_| !shows_sub);
                let focused = pane == Some(v.focus);
                // One row: `▌<logo> title ……… ◨ ⎇ 21m`. The logo is the status; the cursor
                // lights the row.
                let base = if lit {
                    theme::pill()
                } else if focused {
                    theme::focus_surface()
                } else if pane.is_some() {
                    theme::surface()
                } else {
                    Style::default()
                };
                let on = |s: Style| if lit { theme::pill() } else { base.patch(s) };
                let (icon, icon_style) = status_icon(t, v.pulse);
                let mut right = if is_agent(&t.active_agent) { age(v.now, t.updated_at) } else { String::new() };
                if t.worktree.is_some() {
                    right = format!("{} {right}", icons::branch()).trim_end().to_string();
                }
                let mark = match (pane, stacked) {
                    (Some(i), Some(stacked)) => format!("{} ", icons::pane(i > 0, stacked)),
                    _ => String::new(),
                };
                let title = truncate_end(view::title(t), w.saturating_sub(5 + mark.width() + right.width()));
                let pad = w.saturating_sub(4 + title.width() + mark.width() + right.width());
                let (edge, edge_style) = match pane {
                    Some(_) if focused => ("▌", theme::accent()),
                    Some(_) => ("▌", theme::muted()),
                    None => (" ", Style::default()),
                };
                lines.push(Line::from(vec![
                    Span::styled(edge, on(edge_style)),
                    Span::styled(icon, on(icon_style)),
                    Span::styled(" ", base),
                    Span::styled(title, on(if focused { theme::bold() } else { theme::normal() })),
                    Span::styled(" ".repeat(pad), base),
                    Span::styled(mark, on(if focused { theme::accent() } else { theme::dim() })),
                    Span::styled(right, on(if focused { theme::dim() } else { theme::muted() })),
                    Span::styled(" ", base),
                ]));
            }
            Row::Sub(id, k) => {
                let Some(t) = v.threads.iter().find(|t| t.id == *id) else { continue };
                let Some(s) = t.subagents.get(*k) else { continue };
                let (glyph, style) = match view::sub_state(s, t) {
                    SubState::Done => (icons::sub_done(), theme::muted()),
                    SubState::Running => (icons::sub_running(), theme::accent()),
                    SubState::Unfinished => (icons::sub_unfinished(), theme::muted()),
                };
                let last = !matches!(rows.get(i + 1), Some(Row::Sub(next, _)) if next == id);
                let label = view::sub_label(s);
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

/// A project's row: the folder's name, the branch checked out there and how many paths are
/// uncommitted, then dimmer where it lives: `desl ⎇ main ±4  ~/Projects`. On a row too short for
/// all of it the directory goes first, then the branch is cut; the name and the count stay.
fn project_row(path: &str, git: Option<&GitState>, w: usize) -> Line<'static> {
    let p = std::path::Path::new(path);
    let name = p.file_name().map_or(path.to_string(), |n| n.to_string_lossy().into_owned());
    let parent = p.parent().map(|d| tilde(&d.to_string_lossy())).unwrap_or_default();
    // The project's own colour, on its icon (a repository's or a folder's; a dot without a
    // Nerd Font): the same icon in a pane's title says which project the pane is in.
    let mark = icons::project(git.is_some_and(|g| g.branch.is_some()));
    let mut spans = Vec::new();
    let mut used = 0;
    if w >= 12 {
        spans.push(Span::styled(format!("{mark} "), theme::project_hue(path)));
        used += 2;
    }
    let name = truncate_end(&name, w.saturating_sub(used));
    used += name.width();
    spans.push(Span::styled(name, theme::bold()));
    let dirty = git.filter(|g| g.dirty > 0).map(|g| format!(" {}", icons::changed(g.dirty as usize))).unwrap_or_default();
    if let Some(b) = git.and_then(|g| g.branch.as_deref()) {
        // ` ⎇ ` and at least two characters of the branch, or none of it.
        let room = w.saturating_sub(used + dirty.width() + 3);
        if room >= 2 {
            let b = truncate_end(b, room);
            used += 3 + b.width();
            spans.push(Span::styled(format!(" {} {b}", icons::branch()), theme::branch()));
        }
    }
    if !dirty.is_empty() && used + dirty.width() <= w {
        used += dirty.width();
        spans.push(Span::styled(dirty, theme::changes()));
    }
    let room = w.saturating_sub(used + 2);
    if room >= 6 && !parent.is_empty() {
        spans.push(Span::styled(format!("  {}", truncate_start(&parent, room)), theme::muted()));
    }
    Line::from(spans)
}

/// The agent's panel: its border, with the project, agent and model on top and the thread's
/// id and status on the right.
/// In a thread's header while its agent may do anything without asking.
const YOLO: &str = "⚠ yolo ";

fn main_panel(f: &mut Frame, v: &View, p: &PaneView, focused: bool, area: Rect) {
    let mut block = panel(focused && v.cursor.is_none() && v.help.is_none());
    if focused && let Some(s) = &v.sub {
        let (what, state) = match &s.kind {
            SubKind::Subagent { running: true } => {
                ("Subagent", Span::styled(format!(" {} running ", icons::sub_running()), theme::accent()))
            }
            SubKind::Subagent { running: false } => ("Subagent", Span::styled(format!(" {} done ", icons::sub_done()), theme::dim())),
            SubKind::History { place } => ("History", Span::styled(format!(" {place} "), theme::dim())),
        };
        block = block
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(what, theme::accent().add_modifier(Modifier::BOLD)),
                Span::styled(" · ", theme::muted()),
                Span::styled(s.title.clone(), theme::normal()),
                Span::raw(" "),
            ]))
            .title(Line::from(state).right_aligned());
        f.render_widget(block, area);
        return;
    }
    if let Some(t) = v.threads.iter().find(|t| t.id == p.thread_id) {
        // The thread's own title first: what tells two agents of one project apart. Then its
        // project, by the icon in the project's colour the sidebar shows, then the model.
        let project = v.projects.iter().find(|x| x.id == t.project_id);
        let root = project.map_or(t.cwd.clone(), |x| x.path.clone());
        let name = std::path::Path::new(&root).file_name().map_or(root.clone(), |n| n.to_string_lossy().into_owned());
        // An agent works in its project; a shell or a file manager is wherever it is now.
        let place = if is_agent(p.agent) {
            name.clone()
        } else {
            match t.cwd.strip_prefix(root.as_str()) {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{name}{rest}"),
                _ => tilde(&t.cwd),
            }
        };
        // A thread in its own worktree says which branch it works on, and what is uncommitted there.
        let (branch, dirty) = match (&t.worktree, &t.git.branch) {
            (Some(_), Some(b)) => (
                format!(" {} {b}", icons::branch()),
                if t.git.dirty > 0 { format!(" {}", icons::changed(t.git.dirty as usize)) } else { String::new() },
            ),
            _ => (String::new(), String::new()),
        };
        let mut model = view::model(t.bindings.get(p.agent));
        if let Some(chip) = icons::model().filter(|_| !model.is_empty()) {
            model = format!("{chip} {model}");
        }
        let title = view::title(t).to_string();
        let yolo = view::yolo(t);
        let (icon, icon_style) = agent_icon(p.agent);
        let chip = icons::project(project.is_some_and(|x| x.git.branch.is_some()));
        let git = format!("{branch}{dirty}");
        // What gives way when the pane is narrow, in order: the model, the branch, the status
        // word, the length of the place, the thread id, the place (its coloured mark stays), and
        // only then the thread's title, which is what tells the panes apart.
        let room = (area.width as usize).saturating_sub(3);
        let fit = |level: usize, place_w: usize| -> usize {
            let right = match (p.scrolled > 0, level) {
                (true, _) => format!(" ↑ {} lines · wheel down to follow ", p.scrolled).width(),
                (false, 0..=2) => 4 + t.status.word().width() + 1 + t.id.width() + 1,
                (false, 3..=4) => 3 + t.id.width() + 1,
                _ => 3,
            } + if yolo && p.scrolled == 0 { YOLO.width() } else { 0 };
            let mut w = 3 + title.width() + 3 + 2 + place_w + 1 + right;
            if level < 2 {
                w += git.width();
            }
            if level < 1 && !model.is_empty() {
                w += 3 + model.width();
            }
            w
        };
        let mut level = 0;
        let mut place_w = place.width();
        while level < 6 && fit(level, place_w) > room {
            level += 1;
            if level == 4 {
                place_w = place_w.min(room.saturating_sub(fit(4, 0)).max(6));
            }
            if level == 6 {
                place_w = 0;
            }
        }
        let place = if place_w == 0 { String::new() } else { truncate_start(&place, place_w) };
        let title_room = room.saturating_sub(fit(level, place.width()) - title.width()).max(6);
        let title = truncate_end(&title, title_room);
        let right = if p.scrolled > 0 {
            Line::from(Span::styled(format!(" ↑ {} lines · wheel down to follow ", p.scrolled), theme::warning()))
        } else {
            let mut r = vec![Span::raw(" ")];
            // Never dropped for room: the agent asks nothing before it acts.
            if yolo {
                r.push(Span::styled(YOLO, theme::warning().add_modifier(Modifier::BOLD)));
            }
            r.push(Span::styled(icons::status(t.status).to_string(), glyph_style(t.status)));
            if level <= 2 {
                r.push(Span::styled(format!(" {}", t.status.word()), theme::dim()));
            }
            if level <= 4 {
                r.push(Span::styled(format!(" {}", t.id), theme::muted()));
            }
            r.push(Span::raw(" "));
            Line::from(r)
        };
        // The pane with the keys has its title in the accent; the other one is quieter.
        let (title_style, quiet) = if focused {
            (theme::accent().add_modifier(Modifier::BOLD), theme::normal())
        } else {
            (theme::dim().add_modifier(Modifier::BOLD), theme::muted())
        };
        let mut left = vec![
            Span::raw(" "),
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(title, title_style),
            Span::styled(" · ", theme::muted()),
            Span::styled(format!("{chip} "), theme::project_hue(&root)),
            Span::styled(place, quiet),
        ];
        if level < 2 {
            left.push(Span::styled(branch, theme::branch()));
            left.push(Span::styled(dirty, theme::changes()));
        }
        if level < 1 && !model.is_empty() {
            left.push(Span::styled(" · ", theme::muted()));
            left.push(Span::styled(model, if focused { theme::dim() } else { theme::muted() }));
        }
        left.push(Span::raw(" "));
        block = block.title(Line::from(left)).title(right.right_aligned());
    }
    f.render_widget(block, area);
}

/// `keys` after `spans`, each a key and what it does: as many whole ones as `width` columns
/// hold, so a narrow terminal shows fewer keys and none of them cut in two.
fn with_keys(mut spans: Vec<Span<'static>>, keys: &[(&'static str, &'static str)], width: u16) -> Line<'static> {
    let mut used: usize = spans.iter().map(|s| s.content.width()).sum();
    for (k, what) in keys {
        used += k.width() + 1 + what.width();
        if used > usize::from(width) {
            break;
        }
        used += 3;
        spans.push(Span::styled(*k, theme::bold()));
        spans.push(Span::styled(format!(" {what}   "), theme::dim()));
    }
    Line::from(spans)
}

fn footer(f: &mut Frame, v: &View, area: Rect) {
    let line = if let Some(n) = v.notice {
        Line::from(vec![Span::styled(format!("  {} ", icons::notice()), theme::warning()), Span::styled(n.to_string(), theme::normal())])
    } else if let Some(name) = v.renaming {
        Line::from(vec![
            Span::styled("  Rename ", theme::bold()),
            Span::styled(name.to_string(), theme::normal()),
            Span::styled("▏", theme::accent()),
            Span::styled("   enter save   esc cancel", theme::dim()),
        ])
    } else if let Some((text, down)) = v.sub.as_ref().and_then(|s| s.typing) {
        Line::from(vec![
            Span::styled(if down { "  Find below " } else { "  Find above " }, theme::bold()),
            Span::styled(text.to_string(), theme::normal()),
            Span::styled("▏", theme::accent()),
            Span::styled("   enter find   esc cancel", theme::dim()),
        ])
    } else if let Some(s) = &v.sub {
        let tools = if s.tools { "hide tool calls" } else { "show tool calls" };
        let keys = [
            ("esc", "back to the agent"),
            ("j/k", "scroll"),
            ("{ }", "prompts"),
            ("/ ?", "find below, above"),
            ("n N", "next"),
            ("t", tools),
            ("g/G", "top/end"),
        ];
        with_keys(vec![Span::raw("  ")], &keys, area.width)
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
    } else if let Some(m) = &v.menu {
        let mut spans = vec![Span::raw("  ")];
        let worktree = m.worktree.then_some(("w", "own worktree"));
        for (k, what) in [("j/k", "move"), ("enter", m.verb), ("1-9", "pick")].into_iter().chain(worktree).chain([("esc", "cancel")]) {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else if v.cursor.is_some() {
        let mut spans = vec![Span::raw("  ")];
        for (k, what) in
            [("j/k", "move, the pane follows"), ("shift+↑↓", "move a project"), ("enter esc", "back to the agent"), ("1-9", "jump")]
        {
            spans.push(Span::styled(k, theme::bold()));
            spans.push(Span::styled(format!(" {what}   "), theme::dim()));
        }
        Line::from(spans)
    } else {
        let spans = vec![
            Span::raw("  "),
            Span::styled("alt+j k", theme::bold()),
            Span::styled(" threads   ", theme::dim()),
            Span::styled(
                format!(" {} ", v.prefix),
                if v.prefix_armed { theme::pill() } else { theme::accent().add_modifier(Modifier::BOLD) },
            ),
            Span::raw("  "),
        ];
        // The newest key is last: a terminal too narrow for all of them loses that one first.
        let keys = [
            ("s", "switch agent"),
            ("c", "new"),
            ("%", "split"),
            ("w", "sidebar"),
            ("x", "close"),
            ("?", "help"),
            ("d", "detach"),
            ("[", "history"),
        ];
        with_keys(spans, &keys, area.width)
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
    // With a Nerd Font, each label has its icon: two more columns.
    let icon_w = if icons::fact("Thread").is_some() { 2 } else { 0 };
    let label_w = rows.iter().map(|(l, _)| l.width()).max().unwrap_or(0) + icon_w;
    let value_w = rows.iter().map(|(_, v)| v.width()).max().unwrap_or(0);
    let w = ((label_w + value_w + 9) as u16).min(area.width);
    let h = (rows.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 3, w, h);
    let inner_w = (w as usize).saturating_sub(6 + label_w);
    let lines: Vec<Line<'static>> = rows
        .iter()
        .map(|(l, v)| {
            let label = match icons::fact(l) {
                Some(i) => format!("{i} {l}"),
                None => l.clone(),
            };
            Line::from(vec![Span::styled(format!("{label:<label_w$}  "), theme::dim()), Span::raw(truncate_start(v, inner_w))])
        })
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
        // With a Nerd Font, what the row is shows as an icon too, before the path.
        let icon = icons::finder(note).map(|i| format!(" {i}")).unwrap_or_default();
        let path = truncate_start(path, iw.saturating_sub(note_w + 4 + icon.width()));
        let gap = iw.saturating_sub(path.width() + note_w + 2 + icon.width());
        let (ps, ns) = if lit { (theme::pill(), theme::pill()) } else { (theme::normal(), theme::dim()) };
        lines.push(Line::from(vec![
            Span::styled(icon, if lit { theme::pill() } else { theme::dim() }),
            Span::styled(format!(" {path}{}", " ".repeat(gap)), ps),
            Span::styled(format!("{note} "), ns),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// A small box in the middle of the pane: numbered choices, the cursor's lit.
fn menu(f: &mut Frame, m: &Menu, area: Rect) {
    let label_w = m.items.iter().map(|i| i.label.width()).max().unwrap_or(0);
    let note_w = m.items.iter().map(|i| i.note.width()).max().unwrap_or(0);
    // With a Nerd Font each row carries its agent's mark: two more columns.
    let marks = theme::nerd() && m.items.iter().any(|i| !i.kind.is_empty());
    let w = ((label_w + note_w + 13 + if marks { 2 } else { 0 }) as u16).max(m.title.width() as u16 + 6).min(area.width);
    let h = (m.items.len() as u16 + 4).min(area.height);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 3, w, h);
    let inner_w = w.saturating_sub(4) as usize;
    let lines: Vec<Line<'static>> = m
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let lit = i == m.cursor;
            let style = match (lit, item.enabled) {
                (true, _) => theme::pill(),
                (false, true) => theme::normal(),
                (false, false) => theme::muted(),
            };
            if !marks {
                let text = format!(" {}  {:<label_w$}  {}", i + 1, item.label, item.note);
                let text = format!("{text:<inner_w$}");
                return Line::from(Span::styled(truncate_end(&text, inner_w), style));
            }
            let (icon, icon_style) = if item.kind.is_empty() { (" ", Style::default()) } else { agent_icon(&item.kind) };
            let icon_style = if lit {
                theme::pill()
            } else if item.enabled {
                icon_style
            } else {
                theme::muted()
            };
            let head = format!(" {}  ", i + 1);
            let text = format!(" {:<label_w$}  {}", item.label, item.note);
            let text = truncate_end(&text, inner_w.saturating_sub(head.width() + 1));
            let pad = inner_w.saturating_sub(head.width() + 1 + text.width());
            Line::from(vec![
                Span::styled(head, style),
                Span::styled(icon, icon_style),
                Span::styled(format!("{text}{}", " ".repeat(pad)), style),
            ])
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

/// Copies the agent's screen into `area`, the marked text reversed.
pub fn pane(buf: &mut Buffer, area: Rect, screen: &vt100::Screen, selection: Option<Selection>) {
    for r in 0..area.height {
        for c in 0..area.width {
            let out = &mut buf[(area.x + c, area.y + r)];
            let marked = selection.is_some_and(|s| s.holds(r, c));
            let Some(cell) = screen.cell(r, c) else {
                out.reset();
                continue;
            };
            if cell.is_wide_continuation() {
                out.reset();
                if marked {
                    out.modifier = Modifier::REVERSED;
                }
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
            // A cell the program already reversed is put back, so the mark shows on it too.
            if marked {
                m ^= Modifier::REVERSED;
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
        // Two panes side by side or stacked, for the mark that ties a row to its pane.
        let stacked = (a.mains.len() > 1).then(|| a.mains[0].x == a.mains[1].x);
        sidebar(f, v, s, stacked);
    }
    for (i, (main, area)) in a.mains.iter().zip(&a.panes).enumerate() {
        let Some(p) = v.panes.get(i) else { continue };
        let focused = i == v.focus;
        main_panel(f, v, p, focused, *main);
        match &v.sub {
            Some(s) if focused => {
                let h = area.height as usize;
                let mut shown: Vec<Line<'static>> = s.lines.iter().skip(s.top).take(h).cloned().collect();
                // What a search found, reversed for the eye to land on.
                if let Some(line) = s.hit.and_then(|hit| hit.checked_sub(s.top)).and_then(|i| shown.get_mut(i)) {
                    for span in &mut line.spans {
                        span.style = span.style.add_modifier(Modifier::REVERSED);
                    }
                }
                f.render_widget(Clear, *area);
                f.render_widget(Paragraph::new(shown), *area);
                if let Some(marked) = p.selection {
                    let buf = f.buffer_mut();
                    for (row, col) in (0..area.height).flat_map(|r| (0..area.width).map(move |c| (r, c))) {
                        if marked.holds(row, col) {
                            buf[(area.x + col, area.y + row)].modifier.insert(Modifier::REVERSED);
                        }
                    }
                }
            }
            _ => {
                if let Some(screen) = screens.get(i) {
                    pane(f.buffer_mut(), *area, screen, p.selection);
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
        Project { id: id.into(), path: format!("/src/{id}"), default_agent: None, created_at: 0, git: GitState::default() }
    }

    #[test]
    fn a_project_row_keeps_its_name_and_count_and_drops_where_it_lives_first() {
        let text =
            |git: &GitState, w: usize| project_row("/src/desl", Some(git), w).spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        let g = GitState { branch: Some("main".into()), dirty: 4, ..GitState::default() };
        assert_eq!(text(&g, 48), "● desl ⎇ main ±4  /src");
        assert_eq!(text(&g, 17), "● desl ⎇ main ±4", "no room for where it lives");
        assert_eq!(text(&g, 15), "● desl ⎇ ma… ±4", "the branch is cut before the count");
        assert_eq!(text(&GitState { dirty: 0, ..g.clone() }, 48), "● desl ⎇ main  /src", "a clean tree shows no count");
        assert_eq!(text(&GitState::default(), 48), "● desl  /src", "outside a repository, as before");
        let row = project_row("/src/desl", Some(&g), 48);
        assert_eq!(row.spans[0].style, theme::project_hue("/src/desl"), "the project's own colour, on its mark");
        assert_eq!(row.spans[2].style.fg, Some(Color::Magenta), "the branch is rose");
        assert_eq!(row.spans[3].style.fg, Some(Color::Green), "the count is green");
    }

    fn screen(t: &Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// web-spec.md "Indicators": a parity check, a release gate. The page draws what
    /// `desl_proto::view` works out, word for word, so each of those words must be on the
    /// terminal's screen too: the waiting count, each project's name, branch and count, each
    /// thread's title and age, each subagent, and the header's model, status, id and `⚠ yolo`.
    #[test]
    fn the_page_and_the_terminal_say_the_same() {
        let mut projects = vec![project("desl"), project("rebrand")];
        projects[0].git = GitState { branch: Some("main".into()), dirty: 2, ..GitState::default() };
        let mut threads = vec![
            thread("t-1a", "desl", 1, Status::Working, "Web client"),
            thread("t-2b", "desl", 2, Status::Waiting, "History view"),
            thread("t-3c", "rebrand", 3, Status::Idle, ""),
        ];
        threads[0].subagents =
            vec![desl_ir::Subagent { id: "s1".into(), kind: "Explore".into(), description: "price hosts".into(), ..Default::default() }];
        threads[0].updated_at = 2 * 60_000;
        threads[1].permission_mode = desl_ir::PermissionMode::Yolo;
        let now = 23 * 60_000;
        let page = view::sidebar(&projects, &threads, now, view::on_page);
        let a = areas(120, 30, true, None);
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView { thread_id: "t-2b", agent: "claude", scrolled: 0, selection: None }],
            focus: 0,
            cursor: None,
            skip: 0,
            prefix_armed: false,
            notice: None,
            help: None,
            menu: None,
            finder: None,
            sub: None,
            now,
            shown_sub: None,
            renaming: None,
            info: None,
            pulse: 1.0,
            prefix: "ctrl+space",
        };
        let mut t = Terminal::new(TestBackend::new(120, 30)).unwrap();
        t.draw(|f| draw(f, &v, &a, &[vt.screen()])).unwrap();
        let s = screen(&t);
        assert!(s.contains(&format!("{} waiting", page.waiting.len())), "{s}");
        let mut words = 0;
        for p in &page.projects {
            let mut said = vec![p.name.clone(), p.branch.as_ref().map(|b| format!("⎇ {b}")).unwrap_or_default()];
            if p.dirty > 0 {
                said.push(format!("±{}", p.dirty));
            }
            for th in &p.threads {
                // A thread's title and its age sit on one row of the sidebar, under the headers.
                let row =
                    s.lines().skip(1).find(|l| l.contains(&th.title)).unwrap_or_else(|| panic!("{} is not on the screen:\n{s}", th.title));
                assert!(row.contains(&format!(" {} ", th.age)), "{row}");
                said.extend(th.subagents.iter().map(|x| x.label.clone()));
                words += 2;
            }
            for w in said.iter().filter(|w| !w.is_empty()) {
                assert!(s.contains(w.as_str()), "the page says {w:?}, the terminal does not:\n{s}");
                words += 1;
            }
        }
        let shown = page.projects.iter().flat_map(|p| &p.threads).find(|th| th.id == "t-2b").unwrap();
        let header = s.lines().next().unwrap_or_default();
        for w in [shown.header.model.as_str(), shown.word, shown.id.as_str(), "⚠ yolo"] {
            assert!(header.contains(w), "the header lacks {w:?}: {header}");
            words += 1;
        }
        assert_eq!(words, 15, "every word was looked for");
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
    fn a_files_thread_is_titled_by_its_file_manager_and_shows_where_it_opened() {
        let projects = [project("desl")];
        let mut files = thread("t-2b", "desl", 2, Status::Idle, "yazi");
        files.active_agent = FILES.into();
        files.cwd = "/src/desl/docs".into();
        files.bindings.clear();
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), files];
        let a = areas(120, 30, true, None);
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView { thread_id: "t-2b", agent: FILES, scrolled: 0, selection: None }],
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
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].contains("╭ / yazi · ● desl/docs ") && lines[0].contains("○ idle t-2b ╮"), "{s}");
        assert!(lines[4].contains("│ ▌/ yazi ") && !lines[4].contains("now"), "no age, as a shell: {s}");
    }

    #[test]
    fn copilot_has_its_own_mark_and_a_switch_menu_says_what_enter_does() {
        let projects = [project("desl")];
        let mut copilot = thread("t-1a", "desl", 1, Status::Idle, "fix the login");
        copilot.active_agent = "copilot".into();
        let threads = [copilot];
        let a = areas(120, 30, true, None);
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let items = [
            MenuItem { label: "Claude".into(), note: "agent".into(), enabled: true, kind: "claude".into() },
            MenuItem { label: "Codex".into(), note: "agent".into(), enabled: true, kind: "codex".into() },
        ];
        let v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView { thread_id: "t-1a", agent: "copilot", scrolled: 0, selection: None }],
            focus: 0,
            cursor: None,
            skip: 0,
            prefix_armed: false,
            notice: None,
            help: None,
            menu: Some(Menu { title: "Switch to", items: &items, cursor: 0, verb: "switch", worktree: false }),
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
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].contains("╭ ∞ fix the login · ● desl "), "the thread's title, then its project: {s}");
        assert!(lines[3].contains("│ ▌∞ fix the login "), "on screen, with the edge of its pane: {s}");
        assert!(s.contains("╭ Switch to ") && s.contains(" 1  Claude  agent") && s.contains(" 2  Codex   agent"), "{s}");
        let keys = s.lines().last().unwrap();
        assert!(keys.contains("enter switch") && !keys.contains("own worktree"), "a switch keeps the thread where it is: {keys}");
    }

    #[test]
    fn the_cursor_lights_a_projects_row_and_the_key_line_says_how_to_move_it() {
        let projects = [project("desl"), project("api")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "api", 2, Status::Idle, "add a route")];
        let a = areas(120, 30, true, None);
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let r = rows(&projects, &threads);
        let at = r.iter().position(|x| *x == Row::Project("api".into())).unwrap();
        let v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0, selection: None }],
            focus: 0,
            cursor: Some(at),
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
        let y = SIDE_TOP + at as u16;
        assert!(s.lines().nth(y as usize).unwrap().contains("│ ● api  /src "), "{s}");
        let b = t.backend().buffer();
        // Lit from the first column of the row to the last, past where its text ends.
        assert_eq!((b[(2, y)].bg, b[(30, y)].bg), (Color::Blue, Color::Blue), "{s}");
        assert_eq!(b[(2, SIDE_TOP)].bg, Color::Reset, "the other project's row is not: {s}");
        assert!(s.lines().last().unwrap().contains("shift+↑↓ move a project"), "{s}");
    }

    #[test]
    fn marked_text_is_what_the_screen_holds_and_is_drawn_reversed() {
        let mut vt = vt100::Parser::new(4u16.try_into().unwrap(), 20u16.try_into().unwrap(), 0);
        // The second line is longer than the pane and wraps onto the third.
        vt.process(b"$ ls ~/src/desl\r\nalpha beta gamma delta epsilon\r\n");
        let s = vt.screen();
        // Dragged backwards, from the end of the wrapped line to the path on the first.
        let sel = Selection::between((2, 9), (0, 5));
        assert_eq!(sel, Selection { from: (0, 5), to: (2, 9) });
        assert_eq!(sel.text(s), "~/src/desl\nalpha beta gamma delta epsilon", "a wrapped line is one line again");
        assert!(sel.holds(0, 5) && sel.holds(1, 0) && sel.holds(2, 9));
        assert!(!sel.holds(0, 4) && !sel.holds(2, 10));
        assert_eq!(Selection::word(s, 0, 8).map(|w| w.text(s)).as_deref(), Some("~/src/desl"), "a path is one word");
        assert_eq!(Selection::word(s, 1, 0).map(|w| w.text(s)).as_deref(), Some("alpha"));
        assert_eq!(Selection::word(s, 0, 1), None, "a blank cell is no word");
        let area = Rect::new(0, 0, 20, 4);
        let mut buf = Buffer::empty(area);
        pane(&mut buf, area, s, Some(Selection { from: (1, 6), to: (1, 9) }));
        assert!(buf[(6, 1)].modifier.contains(Modifier::REVERSED) && buf[(9, 1)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(5, 1)].modifier.contains(Modifier::REVERSED) && !buf[(10, 1)].modifier.contains(Modifier::REVERSED));
        assert_eq!((6..=9).map(|x| buf[(x, 1)].symbol()).collect::<String>(), "beta");
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
    fn a_conversation_takes_the_pane_and_the_header_says_where_the_reader_is() {
        let projects = [project("desl")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login")];
        let a = areas(120, 30, true, None);
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let lines: Vec<Line<'static>> = ["zero", "  › why is it blank", "  The column was wrong.", "three"].map(Line::from).to_vec();
        let history = |hit, typing| SubView {
            title: "fix the login".into(),
            kind: SubKind::History { place: "Fri 20 Nov 10:41 · [12/340]".into() },
            lines: &lines,
            top: 1,
            hit,
            typing,
            tools: true,
        };
        let mut v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![PaneView {
                thread_id: "t-1a",
                agent: "claude",
                scrolled: 0,
                selection: Some(Selection { from: (0, 4), to: (0, 6) }),
            }],
            focus: 0,
            cursor: None,
            skip: 0,
            prefix_armed: false,
            notice: None,
            help: None,
            menu: None,
            finder: None,
            sub: Some(history(Some(2), None)),
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
        let rows: Vec<&str> = s.lines().collect();
        assert!(rows[0].contains("╭ History · fix the login ") && rows[0].ends_with(" Fri 20 Nov 10:41 · [12/340] ╮"), "{s}");
        assert!(
            rows[1].contains("││   › why is it blank") && rows[2].contains("││   The column was wrong."),
            "the line at the top is line 1: {s}"
        );
        assert!(
            rows[29].contains("esc back to the agent") && rows[29].contains("{ } prompts") && rows[29].contains("t hide tool calls"),
            "{s}"
        );
        let (x, y) = (a.panes[0].x, a.panes[0].y);
        let b = t.backend().buffer();
        let reversed = |col: u16, row: u16| b[(x + col, y + row)].modifier.contains(Modifier::REVERSED);
        assert!(reversed(4, 1) && !reversed(4, 2), "what the search found is reversed");
        assert!(reversed(4, 0) && reversed(6, 0) && !reversed(7, 0) && !reversed(3, 0), "and so is what is marked");
        // A search being typed takes the key line.
        v.sub = Some(history(None, Some(("col", false))));
        t.draw(|f| draw(f, &v, &a, &[vt.screen()])).unwrap();
        assert!(screen(&t).lines().last().unwrap().starts_with("  Find above col▏   enter find"), "{}", screen(&t));
    }

    #[test]
    fn a_narrow_key_line_drops_whole_keys_from_its_end() {
        let keys = [("s", "switch agent"), ("c", "new"), ("d", "detach")];
        let text = |width| with_keys(vec![Span::raw("  ")], &keys, width).spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(text(80), "  s switch agent   c new   d detach   ");
        assert_eq!(text(35), "  s switch agent   c new   d detach   ", "the blanks after the last key need no room");
        assert_eq!(text(34), "  s switch agent   c new   ");
        assert_eq!(text(10), "  ");
    }

    #[test]
    fn two_panes_each_have_their_row_marked_and_the_one_with_the_keys_is_lit() {
        let projects = [project("desl")];
        // Two agents of one project, side by side: only their titles and their marks tell them apart.
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "desl", 2, Status::Idle, "write the docs")];
        let a = areas(150, 30, true, Some(false));
        let vt = vt100::Parser::new(a.panes[0].height.try_into().unwrap(), a.panes[0].width.try_into().unwrap(), 0);
        let v = View {
            projects: &projects,
            threads: &threads,
            panes: vec![
                PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0, selection: None },
                PaneView { thread_id: "t-2b", agent: "claude", scrolled: 0, selection: None },
            ],
            focus: 1,
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
        let mut t = Terminal::new(TestBackend::new(150, 30)).unwrap();
        t.draw(|f| draw(f, &v, &a, &[vt.screen(), vt.screen()])).unwrap();
        let s = screen(&t);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[3].contains("▌✻ fix the login") && lines[3].contains("◧ now"), "the left pane's thread: {s}");
        assert!(lines[4].contains("▌✻ write the docs") && lines[4].contains("◨ now"), "the right pane's: {s}");
        assert!(lines[0].contains("✻ fix the login · ● desl") && lines[0].contains("✻ write the docs · ● desl"), "{s}");
        let b = t.backend().buffer();
        let x = a.sidebar.unwrap().x + 2;
        assert_eq!(b[(x, 4)].fg, Color::Blue, "the edge of the pane with the keys is the accent");
        assert_eq!(b[(x, 3)].fg, Color::DarkGray, "the other one is grey");
        assert!(b[(x + 4, 4)].modifier.contains(Modifier::BOLD) && !b[(x + 4, 3)].modifier.contains(Modifier::BOLD), "{s}");
        assert_ne!(b[(x + 20, 4)].bg, b[(x + 20, 3)].bg, "and its row's tint is not the other one's");
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
            panes: vec![PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0, selection: None }],
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
        assert!(lines[0].contains("╭ ✻ fix the login · ● desl · opus · high ") && lines[0].contains("○ idle t-1a ╮"), "{s}");
        assert!(lines[1].contains("││ hello from the agent"), "{s}");
        assert!(lines[2].contains("│ ● desl  /src "), "{s}");
        assert!(lines[3].contains("│ ▌✻ fix the login ") && lines[3].contains("now  ││"), "{s}");
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
            panes: vec![PaneView { thread_id: "t-1a", agent: "claude", scrolled: 0, selection: None }],
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
            MenuItem { label: "Claude".into(), note: "agent".into(), enabled: true, kind: "claude".into() },
            MenuItem { label: "bash".into(), note: "shell".into(), enabled: true, kind: SHELL.into() },
        ];
        v.menu = Some(Menu { title: "New in desl", items: &items, cursor: 1, verb: "start", worktree: true });
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
