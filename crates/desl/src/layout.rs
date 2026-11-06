//! The workspace screen (SPEC.md "Interface design"): projects and their threads on the left,
//! the shown thread's agent on the right under a one-line header, and the common keys along the
//! bottom. The agent draws into desl's own terminal emulator, which is copied into the pane.

use crate::theme;
use crate::ui::text::truncate_end;
use desl_ir::{Project, Status, Thread};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

/// What a key after the prefix does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Switch,
    New,
    Threads,
    Next,
    Prev,
    Jump(usize),
    Last,
    Sidebar,
    Detach,
    Help,
}

/// The keys desl owns after the prefix, in the order the help shows them. The help is drawn
/// from this table and [`action`] reads it, so the two cannot disagree.
pub const KEYS: &[(&str, &str, &str)] = &[
    ("Agent", "s", "Switch this thread to the other agent"),
    ("Threads", "w", "Move to the thread list (j/k, enter, esc)"),
    ("Threads", "c", "New thread in this project"),
    ("Threads", "n", "Next thread"),
    ("Threads", "p", "Previous thread"),
    ("Threads", "1…9", "Jump to thread n"),
    ("Threads", "l", "Back to the thread shown before"),
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
        'n' => Action::Next,
        'p' => Action::Prev,
        'l' => Action::Last,
        'b' => Action::Sidebar,
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

/// Where everything goes on a terminal of this size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Areas {
    pub sidebar: Option<Rect>,
    pub header: Rect,
    /// The agent's screen: its terminal is exactly this size.
    pub pane: Rect,
    pub footer: Rect,
}

/// The sidebar takes a quarter of the width, 24 to 36 columns, and steps aside below 70.
pub fn areas(cols: u16, rows: u16, sidebar: bool) -> Areas {
    let side_w = if sidebar && cols >= 70 { (cols / 4).clamp(24, 36) } else { 0 };
    let right_x = if side_w > 0 { side_w + 1 } else { 0 };
    let right_w = cols.saturating_sub(right_x).max(1);
    let body_h = rows.saturating_sub(2).max(1);
    Areas {
        sidebar: (side_w > 0).then(|| Rect::new(0, 0, side_w, body_h)),
        header: Rect::new(right_x, 0, right_w, 1),
        pane: Rect::new(right_x, 1, right_w, body_h.saturating_sub(1).max(1)),
        footer: Rect::new(0, rows.saturating_sub(1), cols, 1),
    }
}

/// One line of the sidebar: a project, or a thread that a click or `enter` opens.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Row {
    Project(String),
    Thread(String),
}

pub fn rows(projects: &[Project], threads: &[Thread]) -> Vec<Row> {
    let mut out = Vec::new();
    let mut last: Option<&str> = None;
    for t in order(projects, threads) {
        if last != Some(t.project_id.as_str()) {
            last = Some(t.project_id.as_str());
            out.push(Row::Project(t.project_id.clone()));
        }
        out.push(Row::Thread(t.id.clone()));
    }
    out
}

/// Sidebar lines above the first row: the title and a blank line.
const SIDE_TOP: u16 = 2;

/// The first row shown, so `keep` (an index into [`rows`]) stays in view.
pub fn side_skip(total: usize, keep: usize, area: Rect) -> usize {
    let room = area.height.saturating_sub(SIDE_TOP) as usize;
    if total <= room { 0 } else { (keep + 1).saturating_sub(room).min(total - room) }
}

/// The thread at sidebar line `y` (terminal row), if any.
pub fn thread_at(projects: &[Project], threads: &[Thread], area: Rect, skip: usize, y: u16) -> Option<String> {
    let i = (y.checked_sub(area.y + SIDE_TOP)? as usize) + skip;
    match rows(projects, threads).get(i) {
        Some(Row::Thread(id)) => Some(id.clone()),
        _ => None,
    }
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
    /// A sentence for the user, shown in the footer until the next key.
    pub notice: Option<&'a str>,
    pub help: bool,
    /// Lines scrolled back in the agent's screen, 0 when following it.
    pub scrolled: usize,
}

fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::muted(),
        Status::Failed => theme::critical(),
    }
}

