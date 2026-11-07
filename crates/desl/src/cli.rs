//! The non-interactive CLI (SPEC.md "Commands, keybindings and CLI"): the same client without
//! the screen. Every command takes `--json` and exits non-zero on failure.

use crate::conn::Conn;
use anyhow::{Result, bail};
use desl_ir::{Decision, Event, EventKind, PermissionMode, Status, Thread, tool_target, tool_verb};
use desl_proto::paths::Paths;
use desl_proto::{Change, Command, Reply, ServerFrame, StreamDelta, Workspace};
use serde_json::{Value, json};
use std::io::Write;

pub struct Out {
    pub json: bool,
}

impl Out {
    fn value(&self, v: &Value) {
        println!("{v}");
    }
}

/// Connects to the daemon. With `start`, a daemon that is not running is started: every thread
/// is in the store, so any command can be answered from a cold start.
fn connect(paths: &Paths, start: bool) -> Result<Conn> {
    match Conn::open(paths, start, "cli")? {
        Some(c) => Ok(c),
        None => bail!("The desl daemon is not running. `desl` or `desl daemon start` starts it."),
    }
}

pub fn workspace(conn: &mut Conn) -> Result<Workspace> {
    match conn.call(Command::Workspace)? {
        Reply::Workspace(ws) => Ok(*ws),
        other => bail!("The desl daemon answered with {other:?}. Run `desl daemon restart`."),
    }
}

