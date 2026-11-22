//! `desl mcp`: the MCP server each agent desl starts is given (docs/design/2026-11-21-web-forms.md).
//! It speaks MCP over stdio, one JSON-RPC object a line, and offers `show_form` and `close_form`,
//! and the project's task list as `list_tasks`, `add_task` and `update_task`
//! (docs/design/2026-11-22-tasks-first-class.md), which it turns into daemon commands for its
//! own thread.

use anyhow::{Context, Result};
use desl_proto::paths::Paths;
use desl_proto::{Command, Reply, form};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Serves the agent on stdin and stdout until it closes them, sending each tool call to the
/// daemon on `socket` for `thread`.
pub fn run(socket: &Path, thread: &str) -> Result<()> {
    let paths = Paths { socket: socket.to_path_buf(), ..Paths::resolve() };
    let mut conn = None;
    let mut call = |command: Command| -> Result<Reply, String> {
        // Connected at the first call, and again after a daemon restart closed the connection.
        for _ in 0..2 {
            if conn.is_none() {
                conn = crate::conn::Conn::open(&paths, false, "mcp").map_err(|e| format!("{e:#}"))?;
            }
            let Some(c) = conn.as_mut() else {
                return Err("The desl daemon is not running, so the form cannot be shown. Tell the user to run desl.".to_string());
            };
            match c.call(command.clone()) {
                Ok(reply) => return Ok(reply),
                // The daemon went away mid-call: connect again, once.
                Err(e) if e.downcast_ref::<crate::conn::Closed>().is_some() || e.downcast_ref::<io::Error>().is_some() => conn = None,
                // A refusal is a sentence for the agent.
                Err(e) => return Err(e.to_string()),
            }
        }
        Err("desl lost the daemon twice while showing the form. Try again in a moment.".to_string())
    };
    serve(io::stdin().lock(), io::stdout().lock(), &mut call, thread).context("desl mcp stopped")
}

/// The MCP loop, over any reader and writer: one JSON-RPC object a line each way.
pub fn serve(
    input: impl BufRead,
    mut output: impl Write,
    call: &mut dyn FnMut(Command) -> Result<Reply, String>,
    thread: &str,
) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            write_line(&mut output, &json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Not JSON." } }))?;
            continue;
        };
        // A notification, such as notifications/initialized, gets no answer.
        let Some(id) = msg.get("id").cloned() else { continue };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let answer = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "desl", "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": [
                { "name": "show_form", "description": form::TOOL_DESCRIPTION, "inputSchema": form::input_schema() },
                { "name": "close_form", "description": form::CLOSE_DESCRIPTION, "inputSchema": form::close_schema() },
                { "name": "list_tasks", "description": LIST_TASKS, "inputSchema": list_schema() },
                { "name": "add_task", "description": ADD_TASK, "inputSchema": add_schema() },
                { "name": "update_task", "description": UPDATE_TASK, "inputSchema": update_schema() },
            ] })),
            "tools/call" => Ok(tool(&params, call, thread)),
            other => Err(json!({ "code": -32601, "message": format!("desl mcp has no method {other}.") })),
        };
        let reply = match answer {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        write_line(&mut output, &reply)?;
    }
    Ok(())
}

