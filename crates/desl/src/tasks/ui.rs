//! `desl task ui`: a project's task list on screen (SPEC.md "Tasks"). desl's own program, run
//! in a terminal the daemon owns as a files thread runs yazi, so it detaches, splits and
//! survives a restart like any other thread. A flat list, filtered by status in tabs along the
//! top; a task's description shows when it is opened. It reads the file again whenever it
//! changes, so what an agent does with `desl task` shows at once, and lights up for a moment.

use super::editor::Field;
use super::list::{Change, Edit, Kind, List, Missing, Priority, Stamp, Status, Store, Task};
use crate::theme;
use crate::ui::text::truncate_end;
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

/// How long a task someone else changed stays lit.
const FRESH: Duration = Duration::from_secs(5);
/// How often the file is looked at.
const LOOK: Duration = Duration::from_millis(250);

// ---- how things look ---------------------------------------------------------------------------

/// A status's mark. With a Nerd Font, its Material Design icons: an empty circle, a half-full
/// one, an alert, a check and a cancel. Without, the same shapes in plain Unicode.
fn status_glyph(s: Status) -> &'static str {
    status_mark(s, theme::nerd())
}

fn status_mark(s: Status, nerd: bool) -> &'static str {
    match (s, nerd) {
        (Status::Todo, true) => "\u{f0130}",
        (Status::InProgress, true) => "\u{f1396}",
        (Status::Issue, true) => "\u{f0028}",
        (Status::Done, true) => "\u{f05e0}",
        (Status::Ignored, true) => "\u{f073a}",
        // A merge arrow, and a pause.
        (Status::Ready, true) => "\u{f062c}",
        (Status::Parked, true) => "\u{f03e4}",
        (Status::Todo, false) => "○",
        (Status::InProgress, false) => "◐",
        (Status::Issue, false) => "▲",
        (Status::Done, false) => "✓",
        (Status::Ignored, false) => "⊘",
        (Status::Ready, false) => "⇡",
        (Status::Parked, false) => "‖",
    }
}

fn status_style(s: Status) -> Style {
    match s {
        Status::Todo => theme::normal(),
        Status::InProgress => theme::accent(),
        Status::Issue => theme::warning(),
        Status::Done => theme::added(),
        Status::Ignored => theme::muted(),
        Status::Ready => theme::added(),
        Status::Parked => theme::muted(),
    }
}

fn status_label(s: Status) -> &'static str {
    match s {
        Status::Todo => "Todo",
        Status::InProgress => "In progress",
        Status::Issue => "Issue",
        Status::Done => "Done",
        Status::Ignored => "Ignored",
        Status::Ready => "Ready",
        Status::Parked => "Parked",
    }
}

/// A type's mark: a Nerd Font's Material Design icon when desl uses them (a sparkle, a bug, a
/// checked box, recycling arrows, a test tube, an open book, a bulb), else none: the word
/// carries it.
pub(crate) fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Feature => "\u{f0ae2}",
        Kind::Bug => "\u{f00e4}",
        Kind::Task => "\u{f0135}",
        Kind::Refactor => "\u{f044c}",
        Kind::Test => "\u{f0668}",
        Kind::Docs => "\u{f05da}",
        Kind::Idea => "\u{f0335}",
    }
}

/// After a title that has a description: a page with lines, or `≡`.
fn note_mark(nerd: bool) -> &'static str {
    if nerd { "\u{f09ed}" } else { "≡" }
}

pub(crate) fn kind_style(k: Kind) -> Style {
    theme::hue(match k {
        Kind::Feature => Color::Magenta,
        Kind::Bug => Color::Red,
        Kind::Task => Color::Reset,
        Kind::Refactor => Color::Cyan,
        Kind::Test => Color::Green,
        Kind::Docs => Color::Blue,
        Kind::Idea => Color::Yellow,
    })
}

/// Five columns at most, for the list.
pub(crate) fn kind_short(k: Kind) -> &'static str {
    match k {
        Kind::Feature => "feat",
        Kind::Refactor => "refac",
        other => other.id(),
    }
}

/// With a Nerd Font, signal bars as Linear marks priority: three in the critical colour, two in
/// the warning one, one in grey. Without, `!!!`, `!!` and `!`, as Reminders marks it.
pub(crate) fn priority_mark(p: Priority) -> (&'static str, Style) {
    priority_mark_in(p, theme::nerd())
}

fn priority_mark_in(p: Priority, nerd: bool) -> (&'static str, Style) {
    match (p, nerd) {
        (Priority::High, true) => ("\u{f08be}", theme::critical()),
        (Priority::Medium, true) => ("\u{f08bd}", theme::warning()),
        (Priority::Low, true) => ("\u{f08bc}", theme::muted()),
        (Priority::High, false) => ("!!!", theme::critical().add_modifier(Modifier::BOLD)),
        (Priority::Medium, false) => ("!!", theme::dim()),
        (Priority::Low, false) => ("!", theme::muted()),
    }
}

fn title_style(t: &Task) -> Style {
    match t.status {
        Status::Done => theme::muted().add_modifier(Modifier::CROSSED_OUT),
        Status::Ignored => theme::muted(),
        _ => theme::normal(),
    }
}

/// The row under the cursor: the tint desl's sidebar uses, when the terminal said its colours;
/// else bold.
fn lit() -> Style {
    if theme::has_surface() { theme::surface() } else { Style::default().add_modifier(Modifier::BOLD) }
}

// ---- state --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Filter {
    Open,
    Is(Status),
    All,
}

const FILTERS: [Filter; 7] = [
    Filter::Open,
    Filter::Is(Status::Todo),
    Filter::Is(Status::InProgress),
    Filter::Is(Status::Issue),
    Filter::Is(Status::Done),
    Filter::Is(Status::Ignored),
    Filter::All,
];

impl Filter {
    fn keeps(self, t: &Task) -> bool {
        match self {
            Filter::Open => t.status.open(),
            Filter::Is(s) => t.status == s,
            Filter::All => true,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Filter::Open => "Open",
            Filter::Is(s) => status_label(s),
            Filter::All => "All",
        }
    }
}

