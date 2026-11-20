//! `desl task …`: the task list from a shell or an agent (SPEC.md "Tasks"). Each command reads
//! the file, makes its one change to what the file holds at that moment, and writes it back, so
//! the tasks thread on screen and other agents see it at once.

use super::list::{Change, Edit, Kind, List, Missing, Priority, Status, Store, Task};
use crate::cli::Out;
use crate::conn::Conn;
use anyhow::{Result, anyhow, bail};
use desl_proto::Workspace;
use desl_proto::paths::Paths;
use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};

fn workspace(paths: &Paths) -> Option<Workspace> {
    let mut conn = Conn::open(paths, false, "cli").ok().flatten()?;
    crate::cli::workspace(&mut conn).ok()
}

/// The root of the project whose list a command means: the one named, else the repository this
/// directory is in (its main checkout, from any worktree), else the innermost project this
/// directory is inside, else the directory itself.
pub fn root(paths: &Paths, project: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = project {
        let dir = Path::new(p);
        if dir.is_dir() {
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            return Ok(desl_daemon::git::main_root(&dir).unwrap_or(dir));
        }
        return match workspace(paths).and_then(|ws| ws.projects.into_iter().find(|x| x.id == p)) {
            Some(found) => Ok(found.path.into()),
            None => bail!("There is no project {p}: it is neither a directory nor a project `desl status` lists"),
        };
    }
    let cwd = std::env::current_dir()?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    if let Some(repo) = desl_daemon::git::main_root(&cwd) {
        return Ok(repo);
    }
    // Outside a repository: the innermost project this directory is in, never one elsewhere.
    if let Some(p) =
        workspace(paths).and_then(|ws| ws.projects.into_iter().filter(|p| cwd.starts_with(&p.path)).max_by_key(|p| p.path.len()))
    {
        return Ok(p.path.into());
    }
    Ok(cwd)
}

fn tilde(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && s.starts_with(&home) => format!("~{}", &s[home.len()..]),
        _ => s,
    }
}

fn id_arg(s: &str) -> Result<u32> {
    s.trim()
        .trim_start_matches('#')
        .parse()
        .map_err(|_| anyhow!("{s} is not a task id. Ids are numbers, as `desl task list` shows them: 12 or #12"))
}

fn status_arg(s: &str) -> Result<Status> {
    Status::parse(s).ok_or_else(|| anyhow!("{s} is not a status. A task is todo, in-progress, issue, done or ignored"))
}

fn priority_arg(s: &str) -> Result<Priority> {
    Priority::parse(s).ok_or_else(|| anyhow!("{s} is not a priority. A task's priority is high, medium or low"))
}

fn kind_arg(s: &str) -> Result<Kind> {
    Kind::parse(s).ok_or_else(|| {
        let all = Kind::ALL.map(|k| k.id());
        anyhow!("{s} is not a type. A task is a {} or {}", all[..all.len() - 1].join(", "), all[all.len() - 1])
    })
}

