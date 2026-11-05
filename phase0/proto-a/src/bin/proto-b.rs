//! desl phase 0, prototype B: the native experience. The agents' own TUIs run in PTY panes under
//! the same chrome as Prototype A; continuity comes from tailing their session files, the way
//! tandem does. Throwaway by design (SPEC.md "Roadmap").
//!
//! `proto-b`            attach to this project's workspace (starts the host if needed)
//! `proto-b status`     the sidebar as text
//! `proto-b stop`       stop the host for this project (this ends its agents)
//! `proto-b --host DIR` run the host (started for you by the client)

use anyhow::{Context, Result};
use proto_a::app::{Level, Notice};
use proto_a::b64;
use proto_a::bhost::{self, BCommand, BFrame, CODEX_APPROVAL, ThreadSnap};
use proto_a::git::{self, GitInfo};
use proto_a::keymap::{self, Action, Ctx};
use proto_a::model::{Agent, Status, Thread};
use proto_a::picker::{Entry, Picker};
use proto_a::pty::{keys, widget};
use proto_a::theme;
use proto_a::ui::{self, chrome::ChromeView};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::num::NonZeroU16;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

enum Msg {
    Term(Event),
    Host(BFrame),
    HostGone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Pane,
    Sidebar,
}

enum Overlay {
    None,
    Help,
    Picker(Picker, PickKind),
    Text { title: String, lines: Vec<String>, scroll: usize },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PickKind {
    Threads,
    NewAgent,
}

type PaneKey = (usize, Agent);

struct Client {
    cwd: PathBuf,
    project: String,
    link: UnixStream,
    snaps: Vec<ThreadSnap>,
    threads: Vec<Thread>,
    screens: HashMap<PaneKey, vt100::Parser>,
    typed: HashMap<PaneKey, String>,
    ask: Option<(usize, Agent, String)>,
    current: usize,
    jump_to_new: bool,
    focus: Focus,
    overlay: Overlay,
    prefix_pending: bool,
    sidebar_visible: bool,
    scroll_mode: bool,
    notice: Option<Notice>,
    git: GitInfo,
    git_refreshed: Instant,
    clock: String,
    help_scroll: usize,
    should_quit: bool,
    farewell: Option<String>,
    dirty: bool,
    pane_size: (u16, u16),
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn bell() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Client {
    fn send(&mut self, cmd: &BCommand) {
        if let Ok(mut line) = serde_json::to_string(cmd) {
            line.push('\n');
            let _ = self.link.write_all(line.as_bytes());
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), level: Level::Info, at: Instant::now() });
    }

    fn agent(&self) -> Agent {
        self.snaps.get(self.current).map(|s| s.agent).unwrap_or(Agent::Claude)
    }
    fn key(&self) -> PaneKey {
        (self.current, self.agent())
    }
    fn screen_text(&self) -> String {
        self.screens.get(&self.key()).map(|p| widget::tail_text(p.screen(), 14)).unwrap_or_default()
    }
    fn write(&mut self, bytes: &[u8]) {
        let (idx, agent) = self.key();
        if let Some(p) = self.screens.get_mut(&(idx, agent)) {
            p.screen_mut().set_scrollback(0);
        }
        self.send(&BCommand::Input { idx, agent, data: b64::encode(bytes) });
    }

    fn jump_to(&mut self, idx: usize) {
        if idx < self.snaps.len() {
            self.current = idx;
            self.scroll_mode = false;
        } else {
            self.info(format!("There is no thread {}. {} n opens one.", idx + 1, keymap::PREFIX_LABEL));
        }
    }

    fn tick(&mut self) {
        let clock = now_hhmm();
        if clock != self.clock {
            self.clock = clock;
            self.dirty = true;
        }
        if let Some(n) = &self.notice
            && n.level == Level::Info
            && n.at.elapsed() > Duration::from_secs(8)
        {
            self.notice = None;
            self.dirty = true;
        }
        if self.git_refreshed.elapsed() > Duration::from_secs(5) {
            self.git = git::read(&self.cwd);
            self.git_refreshed = Instant::now();
            self.dirty = true;
        }
    }