/// A thread by its id, or by its name when exactly one thread has it.
fn resolve(conn: &mut Conn, given: &str) -> Result<Thread> {
    let ws = workspace(conn)?;
    if let Some(t) = ws.threads.iter().find(|t| t.id == given) {
        return Ok(t.clone());
    }
    let named: Vec<&Thread> = ws.threads.iter().filter(|t| t.name == given).collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("There is no thread {given}. `desl status` lists the threads."),
        many => bail!(
            "{} threads are named {given}: {}. Use the id.",
            many.len(),
            many.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// What a command would do to the thread it runs in, when that is the thread it names. An agent
/// driving desl from its own terminal (SPEC.md "Agents drive desl") can wait on, prompt, switch
/// or remove any other thread; on its own, each of those would hang it or end it mid-turn.
#[derive(Clone, Copy)]
enum OwnThread {
    Send,
    Wait,
    Switch,
    Remove,
}

fn not_own(paths: &Paths, t: &Thread, what: OwnThread) -> Result<()> {
    if desl_proto::paths::own_thread().as_deref() != Some(t.id.as_str()) {
        return Ok(());
    }
    let id = &t.id;
    match what {
        OwnThread::Send => bail!(
            "{id} is the thread this command runs in, so `desl send` would wait on its own turn. Send to another thread, or `desl fork {id}` and send to the copy."
        ),
        OwnThread::Wait => bail!(
            "{id} is the thread this command runs in, so `desl wait` would wait on its own turn. Wait on a thread opened with `desl new` or `desl fork`."
        ),
        OwnThread::Switch => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let prefix = desl_proto::config::load(&paths.config, desl_proto::config::find_project(&cwd).as_deref()).prefix.label();
            bail!(
                "{id} is the thread this command runs in, and a switch would close its agent mid-turn. The user switches it with {prefix} s, or `desl fork {id} <agent>` hands a copy to the other agent."
            )
        }
        OwnThread::Remove => {
            bail!("{id} is the thread this command runs in, and removing it would end what runs here. Remove it from another terminal.")
        }
    }
}

fn thread_line(t: &Thread) -> String {
    let name = if t.name.is_empty() { "new" } else { &t.name };
    format!("{} {:<9} {:<12} {:<8} {}", t.status.glyph(), t.id, name, t.status.word(), t.active_agent)
}

pub fn status(paths: &Paths, out: &Out) -> Result<()> {
    let Some(mut conn) = Conn::open(paths, false, "cli")? else {
        if out.json {
            out.value(&json!({ "running": false, "projects": [], "threads": [] }));
        } else {
            println!("The desl daemon is not running. `desl` starts it.");
        }
        return Ok(());
    };
    let ws = workspace(&mut conn)?;
    if out.json {
        out.value(&json!({ "running": true, "projects": ws.projects, "threads": ws.threads }));
        return Ok(());
    }
    if ws.threads.is_empty() {
        println!("No threads yet. Run `desl` inside a repository to open one.");
        return Ok(());
    }
    for p in &ws.projects {
        let threads: Vec<&Thread> = ws.threads.iter().filter(|t| t.project_id == p.id).collect();
        if threads.is_empty() {
            continue;
        }
        println!("{}", p.id);
        for t in threads {
            println!("  {}", thread_line(t));
        }
    }
    let waiting: Vec<&str> = ws.threads.iter().filter(|t| t.status == Status::Waiting).map(|t| t.id.as_str()).collect();
    if !waiting.is_empty() {
        println!("waiting: {}", waiting.join(" "));
    }
    Ok(())
}

pub fn thread_list(paths: &Paths, out: &Out) -> Result<()> {
    let ws = workspace(&mut connect(paths, true)?)?;
    if out.json {
        out.value(&json!(ws.threads));
    } else {
        for t in &ws.threads {
            println!("{}  {}", thread_line(t), t.project_id);
        }
    }
    Ok(())
}

/// One event as a line of plain text.
fn event_line(e: &Event) -> Option<String> {
    Some(match &e.kind {
        EventKind::UserMessage { text, .. } => format!("› {text}"),
        EventKind::AssistantMessage { text, .. } => text.clone(),
        EventKind::ToolCall { name, args, .. } => format!("▸ {} {}", tool_verb(name), tool_target(name, args)).trim_end().to_string(),
        EventKind::AgentSwitch { to, .. } => format!("── switched to {to} ──"),
        EventKind::Fork { parent_id, .. } => format!("── forked from {parent_id} ──"),
        EventKind::Brief { token_estimate, delta, .. } => {
            format!("▸ {}brief sent to {} ~{token_estimate} tokens", if *delta { "delta " } else { "" }, e.agent)
        }
        EventKind::ApprovalRequest { summary, .. } => format!("? {summary}"),
        EventKind::ApprovalResponse { decision, by, .. } => format!("  {decision:?} by {by}").to_lowercase(),
        EventKind::Error { message, .. } => format!("✕ {message}"),
        EventKind::Interrupt => "interrupted".to_string(),
        EventKind::ToolResult { .. } | EventKind::FileChange { .. } | EventKind::Usage { .. } => return None,
    })
}

pub fn thread_show(paths: &Paths, out: &Out, thread: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    let Reply::Events { events } = conn.call(Command::Events { thread_id: t.id.clone() })? else {
        bail!("The desl daemon did not return the events.")
    };
    if out.json {
        out.value(&json!({ "thread": t, "events": events }));
        return Ok(());
    }
    println!("{} · {} · {} · {}", t.id, t.active_agent, t.status.word(), t.project_id);
    for b in t.bindings.values() {
        println!(
            "  {}: model {} · session {} · seen {} of {} events",
            b.agent,
            b.model.as_deref().unwrap_or("unknown"),
            b.session_id.as_deref().unwrap_or("not started"),
            b.seen_up_to,
            t.head
        );
    }
    for e in &events {
        if let Some(line) = event_line(e) {
            println!("\n{line}");
        }
    }
    Ok(())
}

fn done(out: &Out, v: Value, text: String) {
    if out.json { out.value(&v) } else { println!("{text}") }
}

pub fn thread_rename(paths: &Paths, out: &Out, thread: &str, name: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    conn.call(Command::Rename { thread_id: t.id.clone(), name: name.to_string() })?;
    done(out, json!({ "thread_id": t.id }), format!("Renamed {}.", t.id));
    Ok(())
}

pub fn thread_rm(paths: &Paths, out: &Out, thread: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    not_own(paths, &t, OwnThread::Remove)?;
    let note = match conn.call(Command::RemoveThread { thread_id: t.id.clone() })? {
        Reply::Removed { note } => note,
        _ => None,
    };
    let line = match &note {
        Some(n) => format!("Removed {} and its history. {n}", t.id),
        None => format!("Removed {} and its history.", t.id),
    };
    done(out, json!({ "thread_id": t.id, "worktree": note }), line);
    Ok(())
}

pub fn new(paths: &Paths, out: &Out, project: Option<String>, agent: Option<String>, prompt: Option<String>, worktree: bool) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let cwd = match project {
        // A registered project by id, else a directory.
        Some(p) => workspace(&mut conn)?.projects.iter().find(|x| x.id == p).map(|x| x.path.clone()).unwrap_or(p),
        None => std::env::current_dir()?.to_string_lossy().to_string(),
    };
    let Reply::Created { thread_id } = conn.call(Command::NewThread { cwd, agent, prompt, beside: None, worktree })? else {
        bail!("The desl daemon did not create the thread.")
    };
    done(out, json!({ "thread_id": thread_id }), thread_id.clone());
    Ok(())
}

