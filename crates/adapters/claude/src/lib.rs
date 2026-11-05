//! The Claude Code adapter: drives `claude -p --output-format stream-json --input-format
//! stream-json` and turns its stream into IR events. The only code that knows Claude Code's
//! flags and output. Verified against Claude Code 2.1.312; see docs/adapters.md.

mod parse;

use desl_ir::adapter::{
    Adapter, AdapterError, AdapterEvent, Capabilities, Detection, Session, Sink, StartOpts, Turn, last_words, scrub_env, terminate_group,
    which,
};
use desl_ir::{Decision, ModelInfo, PermissionMode};
pub use parse::{DEFAULT_MODEL, NativeRequest, Parser};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The newest Claude Code version with fixtures in `fixtures/`.
pub const FIXTURES_VERSION: &str = "2.1.312";

/// Set by a parent Claude Code session for its own children. A thread's agent is its own
/// session and must not inherit another one's plumbing.
const PARENT_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
];

/// Claude Code's own login variables, kept when secrets are scrubbed from the environment.
const LOGIN_VARS: &[&str] = &["CLAUDE_CODE_OAUTH_TOKEN", "ANTHROPIC_AUTH_TOKEN"];

pub struct Claude;

/// `manual` is Claude Code's name for asking before every edit and command; reads inside the
/// project never ask. In `yolo` Claude Code still asks and desl's broker allows at once, so
/// what was allowed is in the thread's log; `bypassPermissions` would leave no trace.
fn permission_flag(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "manual",
        // Worktree isolation is the broker's job too; the agent itself runs as in `edits`.
        PermissionMode::Edits | PermissionMode::Worktree | PermissionMode::Yolo => "acceptEdits",
    }
}

impl Adapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn detect(&self) -> Detection {
        let mut d = Detection {
            install_hint: "Install Claude Code with `npm install -g @anthropic-ai/claude-code`, then run `claude` once to log in.".into(),
            login_hint: "Run `claude auth login` to log in to Claude Code.".into(),
            fixtures_version: FIXTURES_VERSION.into(),
            ..Detection::default()
        };
        let Some(path) = which("claude") else { return d };
        d.installed = true;
        d.binary_path = Some(path.to_string_lossy().to_string());
        let run = |args: &[&str]| {
            let mut cmd = Command::new(&path);
            cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
            for var in PARENT_SESSION_VARS {
                cmd.env_remove(var);
            }
            cmd.output().ok().map(|o| (o.status.success(), String::from_utf8_lossy(&o.stdout).to_string()))
        };
        // "2.1.312 (Claude Code)"
        d.version = run(&["--version"]).and_then(|(_, out)| out.split_whitespace().next().map(str::to_string));
        d.logged_in = run(&["auth", "status", "--json"])
            .and_then(|(_, out)| serde_json::from_str::<Value>(&out).ok())
            .and_then(|v| v.get("loggedIn").and_then(Value::as_bool));
        d
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { resume: true, fork: true, model_switch: true, compact: true, interrupt: true, approvals: true, streaming: true }
    }

    /// Starts Claude Code, asks it to describe itself with an `initialize` control request, and
    /// stops it again. No prompt is sent and nothing is billed; it answers in a fraction of a second.
    fn models(&self, cwd: &std::path::Path) -> Result<Vec<ModelInfo>, AdapterError> {
        let failed =
            |why: String| AdapterError(format!("Could not ask claude for its models ({why}). Type /model <name> to set one by name."));
        let mut cmd = Command::new("claude");
        cmd.args(["-p", "--output-format", "stream-json", "--input-format", "stream-json", "--verbose"]);
        for var in PARENT_SESSION_VARS {
            cmd.env_remove(var);
        }
        scrub_env(&mut cmd, LOGIN_VARS);
        cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0);
        let mut child = cmd.spawn().map_err(|e| failed(e.to_string()))?;
        let (mut stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(i), Some(o)) => (i, o),
            _ => return Err(failed("its pipes did not open".into())),
        };
        let request = json!({ "type": "control_request", "request_id": "desl-models", "request": { "subtype": "initialize" } });
        let asked = stdin.write_all(format!("{request}\n").as_bytes()).and_then(|()| stdin.flush());
        let (tx, rx) = std::sync::mpsc::channel::<Vec<ModelInfo>>();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(models) = serde_json::from_str::<Value>(&line).ok().and_then(|v| parse::models(&v)) {
                    let _ = tx.send(models);
                    break;
                }
            }
        });
        let answer = match asked {
            Ok(()) => rx.recv_timeout(Duration::from_secs(15)).map_err(|_| failed("it did not answer within fifteen seconds".into())),
            Err(e) => Err(failed(e.to_string())),
        };
        drop(stdin);
        terminate_group(&mut child, Duration::from_secs(2));
        answer.and_then(|models| if models.is_empty() { Err(failed("it listed none".into())) } else { Ok(models) })
    }

    fn start(&self, opts: StartOpts, sink: Sink) -> Result<Box<dyn Session>, AdapterError> {
        Ok(Box::new(ClaudeSession {
            session_id: Arc::new(Mutex::new(opts.resume_session_id.clone())),
            opts,
            sink,
            proc: None,
            requests: Arc::new(Mutex::new(Default::default())),
            generation: Arc::new(AtomicU64::new(0)),
            seq: 0,
        }))
    }
}

struct Proc {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
}

