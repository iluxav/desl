//! The client on the native route (SPEC.md "Interface design"). The shown thread's agent runs
//! its own interface, drawn through desl's terminal emulator into the pane beside the sidebar;
//! every key goes to it except the prefix and the key after it.

use crate::conn::{Conn, Link};
use crate::layout::{self, Action, Areas, Row, View};
use crate::rawkeys::{self, Mouse};
use anyhow::{Context, Result, bail};
use desl_ir::{Project, SHELL, Status, Thread};
use desl_proto::{Change, Command, Reply, ServerFrame, b64, read_frame, term};
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
            let ours = self.side.is_some() || self.help;
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
            if m.press && !m.motion() && m.wheel().is_none() && self.side.is_some() {
                self.side = None;
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
            if let Some(id) = layout::thread_at(&self.projects, &self.threads, s, skip, m.y) {
                self.side = None;
                self.dirty = true;
                if id != self.thread_id && self.busy.is_none() {
                    self.go(id);
                }
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
        match a {
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
                let command = Command::NewThread { cwd, agent: Some(SHELL.into()), prompt: None, beside: Some(self.thread_id.clone()) };
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
            Action::New => {
                let cwd = self
                    .threads
                    .iter()
                    .find(|t| t.id == self.thread_id)
                    .map(|t| self.projects.iter().find(|p| p.id == t.project_id).map_or(t.cwd.clone(), |p| p.path.clone()));
                match cwd {
                    Some(cwd) => {
                        let command = Command::NewThread { cwd, agent: None, prompt: None, beside: Some(self.thread_id.clone()) };
                        let id = self.link.request(command);
                        self.busy = Some(Busy::Create(id));
                    }
                    None => self.notice("desl does not know this thread's project yet. Try again in a moment."),
                }
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

    fn side_key(&mut self, key: SKey) {
        self.dirty = true;
        if self.help {
            self.help = false;
            return;
        }
        let Some(cursor) = self.side else { return };
        let rows = layout::rows(&self.projects, &self.threads);
        let thread_at = |i: usize| match rows.get(i) {
            Some(Row::Thread(id)) => Some(id.clone()),
            _ => None,
        };
        match key {
            SKey::Close | SKey::Char('q') => self.side = None,
            SKey::Down | SKey::Char('j') => {
                if let Some(i) = (cursor + 1..rows.len()).find(|&i| thread_at(i).is_some()) {
                    self.side = Some(i);
                }
            }
            SKey::Up | SKey::Char('k') => {
                if let Some(i) = (0..cursor).rev().find(|&i| thread_at(i).is_some()) {
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
            SKey::Char(_) => {}
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
        };
        let screen = self.vt.screen();
        let _ = self.term.draw(|f| layout::draw(f, &view, &a, screen));
        let next = self.term.backend().buffer().clone();
        let mut frame = b"\x1b[?2026h".to_vec();
        frame.extend_from_slice(&layout::paint(self.shown.as_ref(), &next));
        let (row, col) = screen.cursor_position();
        if self.side.is_none() && !self.help && self.scrolled == 0 && !screen.hide_cursor() {
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
            Change::ThreadRemoved { thread_id } => self.threads.retain(|t| t.id != thread_id),
            Change::Project { project } => match self.projects.iter_mut().find(|p| p.id == project.id) {
                Some(p) => *p = project,
                None => self.projects.push(project),
            },
            Change::Bye => {
                return Some("The desl daemon was stopped. Run desl to start it again; the thread resumes where it was.".to_string());
            }
            Change::Appended { .. } | Change::Agents { .. } => return None,
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
