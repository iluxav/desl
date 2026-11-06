//! What desl draws over the agent on the native route (SPEC.md "Interface design"): the thread
//! list (`prefix w`) and the help (`prefix ?`). Both go on the terminal's alternate screen, so
//! closing them leaves the agent's screen and scrollback as they were.

use crate::theme;
use crate::ui::text::truncate_end;
use desl_ir::{Project, Status, Thread};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// What a key after the prefix does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Switch,
    New,
    List,
    Next,
    Prev,
    Jump(usize),
    Last,
    Detach,
    Help,
}

/// The keys desl owns after the prefix, in the order the help shows them. The help is drawn
/// from this table and [`action`] reads it, so the two cannot disagree.
pub const KEYS: &[(&str, &str, &str)] = &[
    ("Agent", "s", "Switch this thread to the other agent"),
    ("Threads", "w", "Thread list"),
    ("Threads", "c", "New thread in this project"),
    ("Threads", "n", "Next thread"),
    ("Threads", "p", "Previous thread"),
    ("Threads", "1…9", "Jump to thread n in the list"),
    ("Threads", "l", "Back to the thread shown before"),
    ("desl", "d", "Detach; the agents keep working"),
    ("desl", "?", "This help"),
    ("desl", "ctrl+b", "Send ctrl+b to the agent"),
];

/// The action for the key pressed after the prefix.
pub fn action(c: char) -> Option<Action> {
    Some(match c {
        's' => Action::Switch,
        'w' => Action::List,
        'c' => Action::New,
        'n' => Action::Next,
        'p' => Action::Prev,
        'l' => Action::Last,
        'd' => Action::Detach,
        '?' => Action::Help,
        '1'..='9' => Action::Jump(c as usize - '0' as usize),
        _ => return None,
    })
}

/// Every thread in a fixed order: by project, then oldest first. `prefix n`, `prefix p` and
/// `prefix 1`…`9` follow it, so a thread keeps its number while statuses change.
pub fn order<'a>(projects: &[Project], threads: &'a [Thread]) -> Vec<&'a Thread> {
    let rank = |t: &Thread| projects.iter().position(|p| p.id == t.project_id).unwrap_or(usize::MAX);
    let mut out: Vec<&Thread> = threads.iter().collect();
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.created_at.cmp(&b.created_at)).then(a.id.cmp(&b.id)));
    out
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

/// One row of the list: a heading, or a thread that `enter` opens.
#[derive(Debug, PartialEq, Eq)]
pub enum Row {
    Heading(String),
    Thread { id: String, number: usize },
}

/// The list's rows: the `WAITING` group first, then every project with its threads.
pub fn rows(projects: &[Project], threads: &[Thread]) -> Vec<Row> {
    let ordered = order(projects, threads);
    let number = |id: &str| ordered.iter().position(|t| t.id == id).map_or(0, |n| n + 1);
    let mut out = Vec::new();
    let waiting: Vec<&&Thread> = ordered.iter().filter(|t| t.status == Status::Waiting).collect();
    if !waiting.is_empty() {
        out.push(Row::Heading("WAITING".into()));
        out.extend(waiting.iter().map(|t| Row::Thread { id: t.id.clone(), number: number(&t.id) }));
    }
    let mut last_project: Option<&str> = None;
    for t in &ordered {
        if last_project != Some(t.project_id.as_str()) {
            last_project = Some(t.project_id.as_str());
            out.push(Row::Heading(t.project_id.clone()));
        }
        out.push(Row::Thread { id: t.id.clone(), number: number(&t.id) });
    }
    out
}

pub struct ListView<'a> {
    pub projects: &'a [Project],
    pub threads: &'a [Thread],
    /// The thread on screen behind the list.
    pub current: &'a str,
    /// Index into [`rows`] of the highlighted row; always a thread row.
    pub cursor: usize,
    pub now: i64,
}

fn glyph_style(s: Status) -> Style {
    match s {
        Status::Working => theme::accent(),
        Status::Waiting => theme::warning(),
        Status::Idle | Status::Detached => theme::muted(),
        Status::Failed => theme::critical(),
    }
}

