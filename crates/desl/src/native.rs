//! The client on the native route (SPEC.md "Interface design"). The shown thread's agent runs
//! its own interface, drawn through desl's terminal emulator into the pane beside the sidebar;
//! every key goes to it except the prefix and the key after it.

use crate::conn::{Conn, Link};
use crate::layout::{self, Action, Areas, Menu, MenuItem, Row, SubView, View};
use crate::rawkeys::{self, Mouse};
use anyhow::{Context, Result, bail};
use desl_ir::{Event, Project, SHELL, Status, Thread};
use desl_proto::{AgentInfo, Change, Command, Reply, ServerFrame, b64, read_frame, term};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::terminal;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

enum Msg {
    Stdin(Vec<u8>),
    Frame(ServerFrame),
    DaemonGone,
}

fn out(b: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

fn nz(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n.max(1)).unwrap_or(std::num::NonZeroU16::MIN)
}

/// The terminal in raw mode for as long as this lives, put back however the client ends.
struct Raw;

impl Raw {
    fn enter() -> Result<Raw> {
        terminal::enable_raw_mode().context("Could not put the terminal in raw mode. Run desl from a terminal")?;
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        out(term::RESET);
        let _ = terminal::disable_raw_mode();
    }
}

/// The alternate screen, button and wheel reports in SGR encoding.
const SCREEN_ON: &[u8] = b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?25l";
/// Puts the terminal's key modes back to plain before the next agent sets its own.
const KEYS_PLAIN: &[u8] = b"\x1b[<10u\x1b[>4;0m\x1b[?2004l\x1b[?1004l";
/// Lines of the agent's output kept for scrolling back.
const SCROLLBACK: usize = 10_000;
/// At most one paint per this long; output in between is drawn together.
const FRAME: Duration = Duration::from_millis(12);

/// Asks the terminal what agents will ask it. Gives up after half a second; a terminal that
/// answers nothing is fine.
fn probe(rx: &mpsc::Receiver<Msg>) -> term::Answers {
    let mut answers = term::Answers::default();
    out(term::PROBE);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut got: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        if let Ok(Msg::Stdin(b)) = rx.recv_timeout(Duration::from_millis(20)) {
            got.extend_from_slice(&b);
            if answers.take(&got) {
                break;
            }
        }
    }
    answers
}

/// Whether to draw Nerd Font icons: `DESL_ICONS=nerd` or `text` says so; otherwise yes when a
/// Nerd Font is installed here and the terminal is local (over SSH, the font is the other end's).
fn nerd_font() -> bool {
    match std::env::var("DESL_ICONS").as_deref() {
        Ok("nerd") => return true,
        Ok("text") => return false,
        _ => {}
    }
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return false;
    }
    std::process::Command::new("fc-list")
        .args([":", "family"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("Nerd Font"))
}

/// The one request in flight that changes what is on screen.
enum Busy {
    Switch(u64),
    Attach(u64, String),
    Create(u64),
}

/// A key the sidebar understands, from whichever encoding the terminal used.
enum SKey {
    Up,
    Down,
    Enter,
    Close,
    Back,
    Char(char),
}

struct Client {
    link: Link,
    thread_id: String,
    agent: String,
    projects: Vec<Project>,
    threads: Vec<Thread>,
    /// The thread shown before this one, for `prefix l`.
    previous: Option<String>,
    answers: term::Answers,
    /// The agent's screen, as desl's emulator keeps it.
    vt: vt100::Parser,
    cols: u16,
    rows: u16,
    sidebar_on: bool,
    term: Terminal<TestBackend>,
    /// What the terminal shows now, for painting only what changes.
    shown: Option<Buffer>,
    dirty: bool,
    painted: Instant,
    armed: Option<Vec<u8>>,
    /// The key after the prefix was taken: swallow its release report too.
    swallow: bool,
    busy: Option<Busy>,
    /// Keys typed while `busy`, for the agent on screen once it is done.
    held: Vec<u8>,
    notice: Option<String>,
    /// The highlighted sidebar row while the sidebar has the keys.
    side: Option<usize>,
    help: bool,
    scrolled: usize,
    agents: Vec<AgentInfo>,
    /// The new-thread menu's cursor while it is open.
    menu: Option<usize>,
    /// The thread `prefix x` asks to close, until the user answers y or n.
    closing: Option<String>,
    /// A subagent's conversation shown in the pane.
    sub: Option<Sub>,
    /// The menu is `prefix f`'s (fork onto an agent), not `prefix c`'s.
    forking: bool,
    /// `prefix ,`: the name being typed.
    renaming: Option<String>,
    /// `prefix i`: the facts shown.
    info: Option<Vec<(String, String)>>,
    /// The menu's choice gets a git worktree of its own (`w` in the menu).
    menu_worktree: bool,
    /// The close request in flight, whose answer may say what became of a worktree.
    removing: Option<u64>,
}

struct Sub {
    thread_id: String,
    id: String,
    events: Vec<Event>,
    scroll: usize,
    /// The request for its events in flight.
    asked: Option<u64>,
    last: Instant,
}

/// What a new-thread menu row starts: an agent's id, or [`SHELL`].
struct Choice {
    start: String,
    item: MenuItem,
}