    // ---- host frames ------------------------------------------------------------

    fn on_frame(&mut self, frame: BFrame) {
        let was_dirty = self.dirty;
        self.dirty = true;
        match frame {
            BFrame::Threads { threads, view } => {
                let grew = threads.len() > self.snaps.len();
                self.threads = threads
                    .iter()
                    .map(|s| {
                        let mut t = Thread::new(s.agent);
                        t.id = s.id.clone();
                        t.name = s.name.clone();
                        t.status = s.status;
                        t
                    })
                    .collect();
                self.snaps = threads;
                if let Some(v) = view {
                    if let Some(c) = v.get("current").and_then(Value::as_u64) {
                        self.current = c as usize;
                    }
                    if let Some(s) = v.get("sidebar").and_then(Value::as_bool) {
                        self.sidebar_visible = s;
                    }
                }
                if grew && self.jump_to_new {
                    self.jump_to_new = false;
                    self.current = self.snaps.len() - 1;
                    self.focus = Focus::Pane;
                }
                self.current = self.current.min(self.snaps.len().saturating_sub(1));
            }
            BFrame::Replay { idx, agent, data } => {
                let (rows, cols) = self.pane_size;
                let mut parser = vt100::Parser::new(nz(rows), nz(cols), 10_000);
                parser.process(&b64::decode(&data));
                self.screens.insert((idx, agent), parser);
            }
            BFrame::Output { idx, agent, data } => {
                let (rows, cols) = self.pane_size;
                let parser = self.screens.entry((idx, agent)).or_insert_with(|| vt100::Parser::new(nz(rows), nz(cols), 10_000));
                parser.process(&b64::decode(&data));
                // Output of a pane that is not on screen needs no redraw of its own.
                if (idx, agent) != (self.current, self.snaps.get(self.current).map(|s| s.agent).unwrap_or(agent)) {
                    self.dirty = was_dirty;
                }
            }
            BFrame::Ask { idx, to, text } => self.ask = Some((idx, to, text)),
            BFrame::Text { title, lines } => self.overlay = Overlay::Text { title, lines, scroll: 0 },
            BFrame::Notice { text, error } => {
                self.notice = Some(Notice { text, level: if error { Level::Error } else { Level::Info }, at: Instant::now() });
            }
            BFrame::Bell => bell(),
            BFrame::Bye => {
                self.farewell = Some("The desl host was stopped, which ended its agents. Their own sessions can be resumed with `claude --resume` and `codex resume`.".into());
                self.should_quit = true;
            }
        }
    }

    /// Quitting detaches: the host and its agents keep running.
    fn detach(&mut self) {
        let view = json!({ "current": self.current, "sidebar": self.sidebar_visible });
        self.send(&BCommand::SaveView { view });
        let n = self.snaps.len();
        let working = self.snaps.iter().filter(|s| matches!(s.status, Status::Working | Status::Waiting)).count();
        let mut msg = format!("Detached. {n} thread{} kept", if n == 1 { "" } else { "s" });
        if working > 0 {
            msg.push_str(&format!(", {working} still working"));
        }
        msg.push_str(". Run the prototype here again to reattach; `proto-b stop` stops the host and its agents.");
        self.farewell = Some(msg);
        self.should_quit = true;
    }

    // ---- keys ----------------------------------------------------------------

