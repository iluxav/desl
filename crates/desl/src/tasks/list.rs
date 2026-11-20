//! A project's task list (SPEC.md "Tasks"): `.tasks.md` at the project's root, Markdown a person
//! reads anywhere and an agent edits like any other file. One `##` heading per task, its fields
//! as a short list under it, then its description:
//!
//! ```markdown
//! ## Rewrap text when a pane's width changes
//! - id: 12
//! - type: feature
//! - status: in-progress
//! - priority: high
//!
//! The emulator's set_size only cuts rows.
//! ```
//!
//! Reading forgives: fields in any order, words the user would type (`in progress`, `wip`,
//! `med`), a task without an id. Writing is always the one shape above, and atomic: the file is
//! read again just before, the one change is made to what is there, and the new file replaces
//! the old in one rename, so a change made meanwhile by an agent or another desl is kept.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The file, at the project's root.
pub const FILE: &str = desl_ir::TASK_LIST;

/// What a new file says before its first task.
const PREAMBLE: &str = "# Tasks

The task list of this project, as desl shows it. Change it in desl's tasks thread, or with
`desl task` (`desl task --help`), which is how agents change it. By hand, keep the shape:
one `##` heading per task, its fields under it, then its description.";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Todo,
    InProgress,
    Issue,
    Done,
    Ignored,
}

impl Status {
    pub const ALL: [Status; 5] = [Status::Todo, Status::InProgress, Status::Issue, Status::Done, Status::Ignored];

    /// As the file, the CLI and JSON write it.
    pub fn id(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::InProgress => "in-progress",
            Status::Issue => "issue",
            Status::Done => "done",
            Status::Ignored => "ignored",
        }
    }

    /// As a sentence or the screen says it.
    pub fn word(self) -> &'static str {
        match self {
            Status::InProgress => "in progress",
            other => other.id(),
        }
    }

    /// Still to do: the tasks the list opens on.
    pub fn open(self) -> bool {
        matches!(self, Status::Todo | Status::InProgress | Status::Issue)
    }

    pub fn parse(s: &str) -> Option<Status> {
        Some(match norm(s).as_str() {
            "todo" | "to-do" | "open" | "new" | "backlog" => Status::Todo,
            "in-progress" | "inprogress" | "doing" | "wip" | "started" | "active" | "progress" => Status::InProgress,
            "issue" | "blocked" | "problem" | "stuck" => Status::Issue,
            "done" | "closed" | "finished" | "complete" | "completed" | "fixed" => Status::Done,
            "ignored" | "ignore" | "wontfix" | "won't-fix" | "skip" | "skipped" | "dropped" => Status::Ignored,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    High,
    Medium,
    Low,
}

impl Priority {
    pub const ALL: [Priority; 3] = [Priority::High, Priority::Medium, Priority::Low];

    pub fn id(self) -> &'static str {
        match self {
            Priority::High => "high",
            Priority::Medium => "medium",
            Priority::Low => "low",
        }
    }

    pub fn parse(s: &str) -> Option<Priority> {
        Some(match norm(s).as_str() {
            "high" | "h" | "hi" | "p1" | "urgent" | "critical" | "!!!" => Priority::High,
            "medium" | "m" | "med" | "mid" | "normal" | "p2" | "!!" => Priority::Medium,
            "low" | "l" | "lo" | "p3" | "minor" | "!" => Priority::Low,
            _ => return None,
        })
    }
}

/// A task's type: what kind of work it is.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Feature,
    Bug,
    Task,
    Refactor,
    Test,
    Docs,
    Idea,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Feature, Kind::Bug, Kind::Task, Kind::Refactor, Kind::Test, Kind::Docs, Kind::Idea];

    pub fn id(self) -> &'static str {
        match self {
            Kind::Feature => "feature",
            Kind::Bug => "bug",
            Kind::Task => "task",
            Kind::Refactor => "refactor",
            Kind::Test => "test",
            Kind::Docs => "docs",
            Kind::Idea => "idea",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match norm(s).as_str() {
            "feature" | "feat" | "story" | "enhancement" => Kind::Feature,
            "bug" | "fix" | "bugfix" | "defect" => Kind::Bug,
            "task" | "chore" | "todo" | "ops" | "build" | "ci" => Kind::Task,
            "refactor" | "refactoring" | "cleanup" | "clean-up" | "tech-debt" | "perf" => Kind::Refactor,
            "test" | "tests" | "testing" => Kind::Test,
            "docs" | "doc" | "documentation" => Kind::Docs,
            "idea" | "question" | "research" | "spike" => Kind::Idea,
            _ => return None,
        })
    }
}