/// A tool's result: a sentence for the agent, marked as an error when it has something to mend.
fn tool(params: &Value, call: &mut dyn FnMut(Command) -> Result<Reply, String>, thread: &str) -> Value {
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let outcome = match params.get("name").and_then(Value::as_str).unwrap_or("") {
        "show_form" => call(Command::FormNew { thread_id: thread.to_string(), form: args }).map(|r| match r {
            Reply::FormShown { form_id } => form::shown(&form_id),
            other => format!("The desl daemon answered with {other:?}."),
        }),
        "close_form" => match args.get("form_id").and_then(Value::as_str) {
            Some(id) => withdraw(id, call, thread),
            None => Err("close_form needs the form_id that show_form gave.".to_string()),
        },
        "list_tasks" => list_tasks(&args, call, thread),
        "add_task" => add_task(&args, call, thread),
        "update_task" => update_task(&args, call, thread),
        other => Err(format!("desl has no tool {other}: it has show_form, close_form, list_tasks, add_task and update_task.")),
    };
    let (text, error) = match outcome {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": error })
}

const LIST_TASKS: &str = "This project's task list (.tasks.md, kept by desl): the work to do, in progress, ready to \
merge or parked for later. Returns the open tasks, or those of one status.";
const ADD_TASK: &str = "Add a task to this project's list. Record work you find on the way rather than doing it \
unasked, and ideas: when the user says not now, later or park it, add it with status parked.";
const UPDATE_TASK: &str = "Change a task as its work moves: in-progress; ready when it is done on a branch of its own; \
done; issue with a note saying why when stuck; parked. Or change its title or priority, or add a note.";

const STATUSES: [&str; 7] = ["todo", "in-progress", "issue", "ready", "done", "ignored", "parked"];
const KINDS: [&str; 7] = ["feature", "bug", "task", "refactor", "test", "docs", "idea"];
const PRIORITIES: [&str; 3] = ["high", "medium", "low"];

fn list_schema() -> Value {
    let mut statuses: Vec<&str> = STATUSES.to_vec();
    statuses.push("all");
    json!({ "type": "object", "properties": { "status": { "type": "string", "enum": statuses } } })
}

fn add_schema() -> Value {
    json!({ "type": "object", "properties": {
        "title": { "type": "string" }, "type": { "type": "string", "enum": KINDS },
        "priority": { "type": "string", "enum": PRIORITIES }, "status": { "type": "string", "enum": ["todo", "parked"] },
        "body": { "type": "string", "description": "Markdown: what and why." } }, "required": ["title"] })
}

fn update_schema() -> Value {
    json!({ "type": "object", "properties": {
        "id": { "type": "integer" }, "status": { "type": "string", "enum": STATUSES }, "title": { "type": "string" },
        "priority": { "type": "string", "enum": PRIORITIES }, "note": { "type": "string", "description": "A paragraph added under its description." } },
        "required": ["id"] })
}

/// A field that names one of `known`, parsed; a sentence naming them when it is not one.
fn named<T>(args: &Value, key: &str, known: &[&str], parse: fn(&str) -> Option<T>) -> Result<Option<T>, String> {
    match args.get(key).and_then(Value::as_str) {
        None => Ok(None),
        Some(s) => parse(s).map(Some).ok_or_else(|| format!("{key} \"{s}\" is not one desl knows: {}.", known.join(", "))),
    }
}

fn one_line(t: &desl_proto::tasks::Task) -> String {
    let mut s = format!("#{} [{}] ({}, {}) {}", t.id, t.status.id(), t.kind.id(), t.priority.id(), t.title);
    if let Some(th) = &t.thread {
        s.push_str(&format!(" · {th}"));
    }
    if let Some(b) = &t.branch {
        s.push_str(&format!(" · {b}"));
    }
    s
}

fn list_tasks(args: &Value, call: &mut dyn FnMut(Command) -> Result<Reply, String>, thread: &str) -> Result<String, String> {
    use desl_proto::tasks::Status;
    let want = args.get("status").and_then(Value::as_str).unwrap_or("");
    let status = if want.is_empty() || want == "all" { None } else { named(args, "status", &STATUSES, Status::parse)? };
    let Reply::Tasks { tasks, .. } = call(Command::Tasks { project_id: thread.to_string() })? else {
        return Err("The desl daemon did not send the list.".to_string());
    };
    let shown: Vec<String> = tasks
        .iter()
        .filter(|t| match status {
            Some(s) => t.status == s,
            None if want == "all" => true,
            None => t.status.open(),
        })
        .map(one_line)
        .collect();
    Ok(if shown.is_empty() { "No task there.".to_string() } else { shown.join("\n") })
}

fn add_task(args: &Value, call: &mut dyn FnMut(Command) -> Result<Reply, String>, thread: &str) -> Result<String, String> {
    use desl_proto::tasks::{Kind, Priority, Status};
    let title = args.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty());
    let Some(title) = title else { return Err("add_task needs a title: one line saying what.".to_string()) };
    let add = Command::TaskAdd {
        project_id: thread.to_string(),
        title: title.to_string(),
        kind: named(args, "type", &KINDS, Kind::parse)?.unwrap_or(Kind::Task),
        status: named(args, "status", &STATUSES, Status::parse)?.unwrap_or(Status::Todo),
        priority: named(args, "priority", &PRIORITIES, Priority::parse)?.unwrap_or(Priority::Medium),
        body: args.get("body").and_then(Value::as_str).unwrap_or("").to_string(),
    };
    match call(add)? {
        Reply::Task { task, .. } => Ok(format!("Added {}", one_line(&task))),
        other => Err(format!("The desl daemon answered with {other:?}.")),
    }
}

