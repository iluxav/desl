//! The task pane on the TUI's right (docs/design/2026-11-22-tasks-first-class.md, the TUI pane):
//! the task list of the project on screen, as desl web's right column has it. `prefix t` shows
//! it and gives it the keys; `esc` gives them back to the agent; `prefix t` again hides it. It
//! opens by itself on a terminal wide enough to keep the agent's screen wide.

use desl_proto::tasks::{Kind, Status, Task};
use std::collections::{HashMap, HashSet};

/// The terminal width at which the pane shows without being asked.
pub const AUTO_COLS: u16 = 180;

/// A filter: its name and which tasks it keeps. `h`/`l` (and tab) cycle through them: desl
/// web's segments, then what its More holds.
pub type Filter = (&'static str, fn(&Task) -> bool);

pub const FILTERS: [Filter; 8] = [
    ("Open", |t| t.status.open()),
    ("To do", |t| t.status == Status::Todo),
    ("In progress", |t| t.status == Status::InProgress),
    ("Ready", |t| t.status == Status::Ready),
    ("Done", |t| t.status == Status::Done),
    ("Issue", |t| t.status == Status::Issue),
    ("Parked", |t| t.status == Status::Parked),
    ("Bugs", |t| t.kind == Kind::Bug && t.status.open()),
];

/// Where a task starts: the panel's choices, as on the page.
pub const PLACES: [(&str, &str); 3] =
    [("worktree", "a new thread, in its own worktree"), ("main", "a new thread, in the main checkout"), ("thread", "this thread")];

/// What a key in the pane asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// Start the task: the agent (None for the project's default) and the place (`worktree`,
    /// `main` or `thread:<id>`).
    Start {
        task: u32,
        agent: Option<String>,
        place: String,
    },
    Go(String),
    Merge(u32),
    Diff(u32),
    Rebase(u32),
    /// The full task editor, for this project.
    Edit,
    /// Give the keys back to the agent.
    Back,
    Say(String),
}

/// The agent-and-place picker under a task, while it is open: indexes into the agents (0 is
/// the project's default) and into [`PLACES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pick {
    pub agent: usize,
    pub place: usize,
}

#[derive(Default)]
pub struct TaskPane {
    /// The user's choice once they pressed `prefix t`; before that, the terminal's width decides.
    pub chosen: Option<bool>,
    pub focused: bool,
    pub cursor: usize,
    pub filter: usize,
    /// Each project's list as the daemon last told it.
    pub lists: HashMap<String, Vec<Task>>,
    /// The projects whose list was asked for on this connection.
    pub asked: HashSet<String>,
    pub picking: Option<Pick>,
    /// The last pick, which ⏎ starts with.
    pub last: Pick,
    /// Ready tasks whose merge was refused because main moved on.
    pub needs_rebase: HashSet<(String, u32)>,
}

impl TaskPane {
    /// Whether the pane shows on a terminal `cols` wide.
    pub fn shown(&self, cols: u16) -> bool {
        self.chosen.unwrap_or(cols >= AUTO_COLS)
    }

    /// Whether to ask the daemon for `project`'s list now: the pane shows on a terminal `cols`
    /// wide, and the list is neither here nor asked for. True once a project.
    pub fn wants(&mut self, project: &str, cols: u16) -> bool {
        self.shown(cols) && !self.lists.contains_key(project) && self.asked.insert(project.to_string())
    }