fn agent_title(agent: &str) -> String {
    let mut c = agent.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn sidebar(f: &mut Frame, v: &View, area: Rect) {
    let w = area.width as usize;
    let waiting = v.threads.iter().filter(|t| t.status == Status::Waiting).count();
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(" projects", theme::bold()),
            Span::styled(if waiting > 0 { format!("  {waiting} waiting") } else { String::new() }, theme::warning()),
        ]),
        Line::default(),
    ];
    for (i, row) in rows(v.projects, v.threads).into_iter().enumerate().skip(v.skip) {
        match row {
            Row::Project(id) => {
                let path = v.projects.iter().find(|p| p.id == id).map_or(id.clone(), |p| tilde(&p.path));
                lines.push(Line::from(vec![
                    Span::styled(" ▾ ", theme::muted()),
                    Span::styled(truncate_start(&path, w.saturating_sub(4)), theme::bold()),
                ]));
            }
            Row::Thread(id) => {
                let Some(t) = v.threads.iter().find(|t| t.id == id) else { continue };
                let current = t.id == v.current;
                let highlighted = v.cursor == Some(i);
                let name = if t.name.is_empty() { "new" } else { t.name.as_str() };
                let text = truncate_end(&format!("{} · {name}", t.active_agent), w.saturating_sub(6));
                let pad = w.saturating_sub(5 + text.width());
                let (mark, text_style) = match (highlighted, current) {
                    (true, _) => ("   ", theme::pill()),
                    (false, true) => (" ▌ ", theme::bold()),
                    (false, false) => ("   ", theme::dim()),
                };
                let fill = if highlighted { theme::pill() } else { Style::default() };
                lines.push(Line::from(vec![
                    Span::styled(mark, theme::accent()),
                    Span::styled(t.status.glyph().to_string(), if highlighted { theme::pill() } else { glyph_style(t.status) }),
                    Span::styled(" ", fill),
                    Span::styled(text, text_style),
                    Span::styled(" ".repeat(pad), fill),
                ]));
            }
        }
    }
    f.render_widget(Paragraph::new(lines), area);
    let rule = Rect::new(area.x + area.width, area.y, 1, area.height);
    let bar: Vec<Line<'static>> = (0..area.height).map(|_| Line::from(Span::styled("│", theme::muted()))).collect();
    f.render_widget(Paragraph::new(bar), rule);
}

fn header(f: &mut Frame, v: &View, area: Rect) {
    let Some(t) = v.threads.iter().find(|t| t.id == v.current) else { return };
    let path = v.projects.iter().find(|p| p.id == t.project_id).map_or(t.cwd.clone(), |p| tilde(&p.path));
    let binding = t.bindings.get(v.agent);
    let mut model = binding.and_then(|b| b.model_choice.clone().or(b.model.clone())).unwrap_or_default();
    if let Some(e) = binding.and_then(|b| b.effort_choice.as_deref()) {
        model = if model.is_empty() { format!("{e} effort") } else { format!("{model} with {e} effort") };
    }
    let right = if v.scrolled > 0 {
        Span::styled(format!("↑ {} lines · wheel down to follow ", v.scrolled), theme::warning())
    } else {
        Span::styled(format!("{} {} ", t.id, t.status.word()), glyph_style(t.status))
    };
    let agent = agent_title(v.agent);
    let tail = if model.is_empty() { 0 } else { model.width() + 3 };
    let room = (area.width as usize).saturating_sub(right.content.width() + agent.width() + tail + 7).max(8);
    let mut left = vec![
        Span::styled(format!(" {}", truncate_start(&path, room)), theme::normal()),
        Span::styled(" · ", theme::muted()),
        Span::styled(agent, theme::accent().add_modifier(Modifier::BOLD)),
    ];
    if !model.is_empty() {
        left.push(Span::styled(" · ", theme::muted()));
        left.push(Span::styled(model, theme::dim()));
    }
    let used: usize = left.iter().map(|s| s.content.width()).sum::<usize>() + right.content.width();
    left.push(Span::raw(" ".repeat((area.width as usize).saturating_sub(used))));
    left.push(right);
    f.render_widget(Paragraph::new(Line::from(left)).style(Style::default().add_modifier(Modifier::UNDERLINED)), area);
}