/// Where a view goes back to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Back {
    List,
    Detail(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pick {
    /// The type of a task about to be written.
    NewKind,
    Kind(u32),
    Status(u32),
    Priority(u32),
}

struct Form {
    /// None for a new task.
    id: Option<u32>,
    kind: Kind,
    title: Field,
    body: Field,
    on_body: bool,
    back: Back,
    /// What the fields held when the form opened: esc leaves at once when nothing changed.
    was: (String, String),
    discard: bool,
}

enum Mode {
    List,
    Detail { id: u32, scroll: usize },
    Pick { what: Pick, cursor: usize, back: Back },
    Form(Box<Form>),
    Confirm { id: u32, back: Back },
    Help(Back),
}

struct Notice {
    text: String,
    warn: bool,
}

/// Where things were drawn, for the mouse.
#[derive(Default)]
struct Hits {
    tabs_y: u16,
    tabs: Vec<(u16, u16, Filter)>,
    rows: Vec<(u16, u32)>,
    picks: Vec<(u16, usize)>,
    pick_box: Option<Rect>,
    chips_y: u16,
    chips: Vec<(u16, u16, Pick)>,
    title: Option<Rect>,
    body: Option<Rect>,
}

pub struct App {
    store: Store,
    list: List,
    stamp: Stamp,
    filter: Filter,
    query: String,
    searching: bool,
    sel: Option<u32>,
    /// Where the selection was among the rows shown: a task that leaves the view hands the
    /// cursor to the one that takes its place.
    at: usize,
    top: usize,
    mode: Mode,
    notice: Option<Notice>,
    fresh: HashMap<u32, Instant>,
    /// What `u` does: the opposite of the last change made here.
    undo: Option<(Change, String)>,
    quit: bool,
    hits: Hits,
    /// The width the form's description was last drawn at, for its up and down.
    body_width: usize,
    list_height: usize,
}

fn matches(t: &Task, query: &str) -> bool {
    let hay = format!("#{} {} {} {}", t.id, t.title, t.kind.id(), t.body).to_lowercase();
    query.to_lowercase().split_whitespace().all(|w| hay.contains(w))
}

impl App {
    pub fn new(store: Store) -> Result<App> {
        let list = store.read()?;
        let stamp = store.stamp();
        let mut app = App {
            store,
            list,
            stamp,
            filter: Filter::Open,
            query: String::new(),
            searching: false,
            sel: None,
            at: 0,
            top: 0,
            mode: Mode::List,
            notice: None,
            fresh: HashMap::new(),
            undo: None,
            quit: false,
            hits: Hits::default(),
            body_width: 40,
            list_height: 10,
        };
        app.settle();
        Ok(app)
    }

    fn visible(&self) -> Vec<&Task> {
        self.list.tasks.iter().filter(|t| self.filter.keeps(t) && matches(t, &self.query)).collect()
    }

    fn ids(&self) -> Vec<u32> {
        self.visible().iter().map(|t| t.id).collect()
    }

    /// The selection stays on its task while it is shown, else moves to the one in its place.
    fn settle(&mut self) {
        let ids = self.ids();
        if ids.is_empty() {
            self.sel = None;
            return;
        }
        match self.sel.and_then(|s| ids.iter().position(|&i| i == s)) {
            Some(i) => self.at = i,
            None => {
                self.at = self.at.min(ids.len() - 1);
                self.sel = Some(ids[self.at]);
            }
        }
    }

    fn move_by(&mut self, d: isize) {
        let ids = self.ids();
        if ids.is_empty() {
            return;
        }
        let i = self.sel.and_then(|s| ids.iter().position(|&x| x == s)).unwrap_or(0) as isize;
        let to = (i + d).clamp(0, ids.len() as isize - 1) as usize;
        self.sel = Some(ids[to]);
        self.at = to;
    }

    fn say(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warn: false });
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), warn: true });
    }

    fn file(&self) -> String {
        let p = self.store.path.to_string_lossy().into_owned();
        match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && p.starts_with(&home) => format!("~{}", &p[home.len()..]),
            _ => p,
        }
    }

    /// Reads the file again; with `theirs`, what changed lights up.
    fn reload(&mut self, theirs: bool) {
        self.stamp = self.store.stamp();
        match self.store.read() {
            Ok(list) => {
                if theirs {
                    let now = Instant::now();
                    for t in &list.tasks {
                        if self.list.get(t.id) != Some(t) {
                            self.fresh.insert(t.id, now);
                        }
                    }
                }
                self.list = list;
            }
            Err(e) => self.warn(format!("Could not read {} ({e}).", self.file())),
        }
        if let Mode::Detail { id, .. } = self.mode
            && self.list.get(id).is_none()
        {
            self.mode = Mode::List;
            self.warn(format!("#{id} was removed from the list."));
        }
        self.settle();
    }

    /// Called between events: the file changed by someone else is read again.
    pub fn tick(&mut self) {
        if self.store.stamp() != self.stamp {
            self.reload(true);
        }
        self.fresh.retain(|_, at| at.elapsed() < FRESH);
    }

    /// Makes one change to the file as it is now. `undo` is its opposite, for `u`.
    fn change(&mut self, change: Change, undo: Option<Change>) -> Option<u32> {
        let before = self.list.clone();
        match self.store.apply(&change) {
            Ok(Ok((list, id))) => {
                self.list = list;
                self.stamp = self.store.stamp();
                self.fresh.remove(&id);
                let undo = undo.or_else(|| opposite(&before, &change, id));
                self.undo = undo.map(|u| (u, describe(&change, id)));
                Some(id)
            }
            Ok(Err(Missing(id))) => {
                self.reload(true);
                self.warn(format!("#{id} is no longer in the list; someone removed it. This is the list as it is now."));
                None
            }
            Err(e) => {
                self.warn(format!("Could not write {} ({e}). Nothing changed.", self.file()));
                None
            }
        }
    }

    fn set(&mut self, id: u32, edit: Edit) -> bool {
        let Some(t) = self.list.get(id) else { return false };
        let words = said(t, &edit);
        if self.change(Change::Set { id, edit }, None).is_none() {
            return false;
        }
        self.settle();
        self.say(format!("{words} · u undoes"));
        true
    }

    fn toggle_done(&mut self, id: u32) {
        let Some(t) = self.list.get(id) else { return };
        let to = if t.status == Status::Done { Status::Todo } else { Status::Done };
        self.set(id, Edit { status: Some(to), ..Edit::default() });
    }

    fn reorder(&mut self, up: bool) {
        let ids = self.ids();
        let Some(i) = self.sel.and_then(|s| ids.iter().position(|&x| x == s)) else { return };
        let j = if up { i.checked_sub(1) } else { (i + 1 < ids.len()).then_some(i + 1) };
        let Some(j) = j else { return };
        let (id, past) = (ids[i], ids[j]);
        if self.change(Change::Move { id, past }, Some(Change::Move { id: past, past: id })).is_some() {
            self.at = j;
            self.notice = None;
        }
    }

    fn undo(&mut self) {
        let Some((change, what)) = self.undo.take() else {
            return self.say("Nothing to undo.");
        };
        if let Some(id) = self.change(change, None) {
            self.undo = None;
            if self.list.get(id).is_some_and(|t| !self.filter.keeps(t)) {
                self.filter = Filter::All;
            }
            self.sel = self.list.get(id).map(|t| t.id).or(self.sel);
            self.settle();
            self.say(format!("Undone: {what}."));
        }
    }

    fn open_form(&mut self, id: Option<u32>, kind: Kind, back: Back) {
        let (title, body) = id.and_then(|id| self.list.get(id)).map(|t| (t.title.clone(), t.body.clone())).unwrap_or_default();
        self.mode = Mode::Form(Box::new(Form {
            id,
            kind,
            title: Field::new(&title, false),
            body: Field::new(&body, true),
            on_body: false,
            back,
            was: (title, body),
            discard: false,
        }));
    }

    fn save(&mut self, form: Box<Form>) {
        let title = form.title.text().trim().to_string();
        if title.is_empty() {
            self.warn("A task needs a title.");
            self.mode = Mode::Form(form);
            return;
        }
        let body = form.body.text();
        match form.id {
            None => {
                let change = Change::Add { title, kind: form.kind, status: Status::Todo, priority: Priority::Medium, body };
                if let Some(id) = self.change(change, None) {
                    if !self.filter.keeps(self.list.get(id).expect("just added")) {
                        self.filter = Filter::Open;
                    }
                    if !self.list.get(id).is_some_and(|t| matches(t, &self.query)) {
                        self.query.clear();
                    }
                    self.sel = Some(id);
                    self.settle();
                    self.say(format!("Added #{id}, todo · p sets its priority · u undoes"));
                }
                self.mode = Mode::List;
            }
            Some(id) => {
                let edit = Edit { title: Some(title), body: Some(body), ..Edit::default() };
                self.set(id, edit);
                self.mode = match form.back {
                    Back::Detail(id) if self.list.get(id).is_some() => Mode::Detail { id, scroll: 0 },
                    _ => Mode::List,
                };
            }
        }
    }

    // ---- keys -----------------------------------------------------------------------------------

    pub fn key(&mut self, k: KeyEvent) {
        if !matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }
        let keep_notice = matches!(self.mode, Mode::Form(_));
        if !keep_notice {
            self.notice = None;
        }
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::List => self.list_key(k),
            Mode::Detail { id, scroll } => {
                self.mode = Mode::Detail { id, scroll };
                self.detail_key(k, id, scroll);
            }
            Mode::Pick { what, cursor, back } => self.pick_key(k, what, cursor, back),
            Mode::Form(form) => self.form_key(k, form),
            Mode::Confirm { id, back } => {
                if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    let title = self.list.get(id).map(|t| t.title.clone()).unwrap_or_default();
                    if self.change(Change::Remove(id), None).is_some() {
                        self.settle();
                        self.say(format!("Removed #{id} “{}” · u brings it back", truncate_end(&title, 32)));
                    }
                    self.mode = Mode::List;
                } else {
                    self.mode = match back {
                        Back::Detail(id) => Mode::Detail { id, scroll: 0 },
                        Back::List => Mode::List,
                    };
                    self.say("Kept it.");
                }
            }
            Mode::Help(back) => {
                self.mode = match back {
                    Back::Detail(id) if self.list.get(id).is_some() => Mode::Detail { id, scroll: 0 },
                    _ => Mode::List,
                }
            }
        }
    }

    fn list_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.list_height.max(2) as isize - 1;
        if self.searching {
            match k.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.query.clear();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace if self.query.is_empty() => self.searching = false,
                KeyCode::Backspace => {
                    self.query.pop();
                }
                KeyCode::Char('u') if ctrl => self.query.clear(),
                KeyCode::Down => self.move_by(1),
                KeyCode::Up => self.move_by(-1),
                KeyCode::Char(c) if !ctrl => self.query.push(c),
                _ => {}
            }
            self.at = 0;
            self.sel = None;
            return self.settle();
        }
        let sel = self.sel;
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Up if shift => self.reorder(true),
            KeyCode::Down if shift => self.reorder(false),
            KeyCode::Char('K') => self.reorder(true),
            KeyCode::Char('J') => self.reorder(false),
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('d') if ctrl => self.move_by(page / 2),
            KeyCode::Char('u') if ctrl => self.move_by(-page / 2),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.move_by(-isize::MAX / 2),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2),
            KeyCode::Tab => self.next_filter(1),
            KeyCode::BackTab => self.next_filter(-1),
            KeyCode::Char(c @ '1'..='7') => self.set_filter(FILTERS[c as usize - '1' as usize]),
            KeyCode::Char('/') => {
                self.searching = true;
            }
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.settle();
            }
            KeyCode::Char('n') | KeyCode::Char('a') => {
                self.mode = Mode::Pick { what: Pick::NewKind, cursor: 0, back: Back::List };
            }
            KeyCode::Char('?') => self.mode = Mode::Help(Back::List),
            KeyCode::Char('r') => {
                self.reload(false);
                self.say(format!("Read {} again.", self.file()));
            }
            KeyCode::Char('u') => self.undo(),
            _ => {
                let Some(id) = sel else {
                    if matches!(k.code, KeyCode::Enter | KeyCode::Char('e' | 's' | 'p' | 't' | 'x' | 'd' | ' ')) {
                        self.say("No task here. n adds one.");
                    }
                    return;
                };
                self.task_key(k, id, Back::List);
            }
        }
    }

    /// The keys that act on one task, the same in the list and in a task's own view.
    fn task_key(&mut self, k: KeyEvent, id: u32, back: Back) {
        let Some(t) = self.list.get(id) else { return };
        let (status, priority, kind) = (t.status, t.priority, t.kind);
        match k.code {
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if back == Back::List => {
                self.mode = Mode::Detail { id, scroll: 0 };
            }
            KeyCode::Char('e') => self.open_form(Some(id), kind, back),
            KeyCode::Char('s') => {
                let cursor = Status::ALL.iter().position(|s| *s == status).unwrap_or(0);
                self.mode = Mode::Pick { what: Pick::Status(id), cursor, back };
            }
            KeyCode::Char('p') => {
                let cursor = Priority::ALL.iter().position(|p| *p == priority).unwrap_or(1);
                self.mode = Mode::Pick { what: Pick::Priority(id), cursor, back };
            }
            KeyCode::Char('t') => {
                let cursor = Kind::ALL.iter().position(|x| *x == kind).unwrap_or(0);
                self.mode = Mode::Pick { what: Pick::Kind(id), cursor, back };
            }
            KeyCode::Char('x') | KeyCode::Char(' ') => self.toggle_done(id),
            KeyCode::Char('d') | KeyCode::Delete => self.mode = Mode::Confirm { id, back },
            _ => {}
        }
    }

    fn detail_key(&mut self, k: KeyEvent, id: u32, scroll: usize) {
        let page = self.list_height.max(2) - 1;
        let to = |s: usize| Mode::Detail { id, scroll: s };
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.mode = Mode::List,
            KeyCode::Char('j') | KeyCode::Down => self.mode = to(scroll + 1),
            KeyCode::Char('k') | KeyCode::Up => self.mode = to(scroll.saturating_sub(1)),
            KeyCode::PageDown => self.mode = to(scroll + page),
            KeyCode::PageUp => self.mode = to(scroll.saturating_sub(page)),
            KeyCode::Char('g') | KeyCode::Home => self.mode = to(0),
            KeyCode::Char('G') | KeyCode::End => self.mode = to(usize::MAX / 2),
            KeyCode::Char('n') => self.mode = Mode::Pick { what: Pick::NewKind, cursor: 0, back: Back::List },
            KeyCode::Char('u') => self.undo(),
            KeyCode::Char('?') => self.mode = Mode::Help(Back::Detail(id)),
            KeyCode::Char(' ') => {}
            _ => self.task_key(k, id, Back::Detail(id)),
        }
    }

    fn picks(&self, what: Pick) -> Vec<(&'static str, Style, &'static str, bool)> {
        let t = match what {
            Pick::NewKind => None,
            Pick::Kind(id) | Pick::Status(id) | Pick::Priority(id) => self.list.get(id),
        };
        match what {
            Pick::NewKind | Pick::Kind(_) => Kind::ALL
                .iter()
                .map(|&k| (if theme::nerd() { kind_icon(k) } else { "•" }, kind_style(k), k.id(), t.is_some_and(|t| t.kind == k)))
                .collect(),
            Pick::Status(_) => {
                Status::ALL.iter().map(|&s| (status_glyph(s), status_style(s), s.word(), t.is_some_and(|t| t.status == s))).collect()
            }
            Pick::Priority(_) => Priority::ALL
                .iter()
                .map(|&p| {
                    let (mark, style) = priority_mark(p);
                    (mark, style, p.id(), t.is_some_and(|t| t.priority == p))
                })
                .collect(),
        }
    }

    fn back_to(&self, back: Back) -> Mode {
        match back {
            Back::Detail(id) if self.list.get(id).is_some() => Mode::Detail { id, scroll: 0 },
            _ => Mode::List,
        }
    }

    fn pick_key(&mut self, k: KeyEvent, what: Pick, cursor: usize, back: Back) {
        let n = self.picks(what).len();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = self.back_to(back),
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => self.mode = Mode::Pick { what, cursor: (cursor + 1).min(n - 1), back },
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => self.mode = Mode::Pick { what, cursor: cursor.saturating_sub(1), back },
            KeyCode::Enter | KeyCode::Char(' ') => self.choose(what, cursor, back),
            KeyCode::Char(c @ '1'..='9') if (c as usize - '1' as usize) < n => self.choose(what, c as usize - '1' as usize, back),
            _ => self.mode = Mode::Pick { what, cursor, back },
        }
    }

    fn choose(&mut self, what: Pick, i: usize, back: Back) {
        self.mode = self.back_to(back);
        match what {
            Pick::NewKind => self.open_form(None, Kind::ALL[i], Back::List),
            Pick::Kind(id) => {
                self.set(id, Edit { kind: Some(Kind::ALL[i]), ..Edit::default() });
            }
            Pick::Status(id) => {
                self.set(id, Edit { status: Some(Status::ALL[i]), ..Edit::default() });
            }
            Pick::Priority(id) => {
                self.set(id, Edit { priority: Some(Priority::ALL[i]), ..Edit::default() });
            }
        }
        if let Mode::Detail { id, .. } = self.mode
            && self.list.get(id).is_none()
        {
            self.mode = Mode::List;
        }
    }

    fn form_key(&mut self, k: KeyEvent, mut form: Box<Form>) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let width = self.body_width;
        if !matches!(k.code, KeyCode::Esc) {
            form.discard = false;
        }
        let on_body = form.on_body;
        let field = if on_body { &mut form.body } else { &mut form.title };
        match k.code {
            KeyCode::Esc => {
                let changed = (form.title.text(), form.body.text()) != form.was;
                if !changed || form.discard {
                    self.notice = None;
                    self.mode = self.back_to(form.back);
                    return;
                }
                form.discard = true;
                self.warn("esc again throws away what you typed · ctrl+s saves it");
            }
            KeyCode::Char('s') if ctrl => return self.save(form),
            KeyCode::Tab | KeyCode::BackTab => form.on_body = !form.on_body,
            KeyCode::Enter if !on_body => form.on_body = true,
            KeyCode::Enter => field.newline(),
            KeyCode::Down if !on_body => form.on_body = true,
            KeyCode::Up if on_body => {
                if !field.up(width) {
                    form.on_body = false;
                }
            }
            KeyCode::Down => {
                field.down(width);
            }
            KeyCode::Up => {}
            KeyCode::Left if ctrl || alt => field.word_left(),
            KeyCode::Right if ctrl || alt => field.word_right(),
            KeyCode::Left => field.left(),
            KeyCode::Right => field.right(),
            KeyCode::Home => field.home(),
            KeyCode::End => field.end(),
            KeyCode::Backspace if ctrl || alt => field.kill_word(),
            KeyCode::Backspace => field.backspace(),
            KeyCode::Delete => field.delete(),
            KeyCode::Char('a') if ctrl => field.home(),
            KeyCode::Char('e') if ctrl => field.end(),
            KeyCode::Char('u') if ctrl => field.kill_back(),
            KeyCode::Char('k') if ctrl => field.kill_forward(),
            KeyCode::Char('w') if ctrl => field.kill_word(),
            KeyCode::Char('h') if ctrl => field.backspace(),
            KeyCode::Char('b') if alt => field.word_left(),
            KeyCode::Char('f') if alt => field.word_right(),
            KeyCode::Char(c) if !ctrl && !alt => {
                field.insert(c);
                if self.notice.as_ref().is_some_and(|n| n.warn) {
                    self.notice = None;
                }
            }
            _ => {}
        }
        self.mode = Mode::Form(form);
    }

    pub fn paste(&mut self, text: &str) {
        match &mut self.mode {
            Mode::Form(form) => {
                if form.on_body {
                    form.body.paste(text);
                } else {
                    form.title.paste(text);
                }
            }
            Mode::List if self.searching => {
                self.query.push_str(&text.replace(['\n', '\r'], " "));
                self.settle();
            }
            _ => {}
        }
    }

    fn next_filter(&mut self, d: isize) {
        let i = FILTERS.iter().position(|f| *f == self.filter).unwrap_or(0) as isize;
        let n = FILTERS.len() as isize;
        self.set_filter(FILTERS[((i + d).rem_euclid(n)) as usize]);
    }

    fn set_filter(&mut self, f: Filter) {
        self.filter = f;
        self.top = 0;
        self.settle();
    }

    // ---- mouse ----------------------------------------------------------------------------------

    pub fn mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown { 1 } else { -1 };
                let n = if let Mode::Pick { what, .. } = &self.mode { self.picks(*what).len() } else { 0 };
                match &mut self.mode {
                    Mode::List => self.move_by(d),
                    Mode::Detail { scroll, .. } => *scroll = (*scroll as isize + 3 * d).max(0) as usize,
                    Mode::Pick { cursor, .. } => {
                        *cursor = (*cursor as isize + d).clamp(0, n as isize - 1) as usize;
                    }
                    _ => {}
                }
            }
            MouseEventKind::Down(MouseButton::Left) => self.click(x, y),
            _ => {}
        }
    }

    fn click(&mut self, x: u16, y: u16) {
        self.notice = None;
        match std::mem::replace(&mut self.mode, Mode::List) {
            Mode::Pick { what, cursor, back } => {
                if let Some(&(_, i)) = self.hits.picks.iter().find(|(row, _)| *row == y) {
                    return self.choose(what, i, back);
                }
                let inside = self.hits.pick_box.is_some_and(|r| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height);
                self.mode = if inside { Mode::Pick { what, cursor, back } } else { self.back_to(back) };
            }
            Mode::List => {
                if y == self.hits.tabs_y
                    && let Some(&(_, _, f)) = self.hits.tabs.iter().find(|(a, b, _)| x >= *a && x < *b)
                {
                    return self.set_filter(f);
                }
                if let Some(&(_, id)) = self.hits.rows.iter().find(|(row, _)| *row == y) {
                    if self.sel == Some(id) {
                        self.mode = Mode::Detail { id, scroll: 0 };
                    } else {
                        self.sel = Some(id);
                        self.settle();
                    }
                }
            }
            Mode::Detail { id, scroll } => {
                self.mode = Mode::Detail { id, scroll };
                if y == self.hits.chips_y
                    && let Some(&(_, _, pick)) = self.hits.chips.iter().find(|(a, b, _)| x >= *a && x < *b)
                {
                    let t = self.list.get(id);
                    let cursor = match pick {
                        Pick::Status(_) => t.and_then(|t| Status::ALL.iter().position(|s| *s == t.status)),
                        Pick::Priority(_) => t.and_then(|t| Priority::ALL.iter().position(|p| *p == t.priority)),
                        _ => t.and_then(|t| Kind::ALL.iter().position(|k| *k == t.kind)),
                    };
                    self.mode = Mode::Pick { what: pick, cursor: cursor.unwrap_or(0), back: Back::Detail(id) };
                }
            }
            Mode::Form(mut form) => {
                let within = |r: Option<Rect>| r.is_some_and(|r| y >= r.y && y < r.y + r.height);
                if within(self.hits.title) {
                    form.on_body = false;
                } else if within(self.hits.body) {
                    form.on_body = true;
                }
                self.mode = Mode::Form(form);
            }
            other => self.mode = other,
        }
    }

    // ---- drawing --------------------------------------------------------------------------------

    pub fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        self.hits = Hits::default();
        if area.width < 20 || area.height < 4 {
            f.render_widget(Paragraph::new("Too small for the task list.").style(theme::muted()), area);
            return;
        }
        let footer = Rect { y: area.y + area.height - 1, height: 1, ..area };
        let body = Rect { height: area.height - 1, ..area };
        match &self.mode {
            Mode::Detail { id, scroll } => {
                let (id, scroll) = (*id, *scroll);
                let shown = self.draw_detail(f, body, id, scroll);
                self.mode = Mode::Detail { id, scroll: shown };
            }
            Mode::Pick { back: Back::Detail(id), .. } | Mode::Confirm { back: Back::Detail(id), .. } | Mode::Help(Back::Detail(id)) => {
                let id = *id;
                self.draw_detail(f, body, id, 0);
            }
            _ => self.draw_list(f, body),
        }
        match &self.mode {
            Mode::Pick { what, cursor, .. } => {
                let (what, cursor) = (*what, *cursor);
                self.draw_pick(f, body, what, cursor);
            }
            Mode::Help(_) => self.draw_help(f, body),
            _ => {}
        }
        if let Mode::Form(_) = &self.mode {
            self.draw_form(f, area);
            return;
        }
        let line = self.footer_line(area.width as usize);
        f.render_widget(Paragraph::new(line), footer);
    }

    fn counts(&self) -> HashMap<Status, usize> {
        let mut c = HashMap::new();
        for t in &self.list.tasks {
            *c.entry(t.status).or_insert(0) += 1;
        }
        c
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect) {
        let w = area.width as usize;
        let counts = self.counts();
        let count = |flt: Filter| match flt {
            Filter::Open => self.list.tasks.iter().filter(|t| t.status.open()).count(),
            Filter::Is(s) => counts.get(&s).copied().unwrap_or(0),
            Filter::All => self.list.tasks.len(),
        };
        // The tabs: whole words when they fit, else marks and counts.
        let tab = |flt: Filter, short: bool| -> Vec<(String, Style)> {
            let n = count(flt);
            let on = flt == self.filter;
            let quiet = if on { theme::pill() } else { theme::muted() };
            let word = if on { theme::pill().add_modifier(Modifier::BOLD) } else { theme::normal() };
            match (flt, short) {
                (Filter::Is(s), true) => {
                    // A Nerd Font's icon spills into the cell after it: that cell stays blank.
                    let n = if theme::nerd() { format!(" {n} ") } else { format!("{n} ") };
                    vec![(" ".into(), quiet), (status_glyph(s).into(), if on { word } else { status_style(s) }), (n, quiet)]
                }
                (Filter::Is(s), false) => vec![
                    (" ".into(), quiet),
                    (status_glyph(s).into(), if on { word } else { status_style(s) }),
                    (format!(" {}", flt.label()), word),
                    (format!(" {n} "), quiet),
                ],
                _ => vec![(format!(" {}", flt.label()), word), (format!(" {n} "), quiet)],
            }
        };
        // Each tab carries a space either side, which is all that parts them.
        let wide: usize = FILTERS.iter().map(|f| tab(*f, false).iter().map(|(s, _)| s.width()).sum::<usize>()).sum();
        let short = wide + 1 > w;
        let mut spans = vec![Span::raw(" ")];
        let mut x = area.x + 1;
        for flt in FILTERS {
            let parts = tab(flt, short);
            let width: usize = parts.iter().map(|(s, _)| s.width()).sum();
            self.hits.tabs.push((x, x + width as u16, flt));
            x += width as u16;
            spans.extend(parts.into_iter().map(|(s, st)| Span::styled(s, st)));
        }
        self.hits.tabs_y = area.y;
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { height: 1, ..area });

        // Under the tabs: a rule, or what is being searched for.
        let rule_area = Rect { y: area.y + 1, height: 1, ..area };
        let shown = self.visible().len();
        let rule = if self.searching || !self.query.is_empty() {
            let mut s = vec![
                Span::styled(" / ", theme::accent()),
                Span::styled(self.query.clone(), theme::bold()),
                Span::styled(format!("   {shown} of {}", self.list.tasks.len()), theme::muted()),
            ];
            if !self.searching {
                s.push(Span::styled("   esc clears", theme::muted()));
            }
            Line::from(s)
        } else {
            Line::from(Span::styled("─".repeat(w), theme::muted()))
        };
        f.render_widget(Paragraph::new(rule), rule_area);
        if self.searching {
            f.set_cursor_position((rule_area.x + 3 + self.query.width() as u16, rule_area.y));
        }

        let rows_area = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
        let h = rows_area.height as usize;
        self.list_height = h;
        let visible: Vec<Task> = self.visible().into_iter().cloned().collect();
        if visible.is_empty() {
            let (head, hint) = if self.list.tasks.is_empty() {
                ("No tasks yet.".to_string(), "n adds one. Agents add theirs with `desl task add`.".to_string())
            } else if !self.query.is_empty() {
                (format!("Nothing matches “{}”.", self.query), "esc clears the search.".to_string())
            } else {
                let what = match self.filter {
                    Filter::Open => "Nothing open: every task is done or ignored.".to_string(),
                    other => format!("No task is {}.", other.label().to_lowercase()),
                };
                (what, "tab shows the other tasks.".to_string())
            };
            let top = rows_area.y + (rows_area.height / 3).min(4);
            let lines = vec![Line::from(Span::styled(head, theme::bold())), Line::from(Span::styled(hint, theme::muted()))];
            f.render_widget(
                Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
                Rect { y: top, height: 2.min(rows_area.height), ..rows_area },
            );
            return;
        }
        let at = self.sel.and_then(|s| visible.iter().position(|t| t.id == s)).unwrap_or(0);
        if at < self.top {
            self.top = at;
        } else if at >= self.top + h {
            self.top = at + 1 - h;
        }
        self.top = self.top.min(visible.len().saturating_sub(h));
        let id_w = visible.iter().map(|t| t.id.to_string().len()).max().unwrap_or(1) + 1;
        let nerd = theme::nerd();
        let show_kind = w >= 44;
        let mut lines = Vec::new();
        for (row, t) in visible.iter().enumerate().skip(self.top).take(h) {
            let on = Some(t.id) == self.sel;
            let base = if on { lit() } else { Style::default() };
            let fresh = self.fresh.contains_key(&t.id);
            let mut spans = vec![
                Span::styled(if on { "▌" } else { " " }, base.patch(theme::accent())),
                Span::styled(" ", base),
                Span::styled(status_glyph(t.status), base.patch(status_style(t.status))),
                Span::styled(" ", base),
                Span::styled(format!("{:>id_w$}", format!("#{}", t.id)), if fresh { theme::pill() } else { base.patch(theme::muted()) }),
                Span::styled("  ", base),
            ];
            if show_kind {
                // In a narrow pane a Nerd Font's icon says it alone.
                let mark = match (nerd, w >= 64) {
                    (true, true) => format!("{} {:<5}", kind_icon(t.kind), kind_short(t.kind)),
                    (true, false) => kind_icon(t.kind).to_string(),
                    (false, _) => format!("{:<5}", kind_short(t.kind)),
                };
                spans.push(Span::styled(mark, base.patch(kind_style(t.kind))));
                spans.push(Span::styled("  ", base));
            }
            let (mark, mark_style) = priority_mark(t.priority);
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            let note = if t.body.trim().is_empty() { String::new() } else { format!(" {}", note_mark(nerd)) };
            let room = w.saturating_sub(used + 3 + 2 + note.width());
            let title = truncate_end(&t.title, room);
            let title_w = title.width();
            let mut title_style = base.patch(title_style(t));
            if on {
                title_style = title_style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(title, title_style));
            let note_w = note.width();
            spans.push(Span::styled(note, base.patch(theme::muted())));
            let pad = w.saturating_sub(used + title_w + note_w + 4);
            spans.push(Span::styled(" ".repeat(pad), base));
            spans.push(Span::styled(format!("{mark:>3}"), base.patch(mark_style)));
            spans.push(Span::styled(" ", base));
            self.hits.rows.push((rows_area.y + (row - self.top) as u16, t.id));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), rows_area);
    }

    /// A task on its own. Returns the scroll it was drawn at, held inside what there is.
    fn draw_detail(&mut self, f: &mut Frame, area: Rect, id: u32, scroll: usize) -> usize {
        let Some(t) = self.list.get(id).cloned() else { return 0 };
        let w = area.width as usize;
        let inner = w.saturating_sub(4).max(10);
        let nerd = theme::nerd();
        // The fields, each a chip that opens its list when clicked.
        let (pmark, pstyle) = priority_mark(t.priority);
        let kind_mark = if nerd { format!("{} {}", kind_icon(t.kind), t.kind.id()) } else { t.kind.id().to_string() };
        let chips: Vec<(Vec<Span>, Pick)> = vec![
            (
                vec![
                    Span::styled(status_glyph(t.status), status_style(t.status)),
                    Span::styled(format!(" {}", t.status.word()), status_style(t.status)),
                ],
                Pick::Status(id),
            ),
            (vec![Span::styled(kind_mark, kind_style(t.kind))], Pick::Kind(id)),
            (vec![Span::styled(pmark, pstyle), Span::styled(format!(" {}", t.priority.id()), theme::dim())], Pick::Priority(id)),
        ];
        let mut spans = vec![Span::raw("  ")];
        let mut x = area.x + 2;
        for (parts, pick) in chips {
            let width: usize = parts.iter().map(|s| s.content.width()).sum();
            self.hits.chips.push((x, x + width as u16, pick));
            spans.extend(parts);
            spans.push(Span::styled("   ", theme::muted()));
            x += width as u16 + 3;
        }
        let used: usize = spans.iter().map(|s| s.content.width()).sum();
        let tag = format!("#{}", t.id);
        spans.push(Span::raw(" ".repeat(w.saturating_sub(used + tag.width() + 2))));
        spans.push(Span::styled(tag, theme::muted()));
        self.hits.chips_y = area.y;
        let mut lines = vec![Line::from(spans), Line::default()];
        for row in crate::ui::text::wrap(&t.title, inner) {
            lines.push(Line::from(vec![Span::raw("  "), Span::styled(row, title_style(&t).add_modifier(Modifier::BOLD))]));
        }
        lines.push(Line::from(Span::styled(format!("  {}", "─".repeat(inner)), theme::muted())));
        let head = lines.len();
        if t.body.trim().is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("  No description. e adds one.", theme::muted())));
        } else {
            lines.push(Line::default());
            for row in crate::ui::markdown::render(&t.body, inner) {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(row);
                lines.push(Line::from(spans));
            }
        }
        // The fields and the title stay; the description scrolls under them.
        let fixed = head.min(area.height as usize);
        let room = (area.height as usize).saturating_sub(fixed);
        let body: Vec<Line> = lines.split_off(fixed);
        let scroll = scroll.min(body.len().saturating_sub(room));
        f.render_widget(Paragraph::new(lines), Rect { height: fixed as u16, ..area });
        let body_area = Rect { y: area.y + fixed as u16, height: room as u16, ..area };
        f.render_widget(Paragraph::new(body.into_iter().skip(scroll).take(room).collect::<Vec<_>>()), body_area);
        self.list_height = room;
        scroll
    }

    fn draw_pick(&mut self, f: &mut Frame, area: Rect, what: Pick, cursor: usize) {
        let options = self.picks(what);
        let title = match what {
            Pick::NewKind => " New task: what kind? ".to_string(),
            Pick::Kind(id) => format!(" Type of #{id} "),
            Pick::Status(id) => format!(" Status of #{id} "),
            Pick::Priority(id) => format!(" Priority of #{id} "),
        };
        let label_w = options.iter().map(|o| o.2.width()).max().unwrap_or(4);
        let mark_w = options.iter().map(|o| o.0.width()).max().unwrap_or(1);
        let inner_w = (label_w + 12).max(title.width() + 2);
        let width = (inner_w as u16 + 2).min(area.width);
        let height = (options.len() as u16 + 2).min(area.height);
        let r = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height.saturating_sub(height)) / 3, width, height };
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::accent())
            .title(Span::styled(title, theme::bold()))
            .title_bottom(Line::from(Span::styled(" enter · 1-9 · esc ", theme::muted())).right_aligned());
        let inner = block.inner(r);
        f.render_widget(block, r);
        let mut lines = Vec::new();
        for (i, (mark, style, label, current)) in options.iter().enumerate() {
            let on = i == cursor;
            let base = if on { lit() } else { Style::default() };
            let mut spans = vec![
                Span::styled(if on { "▌" } else { " " }, base.patch(theme::accent())),
                Span::styled(format!("{} ", i + 1), base.patch(theme::muted())),
                Span::styled(format!("{mark:<mark_w$}"), base.patch(*style)),
                Span::styled(format!(" {label:<label_w$}"), if on { base.add_modifier(Modifier::BOLD) } else { base }),
            ];
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            let tail = if *current { " ✓" } else { "" };
            spans.push(Span::styled(" ".repeat((inner.width as usize).saturating_sub(used + tail.width() + 1)), base));
            spans.push(Span::styled(tail, base.patch(theme::accent())));
            spans.push(Span::styled(" ", base));
            self.hits.picks.push((inner.y + i as u16, i));
            lines.push(Line::from(spans));
        }
        self.hits.pick_box = Some(r);
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_form(&mut self, f: &mut Frame, area: Rect) {
        let Mode::Form(form) = &mut self.mode else { return };
        let width = area.width.min(100);
        let r = Rect { x: area.x + (area.width - width) / 2, width, ..area };
        f.render_widget(Clear, r);
        let what = match form.id {
            None => format!(" New {} ", form.kind.id()),
            Some(id) => format!(" Edit #{id} "),
        };
        let mark = if theme::nerd() { format!(" {}", kind_icon(form.kind)) } else { String::new() };
        let hint = if form.on_body {
            " ctrl+s save · enter new line · tab title · esc cancel "
        } else {
            " enter description · ctrl+s save · esc cancel "
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::accent())
            .title(Line::from(vec![Span::styled(mark, kind_style(form.kind)), Span::styled(what, theme::bold())]))
            .title_bottom(Line::from(Span::styled(hint, theme::muted())).right_aligned());
        let inner = block.inner(r);
        f.render_widget(block, r);
        let fw = inner.width.saturating_sub(2) as usize;
        let label = |text: &str, on: bool| {
            Line::from(vec![
                Span::raw(" "),
                Span::styled(text.to_string(), if on { theme::accent().add_modifier(Modifier::BOLD) } else { theme::muted() }),
            ])
        };
        let x = inner.x + 1;
        // The title: up to three rows.
        let (title_rows, title_cur) = form.title.layout(fw);
        let title_h = title_rows.len().clamp(1, 3);
        form.title.follow(fw, title_h);
        let mut y = inner.y;
        f.render_widget(Paragraph::new(label("Title", !form.on_body)), Rect { x: inner.x, y, width: inner.width, height: 1 });
        y += 1;
        let title_area = Rect { x, y, width: fw as u16, height: title_h as u16 };
        let rows: Vec<Line> =
            title_rows.iter().skip(form.title.scroll).take(title_h).map(|s| Line::from(Span::styled(s.clone(), theme::bold()))).collect();
        f.render_widget(Paragraph::new(rows), title_area);
        self.hits.title = Some(Rect { y: y - 1, height: title_h as u16 + 1, ..title_area });
        y += title_h as u16 + 1;
        if y >= inner.y + inner.height {
            return;
        }
        f.render_widget(
            Paragraph::new(label("Description · Markdown, optional", form.on_body)),
            Rect { x: inner.x, y, width: inner.width, height: 1 },
        );
        y += 1;
        let body_h = (inner.y + inner.height).saturating_sub(y) as usize;
        self.body_width = fw;
        form.body.follow(fw, body_h);
        let (body_rows, body_cur) = form.body.layout(fw);
        let body_area = Rect { x, y, width: fw as u16, height: body_h as u16 };
        let rows: Vec<Line> = body_rows.iter().skip(form.body.scroll).take(body_h).map(|s| Line::from(s.clone())).collect();
        if form.body.is_blank() && !form.on_body {
            f.render_widget(Paragraph::new(Span::styled("What it is about, what done means. enter gets here.", theme::muted())), body_area);
        } else {
            f.render_widget(Paragraph::new(rows), body_area);
        }
        self.hits.body = Some(Rect { y: y - 1, height: body_h as u16 + 1, ..body_area });
        let (cur, scroll, top) =
            if form.on_body { (body_cur, form.body.scroll, body_area.y) } else { (title_cur, form.title.scroll, title_area.y) };
        if cur.0 >= scroll {
            f.set_cursor_position((x + cur.1 as u16, top + (cur.0 - scroll) as u16));
        }
        if let Some(n) = &self.notice {
            let text = format!(" {} ", n.text);
            let width = (text.width() as u16).min(r.width.saturating_sub(4));
            f.render_widget(
                Paragraph::new(Span::styled(text, if n.warn { theme::warning() } else { theme::dim() })),
                Rect { x: r.x + 2, y: r.y, width, height: 1 },
            );
        }
    }

    fn draw_help(&mut self, f: &mut Frame, area: Rect) {
        let rows: &[(&str, &str, &str)] = &[
            ("List", "j k ↑ ↓", "move"),
            ("", "enter, click", "open a task"),
            ("", "tab 1…7", "show by status"),
            ("", "/", "search"),
            ("", "K J shift+↑↓", "move a task up, down"),
            ("Task", "n", "new: type, then title"),
            ("", "e", "edit title, description"),
            ("", "s p t", "status, priority, type"),
            ("", "x space", "done, or todo again"),
            ("", "d", "delete"),
            ("", "u", "undo the last change"),
            ("", "q", "close the task list"),
        ];
        let key_w = rows.iter().map(|r| r.1.width()).max().unwrap_or(0);
        let width = (rows.iter().map(|r| r.2.width()).max().unwrap_or(0) + key_w + 7).min(area.width as usize) as u16;
        let height = (rows.len() as u16 + 5).min(area.height);
        let r = Rect { x: area.x + (area.width - width) / 2, y: area.y + (area.height - height) / 3, width, height };
        f.render_widget(Clear, r);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(theme::accent())
            .title(Span::styled(" Keys ", theme::bold()))
            .title_bottom(Line::from(Span::styled(" any key closes ", theme::muted())).right_aligned());
        let inner = block.inner(r);
        f.render_widget(block, r);
        let mut lines = Vec::new();
        for (group, key, what) in rows {
            if !group.is_empty() {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::from(Span::styled(format!(" {group}"), theme::muted())));
            }
            lines.push(Line::from(vec![
                Span::styled(format!(" {key:<key_w$}"), theme::accent()),
                Span::styled(format!("  {what}"), theme::normal()),
            ]));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn footer_line(&self, w: usize) -> Line<'static> {
        if let Mode::Confirm { id, .. } = self.mode {
            let title = self.list.get(id).map(|t| t.title.clone()).unwrap_or_default();
            let tail = "? y deletes it, any other key keeps it.";
            let room = w.saturating_sub(format!(" Delete #{id} “”{tail}").width());
            let text = if room >= 12 {
                format!(" Delete #{id} “{}”{tail}", truncate_end(&title, room))
            } else {
                format!(" Delete #{id}{tail}")
            };
            return Line::from(Span::styled(text, theme::warning()));
        }
        if let Some(n) = &self.notice {
            return Line::from(Span::styled(
                format!(" {}", truncate_end(&n.text, w.saturating_sub(2))),
                if n.warn { theme::warning() } else { theme::dim() },
            ));
        }
        let keys: &[(&str, &str)] = match self.mode {
            Mode::Detail { .. } => &[
                ("esc", "back"),
                ("e", "edit"),
                ("s", "status"),
                ("p", "priority"),
                ("t", "type"),
                ("x", "done"),
                ("d", "delete"),
                ("?", "keys"),
            ],
            _ if self.searching => &[("enter", "keep"), ("esc", "clear"), ("↑↓", "move")],
            _ => &[
                ("n", "new"),
                ("enter", "open"),
                ("s", "status"),
                ("x", "done"),
                ("p", "priority"),
                ("/", "search"),
                ("tab", "filter"),
                ("?", "keys"),
            ],
        };
        let mut spans = vec![Span::raw(" ")];
        let mut used = 1;
        let last = keys.len() - 1;
        for (i, (k, what)) in keys.iter().enumerate() {
            let need = k.width() + what.width() + 4;
            // `?` always fits: it shows the rest.
            if used + need + if i < last { 8 } else { 0 } > w && i < last {
                continue;
            }
            spans.push(Span::styled(k.to_string(), theme::accent()));
            spans.push(Span::styled(format!(" {what}   "), theme::muted()));
            used += need;
        }
        Line::from(spans)
    }
}