pub struct ClaudeSession {
    opts: StartOpts,
    sink: Sink,
    proc: Option<Proc>,
    /// The native session id, learned from the stream; a respawn resumes it.
    session_id: Arc<Mutex<Option<String>>>,
    requests: Arc<Mutex<std::collections::HashMap<String, NativeRequest>>>,
    /// Bumped on every spawn, so a dying process's last lines never reach a newer one's turn.
    generation: Arc<AtomicU64>,
    seq: u64,
}

impl ClaudeSession {
    fn alive(&mut self) -> bool {
        match &self.proc {
            Some(p) => p.child.lock().map(|mut c| matches!(c.try_wait(), Ok(None))).unwrap_or(false),
            None => false,
        }
    }

    fn spawn(&mut self) -> Result<(), AdapterError> {
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            permission_flag(self.opts.permission_mode),
        ]);
        if let Some(id) = self.session_id.lock().ok().and_then(|s| s.clone()) {
            cmd.args(["--resume", &id]);
        }
        // "default" is Claude Code's own name for not choosing; it needs no flag.
        if let Some(model) = self.opts.model.as_deref().filter(|m| *m != DEFAULT_MODEL) {
            cmd.args(["--model", model]);
        }
        // An unknown level is only a warning to Claude Code, which then uses its default; desl
        // checks the level against the model's list before it gets here.
        if let Some(effort) = &self.opts.effort {
            cmd.args(["--effort", effort]);
        }
        for var in PARENT_SESSION_VARS {
            cmd.env_remove(var);
        }
        scrub_env(&mut cmd, LOGIN_VARS);
        cmd.current_dir(&self.opts.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            AdapterError(format!(
                "Could not start claude ({e}). Check that Claude Code is installed and on your PATH, then send your prompt again."
            ))
        })?;
        let take = |what: &str| AdapterError(format!("Could not open claude's {what}. Send your prompt again to retry."));
        let stdin = child.stdin.take().ok_or_else(|| take("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| take("output"))?;
        let stderr = child.stderr.take().ok_or_else(|| take("error output"))?;
        let child = Arc::new(Mutex::new(child));

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let stderr_tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        {
            let tail = stderr_tail.clone();
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = BufReader::new(stderr).read_to_string(&mut buf);
                let mut keep = buf.len().saturating_sub(2000);
                while !buf.is_char_boundary(keep) {
                    keep += 1;
                }
                if let Ok(mut t) = tail.lock() {
                    *t = buf[keep..].to_string();
                }
            });
        }
        {
            let child = child.clone();
            let sink = self.sink.clone();
            let current = self.generation.clone();
            let session_id = self.session_id.clone();
            let requests = self.requests.clone();
            let cwd = self.opts.cwd.clone();
            std::thread::spawn(move || {
                let mut parser = Parser::new(&cwd);
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                    let events = parser.line(&value);
                    if current.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Ok(mut r) = requests.lock() {
                        *r = parser.requests.clone();
                    }
                    for ev in events {
                        if let AdapterEvent::Session { session_id: id, .. } = &ev
                            && let Ok(mut s) = session_id.lock()
                        {
                            *s = Some(id.clone());
                        }
                        sink(ev);
                    }
                }
                let code = child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
                // Give the stderr reader a moment to finish; it ends when the process does.
                std::thread::sleep(Duration::from_millis(30));
                let detail = stderr_tail.lock().map(|t| last_words(&t)).unwrap_or_default();
                if current.load(Ordering::SeqCst) == generation {
                    sink(AdapterEvent::Exit { code, detail });
                }
            });
        }
        self.proc = Some(Proc { child, stdin });
        Ok(())
    }

    fn write(&mut self, value: &Value) -> Result<(), AdapterError> {
        let gone = || AdapterError("claude is not running any more. Send your prompt again to resume the session.".into());
        let p = self.proc.as_mut().ok_or_else(gone)?;
        let mut line = value.to_string();
        line.push('\n');
        p.stdin.write_all(line.as_bytes()).and_then(|()| p.stdin.flush()).map_err(|_| gone())
    }
}

impl Session for ClaudeSession {
    fn send(&mut self, turn: Turn) -> Result<(), AdapterError> {
        if !self.alive() {
            self.spawn()?;
        }
        let message = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": turn.payload() }] }
        });
        self.write(&message)
    }

    fn interrupt(&mut self) -> bool {
        self.seq += 1;
        let request = json!({
            "type": "control_request",
            "request_id": format!("desl-{}", self.seq),
            "request": { "subtype": "interrupt" }
        });
        // Claude Code answers with a `result`, which ends the turn. If it is already gone, the turn is over now.
        self.write(&request).is_err()
    }

    fn answer(&mut self, call_id: &str, decision: Decision) -> Result<(), AdapterError> {
        let native = self
            .requests
            .lock()
            .ok()
            .and_then(|r| r.get(call_id).cloned())
            .ok_or_else(|| AdapterError("claude is no longer waiting for that answer. Nothing was sent.".into()))?;
        let response = if decision.allows() {
            json!({ "behavior": "allow", "updatedInput": native.input })
        } else {
            json!({ "behavior": "deny", "message": "The user denied this in desl." })
        };
        self.write(&json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": native.request_id, "response": response }
        }))
    }

    fn close(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(p) = self.proc.take() {
            drop(p.stdin);
            if let Ok(mut c) = p.child.lock() {
                terminate_group(&mut c, Duration::from_secs(5));
            }
        }
    }
}

impl Drop for ClaudeSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
