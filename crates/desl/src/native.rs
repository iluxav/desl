//! The client on the native route (SPEC.md "Interface design"). The shown thread's agent runs
//! its own interface, drawn through desl's terminal emulator into the pane beside the sidebar;
//! every key goes to it except the prefix and the key after it.

use crate::conn::{Conn, Link};
use crate::finder::Finder;
use crate::history::{After, Reading};
use crate::layout::{self, Action, Areas, FinderView, Menu, MenuItem, PaneView, Row, Selection, SubKind, SubView, View};
use crate::rawkeys::{self, Mouse};
use anyhow::{Context, Result, bail};
use desl_ir::{FILES, Project, SHELL, Status, TASKS, Thread, is_agent, program_word};
use desl_proto::replay::{self, SCROLLBACK};
use desl_proto::{AgentInfo, Change, Command, Reply, ServerFrame, b64, read_frame, reflow, term};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::terminal;
use ratatui::layout::Rect;
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

/// How the client ended.
pub enum End {
    /// The user detached, or the last program quit: the line to print, if any.
    Line(Option<String>),
    /// The daemon started again in place (SPEC.md "Upgrades"): desl is to start again too, and
    /// finds every thread as it was. The screen is left as it is, for the next desl to draw over.
    Again,
}

/// Puts the terminal back to plain, for a desl that was to start again and could not.
pub fn leave() {
    out(term::RESET);
}

/// The alternate screen, button and wheel reports in SGR encoding.
const SCREEN_ON: &[u8] = b"\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1006h\x1b[?25l";
/// Puts the terminal's key modes back to plain before the next agent sets its own.
const KEYS_PLAIN: &[u8] = b"\x1b[<10u\x1b[>4;0m\x1b[?2004l\x1b[?1004l";
/// At most one paint per this long; output in between is drawn together.
const FRAME: Duration = Duration::from_millis(12);

/// Asks the terminal what agents will ask it. Gives up after half a second; a terminal that
/// answers nothing is fine.
fn probe(rx: &mpsc::Receiver<Msg>) -> term::Answers {
    let mut answers = term::Answers::default();
    // The palette's blue first, for desl's own tint; agents are never told it.
    out(b"\x1b]4;4;?\x1b\\");
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
    let text = String::from_utf8_lossy(&got);
    crate::theme::set_accent_from(text.split("\x1b]4;4;").nth(1));
    answers
}

/// Where a working logo is in its pulse, 0 to 1: one slow breath every 1.6 seconds.
fn pulse() -> f32 {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()) % 1600;
    let phase = ms as f32 / 1600.0 * std::f32::consts::TAU;
    (phase.sin() + 1.0) / 2.0
}

/// Whether to draw Nerd Font icons: `DESL_ICONS=nerd` or `text` says so; otherwise yes when a
/// Nerd Font is installed here and the terminal is local (over SSH, the font is the other end's).
pub(crate) fn nerd_font() -> bool {
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
    Restart(u64),
    /// The request, the thread wanted, and the thread the pane showed when it was asked.
    Attach(u64, String, String),
    Create(u64),
}

/// `prefix ?`: the key list, with what the user typed to filter it and the row under the cursor.
#[derive(Default)]
struct Palette {
    query: String,
    cursor: usize,
}

/// A key the sidebar understands, from whichever encoding the terminal used.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SKey {
    Up,
    Down,
    /// An arrow with shift held: moves what the cursor is on, where something can move.
    MoveUp,
    MoveDown,
    PageUp,
    PageDown,
    Enter,
    Close,
    Back,
    Char(char),
}

/// One pane: the thread it shows and its agent's screen, as desl's emulator keeps it.
struct Pane {
    thread_id: String,
    agent: String,
    vt: vt100::Parser,
    scrolled: usize,
    /// The key modes its agent set (kitty keys, bracketed paste), for the terminal while this
    /// pane has the keys.
    modes: term::KeyModes,
    /// Text being marked with the mouse, or just marked.
    select: Option<Select>,
}

/// A mark made with the mouse in a pane whose program does not take the mouse: from the cell
/// the button went down on to the one the pointer is on, as (row, column) on the pane.
struct Select {
    anchor: (u16, u16),
    head: (u16, u16),
    /// The button is still down.
    dragging: bool,
    /// The pointer has left the cell it went down on: a drag, not a click.
    moved: bool,
}

impl Pane {
    fn new(thread_id: String, agent: String, area: Rect) -> Pane {
        Pane {
            thread_id,
            agent,
            vt: vt100::Parser::new(nz(area.height), nz(area.width), SCROLLBACK),
            scrolled: 0,
            modes: term::KeyModes::default(),
            select: None,
        }
    }

    /// What is marked, once it is more than a click.
    fn selection(&self) -> Option<Selection> {
        self.select.as_ref().filter(|s| s.moved || !s.dragging).map(|s| Selection::between(s.anchor, s.head))
    }
}

/// Two presses on one cell within this long are a double click, three a triple.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// Puts `text` on the clipboard: through the terminal (OSC 52), which reaches the user's own
/// machine over SSH too, and, when desl runs on that machine, through the desktop's tool as
/// well, for a terminal that does not take OSC 52.
fn clipboard(text: &str) {
    out(format!("\x1b]52;c;{}\x07", b64::encode(text.as_bytes())).as_bytes());
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return;
    }
    let tool: &[&str] = if cfg!(target_os = "macos") {
        &["pbcopy"]
    } else if set("WAYLAND_DISPLAY") {
        &["wl-copy"]
    } else if set("DISPLAY") {
        &["xclip", "-selection", "clipboard"]
    } else {
        return;
    };
    let child = std::process::Command::new(tool[0])
        .args(&tool[1..])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        let text = text.to_string();
        // Off the interface's thread: the tool may take a moment, and is not there on every machine.
        std::thread::spawn(move || {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            let _ = child.wait();
        });
    }
}

struct Client {
    link: Link,
    /// The panes on screen, one or two, left to right or top to bottom; `focus` has the keys.
    panes: Vec<Pane>,
    focus: usize,
    /// Two panes sit one above the other, not beside each other.
    stacked: bool,
    projects: Vec<Project>,
    threads: Vec<Thread>,
    /// The thread shown before this one, for `prefix l`.
    previous: Option<String>,
    answers: term::Answers,
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
    /// Where the pane goes once the thread it is moving to has arrived: keys that move between
    /// threads (`alt+j`, the sidebar's cursor) do not wait for each other.
    then: Option<String>,
    /// The thread shown when the sidebar took the keys, for `prefix l` after browsing in it.
    side_from: Option<String>,
    /// Keys typed while `busy`, for the agent on screen once it is done.
    held: Vec<u8>,
    notice: Option<String>,
    /// The highlighted sidebar row while the sidebar has the keys.
    side: Option<usize>,
    /// `prefix ?`: the key list while it is open.
    help: Option<Palette>,
    /// An action picked in the key list, run once its key is handled.
    run: Option<Action>,
    agents: Vec<AgentInfo>,
    /// The file manager `prefix c` offers, when the daemon found one installed.
    file_manager: Option<String>,
    /// The new-thread menu's cursor while it is open.
    menu: Option<usize>,
    /// `prefix o`: the directory finder while it is open.
    finder: Option<Finder>,
    /// The thread `prefix x` asks to close, until the user answers y or n.
    closing: Option<String>,
    /// A conversation shown in the pane: a subagent's, or the thread's own from its start.
    sub: Option<Sub>,
    /// Which key opened the menu, and so what a row in it does.
    menu_for: MenuFor,
    /// `prefix ,`: the name being typed.
    renaming: Option<String>,
    /// `prefix i`: the facts shown.
    info: Option<Vec<(String, String)>>,
    /// The menu's choice gets a git worktree of its own (`w` in the menu).
    menu_worktree: bool,
    /// The last press of the left button in a pane: when, on which cell, and how many in a row.
    clicked: Option<(Instant, (u16, u16), u8)>,
    /// The close request in flight, whose answer may say what became of a worktree.
    removing: Option<u64>,
    /// The request to move a project in flight, whose answer may say it is at the edge already.
    moving: Option<u64>,
    /// The daemon handed its agents to a new one: this client starts again to reach it.
    again: bool,
    prefix: desl_proto::config::Prefix,
    /// The prefix as the interface writes it: `ctrl+space`.
    prefix_label: String,
}