/// Sends a prompt and streams the reply to stdout until the thread stops working.
pub fn send(paths: &Paths, out: &Out, thread: &str, prompt: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    not_own(paths, &t, OwnThread::Send)?;
    conn.call(Command::Subscribe { cwd: None })?;
    conn.call(Command::View { thread_id: t.id.clone() })?;
    let Reply::Sent { queued } = conn.call(Command::Prompt { thread_id: t.id.clone(), text: prompt.to_string() })? else {
        bail!("The desl daemon did not take the prompt.")
    };
    if queued && !out.json {
        eprintln!("{} is busy; the prompt is queued and goes out when the current turn ends.", t.active_agent);
    }
    let mut stdout = std::io::stdout();
    // True while streamed text of the current block is on screen without its newline.
    let mut streaming = false;
    // The turn has begun unless the prompt was queued; then it begins when the prompt shows up in the log.
    let mut sent = !queued;
    loop {
        let Some(frame) = conn.read()? else {
            bail!("The desl daemon closed the connection. `desl thread show {}` has what was said.", t.id)
        };
        match frame {
            ServerFrame::Stream { thread_id, delta } if thread_id == t.id && !out.json => match delta {
                StreamDelta::Start => {}
                StreamDelta::Text { text } => {
                    print!("{text}");
                    let _ = stdout.flush();
                    streaming = true;
                }
            },
            ServerFrame::Event { change: Change::Appended { event } } if event.thread_id == t.id => {
                if matches!(&event.kind, EventKind::UserMessage { text, .. } if text == prompt) {
                    sent = true;
                    continue;
                }
                if !sent {
                    continue;
                }
                if out.json {
                    out.value(&json!(event));
                    continue;
                }
                match &event.kind {
                    // The text was already streamed; the final message only ends the line.
                    EventKind::AssistantMessage { .. } if streaming => {
                        println!();
                        streaming = false;
                    }
                    _ => {
                        if let Some(line) = event_line(&event) {
                            println!("{line}");
                        }
                    }
                }
            }
            ServerFrame::Event { change: Change::Thread { thread } } if thread.id == t.id && sent => match thread.status {
                Status::Working => {}
                Status::Waiting => {
                    let what = thread.pending_approval.map(|p| p.summary);
                    if out.json {
                        out.value(&json!({ "status": "waiting", "approval": what }));
                    } else if let Some(what) = what {
                        eprintln!(
                            "{} is waiting for approval: {what}. `desl approve {}` or `desl deny {}` answers it.",
                            thread.active_agent, t.id, t.id
                        );
                    } else {
                        // The agent asks in its own interface: that is where it is answered.
                        eprintln!(
                            "{} is asking something before it goes on. Run `desl` and answer it in its own screen.",
                            thread.active_agent
                        );
                    }
                    return Ok(());
                }
                Status::Failed => bail!("{} failed. `desl thread show {}` has what it said.", thread.active_agent, t.id),
                // The turn ended, and nothing queued is about to start.
                _ if thread.queued.is_empty() => return Ok(()),
                _ => {}
            },
            ServerFrame::Event { change: Change::Bye } => bail!("The desl daemon was stopped."),
            _ => {}
        }
    }
}