fn norm(s: &str) -> String {
    s.trim().trim_matches(['`', '*', '"', '\'']).to_lowercase().replace([' ', '_'], "-")
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Task {
    pub id: u32,
    pub title: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub status: Status,
    pub priority: Priority,
    /// Markdown; empty when the task has none.
    pub body: String,
    /// Fields of the task's own list that desl does not know (`- owner: claude`), kept as written.
    #[serde(skip)]
    pub extra: Vec<(String, String)>,
}

impl Task {
    pub fn new(id: u32, title: &str, kind: Kind) -> Task {
        Task { id, title: one_line(title), kind, status: Status::Todo, priority: Priority::Medium, body: String::new(), extra: Vec::new() }
    }
}

/// A title is one line.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct List {
    /// What comes before the first task, kept as written.
    pub preamble: String,
    pub tasks: Vec<Task>,
}

/// `- key: value`, the shape of a task's fields.
fn field(line: &str) -> Option<(String, String)> {
    let rest = line.trim_start().strip_prefix(['-', '*'])?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let (key, value) = rest.trim_start().split_once(':')?;
    let key = key.trim();
    if key.is_empty() || key.len() > 24 || !key.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some((key.to_lowercase(), value.trim().to_string()))
}

/// A fence opens or closes a code block, where a `##` line is code, not a task.
fn fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

fn heading(line: &str) -> Option<&str> {
    line.strip_prefix("## ").or_else(|| (line.trim_end() == "##").then_some(""))
}

impl List {
    pub fn parse(text: &str) -> List {
        let mut preamble = Vec::new();
        // Each task's heading and the lines under it, up to the next heading.
        let mut chunks: Vec<(String, Vec<&str>)> = Vec::new();
        let mut in_code = false;
        for line in text.lines() {
            if !in_code && let Some(title) = heading(line) {
                chunks.push((title.trim().to_string(), Vec::new()));
                continue;
            }
            if fence(line) {
                in_code = !in_code;
            }
            match chunks.last_mut() {
                Some((_, lines)) => lines.push(line),
                None => preamble.push(line),
            }
        }
        let mut list = List { preamble: preamble.join("\n").trim().to_string(), tasks: Vec::new() };
        let mut ids = std::collections::HashSet::new();
        let mut unnumbered = Vec::new();
        for (title, lines) in chunks {
            let mut task = Task::new(0, &title, Kind::Task);
            // The fields come first, perhaps after a blank line.
            let mut at = match lines.iter().position(|l| !l.trim().is_empty()) {
                Some(first) if field(lines[first]).is_some() => first,
                _ => 0,
            };
            while let Some((key, value)) = lines.get(at).and_then(|l| field(l)) {
                match key.as_str() {
                    "id" => task.id = value.trim_start_matches('#').parse().unwrap_or(0),
                    "type" | "kind" => task.kind = Kind::parse(&value).unwrap_or(Kind::Task),
                    "status" | "state" => task.status = Status::parse(&value).unwrap_or(Status::Todo),
                    "priority" | "prio" => task.priority = Priority::parse(&value).unwrap_or(Priority::Medium),
                    _ => task.extra.push((key, value)),
                }
                at += 1;
            }
            task.body = trim_lines(&lines[at..]);
            if task.title.is_empty() {
                task.title = "Untitled".to_string();
            }
            if task.id == 0 || !ids.insert(task.id) {
                unnumbered.push(list.tasks.len());
            }
            list.tasks.push(task);
        }
        // A task written without an id, or with one another task has, gets the next free one.
        for i in unnumbered {
            let id = list.next_id();
            list.tasks[i].id = id;
        }
        list
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let preamble = if self.preamble.trim().is_empty() { PREAMBLE } else { self.preamble.trim() };
        out.push_str(preamble);
        out.push_str("\n\n");
        for t in &self.tasks {
            out.push_str(&format!("## {}\n", one_line(&t.title)));
            out.push_str(&format!(
                "- id: {}\n- type: {}\n- status: {}\n- priority: {}\n",
                t.id,
                t.kind.id(),
                t.status.id(),
                t.priority.id()
            ));
            for (k, v) in &t.extra {
                out.push_str(&format!("- {k}: {}\n", one_line(v)));
            }
            let body = clean_body(&t.body);
            if !body.is_empty() {
                out.push('\n');
                out.push_str(&body);
                out.push('\n');
            }
            out.push('\n');
        }
        while out.ends_with("\n\n") {
            out.pop();
        }
        out
    }