/// The user's shell by name: `bash`, `zsh`.
fn shell_name() -> String {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "bash".into());
    std::path::Path::new(&shell).file_name().map_or(shell.clone(), |n| n.to_string_lossy().into_owned())
}

impl Client {
    fn areas(&self) -> Areas {
        layout::areas(self.cols, self.rows, self.sidebar_on)
    }

    fn title(&self) {
        let waiting = self.threads.iter().filter(|t| t.status == Status::Waiting && t.id != self.thread_id).count();
        let tail = if waiting > 0 { format!(" · {waiting} waiting") } else { String::new() };
        out(format!("\x1b]0;desl · {} · {}{tail}\x07", self.agent, self.thread_id).as_bytes());
    }

    fn notice(&mut self, text: &str) {
        self.notice = Some(text.to_string());
        self.dirty = true;
    }

    /// A fresh screen for the agent now shown, with everything it drew so far.
    fn screen_from(&mut self, replay: &[u8]) {
        let pane = self.areas().pane;
        self.vt = vt100::Parser::new(nz(pane.height), nz(pane.width), SCROLLBACK);
        self.vt.process(replay);
        let mut modes = KEYS_PLAIN.to_vec();
        modes.extend_from_slice(&term::key_modes(replay));
        out(&modes);
        self.scrolled = 0;
        self.dirty = true;
    }

    fn input(&mut self, keys: &[u8]) {
        if !keys.is_empty() {
            self.link.request(Command::Input { thread_id: self.thread_id.clone(), data: b64::encode(keys) });
        }
    }

    fn send_keys(&mut self, keys: Vec<u8>) {
        if keys.is_empty() {
            return;
        }
        if self.scrolled > 0 {
            self.scrolled = 0;
            self.dirty = true;
        }
        if self.busy.is_some() {
            self.held.extend_from_slice(&keys);
        } else {
            self.input(&keys);
        }
    }

    fn release_held(&mut self) {
        if !self.held.is_empty() && self.busy.is_none() {
            let keys = std::mem::take(&mut self.held);
            self.input(&keys);
        }
    }