/// The opposite of `change`, made on `before`, for `u`.
fn opposite(before: &List, change: &Change, id: u32) -> Option<Change> {
    match change {
        Change::Add { .. } => Some(Change::Remove(id)),
        Change::Remove(id) => {
            let at = before.tasks.iter().position(|t| t.id == *id)?;
            Some(Change::Restore { task: Box::new(before.tasks[at].clone()), at })
        }
        Change::Set { id, edit } => {
            let t = before.get(*id)?;
            Some(Change::Set {
                id: *id,
                edit: Edit {
                    title: edit.title.as_ref().map(|_| t.title.clone()),
                    kind: edit.kind.map(|_| t.kind),
                    status: edit.status.map(|_| t.status),
                    priority: edit.priority.map(|_| t.priority),
                    body: (edit.body.is_some() || edit.note.is_some()).then(|| t.body.clone()),
                    note: None,
                    thread: edit.thread.as_ref().map(|_| t.thread.clone().unwrap_or_default()),
                    branch: edit.branch.as_ref().map(|_| t.branch.clone().unwrap_or_default()),
                },
            })
        }
        Change::Move { .. } | Change::Restore { .. } => None,
    }
}

/// What a change did, in a few words, for "Undone: …".
fn describe(change: &Change, id: u32) -> String {
    match change {
        Change::Add { .. } => format!("adding #{id}"),
        Change::Remove(_) => format!("removing #{id}"),
        Change::Restore { .. } => format!("bringing back #{id}"),
        Change::Move { .. } => format!("moving #{id}"),
        Change::Set { .. } => format!("the change to #{id}"),
    }
}