    /// `project`'s tasks the filter keeps, in the file's order.
    pub fn rows<'a>(&'a self, project: &str) -> Vec<&'a Task> {
        let keep = FILTERS[self.filter % FILTERS.len()].1;
        self.lists.get(project).map(|l| l.iter().filter(|t| keep(t)).collect()).unwrap_or_default()
    }

    /// Every filter, each with how many of `project`'s tasks it keeps.
    pub fn segments(&self, project: &str) -> Vec<(&'static str, usize)> {
        let list = self.lists.get(project).map(Vec::as_slice).unwrap_or_default();
        FILTERS.iter().map(|(name, keep)| (*name, list.iter().filter(|t| keep(t)).count())).collect()
    }

    /// How many tasks the filter keeps.
    pub fn count(&self, project: &str) -> usize {
        self.rows(project).len()
    }

    /// A key while the pane has them. `agents` are the installed agents' ids; `here` is the
    /// thread on screen.
    pub fn key(&mut self, project: &str, key: Key, agents: &[String], here: &str) -> Option<Ask> {
        let n = self.count(project);
        if let Some(mut p) = self.picking {
            let choices = agents.len() + 1;
            match key {
                Key::Close => self.picking = None,
                Key::Char('a') | Key::Char('l') => p.agent = (p.agent + 1) % choices,
                Key::Char('h') => p.agent = (p.agent + choices - 1) % choices,
                Key::Char('p') | Key::Down | Key::Char('j') => p.place = (p.place + 1) % PLACES.len(),
                Key::Up | Key::Char('k') => p.place = (p.place + PLACES.len() - 1) % PLACES.len(),
                Key::Enter => {
                    self.picking = None;
                    self.last = p;
                    let task = self.rows(project).get(self.cursor).map(|t| t.id)?;
                    return Some(self.start(task, p, agents, here));
                }
                _ => {}
            }
            if self.picking.is_some() {
                self.picking = Some(p);
            }
            return None;
        }
        let task = self.rows(project).get(self.cursor.min(n.saturating_sub(1))).map(|t| (*t).clone());
        match key {
            Key::Close => return Some(Ask::Back),
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Char('l') | Key::Tab => {
                self.filter = (self.filter + 1) % FILTERS.len();
                self.cursor = 0;
            }
            Key::Char('h') => {
                self.filter = (self.filter + FILTERS.len() - 1) % FILTERS.len();
                self.cursor = 0;
            }
            Key::Char('e') => return Some(Ask::Edit),
            Key::Enter => {
                let t = task?;
                return Some(match t.status {
                    Status::Todo | Status::Issue | Status::Parked => self.start(t.id, self.last, agents, here),
                    Status::Ready => Ask::Diff(t.id),
                    _ => match t.thread {
                        Some(th) => Ask::Go(th),
                        None => Ask::Say(format!("#{} is {}: nothing to start.", t.id, t.status.word())),
                    },
                });
            }
            Key::Char('w') => match task {
                Some(t) if matches!(t.status, Status::Todo | Status::Issue | Status::Parked) => self.picking = Some(self.last),
                Some(t) => return Some(Ask::Say(format!("#{} is {}: only a task to do starts.", t.id, t.status.word()))),
                None => {}
            },
            Key::Char('g') => {
                let t = task?;
                return Some(match t.thread {
                    Some(th) => Ask::Go(th),
                    None => Ask::Say(format!("No thread works on #{} yet.", t.id)),
                });
            }
            Key::Char('m') | Key::Char('d') | Key::Char('r') => {
                let t = task?;
                if t.status != Status::Ready {
                    return Some(Ask::Say(format!(
                        "#{} is {}, not ready: its agent marks it ready when its work is done.",
                        t.id,
                        t.status.word()
                    )));
                }
                return Some(match key {
                    Key::Char('m') => Ask::Merge(t.id),
                    Key::Char('d') => Ask::Diff(t.id),
                    _ => Ask::Rebase(t.id),
                });
            }
            _ => {}
        }
        None
    }

    fn start(&self, task: u32, p: Pick, agents: &[String], here: &str) -> Ask {
        let agent = p.agent.checked_sub(1).and_then(|i| agents.get(i)).cloned();
        let place = match PLACES[p.place % PLACES.len()].0 {
            "thread" => format!("thread:{here}"),
            other => other.to_string(),
        };
        Ask::Start { task, agent, place }
    }
}