    /// What the terminal sent. Returns the farewell when the user detached.
    fn stdin(&mut self, b: &[u8]) -> Option<String> {
        if self.notice.take().is_some() {
            self.dirty = true;
        }
        let mut to_agent: Vec<u8> = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let rest = &b[at..];
            if let Some((m, n)) = rawkeys::mouse(rest) {
                at += n;
                self.send_keys(std::mem::take(&mut to_agent));
                self.mouse(m);
                continue;
            }
            let ours = self.side.is_some()
                || self.help
                || self.menu.is_some()
                || self.closing.is_some()
                || self.sub.is_some()
                || self.renaming.is_some()
                || self.info.is_some();
            if ours && self.armed.is_none() {
                // Keys rawkeys leaves whole: a lone escape, and arrows in application mode.
                if rest == [0x1b] {
                    self.side_key(SKey::Close);
                    break;
                }
                if rest.len() >= 3 && rest[0] == 0x1b && rest[1] == b'O' {
                    match rest[2] {
                        b'A' => self.side_key(SKey::Up),
                        b'B' => self.side_key(SKey::Down),
                        _ => {}
                    }
                    at += 3;
                    continue;
                }
            }
            let Some((k, n)) = rawkeys::next(rest) else {
                if !ours {
                    to_agent.extend_from_slice(rest);
                }
                break;
            };
            let raw = rest[..n].to_vec();
            at += n;
            if let Some(prefix) = self.armed.take() {
                if k.noise {
                    self.armed = Some(prefix);
                    continue;
                }
                self.swallow = true;
                self.dirty = true;
                if rawkeys::is_prefix(&k) {
                    to_agent.extend_from_slice(&raw);
                    continue;
                }
                match k.code.and_then(char::from_u32).and_then(layout::action) {
                    Some(a) => {
                        self.send_keys(std::mem::take(&mut to_agent));
                        if let Some(bye) = self.act(a) {
                            return Some(bye);
                        }
                    }
                    None => {
                        self.swallow = false;
                        if !ours {
                            to_agent.extend_from_slice(&prefix);
                            to_agent.extend_from_slice(&raw);
                        }
                    }
                }
                continue;
            }
            if k.noise && (self.swallow || ours) {
                continue;
            }
            self.swallow = false;
            if rawkeys::is_prefix(&k) {
                self.armed = Some(raw);
                self.dirty = true;
                continue;
            }
            if ours {
                let key = match k.code {
                    Some(13) => SKey::Enter,
                    Some(27) => SKey::Close,
                    Some(127 | 8) => SKey::Back,
                    Some(c) => match char::from_u32(c) {
                        Some(ch) => SKey::Char(ch),
                        None => continue,
                    },
                    None if raw.starts_with(b"\x1b[") && raw.ends_with(b"A") => SKey::Up,
                    None if raw.starts_with(b"\x1b[") && raw.ends_with(b"B") => SKey::Down,
                    None => continue,
                };
                self.side_key(key);
                continue;
            }
            to_agent.extend_from_slice(&raw);
        }
        self.send_keys(to_agent);
        None
    }

    fn mouse(&mut self, m: Mouse) {
        let a = self.areas();
        let inside = |r: ratatui::layout::Rect| m.x >= r.x && m.x < r.x + r.width && m.y >= r.y && m.y < r.y + r.height;
        if inside(a.pane)
            && let Some(sub) = &mut self.sub
        {
            if let Some(up) = m.wheel() {
                sub.scroll = if up { sub.scroll + 3 } else { sub.scroll.saturating_sub(3) };
                self.dirty = true;
            }
            return;
        }
        if inside(a.pane) {
            let screen = self.vt.screen();
            let wants = match screen.mouse_protocol_mode() {
                MouseProtocolMode::None => false,
                MouseProtocolMode::Press => m.press && !m.motion(),
                MouseProtocolMode::PressRelease => !m.motion(),
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion => true,
            };
            if wants && self.scrolled == 0 {
                let sgr = screen.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr;
                let bytes = m.encode(a.pane.x, a.pane.y, sgr);
                self.input(&bytes);
            } else if let Some(up) = m.wheel() {
                if screen.alternate_screen() && self.scrolled == 0 {
                    // A full-screen program without the mouse: the wheel is its arrow keys.
                    let arrow: &[u8] = match (up, screen.application_cursor()) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    self.input(&arrow.repeat(3));
                } else {
                    self.scrolled = if up { self.scrolled + 3 } else { self.scrolled.saturating_sub(3) };
                    self.dirty = true;
                }
            }
            if m.press && !m.motion() && m.wheel().is_none() && (self.side.is_some() || self.menu.is_some()) {
                self.side = None;
                self.menu = None;
                self.dirty = true;
            }
            return;
        }
        if let Some(s) = a.sidebar
            && inside(s)
            && m.press
            && !m.motion()
            && m.button & 3 == 0
            && m.wheel().is_none()
        {
            let skip = self.side_skip();
            match layout::row_at(&self.projects, &self.threads, s, skip, m.y) {
                Some(Row::Thread(id) | Row::Meta(id)) => {
                    self.side = None;
                    self.sub = None;
                    self.dirty = true;
                    if id != self.thread_id && self.busy.is_none() {
                        self.go(id);
                    }
                }
                Some(Row::Sub(id, k)) => {
                    self.side = None;
                    self.open_sub(&id, k);
                }
                _ => {}
            }
        }
    }

    fn side_skip(&self) -> usize {
        let Some(s) = self.areas().sidebar else { return 0 };
        let rows = layout::rows(&self.projects, &self.threads);
        let keep = self.side.unwrap_or_else(|| rows.iter().position(|r| *r == Row::Thread(self.thread_id.clone())).unwrap_or(0));
        layout::side_skip(rows.len(), keep, s)
    }

    fn ordered(&self) -> Vec<String> {
        layout::order(&self.projects, &self.threads).iter().map(|t| t.id.clone()).collect()
    }

    fn act(&mut self, a: Action) -> Option<String> {
        self.dirty = true;
        match a {
            Action::Detach => return Some("Detached. The agents keep working; run desl to come back.".to_string()),
            Action::Help => {
                self.help = !self.help;
                return None;
            }
            Action::Sidebar => {
                self.sidebar_on = !self.sidebar_on;
                self.side = None;
                self.relayout();
                return None;
            }
            _ => {}
        }
        if self.busy.is_some() {
            return None;
        }
        self.help = false;
        if a != Action::New {
            self.menu = None;
        }
        self.sub = None;
        self.closing = None;
        match a {
            Action::Close => {
                self.side = None;
                let what = match self.threads.iter().find(|t| t.id == self.thread_id) {
                    Some(t) if t.active_agent == SHELL => format!("the shell {}", t.name),
                    Some(t) => format!("{} · {}", t.active_agent, if t.name.is_empty() { "new" } else { t.name.as_str() }),
                    None => self.thread_id.clone(),
                };
                let wt = self.threads.iter().find(|t| t.id == self.thread_id).and_then(|t| t.worktree.clone());
                let note = match (&wt, self.agent == SHELL) {
                    (Some(name), _) => format!(" Its worktree goes if nothing in it is uncommitted; the branch desl/{name} stays."),
                    (None, true) => String::new(),
                    (None, false) => " The agent keeps the conversation in its own history.".to_string(),
                };
                self.notice(&format!("Close {} ({what})?{note}  y close · any other key keeps it", self.thread_id));
                self.closing = Some(self.thread_id.clone());
            }
            Action::Threads if self.side.is_some() => self.side = None,
            Action::Threads => {
                if !self.sidebar_on {
                    self.sidebar_on = true;
                    self.relayout();
                }
                let rows = layout::rows(&self.projects, &self.threads);
                self.side = Some(rows.iter().position(|r| *r == Row::Thread(self.thread_id.clone())).unwrap_or(0));
            }
            Action::Switch if self.agent == SHELL => {
                self.notice("A shell has no conversation to switch. Press ctrl+b c to start an agent where it is.");
            }
            Action::Open => {
                // From beside the current project, the usual place to find another one.
                let here = self.threads.iter().find(|t| t.id == self.thread_id).map(|t| PathBuf::from(&t.cwd));
                let start = here.as_deref().and_then(|p| p.parent()).map(|p| p.to_string_lossy().into_owned());
                let cwd = start.or_else(|| std::env::var("HOME").ok()).unwrap_or_else(|| "/".into());
                let command = Command::NewThread {
                    cwd,
                    agent: Some(SHELL.into()),
                    prompt: None,
                    beside: Some(self.thread_id.clone()),
                    worktree: false,
                };
                let id = self.link.request(command);
                self.busy = Some(Busy::Create(id));
                self.notice("cd to the directory, then press ctrl+b c to start an agent there.");
            }
            Action::Switch => {
                self.side = None;
                let to = if self.agent == "claude" { "codex" } else { "claude" };
                self.notice(&format!("Switching to {to}…"));
                let id = self.link.request(Command::Switch { thread_id: self.thread_id.clone(), agent: to.to_string() });
                self.busy = Some(Busy::Switch(id));
            }
            Action::New | Action::Fork if self.menu.is_some() => self.menu = None,
            Action::Fork if self.agent == SHELL => self.notice("A shell has no conversation to fork."),
            Action::Fork => {
                self.side = None;
                self.forking = true;
                self.menu_worktree = false;
                // The cursor starts on the thread's own agent.
                let at = self.choices().iter().position(|c| c.start == self.agent).unwrap_or(0);
                self.menu = Some(at);
            }
            Action::Rename => {
                self.side = None;
                let name = self.threads.iter().find(|t| t.id == self.thread_id).map(|t| t.name.clone()).unwrap_or_default();
                self.renaming = Some(name);
            }
            Action::Info => {
                self.side = None;
                self.info = Some(self.facts());
            }
            Action::New => {
                self.side = None;
                self.forking = false;
                self.menu_worktree = false;
                // The cursor starts on the project's default agent, else the first that works.
                let choices = self.choices();
                let default = self.project().and_then(|p| p.default_agent.clone());
                let at = choices
                    .iter()
                    .position(|c| Some(&c.start) == default.as_ref() && c.item.enabled)
                    .or_else(|| choices.iter().position(|c| c.item.enabled))
                    .unwrap_or(0);
                self.menu = Some(at);
            }
            Action::Next | Action::Prev => {
                let ids = self.ordered();
                if ids.len() < 2 {
                    self.notice("This is the only thread. Press ctrl+b c for a new one.");
                } else {
                    let here = ids.iter().position(|id| *id == self.thread_id).unwrap_or(0);
                    let to = if a == Action::Next { (here + 1) % ids.len() } else { (here + ids.len() - 1) % ids.len() };
                    self.go(ids[to].clone());
                }
            }
            Action::Jump(n) => match self.ordered().get(n - 1) {
                Some(id) if *id == self.thread_id => self.side = None,
                Some(id) => self.go(id.clone()),
                None => self.notice(&format!("There is no thread {n}.")),
            },
            Action::Last => match self.previous.clone().filter(|id| self.threads.iter().any(|t| t.id == *id)) {
                Some(id) => self.go(id),
                None => self.notice("No thread was shown before this one."),
            },
            Action::Detach | Action::Help | Action::Sidebar => {}
        }
        None
    }

    /// Shows `thread_id` in the pane once the daemon sends what its agent drew.
    fn go(&mut self, thread_id: String) {
        let pane = self.areas().pane;
        let command = Command::Attach { thread_id: thread_id.clone(), rows: pane.height, cols: pane.width, answers: self.answers.clone() };
        let id = self.link.request(command);
        self.busy = Some(Busy::Attach(id, thread_id));
    }

    fn project(&self) -> Option<&Project> {
        let t = self.threads.iter().find(|t| t.id == self.thread_id)?;
        self.projects.iter().find(|p| p.id == t.project_id)
    }

    /// `prefix i`: what the thread is and where its agents keep it.
    fn facts(&self) -> Vec<(String, String)> {
        let Some(t) = self.threads.iter().find(|t| t.id == self.thread_id) else { return Vec::new() };
        let mut out = vec![
            ("Thread".to_string(), t.id.clone()),
            ("Title".to_string(), if t.name.is_empty() { "New thread".into() } else { t.name.clone() }),
            ("Directory".to_string(), layout::tilde(&t.cwd)),
        ];
        if let Some(b) = &t.git.branch {
            out.push(("Branch".into(), b.clone()));
        }
        if let Some(w) = &t.worktree {
            out.push(("Worktree".into(), format!("{w} (its own folder and branch)")));
        }
        if let Some(p) = &t.parent_id {
            out.push(("Forked from".into(), format!("{p} at event {}", t.fork_point.unwrap_or(0))));
        }
        out.push(("Agent".into(), t.active_agent.clone()));
        out.push(("Permissions".into(), t.permission_mode.word().to_string()));
        for (agent, b) in &t.bindings {
            let mark = if *agent == t.active_agent { " (on screen)" } else { "" };
            out.push((format!("{agent} session"), format!("{}{mark}", b.session_id.as_deref().unwrap_or("none yet"))));
            let model = b.model_choice.clone().or(b.model.clone()).unwrap_or_else(|| "its default".into());
            let effort = b.effort_choice.as_deref().map(|e| format!(" · {e}")).unwrap_or_default();
            out.push((format!("{agent} model"), format!("{model}{effort}")));
        }
        if !t.subagents.is_empty() {
            let done = t.subagents.iter().filter(|s| s.finished).count();
            out.push(("Subagents".into(), format!("{} ({done} done)", t.subagents.len())));
        }
        out
    }

    /// What `prefix c` offers: every agent desl drives, then the user's shell. `prefix f`
    /// offers the agents only.
    fn choices(&self) -> Vec<Choice> {
        let mut out: Vec<Choice> = self
            .agents
            .iter()
            .map(|a| {
                let note = if !a.detection.installed {
                    "not installed".to_string()
                } else if a.detection.logged_in == Some(false) {
                    "not logged in".to_string()
                } else {
                    "agent".to_string()
                };
                let mut label = a.id.clone();
                if let Some(f) = label.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                Choice { start: a.id.clone(), item: MenuItem { label, note, enabled: a.usable() } }
            })
            .collect();
        if !self.forking {
            out.push(Choice { start: SHELL.into(), item: MenuItem { label: shell_name(), note: "shell".into(), enabled: true } });
        }
        out
    }

    /// Starts the menu's choice `i` in this project (from a shell: where it is).
    fn start(&mut self, i: usize) {
        let choices = self.choices();
        let Some(choice) = choices.get(i) else { return };
        if !choice.item.enabled {
            let hint = self
                .agents
                .iter()
                .find(|a| a.id == choice.start)
                .map(|a| if a.detection.installed { a.detection.login_hint.clone() } else { a.detection.install_hint.clone() });
            self.notice(&format!("{} is {}. {}", choice.item.label, choice.item.note, hint.unwrap_or_default()));
            return;
        }
        self.menu = None;
        if self.forking {
            let id = self.link.request(Command::Fork {
                thread_id: self.thread_id.clone(),
                agent: Some(choice.start.clone()),
                at: None,
                worktree: self.menu_worktree,
            });
            self.busy = Some(Busy::Create(id));
            return;
        }
        let cwd = match (self.project(), self.threads.iter().find(|t| t.id == self.thread_id)) {
            (Some(p), _) => p.path.clone(),
            (None, Some(t)) => t.cwd.clone(),
            (None, None) => return self.notice("desl does not know this thread's project yet. Try again in a moment."),
        };
        let worktree = self.menu_worktree && choice.start != SHELL;
        let command =
            Command::NewThread { cwd, agent: Some(choice.start.clone()), prompt: None, beside: Some(self.thread_id.clone()), worktree };
        let id = self.link.request(command);
        self.busy = Some(Busy::Create(id));
    }

    fn menu_key(&mut self, key: SKey) {
        let Some(at) = self.menu else { return };
        let n = self.choices().len();
        match key {
            SKey::Close | SKey::Char('q') => self.menu = None,
            SKey::Down | SKey::Char('j') => self.menu = Some((at + 1).min(n.saturating_sub(1))),
            SKey::Up | SKey::Char('k') => self.menu = Some(at.saturating_sub(1)),
            SKey::Enter => self.start(at),
            SKey::Char(c @ '1'..='9') => self.start(c as usize - '1' as usize),
            SKey::Char('w') => self.menu_worktree = !self.menu_worktree,
            SKey::Char(_) | SKey::Back => {}
        }
    }

    /// Shows subagent `k` of `thread_id` in the pane.
    fn open_sub(&mut self, thread_id: &str, k: usize) {
        let Some(s) = self.threads.iter().find(|t| t.id == thread_id).and_then(|t| t.subagents.get(k)) else { return };
        self.sub = Some(Sub {
            thread_id: thread_id.to_string(),
            id: s.id.clone(),
            events: Vec::new(),
            scroll: 0,
            asked: None,
            last: Instant::now(),
        });
        self.ask_sub();
        self.dirty = true;
    }

    fn ask_sub(&mut self) {
        let Some(sub) = &mut self.sub else { return };
        let id = self.link.request(Command::Subagent { thread_id: sub.thread_id.clone(), subagent_id: sub.id.clone() });
        sub.asked = Some(id);
        sub.last = Instant::now();
    }

    /// The subagent shown, as its thread knows it now.
    fn sub_meta(&self) -> Option<(&Thread, &desl_ir::Subagent)> {
        let sub = self.sub.as_ref()?;
        let t = self.threads.iter().find(|t| t.id == sub.thread_id)?;
        Some((t, t.subagents.iter().find(|s| s.id == sub.id)?))
    }

    /// A running subagent's view is read again every second.
    fn tick(&mut self) {
        let running = self.sub_meta().is_some_and(|(t, s)| !s.finished && t.status.busy());
        if let Some(sub) = &self.sub
            && running
            && sub.asked.is_none()
            && sub.last.elapsed() >= Duration::from_secs(1)
        {
            self.ask_sub();
        }
    }

    fn side_key(&mut self, key: SKey) {
        self.dirty = true;
        if self.help {
            self.help = false;
            return;
        }
        if let Some(sub) = &mut self.sub {
            match key {
                SKey::Close | SKey::Char('q') => self.sub = None,
                SKey::Up | SKey::Char('k') => sub.scroll += 1,
                SKey::Down | SKey::Char('j') => sub.scroll = sub.scroll.saturating_sub(1),
                SKey::Char('g') => sub.scroll = usize::MAX / 2,
                SKey::Char('G') => sub.scroll = 0,
                _ => {}
            }
            return;
        }
        if self.info.take().is_some() {
            return;
        }
        if let Some(name) = &mut self.renaming {
            match key {
                SKey::Close => self.renaming = None,
                SKey::Back => {
                    name.pop();
                }
                SKey::Char(c) if !c.is_control() && name.chars().count() < desl_ir::MAX_NAME => name.push(c),
                SKey::Enter => {
                    let name = self.renaming.take().unwrap_or_default();
                    let name = name.trim().to_string();
                    if !name.is_empty() {
                        self.link.request(Command::Rename { thread_id: self.thread_id.clone(), name });
                    }
                }
                _ => {}
            }
            return;
        }
        if let Some(id) = self.closing.take() {
            if matches!(key, SKey::Char('y' | 'Y')) {
                self.removing = Some(self.link.request(Command::RemoveThread { thread_id: id }));
            }
            return;
        }
        if self.menu.is_some() {
            return self.menu_key(key);
        }
        let Some(cursor) = self.side else { return };
        let rows = layout::rows(&self.projects, &self.threads);
        let thread_at = |i: usize| match rows.get(i) {
            Some(Row::Thread(id)) => Some(id.clone()),
            _ => None,
        };
        let pickable = |i: usize| matches!(rows.get(i), Some(Row::Thread(_) | Row::Sub(..)));
        // A click or a cursor on a thread's second line means the thread.
        match key {
            SKey::Enter if matches!(rows.get(cursor), Some(Row::Sub(..))) => {
                if let Some(Row::Sub(id, k)) = rows.get(cursor).cloned() {
                    self.side = None;
                    self.open_sub(&id, k);
                }
            }
            SKey::Close | SKey::Char('q') => self.side = None,
            SKey::Down | SKey::Char('j') => {
                if let Some(i) = (cursor + 1..rows.len()).find(|&i| pickable(i)) {
                    self.side = Some(i);
                }
            }
            SKey::Up | SKey::Char('k') => {
                if let Some(i) = (0..cursor).rev().find(|&i| pickable(i)) {
                    self.side = Some(i);
                }
            }
            SKey::Enter => match thread_at(cursor) {
                Some(id) if id == self.thread_id => self.side = None,
                Some(id) if self.busy.is_none() => self.go(id),
                _ => {}
            },
            SKey::Char(c @ '1'..='9') => {
                self.act(Action::Jump(c as usize - '0' as usize));
            }
            SKey::Char(_) | SKey::Back => {}
        }
    }

    /// The terminal or the sidebar changed size: the agent's screen follows the pane.
    fn relayout(&mut self) {
        let pane = self.areas().pane;
        self.vt.screen_mut().set_size(nz(pane.height), nz(pane.width));
        self.link.request(Command::Resize { thread_id: self.thread_id.clone(), rows: pane.height, cols: pane.width });
        self.dirty = true;
    }

    fn paint(&mut self) {
        let a = self.areas();
        if self.term.backend().buffer().area != ratatui::layout::Rect::new(0, 0, self.cols, self.rows) {
            self.term.backend_mut().resize(self.cols, self.rows);
            self.shown = None;
        }
        self.vt.screen_mut().set_scrollback(self.scrolled);
        self.scrolled = self.vt.screen().scrollback();
        let skip = self.side_skip();
        let view = View {
            projects: &self.projects,
            threads: &self.threads,
            current: &self.thread_id,
            cursor: self.side,
            skip,
            agent: &self.agent,
            prefix_armed: self.armed.is_some(),
            notice: self.notice.as_deref(),
            help: self.help,
            scrolled: self.scrolled,
            menu: None,
            sub: None,
            now: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64),
            shown_sub: self.sub.as_ref().map(|s| (s.thread_id.as_str(), s.id.as_str())),
            renaming: self.renaming.as_deref(),
            info: self.info.as_deref(),
        };
        let choices = self.choices();
        let items: Vec<MenuItem> = choices.into_iter().map(|c| c.item).collect();
        let mut title = if self.forking {
            "Fork onto".to_string()
        } else {
            format!("New in {}", self.project().map_or("this project".to_string(), |p| p.id.clone()))
        };
        if self.menu_worktree {
            title.push_str(" · ⎇ own worktree");
        }
        // A subagent's conversation, laid out by the transcript renderer.
        let width = a.pane.width as usize;
        let sub_lines: Vec<ratatui::text::Line<'static>> = match (&self.sub, self.sub_meta()) {
            (Some(sub), Some((t, _))) => {
                let mut m = crate::model::Thread::new(t.clone());
                m.load(&sub.events, None);
                crate::ui::transcript::layout(&m.items, &[], width, None, None).lines
            }
            _ => Vec::new(),
        };
        let sub_view = self.sub.as_ref().map(|sub| {
            let (title, running) = match self.sub_meta() {
                Some((t, s)) => {
                    let title = if s.description.is_empty() { s.kind.clone() } else { s.description.clone() };
                    (title, !s.finished && t.status.busy())
                }
                None => ("gone".to_string(), false),
            };
            let max = sub_lines.len().saturating_sub(a.pane.height as usize);
            SubView { title, running, lines: &sub_lines, scroll: sub.scroll.min(max) }
        });
        let view = View { menu: self.menu.map(|cursor| Menu { title: &title, items: &items, cursor }), sub: sub_view, ..view };
        let screen = self.vt.screen();
        let _ = self.term.draw(|f| layout::draw(f, &view, &a, screen));
        let next = self.term.backend().buffer().clone();
        let mut frame = b"\x1b[?2026h".to_vec();
        frame.extend_from_slice(&layout::paint(self.shown.as_ref(), &next));
        let (row, col) = screen.cursor_position();
        if self.side.is_none()
            && self.menu.is_none()
            && self.sub.is_none()
            && self.info.is_none()
            && self.renaming.is_none()
            && !self.help
            && self.scrolled == 0
            && !screen.hide_cursor()
        {
            frame.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", a.pane.y + row + 1, a.pane.x + col + 1).as_bytes());
        } else {
            frame.extend_from_slice(b"\x1b[?25l");
        }
        frame.extend_from_slice(b"\x1b[?2026l");
        out(&frame);
        self.shown = Some(next);
        self.dirty = false;
        self.painted = Instant::now();
    }

    /// Returns the farewell when the client should end.
    fn frame(&mut self, frame: ServerFrame) -> Option<String> {
        match frame {
            ServerFrame::Pty { thread_id, data } if thread_id == self.thread_id => {
                let bytes = b64::decode(&data)?;
                self.vt.process(&bytes);
                // Nothing else is there to answer what the agent asks its terminal.
                let replies = term::replies(&bytes, &self.answers, self.vt.screen().cursor_position());
                self.input(&replies);
                let modes = term::key_modes(&bytes);
                if !modes.is_empty() {
                    out(&modes);
                }
                self.dirty = true;
            }
            ServerFrame::PtyEnded { thread_id, .. } if thread_id == self.thread_id => {
                let name = if self.agent == SHELL { "The shell".to_string() } else { self.agent.clone() };
                // Another thread takes the pane; desl ends only with the last one.
                let others: Vec<String> = self.ordered().into_iter().filter(|id| *id != thread_id).collect();
                let next = self.previous.clone().filter(|p| others.contains(p)).or_else(|| others.first().cloned());
                match next {
                    Some(id) if self.busy.is_none() => {
                        self.go(id);
                        self.notice(&format!("{name} in {thread_id} ended."));
                    }
                    _ => return Some(format!("{name} ended. Run desl to open the thread again.")),
                }
            }
            ServerFrame::PtyReplace { thread_id, agent, data } if thread_id == self.thread_id => {
                self.screen_from(&b64::decode(&data).unwrap_or_default());
                self.agent = agent;
                self.notice = None;
                self.title();
                self.busy = None;
                self.release_held();
            }
            ServerFrame::Reply { id, ok, error } => self.reply(id, ok, error),
            ServerFrame::Notice { thread_id, text } if thread_id == self.thread_id => {
                if matches!(self.busy, Some(Busy::Switch(_))) {
                    self.busy = None;
                }
                self.notice(&text);
                self.release_held();
            }
            ServerFrame::Event { change } => return self.change(change),
            _ => {}
        }
        None
    }

    fn reply(&mut self, id: u64, ok: Option<Reply>, error: Option<String>) {
        if self.removing == Some(id) {
            self.removing = None;
            match (ok, error) {
                (Some(Reply::Removed { note: Some(n) }), _) => self.notice(&n),
                (_, Some(sentence)) => self.notice(&sentence),
                _ => {}
            }
            return;
        }
        if let Some(sub) = &mut self.sub
            && sub.asked == Some(id)
        {
            sub.asked = None;
            match (ok, error) {
                (Some(Reply::Events { events }), _) => sub.events = events,
                (_, Some(sentence)) => self.notice = Some(sentence),
                _ => {}
            }
            self.dirty = true;
            return;
        }
        let mine = match &self.busy {
            Some(Busy::Switch(r) | Busy::Attach(r, _) | Busy::Create(r)) => *r == id,
            None => false,
        };
        if !mine {
            return;
        }
        if let Some(sentence) = error {
            self.busy = None;
            self.notice(&sentence);
            self.release_held();
            return;
        }
        match (self.busy.take(), ok) {
            (Some(Busy::Attach(_, want)), Some(Reply::Attached { thread_id, agent, replay })) if thread_id == want => {
                self.screen_from(&b64::decode(&replay).unwrap_or_default());
                if thread_id != self.thread_id {
                    self.previous = Some(std::mem::replace(&mut self.thread_id, thread_id));
                    self.link.request(Command::SaveView { view: serde_json::json!({ "current": self.thread_id }) });
                }
                self.side = None;
                self.agent = agent;
                self.title();
                self.release_held();
            }
            (Some(Busy::Create(_)), Some(Reply::Created { thread_id })) => self.go(thread_id),
            // A switch is done when its screen arrives (`pty_replace`), not when it is accepted.
            (Some(Busy::Switch(r)), _) => self.busy = Some(Busy::Switch(r)),
            _ => self.release_held(),
        }
    }

    fn change(&mut self, change: Change) -> Option<String> {
        match change {
            Change::Thread { thread } => {
                let before = self.threads.iter().find(|t| t.id == thread.id).map(|t| t.status);
                if thread.status == Status::Waiting && before != Some(Status::Waiting) && thread.id != self.thread_id {
                    out(b"\x07");
                }
                match self.threads.iter_mut().find(|t| t.id == thread.id) {
                    Some(t) => *t = *thread,
                    None => self.threads.push(*thread),
                }
            }
            Change::ThreadRemoved { thread_id } => {
                self.threads.retain(|t| t.id != thread_id);
                // The thread on screen was closed: the pane moves on, and desl ends with the last.
                if thread_id == self.thread_id && self.busy.is_none() {
                    let next = self.previous.clone().filter(|p| self.threads.iter().any(|t| t.id == *p));
                    match next.or_else(|| self.ordered().into_iter().next()) {
                        Some(id) => self.go(id),
                        None => return Some("That was the last thread. Run desl to start a new one.".to_string()),
                    }
                }
            }
            Change::Project { project } => match self.projects.iter_mut().find(|p| p.id == project.id) {
                Some(p) => *p = project,
                None => self.projects.push(project),
            },
            Change::Bye => {
                return Some("The desl daemon was stopped. Run desl to start it again; the thread resumes where it was.".to_string());
            }
            Change::Agents { agents } => self.agents = agents,
            Change::Appended { .. } => return None,
        }
        self.title();
        self.dirty = true;
        None
    }

    fn resized(&mut self) {
        if let Ok((c, r)) = terminal::size()
            && (c, r) != (self.cols, self.rows)
            && c > 0
            && r > 0
        {
            (self.cols, self.rows) = (c, r);
            self.relayout();
        }
    }
}