/// The sentence for an edit: `#12 is done`, `#12 is high priority`.
fn said(t: &Task, e: &Edit) -> String {
    let id = t.id;
    match (e.status, e.priority, e.kind, &e.title, &e.body) {
        (Some(s), None, None, None, None) => format!("#{id} is {}", s.word()),
        (None, Some(p), None, None, None) => format!("#{id} is {} priority", p.id()),
        (None, None, Some(k), None, None) => format!("#{id} is a {}", k.id()),
        _ => format!("Saved #{id}"),
    }
}

/// Asks the terminal for its colours, as the desl client does at start, so the row under the
/// cursor takes the same tint as the sidebar's. Under desl, desl answers for the terminal.
fn probe_colours() {
    use filedescriptor::{FileDescriptor, POLLIN, pollfd};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    let Ok(mut input) = FileDescriptor::dup(&std::io::stdin()) else { return };
    let mut out = std::io::stdout();
    if out.write_all(desl_proto::term::PROBE).and_then(|()| out.flush()).is_err() {
        return;
    }
    let mut answers = desl_proto::term::Answers::default();
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut buf = [0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut wait = [pollfd { fd: input.as_raw_fd(), events: POLLIN, revents: 0 }];
        match filedescriptor::poll(&mut wait, Some(left)) {
            Ok(n) if n > 0 => {}
            _ => break,
        }
        match input.read(&mut buf) {
            Ok(n) if n > 0 => {
                if answers.take(&buf[..n]) {
                    break;
                }
            }
            _ => break,
        }
    }
    theme::set_surface_from(answers.bg.as_deref(), answers.fg.as_deref());
}