    pub fn next_id(&self) -> u32 {
        self.tasks.iter().map(|t| t.id).max().unwrap_or(0) + 1
    }

    pub fn get(&self, id: u32) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    fn get_mut(&mut self, id: u32) -> Result<&mut Task, Missing> {
        self.tasks.iter_mut().find(|t| t.id == id).ok_or(Missing(id))
    }

    /// Makes one change. The id of the task it made or changed.
    pub fn apply(&mut self, change: &Change) -> Result<u32, Missing> {
        match change {
            Change::Add { title, kind, status, priority, body } => {
                let id = self.next_id();
                let mut t = Task::new(id, title, *kind);
                t.status = *status;
                t.priority = *priority;
                t.body = body.trim().to_string();
                self.tasks.push(t);
                Ok(id)
            }
            Change::Set { id, edit } => {
                let t = self.get_mut(*id)?;
                if let Some(title) = edit.title.as_deref().map(one_line).filter(|s| !s.is_empty()) {
                    t.title = title;
                }
                if let Some(k) = edit.kind {
                    t.kind = k;
                }
                if let Some(s) = edit.status {
                    t.status = s;
                }
                if let Some(p) = edit.priority {
                    t.priority = p;
                }
                if let Some(body) = &edit.body {
                    t.body = body.trim().to_string();
                }
                if let Some(note) = edit.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                    t.body = if t.body.is_empty() { note.to_string() } else { format!("{}\n\n{note}", t.body) };
                }
                Ok(*id)
            }
            Change::Remove(id) => {
                let i = self.tasks.iter().position(|t| t.id == *id).ok_or(Missing(*id))?;
                self.tasks.remove(i);
                Ok(*id)
            }
            Change::Restore { task, at } => {
                let mut t = (**task).clone();
                if self.get(t.id).is_some() {
                    t.id = self.next_id();
                }
                let id = t.id;
                self.tasks.insert((*at).min(self.tasks.len()), t);
                Ok(id)
            }
            Change::Move { id, past } => {
                let from = self.tasks.iter().position(|t| t.id == *id).ok_or(Missing(*id))?;
                let to = self.tasks.iter().position(|t| t.id == *past).ok_or(Missing(*past))?;
                let t = self.tasks.remove(from);
                self.tasks.insert(to, t);
                Ok(*id)
            }
        }
    }
}

/// The task a change named is not in the list (any more).
#[derive(Debug, PartialEq, Eq)]
pub struct Missing(pub u32);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub title: Option<String>,
    pub kind: Option<Kind>,
    pub status: Option<Status>,
    pub priority: Option<Priority>,
    /// The whole description, in place of what it was.
    pub body: Option<String>,
    /// A paragraph added under the description.
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Add {
        title: String,
        kind: Kind,
        status: Status,
        priority: Priority,
        body: String,
    },
    Set {
        id: u32,
        edit: Edit,
    },
    Remove(u32),
    /// Takes the place of task `past`, which moves one place toward where it came from.
    Move {
        id: u32,
        past: u32,
    },
    /// Puts a removed task back at its place, with its id unless another task has it now.
    Restore {
        task: Box<Task>,
        at: usize,
    },
}

