//! The client on the native route (SPEC.md "Interface design"): the shown thread's agent draws
//! straight onto this terminal and gets every key, except the prefix and the key after it.
//! desl draws only its overlays, on the alternate screen, and the terminal title.

use crate::conn::{Conn, Link};
use crate::overlay::{self, Action, ListView, Row};
use crate::rawkeys;
use anyhow::{Context, Result, bail};
use desl_ir::{Project, Status, Thread};
use desl_proto::{Change, Command, Reply, ServerFrame, b64, read_frame, term};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::terminal;
use ratatui::layout::Rect;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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

/// Asks the terminal what agents will ask it, so the daemon can answer for it while no client
/// shows an agent. Gives up after half a second; a terminal that answers nothing is fine.
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

/// What the agent drew while an overlay covered it is kept up to this much; past it, closing
/// the overlay asks the daemon for the whole screen again.
const COVERED_MAX: usize = 4 * 1024 * 1024;

enum Kind {
    /// The row of the list that is highlighted.
    List(usize),
    Help,
}

struct Overlay {
    kind: Kind,
    /// Drawn in memory and painted whole: ratatui talking to the terminal itself would ask it
    /// where the cursor is, and the answer would go to the stdin reader.
    term: Terminal<TestBackend>,
    /// The agent was on the alternate screen already, so the overlay drew over it there and
    /// closing it needs the agent's screen from the daemon.
    over_alternate: bool,
}

/// The one request in flight that changes what is on screen.
enum Busy {
    Switch(u64),
    Attach(u64, String),
    Create(u64),
}

/// A key the overlays understand, from whichever encoding the terminal used.
enum OKey {
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
    modes: term::Modes,
    rows: u16,
    cols: u16,
    armed: Option<Vec<u8>>,
    /// The key after the prefix was taken: swallow its release report too.
    swallow: bool,
    busy: Option<Busy>,
    /// Keys typed while `busy`, for the agent on screen once it is done.
    held: Vec<u8>,
    /// A sentence is in the title until the next key.
    notice: bool,
    overlay: Option<Overlay>,
    covered: Vec<u8>,
    covered_overflow: bool,
}

impl Client {
    fn title(&self) {
        let waiting = self.threads.iter().filter(|t| t.status == Status::Waiting && t.id != self.thread_id).count();
        let tail = if waiting > 0 { format!(" · {waiting} waiting") } else { String::new() };
        out(format!("\x1b]0;desl · {} · {}{tail}\x07", self.agent, self.thread_id).as_bytes());
    }

    fn notice(&mut self, text: &str) {
        out(format!("\x1b]0;desl · {} · {text}\x07", self.agent).as_bytes());
        self.notice = true;
    }

    fn send_keys(&mut self, keys: Vec<u8>) {
        if keys.is_empty() {
            return;
        }
        if self.busy.is_some() {
            self.held.extend_from_slice(&keys);
        } else {
            self.link.request(Command::Input { thread_id: self.thread_id.clone(), data: b64::encode(&keys) });
        }
    }

    fn release_held(&mut self) {
        if !self.held.is_empty() && self.busy.is_none() {
            let keys = std::mem::take(&mut self.held);
            self.send_keys(keys);
        }
    }