fn thread_line(t: &Thread, number: usize, current: bool, highlighted: bool, now: i64, w: usize) -> Line<'static> {
    let marker = if current { "▸" } else { " " };
    let num = if number <= 9 { number.to_string() } else { " ".into() };
    let branch = t.git.branch.clone().unwrap_or_default();
    let right = format!("{:<7} {:<14} {:>4}", t.active_agent, truncate_end(&branch, 14), age(now, t.updated_at));
    let left = format!(" {marker} {num} ");
    let fixed = left.width() + 2 + t.id.width() + 2 + right.width() + 1;
    let name = truncate_end(if t.name.is_empty() { "new" } else { &t.name }, w.saturating_sub(fixed).max(4));
    let pad = w.saturating_sub(fixed + name.width()).max(1);
    let base = if highlighted { theme::pill() } else { Style::default() };
    let pick = |s: Style| if highlighted { theme::pill() } else { s };
    Line::from(vec![
        Span::styled(left, pick(theme::accent())),
        Span::styled(t.status.glyph().to_string(), pick(glyph_style(t.status))),
        Span::styled(" ", base),
        Span::styled(t.id.clone(), pick(if current { theme::normal() } else { theme::muted() })),
        Span::styled("  ", base),
        Span::styled(name, pick(if current { theme::bold() } else { theme::normal() })),
        Span::styled(" ".repeat(pad + 1), base),
        Span::styled(right, pick(theme::dim())),
        Span::styled(" ", base),
    ])
}

fn heading(text: &str) -> Line<'static> {
    let style = if text == "WAITING" { theme::warning() } else { theme::muted() };
    Line::from(Span::styled(format!(" {text}"), style.add_modifier(Modifier::BOLD)))
}

pub fn draw_list(f: &mut Frame, v: &ListView) {
    let area = f.area();
    let w = area.width as usize;
    let all = rows(v.projects, v.threads);
    let mut lines: Vec<Line<'static>> = vec![Line::from(vec![
        Span::styled(" desl", theme::accent().add_modifier(Modifier::BOLD)),
        Span::styled(" · threads", theme::muted()),
    ])];
    lines.push(Line::default());
    for (i, row) in all.iter().enumerate() {
        match row {
            Row::Heading(h) => lines.push(heading(h)),
            Row::Thread { id, number } => {
                if let Some(t) = v.threads.iter().find(|t| t.id == *id) {
                    lines.push(thread_line(t, *number, t.id == v.current, i == v.cursor, v.now, w));
                }
            }
        }
    }
    let body_h = area.height.saturating_sub(1) as usize;
    // Keep the highlighted row in view on a short terminal.
    let skip = (v.cursor + 3).saturating_sub(body_h);
    let shown: Vec<Line<'static>> = lines.into_iter().skip(skip).take(body_h).collect();
    f.render_widget(Paragraph::new(shown), Rect { height: area.height.saturating_sub(1), ..area });
    hint(f, "enter open · j/k move · 1-9 jump · esc close");
}

fn hint(f: &mut Frame, text: &str) {
    let area = f.area();
    if area.height == 0 {
        return;
    }
    let line = Line::from(Span::styled(format!(" {}", truncate_end(text, area.width.saturating_sub(1) as usize)), theme::muted()));
    f.render_widget(Paragraph::new(line), Rect { y: area.y + area.height - 1, height: 1, ..area });
}

