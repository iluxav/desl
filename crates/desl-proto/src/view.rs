//! What a client shows of the workspace (SPEC.md "Interface design", web-spec.md "Indicators"):
//! the order of projects and threads, and the words and marks of each row and of a thread's
//! header. The TUI draws them with its own glyphs, and `desl web` sends them to the page as they
//! are, so the two clients cannot say different things.

use desl_ir::{Binding, FILES, PermissionMode, Project, SHELL, Status, Subagent, Thread, is_agent};
use serde::Serialize;
use std::ops::Range;
use std::path::Path;

/// How many of a thread's subagents are listed under it: the newest.
pub const SUBAGENTS_SHOWN: usize = 4;

/// Every thread in a fixed order: by project, then oldest first. The sidebar, `prefix n`,
/// `prefix p` and `prefix 1`…`9` follow it, so a thread keeps its place while statuses change.
pub fn order<'a>(projects: &[Project], threads: &'a [Thread]) -> Vec<&'a Thread> {
    let rank = |t: &Thread| projects.iter().position(|p| p.id == t.project_id).unwrap_or(usize::MAX);
    let mut out: Vec<&Thread> = threads.iter().collect();
    out.sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.created_at.cmp(&b.created_at)).then(a.id.cmp(&b.id)));
    out
}

/// `path` with the home directory written `~`.
pub fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// How long ago `then` was, as the sidebar writes it: `now`, `21m`, `3h`, `2d`.
pub fn age(now: i64, then: i64) -> String {
    let s = (now - then).max(0) / 1000;
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

/// A thread's title, or what stands for one until the agent writes one.
pub fn title(t: &Thread) -> &str {
    if t.name.is_empty() { "New thread" } else { t.name.as_str() }
}

/// The threads waiting for the user, in the order the sidebar lists them.
pub fn waiting<'a>(projects: &[Project], threads: &'a [Thread]) -> Vec<&'a Thread> {
    order(projects, threads).into_iter().filter(|t| t.status == Status::Waiting).collect()
}

/// Which of a thread's subagents are listed under it: the newest [`SUBAGENTS_SHOWN`], oldest
/// first.
pub fn subagents_shown(t: &Thread) -> Range<usize> {
    let n = t.subagents.len();
    n.saturating_sub(SUBAGENTS_SHOWN)..n
}

/// Where a subagent is.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum SubState {
    Running,
    Done,
    /// Left unfinished by a turn that was stopped.
    Unfinished,
}

/// Running only while its thread's turn is: one left unfinished by a stopped turn is not.
pub fn sub_state(s: &Subagent, t: &Thread) -> SubState {
    match (s.finished, t.mid_turn()) {
        (true, _) => SubState::Done,
        (false, true) => SubState::Running,
        (false, false) => SubState::Unfinished,
    }
}

/// A subagent's line: `Explore · price the hosts`, or its description alone when its kind says
/// nothing (`general-purpose`).
pub fn sub_label(s: &Subagent) -> String {
    let what = if s.description.is_empty() { s.kind.clone() } else { s.description.clone() };
    if s.kind == "general-purpose" || s.description.is_empty() { what } else { format!("{} · {what}", s.kind) }
}

/// The model a thread's header names: the one the agent reported for its session, else the one
/// picked with `desl model` that it has not started on yet, then the effort picked. Empty when
/// neither is known (SPEC.md principle 8, honest status).
pub fn model(b: Option<&Binding>) -> String {
    let name = b.and_then(|b| b.model.clone().or_else(|| b.model_choice.clone())).unwrap_or_default();
    match b.and_then(|b| b.effort_choice.as_deref()) {
        Some(e) if name.is_empty() => format!("{e} effort"),
        Some(e) => format!("{name} · {e}"),
        None => name,
    }
}

/// The thread lets its agent do anything without asking.
pub fn yolo(t: &Thread) -> bool {
    t.permission_mode == PermissionMode::Yolo
}