pub fn switch(paths: &Paths, out: &Out, thread: &str, agent: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    not_own(paths, &t, OwnThread::Switch)?;
    conn.call(Command::Switch { thread_id: t.id.clone(), agent: agent.to_string() })?;
    done(out, json!({ "thread_id": t.id, "agent": agent }), format!("{} is on {agent}. Its next prompt carries the brief.", t.id));
    Ok(())
}

pub fn fork(paths: &Paths, out: &Out, thread: &str, agent: Option<String>, at: Option<u64>, worktree: bool) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    let Reply::Created { thread_id } = conn.call(Command::Fork { thread_id: t.id, agent, at, worktree })? else {
        bail!("The desl daemon did not create the fork.")
    };
    done(out, json!({ "thread_id": thread_id }), thread_id.clone());
    Ok(())
}

/// `desl permissions <thread>` says what the thread's agent may do without asking;
/// `desl permissions <thread> <mode>` changes it.
pub fn permissions(paths: &Paths, out: &Out, thread: &str, mode: Option<String>) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    let Some(word) = mode else {
        let Reply::Agents { agents } = conn.call(Command::Agents { refresh: false })? else {
            bail!("The desl daemon did not list the agents.")
        };
        let now = t.permission_mode;
        let meaning = agents.iter().find(|a| a.id == t.active_agent).map_or_else(|| now.meaning().to_string(), |a| a.mode_meaning(now));
        let text = format!("{} · {} · {}: {meaning}", t.id, t.active_agent, now.word());
        done(out, json!({ "thread_id": t.id, "agent": t.active_agent, "mode": now, "meaning": meaning }), text);
        return Ok(());
    };
    let Some(mode) = PermissionMode::parse(&word.trim().to_lowercase()) else {
        bail!("There is no permission mode named {word}. Pick one of ask, edits or yolo.")
    };
    let Reply::ModeSet { mode, meaning } = conn.call(Command::SetMode { thread_id: t.id.clone(), mode })? else {
        bail!("The desl daemon did not set the permissions.")
    };
    let text = format!("{} is on {}: {meaning}.", t.id, mode.word());
    done(out, json!({ "thread_id": t.id, "agent": t.active_agent, "mode": mode, "meaning": meaning }), text);
    Ok(())
}

/// `desl model <thread>` lists the models of the thread's agent with the current one marked;
/// `desl model <thread> <name>` picks one, and `--effort <level>` picks how hard it thinks.
pub fn model(paths: &Paths, out: &Out, thread: &str, name: Option<String>, effort: Option<String>) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    // The list makes a name checkable; ask for it first so a typo is caught here.
    let Reply::Models { agent, models, current, effort: effort_now } = conn.call(Command::Models { thread_id: t.id.clone() })? else {
        bail!("The desl daemon did not list the models.")
    };
    if name.is_none() && effort.is_none() {
        if out.json {
            out.value(&json!({ "thread_id": t.id, "agent": agent, "current": current, "effort": effort_now, "models": models }));
            return Ok(());
        }
        let id_w = models.iter().map(|m| m.id.len()).max().unwrap_or(0);
        let name_w = models.iter().map(|m| m.name.chars().count()).max().unwrap_or(0);
        println!("{} · {agent}", t.id);
        for m in &models {
            let mark = if Some(&m.id) == current.as_ref() { "✓" } else { " " };
            println!("{mark} {:<id_w$}  {:<name_w$}  {}", m.id, m.name, m.description);
        }
        // The effort line is about the model in use: its levels, and which one applies.
        if let Some(m) = models.iter().find(|m| Some(&m.id) == current.as_ref()).filter(|m| !m.efforts.is_empty()) {
            let levels: Vec<&str> = m.efforts.iter().map(|e| e.id.as_str()).collect();
            let now = match (&effort_now, &m.default_effort) {
                (Some(level), _) => level.clone(),
                (None, Some(level)) => format!("{agent}'s own setting, now {level}"),
                (None, None) => format!("{agent}'s own setting"),
            };
            println!("effort: {now} (levels: {})", levels.join(", "));
        }
        return Ok(());
    }
    let mut set = None;
    if let Some(name) = name {
        set = Some(conn.call(Command::SetModel { thread_id: t.id.clone(), model: Some(name) })?);
    }
    if let Some(level) = effort {
        set = Some(conn.call(Command::SetEffort { thread_id: t.id.clone(), effort: Some(level) })?);
    }
    let Some(Reply::ModelSet { agent, model, effort }) = set else { bail!("The desl daemon did not set the model.") };
    let model = model.filter(|m| !m.default);
    let text = match (&model, &effort) {
        (Some(m), Some(e)) => format!("{agent} will use {} at {e} effort in {} from its next prompt.", m.name, t.id),
        (Some(m), None) => format!("{agent} will use {} in {} from its next prompt.", m.name, t.id),
        (None, Some(e)) => format!("{agent} will use its default model at {e} effort in {} from its next prompt.", t.id),
        (None, None) => format!("{agent} is on its default model and effort in {}.", t.id),
    };
    done(out, json!({ "thread_id": t.id, "agent": agent, "model": model, "effort": effort }), text);
    Ok(())
}

