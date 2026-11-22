//! The store (SPEC.md "Storage"): one SQLite file in WAL mode. Events are rows; large tool
//! outputs live in content-addressed files beside the database and are referenced by hash.
//! Every append is its own transaction, so a crash never loses an event that was shown.

use anyhow::{Context, Result};
use desl_ir::{Binding, Event, EventKind, GitState, OpenForm, PermissionMode, Project, Status, Thread};
use desl_proto::PastPrompt;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Bumped with every migration. Migrations only go forward.
const SCHEMA_VERSION: i64 = 7;

/// A tool output or a diff larger than this is cut in the event; the full text goes to a blob.
pub const INLINE_LIMIT: usize = 16 * 1024;

pub struct Store {
    conn: Connection,
    blobs: PathBuf,
}

fn cut(text: &str, limit: usize) -> &str {
    let mut end = limit.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Whole lines only, when there is a line break to cut at.
    match text[..end].rfind('\n') {
        Some(i) if i > 0 => &text[..i],
        _ => &text[..end],
    }
}

impl Store {
    pub fn open(db: &Path, blobs: &Path) -> Result<Store> {
        if let Some(dir) = db.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        std::fs::create_dir_all(blobs).with_context(|| format!("creating {}", blobs.display()))?;
        let conn = Connection::open(db).with_context(|| format!("opening the store at {}", db.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Store { conn, blobs: blobs.to_path_buf() };
        store.backup_before_migration(db)?;
        store.migrate()?;
        Ok(store)
    }

    /// A store about to be migrated is copied first, as `workspace.db.v<old>.bak`, and kept
    /// until the next migration replaces it (SPEC.md "Upgrades").
    fn backup_before_migration(&self, db: &Path) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 || version >= SCHEMA_VERSION {
            return Ok(());
        }
        let name = db.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "workspace.db".into());
        if let Some(dir) = db.parent() {
            for old in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let file = old.file_name().to_string_lossy().to_string();
                if file.starts_with(&format!("{name}.v")) && file.ends_with(".bak") {
                    let _ = std::fs::remove_file(old.path());
                }
            }
            let backup = dir.join(format!("{name}.v{version}.bak"));
            // Through SQLite, never by opening the file: closing another handle on it drops every
            // lock this process holds on it, and then the daemon an upgrade replaces, on its way
            // out, takes itself for the last one and deletes the write-ahead log under this one.
            self.conn
                .execute("VACUUM INTO ?1", params![backup.to_string_lossy()])
                .with_context(|| format!("backing up the store to {}", backup.display()))?;
        }
        Ok(())
    }

    /// An in-memory store for tests.
    pub fn in_memory(blobs: &Path) -> Result<Store> {
        std::fs::create_dir_all(blobs)?;
        let store = Store { conn: Connection::open_in_memory()?, blobs: blobs.to_path_buf() };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            anyhow::bail!(
                "The store was written by a newer desl (schema {version}, this build knows {SCHEMA_VERSION}). Upgrade desl with `desl upgrade`."
            );
        }
        if version < 1 {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
                 CREATE TABLE projects (
                     id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, default_agent TEXT, created_at INTEGER NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE threads (
                     id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, cwd TEXT NOT NULL,
                     worktree TEXT, parent_id TEXT, fork_point INTEGER, active_agent TEXT NOT NULL,
                     status TEXT NOT NULL, permission_mode TEXT NOT NULL,
                     created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE bindings (
                     thread_id TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT, seen_up_to INTEGER NOT NULL,
                     model TEXT, last_used_at INTEGER NOT NULL, PRIMARY KEY (thread_id, agent)
                 ) WITHOUT ROWID;
                 CREATE TABLE events (
                     thread_id TEXT NOT NULL, idx INTEGER NOT NULL, ts INTEGER NOT NULL, agent TEXT NOT NULL,
                     kind TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (thread_id, idx)
                 ) WITHOUT ROWID;
                 CREATE INDEX events_prompts ON events (ts) WHERE kind = 'user_message';
                 CREATE TABLE queue (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, text TEXT NOT NULL, created_at INTEGER NOT NULL
                 );
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        if version < 2 {
            // The model the user picked with /model, per thread and agent.
            self.conn.execute_batch("BEGIN; ALTER TABLE bindings ADD COLUMN model_choice TEXT; PRAGMA user_version = 2; COMMIT;")?;
        }
        if version < 3 {
            // The reasoning effort the user picked with /model, per thread and agent.
            self.conn.execute_batch("BEGIN; ALTER TABLE bindings ADD COLUMN effort_choice TEXT; PRAGMA user_version = 3; COMMIT;")?;
        }
        if version < 4 {
            // How far desl has read the agent's session file, per thread and agent.
            self.conn.execute_batch(
                "BEGIN; ALTER TABLE bindings ADD COLUMN file_offset INTEGER NOT NULL DEFAULT 0; PRAGMA user_version = 4; COMMIT;",
            )?;
        }
        if version < 5 {
            // The helpers a thread's agents started, as JSON: kept across switches and restarts.
            self.conn.execute_batch(
                "BEGIN; ALTER TABLE threads ADD COLUMN subagents TEXT NOT NULL DEFAULT '[]'; PRAGMA user_version = 5; COMMIT;",
            )?;
        }
        if version < 6 {
            // The user named the thread: the agents' own titles leave its name alone. Before
            // this, a name the user gave was not marked; one that is neither of the names desl
            // derives from the first prompt is taken as theirs.
            self.conn.execute_batch("BEGIN; ALTER TABLE threads ADD COLUMN named INTEGER NOT NULL DEFAULT 0;")?;
            let marked = self.mark_named();
            match marked {
                Ok(()) => self.conn.execute_batch("PRAGMA user_version = 6; COMMIT;")?,
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK;");
                    return Err(e);
                }
            }
        }
        if version < 7 {
            // Forms (docs/design/2026-11-21-web-forms.md): the form and its answer are blobs; a
            // draft is the page's state of an open form; `told` marks a dismissal the agent heard.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE forms (
                     id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, title TEXT NOT NULL, created_at INTEGER NOT NULL,
                     blob TEXT NOT NULL, closed_by TEXT, answer_blob TEXT, told INTEGER NOT NULL DEFAULT 0, draft TEXT
                 ) WITHOUT ROWID;
                 CREATE INDEX forms_open ON forms (thread_id) WHERE closed_by IS NULL;
                 PRAGMA user_version = 7;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    /// Marks the threads whose name the user gave (schema 6).
    fn mark_named(&self) -> Result<()> {
        let threads: Vec<(String, String)> = {
            let mut st = self.conn.prepare("SELECT id, name FROM threads WHERE active_agent != ?1 AND name != ''")?;
            st.query_map(params![desl_ir::SHELL], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
        };
        for (id, name) in threads {
            let first: Option<String> = self
                .conn
                .query_row(
                    "SELECT body FROM events WHERE thread_id = ?1 AND kind = 'user_message' ORDER BY idx LIMIT 1",
                    params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .and_then(|body| match serde_json::from_str::<EventKind>(&body) {
                    Ok(EventKind::UserMessage { text, .. }) => Some(text),
                    _ => None,
                });
            let derived = first.is_some_and(|text| name == desl_ir::derive_name(&text) || name == desl_ir::derive_word(&text));
            if !derived {
                self.conn.execute("UPDATE threads SET named = 1 WHERE id = ?1", params![id])?;
            }
        }
        Ok(())
    }

    // ---- projects ----------------------------------------------------------------------------

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare("SELECT id, path, default_agent, created_at FROM projects ORDER BY created_at, id")?;
        let rows = st.query_map([], |r| {
            Ok(Project { id: r.get(0)?, path: r.get(1)?, default_agent: r.get(2)?, created_at: r.get(3)?, git: GitState::default() })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn put_project(&self, p: &Project) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (id, path, default_agent, created_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET path = ?2, default_agent = ?3",
            params![p.id, p.path, p.default_agent, p.created_at],
        )?;
        Ok(())
    }

    // ---- threads -----------------------------------------------------------------------------

    pub fn threads(&self) -> Result<Vec<Thread>> {
        let mut bindings: BTreeMap<String, BTreeMap<String, Binding>> = BTreeMap::new();
        {
            let mut st = self.conn.prepare(
                "SELECT thread_id, agent, session_id, seen_up_to, model, last_used_at, model_choice, effort_choice, file_offset FROM bindings",
            )?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Binding {
                        agent: r.get(1)?,
                        session_id: r.get(2)?,
                        seen_up_to: r.get(3)?,
                        model: r.get(4)?,
                        last_used_at: r.get(5)?,
                        model_choice: r.get(6)?,
                        effort_choice: r.get(7)?,
                        file_offset: r.get(8)?,
                    },
                ))
            })?;
            for row in rows {
                let (thread_id, b) = row?;
                bindings.entry(thread_id).or_default().insert(b.agent.clone(), b);
            }
        }
        let mut queued: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let mut st = self.conn.prepare("SELECT thread_id, text FROM queue ORDER BY id")?;
            for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
                let (thread_id, text) = row?;
                queued.entry(thread_id).or_default().push(text);
            }
        }
        let mut heads: BTreeMap<String, u64> = BTreeMap::new();
        {
            let mut st = self.conn.prepare("SELECT thread_id, MAX(idx) FROM events GROUP BY thread_id")?;
            for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))? {
                let (thread_id, head) = row?;
                heads.insert(thread_id, head);
            }
        }
        let mut st = self.conn.prepare(
            "SELECT id, project_id, name, cwd, worktree, parent_id, fork_point, active_agent, status, permission_mode, created_at, updated_at, subagents, named
             FROM threads ORDER BY created_at, id",
        )?;
        let rows = st.query_map([], |r| {
            let id: String = r.get(0)?;
            let status: String = r.get(8)?;
            let mode: String = r.get(9)?;
            Ok(Thread {
                project_id: r.get(1)?,
                name: r.get(2)?,
                cwd: r.get(3)?,
                worktree: r.get(4)?,
                parent_id: r.get(5)?,
                fork_point: r.get(6)?,
                active_agent: r.get(7)?,
                status: Status::parse(&status).unwrap_or(Status::Idle),
                permission_mode: PermissionMode::parse(&mode).unwrap_or_default(),
                created_at: r.get(10)?,
                updated_at: r.get(11)?,
                head: heads.get(&id).copied().unwrap_or(0),
                bindings: bindings.remove(&id).unwrap_or_default(),
                queued: queued.remove(&id).unwrap_or_default(),
                pending_approval: None,
                subagents: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
                open_forms: Vec::new(),
                named: r.get(13)?,
                git: GitState::default(),
                id,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Writes a thread's row and its bindings. The queue and the events have their own writes.
    pub fn put_thread(&self, t: &Thread) -> Result<()> {
        self.conn.execute(
            "INSERT INTO threads (id, project_id, name, cwd, worktree, parent_id, fork_point, active_agent, status, permission_mode, created_at, updated_at, subagents, named)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT (id) DO UPDATE SET project_id = ?2, name = ?3, cwd = ?4, worktree = ?5, active_agent = ?8, status = ?9, permission_mode = ?10, updated_at = ?12, subagents = ?13, named = ?14",
            params![
                t.id, t.project_id, t.name, t.cwd, t.worktree, t.parent_id, t.fork_point, t.active_agent,
                t.status.word(), t.permission_mode.word(), t.created_at, t.updated_at,
                serde_json::to_string(&t.subagents).unwrap_or_else(|_| "[]".into()),
                t.named
            ],
        )?;
        self.conn.execute("DELETE FROM bindings WHERE thread_id = ?1", params![t.id])?;
        for b in t.bindings.values() {
            self.conn.execute(
                "INSERT INTO bindings (thread_id, agent, session_id, seen_up_to, model, last_used_at, model_choice, effort_choice, file_offset)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![t.id, b.agent, b.session_id, b.seen_up_to, b.model, b.last_used_at, b.model_choice, b.effort_choice, b.file_offset],
            )?;
        }
        Ok(())
    }

    pub fn delete_thread(&self, id: &str) -> Result<()> {
        for table in ["events", "bindings", "queue", "forms"] {
            self.conn.execute(&format!("DELETE FROM {table} WHERE thread_id = ?1"), params![id])?;
        }
        self.conn.execute("DELETE FROM threads WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---- forms -------------------------------------------------------------------------------

    pub fn form_put(&self, id: &str, thread_id: &str, title: &str, at: i64, blob: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO forms (id, thread_id, title, created_at, blob) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, thread_id, title, at, blob],
        )?;
        Ok(())
    }

    /// Closes an open form. False when it was closed already, or is not there.
    pub fn form_close(&self, id: &str, by: &str, answer_blob: Option<&str>) -> Result<bool> {
        let n = self.conn.execute(
            // An answered form needs its draft no more; one closed unanswered keeps it, so what the
            // user wrote can still be read and copied from its card.
            "UPDATE forms SET closed_by = ?2, answer_blob = ?3, draft = CASE WHEN ?3 IS NULL THEN draft END
             WHERE id = ?1 AND closed_by IS NULL",
            params![id, by, answer_blob],
        )?;
        Ok(n == 1)
    }

    /// Keeps the page's state of an open form. False when it is closed, or is not there.
    pub fn form_draft(&self, id: &str, draft: &str) -> Result<bool> {
        let n = self.conn.execute("UPDATE forms SET draft = ?2 WHERE id = ?1 AND closed_by IS NULL", params![id, draft])?;
        Ok(n == 1)
    }

    pub fn form(&self, id: &str) -> Result<Option<StoredForm>> {
        let mut st =
            self.conn.prepare("SELECT id, thread_id, title, created_at, blob, closed_by, answer_blob, draft FROM forms WHERE id = ?1")?;
        let mut rows = st.query_map(params![id], |r| {
            Ok(StoredForm {
                id: r.get(0)?,
                thread_id: r.get(1)?,
                title: r.get(2)?,
                at: r.get(3)?,
                blob: r.get(4)?,
                closed_by: r.get(5)?,
                answer_blob: r.get(6)?,
                draft: r.get(7)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Every open form, with its thread's id, oldest first.
    pub fn open_forms(&self) -> Result<Vec<(String, OpenForm)>> {
        let mut st =
            self.conn.prepare("SELECT thread_id, id, title, created_at FROM forms WHERE closed_by IS NULL ORDER BY created_at, id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, OpenForm { id: r.get(1)?, title: r.get(2)?, at: r.get(3)? })))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The forms of a thread the user dismissed and its agent has not heard of yet: id, title.
    pub fn untold_dismissals(&self, thread_id: &str) -> Result<Vec<(String, String)>> {
        let mut st = self
            .conn
            .prepare("SELECT id, title FROM forms WHERE thread_id = ?1 AND closed_by = 'dismissed' AND told = 0 ORDER BY created_at, id")?;
        let rows = st.query_map(params![thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn mark_told(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            self.conn.execute("UPDATE forms SET told = 1 WHERE id = ?1", params![id])?;
        }
        Ok(())
    }

    // ---- events ------------------------------------------------------------------------------

    /// Cuts an oversized tool output to the inline limit and keeps the whole text as a blob.
    /// If the blob cannot be written the output stays whole in the event: nothing is dropped.
    pub fn shrink(&self, kind: EventKind) -> EventKind {
        match kind {
            EventKind::ToolResult { call_id, output, exit_code, is_error, truncated, blob } if output.len() > INLINE_LIMIT => {
                match self.blob_put(&output) {
                    Ok(hash) => EventKind::ToolResult {
                        call_id,
                        output: cut(&output, INLINE_LIMIT).to_string(),
                        exit_code,
                        is_error,
                        truncated: true,
                        blob: Some(hash),
                    },
                    Err(_) => EventKind::ToolResult { call_id, output, exit_code, is_error, truncated, blob },
                }
            }
            other => other,
        }
    }

    pub fn append(&self, e: &Event) -> Result<()> {
        let body = serde_json::to_string(&e.kind)?;
        self.conn.execute(
            "INSERT INTO events (thread_id, idx, ts, agent, kind, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.thread_id, e.index, e.timestamp, e.agent, e.kind.name(), body],
        )?;
        Ok(())
    }

    /// Events `from..=to` of a thread, in order. Indices start at 1.
    pub fn events(&self, thread_id: &str, from: u64, to: u64) -> Result<Vec<Event>> {
        let mut st =
            self.conn.prepare("SELECT idx, ts, agent, body FROM events WHERE thread_id = ?1 AND idx BETWEEN ?2 AND ?3 ORDER BY idx")?;
        let rows = st.query_map(params![thread_id, from, to], |r| {
            Ok((r.get::<_, u64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (index, timestamp, agent, body) = row?;
            let kind: EventKind = serde_json::from_str(&body).with_context(|| format!("reading event {index} of {thread_id}"))?;
            out.push(Event { index, thread_id: thread_id.to_string(), timestamp, agent, kind });
        }
        Ok(out)
    }

    pub fn all_events(&self, thread_id: &str) -> Result<Vec<Event>> {
        self.events(thread_id, 1, u64::MAX >> 1)
    }

    /// Copies events `1..=upto` of one thread into another, which must be empty. Blobs are
    /// shared by hash, so a fork costs only its rows.
    pub fn copy_events(&self, from: &str, to: &str, upto: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (thread_id, idx, ts, agent, kind, body)
             SELECT ?2, idx, ts, agent, kind, body FROM events WHERE thread_id = ?1 AND idx <= ?3",
            params![from, to, upto],
        )?;
        Ok(())
    }

    pub fn recent_prompts(&self, limit: u32) -> Result<Vec<PastPrompt>> {
        let mut st =
            self.conn.prepare("SELECT thread_id, body FROM events WHERE kind = 'user_message' ORDER BY ts DESC, idx DESC LIMIT ?1")?;
        let rows = st.query_map(params![limit], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (thread_id, body) = row?;
            if let Ok(EventKind::UserMessage { text, .. }) = serde_json::from_str(&body) {
                out.push(PastPrompt { thread_id, text });
            }
        }
        Ok(out)
    }

    // ---- queue -------------------------------------------------------------------------------

    pub fn queue_push(&self, thread_id: &str, text: &str, now: i64) -> Result<()> {
        self.conn.execute("INSERT INTO queue (thread_id, text, created_at) VALUES (?1, ?2, ?3)", params![thread_id, text, now])?;
        Ok(())
    }

    /// Removes and returns the oldest queued prompt of a thread.
    pub fn queue_pop(&self, thread_id: &str) -> Result<Option<String>> {
        let row: Option<(i64, String)> = self
            .conn
            .query_row("SELECT id, text FROM queue WHERE thread_id = ?1 ORDER BY id LIMIT 1", params![thread_id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let Some((id, text)) = row else { return Ok(None) };
        self.conn.execute("DELETE FROM queue WHERE id = ?1", params![id])?;
        Ok(Some(text))
    }

    // ---- meta --------------------------------------------------------------------------------

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute("INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = ?2", params![key, value])?;
        Ok(())
    }

    // ---- blobs -------------------------------------------------------------------------------

    pub fn blob_put(&self, text: &str) -> Result<String> {
        let hash: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        let path = self.blobs.join(&hash);
        if !path.exists() {
            let tmp = self.blobs.join(format!("{hash}.tmp"));
            std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
            std::fs::rename(&tmp, &path)?;
        }
        Ok(hash)
    }

    pub fn blob_get(&self, hash: &str) -> Result<Option<String>> {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        match std::fs::read_to_string(self.blobs.join(hash)) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ---- doctor ------------------------------------------------------------------------------

    /// `ok` when SQLite finds the file sound, else what it found.
    pub fn integrity(&self) -> Result<String> {
        Ok(self.conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?)
    }

    pub fn counts(&self) -> Result<(u64, u64)> {
        let threads = self.conn.query_row("SELECT COUNT(*) FROM threads", [], |r| r.get(0))?;
        let events = self.conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        Ok((threads, events))
    }
}

/// A form as the store keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredForm {
    pub id: String,
    pub thread_id: String,
    pub title: String,
    pub at: i64,
    /// The form's JSON.
    pub blob: String,
    /// `user`, `withdrawn` or `dismissed`; None while it is open.
    pub closed_by: Option<String>,
    /// The answer's JSON, for `user`.
    pub answer_blob: Option<String>,
    pub draft: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_ir::tool;
    use serde_json::json;

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!("desl-store-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Store::in_memory(&dir.join("blobs")).unwrap(), dir)
    }

    fn thread(id: &str) -> Thread {
        Thread {
            id: id.into(),
            project_id: "api".into(),
            name: "auth".into(),
            named: false,
            cwd: "/p/api".into(),
            worktree: None,
            parent_id: None,
            fork_point: None,
            active_agent: "claude".into(),
            status: Status::Idle,
            permission_mode: PermissionMode::Edits,
            created_at: 1,
            updated_at: 1,
            head: 0,
            bindings: BTreeMap::new(),
            queued: vec![],
            pending_approval: None,
            subagents: Vec::new(),
            open_forms: Vec::new(),
            git: GitState::default(),
        }
    }

    fn event(thread_id: &str, index: u64, kind: EventKind) -> Event {
        Event { index, thread_id: thread_id.into(), timestamp: index as i64, agent: "claude".into(), kind }
    }

    #[test]
    fn a_thread_comes_back_with_its_bindings_queue_and_head() {
        let (s, dir) = store();
        let mut t = thread("t-1");
        t.bindings.insert(
            "claude".into(),
            Binding {
                agent: "claude".into(),
                session_id: Some("s1".into()),
                seen_up_to: 2,
                model: Some("m".into()),
                model_choice: Some("sonnet".into()),
                effort_choice: Some("high".into()),
                last_used_at: 5,
                file_offset: 7,
            },
        );
        s.put_thread(&t).unwrap();
        s.append(&event("t-1", 1, EventKind::UserMessage { text: "hi".into(), attachments: vec![] })).unwrap();
        s.append(&event("t-1", 2, EventKind::AssistantMessage { text: "hello".into(), model: None })).unwrap();
        s.queue_push("t-1", "next", 9).unwrap();
        let back = s.threads().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].head, 2);
        assert_eq!(back[0].queued, vec!["next".to_string()]);
        assert_eq!(back[0].bindings["claude"].session_id.as_deref(), Some("s1"));
        assert_eq!(back[0].bindings["claude"].model_choice.as_deref(), Some("sonnet"));
        assert_eq!(back[0].bindings["claude"].effort_choice.as_deref(), Some("high"));
        assert_eq!(s.queue_pop("t-1").unwrap().as_deref(), Some("next"));
        assert_eq!(s.queue_pop("t-1").unwrap(), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn subagents_are_kept_with_their_thread() {
        let (s, dir) = store();
        let mut t = thread("t-1");
        t.subagents = vec![desl_ir::Subagent {
            id: "a1".into(),
            kind: "Explore".into(),
            description: "find auth".into(),
            finished: true,
            started_at: 5,
            agent: "claude".into(),
            session_id: "s-1".into(),
        }];
        s.put_thread(&t).unwrap();
        assert_eq!(s.threads().unwrap()[0].subagents, t.subagents);
        t.subagents.clear();
        s.put_thread(&t).unwrap();
        assert!(s.threads().unwrap()[0].subagents.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_come_back_in_order_and_an_index_is_never_reused() {
        let (s, dir) = store();
        s.put_thread(&thread("t-1")).unwrap();
        for i in 1..=3 {
            s.append(&event("t-1", i, EventKind::UserMessage { text: format!("p{i}"), attachments: vec![] })).unwrap();
        }
        assert!(s.append(&event("t-1", 2, EventKind::Interrupt)).is_err(), "an index is written once");
        let all = s.all_events("t-1").unwrap();
        assert_eq!(all.iter().map(|e| e.index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(s.events("t-1", 2, 3).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_fork_copies_the_prefix_and_nothing_after_it() {
        let (s, dir) = store();
        s.put_thread(&thread("t-1")).unwrap();
        for i in 1..=5 {
            s.append(&event("t-1", i, EventKind::UserMessage { text: format!("p{i}"), attachments: vec![] })).unwrap();
        }
        s.put_thread(&thread("t-1/1")).unwrap();
        s.copy_events("t-1", "t-1/1", 3).unwrap();
        let parent = s.events("t-1", 1, 3).unwrap();
        let fork = s.all_events("t-1/1").unwrap();
        assert_eq!(fork.len(), 3);
        for (a, b) in parent.iter().zip(&fork) {
            assert_eq!((a.index, &a.kind), (b.index, &b.kind));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_large_output_is_cut_in_the_event_and_whole_in_a_blob() {
        let (s, dir) = store();
        let big: String = (0..4000).map(|i| format!("line {i}\n")).collect();
        let kind = s.shrink(EventKind::ToolResult {
            call_id: "c".into(),
            output: big.clone(),
            exit_code: None,
            is_error: false,
            truncated: false,
            blob: None,
        });
        let EventKind::ToolResult { output, truncated, blob, .. } = kind else { panic!() };
        assert!(truncated && output.len() <= INLINE_LIMIT && big.starts_with(&output));
        assert_eq!(s.blob_get(&blob.unwrap()).unwrap().as_deref(), Some(big.as_str()));
        let small = EventKind::ToolCall { id: "c".into(), name: tool::READ.into(), args: json!({ "path": "a" }) };
        assert_eq!(s.shrink(small.clone()), small);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn recent_prompts_are_newest_first_across_threads() {
        let (s, dir) = store();
        s.append(&event("t-1", 1, EventKind::UserMessage { text: "old".into(), attachments: vec![] })).unwrap();
        s.append(&event("t-2", 2, EventKind::UserMessage { text: "new".into(), attachments: vec![] })).unwrap();
        let p = s.recent_prompts(10).unwrap();
        assert_eq!(p.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), vec!["new", "old"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_form_is_kept_until_it_closes_once() {
        let (s, dir) = store();
        s.form_put("f-1", "t-1", "Pick", 10, "ab").unwrap();
        assert!(s.form_draft("f-1", "{\"next\":\"M5\"}").unwrap());
        assert_eq!(s.open_forms().unwrap(), [("t-1".to_string(), OpenForm { id: "f-1".into(), title: "Pick".into(), at: 10 })]);
        let row = s.form("f-1").unwrap().unwrap();
        assert_eq!((row.thread_id.as_str(), row.blob.as_str(), row.draft.as_deref()), ("t-1", "ab", Some("{\"next\":\"M5\"}")));
        assert!(s.form_close("f-1", "dismissed", None).unwrap());
        assert!(!s.form_close("f-1", "user", Some("cd")).unwrap(), "closed once");
        assert!(!s.form_draft("f-1", "{}").unwrap(), "a closed form takes no draft");
        assert_eq!(s.form("f-1").unwrap().unwrap().closed_by.as_deref(), Some("dismissed"));
        assert_eq!(s.untold_dismissals("t-1").unwrap(), [("f-1".to_string(), "Pick".to_string())]);
        s.mark_told(&["f-1".to_string()]).unwrap();
        assert!(s.untold_dismissals("t-1").unwrap().is_empty());
        assert!(s.open_forms().unwrap().is_empty());
        s.form_put("f-2", "t-1", "Again", 20, "ef").unwrap();
        s.delete_thread("t-1").unwrap();
        assert!(s.form("f-2").unwrap().is_none(), "a removed thread's forms go with it");
        assert!(s.form("f-9").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Run by the next test in a process of its own, as the daemon an upgrade replaces: with
    /// `DESL_TEST_STORE=hold:<db>` it opens the store, says so and closes it when stdin closes;
    /// with `count:<db>` it says how many forms the store has.
    #[test]
    #[ignore = "a helper process of a_store_migrated_beside_the_daemon_before_it_keeps_what_it_writes"]
    fn store_in_another_process() {
        let Ok(what) = std::env::var("DESL_TEST_STORE") else { return };
        let (mode, db) = what.split_once(':').unwrap();
        let c = Connection::open(db).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        if mode == "count" {
            let n: i64 = c.query_row("SELECT count(*) FROM forms", [], |r| r.get(0)).unwrap();
            println!("forms={n}");
            return;
        }
        let _: i64 = c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0)).unwrap();
        println!("open");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        drop(c);
    }

    fn another_process(mode: &str, db: &Path) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["store::tests::store_in_another_process", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("DESL_TEST_STORE", format!("{mode}:{}", db.display()))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    /// An upgrade: the daemon before it still has the store open while the new one backs it up
    /// and migrates it, then closes it. What the new daemon writes after that must reach the
    /// file. A backup made by copying the file outside SQLite dropped this process's locks on
    /// it, so the daemon that left took itself for the last one, folded and deleted the
    /// write-ahead log, and every later write went to the deleted file.
    #[test]
    fn a_store_migrated_beside_the_daemon_before_it_keeps_what_it_writes() {
        use std::io::{BufRead, Write};
        let dir = std::env::temp_dir().join(format!("desl-store-beside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("workspace.db");
        {
            let conn = Connection::open(&db).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
                 CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, default_agent TEXT, created_at INTEGER NOT NULL) WITHOUT ROWID;
                 CREATE TABLE threads (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, cwd TEXT NOT NULL, worktree TEXT, parent_id TEXT, fork_point INTEGER, active_agent TEXT NOT NULL, status TEXT NOT NULL, permission_mode TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL) WITHOUT ROWID;
                 CREATE TABLE bindings (thread_id TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT, seen_up_to INTEGER NOT NULL, model TEXT, last_used_at INTEGER NOT NULL, PRIMARY KEY (thread_id, agent)) WITHOUT ROWID;
                 CREATE TABLE events (thread_id TEXT NOT NULL, idx INTEGER NOT NULL, ts INTEGER NOT NULL, agent TEXT NOT NULL, kind TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (thread_id, idx)) WITHOUT ROWID;
                 CREATE TABLE queue (id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, text TEXT NOT NULL, created_at INTEGER NOT NULL);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let mut before = another_process("hold", &db);
        let mut said = String::new();
        let mut out = std::io::BufReader::new(before.stdout.take().unwrap());
        while !said.contains("open") {
            said.clear();
            assert!(out.read_line(&mut said).unwrap() > 0, "the other process opened the store");
        }
        let s = Store::open(&db, &dir.join("blobs")).unwrap();
        // The daemon before it leaves.
        before.stdin.take().unwrap().write_all(b"\n").unwrap();
        before.wait().unwrap();
        s.form_put("f-1", "t-1", "Pick", 10, "ab").unwrap();
        let counted = another_process("count", &db).wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&counted.stdout);
        assert!(text.contains("forms=1"), "another process sees the write: {text}");
        assert!(dir.join("workspace.db.v1.bak").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_store_from_schema_one_is_backed_up_and_migrated_in_place() {
        let dir = std::env::temp_dir().join(format!("desl-store-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("workspace.db");
        {
            // The schema as the first build created it, with one binding in it.
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
                 CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, default_agent TEXT, created_at INTEGER NOT NULL) WITHOUT ROWID;
                 CREATE TABLE threads (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, name TEXT NOT NULL, cwd TEXT NOT NULL, worktree TEXT, parent_id TEXT, fork_point INTEGER, active_agent TEXT NOT NULL, status TEXT NOT NULL, permission_mode TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL) WITHOUT ROWID;
                 CREATE TABLE bindings (thread_id TEXT NOT NULL, agent TEXT NOT NULL, session_id TEXT, seen_up_to INTEGER NOT NULL, model TEXT, last_used_at INTEGER NOT NULL, PRIMARY KEY (thread_id, agent)) WITHOUT ROWID;
                 CREATE TABLE events (thread_id TEXT NOT NULL, idx INTEGER NOT NULL, ts INTEGER NOT NULL, agent TEXT NOT NULL, kind TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY (thread_id, idx)) WITHOUT ROWID;
                 CREATE TABLE queue (id INTEGER PRIMARY KEY AUTOINCREMENT, thread_id TEXT NOT NULL, text TEXT NOT NULL, created_at INTEGER NOT NULL);
                 INSERT INTO threads VALUES ('t-1', 'api', 'auth', '/p/api', NULL, NULL, NULL, 'claude', 'idle', 'edits', 1, 1);
                 INSERT INTO bindings VALUES ('t-1', 'claude', 's1', 0, 'claude-opus-5-5', 1);
                 INSERT INTO events VALUES ('t-1', 1, 1, 'claude', 'user_message', '{\"kind\":\"user_message\",\"text\":\"Add refresh-token rotation to the auth\",\"attachments\":[]}');
                 INSERT INTO threads VALUES ('t-2', 'api', 'Fix the flaky login test', '/p/api', NULL, NULL, NULL, 'claude', 'idle', 'edits', 2, 2);
                 INSERT INTO events VALUES ('t-2', 1, 2, 'claude', 'user_message', '{\"kind\":\"user_message\",\"text\":\"Fix the flaky login test\",\"attachments\":[]}');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let s = Store::open(&db, &dir.join("blobs")).unwrap();
        let t = &s.threads().unwrap()[0];
        assert_eq!((t.bindings["claude"].session_id.as_deref(), t.bindings["claude"].model_choice.as_deref()), (Some("s1"), None));
        assert_eq!(t.bindings["claude"].effort_choice, None);
        let named: Vec<(String, bool)> = s.threads().unwrap().into_iter().map(|t| (t.id, t.named)).collect();
        assert_eq!(named, [("t-1".to_string(), true), ("t-2".to_string(), false)], "a name that is not the first prompt's was the user's");
        assert!(dir.join("workspace.db.v1.bak").exists(), "the store as it was is kept beside the migrated one");
        assert!(s.open_forms().unwrap().is_empty(), "the forms table is there");
        let version: i64 = s.conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let _ = std::fs::remove_dir_all(dir);
    }
}