/// A conversation drawn in the pane in place of the agent's screen.
struct Sub {
    thread_id: String,
    /// The subagent whose conversation it is; `None` for the thread's own (`prefix [`).
    id: Option<String>,
    reading: Reading,
    /// The request for its events in flight.
    asked: Option<u64>,
    last: Instant,
}

/// What the menu on screen picks for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuFor {
    /// `prefix c`: a new thread on an agent, a shell or the file manager.
    New,
    /// `prefix f`: this conversation in a new thread, on an agent.
    Fork,
    /// `prefix s` with more than one other agent installed: which one takes the thread.
    Switch,
}

/// What a new-thread menu row starts: an agent's id, [`SHELL`] or [`FILES`].
struct Choice {
    start: String,
    item: MenuItem,
}

/// The agents a menu offers: those installed on this machine. A switch marks the thread's own
/// agent, which it cannot switch to; one that is installed and logged out stays, dimmed, to
/// say what is missing.
fn agent_choices(agents: &[AgentInfo], menu_for: MenuFor, current: &str) -> Vec<Choice> {
    agents
        .iter()
        .filter(|a| a.detection.installed)
        .map(|a| {
            let own = menu_for == MenuFor::Switch && a.id == current;
            let note = if own {
                "✓ current"
            } else if a.detection.logged_in == Some(false) {
                "not logged in"
            } else {
                "agent"
            };
            let mut label = a.id.clone();
            if let Some(f) = label.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            Choice {
                start: a.id.clone(),
                item: MenuItem { label, note: note.to_string(), enabled: a.usable() && !own, kind: a.id.clone() },
            }
        })
        .collect()
}

/// An up or down arrow as the terminal reports it, plain (`CSI A`) or with shift held
/// (`CSI 1;2 A`, and `CSI 1;2:1 A` when a program asked for event types), or a page key.
fn arrow(raw: &[u8]) -> Option<SKey> {
    let (params, last) = raw.strip_prefix(b"\x1b[")?.split_last().map(|(l, p)| (p, *l))?;
    let mods = std::str::from_utf8(params).ok()?.split(';').nth(1).and_then(|m| m.split(':').next()?.parse::<u32>().ok()).unwrap_or(1);
    let shift = mods.saturating_sub(1) & 1 != 0;
    match (last, shift) {
        (b'A', false) => Some(SKey::Up),
        (b'A', true) => Some(SKey::MoveUp),
        (b'B', false) => Some(SKey::Down),
        (b'B', true) => Some(SKey::MoveDown),
        // Page up and page down: `CSI 5 ~` and `CSI 6 ~`.
        (b'~', _) if params.starts_with(b"5") => Some(SKey::PageUp),
        (b'~', _) if params.starts_with(b"6") => Some(SKey::PageDown),
        _ => None,
    }
}

/// The user's shell by name: `bash`, `zsh`.
fn shell_name() -> String {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "bash".into());
    std::path::Path::new(&shell).file_name().map_or(shell.clone(), |n| n.to_string_lossy().into_owned())
}

impl Client {
    fn areas(&self) -> Areas {
        layout::areas(self.cols, self.rows, self.sidebar_on, (self.panes.len() > 1).then_some(self.stacked))
    }

    /// Where pane `i` draws its agent's screen.
    fn rect(&self, i: usize) -> Rect {
        let a = self.areas();
        a.panes.get(i).or(a.panes.first()).copied().unwrap_or_default()
    }

    fn pane(&self) -> &Pane {
        &self.panes[self.focus]
    }

    fn pane_mut(&mut self) -> &mut Pane {
        let i = self.focus;
        &mut self.panes[i]
    }

    /// The thread in the pane that has the keys.
    fn thread_id(&self) -> &str {
        &self.pane().thread_id
    }

    fn agent(&self) -> &str {
        &self.pane().agent
    }

    /// The pane showing `thread_id`, if one does.
    fn pane_of(&self, thread_id: &str) -> Option<usize> {
        self.panes.iter().position(|p| p.thread_id == thread_id)
    }

    /// The keys move to pane `i`; the terminal takes its agent's key modes.
    fn focus(&mut self, i: usize) {
        if i < self.panes.len() && i != self.focus {
            // A conversation being read stays with the pane it was opened in: it closes.
            self.close_sub();
            self.focus = i;
            self.apply_modes();
            self.title();
            self.save_view();
            self.dirty = true;
        }
    }

    /// The terminal takes the focused pane's agent's key modes (kitty keys, bracketed paste).
    fn apply_modes(&self) {
        let mut modes = KEYS_PLAIN.to_vec();
        modes.extend_from_slice(&self.pane().modes.bytes());
        out(&modes);
    }

    /// What to show again next time: the focused thread, the panes in order, and how they sit.
    fn save_view(&mut self) {
        let panes: Vec<&str> = self.panes.iter().map(|p| p.thread_id.as_str()).collect();
        let view = serde_json::json!({ "current": self.thread_id(), "panes": panes, "focus": self.focus, "stacked": self.stacked });
        self.link.request(Command::SaveView { view });
    }

    fn title(&self) {
        let waiting = self.threads.iter().filter(|t| t.status == Status::Waiting && t.id != self.thread_id()).count();
        let tail = if waiting > 0 { format!(" · {waiting} waiting") } else { String::new() };
        out(format!("\x1b]0;desl · {} · {}{tail}\x07", self.agent(), self.thread_id()).as_bytes());
    }

    fn notice(&mut self, text: &str) {
        self.notice = Some(text.to_string());
        self.dirty = true;
    }

    /// A fresh screen in pane `i` for the agent now shown there, with everything it drew so far.
    fn screen_from(&mut self, i: usize, replay: &[u8]) {
        let r = self.rect(i);
        let p = &mut self.panes[i];
        p.vt = vt100::Parser::new(nz(r.height), nz(r.width), SCROLLBACK);
        // What was drawn says at which sizes; the last of them is this pane's, as a rule.
        replay::play(&mut p.vt, replay);
        reflow::resize(&mut p.vt, r.height, r.width);
        p.modes = term::KeyModes::default();
        p.modes.track(replay);
        p.scrolled = 0;
        p.select = None;
        if i == self.focus {
            self.apply_modes();
        }
        self.dirty = true;
    }

    /// The left button in a pane desl marks text in, as tmux does with the mouse on: a drag
    /// marks from where it began to where it is, a double click a word and a triple click a
    /// line, and the mark is copied when the button is let go.
    fn select(&mut self, i: usize, m: Mouse, r: Rect) {
        let cell = (m.y.saturating_sub(r.y).min(r.height.saturating_sub(1)), m.x.saturating_sub(r.x).min(r.width.saturating_sub(1)));
        self.dirty = true;
        if m.press && !m.motion() {
            let now = Instant::now();
            let clicks = match self.clicked {
                Some((at, on, n)) if on == cell && now.duration_since(at) < DOUBLE_CLICK => n % 3 + 1,
                _ => 1,
            };
            self.clicked = Some((now, cell, clicks));
            // A word or a line is marked at once and copied, like a drag, when the button is let go.
            let whole = |s: Selection| Select { anchor: s.from, head: s.to, dragging: true, moved: true };
            self.panes[i].select = match clicks {
                2 => self.word_at(i, cell).map(whole),
                3 => Some(whole(Selection { from: (cell.0, 0), to: (cell.0, r.width.saturating_sub(1)) })),
                _ => Some(Select { anchor: cell, head: cell, dragging: true, moved: false }),
            };
            return;
        }
        let Some(s) = &mut self.panes[i].select else { return };
        if !s.dragging {
            return;
        }
        if m.press {
            s.moved |= cell != s.anchor;
            s.head = cell;
        } else if s.moved {
            s.dragging = false;
            self.copy_marked(i);
        } else {
            // A click that went nowhere marks nothing.
            self.panes[i].select = None;
        }
    }

