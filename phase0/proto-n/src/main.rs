//! proto-n: one thread, the agent's own interface passed straight through to the terminal, and
//! `ctrl+b s` to continue the same conversation in the other agent.
//!
//! The switch copies the conversation into a new session of the other agent, starts that agent
//! out of sight, waits until it has drawn the history, then replaces the screen in one
//! synchronized frame. Throwaway; see `docs/adapters.md` for the session-file findings.

mod convert;
mod keys;
mod term;

use anyhow::{Context, Result};
use convert::{Item, Kind};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

enum Msg {
    Stdin(Vec<u8>),
    Out(u64, Vec<u8>),
    Exit(u64),
}

struct Pane {
    id: u64,
    kind: Kind,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    parser: vt100::Parser,
    /// Everything the agent printed while hidden, replayed onto the terminal at the swap.
    log: Vec<u8>,
    spawned: Instant,
    started_at: SystemTime,
    last_output: Instant,
    saw_output: bool,
    /// The agent's native session id, when desl knows it.
    session: Option<String>,
    /// How many items of the thread's history this session already holds.
    copied: usize,
}

impl Pane {
    fn write(&mut self, b: &[u8]) {
        let _ = self.writer.write_all(b);
        let _ = self.writer.flush();
    }
    fn resize(&mut self, rows: u16, cols: u16) {
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn nz(n: u16) -> std::num::NonZeroU16 {
    std::num::NonZeroU16::new(n.max(1)).unwrap_or(std::num::NonZeroU16::MIN)
}

fn extra_args(kind: Kind) -> Vec<String> {
    let var = match kind {
        Kind::Claude => "PROTO_N_CLAUDE_ARGS",
        Kind::Codex => "PROTO_N_CODEX_ARGS",
    };
    std::env::var(var).unwrap_or_default().split_whitespace().map(String::from).collect()
}

fn spawn(id: u64, kind: Kind, resume: Option<&str>, new_id: Option<&str>, cwd: &Path, size: (u16, u16), tx: Sender<Msg>) -> Result<Pane> {
    let (cols, rows) = size;
    let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).context("opening a pty")?;
    let mut cmd = CommandBuilder::new(kind.name());
    match kind {
        Kind::Claude => {
            if let Some(s) = resume {
                cmd.args(["--resume", s]);
            } else if let Some(s) = new_id {
                cmd.args(["--session-id", s]);
            }
        }
        Kind::Codex => {
            cmd.arg("--no-daemon");
            if let Some(s) = resume {
                cmd.args(["resume", s]);
            }
        }
    }
    cmd.args(extra_args(kind));
    cmd.cwd(cwd);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("CLAUDE") {
            cmd.env_remove(k);
        }
    }
    let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting {}", kind.name()))?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    std::thread::spawn(move || {
        let mut buf = [0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Msg::Out(id, buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(Msg::Exit(id));
    });
    Ok(Pane {
        id,
        kind,
        master: pair.master,
        writer,
        child,
        parser: vt100::Parser::new(nz(rows), nz(cols), 0),
        log: Vec::new(),
        spawned: Instant::now(),
        started_at: SystemTime::now(),
        last_output: Instant::now(),
        saw_output: false,
        session: resume.or(new_id).map(String::from),
        copied: 0,
    })
}

fn out(b: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

fn title(s: &str) {
    out(format!("\x1b]0;{s}\x07").as_bytes());
}

/// Puts the terminal back the way an agent may have changed it: mouse, paste, focus and
/// keyboard modes off, main screen, cursor shown, colours reset.
const RESET: &[u8] = b"\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[<10u\x1b[=0u\x1b[0m\x1b[?25h";

/// Answers what a hidden agent asks the terminal, as the real terminal answered desl.
fn answer(p: &mut Pane, chunk: &[u8], a: &term::Answers) {
    for (_, _, q) in term::scan(chunk) {
        let reply: Option<Vec<u8>> = match q {
            Some(term::Query::CursorPosition) => {
                let (r, c) = p.parser.screen().cursor_position();
                Some(format!("\x1b[{};{}R", r + 1, c + 1).into_bytes())
            }
            Some(term::Query::PrimaryAttributes) => Some(a.da1.clone().unwrap_or_else(|| b"\x1b[?62;22c".to_vec())),
            Some(term::Query::SecondaryAttributes) => Some(b"\x1b[>1;10;0c".to_vec()),
            Some(term::Query::Color(10)) => a.fg.clone(),
            Some(term::Query::Color(11)) => a.bg.clone(),
            Some(term::Query::Color(_)) => a.cursor_color.clone(),
            Some(term::Query::KittyKeyboard) => a.kitty.clone(),
            _ => None,
        };
        if let Some(r) = reply {
            p.write(&r);
        }
    }
}

/// The session file of a pane's agent, finding a Codex session Codex named itself.
fn session_file(p: &mut Pane, cwd: &Path) -> Option<PathBuf> {
    match p.kind {
        Kind::Claude => convert::claude_file(cwd, p.session.as_deref()?),
        Kind::Codex => {
            if let Some(s) = &p.session {
                return convert::codex_file(s);
            }
            let (id, path) = convert::codex_find_new(cwd, p.started_at)?;
            p.session = Some(id);
            Some(path)
        }
    }
}

/// Appends what the terminal sent, escaped, to `$PROTO_N_KEYLOG` when it is set.
fn keylog(b: &[u8]) {
    let Some(path) = std::env::var_os("PROTO_N_KEYLOG") else { return };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", b.escape_ascii());
    }
}