    fn on_key(&mut self, k: KeyEvent) {
        if let Some(n) = &self.notice
            && n.level == Level::Info
        {
            self.notice = None;
        }
        if matches!(self.overlay, Overlay::Picker(..)) {
            self.picker_key(k);
            return;
        }
        if let Overlay::Text { scroll, .. } = &mut self.overlay {
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.overlay = Overlay::None,
                KeyCode::Down | KeyCode::Char('j') => *scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                _ => {}
            }
            return;
        }
        if matches!(self.overlay, Overlay::Help) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter => self.overlay = Overlay::None,
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                _ => {}
            }
            return;
        }
        if self.prefix_pending {
            self.prefix_pending = false;
            self.prefixed(k);
            return;
        }
        if keymap::is_prefix(&k) {
            self.prefix_pending = true;
            return;
        }
        // A desl question above the prompt takes y, n and esc first.
        if let Some((idx, to, _)) = self.ask.clone()
            && idx == self.current
        {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.ask = None;
                    self.send(&BCommand::Switch { idx, to });
                    return;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.ask = None;
                    return;
                }
                _ => {}
            }
        }
        if self.focus == Focus::Sidebar {
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => self.jump_to(self.current + 1),
                KeyCode::Char('k') | KeyCode::Up => self.jump_to(self.current.saturating_sub(1)),
                KeyCode::Enter | KeyCode::Esc => self.focus = Focus::Pane,
                KeyCode::Char('?') => {
                    self.overlay = Overlay::Help;
                    self.help_scroll = 0;
                }
                _ => {}
            }
            return;
        }
        let key = self.key();
        if self.scroll_mode {
            let rows = self.pane_size.0 as isize;
            let Some(p) = self.screens.get_mut(&key) else { return };
            let cur = p.screen().scrollback() as isize;
            let mut to = cur;
            match k.code {
                KeyCode::Char('k') | KeyCode::Up => to = cur + 1,
                KeyCode::Char('j') | KeyCode::Down => to = cur - 1,
                KeyCode::PageUp | KeyCode::Char('u') => to = cur + rows - 1,
                KeyCode::PageDown | KeyCode::Char('d') => to = cur - (rows - 1),
                KeyCode::Char('g') => to = 100_000,
                KeyCode::Char('G') | KeyCode::Esc | KeyCode::Char('q') => {
                    to = 0;
                    self.scroll_mode = false;
                }
                _ => {}
            }
            p.screen_mut().set_scrollback(to.max(0) as usize);
            return;
        }
        if !self.screens.contains_key(&key) {
            self.info(format!("This thread's agent is not running. {} n opens a new thread.", keymap::PREFIX_LABEL));
            return;
        }
        // Claude Code's permission dialog: desl's y, n and a map onto its options.
        // Only while the host says the thread is waiting: the same words in an answer must not steal keys.
        if key.1 == Agent::Claude
            && self.threads.get(self.current).is_some_and(|t| t.status == Status::Waiting)
            && bhost::claude_dialog(&self.screen_text())
            && let KeyCode::Char(c) = k.code
            && matches!(c, 'y' | 'n' | 'a')
        {
            let bytes: &[u8] = match c {
                'y' => b"\r",
                'n' => b"\x1b",
                _ => b"\x1b[B\r",
            };
            self.write(bytes);
            return;
        }
        // desl's own slash commands, typed into the agent's input, are taken back before Enter reaches it.
        if k.code == KeyCode::Enter && k.modifiers.is_empty() {
            let typed = self.typed.get(&key).map(|s| s.trim().to_string()).unwrap_or_default();
            if typed.starts_with('/') && is_desl_command(&typed) {
                let n = self.typed.get(&key).map(|s| s.chars().count()).unwrap_or(0);
                self.write(&vec![0x7f; n]);
                self.typed.remove(&key);
                self.slash(&typed);
                return;
            }
        }
        let app_cursor = self.screens.get(&key).is_some_and(|p| p.screen().application_cursor());
        if let Some(bytes) = keys::encode(&k, app_cursor) {
            self.write(&bytes);
            let typed = self.typed.entry(key).or_default();
            match k.code {
                KeyCode::Char(c) if !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => typed.push(c),
                KeyCode::Backspace => {
                    typed.pop();
                }
                KeyCode::Esc | KeyCode::Enter => typed.clear(),
                KeyCode::Char('u') | KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => typed.clear(),
                _ => {}
            }
        }
    }

    fn prefixed(&mut self, k: KeyEvent) {
        if k.code == KeyCode::Esc {
            return;
        }
        if let KeyCode::Char(c) = k.code
            && let Some(d) = c.to_digit(10)
            && (1..=9).contains(&d)
        {
            self.jump_to(d as usize - 1);
            return;
        }
        let binding = keymap::lookup(&k, Ctx::Workspace, true).or_else(|| keymap::lookup(&k, Ctx::Pane, true));
        let Some(b) = binding else {
            self.info(format!(
                "{} {} is not a key. Press {} ? for the list.",
                keymap::PREFIX_LABEL,
                keymap::event_label(&k),
                keymap::PREFIX_LABEL
            ));
            return;
        };
        match b.action {
            Action::Help => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            Action::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                if !self.sidebar_visible {
                    self.focus = Focus::Pane;
                }
            }
            Action::NewThread => self.open_agent_picker(),
            Action::JumpThread => self.open_thread_picker(),
            Action::NextPane => {
                self.focus = match self.focus {
                    Focus::Pane if self.sidebar_visible => Focus::Sidebar,
                    _ => Focus::Pane,
                };
            }
            Action::ScrollMode => {
                self.scroll_mode = true;
                self.focus = Focus::Pane;
            }
            Action::Switch => {
                let (idx, agent) = self.key();
                self.send(&BCommand::SwitchAsk { idx, to: agent.other() });
            }
            Action::Detach => self.detach(),
            _ => self.info(format!("{} ({}) is not built yet in prototype B.", b.desc, keymap::binding_label(b))),
        }
    }

    fn slash(&mut self, text: &str) {
        let mut parts = text.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        let (idx, agent) = self.key();
        match cmd {
            "/switch" => {
                let to = if arg.is_empty() { Some(agent.other()) } else { Agent::parse(&arg) };
                match to {
                    Some(to) => self.send(&BCommand::SwitchAsk { idx, to }),
                    None => self.info(format!("desl drives claude and codex. /switch codex or /switch claude, not {arg}.")),
                }
            }
            "/quit" | "/exit" | "/q" | "/detach" => self.detach(),
            "/help" => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            "/threads" => self.open_thread_picker(),
            "/new" => self.open_agent_picker(),
            _ => self.send(&BCommand::Slash { idx, text: text.to_string() }),
        }
    }

    // ---- pickers -------------------------------------------------------------

    fn open_agent_picker(&mut self) {
        let entries = vec![
            Entry { key: "claude".into(), label: "claude".into(), hint: "Claude Code".into() },
            Entry { key: "codex".into(), label: "codex".into(), hint: "Codex CLI".into() },
        ];
        self.overlay = Overlay::Picker(Picker::new("new thread on", entries), PickKind::NewAgent);
    }

    fn open_thread_picker(&mut self) {
        let entries = self
            .snaps
            .iter()
            .enumerate()
            .map(|(i, t)| Entry {
                key: i.to_string(),
                label: format!("{} {}", t.id, if t.name.is_empty() { "new" } else { &t.name }),
                hint: format!("{} {}", t.status.glyph(), t.agent.name()),
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new("threads", entries), PickKind::Threads);
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Overlay::Picker(p, kind) = &mut self.overlay else { return };
        let kind = *kind;
        match (k.code, k.modifiers.contains(KeyModifiers::CONTROL)) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => self.overlay = Overlay::None,
            (KeyCode::Enter, _) => {
                let chosen = p.current().map(|e| e.key.clone());
                self.overlay = Overlay::None;
                if let Some(key) = chosen {
                    match kind {
                        PickKind::Threads => {
                            if let Ok(i) = key.parse::<usize>() {
                                self.jump_to(i);
                                self.focus = Focus::Pane;
                            }
                        }
                        PickKind::NewAgent => {
                            if let Some(agent) = Agent::parse(&key) {
                                self.jump_to_new = true;
                                self.send(&BCommand::NewThread { agent });
                            }
                        }
                    }
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) | (KeyCode::Char('j'), true) => p.down(),
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) | (KeyCode::Char('k'), true) => p.up(),
            (KeyCode::Backspace, _) => p.backspace(),
            (KeyCode::Char(c), false) => p.type_char(c),
            _ => {}
        }
    }

    // ---- drawing -------------------------------------------------------------

    fn draw(&mut self, f: &mut Frame) {
        if self.threads.is_empty() {
            return;
        }
        let hint = format!("{} ? help", keymap::PREFIX_LABEL);
        let focused_ctx = match self.focus {
            Focus::Pane => Ctx::Pane,
            Focus::Sidebar => Ctx::Sidebar,
        };
        let view = ChromeView {
            project: &self.project,
            threads: &self.threads,
            current: self.current,
            sidebar_focused: self.focus == Focus::Sidebar,
            notice: self.notice.as_ref(),
            git: &self.git,
            clock: &self.clock,
            prefix_pending: self.prefix_pending,
            focused_ctx,
            approval_pending: false,
            hint: &hint,
            prefix_only: true,
            not_built: &[],
        };
        let Some(main) = ui::frame(f, &view, self.sidebar_visible) else { return };
        let [title, pane_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(main);

        let snap = &self.snaps[self.current];
        let agent = snap.agent;
        let branch = self.git.branch.clone().unwrap_or_else(|| "no git".into());
        let mut title_text = format!(" {} · {} · {}", snap.id, agent.name(), branch);
        if self.scroll_mode {
            title_text.push_str(" · scrolling, esc returns");
        }
        f.render_widget(Line::from(title_text).style(theme::muted()), title);

        // One size for every pane, so switching threads never resizes the agents.
        let size = (pane_area.height.max(1), pane_area.width.max(1));
        if size != self.pane_size {
            self.pane_size = size;
            for p in self.screens.values_mut() {
                p.screen_mut().set_size(nz(size.0), nz(size.1));
            }
            let cmd = BCommand::Resize { rows: size.0, cols: size.1 };
            if let Ok(mut line) = serde_json::to_string(&cmd) {
                line.push('\n');
                let _ = self.link.write_all(line.as_bytes());
            }
        }

        let ask_text = self.ask.as_ref().filter(|(i, _, _)| *i == self.current).map(|(_, _, t)| t.clone());
        let exited = snap.panes.iter().find(|(a, _)| *a == agent).and_then(|(_, e)| *e);
        match self.screens.get(&(self.current, agent)) {
            Some(p) => {
                let screen = p.screen();
                widget::render(screen, pane_area, f.buffer_mut());
                let offset = screen.scrollback();
                let bottom = Rect::new(pane_area.x, pane_area.bottom() - 1, pane_area.width, 1);
                let tail = widget::tail_text(screen, 14);
                if offset > 0 {
                    let label = format!(" ↓ {offset} new ");
                    let w = label.len() as u16;
                    if w + 2 <= pane_area.width {
                        let pill = Rect::new(pane_area.right() - w - 1, pane_area.bottom() - 1, w, 1);
                        f.render_widget(Line::from(Span::styled(label, theme::pill())), pill);
                    }
                } else if self.focus == Focus::Pane && !screen.hide_cursor() && !self.scroll_mode && ask_text.is_none() {
                    let (row, col) = screen.cursor_position();
                    f.set_cursor_position(Position::new(pane_area.x + col.min(pane_area.width - 1), pane_area.y + row.min(pane_area.height - 1)));
                }
                // desl's one-line strip over the pane's last row: a question, or an approval the agent is asking.
                if let Some(text) = ask_text {
                    strip(f, bottom, &text, &[("[y]", "es "), ("[n]", "o")]);
                } else if agent == Agent::Claude && snap.status == Status::Waiting && bhost::claude_dialog(&tail) {
                    strip(f, bottom, "claude is asking for approval", &[("[y]", "es "), ("[n]", "o "), ("[a]", "lways")]);
                } else if agent == Agent::Codex && snap.status == Status::Waiting && CODEX_APPROVAL.iter().any(|p| tail.contains(p)) {
                    strip(f, bottom, "codex is asking for approval; answer in its prompt", &[]);
                }
                if let Some(code) = exited {
                    let msg = format!(
                        " {} exited{}. {} n opens a new thread; {} t jumps to another.",
                        agent.name(),
                        code.map(|c| format!(" with code {c}")).unwrap_or_default(),
                        keymap::PREFIX_LABEL,
                        keymap::PREFIX_LABEL
                    );
                    f.render_widget(Clear, bottom);
                    f.render_widget(Line::from(Span::styled(msg, theme::critical())), bottom);
                }
            }
            None => {
                let msg = format!(" Starting {}…", agent.name());
                f.render_widget(Line::from(Span::styled(msg, theme::muted())), pane_area);
            }
        }

        let area = f.area();
        match &mut self.overlay {
            Overlay::Help => {
                let mut scroll = self.help_scroll;
                ui::help::draw(f, &view, &mut scroll, area);
                self.help_scroll = scroll;
            }
            Overlay::Picker(p, _) => ui::picker::draw(f, p, area),
            Overlay::Text { title, lines, scroll } => text_popup(f, title, lines, scroll, area),
            Overlay::None => {}
        }
    }
}