/// Runs the task list in this terminal until `q`.
pub fn run(store: Store) -> Result<()> {
    use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
    use ratatui::crossterm::execute;
    // Opening the list of a project that has none makes it, so there is a file to show.
    store.create()?;
    theme::set_nerd(crate::native::nerd_font());
    let mut app = App::new(store)?;
    let mut terminal = ratatui::try_init()?;
    probe_colours();
    let _ = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste);
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|f| app.draw(f))?;
            if app.quit {
                return Ok(());
            }
            if event::poll(LOOK)? {
                match event::read()? {
                    Event::Key(k) => app.key(k),
                    Event::Mouse(m) => app.mouse(m),
                    Event::Paste(text) => app.paste(&text),
                    _ => {}
                }
            }
            app.tick();
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }
    fn code(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn rig(name: &str, text: &str) -> (App, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("desl-tasks-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(super::super::list::FILE), text).unwrap();
        (App::new(Store::of(&dir)).unwrap(), dir)
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| app.draw(f)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    const THREE: &str = "## Rewrap text on resize\n- id: 1\n- type: feature\n- status: in-progress\n- priority: high\n\nThe emulator only cuts rows.\n\n## Stale rows in a dialog\n- id: 2\n- type: bug\n\n## Ship 0.2\n- id: 3\n- status: done\n";

    #[test]
    fn the_list_opens_on_what_is_open_with_counts_in_tabs() {
        let (mut app, dir) = rig("open", THREE);
        let s = screen(&mut app, 90, 8);
        let lines: Vec<&str> = s.lines().collect();
        assert!(
            lines[0].contains("Open 2")
                && lines[0].contains("◐ In progress 1")
                && lines[0].contains("✓ Done 1")
                && lines[0].contains("All 3"),
            "{s}"
        );
        assert!(lines[2].contains("▌ ◐ #1  feat   Rewrap text on resize ≡") && lines[2].ends_with("!!!"), "{s}");
        assert!(lines[3].contains("○ #2  bug    Stale rows in a dialog") && lines[3].ends_with("!!"), "{s}");
        assert!(!s.contains("Ship 0.2"), "done is not open: {s}");
        assert!(lines[7].contains("n new") && lines[7].contains("? keys"), "{s}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_task_is_added_by_type_title_and_description() {
        let (mut app, dir) = rig("add", THREE);
        app.key(key('n'));
        let s = screen(&mut app, 90, 16);
        assert!(s.contains("New task: what kind?") && s.contains("1 ") && s.contains("feature") && s.contains("bug"), "{s}");
        app.key(key('2'));
        assert!(screen(&mut app, 90, 16).contains("New bug"));
        for c in "Copilot dialog".chars() {
            app.key(key(c));
        }
        app.key(code(KeyCode::Enter));
        app.paste("Seen at width 150.\nTwice.");
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        let t = app.list.get(4).expect("added as #4");
        assert_eq!((t.title.as_str(), t.kind, t.status, t.priority), ("Copilot dialog", Kind::Bug, Status::Todo, Priority::Medium));
        assert_eq!(t.body, "Seen at width 150.\nTwice.");
        assert_eq!(app.sel, Some(4), "the new task is the one under the cursor");
        let on_disk = std::fs::read_to_string(dir.join(".tasks.md")).unwrap();
        assert!(on_disk.contains("## Copilot dialog\n- id: 4\n- type: bug\n- status: todo\n"), "{on_disk}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn done_leaves_the_open_list_and_undo_brings_it_back() {
        let (mut app, dir) = rig("done", THREE);
        assert_eq!(app.sel, Some(1));
        app.key(key('x'));
        assert_eq!(app.list.get(1).unwrap().status, Status::Done);
        assert_eq!(app.sel, Some(2), "the cursor goes to the task that took its place");
        assert!(screen(&mut app, 90, 8).contains("#1 is done · u undoes"));
        app.key(key('u'));
        assert_eq!(app.list.get(1).unwrap().status, Status::InProgress);
        app.key(key('d'));
        assert!(screen(&mut app, 90, 8).contains("Delete #"));
        app.key(key('y'));
        assert_eq!(app.list.tasks.len(), 2);
        app.key(key('u'));
        assert_eq!(app.list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 2, 3], "back in its place, with its id");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_change_in_the_file_shows_and_lights_up() {
        let (mut app, dir) = rig("fresh", THREE);
        std::thread::sleep(Duration::from_millis(20));
        let edited = std::fs::read_to_string(dir.join(".tasks.md"))
            .unwrap()
            .replace("- id: 2\n- type: bug\n", "- id: 2\n- type: bug\n- status: issue\n");
        std::fs::write(dir.join(".tasks.md"), edited).unwrap();
        app.tick();
        assert_eq!(app.list.get(2).unwrap().status, Status::Issue);
        assert!(app.fresh.contains_key(&2) && !app.fresh.contains_key(&1), "{:?}", app.fresh);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_task_opens_with_its_description_and_its_fields_change_from_lists() {
        let (mut app, dir) = rig("detail", THREE);
        app.key(code(KeyCode::Enter));
        let s = screen(&mut app, 70, 12);
        assert!(s.contains("◐ in progress") && s.contains("feature") && s.contains("!!! high") && s.contains("#1"), "{s}");
        assert!(s.contains("Rewrap text on resize") && s.contains("The emulator only cuts rows."), "{s}");
        app.key(key('p'));
        let s = screen(&mut app, 70, 12);
        assert!(s.contains("Priority of #1") && s.contains("high") && s.contains("✓"), "the current one is marked: {s}");
        app.key(key('3'));
        assert_eq!(app.list.get(1).unwrap().priority, Priority::Low);
        assert!(matches!(app.mode, Mode::Detail { id: 1, .. }), "back on the task");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn with_a_nerd_font_every_mark_is_one_icon_of_its_own() {
        let pua = |m: &str| m.chars().count() == 1 && ('\u{f0000}'..='\u{fffff}').contains(&m.chars().next().unwrap());
        let status: Vec<&str> = Status::ALL.iter().map(|s| status_mark(*s, true)).collect();
        let priority: Vec<&str> = Priority::ALL.iter().map(|p| priority_mark_in(*p, true).0).collect();
        let kinds: Vec<&str> = Kind::ALL.iter().map(|k| kind_icon(*k)).collect();
        for marks in [&status, &priority, &kinds] {
            assert!(marks.iter().all(|m| pua(m)), "Material Design icons, one cell each: {marks:?}");
            assert_eq!(marks.iter().collect::<std::collections::HashSet<_>>().len(), marks.len(), "no two alike: {marks:?}");
        }
        assert!(pua(note_mark(true)));
        assert_eq!(Status::ALL.map(|s| status_mark(s, false)), ["○", "◐", "▲", "⇡", "✓", "⊘", "‖"], "without, plain Unicode");
        assert_eq!(Priority::ALL.map(|p| priority_mark_in(p, false).0), ["!!!", "!!", "!"]);
    }

    #[test]
    fn tabs_and_search_narrow_the_list() {
        let (mut app, dir) = rig("filter", THREE);
        app.key(key('5'));
        assert_eq!(app.ids(), vec![3]);
        app.key(code(KeyCode::Tab));
        assert_eq!(app.filter, Filter::Is(Status::Ignored));
        assert!(screen(&mut app, 90, 8).contains("No task is ignored."));
        app.key(code(KeyCode::Tab));
        app.key(key('/'));
        for c in "stale".chars() {
            app.key(key(c));
        }
        assert_eq!(app.ids(), vec![2]);
        app.key(code(KeyCode::Esc));
        assert_eq!(app.ids(), vec![1, 2, 3]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