struct App {
    cwd: PathBuf,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    answers: term::Answers,
    visible: Pane,
    pending: Option<Pane>,
    held_input: Vec<u8>,
    /// The prefix key as it arrived, while waiting for the key after it.
    armed: Option<Vec<u8>>,
    /// Swallow release reports right after a desl key, so the agent never sees half a key.
    swallow: bool,
    history: Vec<Item>,
    next_id: u64,
    size: (u16, u16),
    quit: bool,
    notice: Option<String>,
}

impl App {
    fn begin_switch(&mut self) -> Result<()> {
        if self.pending.is_some() {
            return Ok(());
        }
        let to = self.visible.kind.other();
        if let Some(path) = session_file(&mut self.visible, &self.cwd) {
            let items = convert::read(self.visible.kind, &path)?;
            let from = self.visible.copied.min(items.len());
            self.history.extend_from_slice(&items[from..]);
        }
        title(&format!("desl · switching to {}…", to.name()));
        self.next_id += 1;
        let mut p = if self.history.is_empty() {
            let new_id = convert::uuid();
            spawn(self.next_id, to, None, (to == Kind::Claude).then_some(new_id.as_str()), &self.cwd, self.size, self.tx.clone())?
        } else {
            let sid = convert::write(to, &self.history, &self.cwd)?;
            spawn(self.next_id, to, Some(&sid), None, &self.cwd, self.size, self.tx.clone())?
        };
        p.copied = self.history.len();
        self.pending = Some(p);
        Ok(())
    }

    fn finish_switch(&mut self) {
        let Some(p) = self.pending.take() else { return };
        let mut frame = Vec::with_capacity(p.log.len() + 256);
        frame.extend_from_slice(b"\x1b[?2026h");
        frame.extend_from_slice(RESET);
        frame.extend_from_slice(b"\x1b[H\x1b[2J\x1b[3J");
        frame.extend_from_slice(&term::without_queries(&p.log));
        frame.extend_from_slice(b"\x1b[?2026l");
        out(&frame);
        let mut old = std::mem::replace(&mut self.visible, p);
        old.kill();
        self.visible.log = Vec::new();
        title(&format!("desl · {}", self.visible.kind.name()));
        if !self.held_input.is_empty() {
            let held = std::mem::take(&mut self.held_input);
            self.visible.write(&held);
        }
    }

    fn on_stdin(&mut self, b: &[u8]) {
        keylog(b);
        let mut to_agent: Vec<u8> = Vec::new();
        let mut at = 0;
        while at < b.len() {
            let Some((k, n)) = keys::next(&b[at..]) else {
                to_agent.extend_from_slice(&b[at..]);
                break;
            };
            let raw = &b[at..at + n];
            at += n;
            if let Some(prefix) = self.armed.take() {
                if k.noise {
                    self.armed = Some(prefix);
                    continue;
                }
                self.swallow = true;
                match k.code {
                    _ if keys::is_prefix(&k) => to_agent.extend_from_slice(raw),
                    Some(115) => {
                        if let Err(e) = self.begin_switch() {
                            self.notice = Some(format!("The switch did not start: {e:#}. Press ctrl+b s to try again."));
                        }
                    }
                    Some(113) => {
                        self.quit = true;
                        return;
                    }
                    _ => {
                        self.swallow = false;
                        to_agent.extend_from_slice(&prefix);
                        to_agent.extend_from_slice(raw);
                    }
                }
                continue;
            }
            if k.noise && self.swallow {
                continue;
            }
            self.swallow = false;
            if keys::is_prefix(&k) {
                self.armed = Some(raw.to_vec());
                continue;
            }
            to_agent.extend_from_slice(raw);
        }
        if to_agent.is_empty() {
            return;
        }
        if self.pending.is_some() {
            self.held_input.extend_from_slice(&to_agent);
        } else {
            self.visible.write(&to_agent);
        }
    }