    /// Back to the agent's screen, with nothing left marked on it.
    fn close_sub(&mut self) {
        if self.sub.take().is_some() {
            self.pane_mut().select = None;
            self.dirty = true;
        }
    }

    /// The conversation drawn in pane `i` in place of its agent's screen, when one is.
    fn read_in(&self, i: usize) -> Option<&Reading> {
        self.sub.as_ref().filter(|_| i == self.focus).map(|sub| &sub.reading)
    }

    /// The word around a cell of pane `i`, on whichever is drawn there.
    fn word_at(&self, i: usize, cell: (u16, u16)) -> Option<Selection> {
        match self.read_in(i) {
            Some(reading) => {
                let (from, to) = reading.word(cell.0, cell.1, usize::from(self.rect(i).height))?;
                Some(Selection { from: (cell.0, from), to: (cell.0, to) })
            }
            None => Selection::word(self.panes[i].vt.screen(), cell.0, cell.1),
        }
    }

    /// Copies what is marked in pane `i` and says how much; a mark over blank cells is dropped.
    fn copy_marked(&mut self, i: usize) {
        let Some(sel) = self.panes[i].selection() else { return };
        let text = match self.read_in(i) {
            Some(reading) => reading.text_between(sel.from, sel.to, usize::from(self.rect(i).height)),
            None => sel.text(self.panes[i].vt.screen()),
        };
        if text.trim().is_empty() {
            self.panes[i].select = None;
            return;
        }
        clipboard(&text);
        let lines = text.trim_end_matches('\n').lines().count();
        let what = if lines > 1 {
            format!("{lines} lines")
        } else {
            let n = text.trim_end_matches('\n').chars().count();
            format!("{n} character{}", if n == 1 { "" } else { "s" })
        };
        self.notice(&format!("Copied {what}."));
    }

    /// Keys for the agent in the focused pane.
    fn input(&mut self, keys: &[u8]) {
        let id = self.thread_id().to_string();
        self.input_to(&id, keys);
    }

    fn input_to(&mut self, thread_id: &str, keys: &[u8]) {
        if !keys.is_empty() {
            self.link.request(Command::Input { thread_id: thread_id.to_string(), data: b64::encode(keys) });
        }
    }