/// Shows the workspace's thread for `cwd` until the user detaches. Returns the line to print
/// once the terminal is back to normal.
pub fn run(mut conn: Conn, cwd: PathBuf) -> Result<Option<String>> {
    let ws = match conn.call(Command::Subscribe { cwd: Some(cwd.to_string_lossy().into_owned()) })? {
        Reply::Workspace(ws) => ws,
        other => bail!("The desl daemon answered with {other:?}. Run `desl daemon restart`"),
    };
    let Some(thread_id) = ws.focus.clone() else { bail!("desl found no thread to open here. Run `desl new` to start one") };
    let raw = Raw::enter()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 8192];
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 || tx.send(Msg::Stdin(buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
    }
    let answers = probe(&rx);
    crate::theme::set_surface_from(answers.bg.as_deref(), answers.fg.as_deref());
    crate::theme::set_nerd(nerd_font());
    out(SCREEN_ON);
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let pane = layout::areas(cols, rows, true).pane;
    let attach = Command::Attach { thread_id: thread_id.clone(), rows: pane.height, cols: pane.width, answers: answers.clone() };
    let (agent, replay) = match conn.call(attach) {
        Ok(Reply::Attached { agent, replay, .. }) => (agent, b64::decode(&replay).unwrap_or_default()),
        Ok(other) => {
            drop(raw);
            bail!("The desl daemon answered attach with {other:?}. Run `desl daemon restart`")
        }
        Err(e) => {
            drop(raw);
            return Err(e);
        }
    };
    let (link, mut reader) = conn.split();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame::<_, ServerFrame>(&mut reader) {
                if tx.send(Msg::Frame(frame)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::DaemonGone);
        });
    }
    let mut c = Client {
        link,
        thread_id,
        agent,
        projects: ws.projects,
        threads: ws.threads,
        previous: None,
        answers,
        vt: vt100::Parser::new(nz(pane.height), nz(pane.width), SCROLLBACK),
        cols,
        rows,
        sidebar_on: true,
        term: Terminal::new(TestBackend::new(cols, rows))?,
        shown: None,
        dirty: true,
        painted: Instant::now(),
        armed: None,
        swallow: false,
        busy: None,
        held: Vec::new(),
        notice: None,
        side: None,
        help: false,
        scrolled: 0,
        agents: ws.agents,
        menu: None,
        closing: None,
        sub: None,
        forking: false,
        renaming: None,
        info: None,
        menu_worktree: false,
        removing: None,
    };
    c.screen_from(&replay);
    c.title();
    c.paint();
    let farewell = loop {
        let wait = if c.dirty { FRAME.saturating_sub(c.painted.elapsed()) } else { Duration::from_millis(50) };
        let msg = match rx.recv_timeout(wait) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        };
        let bye = match msg {
            Some(Msg::Stdin(b)) => c.stdin(&b),
            Some(Msg::Frame(f)) => c.frame(f),
            Some(Msg::DaemonGone) => {
                Some("The desl daemon closed the connection. Run desl to reconnect; the thread resumes where it was.".to_string())
            }
            None => None,
        };
        if bye.is_some() {
            break bye;
        }
        c.resized();
        c.tick();
        if c.dirty && c.painted.elapsed() >= FRAME {
            c.paint();
        }
    };
    drop(raw);
    out(b"\r\n");
    // Give the last keys a moment to reach the daemon before the socket closes.
    std::thread::sleep(Duration::from_millis(30));
    Ok(farewell)
}