fn trim_lines(lines: &[&str]) -> String {
    let start = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(start, |i| i + 1);
    lines[start..end].iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
}

/// A description as the file holds it: a `##` line outside code would start a task, so it
/// becomes a `###` one.
fn clean_body(body: &str) -> String {
    let mut in_code = false;
    let lines: Vec<String> = body
        .lines()
        .map(|l| {
            if fence(l) {
                in_code = !in_code;
            }
            if !in_code && heading(l).is_some() { format!("#{l}") } else { l.trim_end().to_string() }
        })
        .collect();
    trim_lines(&lines.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Where a project's list lives, and the only way it is written.
#[derive(Clone, Debug)]
pub struct Store {
    pub path: PathBuf,
}

/// When the file last changed and how long it is: a change by anyone else shows in one of them.
pub type Stamp = Option<(SystemTime, u64)>;

impl Store {
    pub fn of(root: &Path) -> Store {
        Store { path: root.join(FILE) }
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn stamp(&self) -> Stamp {
        let m = std::fs::metadata(&self.path).ok()?;
        Some((m.modified().ok()?, m.len()))
    }

    /// The list as the file has it now; an empty list when there is no file yet.
    pub fn read(&self) -> io::Result<List> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(List::parse(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(List::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes a new file with no tasks, when there is none. True when it did.
    pub fn create(&self) -> io::Result<bool> {
        if self.exists() {
            return Ok(false);
        }
        self.write(&List::default())?;
        Ok(true)
    }

    /// Reads the file, makes the one change to what it holds now, and writes it back.
    pub fn apply(&self, change: &Change) -> io::Result<Result<(List, u32), Missing>> {
        let mut list = self.read()?;
        let id = match list.apply(change) {
            Ok(id) => id,
            Err(missing) => return Ok(Err(missing)),
        };
        self.write(&list)?;
        Ok(Ok((list, id)))
    }

    fn write(&self, list: &List) -> io::Result<()> {
        // Through a link, to the file it points to.
        let target = std::fs::canonicalize(&self.path).unwrap_or_else(|_| self.path.clone());
        let dir = target.parent().unwrap_or(Path::new("."));
        let name = target.file_name().map_or_else(|| FILE.into(), |n| n.to_string_lossy().into_owned());
        let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        std::fs::write(&temp, list.render())?;
        std::fs::rename(&temp, &target).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        "# Tasks\n\nNotes the user keeps.\n\n## Rewrap text on resize\n- id: 12\n- type: feature\n- status: in progress\n- priority: high\n\nThe emulator only cuts rows.\n\n```md\n## not a task\n```\n\n## Header shows a raw model id\n- status: WIP\n- type: bug\n- owner: claude\n\n## Plain one\n"
    }

    #[test]
    fn a_list_reads_forgivingly_and_writes_one_shape() {
        let list = List::parse(sample());
        assert_eq!(list.preamble, "# Tasks\n\nNotes the user keeps.");
        assert_eq!(list.tasks.len(), 3, "a heading inside code is code: {list:#?}");
        let t = &list.tasks[0];
        assert_eq!((t.id, t.kind, t.status, t.priority), (12, Kind::Feature, Status::InProgress, Priority::High));
        assert_eq!(t.body, "The emulator only cuts rows.\n\n```md\n## not a task\n```");
        let t = &list.tasks[1];
        assert_eq!((t.id, t.kind, t.status, t.priority), (13, Kind::Bug, Status::InProgress, Priority::Medium), "no id: the next free one");
        assert_eq!(t.extra, vec![("owner".to_string(), "claude".to_string())], "a field desl does not know stays");
        assert_eq!((list.tasks[2].id, list.tasks[2].kind, list.tasks[2].status), (14, Kind::Task, Status::Todo));

        let text = list.render();
        assert!(text.starts_with("# Tasks\n\nNotes the user keeps.\n\n## Rewrap text on resize\n- id: 12\n- type: feature\n- status: in-progress\n- priority: high\n\nThe emulator"), "{text}");
        assert!(text.contains("## Header shows a raw model id\n- id: 13\n- type: bug\n- status: in-progress\n- priority: medium\n- owner: claude\n\n## Plain one\n- id: 14\n"), "{text}");
        assert!(text.ends_with("- priority: medium\n"), "one newline at the end: {text:?}");
        assert_eq!(List::parse(&text), list, "what is written reads back the same");
    }

    #[test]
    fn a_description_never_starts_a_task() {
        let mut list = List::default();
        let id = list
            .apply(&Change::Add {
                title: "Two\nlines".into(),
                kind: Kind::Docs,
                status: Status::Todo,
                priority: Priority::Low,
                body: "## Heading in the body\n\n- id: 99\n".into(),
            })
            .unwrap();
        assert_eq!(id, 1);
        let back = List::parse(&list.render());
        assert_eq!(back.tasks.len(), 1, "{}", list.render());
        assert_eq!(back.tasks[0].title, "Two lines");
        assert_eq!(back.tasks[0].body, "### Heading in the body\n\n- id: 99");
        assert!(list.render().starts_with(PREAMBLE), "a new file explains itself");
    }

    #[test]
    fn duplicate_ids_are_made_unique() {
        let list = List::parse("## a\n- id: 3\n## b\n- id: 3\n## c\n- id: 1\n");
        assert_eq!(list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 1 + 3, 1]);
    }

    #[test]
    fn changes_set_note_move_and_remove() {
        let mut list = List::parse("## a\n- id: 1\n## b\n- id: 2\n## c\n- id: 3\n");
        let edit = Edit { status: Some(Status::Done), note: Some("Fixed in de648fe.".into()), ..Edit::default() };
        list.apply(&Change::Set { id: 2, edit }).unwrap();
        list.apply(&Change::Set { id: 2, edit: Edit { note: Some("Checked live.".into()), ..Edit::default() } }).unwrap();
        assert_eq!(list.get(2).unwrap().body, "Fixed in de648fe.\n\nChecked live.");
        assert_eq!(list.get(2).unwrap().status, Status::Done);
        list.apply(&Change::Move { id: 3, past: 1 }).unwrap();
        assert_eq!(list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 1, 2]);
        list.apply(&Change::Move { id: 3, past: 1 }).unwrap();
        assert_eq!(list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 3, 2]);
        assert_eq!(list.apply(&Change::Remove(9)), Err(Missing(9)));
        list.apply(&Change::Remove(1)).unwrap();
        assert_eq!(list.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), vec![3, 2]);
    }

    #[test]
    fn the_store_changes_what_the_file_holds_now() {
        let dir = std::env::temp_dir().join(format!("desl-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::of(&dir);
        assert!(store.read().unwrap().tasks.is_empty(), "no file is an empty list");
        assert!(store.create().unwrap());
        assert!(!store.create().unwrap(), "a file that is there stays");
        let add = |title: &str| Change::Add {
            title: title.into(),
            kind: Kind::Bug,
            status: Status::Todo,
            priority: Priority::High,
            body: String::new(),
        };
        store.apply(&add("first")).unwrap().unwrap();
        // Someone else adds a task by hand meanwhile.
        let by_hand = std::fs::read_to_string(&store.path).unwrap() + "\n## by hand\n";
        std::fs::write(&store.path, by_hand).unwrap();
        let (list, id) = store.apply(&add("second")).unwrap().unwrap();
        assert_eq!(id, 3);
        assert_eq!(list.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["first", "by hand", "second"]);
        assert_eq!(store.read().unwrap(), list);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from(FILE)], "no temporary file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