/// Eight hues a project can have, none of them the accent's blue: a project's colour is never
/// mistaken for "this pane has the keys".
pub const HUES: [(u8, u8, u8); 8] =
    [(242, 142, 43), (118, 183, 178), (89, 161, 79), (237, 201, 72), (176, 122, 161), (255, 157, 167), (156, 117, 95), (181, 189, 104)];

/// A project's own colour, the same in every client and every build: one of [`HUES`], picked
/// from its path with FNV-1a.
pub fn hue(path: &str) -> (u8, u8, u8) {
    let h = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    HUES[(h % HUES.len() as u64) as usize]
}

/// A project's name: its folder's.
pub fn project_name(path: &str) -> String {
    Path::new(path).file_name().map_or(path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Shown in the browser: agent threads and the task list. Shells and file managers stay in the
/// terminal, where they already are what a browser could show (web-spec.md "Scope").
pub fn on_page(t: &Thread) -> bool {
    t.active_agent != SHELL && t.active_agent != FILES
}

/// The sidebar, as words and marks.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Sidebar {
    /// The threads waiting for the user, in the sidebar's order.
    pub waiting: Vec<String>,
    pub projects: Vec<ProjectRow>,
    /// Every open form, oldest first: desl web's waiting column (docs/design/2026-11-21-web-forms.md).
    pub forms: Vec<FormRow>,
}

/// An open form, as the waiting column names it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FormRow {
    pub form_id: String,
    pub thread_id: String,
    pub title: String,
    /// The name of its thread's project.
    pub project: String,
    /// Since it was shown.
    pub age: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProjectRow {
    pub id: String,
    pub name: String,
    /// The folder it is in, with the home directory as `~`.
    pub parent: String,
    pub branch: Option<String>,
    /// How many paths `git status` lists.
    pub dirty: u32,
    pub hue: (u8, u8, u8),
    pub threads: Vec<ThreadRow>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ThreadRow {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub status: Status,
    pub glyph: &'static str,
    pub word: &'static str,
    /// Since it last changed; empty for a thread that runs no agent.
    pub age: String,
    /// It works in a git worktree of its own.
    pub worktree: bool,
    pub subagents: Vec<SubRow>,
    pub header: Header,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SubRow {
    pub id: String,
    pub label: String,
    pub state: SubState,
}

/// What a thread's header says besides its title, status and id.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Header {
    /// Its project's name.
    pub project: String,
    /// The branch of its own worktree; none for a thread in the project's checkout.
    pub branch: Option<String>,
    /// What is uncommitted in its own worktree.
    pub dirty: u32,
    pub model: String,
    /// Its permission mode: `ask`, `edits`, `worktree` or `yolo`.
    pub mode: &'static str,
    pub yolo: bool,
}

/// The sidebar of `threads` that `keep` says to show, at `now` (milliseconds since the epoch).
/// A project with no thread shown is left out.
pub fn sidebar(projects: &[Project], threads: &[Thread], now: i64, keep: impl Fn(&Thread) -> bool) -> Sidebar {
    let mut rows: Vec<ProjectRow> = Vec::new();
    for t in order(projects, threads).into_iter().filter(|t| keep(t)) {
        if rows.last().is_none_or(|p| p.id != t.project_id) {
            let p = projects.iter().find(|p| p.id == t.project_id);
            let path = p.map_or(t.project_id.as_str(), |p| p.path.as_str());
            let parent = Path::new(path).parent().map(|d| tilde(&d.to_string_lossy())).unwrap_or_default();
            let git = p.map(|p| &p.git);
            rows.push(ProjectRow {
                id: t.project_id.clone(),
                name: project_name(path),
                parent,
                branch: git.and_then(|g| g.branch.clone()),
                dirty: git.map_or(0, |g| g.dirty),
                hue: hue(path),
                threads: Vec::new(),
            });
        }
        let Some(project) = rows.last_mut() else { continue };
        let own = t.worktree.is_some();
        let row = ThreadRow {
            id: t.id.clone(),
            agent: t.active_agent.clone(),
            title: title(t).to_string(),
            status: t.status,
            glyph: t.status.glyph(),
            word: t.status.word(),
            age: if is_agent(&t.active_agent) { age(now, t.updated_at) } else { String::new() },
            worktree: own,
            subagents: subagents_shown(t)
                .map(|k| &t.subagents[k])
                .map(|s| SubRow { id: s.id.clone(), label: sub_label(s), state: sub_state(s, t) })
                .collect(),
            header: Header {
                project: project.name.clone(),
                branch: if own { t.git.branch.clone() } else { None },
                dirty: if own { t.git.dirty } else { 0 },
                model: model(t.binding(&t.active_agent)),
                mode: t.permission_mode.word(),
                yolo: yolo(t),
            },
        };
        project.threads.push(row);
    }
    let waiting = waiting(projects, threads).into_iter().filter(|t| keep(t)).map(|t| t.id.clone()).collect();
    let mut forms: Vec<(i64, FormRow)> = threads
        .iter()
        .filter(|t| keep(t))
        .flat_map(|t| {
            let path = projects.iter().find(|p| p.id == t.project_id).map_or(t.project_id.as_str(), |p| p.path.as_str());
            t.open_forms.iter().map(move |f| {
                let row = FormRow {
                    form_id: f.id.clone(),
                    thread_id: t.id.clone(),
                    title: f.title.clone(),
                    project: project_name(path),
                    age: age(now, f.at),
                };
                (f.at, row)
            })
        })
        .collect();
    forms.sort_by_key(|(at, f)| (*at, f.form_id.clone()));
    Sidebar { waiting, projects: rows, forms: forms.into_iter().map(|(_, f)| f).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_ir::{GitState, OpenForm};

    fn thread(id: &str, project: &str, created: i64, agent: &str, status: Status) -> Thread {
        serde_json::from_value(serde_json::json!({
            "id": id, "project_id": project, "name": "", "cwd": "/w", "worktree": null,
            "parent_id": null, "fork_point": null, "active_agent": agent, "status": status,
            "permission_mode": "edits", "created_at": created, "updated_at": 0, "head": 0,
            "bindings": {}, "git": { "branch": "main", "ahead": 0, "behind": 0 }
        }))
        .expect("a thread")
    }

    fn project(id: &str, branch: Option<&str>, dirty: u32) -> Project {
        Project {
            id: id.into(),
            path: format!("/src/{id}"),
            default_agent: None,
            created_at: 0,
            git: GitState { branch: branch.map(str::to_string), dirty, ..GitState::default() },
        }
    }

    fn sub(id: &str, kind: &str, description: &str, finished: bool) -> Subagent {
        Subagent { id: id.into(), kind: kind.into(), description: description.into(), finished, ..Subagent::default() }
    }

    #[test]
    fn the_sidebar_lists_projects_in_their_order_and_threads_oldest_first() {
        let projects = [project("desl", Some("main"), 2), project("rebrand", None, 0)];
        let threads = [
            thread("t-3", "rebrand", 1, "codex", Status::Working),
            thread("t-2", "desl", 2, "claude", Status::Waiting),
            thread("t-1", "desl", 1, "shell", Status::Idle),
            thread("t-4", "desl", 3, "copilot", Status::Idle),
        ];
        let all = sidebar(&projects, &threads, 0, |_| true);
        let ids: Vec<Vec<&str>> = all.projects.iter().map(|p| p.threads.iter().map(|t| t.id.as_str()).collect()).collect();
        assert_eq!(ids, [vec!["t-1", "t-2", "t-4"], vec!["t-3"]]);
        assert_eq!(all.waiting, ["t-2"]);
        let p = &all.projects[0];
        assert_eq!((p.name.as_str(), p.parent.as_str(), p.branch.as_deref(), p.dirty), ("desl", "/src", Some("main"), 2));
        assert_eq!(p.hue, hue("/src/desl"));
        // A shell has no age; agents do.
        assert_eq!((p.threads[0].age.as_str(), p.threads[1].age.as_str()), ("", "now"));
        // The browser leaves the shell out, and so a project with nothing else to show.
        let page = sidebar(&projects, &threads[1..2], 0, on_page);
        assert_eq!(page.projects.len(), 1);
        let page = sidebar(&projects, &threads, 0, on_page);
        assert_eq!(page.projects[0].threads.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t-2", "t-4"]);
    }

    #[test]
    fn a_row_says_the_status_title_age_and_the_newest_subagents() {
        let projects = [project("desl", Some("main"), 0)];
        let mut t = thread("t-1", "desl", 1, "claude", Status::Working);
        t.updated_at = 1_000;
        t.worktree = Some("t-1".into());
        t.git = GitState { branch: Some("desl/t-1".into()), dirty: 3, ..GitState::default() };
        t.subagents = (1..=5).map(|n| sub(&format!("s{n}"), "Explore", &format!("look {n}"), n < 5)).collect();
        t.subagents[0].kind = "general-purpose".into();
        let row = &sidebar(&projects, std::slice::from_ref(&t), 1_000 + 21 * 60_000, |_| true).projects[0].threads[0];
        assert_eq!((row.title.as_str(), row.glyph, row.word, row.age.as_str(), row.worktree), ("New thread", "●", "working", "21m", true));
        let subs: Vec<(&str, SubState)> = row.subagents.iter().map(|s| (s.label.as_str(), s.state)).collect();
        assert_eq!(
            subs,
            [
                ("Explore · look 2", SubState::Done),
                ("Explore · look 3", SubState::Done),
                ("Explore · look 4", SubState::Done),
                ("Explore · look 5", SubState::Running)
            ],
            "the newest four, oldest first"
        );
        assert_eq!((row.header.branch.as_deref(), row.header.dirty), (Some("desl/t-1"), 3), "its own worktree's branch");
        // Once the turn is over, a subagent it did not finish is left unfinished.
        t.status = Status::Idle;
        assert_eq!(sub_state(&t.subagents[4], &t), SubState::Unfinished);
        assert_eq!(sub_label(&t.subagents[0]), "look 1", "a general-purpose helper is its description");
    }

    #[test]
    fn the_header_names_the_model_the_agent_reported_before_the_one_picked() {
        let b = |model: Option<&str>, choice: Option<&str>, effort: Option<&str>| Binding {
            agent: "claude".into(),
            model: model.map(str::to_string),
            model_choice: choice.map(str::to_string),
            effort_choice: effort.map(str::to_string),
            ..Binding::default()
        };
        assert_eq!(model(Some(&b(Some("claude-opus-5-5"), Some("haiku"), Some("high")))), "claude-opus-5-5 · high");
        assert_eq!(model(Some(&b(None, Some("haiku"), None))), "haiku", "picked, and not started on yet");
        assert_eq!(model(Some(&b(None, None, Some("low")))), "low effort");
        assert_eq!(model(None), "");
    }

    #[test]
    fn the_open_forms_are_listed_oldest_first_with_their_project() {
        let projects = [project("desl", Some("main"), 0), project("rebrand", None, 0)];
        let mut a = thread("t-1", "desl", 1, "claude", Status::Waiting);
        let mut b = thread("t-2", "rebrand", 2, "codex", Status::Waiting);
        b.open_forms = vec![OpenForm { id: "f-2".into(), title: "Which logo?".into(), at: 10 }];
        a.open_forms = vec![OpenForm { id: "f-1".into(), title: "Next milestone?".into(), at: 20 * 60_000 }];
        let rows = sidebar(&projects, &[a, b], 21 * 60_000, |_| true).forms;
        let seen: Vec<(&str, &str, &str, &str, &str)> =
            rows.iter().map(|f| (f.form_id.as_str(), f.thread_id.as_str(), f.title.as_str(), f.project.as_str(), f.age.as_str())).collect();
        assert_eq!(seen, [("f-2", "t-2", "Which logo?", "rebrand", "20m"), ("f-1", "t-1", "Next milestone?", "desl", "1m")]);
    }

    #[test]
    fn ages_read_as_the_sidebar_writes_them() {
        assert_eq!(age(59_999, 0), "now");
        assert_eq!(age(60_000, 0), "1m");
        assert_eq!(age(3_600_000, 0), "1h");
        assert_eq!(age(86_400_000 * 2, 0), "2d");
        assert_eq!(age(0, 5_000), "now", "a clock that went back");
    }
}