    fn send_keys(&mut self, keys: Vec<u8>) {
        if keys.is_empty() {
            return;
        }
        if self.pane().scrolled > 0 || self.pane().select.is_some() {
            self.pane_mut().scrolled = 0;
            self.pane_mut().select = None;
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
                || self.help.is_some()
                || self.menu.is_some()
                || self.finder.is_some()
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
                if rawkeys::is_prefix(&k, self.prefix) {
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
            // alt+j / alt+k move between threads without the prefix, as in zellij.
            if k.alt && !k.ctrl && !ours {
                let a = match k.code {
                    Some(106) => Some(Action::Next),
                    Some(107) => Some(Action::Prev),
                    Some(104 | 108) if self.panes.len() > 1 => Some(Action::OtherPane),
                    _ => None,
                };
                if let Some(a) = a {
                    self.swallow = true;
                    self.send_keys(std::mem::take(&mut to_agent));
                    if let Some(bye) = self.act(a) {
                        return Some(bye);
                    }
                    continue;
                }
            }
            if rawkeys::is_prefix(&k, self.prefix) {
                self.armed = Some(raw);
                self.dirty = true;
                continue;
            }
            if ours {
                let key = match k.code {
                    Some(106) if k.alt => SKey::Down,
                    Some(107) if k.alt => SKey::Up,
                    Some(_) if k.alt => continue,
                    Some(13) => SKey::Enter,
                    Some(27) => SKey::Close,
                    Some(127 | 8) => SKey::Back,
                    // ctrl and a letter is that letter's control character, as a plain terminal sends it.
                    Some(c @ 97..=122) if k.ctrl => SKey::Char(char::from(u8::try_from(c - 96).unwrap_or(0))),
                    Some(c) => match char::from_u32(c) {
                        Some(ch) => SKey::Char(ch),
                        None => continue,
                    },
                    None => match arrow(&raw) {
                        Some(key) => key,
                        None => continue,
                    },
                };
                self.side_key(key);
                if let Some(a) = self.run.take()
                    && let Some(bye) = self.act(a)
                {
                    return Some(bye);
                }
                continue;
            }
            to_agent.extend_from_slice(&raw);
        }
        self.send_keys(to_agent);
        None
    }

    fn mouse(&mut self, m: Mouse) {
        let a = self.areas();
        // A drag that began in a pane is that pane's until the button is let go, wherever the
        // pointer has gone since.
        if let Some(i) = self.panes.iter().position(|p| p.select.as_ref().is_some_and(|s| s.dragging))
            && m.wheel().is_none()
            && (m.motion() || !m.press)
        {
            return self.select(i, m, a.panes.get(i).copied().unwrap_or_default());
        }
        let inside = |r: Rect| m.x >= r.x && m.x < r.x + r.width && m.y >= r.y && m.y < r.y + r.height;
        if let Some(i) = a.mains.iter().position(|r| inside(*r)) {
            // A click in the other pane gives it the keys first.
            if i != self.focus && m.press && !m.motion() && m.wheel().is_none() {
                self.focus(i);
            }
            if i == self.focus && self.sub.is_some() {
                let r = a.panes.get(i).copied().unwrap_or_default();
                if let Some(up) = m.wheel() {
                    if let Some(sub) = &mut self.sub {
                        sub.reading.wheel(up, usize::from(r.height));
                    }
                    // The mark is on cells of the pane, and other lines are under them now.
                    self.panes[i].select = None;
                    self.dirty = true;
                } else if m.button & 3 == 0 {
                    // desl draws this itself, so it marks the text itself.
                    self.select(i, m, r);
                }
                return;
            }
            let r = a.panes.get(i).copied().unwrap_or_default();
            let thread_id = self.panes[i].thread_id.clone();
            let (mode, sgr, alternate, app_cursor) = {
                let s = self.panes[i].vt.screen();
                (
                    s.mouse_protocol_mode(),
                    s.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr,
                    s.alternate_screen(),
                    s.application_cursor(),
                )
            };
            let wants = match mode {
                MouseProtocolMode::None => false,
                MouseProtocolMode::Press => m.press && !m.motion(),
                MouseProtocolMode::PressRelease => !m.motion(),
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion => true,
            };
            if wants && self.panes[i].scrolled == 0 {
                let bytes = m.encode(r.x, r.y, sgr);
                self.input_to(&thread_id, &bytes);
            } else if let Some(up) = m.wheel() {
                if alternate && self.panes[i].scrolled == 0 {
                    // A full-screen program without the mouse: the wheel is its arrow keys.
                    let arrow: &[u8] = match (up, app_cursor) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    self.input_to(&thread_id, &arrow.repeat(3));
                } else {
                    let p = &mut self.panes[i];
                    p.scrolled = if up { p.scrolled + 3 } else { p.scrolled.saturating_sub(3) };
                    // The mark is on cells of the pane, and other lines are under them now.
                    p.select = None;
                    self.dirty = true;
                }
            } else if m.button & 3 == 0 && self.sub.is_none() {
                // The program does not take the mouse (a shell), or desl's own scrollback is
                // on screen: desl marks the text itself.
                self.select(i, m, r);
            }
            if m.press
                && !m.motion()
                && m.wheel().is_none()
                && (self.side.is_some() || self.menu.is_some() || self.finder.is_some() || self.help.is_some())
            {
                self.side = None;
                self.menu = None;
                self.finder = None;
                self.help = None;
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
                Some(Row::Thread(id)) => {
                    self.side = None;
                    self.close_sub();
                    self.dirty = true;
                    if id != self.thread_id() && self.busy.is_none() {
                        self.go(id);
                    }
                }
                Some(Row::Sub(id, k)) => {
                    self.side = None;
                    self.open_sub(&id, k);
                }
                // A click on a project's row puts the sidebar's cursor on it, ready to be moved.
                Some(row @ Row::Project(_)) => {
                    let rows = layout::rows(&self.projects, &self.threads);
                    self.side = rows.iter().position(|r| *r == row);
                    self.side_from = Some(self.thread_id().to_string());
                    self.close_sub();
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    fn side_skip(&self) -> usize {
        let Some(s) = self.areas().sidebar else { return 0 };
        let rows = layout::rows(&self.projects, &self.threads);
        let keep = self.side.unwrap_or_else(|| rows.iter().position(|r| *r == Row::Thread(self.thread_id().to_string())).unwrap_or(0));
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
                self.help = if self.help.is_some() { None } else { Some(Palette::default()) };
                return None;
            }
            Action::Sidebar => {
                self.sidebar_on = !self.sidebar_on;
                self.side = None;
                self.relayout();
                return None;
            }
            Action::OtherPane => {
                if self.panes.len() > 1 {
                    let i = 1 - self.focus.min(1);
                    self.focus(i);
                }
                return None;
            }
            _ => {}
        }
        if let (Action::Next | Action::Prev, Some(Busy::Attach(_, want, _))) = (a, &self.busy) {
            let ids = self.ordered();
            let from = self.then.clone().unwrap_or_else(|| want.clone());
            if let Some(here) = ids.iter().position(|id| *id == from) {
                let to = if a == Action::Next { (here + 1) % ids.len() } else { (here + ids.len() - 1) % ids.len() };
                self.then = Some(ids[to].clone());
            }
            return None;
        }
        if self.busy.is_some() {
            return None;
        }
        self.help = None;
        if a != Action::New {
            self.menu = None;
        }
        if a != Action::Open {
            self.finder = None;
        }
        self.close_sub();
        self.closing = None;
        match a {
            Action::Close => {
                self.side = None;
                let what = match self.threads.iter().find(|t| t.id == self.thread_id()) {
                    Some(t) if !is_agent(&t.active_agent) => format!("the {} {}", program_word(&t.active_agent), t.name),
                    Some(t) => format!("{} · {}", t.active_agent, if t.name.is_empty() { "new" } else { t.name.as_str() }),
                    None => self.thread_id().to_string(),
                };
                let wt = self.threads.iter().find(|t| t.id == self.thread_id()).and_then(|t| t.worktree.clone());
                let note = match (&wt, !is_agent(self.agent())) {
                    (Some(name), _) => format!(" Its worktree goes if nothing in it is uncommitted; the branch desl/{name} stays."),
                    (None, true) => String::new(),
                    (None, false) => " The agent keeps the conversation in its own history.".to_string(),
                };
                self.notice(&format!("Close {} ({what})?{note}  y close · any other key keeps it", self.thread_id()));
                self.closing = Some(self.thread_id().to_string());
            }
            Action::Threads if self.side.is_some() => self.side = None,
            Action::Threads => {
                if !self.sidebar_on {
                    self.sidebar_on = true;
                    self.relayout();
                }
                let rows = layout::rows(&self.projects, &self.threads);
                self.side = Some(rows.iter().position(|r| *r == Row::Thread(self.thread_id().to_string())).unwrap_or(0));
                self.side_from = Some(self.thread_id().to_string());
            }
            Action::New | Action::Fork | Action::Switch if self.menu.is_some() => self.menu = None,
            Action::Switch if !is_agent(self.agent()) => {
                self.notice(&format!(
                    "A {} has no conversation to switch. Press {} c to start an agent where it is.",
                    program_word(self.agent()),
                    self.prefix_label
                ));
            }
            Action::Open if self.finder.is_some() => self.finder = None,
            Action::Open => {
                self.side = None;
                let known: Vec<String> = self.projects.iter().map(|p| p.path.clone()).collect();
                self.finder = Some(Finder::new(&known));
            }
            Action::Switch => {
                self.side = None;
                // The agents installed here besides this thread's own: one is switched to at
                // once, as the key always did; more open a menu to pick from.
                let others: Vec<String> =
                    self.agents.iter().filter(|a| a.detection.installed && a.id != self.agent()).map(|a| a.id.clone()).collect();
                match others.as_slice() {
                    [] => self.notice("No other agent is installed to hand this thread to. `desl agents` shows what desl looks for."),
                    [one] => self.switch_to(&one.clone()),
                    _ => {
                        self.menu_for = MenuFor::Switch;
                        self.menu_worktree = false;
                        self.menu = Some(self.choices().iter().position(|c| c.item.enabled).unwrap_or(0));
                    }
                }
            }
            Action::Restart if self.agent() == SHELL => {
                self.notice("A shell has no conversation to restart; `exec $SHELL` typed in it starts it again.")
            }
            Action::Restart if self.agent() == FILES || self.agent() == TASKS => self.notice(&format!(
                "A {} has no conversation to restart; q closes it, and {} c opens it again.",
                program_word(self.agent()),
                self.prefix_label
            )),
            Action::Restart => {
                self.side = None;
                self.notice(&format!("Restarting {}…", self.agent()));
                let id = self.link.request(Command::Restart { thread_id: self.thread_id().to_string() });
                self.busy = Some(Busy::Restart(id));
            }
            Action::History if !is_agent(self.agent()) => {
                self.notice(&format!("A {} has no conversation to read.", program_word(self.agent())));
            }
            Action::History => self.open_history(),
            Action::Fork if !is_agent(self.agent()) => {
                self.notice(&format!("A {} has no conversation to fork.", program_word(self.agent())))
            }
            Action::Fork => {
                self.side = None;
                self.menu_for = MenuFor::Fork;
                self.menu_worktree = false;
                // The cursor starts on the thread's own agent.
                let at = self.choices().iter().position(|c| c.start == self.agent()).unwrap_or(0);
                self.menu = Some(at);
            }
            Action::Rename => {
                self.side = None;
                let name = self.threads.iter().find(|t| t.id == self.thread_id()).map(|t| t.name.clone()).unwrap_or_default();
                self.renaming = Some(name);
            }
            Action::Info => {
                self.side = None;
                self.info = Some(self.facts());
            }
            Action::New => {
                self.side = None;
                self.menu_for = MenuFor::New;
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
                    self.notice(&format!("This is the only thread. Press {} c for a new one.", self.prefix_label));
                } else {
                    let here = ids.iter().position(|id| *id == self.thread_id()).unwrap_or(0);
                    let to = if a == Action::Next { (here + 1) % ids.len() } else { (here + ids.len() - 1) % ids.len() };
                    self.go(ids[to].clone());
                }
            }
            Action::Jump(n) => match self.ordered().get(n - 1) {
                Some(id) if *id == self.thread_id() => self.side = None,
                Some(id) => self.go(id.clone()),
                None => self.notice(&format!("There is no thread {n}.")),
            },
            Action::Last => match self.previous.clone().filter(|id| self.threads.iter().any(|t| t.id == *id)) {
                Some(id) => self.go(id),
                None => self.notice("No thread was shown before this one."),
            },
            Action::SplitBeside | Action::SplitStacked => self.split(a == Action::SplitStacked, None),
            Action::Zoom => self.zoom(),
            Action::Detach | Action::Help | Action::Sidebar | Action::OtherPane => {}
        }
        None
    }

    /// Shows `thread_id` in the focused pane once the daemon sends what its agent drew; a thread
    /// the other pane shows gets the keys instead of a copy.
    fn go(&mut self, thread_id: String) {
        if let Some(i) = self.pane_of(&thread_id)
            && i != self.focus
        {
            return self.focus(i);
        }
        let r = self.rect(self.focus);
        let from = self.thread_id().to_string();
        let command = Command::Attach { thread_id: thread_id.clone(), rows: r.height, cols: r.width, answers: self.answers.clone() };
        let id = self.link.request(command);
        self.busy = Some(Busy::Attach(id, thread_id, from));
    }

    /// `prefix %` / `prefix "`: a second pane. It shows `thread`, else the thread shown before
    /// this one, else the next in the list.
    fn split(&mut self, stacked: bool, thread: Option<String>) {
        if self.panes.len() > 1 {
            return self.notice(&format!("Two panes are open already. {} z zooms back to one.", self.prefix_label));
        }
        let here = self.thread_id().to_string();
        let ids = self.ordered();
        let pick = |t: Option<String>| t.filter(|t| *t != here && ids.contains(t));
        let next = || {
            let at = ids.iter().position(|id| *id == here).unwrap_or(0);
            ids.iter().cycle().skip(at + 1).take(ids.len()).find(|id| **id != here).cloned()
        };
        let Some(thread) = pick(thread).or_else(|| pick(self.previous.clone())).or_else(next) else {
            return self.notice(&format!("This is the only thread. Press {} c for a new one to show beside it.", self.prefix_label));
        };
        let agent = self.threads.iter().find(|t| t.id == thread).map(|t| t.active_agent.clone()).unwrap_or_default();
        self.stacked = stacked;
        self.side = None;
        self.panes.push(Pane::new(thread.clone(), agent, Rect::default()));
        self.focus = 1;
        self.relayout();
        self.go(thread);
        self.save_view();
    }

    /// `prefix z`: back to one pane, the one with the keys.
    fn zoom(&mut self) {
        if self.panes.len() < 2 {
            let p = &self.prefix_label;
            return self.notice(&format!("One pane is open. {p} % shows another thread beside it, {p} \" under it."));
        }
        let gone = self.panes.remove(1 - self.focus.min(1));
        self.focus = 0;
        self.link.request(Command::Leave { thread_id: gone.thread_id });
        self.relayout();
        self.save_view();
    }

    fn project(&self) -> Option<&Project> {
        let t = self.threads.iter().find(|t| t.id == self.thread_id())?;
        self.projects.iter().find(|p| p.id == t.project_id)
    }

    /// `prefix i`: what the thread is and where its agents keep it.
    fn facts(&self) -> Vec<(String, String)> {
        let Some(t) = self.threads.iter().find(|t| t.id == self.thread_id()) else { return Vec::new() };
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

    /// What `prefix c` offers: every agent installed here, then the user's shell, their file
    /// manager when it is installed, and the project's task list. `prefix f` and `prefix s` offer the agents only, the
    /// second with the thread's own marked. An agent that is not installed is not a row.
    fn choices(&self) -> Vec<Choice> {
        let mut out = agent_choices(&self.agents, self.menu_for, self.agent());
        if self.menu_for == MenuFor::New {
            out.push(Choice {
                start: SHELL.into(),
                item: MenuItem { label: shell_name(), note: "shell".into(), enabled: true, kind: SHELL.into() },
            });
            if let Some(name) = &self.file_manager {
                out.push(Choice {
                    start: FILES.into(),
                    item: MenuItem { label: name.clone(), note: "files".into(), enabled: true, kind: FILES.into() },
                });
            }
            out.push(Choice {
                start: TASKS.into(),
                item: MenuItem { label: TASKS.into(), note: "task list".into(), enabled: true, kind: TASKS.into() },
            });
        }
        out
    }

    /// Starts the menu's choice `i` in this project (from a shell: where it is).
    fn start(&mut self, i: usize) {
        let choices = self.choices();
        let Some(choice) = choices.get(i) else { return };
        if self.menu_for == MenuFor::Switch && choice.start == self.agent() {
            return self.notice(&format!("This thread is already on {}. Pick another agent, or esc to cancel.", choice.start));
        }
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
        if self.menu_for == MenuFor::Switch {
            return self.switch_to(&choice.start);
        }
        if self.menu_for == MenuFor::Fork {
            let id = self.link.request(Command::Fork {
                thread_id: self.thread_id().to_string(),
                agent: Some(choice.start.clone()),
                at: None,
                worktree: self.menu_worktree,
            });
            self.busy = Some(Busy::Create(id));
            return;
        }
        let cwd = match (self.project(), self.threads.iter().find(|t| t.id == self.thread_id())) {
            (Some(p), _) => p.path.clone(),
            (None, Some(t)) => t.cwd.clone(),
            (None, None) => return self.notice("desl does not know this thread's project yet. Try again in a moment."),
        };
        let worktree = self.menu_worktree && is_agent(&choice.start);
        let command = Command::NewThread {
            cwd,
            agent: Some(choice.start.clone()),
            prompt: None,
            beside: Some(self.thread_id().to_string()),
            worktree,
        };
        let id = self.link.request(command);
        self.busy = Some(Busy::Create(id));
    }

    /// Hands this thread, with its whole conversation, to agent `to`.
    fn switch_to(&mut self, to: &str) {
        self.notice(&format!("Switching to {to}…"));
        let id = self.link.request(Command::Switch { thread_id: self.thread_id().to_string(), agent: to.to_string() });
        self.busy = Some(Busy::Switch(id));
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
            // A switch keeps the thread where it is; only a new thread can get a worktree.
            SKey::Char('w') if self.menu_for != MenuFor::Switch => self.menu_worktree = !self.menu_worktree,
            SKey::Char(_) | SKey::Back | SKey::MoveUp | SKey::MoveDown | SKey::PageUp | SKey::PageDown => {}
        }
    }

    /// Typing filters, arrows or ctrl+n/p move, tab completes a path, enter opens a shell there.
    /// The key list: typing filters it, `↑`/`↓` move, `enter` runs the key under the cursor.
    fn palette_key(&mut self, key: SKey) {
        let Some(p) = &mut self.help else { return };
        let n = layout::palette_rows(&p.query).len();
        match key {
            SKey::Close => self.help = None,
            SKey::Down | SKey::Char('\x0e') => p.cursor = (p.cursor + 1).min(n.saturating_sub(1)),
            SKey::Up | SKey::Char('\x10') => p.cursor = p.cursor.saturating_sub(1),
            SKey::Back => {
                p.query.pop();
                p.cursor = 0;
            }
            SKey::Char('\x15') => {
                p.query.clear();
                p.cursor = 0;
            }
            SKey::Char(c) if !c.is_control() => {
                p.query.push(c);
                p.cursor = 0;
            }
            SKey::Enter => {
                let Some(bind) = layout::palette_rows(&p.query).get(p.cursor).copied() else {
                    return self.notice("No key matches what you typed. Backspace, or esc closes the list.");
                };
                self.help = None;
                // `?` is this list; picking it just closes it.
                self.run = bind.action.filter(|a| *a != Action::Help);
            }
            SKey::Char(_) | SKey::MoveUp | SKey::MoveDown | SKey::PageUp | SKey::PageDown => {}
        }
    }

    fn finder_key(&mut self, key: SKey) {
        let Some(f) = &mut self.finder else { return };
        let n = f.matches().len();
        match key {
            SKey::Close => self.finder = None,
            SKey::Down | SKey::Char('\x0e') => f.cursor = (f.cursor + 1).min(n.saturating_sub(1)),
            SKey::Up | SKey::Char('\x10') => f.cursor = f.cursor.saturating_sub(1),
            SKey::Char('\t') => f.complete(),
            SKey::Back => {
                f.query.pop();
                f.cursor = 0;
            }
            // ctrl+u clears, ctrl+w takes back a word, as in a shell.
            SKey::Char('\x15') => {
                f.query.clear();
                f.cursor = 0;
            }
            SKey::Char('\x17') => {
                let keep = f.query.trim_end_matches('/').rfind(['/', ' ']).map_or(0, |i| i + 1);
                f.query.truncate(keep);
                f.cursor = 0;
            }
            SKey::Char(c) if !c.is_control() => {
                f.query.push(c);
                f.cursor = 0;
            }
            SKey::Enter => {
                let Some((dir, _)) = f.matches().get(f.cursor).cloned() else {
                    return self.notice("No directory matches. Type a path like ~/src/app, or esc to cancel.");
                };
                let shown = f.tilde(&dir);
                self.finder = None;
                let command = Command::NewThread {
                    cwd: dir.to_string_lossy().into_owned(),
                    agent: Some(SHELL.into()),
                    prompt: None,
                    beside: None,
                    worktree: false,
                };
                let id = self.link.request(command);
                self.busy = Some(Busy::Create(id));
                self.notice(&format!("A shell in {shown}. Press {} c to start an agent here.", self.prefix_label));
            }
            SKey::Char(_) | SKey::MoveUp | SKey::MoveDown | SKey::PageUp | SKey::PageDown => {}
        }
    }

    /// Shows subagent `k` of `thread_id` in the pane.
    fn open_sub(&mut self, thread_id: &str, k: usize) {
        let Some(s) = self.threads.iter().find(|t| t.id == thread_id).and_then(|t| t.subagents.get(k)) else { return };
        self.sub = Some(Sub {
            thread_id: thread_id.to_string(),
            id: Some(s.id.clone()),
            reading: Reading::new(),
            asked: None,
            last: Instant::now(),
        });
        self.ask_sub();
        self.dirty = true;
    }

    /// `prefix [`: the whole conversation of the thread in the focused pane, as the daemon's
    /// log has it. It is read once, when it opens: like tmux's copy mode it shows what there
    /// was then, and the agent goes on behind it.
    fn open_history(&mut self) {
        self.side = None;
        self.sub =
            Some(Sub { thread_id: self.thread_id().to_string(), id: None, reading: Reading::new(), asked: None, last: Instant::now() });
        self.ask_sub();
        self.dirty = true;
    }

    fn ask_sub(&mut self) {
        let Some(sub) = &mut self.sub else { return };
        let thread_id = sub.thread_id.clone();
        let id = self.link.request(match &sub.id {
            Some(subagent_id) => Command::Subagent { thread_id, subagent_id: subagent_id.clone() },
            None => Command::Events { thread_id },
        });
        sub.asked = Some(id);
        sub.last = Instant::now();
    }

    /// The subagent shown, as its thread knows it now.
    fn sub_meta(&self) -> Option<(&Thread, &desl_ir::Subagent)> {
        let sub = self.sub.as_ref()?;
        let id = sub.id.as_ref()?;
        let t = self.threads.iter().find(|t| t.id == sub.thread_id)?;
        Some((t, t.subagents.iter().find(|s| s.id == *id)?))
    }

    /// A running subagent's view is read again every second; a working thread's logo pulses.
    fn tick(&mut self) {
        if self.finder.as_mut().is_some_and(Finder::poll) {
            self.dirty = true;
        }
        if self.sidebar_on
            && self.threads.iter().any(|t| t.status == Status::Working)
            && self.painted.elapsed() >= Duration::from_millis(120)
        {
            self.dirty = true;
        }
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
        // Shift and an arrow reorder in the thread list; in a menu or a list over it they are the arrow.
        let listing = self.help.is_none()
            && self.finder.is_none()
            && self.sub.is_none()
            && self.info.is_none()
            && self.renaming.is_none()
            && self.closing.is_none()
            && self.menu.is_none();
        let key = match key {
            SKey::MoveUp if !listing => SKey::Up,
            SKey::MoveDown if !listing => SKey::Down,
            key => key,
        };
        if self.help.is_some() {
            return self.palette_key(key);
        }
        if self.finder.is_some() {
            return self.finder_key(key);
        }
        if self.sub.is_some() {
            // A mark was on what the pane showed before this key.
            self.pane_mut().select = None;
            let height = usize::from(self.rect(self.focus).height);
            let after = self.sub.as_mut().map_or(After::Stay, |sub| sub.reading.key(key, height));
            match after {
                After::Stay => {}
                After::Close => self.close_sub(),
                After::Say(sentence) => self.notice(&sentence),
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
                        self.link.request(Command::Rename { thread_id: self.thread_id().to_string(), name });
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
        // The cursor stops on a project's row too: that is where a project is moved from.
        let pickable = |i: usize| matches!(rows.get(i), Some(Row::Project(_) | Row::Thread(_) | Row::Sub(..)));
        match key {
            SKey::MoveUp | SKey::MoveDown => match rows.get(cursor) {
                Some(Row::Project(id)) => {
                    self.moving = Some(self.link.request(Command::MoveProject { project_id: id.clone(), up: key == SKey::MoveUp }));
                }
                _ => self.notice("Shift and an arrow move a project. Put the cursor on the project's row first, with k."),
            },
            // A project's row stands for its first thread.
            SKey::Enter if matches!(rows.get(cursor), Some(Row::Project(_))) => {
                self.side = None;
                self.preview(thread_at(cursor + 1));
            }
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
                    self.preview(thread_at(i));
                }
            }
            SKey::Up | SKey::Char('k') => {
                if let Some(i) = (0..cursor).rev().find(|&i| pickable(i)) {
                    self.side = Some(i);
                    self.preview(thread_at(i));
                }
            }
            // The pane already shows the thread under the cursor: enter gives it the keys.
            SKey::Enter => {
                self.side = None;
                self.preview(thread_at(cursor));
            }
            SKey::Char(c @ '1'..='9') => {
                self.act(Action::Jump(c as usize - '0' as usize));
            }
            SKey::Char(_) | SKey::Back | SKey::PageUp | SKey::PageDown => {}
        }
    }

    /// Shows the thread under the sidebar's cursor in the pane as the cursor moves (tmux's
    /// choose-tree, lazygit's panels); a move made while another is arriving goes after it.
    fn preview(&mut self, thread: Option<String>) {
        let Some(id) = thread else { return };
        match &self.busy {
            None if id != self.thread_id() => self.go(id),
            Some(Busy::Attach(..)) => self.then = Some(id),
            _ => {}
        }
    }

    /// The terminal, the sidebar or the split changed: each agent's screen follows its pane.
    fn relayout(&mut self) {
        let a = self.areas();
        for (i, p) in self.panes.iter_mut().enumerate() {
            let Some(r) = a.panes.get(i) else { continue };
            reflow::resize(&mut p.vt, r.height, r.width);
            p.select = None;
            self.link.request(Command::Resize { thread_id: p.thread_id.clone(), rows: r.height, cols: r.width });
        }
        self.dirty = true;
    }

    fn paint(&mut self) {
        let a = self.areas();
        if self.term.backend().buffer().area != ratatui::layout::Rect::new(0, 0, self.cols, self.rows) {
            self.term.backend_mut().resize(self.cols, self.rows);
            self.shown = None;
        }
        for p in &mut self.panes {
            p.vt.screen_mut().set_scrollback(p.scrolled);
            p.scrolled = p.vt.screen().scrollback();
        }
        let focused = self.rect(self.focus);
        // A conversation in place of the agent's screen is laid out for the pane's width.
        if let Some(sub) = &mut self.sub {
            sub.reading.page(usize::from(focused.width));
        }
        let skip = self.side_skip();
        let view = View {
            projects: &self.projects,
            threads: &self.threads,
            panes: self
                .panes
                .iter()
                .map(|p| PaneView { thread_id: &p.thread_id, agent: &p.agent, scrolled: p.scrolled, selection: p.selection() })
                .collect(),
            focus: self.focus,
            cursor: self.side,
            skip,
            prefix_armed: self.armed.is_some(),
            notice: self.notice.as_deref(),
            help: self.help.as_ref().map(|p| layout::PaletteView { query: &p.query, cursor: p.cursor }),
            menu: None,
            finder: None,
            sub: None,
            now: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64),
            shown_sub: self.sub.as_ref().and_then(|s| Some((s.thread_id.as_str(), s.id.as_deref()?))),
            renaming: self.renaming.as_deref(),
            info: self.info.as_deref(),
            pulse: pulse(),
            prefix: &self.prefix_label,
        };
        let choices = self.choices();
        let items: Vec<MenuItem> = choices.into_iter().map(|c| c.item).collect();
        let mut title = match self.menu_for {
            MenuFor::Fork => "Fork onto".to_string(),
            MenuFor::Switch => "Switch to".to_string(),
            MenuFor::New => format!("New in {}", self.project().map_or("this project".to_string(), |p| p.id.clone())),
        };
        if self.menu_worktree {
            title.push_str(&format!(" · {} own worktree", crate::icons::branch()));
        }
        // A conversation in place of the agent's screen, laid out by the transcript renderer.
        let height = usize::from(focused.height);
        let subagent = self.sub_meta().map(|(t, s)| {
            let title = if s.description.is_empty() { s.kind.clone() } else { s.description.clone() };
            (title, !s.finished && t.status.busy())
        });
        let read = self.sub.as_ref().and_then(|sub| self.threads.iter().find(|t| t.id == sub.thread_id));
        let read = read.map(|t| if t.name.is_empty() { t.id.clone() } else { t.name.clone() });
        let sub_view = self.sub.as_ref().map(|sub| {
            let (title, kind) = match (&sub.id, subagent) {
                (None, _) => (read.unwrap_or_default(), SubKind::History { place: sub.reading.place(height) }),
                (Some(_), Some((title, running))) => (title, SubKind::Subagent { running }),
                (Some(_), None) => ("gone".to_string(), SubKind::Subagent { running: false }),
            };
            SubView {
                title,
                kind,
                lines: sub.reading.lines(),
                top: sub.reading.top(height),
                hit: sub.reading.hit(),
                typing: sub.reading.typing(),
                tools: sub.reading.tools(),
            }
        });
        let finder_rows: Vec<(String, &'static str)> =
            self.finder.as_ref().map(|f| f.matches().iter().map(|(p, k)| (f.tilde(p), k.note())).collect()).unwrap_or_default();
        let finder = self.finder.as_ref().map(|f| FinderView {
            query: &f.query,
            rows: &finder_rows,
            cursor: f.cursor.min(finder_rows.len().saturating_sub(1)),
            scanning: f.scanning(),
        });
        let switching = self.menu_for == MenuFor::Switch;
        let menu = self.menu.map(|cursor| Menu {
            title: &title,
            items: &items,
            cursor,
            verb: if switching { "switch" } else { "start" },
            worktree: !switching,
        });
        let view = View { menu, sub: sub_view, finder, ..view };
        let screens: Vec<&vt100::Screen> = self.panes.iter().map(|p| p.vt.screen()).collect();
        let _ = self.term.draw(|f| layout::draw(f, &view, &a, &screens));
        let screen = self.pane().vt.screen();
        let next = self.term.backend().buffer().clone();
        let mut frame = b"\x1b[?2026h".to_vec();
        frame.extend_from_slice(&layout::paint(self.shown.as_ref(), &next));
        let (row, col) = screen.cursor_position();
        if self.side.is_none()
            && self.menu.is_none()
            && self.finder.is_none()
            && self.sub.is_none()
            && self.info.is_none()
            && self.renaming.is_none()
            && self.help.is_none()
            && self.pane().scrolled == 0
            && !screen.hide_cursor()
        {
            frame.extend_from_slice(format!("\x1b[{};{}H\x1b[?25h", focused.y + row + 1, focused.x + col + 1).as_bytes());
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
            ServerFrame::Pty { thread_id, data } => {
                let i = self.pane_of(&thread_id)?;
                let bytes = b64::decode(&data)?;
                let p = &mut self.panes[i];
                p.vt.process(&bytes);
                // A finished mark was on what the screen showed; one being dragged stays with the pointer.
                if p.select.as_ref().is_some_and(|s| !s.dragging) {
                    p.select = None;
                }
                // Nothing else is there to answer what the agent asks its terminal.
                let replies = term::replies(&bytes, &self.answers, p.vt.screen().cursor_position());
                let modes = p.modes.track(&bytes);
                if !modes.is_empty() && i == self.focus {
                    out(&modes);
                }
                self.input_to(&thread_id, &replies);
                self.dirty = true;
            }
            ServerFrame::PtyEnded { thread_id, .. } => {
                let i = self.pane_of(&thread_id)?;
                // A restart whose agent could not start again ends as any other exit.
                if matches!(self.busy, Some(Busy::Restart(_))) {
                    self.busy = None;
                }
                let agent = self.panes[i].agent.clone();
                let name = if is_agent(&agent) { agent } else { format!("The {}", program_word(&agent)) };
                if self.panes.len() > 1 {
                    // Its pane closes; the other one stays.
                    self.panes.remove(i);
                    self.focus = 0;
                    self.apply_modes();
                    self.relayout();
                    self.save_view();
                    self.notice(&format!("{name} in {thread_id} ended; its pane closed."));
                    return None;
                }
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
            ServerFrame::PtyReplace { thread_id, agent, data } => {
                let i = self.pane_of(&thread_id)?;
                self.screen_from(i, &b64::decode(&data).unwrap_or_default());
                self.panes[i].agent = agent;
                self.notice = None;
                self.title();
                if matches!(self.busy, Some(Busy::Switch(_) | Busy::Restart(_))) {
                    self.busy = None;
                }
                self.release_held();
            }
            ServerFrame::Reply { id, ok, error } => self.reply(id, ok, error),
            ServerFrame::Notice { thread_id, text } if self.pane_of(&thread_id).is_some() => {
                if matches!(self.busy, Some(Busy::Switch(_) | Busy::Restart(_))) {
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
        if self.moving == Some(id) {
            self.moving = None;
            if let Some(sentence) = error {
                self.notice(&sentence);
            }
            return;
        }
        if let Some(sub) = &mut self.sub
            && sub.asked == Some(id)
        {
            sub.asked = None;
            match (ok, error) {
                (Some(Reply::Events { events }), _) => sub.reading.load(events),
                (_, Some(sentence)) => {
                    // Nothing came to read: back to the agent, with the reason.
                    self.close_sub();
                    self.notice = Some(sentence);
                }
                _ => {}
            }
            self.dirty = true;
            return;
        }
        let mine = match &self.busy {
            Some(Busy::Switch(r) | Busy::Restart(r) | Busy::Attach(r, _, _) | Busy::Create(r)) => *r == id,
            None => false,
        };
        if !mine {
            return;
        }
        if let Some(sentence) = error {
            self.busy = None;
            self.then = None;
            self.notice(&sentence);
            self.release_held();
            return;
        }
        match (self.busy.take(), ok) {
            (Some(Busy::Attach(_, want, from)), Some(Reply::Attached { thread_id, agent, replay })) if thread_id == want => {
                let Some(p) = self.pane_of(&from) else {
                    // The pane closed while its thread was arriving.
                    self.link.request(Command::Leave { thread_id });
                    self.then = None;
                    return self.release_held();
                };
                self.screen_from(p, &b64::decode(&replay).unwrap_or_default());
                // Browsing in the sidebar keeps its keys; any other move gives them back.
                let rows = layout::rows(&self.projects, &self.threads);
                let browsing = self.side.and_then(|c| rows.get(c)).is_some_and(|r| *r == Row::Thread(thread_id.clone()))
                    || (self.side.is_some() && self.then.is_some());
                if thread_id != from {
                    self.panes[p].thread_id = thread_id.clone();
                    if self.pane_of(&from).is_none() {
                        self.link.request(Command::Leave { thread_id: from.clone() });
                    }
                    self.previous = match (browsing, self.side_from.clone()) {
                        (true, Some(other)) if other != thread_id => Some(other),
                        (true, _) => self.previous.take(),
                        (false, _) => Some(from),
                    };
                    self.save_view();
                }
                if !browsing {
                    self.side = None;
                }
                self.panes[p].agent = agent;
                self.title();
                match self.then.take() {
                    Some(next) if next != self.thread_id() => self.go(next),
                    _ => self.release_held(),
                }
            }
            (Some(Busy::Create(_)), Some(Reply::Created { thread_id })) => self.go(thread_id),
            // A switch or a restart is done when its screen arrives (`pty_replace`), not when it is accepted.
            (Some(Busy::Switch(r)), _) => self.busy = Some(Busy::Switch(r)),
            (Some(Busy::Restart(r)), _) => self.busy = Some(Busy::Restart(r)),
            _ => self.release_held(),
        }
    }

    fn change(&mut self, change: Change) -> Option<String> {
        match change {
            Change::Thread { thread } => {
                let before = self.threads.iter().find(|t| t.id == thread.id).map(|t| t.status);
                if thread.status == Status::Waiting && before != Some(Status::Waiting) && self.pane_of(&thread.id).is_none() {
                    out(b"\x07");
                }
                match self.threads.iter_mut().find(|t| t.id == thread.id) {
                    Some(t) => *t = *thread,
                    None => self.threads.push(*thread),
                }
            }
            Change::ThreadRemoved { thread_id } => {
                self.threads.retain(|t| t.id != thread_id);
                if let Some(i) = self.pane_of(&thread_id) {
                    if self.panes.len() > 1 {
                        // Its pane closes; the other one stays.
                        self.panes.remove(i);
                        self.focus = 0;
                        self.apply_modes();
                        self.relayout();
                        self.save_view();
                    } else if self.busy.is_none() {
                        // The thread on screen was closed: the pane moves on, and desl ends with the last.
                        let next = self.previous.clone().filter(|p| self.threads.iter().any(|t| t.id == *p));
                        match next.or_else(|| self.ordered().into_iter().next()) {
                            Some(id) => self.go(id),
                            None => return Some("That was the last thread. Run desl to start a new one.".to_string()),
                        }
                    }
                }
            }
            Change::ProjectOrder { ids } => {
                // The cursor stays on the row it was on, wherever that row is now.
                let on = self.side.and_then(|i| layout::rows(&self.projects, &self.threads).get(i).cloned());
                self.projects.sort_by_key(|p| ids.iter().position(|id| *id == p.id).unwrap_or(usize::MAX));
                if let Some(row) = on {
                    self.side = layout::rows(&self.projects, &self.threads).iter().position(|r| *r == row).or(self.side);
                }
            }
            Change::Project { project } => match self.projects.iter_mut().find(|p| p.id == project.id) {
                Some(p) => *p = project,
                None => self.projects.push(project),
            },
            Change::Bye => {
                return Some("The desl daemon was stopped. Run desl to start it again; the thread resumes where it was.".to_string());
            }
            Change::Restarting => {
                self.again = true;
                return Some(String::new());
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
pub fn run(mut conn: Conn, cwd: PathBuf, settings: desl_proto::config::Config) -> Result<End> {
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
    let pane = layout::areas(cols, rows, true, None).panes[0];
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
    let saved = ws.view.clone().unwrap_or_default();
    let saved_panes: Vec<String> = saved
        .get("panes")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    let saved_focus = saved.get("focus").and_then(|f| f.as_u64()).unwrap_or(0) as usize;
    let saved_stacked = saved.get("stacked").and_then(|s| s.as_bool()).unwrap_or(false);
    let mut c = Client {
        link,
        panes: vec![Pane::new(thread_id, agent, pane)],
        focus: 0,
        stacked: false,
        projects: ws.projects,
        threads: ws.threads,
        previous: None,
        answers,
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
        then: None,
        side_from: None,
        held: Vec::new(),
        notice: None,
        side: None,
        help: None,
        run: None,
        agents: ws.agents,
        file_manager: ws.file_manager,
        menu: None,
        finder: None,
        closing: None,
        sub: None,
        menu_for: MenuFor::New,
        renaming: None,
        info: None,
        menu_worktree: false,
        clicked: None,
        removing: None,
        moving: None,
        again: false,
        prefix: settings.prefix,
        prefix_label: settings.prefix.label(),
    };
    if let Some(problem) = settings.problems.first() {
        c.notice(&format!("{problem} `desl config show` lists every setting."));
    }
    c.screen_from(0, &replay);
    c.title();
    // The split from last time, when both its threads are still here, in the same order.
    if saved_panes.len() == 2
        && let Some(other) = saved_panes.iter().find(|id| **id != c.thread_id()).cloned()
        && c.threads.iter().any(|t| t.id == other)
    {
        c.split(saved_stacked, Some(other.clone()));
        if c.panes.len() == 2 && saved_panes[0] == other {
            c.panes.swap(0, 1);
            c.focus = 1;
        }
        c.focus = c.focus.min(1);
        c.focus(saved_focus.min(1));
        c.relayout();
    }
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
    if c.again {
        // Only the terminal's line settings go back, so the next desl records the right ones
        // to restore; what is on screen stays until it paints.
        std::mem::forget(raw);
        let _ = terminal::disable_raw_mode();
        return Ok(End::Again);
    }
    drop(raw);
    out(b"\r\n");
    // Give the last keys a moment to reach the daemon before the socket closes.
    std::thread::sleep(Duration::from_millis(30));
    Ok(End::Line(farewell))
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_ir::adapter::Detection;

    fn agent(id: &str, installed: bool, logged_in: Option<bool>) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            detection: Detection { installed, logged_in, ..Detection::default() },
            capabilities: Default::default(),
            mode_notes: Vec::new(),
        }
    }

    #[test]
    fn shift_and_an_arrow_is_told_from_the_arrow_in_every_encoding() {
        assert_eq!(arrow(b"\x1b[A"), Some(SKey::Up));
        assert_eq!(arrow(b"\x1b[B"), Some(SKey::Down));
        assert_eq!(arrow(b"\x1b[1;2A"), Some(SKey::MoveUp));
        assert_eq!(arrow(b"\x1b[1;2B"), Some(SKey::MoveDown));
        assert_eq!(arrow(b"\x1b[1;2:1B"), Some(SKey::MoveDown), "with an event type, as the kitty protocol reports it");
        assert_eq!(arrow(b"\x1b[1;5A"), Some(SKey::Up), "ctrl is not shift");
        assert_eq!(arrow(b"\x1b[1;2C"), None);
        assert_eq!(arrow(b"j"), None);
    }

    #[test]
    fn a_menu_offers_the_agents_installed_here_and_a_switch_marks_the_threads_own() {
        let agents = [agent("claude", true, Some(true)), agent("codex", false, None), agent("copilot", true, None)];
        let rows = |menu_for, current: &str| -> Vec<(String, bool)> {
            agent_choices(&agents, menu_for, current).into_iter().map(|c| (c.item.label, c.item.enabled)).collect()
        };
        assert_eq!(rows(MenuFor::New, "claude"), [("Claude".to_string(), true), ("Copilot".to_string(), true)], "codex is not installed");
        assert_eq!(rows(MenuFor::Fork, "claude").len(), 2, "a fork may stay on the same agent");
        assert_eq!(rows(MenuFor::Switch, "claude"), [("Claude".to_string(), false), ("Copilot".to_string(), true)]);
        assert_eq!(agent_choices(&agents, MenuFor::Switch, "claude")[0].item.note, "✓ current", "the thread's own agent is marked");
        // Installed and logged out: shown, and not startable, so the row can say why.
        let out = [agent("claude", true, Some(false))];
        let row = &agent_choices(&out, MenuFor::New, "")[0];
        assert_eq!((row.item.note.as_str(), row.item.enabled), ("not logged in", false));
    }
}