pub fn brief(paths: &Paths, out: &Out, thread: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    let Reply::Brief(b) = conn.call(Command::Brief { thread_id: t.id, to: None })? else {
        bail!("The desl daemon did not return the brief.")
    };
    if out.json {
        out.value(&json!(b))
    } else {
        print!("{}", b.text)
    }
    Ok(())
}

pub fn wait(paths: &Paths, out: &Out, thread: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    not_own(paths, &t, OwnThread::Wait)?;
    let Reply::Waited { status } = conn.call(Command::Wait { thread_id: t.id.clone() })? else {
        bail!("The desl daemon did not answer the wait.")
    };
    done(out, json!({ "thread_id": t.id, "status": status }), format!("{} is {}.", t.id, status.word()));
    if status == Status::Failed {
        std::process::exit(1);
    }
    Ok(())
}

pub fn answer(paths: &Paths, out: &Out, thread: &str, decision: Decision) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    conn.call(Command::Answer { thread_id: t.id.clone(), call_id: None, decision })?;
    let word = if decision.allows() { "Allowed" } else { "Denied" };
    done(out, json!({ "thread_id": t.id, "decision": decision }), format!("{word}."));
    Ok(())
}

/// `desl log <thread>`: every approval, who answered it and when.
pub fn log(paths: &Paths, out: &Out, thread: &str) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let t = resolve(&mut conn, thread)?;
    let Reply::Events { events } = conn.call(Command::Events { thread_id: t.id.clone() })? else {
        bail!("The desl daemon did not return the events.")
    };
    let approvals: Vec<&Event> =
        events.iter().filter(|e| matches!(e.kind, EventKind::ApprovalRequest { .. } | EventKind::ApprovalResponse { .. })).collect();
    if out.json {
        out.value(&json!(approvals));
        return Ok(());
    }
    if approvals.is_empty() {
        println!("{} has asked for no approvals.", t.id);
    }
    for e in approvals {
        let at = chrono::DateTime::from_timestamp_millis(e.timestamp)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        match &e.kind {
            EventKind::ApprovalRequest { summary, risk, .. } => println!("{at}  {} asked ({risk:?}): {summary}", e.agent),
            EventKind::ApprovalResponse { decision, by, .. } => println!("{at}  {by} answered {decision:?}"),
            _ => {}
        }
    }
    Ok(())
}

pub fn agents(paths: &Paths, out: &Out) -> Result<()> {
    let mut conn = connect(paths, true)?;
    let Reply::Agents { agents } = conn.call(Command::Agents { refresh: true })? else { bail!("The desl daemon did not list the agents.") };
    if out.json {
        out.value(&json!(agents));
        return Ok(());
    }
    for a in &agents {
        let d = &a.detection;
        let state = if !d.installed {
            format!("not installed. {}", d.install_hint)
        } else {
            let login = match d.logged_in {
                Some(true) => "logged in".to_string(),
                Some(false) => format!("not logged in. {}", d.login_hint),
                None => "login unknown".to_string(),
            };
            format!("{} · {login}", d.version.as_deref().unwrap_or("unknown version"))
        };
        println!("{:<7} {state}", a.id);
    }
    Ok(())
}