/// `-` reads the text from standard input, for a long description.
fn text_arg(s: String) -> Result<String> {
    if s != "-" {
        return Ok(s);
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    Ok(text)
}

fn missing(store: &Store, Missing(id): Missing) -> anyhow::Error {
    anyhow!("There is no task #{id} in {}. `desl task list --status all` lists every task", tilde(&store.path))
}

fn line(t: &Task, id_w: usize) -> String {
    format!("{:<id_w$}  {:<11}  {:<6}  {:<8}  {}", format!("#{}", t.id), t.status.id(), t.priority.id(), t.kind.id(), t.title)
}

pub fn list(paths: &Paths, out: &Out, project: Option<&str>, status: &str) -> Result<()> {
    let store = Store::of(&root(paths, project)?);
    let list = store.read()?;
    let words: Vec<String> = status.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
    let mut want = Vec::new();
    for w in &words {
        match w.as_str() {
            "open" => want.extend([Status::Todo, Status::InProgress, Status::Issue]),
            "all" => want.extend(Status::ALL),
            other => want.push(status_arg(other)?),
        }
    }
    let shown: Vec<&Task> = list.tasks.iter().filter(|t| want.contains(&t.status)).collect();
    if out.json {
        out.value(&json!({ "file": store.path, "tasks": shown }));
        return Ok(());
    }
    let open = list.tasks.iter().filter(|t| t.status.open()).count();
    if !store.exists() {
        println!("{} has no task list yet. `desl task add \"…\"` starts one.", tilde(store.path.parent().unwrap_or(Path::new("."))));
        return Ok(());
    }
    println!("{}: {open} open of {}", tilde(&store.path), list.tasks.len());
    if shown.is_empty() {
        println!("No task is {}.", words.join(" or "));
    }
    let id_w = shown.iter().map(|t| t.id.to_string().len() + 1).max().unwrap_or(2);
    for t in shown {
        println!("{}", line(t, id_w));
    }
    Ok(())
}

pub fn show(paths: &Paths, out: &Out, project: Option<&str>, id: &str) -> Result<()> {
    let store = Store::of(&root(paths, project)?);
    let id = id_arg(id)?;
    let list = store.read()?;
    let t = list.get(id).ok_or_else(|| missing(&store, Missing(id)))?;
    if out.json {
        out.value(&json!(t));
        return Ok(());
    }
    println!("#{} {}", t.id, t.title);
    println!("{} · {} · {} priority", t.kind.id(), t.status.word(), t.priority.id());
    if !t.body.is_empty() {
        println!("\n{}", t.body);
    }
    Ok(())
}

fn changed(store: &Store, change: &Change) -> Result<(List, u32)> {
    store.apply(change)?.map_err(|m| missing(store, m))
}

pub struct Add {
    pub title: String,
    pub kind: String,
    pub status: String,
    pub priority: String,
    pub body: Option<String>,
}

pub fn add(paths: &Paths, out: &Out, project: Option<&str>, a: Add) -> Result<()> {
    let store = Store::of(&root(paths, project)?);
    let title = super::list::one_line(&a.title);
    if title.is_empty() {
        bail!("A task needs a title: `desl task add \"Fix the flaky login test\"`");
    }
    let change = Change::Add {
        title,
        kind: kind_arg(&a.kind)?,
        status: status_arg(&a.status)?,
        priority: priority_arg(&a.priority)?,
        body: a.body.map(text_arg).transpose()?.unwrap_or_default(),
    };
    let (list, id) = changed(&store, &change)?;
    let t = list.get(id).expect("just added");
    if out.json {
        out.value(&json!(t));
    } else {
        println!(
            "Added #{id} to {}: {} ({}, {}, {} priority).",
            tilde(&store.path),
            t.title,
            t.kind.id(),
            t.status.word(),
            t.priority.id()
        );
    }
    Ok(())
}

pub struct Set {
    pub id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub body: Option<String>,
    pub note: Option<String>,
}

pub fn set(paths: &Paths, out: &Out, project: Option<&str>, s: Set) -> Result<()> {
    let store = Store::of(&root(paths, project)?);
    let id = id_arg(&s.id)?;
    let edit = Edit {
        title: s.title.filter(|t| !t.trim().is_empty()),
        kind: s.kind.as_deref().map(kind_arg).transpose()?,
        status: s.status.as_deref().map(status_arg).transpose()?,
        priority: s.priority.as_deref().map(priority_arg).transpose()?,
        body: s.body.map(text_arg).transpose()?,
        note: s.note.map(text_arg).transpose()?,
    };
    if edit == Edit::default() {
        bail!("Nothing to change. Name what changes: --status, --priority, --type, --title, --body or --note");
    }
    let (list, _) = changed(&store, &Change::Set { id, edit: edit.clone() })?;
    let t = list.get(id).expect("just changed");
    if out.json {
        out.value(&json!(t));
        return Ok(());
    }
    let mut what = Vec::new();
    if edit.status.is_some() {
        what.push(format!("status {}", t.status.word()));
    }
    if edit.priority.is_some() {
        what.push(format!("priority {}", t.priority.id()));
    }
    if edit.kind.is_some() {
        what.push(format!("type {}", t.kind.id()));
    }
    if edit.title.is_some() {
        what.push("title".into());
    }
    if edit.body.is_some() {
        what.push("description".into());
    }
    if edit.note.is_some() {
        what.push("a note under the description".into());
    }
    println!("#{id} {}: {}.", t.title, what.join(", "));
    Ok(())
}

pub fn rm(paths: &Paths, out: &Out, project: Option<&str>, id: &str) -> Result<()> {
    let store = Store::of(&root(paths, project)?);
    let id = id_arg(id)?;
    let title = store.read()?.get(id).map(|t| t.title.clone()).ok_or_else(|| missing(&store, Missing(id)))?;
    changed(&store, &Change::Remove(id))?;
    if out.json {
        out.value(&json!({ "removed": id }));
    } else {
        println!("Removed #{id}: {title}.");
    }
    Ok(())
}

pub fn ui(paths: &Paths, project: Option<&str>) -> Result<()> {
    super::ui::run(Store::of(&root(paths, project)?))
}