    /// What the terminal sent. Returns the farewell when the user detached.
    fn stdin(&mut self, b: &[u8]) -> Option<String> {
        if self.notice {
            self.notice = false;
            self.title();
        }
        let mut to_agent: Vec<u8> = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let rest = &b[at..];
            if self.overlay.is_some() && self.armed.is_none() {
                // Keys rawkeys leaves whole: a lone escape, and arrows in application mode.
                if rest == [0x1b] {
                    self.overlay_key(OKey::Close);
                    break;
                }
                if rest.len() >= 3 && rest[0] == 0x1b && rest[1] == b'O' {
                    match rest[2] {
                        b'A' => self.overlay_key(OKey::Up),
                        b'B' => self.overlay_key(OKey::Down),
                        _ => {}
                    }
                    at += 3;
                    continue;
                }
            }
            let Some((k, n)) = rawkeys::next(rest) else {
                if self.overlay.is_none() {
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
                if rawkeys::is_prefix(&k) {
                    if self.overlay.is_none() {
                        to_agent.extend_from_slice(&raw);
                    }
                    continue;
                }
                match k.code.and_then(char::from_u32).and_then(overlay::action) {
                    Some(a) => {
                        self.send_keys(std::mem::take(&mut to_agent));
                        if let Some(bye) = self.act(a) {
                            return Some(bye);
                        }
                    }
                    None => {
                        self.swallow = false;
                        if self.overlay.is_none() {
                            to_agent.extend_from_slice(&prefix);
                            to_agent.extend_from_slice(&raw);
                        }
                    }
                }
                continue;
            }
            if k.noise && (self.swallow || self.overlay.is_some()) {
                continue;
            }
            self.swallow = false;
            if rawkeys::is_prefix(&k) {
                self.armed = Some(raw);
                continue;
            }
            if self.overlay.is_some() {
                let key = match k.code {
                    Some(13) => OKey::Enter,
                    Some(27) => OKey::Close,
                    Some(c) => match char::from_u32(c) {
                        Some(ch) => OKey::Char(ch),
                        None => continue,
                    },
                    None if raw.starts_with(b"\x1b[") && raw.ends_with(b"A") => OKey::Up,
                    None if raw.starts_with(b"\x1b[") && raw.ends_with(b"B") => OKey::Down,
                    None => continue,
                };
                self.overlay_key(key);
                continue;
            }
            to_agent.extend_from_slice(&raw);
        }
        self.send_keys(to_agent);
        None
    }

    fn ordered(&self) -> Vec<String> {
        overlay::order(&self.projects, &self.threads).iter().map(|t| t.id.clone()).collect()
    }