pub fn doctor(paths: &Paths, out: &Out) -> Result<()> {
    let mut checks: Vec<Value> = Vec::new();
    let mut failed = false;
    let mut add = |name: &str, ok: bool, detail: String| {
        failed |= !ok;
        checks.push(json!({ "name": name, "ok": ok, "detail": detail }));
    };
    add("paths", true, format!("store {} · socket {} · log {}", paths.db.display(), paths.socket.display(), paths.log.display()));
    match Conn::open(paths, true, "cli") {
        Ok(Some(mut conn)) => match conn.call(Command::Doctor) {
            Ok(Reply::Doctor { checks: theirs }) => {
                for c in theirs {
                    add(&c.name, c.ok, c.detail);
                }
            }
            Ok(_) => add("daemon", false, "answered the doctor with something else. Run `desl daemon restart`.".into()),
            Err(e) => add("daemon", false, format!("{e:#}")),
        },
        Ok(None) => add("daemon", false, "not running and could not be started.".into()),
        Err(e) => add("daemon", false, format!("{e:#}")),
    }
    add(
        "git",
        desl_ir::adapter::which("git").is_some(),
        if desl_ir::adapter::which("git").is_some() {
            "on PATH".into()
        } else {
            "not on PATH. Install git; desl reads branches and diffs with it.".into()
        },
    );
    if out.json {
        out.value(&json!({ "ok": !failed, "checks": checks }));
    } else {
        for c in &checks {
            let mark = if c["ok"] == true { "✓" } else { "✕" };
            println!("{mark} {:<7} {}", c["name"].as_str().unwrap_or(""), c["detail"].as_str().unwrap_or(""));
        }
    }
    if failed {
        std::process::exit(1);
    }
    Ok(())
}

pub fn daemon_start(paths: &Paths, out: &Out) -> Result<()> {
    let running = Conn::open(paths, false, "cli")?.is_some();
    let conn = connect(paths, true)?;
    let text = if running {
        format!("The desl daemon is already running (pid {}).", conn.daemon_pid)
    } else {
        format!("Started the desl daemon (pid {}).", conn.daemon_pid)
    };
    done(out, json!({ "running": true, "pid": conn.daemon_pid, "started": !running }), text);
    Ok(())
}

/// Stops the daemon and waits until it has really ended: it ends its agents first (SIGTERM,
/// then up to five seconds before SIGKILL) and closes this connection last. False when none
/// was running.
pub fn stop_daemon(paths: &Paths) -> Result<bool> {
    let Some(mut conn) = Conn::open(paths, false, "cli")? else { return Ok(false) };
    conn.set_read_timeout(std::time::Duration::from_secs(20));
    conn.send(Command::Shutdown)?;
    loop {
        match conn.read() {
            // Its reply, or a last event: keep reading until the daemon hangs up.
            Ok(Some(_)) => {}
            Ok(None) => return Ok(true),
            Err(e)
                if e.root_cause()
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|io| matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)) =>
            {
                bail!("The desl daemon did not stop within twenty seconds. Its log is {}.", paths.log.display())
            }
            // A connection that broke instead of closing: the daemon is gone all the same.
            Err(_) => return Ok(true),
        }
    }
}

pub fn daemon_stop(paths: &Paths, out: &Out) -> Result<()> {
    let stopped = stop_daemon(paths)?;
    let text = if stopped { "Stopped the desl daemon and its agents. Every thread is kept." } else { "The desl daemon is not running." };
    done(out, json!({ "stopped": stopped }), text.to_string());
    Ok(())
}

pub fn daemon_restart(paths: &Paths, out: &Out) -> Result<()> {
    stop_daemon(paths)?;
    let conn = connect(paths, true)?;
    done(out, json!({ "running": true, "pid": conn.daemon_pid }), format!("Restarted the desl daemon (pid {}).", conn.daemon_pid));
    Ok(())
}

pub fn daemon_logs(paths: &Paths, lines: usize) -> Result<()> {
    let text = match std::fs::read_to_string(&paths.log) {
        Ok(t) => t,
        Err(_) => bail!("There is no daemon log yet at {}. `desl daemon start` starts the daemon.", paths.log.display()),
    };
    let all: Vec<&str> = text.lines().collect();
    for l in &all[all.len().saturating_sub(lines)..] {
        println!("{l}");
    }
    Ok(())
}