fn footer(f: &mut Frame, v: &View, area: Rect) {
    let line = if let Some(n) = v.notice {
        Line::from(Span::styled(format!(" {n}"), theme::warning()))
    } else if v.cursor.is_some() {
        Line::from(Span::styled(" j/k move · enter open · 1-9 jump · esc back to the agent", theme::muted()))
    } else {
        let mut spans = vec![Span::styled(" ctrl+b ", if v.prefix_armed { theme::pill() } else { theme::accent() })];
        for (k, what) in
            [("s", "switch agent"), ("c", "new"), ("w", "threads"), ("n/p", "next/prev"), ("b", "sidebar"), ("?", "help"), ("d", "detach")]
        {
            spans.push(Span::styled(format!(" {k} "), theme::bold()));
            spans.push(Span::styled(format!("{what} "), theme::muted()));
        }
        Line::from(spans)
    };
    f.render_widget(Paragraph::new(line), area);
}

fn help(f: &mut Frame, area: Rect) {
    let h = (KEYS.len() as u16 + 9).min(area.height);
    let w = 62.min(area.width);
    let r = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(" keys, after ctrl+b", theme::bold())), Line::default()];
    let mut group = "";
    for (g, key, what) in KEYS {
        if *g != group {
            group = g;
            lines.push(Line::from(Span::styled(format!(" {g}"), theme::muted().add_modifier(Modifier::BOLD))));
        }
        lines.push(Line::from(vec![Span::styled(format!("   {key:<8}"), theme::accent()), Span::raw(what.to_string())]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("   Every other key goes to the agent. Any key closes this.", theme::dim())));
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(lines).block(Block::bordered().border_style(theme::muted())), r);
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
            let Some(cell) = screen.cell(r, c) else { continue };
            let out = &mut buf[(area.x + c, area.y + r)];
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

/// Draws everything but the agent's screen, which [`pane`] copies in.
pub fn draw(f: &mut Frame, v: &View, a: &Areas, screen: &vt100::Screen) {
    if let Some(s) = a.sidebar {
        sidebar(f, v, s);
    }
    header(f, v, a.header);
    pane(f.buffer_mut(), a.pane, screen);
    footer(f, v, a.footer);
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
        let r = rows(&projects, &threads);
        assert_eq!(r[0], Row::Project("a".into()));
        assert_eq!(r[3], Row::Project("b".into()));
        let a = areas(100, 30, true);
        assert_eq!(thread_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP + 1), Some("t-1".into()));
        assert_eq!(thread_at(&projects, &threads, a.sidebar.unwrap(), 0, SIDE_TOP), None);
    }

    #[test]
    fn the_layout_gives_the_agent_what_the_chrome_leaves() {
        let a = areas(100, 30, true);
        assert_eq!(a.sidebar, Some(Rect::new(0, 0, 25, 28)));
        assert_eq!(a.pane, Rect::new(26, 1, 74, 27));
        assert_eq!(a.footer, Rect::new(0, 29, 100, 1));
        assert_eq!(areas(60, 20, true).sidebar, None);
        assert_eq!(areas(100, 30, false).pane.width, 100);
    }

    #[test]
    fn the_screen_reads_at_100_by_30_and_paints_only_what_changed() {
        let projects = [project("desl")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "desl", 2, Status::Waiting, "")];
        let a = areas(100, 30, true);
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
        };
        let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
        t.draw(|f| draw(f, &v, &a, vt.screen())).unwrap();
        let s = screen(&t);
        assert!(s.contains(" projects  1 waiting"), "{s}");
        assert!(s.contains(" ▾ /src/desl"), "{s}");
        assert_eq!(truncate_start("/tmp/a/b/demo", 8), "…/b/demo");
        assert!(s.contains(" ▌ ○ claude · fix the l…"), "{s}");
        assert!(s.contains("◐ claude · new"), "{s}");
        assert!(s.contains("│ /src/desl · Claude · opus with high effort"), "{s}");
        assert!(s.contains("│hello from the agent"), "{s}");
        assert!(s.lines().last().unwrap().contains("ctrl+b  s switch agent"), "{s}");
        let before = t.backend().buffer().clone();
        let full = String::from_utf8(paint(None, &before)).unwrap();
        assert!(full.contains("\x1b[2J") && full.contains("hello"), "{full:?}");
        v.help = true;
        t.draw(|f| draw(f, &v, &a, vt.screen())).unwrap();
        assert!(screen(&t).contains("Every other key goes to the agent."));
        let diff = String::from_utf8(paint(Some(&before), t.backend().buffer())).unwrap();
        assert!(!diff.contains("\x1b[2J") && diff.contains("Switch") && diff.contains("agent") && !diff.contains("projects"), "{diff:?}");
    }
}
