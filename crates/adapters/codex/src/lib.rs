//! The Codex adapter: drives `codex exec --json` (and `codex exec resume <id> --json`), one
//! process per turn, and turns its stream into IR events. The only code that knows Codex's
//! flags and output. Verified against Codex CLI 0.165.0; see docs/adapters.md.

mod parse;
mod session;

use desl_ir::adapter::{
    Adapter, AdapterError, AdapterEvent, Capabilities, Detection, Launch, LaunchOpts, Open, Session, SessionReader, Sink, StartOpts, Turn,
    last_words, scrub_env, terminate_group, which,
};
use desl_ir::{Decision, ModelInfo, PermissionMode};
pub use parse::Parser;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The newest Codex CLI version with fixtures in `fixtures/`.
pub const FIXTURES_VERSION: &str = "0.165.0";

pub struct Codex;

/// Exec mode has no per-call approvals; the sandbox decides. `ask` cannot ask, so it changes
/// nothing: a read-only sandbox. `edits` is a sandbox that can write inside the workspace only.
fn sandbox_flag(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Ask => "read-only",
        PermissionMode::Edits | PermissionMode::Worktree => "workspace-write",
        PermissionMode::Yolo => "danger-full-access",
    }
}

/// In its own interface Codex asks when the sandbox would stop a call (`-a on-request`; 0.166
/// takes only `on-request` and `never`), so the sandbox decides what needs a human: in `ask`
/// nothing may write without asking, in `edits` everything inside the project may.
fn interactive_flags(mode: PermissionMode) -> Vec<String> {
    let pair = |s: &str, a: &str| vec!["-s".to_string(), s.to_string(), "-a".to_string(), a.to_string()];
    match mode {
        PermissionMode::Ask => pair("read-only", "on-request"),
        PermissionMode::Edits | PermissionMode::Worktree => pair("workspace-write", "on-request"),
        PermissionMode::Yolo => vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
    }
}

/// A top-level string from the user's own Codex config: `model`, `model_reasoning_effort`.
/// Read so the lists can mark what Codex uses when the user has not picked in desl.
fn configured(key: &str) -> Option<String> {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .map(|d| d.join("config.toml"))?;
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().take_while(|l| !l.trim_start().starts_with('[')).find_map(|l| {
        let rest = l.trim().strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string()).filter(|m| !m.is_empty())
    })
}

fn configured_model() -> Option<String> {
    configured("model")
}

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn detect(&self) -> Detection {
        let mut d = Detection {
            install_hint: "Install Codex with `npm install -g @openai/codex`, then run `codex login`.".into(),
            login_hint: "Run `codex login` to log in to Codex.".into(),
            fixtures_version: FIXTURES_VERSION.into(),
            ..Detection::default()
        };
        let Some(path) = which("codex") else { return d };
        d.installed = true;
        d.binary_path = Some(path.to_string_lossy().to_string());
        let run = |args: &[&str]| {
            Command::new(&path)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .ok()
                .map(|o| (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))))
        };
        // "codex-cli 0.165.0"
        d.version = run(&["--version"]).and_then(|(_, out)| out.split_whitespace().last().map(str::to_string));
        // "Logged in using ChatGPT", exit 0.
        d.logged_in = run(&["login", "status"]).map(|(ok, out)| ok && out.to_lowercase().contains("logged in"));
        d
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { resume: true, fork: true, model_switch: true, compact: false, interrupt: true, approvals: false, streaming: false }
    }

    fn permission_note(&self, mode: PermissionMode) -> Option<String> {
        match mode {
            PermissionMode::Ask => Some("read-only; codex asks to write only when it decides to".into()),
            PermissionMode::Edits | PermissionMode::Worktree => {
                Some("edits and commands inside this project run without asking; codex may ask to go outside it".into())
            }
            PermissionMode::Yolo => None,
        }
    }

    /// `codex debug models` prints the catalog as JSON at once; nothing is sent to a model.
    fn models(&self, cwd: &std::path::Path) -> Result<Vec<ModelInfo>, AdapterError> {
        let failed =
            |why: String| AdapterError(format!("Could not ask codex for its models ({why}). Type /model <name> to set one by name."));
        let out = Command::new("codex")
            .args(["debug", "models"])
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| failed(e.to_string()))?;
        if !out.status.success() {
            return Err(failed("`codex debug models` failed".into()));
        }
        let catalog: Value = serde_json::from_slice(&out.stdout).map_err(|_| failed("its answer was not JSON".into()))?;
        let models = parse::models(&catalog, configured_model().as_deref(), configured("model_reasoning_effort").as_deref());
        if models.is_empty() { Err(failed("it listed none".into())) } else { Ok(models) }
    }

    fn session_file(
        &self,
        cwd: &std::path::Path,
        session_id: Option<&str>,
        since: std::time::SystemTime,
    ) -> Option<(String, std::path::PathBuf)> {
        session::file(cwd, session_id, since)
    }

    fn session_reader(&self, cwd: &std::path::Path) -> Option<Box<dyn SessionReader>> {
        Some(Box::new(session::Reader::new(cwd)))
    }

    fn waiting(&self, screen_tail: &str) -> bool {
        session::waiting(screen_tail)
    }

    fn write_session(
        &self,
        cwd: &std::path::Path,
        events: &[desl_ir::EventKind],
    ) -> Result<(String, std::path::PathBuf, u64), AdapterError> {
        session::write(cwd, events).map_err(|e| AdapterError(format!("{e}, so the conversation could not be copied into codex.")))
    }

    fn launch(&self, opts: &LaunchOpts) -> Launch {
        // The shared app-server daemon would keep the thread open after the interface quits.
        let mut args: Vec<String> = vec!["--no-daemon".into()];
        args.extend(interactive_flags(opts.permission_mode));
        if let Some(model) = &opts.model {
            args.extend(["-m".into(), model.clone()]);
        }
        if let Some(effort) = &opts.effort {
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{effort}\"")]);
        }
        // Codex names a new session itself; the adapter finds it in its session files.
        let session_id = match &opts.open {
            Open::Resume { session_id } => {
                args.extend(["resume".into(), session_id.clone()]);
                Some(session_id.clone())
            }
            Open::New { .. } => None,
        };
        Launch { program: "codex".into(), args, env_remove: Vec::new(), keep: Vec::new(), session_id }
    }

    fn start(&self, opts: StartOpts, sink: Sink) -> Result<Box<dyn Session>, AdapterError> {
        Ok(Box::new(CodexSession {
            session_id: Arc::new(Mutex::new(opts.resume_session_id.clone())),
            opts,
            sink,
            child: None,
            generation: Arc::new(AtomicU64::new(0)),
        }))
    }
}