pub fn draw_help(f: &mut Frame) {
    let area = f.area();
    let mut lines: Vec<Line<'static>> = vec![
        Line::from(vec![
            Span::styled(" desl", theme::accent().add_modifier(Modifier::BOLD)),
            Span::styled(" · keys, after ctrl+b", theme::muted()),
        ]),
        Line::default(),
    ];
    let mut group = "";
    for (g, key, what) in KEYS {
        if *g != group {
            if !group.is_empty() {
                lines.push(Line::default());
            }
            group = g;
            lines.push(heading(g));
        }
        lines.push(Line::from(vec![Span::styled(format!("   {key:<8}"), theme::accent()), Span::raw(what.to_string())]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("   Every other key goes to the agent.", theme::dim())));
    f.render_widget(Paragraph::new(lines), Rect { height: area.height.saturating_sub(1), ..area });
    hint(f, "esc close");
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

/// The whole of `buf` as terminal output, in one synchronized frame. Every cell is written, so
/// nothing of what was on the screen before shows through.
pub fn paint(buf: &Buffer) -> Vec<u8> {
    let mut out = String::from("\x1b[?2026h\x1b[?25l\x1b[0m");
    let area = buf.area;
    for y in 0..area.height {
        out.push_str(&format!("\x1b[{};1H", y + 1));
        let mut style = String::new();
        let mut skip = 0;
        for x in 0..area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buf[(area.x + x, area.y + y)];
            let s = sgr(cell.fg, cell.bg, cell.modifier);
            if s != style {
                out.push_str(&s);
                style = s;
            }
            let sym = cell.symbol();
            out.push_str(if sym.is_empty() { " " } else { sym });
            skip = sym.width().saturating_sub(1);
        }
    }
    out.push_str("\x1b[0m\x1b[?2026l");
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
            "bindings": {}, "git": { "branch": "main", "ahead": 0, "behind": 0 }
        }))
        .expect("a thread")
    }

    fn project(id: &str) -> Project {
        Project { id: id.into(), path: format!("/{id}"), default_agent: None, created_at: 0 }
    }

    fn screen(t: &mut Terminal<TestBackend>) -> String {
        let b = t.backend().buffer();
        (0..b.area.height)
            .map(|y| (0..b.area.width).map(|x| b[(x, y)].symbol()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_documented_key_has_an_action_and_digits_jump() {
        for (_, key, _) in KEYS {
            let c = key.chars().next().unwrap();
            if *key != "ctrl+b" {
                assert!(action(c).is_some(), "{key} does nothing");
            }
        }
        assert_eq!(action('3'), Some(Action::Jump(3)));
        assert_eq!(action('z'), None);
    }

    #[test]
    fn numbers_follow_projects_then_age_and_waiting_threads_lead_the_list() {
        let projects = [project("a"), project("b")];
        let threads = [
            thread("t-3", "b", 1, Status::Idle, "third"),
            thread("t-2", "a", 2, Status::Waiting, "second"),
            thread("t-1", "a", 1, Status::Working, "first"),
        ];
        let ids: Vec<&str> = order(&projects, &threads).iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t-1", "t-2", "t-3"]);
        let r = rows(&projects, &threads);
        assert_eq!(r[0], Row::Heading("WAITING".into()));
        assert_eq!(r[1], Row::Thread { id: "t-2".into(), number: 2 });
        assert_eq!(r[2], Row::Heading("a".into()));
        assert_eq!(r.last(), Some(&Row::Thread { id: "t-3".into(), number: 3 }));
    }

    #[test]
    fn ages_are_one_short_word() {
        assert_eq!(age(30_000, 0), "now");
        assert_eq!(age(5 * 60_000, 0), "5m");
        assert_eq!(age(3 * 3_600_000, 0), "3h");
        assert_eq!(age(2 * 86_400_000, 0), "2d");
    }

    #[test]
    fn the_list_and_help_read_at_80_by_24() {
        let projects = [project("desl")];
        let threads = [thread("t-1a", "desl", 1, Status::Idle, "fix the login"), thread("t-2b", "desl", 2, Status::Waiting, "")];
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        t.draw(|f| draw_list(f, &ListView { projects: &projects, threads: &threads, current: "t-1a", cursor: 4, now: 0 })).unwrap();
        let s = screen(&mut t);
        assert!(s.contains("WAITING"), "{s}");
        assert!(s.contains("▸ 1 ○ t-1a  fix the login"), "{s}");
        assert!(s.contains("2 ◐ t-2b  new"), "{s}");
        assert!(s.contains("claude  main"), "{s}");
        assert!(s.lines().last().unwrap().contains("esc close"), "{s}");
        let painted = String::from_utf8(paint(t.backend().buffer())).unwrap();
        assert!(painted.starts_with("\x1b[?2026h") && painted.contains("\x1b[24;1H") && painted.contains("t-1a"), "{painted:?}");
        t.draw(draw_help).unwrap();
        let s = screen(&mut t);
        assert!(s.contains("s       Switch this thread to the other agent"), "{s}");
        assert!(s.contains("Every other key goes to the agent."), "{s}");
    }
}