/// The keys the pane reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Tab,
    Enter,
    Close,
    Char(char),
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_proto::tasks::Priority;

    fn task(id: u32, kind: Kind, status: Status) -> Task {
        Task { status, priority: Priority::Medium, ..Task::new(id, &format!("Task {id}"), kind) }
    }

    fn pane() -> TaskPane {
        let mut p = TaskPane::default();
        let mut ready = task(4, Kind::Bug, Status::Ready);
        ready.thread = Some("t-4".into());
        ready.branch = Some("desl/t-4".into());
        p.lists.insert(
            "desl".into(),
            vec![task(1, Kind::Bug, Status::Todo), task(2, Kind::Feature, Status::Parked), task(3, Kind::Task, Status::Done), ready],
        );
        p
    }

    #[test]
    fn the_pane_opens_by_itself_on_a_wide_terminal_until_the_user_says_otherwise() {
        let mut p = TaskPane::default();
        assert!(!p.shown(150) && p.shown(AUTO_COLS));
        p.chosen = Some(true);
        assert!(p.shown(100), "prefix t shows it anywhere");
        p.chosen = Some(false);
        assert!(!p.shown(250));
    }

    #[test]
    fn a_shown_pane_asks_once_for_each_project_it_shows() {
        let mut p = TaskPane::default();
        assert!(!p.wants("desl", 150), "hidden on a narrow terminal: nothing to ask");
        assert!(p.wants("desl", AUTO_COLS), "shown by itself on a wide one: ask");
        assert!(!p.wants("desl", AUTO_COLS), "once");
        assert!(p.wants("osfeed", AUTO_COLS), "another project on screen: ask for its list");
        let mut known = pane();
        assert!(!known.wants("desl", AUTO_COLS), "a list the daemon already sent needs no asking");
    }

    #[test]
    fn the_filters_keep_what_the_pages_do() {
        let mut p = pane();
        let ids = |p: &TaskPane| p.rows("desl").iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(&p), [1, 4], "open: to do and ready");
        p.key("desl", Key::Char('l'), &[], "t-0");
        assert_eq!((FILTERS[p.filter].0, ids(&p)), ("To do", vec![1]));
        p.filter = 6;
        assert_eq!(ids(&p), [2], "parked");
        p.filter = 7;
        assert_eq!(ids(&p), [1, 4], "open bugs");
    }

    #[test]
    fn keys_start_pick_merge_and_go() {
        let agents = vec!["claude".to_string(), "codex".to_string()];
        let mut p = pane();
        // ⏎ on a task to do starts it with the last pick: the default agent, its own worktree.
        assert_eq!(p.key("desl", Key::Enter, &agents, "t-0"), Some(Ask::Start { task: 1, agent: None, place: "worktree".into() }));
        // w picks: the next agent, the place two down (this thread), ⏎ starts and remembers.
        assert_eq!(p.key("desl", Key::Char('w'), &agents, "t-0"), None);
        p.key("desl", Key::Char('a'), &agents, "t-0");
        p.key("desl", Key::Char('p'), &agents, "t-0");
        p.key("desl", Key::Char('p'), &agents, "t-0");
        assert_eq!(
            p.key("desl", Key::Enter, &agents, "t-0"),
            Some(Ask::Start { task: 1, agent: Some("claude".into()), place: "thread:t-0".into() })
        );
        assert_eq!(p.last, Pick { agent: 1, place: 2 });
        // On the ready task: ⏎ shows the diff, m merges, g goes to its thread.
        p.key("desl", Key::Down, &agents, "t-0");
        assert_eq!(p.key("desl", Key::Enter, &agents, "t-0"), Some(Ask::Diff(4)));
        assert_eq!(p.key("desl", Key::Char('m'), &agents, "t-0"), Some(Ask::Merge(4)));
        assert_eq!(p.key("desl", Key::Char('g'), &agents, "t-0"), Some(Ask::Go("t-4".into())));
        // m on a task to do says why not.
        p.key("desl", Key::Up, &agents, "t-0");
        assert!(matches!(p.key("desl", Key::Char('m'), &agents, "t-0"), Some(Ask::Say(s)) if s.contains("not ready")));
        assert_eq!(p.key("desl", Key::Close, &agents, "t-0"), Some(Ask::Back));
    }
}