fn update_task(args: &Value, call: &mut dyn FnMut(Command) -> Result<Reply, String>, thread: &str) -> Result<String, String> {
    use desl_proto::tasks::{Edit, Priority, Status};
    let Some(id) = args.get("id").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()) else {
        return Err("update_task needs the task's id, its number: list_tasks gives it.".to_string());
    };
    let text = |k: &str| args.get(k).and_then(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty());
    let edit = Edit {
        status: named(args, "status", &STATUSES, Status::parse)?,
        priority: named(args, "priority", &PRIORITIES, Priority::parse)?,
        title: text("title"),
        note: text("note"),
        ..Default::default()
    };
    if edit == Edit::default() {
        return Err("Nothing to change: give a status, a title, a priority or a note.".to_string());
    }
    match call(Command::TaskSet { project_id: thread.to_string(), task: id, edit })? {
        Reply::Task { task, .. } => Ok(format!("Now {}", one_line(&task))),
        other => Err(format!("The desl daemon answered with {other:?}.")),
    }
}

/// Withdraws a form of this agent's own thread only: a fork's agent knows its parent's forms
/// from the copied conversation, and they are not its to close.
fn withdraw(id: &str, call: &mut dyn FnMut(Command) -> Result<Reply, String>, thread: &str) -> Result<String, String> {
    match call(Command::FormGet { form_id: id.to_string() })? {
        Reply::Form { thread_id, .. } if thread_id == thread => {}
        Reply::Form { thread_id, .. } => {
            return Err(format!("{id} belongs to another thread, {thread_id}; you can withdraw only the forms you showed."));
        }
        other => return Err(format!("The desl daemon answered with {other:?}.")),
    }
    call(Command::FormClose { form_id: id.to_string(), by: "withdrawn".into() }).map(|_| format!("{id} is withdrawn."))
}