    fn run(&mut self) -> Result<()> {
        title(&format!("desl · {}", self.visible.kind.name()));
        loop {
            if self.quit {
                return Ok(());
            }
            match self.rx.recv_timeout(Duration::from_millis(30)) {
                Ok(Msg::Stdin(b)) => self.on_stdin(&b),
                Ok(Msg::Out(id, b)) => {
                    if id == self.visible.id {
                        self.visible.parser.process(&b);
                        out(&b);
                    } else if let Some(p) = self.pending.as_mut().filter(|p| p.id == id) {
                        p.parser.process(&b);
                        p.log.extend_from_slice(&b);
                        p.last_output = Instant::now();
                        p.saw_output = true;
                        answer(p, &b, &self.answers);
                    }
                }
                Ok(Msg::Exit(id)) => {
                    if id == self.visible.id {
                        return Ok(());
                    }
                    if self.pending.as_ref().is_some_and(|p| p.id == id) {
                        let p = self.pending.take();
                        self.notice = Some(format!(
                            "{} quit before it was ready, so desl stayed on {}. Press ctrl+b s to try again.",
                            p.map(|p| p.kind.name()).unwrap_or("The agent"),
                            self.visible.kind.name()
                        ));
                        title(&format!("desl · {} · switch failed", self.visible.kind.name()));
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Ok(()),
            }
            if let Ok(s) = crossterm::terminal::size()
                && s != self.size
                && s.0 > 0
                && s.1 > 0
            {
                self.size = s;
                self.visible.resize(s.1, s.0);
                if let Some(p) = self.pending.as_mut() {
                    p.resize(s.1, s.0);
                }
            }
            // Ready once it has drawn and then gone quiet; never wait forever.
            let ready = self.pending.as_ref().is_some_and(|p| {
                (p.saw_output && p.last_output.elapsed() >= Duration::from_millis(600) && p.spawned.elapsed() >= Duration::from_millis(1200))
                    || p.spawned.elapsed() >= Duration::from_secs(15)
            });
            if ready {
                self.finish_switch();
            }
        }
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let kind = match args.next().as_deref() {
        None | Some("claude") => Kind::Claude,
        Some("codex") => Kind::Codex,
        Some(other) => {
            eprintln!("proto-n: `{other}` is not an agent desl knows. Run `proto-n claude` or `proto-n codex`.");
            std::process::exit(2);
        }
    };
    let cwd = std::env::current_dir()?;
    let (tx, rx) = channel();
    crossterm::terminal::enable_raw_mode().context("putting the terminal in raw mode")?;
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
    // Ask the terminal what agents will ask it, so a hidden agent gets the same answers.
    let mut answers = term::Answers::default();
    out(term::PROBE);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut early: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        if let Ok(Msg::Stdin(b)) = rx.recv_timeout(Duration::from_millis(20)) {
            early.extend_from_slice(&b);
            if answers.take(&early) {
                break;
            }
        }
    }
    let size = crossterm::terminal::size().unwrap_or((80, 24));
    let new_id = convert::uuid();
    let visible = spawn(0, kind, None, (kind == Kind::Claude).then_some(new_id.as_str()), &cwd, size, tx.clone());
    let visible = match visible {
        Ok(v) => v,
        Err(e) => {
            let _ = crossterm::terminal::disable_raw_mode();
            eprintln!("proto-n could not start {}: {e:#}. Check that `{}` runs in this directory.", kind.name(), kind.name());
            std::process::exit(1);
        }
    };
    let mut app = App {
        cwd,
        tx,
        rx,
        answers,
        visible,
        pending: None,
        held_input: Vec::new(),
        armed: None,
        swallow: false,
        history: Vec::new(),
        next_id: 0,
        size,
        quit: false,
        notice: None,
    };
    let res = app.run();
    if let Some(mut p) = app.pending.take() {
        p.kill();
    }
    let kind = app.visible.kind;
    let sid = session_file(&mut app.visible, &app.cwd.clone()).and(app.visible.session.clone());
    app.visible.kill();
    out(RESET);
    let _ = crossterm::terminal::disable_raw_mode();
    println!();
    if let Some(n) = app.notice {
        println!("{n}");
    }
    match (kind, sid) {
        (Kind::Claude, Some(s)) => println!("desl left. The conversation is in claude: claude --resume {s}"),
        (Kind::Codex, Some(s)) => println!("desl left. The conversation is in codex: codex resume {s}"),
        _ => println!("desl left."),
    }
    res
}