fn is_desl_command(line: &str) -> bool {
    let name = line.split_whitespace().next().unwrap_or("");
    matches!(name, "/switch" | "/brief" | "/info" | "/quit" | "/exit" | "/q" | "/detach" | "/help" | "/threads" | "/new" | "/rename")
}

/// One line over the pane's bottom row, keys anchored right, as in Prototype A's strip.
fn strip(f: &mut Frame, area: Rect, text: &str, keys: &[(&str, &str)]) {
    let keys_w: usize = keys.iter().map(|(k, rest)| k.width() + rest.width()).sum();
    let avail = (area.width as usize).saturating_sub(keys_w + 3);
    let text = proto_a::ui::text::truncate_end(text, avail.max(8));
    let pad = (area.width as usize).saturating_sub(1 + text.width() + keys_w);
    let mut spans = vec![Span::raw(" "), Span::styled(text, theme::warning()), Span::raw(" ".repeat(pad))];
    for (k, rest) in keys {
        spans.push(Span::styled(k.to_string(), theme::accent()));
        spans.push(Span::styled(rest.to_string(), theme::dim()));
    }
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn text_popup(f: &mut Frame, title: &str, lines: &[String], scroll: &mut usize, area: Rect) {
    let width = 84u16.min(area.width.saturating_sub(4)).max(20);
    let body_rows = (area.height.saturating_sub(6) as usize).max(3);
    let height = (lines.len().min(body_rows) + 3) as u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup = Rect::new(x, y, width, height);
    let max_scroll = lines.len().saturating_sub(body_rows);
    *scroll = (*scroll).min(max_scroll);
    f.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme::muted())
        .title(Line::from(format!(" {title} ")).style(theme::muted()));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    let mut visible: Vec<Line<'static>> = lines
        .iter()
        .skip(*scroll)
        .take(body_rows)
        .map(|l| Line::from(Span::styled(format!(" {}", proto_a::ui::text::truncate_end(l, inner.width.saturating_sub(2) as usize)), theme::dim())))
        .collect();
    let below = lines.len().saturating_sub(*scroll + body_rows);
    visible.push(Line::from(Span::styled(
        if below > 0 { format!(" ↓ {below} more · j/k scroll · esc closes") } else { " esc closes".to_string() },
        theme::muted(),
    )));
    f.render_widget(Paragraph::new(visible), inner);
}