pub struct CodexSession {
    opts: StartOpts,
    sink: Sink,
    session_id: Arc<Mutex<Option<String>>>,
    child: Option<Arc<Mutex<Child>>>,
    /// Bumped on every turn and on interrupt, so a killed turn's last lines are dropped.
    generation: Arc<AtomicU64>,
}

impl CodexSession {
    fn running(&self) -> bool {
        self.child.as_ref().is_some_and(|c| c.lock().map(|mut c| matches!(c.try_wait(), Ok(None))).unwrap_or(false))
    }

    fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(c) = self.child.take()
            && let Ok(mut c) = c.lock()
        {
            terminate_group(&mut c, Duration::from_secs(5));
        }
    }
}

impl Session for CodexSession {
    fn send(&mut self, turn: Turn) -> Result<(), AdapterError> {
        if self.running() {
            return Err(AdapterError(
                "codex is still running the previous turn. Wait for it to finish or press ctrl+c, then send again.".into(),
            ));
        }
        let mut cmd = Command::new("codex");
        // `-s` is an option of `exec`. After `resume` it is refused ("unexpected argument '-s'", exit 2),
        // so it goes first; a resumed turn can then still write in the workspace.
        cmd.args(["exec", "-s", sandbox_flag(self.opts.permission_mode)]);
        if let Some(model) = &self.opts.model {
            cmd.args(["-m", model]);
        }
        // A config override, parsed as TOML, so the level is quoted. Like `-s` it goes before
        // `resume`. A level the model does not take fails the turn with a 400; desl checks the
        // level against the model's list before it gets here.
        if let Some(effort) = &self.opts.effort {
            cmd.args(["-c", &format!("model_reasoning_effort=\"{effort}\"")]);
        }
        if let Some(id) = self.session_id.lock().ok().and_then(|s| s.clone()) {
            cmd.args(["resume", &id]);
        }
        cmd.args(["--json", "--skip-git-repo-check", "-"]);
        scrub_env(&mut cmd, &[]);
        cmd.current_dir(&self.opts.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
        let mut child = cmd.spawn().map_err(|e| {
            AdapterError(format!(
                "Could not start codex ({e}). Check that Codex is installed and on your PATH, then send your prompt again."
            ))
        })?;
        let take = |what: &str| AdapterError(format!("Could not open codex's {what}. Send your prompt again to retry."));
        let mut stdin = child.stdin.take().ok_or_else(|| take("input"))?;
        let stdout = child.stdout.take().ok_or_else(|| take("output"))?;
        let stderr = child.stderr.take().ok_or_else(|| take("error output"))?;
        stdin
            .write_all(turn.payload().as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|e| AdapterError(format!("Could not send the prompt to codex ({e}). Send it again to retry.")))?;
        drop(stdin);
        let child = Arc::new(Mutex::new(child));
        self.child = Some(child.clone());

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
        let sink = self.sink.clone();
        let current = self.generation.clone();
        let session_id = self.session_id.clone();
        let cwd = self.opts.cwd.clone();
        let model = self.opts.model.clone().or_else(configured_model);
        // One process is one turn; its start time names the turn, also across daemon restarts.
        let turn_name = format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        std::thread::spawn(move || {
            let mut parser = Parser::new(&cwd, &turn_name, model);
            let mut ended = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                let events = parser.line(&value);
                if current.load(Ordering::SeqCst) != generation {
                    return;
                }
                for ev in events {
                    match &ev {
                        AdapterEvent::Session { session_id: id, .. } => {
                            if let Ok(mut s) = session_id.lock() {
                                *s = Some(id.clone());
                            }
                        }
                        AdapterEvent::TurnEnd { .. } => ended = true,
                        _ => {}
                    }
                    sink(ev);
                }
            }
            let code = child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
            std::thread::sleep(Duration::from_millis(30));
            // The process ending after its turn is how exec mode works; only an early end is news.
            if !ended && current.load(Ordering::SeqCst) == generation {
                // "Reading additional input from stdin..." is Codex's normal note when the prompt is piped.
                let tail = stderr_tail.lock().map(|t| t.clone()).unwrap_or_default();
                let tail: String = tail.lines().filter(|l| !l.contains("Reading additional input")).collect::<Vec<_>>().join("\n");
                sink(AdapterEvent::Exit { code, detail: last_words(&tail) });
            }
        });
        Ok(())
    }

    /// Exec mode has no interrupt message; stopping the process is the only way, and it ends the turn at once.
    fn interrupt(&mut self) -> bool {
        self.stop();
        true
    }

    fn answer(&mut self, _call_id: &str, _decision: Decision) -> Result<(), AdapterError> {
        Err(AdapterError("codex does not ask desl for approvals in this mode; its sandbox decides. Nothing was sent.".into()))
    }

    fn close(&mut self) {
        self.stop();
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