/// `desl config show`: every setting in effect in this directory, and which file set it.
pub fn config_show(paths: &Paths, out: &Out) -> Result<()> {
    use desl_proto::config;
    let cwd = std::env::current_dir()?;
    let project = config::find_project(&cwd);
    let c = config::load(&paths.config, project.as_deref());
    let source = |key: &str| c.set.iter().find(|(k, _, _)| k == key).map_or("default".to_string(), |(_, _, s)| s.clone());
    let rows = [
        ("general.prefix", c.prefix.label()),
        ("general.default_agent", c.default_agent.clone().unwrap_or_else(|| "(the first agent that is ready)".into())),
        ("permissions.default_mode", c.default_mode.clone().unwrap_or_else(|| "edits".into())),
        ("worktrees.dir", c.worktree_dir.clone()),
        ("worktrees.branch_prefix", c.branch_prefix.clone()),
    ];
    if out.json {
        out.value(&json!({
            "settings": rows.iter().map(|(k, v)| json!({ "key": k, "value": v, "source": source(k) })).collect::<Vec<_>>(),
            "user_file": paths.config,
            "project_file": project.as_deref().map(config::project_file),
            "problems": c.problems,
        }));
        return Ok(());
    }
    let w = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (k, v) in &rows {
        println!("{k:<w$}  {v:<32}  {}", source(k));
    }
    let state = |p: &std::path::Path| if p.is_file() { "" } else { " (none yet)" };
    println!();
    println!("user file     {}{}", paths.config.display(), state(&paths.config));
    match project.as_deref().map(config::project_file) {
        Some(p) => println!("project file  {}", p.display()),
        None => println!("project file  none here (a project's goes in <repo>/.desl/config.toml)"),
    }
    for p in &c.problems {
        println!("problem       {p}");
    }
    Ok(())
}

/// The skill that teaches agents to drive desl (SPEC.md "Agents drive desl"), as shipped.
const SKILL: &str = include_str!("../../../skills/desl/SKILL.md");

/// `desl skill install`: writes the skill into the skill folder of every agent that has been
/// set up here. Running it again brings the copy up to date.
pub fn skill_install(out: &Out) -> Result<()> {
    let mut installed = Vec::new();
    let mut linked = Vec::new();
    for adapter in desl_daemon::server::adapters() {
        let Some(dir) = adapter.skills_dir() else { continue };
        // An agent that was never run here has no folder of its own yet.
        if !dir.parent().is_some_and(|p| p.is_dir()) {
            continue;
        }
        let folder = dir.join("desl");
        // A link someone made, such as `npx skills add`, is theirs to update.
        if folder.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
            linked.push((adapter.id(), folder));
            continue;
        }
        let file = folder.join("SKILL.md");
        std::fs::create_dir_all(&folder)
            .and_then(|()| std::fs::write(&file, SKILL))
            .map_err(|e| anyhow::anyhow!("Could not write {} ({e}). Check that the folder is writable", file.display()))?;
        installed.push((adapter.id(), file));
    }
    if installed.is_empty() && linked.is_empty() {
        bail!("Neither Claude Code nor Codex has been set up here. Run `claude` or `codex` once, then `desl skill install` again");
    }
    let v = json!({
        "installed": installed.iter().map(|(a, f)| json!({ "agent": a, "path": f })).collect::<Vec<_>>(),
        "linked": linked.iter().map(|(a, f)| json!({ "agent": a, "path": f })).collect::<Vec<_>>(),
    });
    let mut lines: Vec<String> = installed.iter().map(|(a, f)| format!("Installed the desl skill for {a}: {}", f.display())).collect();
    lines.extend(
        linked.iter().map(|(a, f)| format!("Left {a}'s desl skill alone: {} is a link, so update the folder it points to.", f.display())),
    );
    lines.push("Agents started from now on can drive desl; ask one to fork this thread to the other agent.".into());
    done(out, v, lines.join("\n"));
    Ok(())
}
