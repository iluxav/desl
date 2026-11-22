//! The GitHub Copilot CLI adapter. Copilot runs in its own interface (the native route); this
//! crate is the only code that knows its flags, its session files and its dialogs. Verified
//! against GitHub Copilot CLI 1.0.112; see docs/adapters.md.

mod models;
mod session;

use desl_ir::adapter::{
    Adapter, AdapterError, Capabilities, Detection, Launch, LaunchOpts, Open, Session, SessionReader, Sink, StartOpts, new_uuid, which,
};
use desl_ir::{ModelInfo, PermissionMode};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The newest Copilot CLI version this adapter was verified against.
pub const FIXTURES_VERSION: &str = "1.0.112";

/// The variables Copilot logs in through (`copilot help environment`), kept despite the
/// secrets denylist.
const LOGIN_VARS: &[&str] = &["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"];

pub struct Copilot;

/// Copilot's own folder: `$COPILOT_HOME`, or `~/.copilot` when it is unset.
fn copilot_home() -> Option<PathBuf> {
    std::env::var_os("COPILOT_HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".copilot")))
}

/// Copilot asks before every edit and every command that is not read-only. `edits` allows its
/// file tools, `worktree` every tool inside the directory, `yolo` everything.
fn permission_flags(mode: PermissionMode) -> Vec<String> {
    match mode {
        PermissionMode::Ask => Vec::new(),
        PermissionMode::Edits => vec!["--allow-tool=write".to_string()],
        PermissionMode::Worktree => vec!["--allow-all-tools".to_string()],
        PermissionMode::Yolo => vec!["--allow-all".to_string()],
    }
}

impl Adapter for Copilot {
    fn id(&self) -> &'static str {
        "copilot"
    }

    fn detect(&self) -> Detection {
        let mut d = Detection {
            install_hint: "Install GitHub Copilot CLI with `npm install -g @github/copilot`, then run `copilot login`.".into(),
            login_hint: "Run `copilot login` to log in to GitHub Copilot.".into(),
            fixtures_version: FIXTURES_VERSION.into(),
            ..Detection::default()
        };
        let Some(path) = which("copilot") else { return d };
        d.installed = true;
        d.binary_path = Some(path.to_string_lossy().to_string());
        // "GitHub Copilot CLI 1.0.112."
        d.version = Command::new(&path).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok().and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .find(|w| w.starts_with(|c: char| c.is_ascii_digit()))
                .map(|w| w.trim_end_matches('.').to_string())
        });
        // Copilot has no command that says whether it is logged in: it may be through its own
        // login or a token in the environment. Its interface asks when it is not.
        d.logged_in = None;
        d
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { resume: true, fork: true, model_switch: true, compact: false, interrupt: true, approvals: false, streaming: false }
    }

    fn permission_note(&self, mode: PermissionMode) -> Option<String> {
        match mode {
            PermissionMode::Ask => Some("every edit asks first, and every command copilot does not know to be read-only".into()),
            _ => None,
        }
    }

    fn models(&self, cwd: &Path) -> Result<Vec<ModelInfo>, AdapterError> {
        models::list(cwd, LOGIN_VARS).map_err(|why| {
            AdapterError(format!("Could not ask copilot for its models ({why}). Type /model in its own interface to pick one."))
        })
    }

    fn session_file(&self, cwd: &Path, session_id: Option<&str>, since: std::time::SystemTime) -> Option<(String, PathBuf)> {
        session::file_in(&copilot_home()?, cwd, session_id, since)
    }

    fn session_reader(&self, cwd: &Path) -> Option<Box<dyn SessionReader>> {
        Some(Box::new(session::Reader::new(cwd)))
    }

    fn write_session(&self, cwd: &Path, events: &[desl_ir::EventKind]) -> Result<(String, PathBuf, u64), AdapterError> {
        let home =
            copilot_home().ok_or_else(|| AdapterError("HOME is not set, so the conversation could not be copied into copilot.".into()))?;
        session::write_in(&home, cwd, events)
            .map_err(|e| AdapterError(format!("{e}, so the conversation could not be copied into copilot.")))
    }

    fn waiting(&self, screen_tail: &str) -> bool {
        session::waiting(screen_tail)
    }

    /// Copilot names a session in its `workspace.yaml`, not in its event log (docs/adapters.md).
    fn session_title(&self, session_id: &str) -> Option<String> {
        session::title_in(&copilot_home()?, session_id)
    }

    /// `ctrl+c` stops Copilot's turn at once; `esc` only does on the second press.
    fn interrupt_key(&self) -> Option<&'static [u8]> {
        Some(b"\x03")
    }

    /// Copilot's personal skills: `$COPILOT_HOME/skills/<name>/SKILL.md`.
    fn skills_dir(&self) -> Option<PathBuf> {
        copilot_home().map(|h| h.join("skills"))
    }

    /// `--session-id` both names a new session and opens an existing one, so desl always knows
    /// the session's id, as it does Claude's.
    fn launch(&self, opts: &LaunchOpts) -> Launch {
        let mut args = permission_flags(opts.permission_mode);
        if let Some(model) = &opts.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &opts.effort {
            args.extend(["--reasoning-effort".into(), effort.clone()]);
        }
        if let Some(m) = &opts.mcp {
            let config =
                serde_json::json!({ "mcpServers": { "desl": { "type": "local", "command": m.command, "args": m.args, "tools": ["*"] } } });
            args.extend(["--additional-mcp-config".into(), config.to_string(), "--allow-tool=desl".into()]);
        }
        let session_id = match &opts.open {
            Open::Resume { session_id } => session_id.clone(),
            Open::New { session_id } => session_id.clone().unwrap_or_else(new_uuid),
        };
        args.push(format!("--session-id={session_id}"));
        Launch {
            program: "copilot".into(),
            args,
            env_remove: Vec::new(),
            keep: LOGIN_VARS.iter().map(|v| (*v).to_string()).collect(),
            session_id: Some(session_id),
        }
    }

    /// desl runs Copilot in its own interface only; there is no headless turn to start.
    fn start(&self, _opts: StartOpts, _sink: Sink) -> Result<Box<dyn Session>, AdapterError> {
        Err(AdapterError("copilot runs in its own interface only. Open the thread in desl and send the prompt again.".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(open: Open, mode: PermissionMode) -> Launch {
        Copilot.launch(&LaunchOpts {
            cwd: "/w".into(),
            open,
            model: Some("gpt-6-luna".into()),
            effort: Some("low".into()),
            permission_mode: mode,
            mcp: None,
        })
    }

    #[test]
    fn desls_form_tool_comes_with_the_launch_and_is_allowed() {
        use desl_ir::adapter::McpServer;
        let l = Copilot.launch(&LaunchOpts {
            cwd: "/w".into(),
            open: Open::Resume { session_id: "s1".into() },
            model: None,
            effort: None,
            permission_mode: PermissionMode::Edits,
            mcp: Some(McpServer {
                command: "/bin/desl".into(),
                args: ["mcp", "--thread", "t-1", "--socket", "/run/d.sock"].map(String::from).to_vec(),
            }),
        });
        let at = l.args.iter().position(|a| a == "--additional-mcp-config").expect("the flag");
        let cfg: serde_json::Value = serde_json::from_str(&l.args[at + 1]).unwrap();
        assert_eq!(
            cfg,
            serde_json::json!({"mcpServers": {"desl": {"type": "local", "command": "/bin/desl", "args": ["mcp", "--thread", "t-1", "--socket", "/run/d.sock"], "tools": ["*"]}}})
        );
        assert!(l.args.iter().any(|a| a == "--allow-tool=desl"));
        assert_eq!(l.args.last().map(String::as_str), Some("--session-id=s1"), "the session stays last");
    }

    #[test]
    fn a_session_opens_by_the_id_desl_gives_it_new_or_not() {
        let new = launch(Open::New { session_id: Some("0cb916db-26aa-40f2-86b5-1ba81b225fd2".into()) }, PermissionMode::Edits);
        assert_eq!(
            new.args,
            [
                "--allow-tool=write",
                "--model",
                "gpt-6-luna",
                "--reasoning-effort",
                "low",
                "--session-id=0cb916db-26aa-40f2-86b5-1ba81b225fd2"
            ]
        );
        assert_eq!(new.session_id.as_deref(), Some("0cb916db-26aa-40f2-86b5-1ba81b225fd2"));
        let again = launch(Open::Resume { session_id: "0cb916db-26aa-40f2-86b5-1ba81b225fd2".into() }, PermissionMode::Ask);
        assert_eq!(again.args, ["--model", "gpt-6-luna", "--reasoning-effort", "low", "--session-id=0cb916db-26aa-40f2-86b5-1ba81b225fd2"]);
        let unnamed = launch(Open::New { session_id: None }, PermissionMode::Yolo);
        assert_eq!(unnamed.args[0], "--allow-all");
        assert!(unnamed.session_id.is_some_and(|id| unnamed.args.last().is_some_and(|a| *a == format!("--session-id={id}"))));
    }

    #[test]
    fn the_login_variables_reach_copilot() {
        let l = launch(Open::New { session_id: None }, PermissionMode::Ask);
        for var in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
            assert!(l.keep.iter().any(|k| k == var), "{var} would be scrubbed");
        }
    }
}