/// The pane's size for a terminal of `cols` by `rows`, before the first frame is laid out.
fn guess_pane_size(cols: u16, rows: u16) -> (u16, u16) {
    let sidebar = if cols >= 120 { 24 } else { 20 };
    (rows.saturating_sub(5).max(5), cols.saturating_sub(2 + sidebar).max(20))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir()?;
    match args.first().map(String::as_str) {
        Some("--host") => return bhost::run(args.get(1).map(PathBuf::from).unwrap_or(cwd)),
        Some("stop") => {
            match bhost::connect(&cwd, false)? {
                None => println!("No desl host (prototype B) is running for {}.", cwd.display()),
                Some(mut s) => {
                    let mut line = serde_json::to_string(&BCommand::Shutdown)?;
                    line.push('\n');
                    s.write_all(line.as_bytes())?;
                    println!("Stopped the desl host (prototype B) for {}; its agents ended with it.", cwd.display());
                }
            }
            return Ok(());
        }
        Some("status") => {
            let Some(mut s) = bhost::connect(&cwd, false)? else {
                println!("No desl host (prototype B) is running for {}. Run the prototype here to start one.", cwd.display());
                return Ok(());
            };
            let mut line = serde_json::to_string(&BCommand::Attach { rows: 0, cols: 0 })?;
            line.push('\n');
            s.write_all(line.as_bytes())?;
            let mut buf = String::new();
            BufReader::new(s.try_clone()?).read_line(&mut buf)?;
            if let Ok(BFrame::Threads { threads, .. }) = serde_json::from_str::<BFrame>(&buf) {
                for t in &threads {
                    let name = if t.name.is_empty() { "new" } else { &t.name };
                    println!("{} {}  {:<12} {:<8} {}", t.status.glyph(), t.id, name, t.status.word(), t.agent.name());
                }
            }
            return Ok(());
        }
        Some(other) if other != "attach" => {
            eprintln!("Unknown command `{other}`. Run with no arguments to attach, or `status` or `stop`.");
            std::process::exit(2);
        }
        _ => {}
    }

    let stream = bhost::connect(&cwd, true)?.context("no host")?;
    let reader = stream.try_clone()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if let Ok(frame) = serde_json::from_str::<BFrame>(&line)
                    && tx.send(Msg::Host(frame)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send(Msg::HostGone);
        });
    }

    let (tcols, trows) = ratatui::crossterm::terminal::size().unwrap_or((80, 24));
    let pane_size = guess_pane_size(tcols, trows);
    let project = cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
    let git = git::read(&cwd);
    let mut app = Client {
        cwd,
        project,
        link: stream,
        snaps: Vec::new(),
        threads: Vec::new(),
        screens: HashMap::new(),
        typed: HashMap::new(),
        ask: None,
        current: 0,
        jump_to_new: false,
        focus: Focus::Pane,
        overlay: Overlay::None,
        prefix_pending: false,
        sidebar_visible: true,
        scroll_mode: false,
        notice: None,
        git,
        git_refreshed: Instant::now(),
        clock: now_hhmm(),
        help_scroll: 0,
        should_quit: false,
        farewell: None,
        dirty: true,
        pane_size,
    };
    app.send(&BCommand::Attach { rows: pane_size.0, cols: pane_size.1 });

    let mut terminal = ratatui::init();
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    let mut out = io::stdout();
    if enhanced {
        let _ = execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
    }
    let _ = execute!(out, EnableBracketedPaste);
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(ev) = event::read() {
                if tx.send(Msg::Term(ev)).is_err() {
                    break;
                }
            }
        });
    }

    let handle = |app: &mut Client, msg: Msg| match msg {
        Msg::Term(Event::Key(k)) => {
            if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                app.dirty = true;
                app.on_key(k);
            }
        }
        Msg::Term(Event::Paste(s)) => {
            if app.focus == Focus::Pane {
                let bracketed = app.screens.get(&app.key()).is_some_and(|p| p.screen().bracketed_paste());
                app.write(&keys::paste(&s, bracketed));
            }
        }
        Msg::Term(Event::Resize(..)) => app.dirty = true,
        Msg::Term(_) => {}
        Msg::Host(frame) => app.on_frame(frame),
        Msg::HostGone => {
            if !app.should_quit {
                app.notice = Some(Notice {
                    text: "Lost the desl host. Press ctrl+b d, then run the prototype again.".into(),
                    level: Level::Error,
                    at: Instant::now(),
                });
                app.dirty = true;
            }
        }
    };

    loop {
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(msg) => handle(&mut app, msg),
            Err(mpsc::RecvTimeoutError::Timeout) => app.tick(),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        while let Ok(msg) = rx.try_recv() {
            handle(&mut app, msg);
        }
        if app.should_quit {
            break;
        }
        if app.dirty {
            app.dirty = false;
            terminal.draw(|f| app.draw(f))?;
        }
    }

    let _ = execute!(out, DisableBracketedPaste);
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    if let Some(msg) = app.farewell.take() {
        println!("{msg}");
    }
    Ok(())
}