fn write_line(out: &mut impl Write, v: &Value) -> io::Result<()> {
    writeln!(out, "{v}")?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_proto::{Command, Reply, form};
    use serde_json::{Value, json};

    fn run(input: &[Value], mut call: impl FnMut(Command) -> Result<Reply, String>) -> Vec<Value> {
        let text: String = input.iter().map(|v| v.to_string() + "\n").collect();
        let mut out = Vec::new();
        serve(text.as_bytes(), &mut out, &mut call, "t-7f3a").unwrap();
        String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    #[test]
    fn the_server_lists_its_tools_and_shows_a_form() {
        let mut asked = Vec::new();
        let replies = run(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
                json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "show_form", "arguments": {"title": "T", "widgets": []}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "ping"}),
            ],
            |c| {
                asked.push(c);
                Ok(Reply::FormShown { form_id: "f-4b1e".into() })
            },
        );
        assert_eq!(replies.len(), 5, "a notification gets no reply");
        assert_eq!(replies[0]["result"]["serverInfo"]["name"], "desl");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18", "the client's version, echoed");
        assert!(replies[0]["result"]["capabilities"]["tools"].is_object());
        let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["show_form", "close_form", "list_tasks", "add_task", "update_task"]);
        assert_eq!(replies[1]["result"]["tools"][0]["inputSchema"], form::input_schema());
        assert_eq!(replies[2]["result"]["content"][0]["text"], form::shown("f-4b1e"));
        assert_eq!(replies[2]["result"]["isError"], false);
        assert!(matches!(&asked[0], Command::FormNew { thread_id, form } if thread_id == "t-7f3a" && form["title"] == "T"));
        assert_eq!(replies[3]["error"]["code"], -32601);
        assert_eq!(replies[4]["result"], json!({}));
    }

    #[test]
    fn the_task_tools_read_add_and_change_this_projects_list() {
        use desl_proto::tasks::{Kind, Priority, Status, Task};
        let mut asked = Vec::new();
        let task = |id: u32, status: Status| Task { status, ..Task::new(id, "Forms in the terminal", Kind::Idea) };
        let replies = run(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "add_task", "arguments": {"title": "Forms in the terminal", "type": "idea", "status": "parked", "priority": "low"}}}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "update_task", "arguments": {"id": 4, "status": "ready", "note": "On desl/t-1."}}}),
                json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "list_tasks", "arguments": {}}}),
                json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "update_task", "arguments": {"id": 4, "status": "sideways"}}}),
            ],
            |c| {
                let reply = match &c {
                    Command::TaskAdd { .. } => Reply::Task { project_id: "p".into(), task: task(4, Status::Parked) },
                    Command::TaskSet { .. } => Reply::Task { project_id: "p".into(), task: task(4, Status::Ready) },
                    _ => Reply::Tasks { project_id: "p".into(), tasks: vec![task(4, Status::Ready)] },
                };
                asked.push(c);
                Ok(reply)
            },
        );
        let names: Vec<&str> = replies[0]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["show_form", "close_form", "list_tasks", "add_task", "update_task"]);
        assert!(
            matches!(&asked[0], Command::TaskAdd { project_id, kind: Kind::Idea, status: Status::Parked, priority: Priority::Low, .. } if project_id == "t-7f3a")
        );
        assert!(replies[1]["result"]["content"][0]["text"].as_str().unwrap().contains("#4"), "{}", replies[1]);
        assert!(
            matches!(&asked[1], Command::TaskSet { task: 4, edit, .. } if edit.status == Some(Status::Ready) && edit.note.as_deref() == Some("On desl/t-1."))
        );
        let listed = replies[3]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(listed.contains("#4") && listed.contains("ready") && listed.contains("Forms in the terminal"), "{listed}");
        assert_eq!(replies[4]["result"]["isError"], true, "a status desl does not know is the agent's to mend");
        assert_eq!(asked.len(), 3, "nothing sent for it");
    }

    #[test]
    fn close_withdraws_only_a_form_of_its_own_thread() {
        let mut asked = Vec::new();
        let replies = run(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "close_form", "arguments": {"form_id": "f-4b1e"}}}),
            ],
            |c| {
                asked.push(c);
                // A fork's agent knows its parent's form from the copied conversation.
                Ok(Reply::Form {
                    form_id: "f-4b1e".into(),
                    thread_id: "t-0000".into(),
                    form: json!({}),
                    draft: None,
                    answer: None,
                    closed_by: None,
                })
            },
        );
        assert_eq!(replies[0]["result"]["isError"], true);
        assert!(replies[0]["result"]["content"][0]["text"].as_str().unwrap().contains("another thread"), "{}", replies[0]);
        assert!(!asked.iter().any(|c| matches!(c, Command::FormClose { .. })), "nothing closed: {asked:?}");
    }

    #[test]
    fn a_refusal_goes_back_to_the_agent_to_mend_and_close_withdraws() {
        let mut asked = Vec::new();
        let replies = run(
            &[
                json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "show_form", "arguments": {"title": ""}}}),
                json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "close_form", "arguments": {"form_id": "f-4b1e"}}}),
                json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "rm", "arguments": {}}}),
            ],
            |c| {
                let reply = match &c {
                    Command::FormNew { .. } => Err("The form has no title. Give it one.".into()),
                    Command::FormGet { form_id } => Ok(Reply::Form {
                        form_id: form_id.clone(),
                        thread_id: "t-7f3a".into(),
                        form: json!({}),
                        draft: None,
                        answer: None,
                        closed_by: None,
                    }),
                    _ => Ok(Reply::Done),
                };
                asked.push(c);
                reply
            },
        );
        assert_eq!(replies[0]["result"]["isError"], true);
        assert_eq!(replies[0]["result"]["content"][0]["text"], "The form has no title. Give it one.");
        assert!(matches!(&asked[2], Command::FormClose { form_id, by } if form_id == "f-4b1e" && by == "withdrawn"));
        assert_eq!(replies[1]["result"]["isError"], false);
        assert_eq!(replies[2]["result"]["isError"], true, "an unknown tool is the agent's to hear about");
    }
}