    fn act(&mut self, a: Action) -> Option<String> {
        if a == Action::Detach {
            return Some("Detached. The agents keep working; run desl to come back.".to_string());
        }
        if self.busy.is_some() {
            return None;
        }
        match a {
            Action::List if matches!(self.overlay, Some(Overlay { kind: Kind::List(_), .. })) => self.close_overlay(),
            Action::Help if matches!(self.overlay, Some(Overlay { kind: Kind::Help, .. })) => self.close_overlay(),
            Action::List => {
                let rows = overlay::rows(&self.projects, &self.threads);
                let at = rows.iter().rposition(|r| matches!(r, Row::Thread { id, .. } if *id == self.thread_id)).unwrap_or(0);
                self.open_overlay(Kind::List(at));
            }
            Action::Help => self.open_overlay(Kind::Help),
            Action::Switch => {
                self.close_overlay();
                if self.busy.is_none() {
                    let to = if self.agent == "claude" { "codex" } else { "claude" };
                    out(format!("\x1b]0;desl · switching to {to}…\x07").as_bytes());
                    let id = self.link.request(Command::Switch { thread_id: self.thread_id.clone(), agent: to.to_string() });
                    self.busy = Some(Busy::Switch(id));
                }
            }
            Action::New => {
                let cwd = self
                    .threads
                    .iter()
                    .find(|t| t.id == self.thread_id)
                    .and_then(|t| self.projects.iter().find(|p| p.id == t.project_id).map(|p| p.path.clone()).or(Some(t.cwd.clone())));
                match cwd {
                    Some(cwd) => {
                        let id = self.link.request(Command::NewThread { cwd, agent: None, prompt: None });
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
                Some(id) if *id == self.thread_id => self.close_overlay(),
                Some(id) => self.go(id.clone()),
                None => self.notice(&format!("There is no thread {n}. Press ctrl+b w for the list.")),
            },
            Action::Last => match self.previous.clone().filter(|id| self.threads.iter().any(|t| t.id == *id)) {
                Some(id) => self.go(id),
                None => self.notice("No thread was shown before this one."),
            },
            Action::Detach => {}
        }
        None
    }

    /// Shows `thread_id`. An open overlay stays until the new screen replaces it in one frame.
    fn go(&mut self, thread_id: String) {
        let id = self.link.request(Command::Attach {
            thread_id: thread_id.clone(),
            rows: self.rows,
            cols: self.cols,
            answers: term::Answers::default(),
        });
        self.busy = Some(Busy::Attach(id, thread_id));
    }

    fn overlay_key(&mut self, key: OKey) {
        let cursor = match &self.overlay {
            Some(Overlay { kind: Kind::List(c), .. }) => *c,
            Some(_) => return self.close_overlay(),
            None => return,
        };
        let rows = overlay::rows(&self.projects, &self.threads);
        let thread_at = |i: usize| match rows.get(i) {
            Some(Row::Thread { id, .. }) => Some(id.clone()),
            _ => None,
        };
        let moved = match key {
            OKey::Close | OKey::Char('q') => return self.close_overlay(),
            OKey::Down | OKey::Char('j') => (cursor + 1..rows.len()).find(|&i| thread_at(i).is_some()),
            OKey::Up | OKey::Char('k') => (0..cursor).rev().find(|&i| thread_at(i).is_some()),
            OKey::Enter => {
                match thread_at(cursor) {
                    Some(id) if id == self.thread_id => self.close_overlay(),
                    Some(id) if self.busy.is_none() => self.go(id),
                    _ => {}
                }
                return;
            }
            OKey::Char(c @ '1'..='9') => {
                self.act(Action::Jump(c as usize - '0' as usize));
                return;
            }
            OKey::Char(_) => return,
        };
        if let (Some(i), Some(Overlay { kind: Kind::List(c), .. })) = (moved, &mut self.overlay) {
            *c = i;
            self.redraw();
        }
    }

    fn open_overlay(&mut self, kind: Kind) {
        if self.overlay.is_some() {
            if let Some(o) = &mut self.overlay {
                o.kind = kind;
            }
            self.redraw();
            return;
        }
        let over_alternate = self.modes.alternate;
        // End a frame the agent may have left half-drawn, so the terminal shows the overlay.
        out(b"\x1b[?2026l");
        if !over_alternate {
            out(b"\x1b[?1049h");
        }
        let Ok(term) = Terminal::new(TestBackend::new(self.cols, self.rows));
        self.overlay = Some(Overlay { kind, term, over_alternate });
        self.redraw();
    }

    fn redraw(&mut self) {
        let Some(o) = &mut self.overlay else { return };
        if o.term.backend().buffer().area != Rect::new(0, 0, self.cols, self.rows) {
            o.term.backend_mut().resize(self.cols, self.rows);
        }
        match &mut o.kind {
            Kind::List(cursor) => {
                let rows = overlay::rows(&self.projects, &self.threads);
                if !matches!(rows.get(*cursor), Some(Row::Thread { .. })) {
                    let below = (*cursor..rows.len()).find(|&i| matches!(rows[i], Row::Thread { .. }));
                    let above = (0..(*cursor).min(rows.len())).rev().find(|&i| matches!(rows[i], Row::Thread { .. }));
                    *cursor = below.or(above).unwrap_or(0);
                }
                let view =
                    ListView { projects: &self.projects, threads: &self.threads, current: &self.thread_id, cursor: *cursor, now: now_ms() };
                let _ = o.term.draw(|f| overlay::draw_list(f, &view));
            }
            Kind::Help => {
                let _ = o.term.draw(overlay::draw_help);
            }
        }
        out(&overlay::paint(o.term.backend().buffer()));
    }

    /// Takes the overlay away and puts the agent's screen back as it was.
    fn close_overlay(&mut self) {
        let Some(o) = self.overlay.take() else { return };
        let over_alternate = o.over_alternate;
        drop(o);
        let covered = std::mem::take(&mut self.covered);
        if over_alternate || self.covered_overflow {
            self.covered_overflow = false;
            if self.busy.is_none() {
                let id = self.thread_id.clone();
                self.go(id);
            }
            return;
        }
        out(b"\x1b[H\x1b[2J\x1b[?1049l");
        out(if self.modes.cursor_hidden { b"\x1b[?25l" } else { b"\x1b[?25h" });
        out(&term::without_titles(&covered));
    }

    /// Returns the farewell when the client should end.
    fn frame(&mut self, frame: ServerFrame) -> Option<String> {
        match frame {
            ServerFrame::Pty { thread_id, data } if thread_id == self.thread_id => {
                let bytes = b64::decode(&data)?;
                if self.overlay.is_some() {
                    if self.covered.len() + bytes.len() > COVERED_MAX {
                        self.covered.clear();
                        self.covered_overflow = true;
                    } else if !self.covered_overflow {
                        self.covered.extend_from_slice(&bytes);
                    }
                    // What the agent sets now applies once the overlay closes.
                    if self.overlay.as_ref().is_some_and(|o| !o.over_alternate) {
                        self.modes.track(&bytes);
                    }
                    return None;
                }
                self.modes.track(&bytes);
                out(&term::without_titles(&bytes));
                // tmux takes an APC (a kitty graphics query, say) as a title: put desl's back.
                if bytes.windows(2).any(|w| w == b"\x1b_") {
                    self.title();
                }
            }
            ServerFrame::PtyEnded { thread_id, .. } if thread_id == self.thread_id => {
                return Some(format!("{} ended. Run desl to open the thread again; it resumes the same conversation.", self.agent));
            }
            ServerFrame::PtyReplace { thread_id, agent, data } if thread_id == self.thread_id => {
                let bytes = b64::decode(&data).unwrap_or_default();
                out(&term::replace_screen(&bytes));
                self.modes = term::Modes::default();
                self.modes.track(&bytes);
                self.agent = agent;
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
            self.close_overlay();
            self.notice(&sentence);
            self.release_held();
            return;
        }
        match (self.busy.take(), ok) {
            (Some(Busy::Attach(_, want)), Some(Reply::Attached { thread_id, agent, replay })) if thread_id == want => {
                // The new screen replaces the overlay and the old agent in one frame.
                self.overlay = None;
                self.covered.clear();
                self.covered_overflow = false;
                let bytes = b64::decode(&replay).unwrap_or_default();
                out(&term::replace_screen(&bytes));
                self.modes = term::Modes::default();
                self.modes.track(&bytes);
                if thread_id != self.thread_id {
                    self.previous = Some(std::mem::replace(&mut self.thread_id, thread_id));
                    self.link.request(Command::SaveView { view: serde_json::json!({ "current": self.thread_id }) });
                }
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
        if !self.notice && self.busy.is_none() {
            self.title();
        }
        self.redraw();
        None
    }

    fn resized(&mut self) {
        if let Ok((c, r)) = terminal::size()
            && (c, r) != (self.cols, self.rows)
            && c > 0
            && r > 0
        {
            (self.cols, self.rows) = (c, r);
            self.link.request(Command::Resize { thread_id: self.thread_id.clone(), rows: r, cols: c });
            self.redraw();
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
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let (agent, replay) = match conn.call(Command::Attach { thread_id: thread_id.clone(), rows, cols, answers }) {
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
    out(&term::replace_screen(&replay));
    let mut modes = term::Modes::default();
    modes.track(&replay);
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
        modes,
        rows,
        cols,
        armed: None,
        swallow: false,
        busy: None,
        held: Vec::new(),
        notice: false,
        overlay: None,
        covered: Vec::new(),
        covered_overflow: false,
    };
    c.title();
    let farewell = loop {
        let msg = match rx.recv_timeout(Duration::from_millis(50)) {
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
    };
    c.overlay = None;
    drop(raw);
    out(b"\r\n");
    // Give the last keys a moment to reach the daemon before the socket closes.
    std::thread::sleep(Duration::from_millis(30));
    Ok(farewell)
}
