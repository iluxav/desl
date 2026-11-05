//! desl (say diesel): one persistent terminal workspace for coding agents.
//!
//! One binary, two roles (SPEC.md "Architecture"): `desl` is the thin client, a TUI or a CLI
//! command; `desl daemon run` is the long-lived daemon the client starts on first use.

mod app;
mod cli;
mod commands;
mod conn;
mod input;
mod keymap;
mod model;
mod picker;
mod theme;
mod tui;
mod ui;

use clap::{Parser, Subcommand};
use desl_ir::Decision;
use desl_proto::paths::Paths;

#[derive(Parser)]
#[command(
    name = "desl",
    version,
    about = "desl (say diesel): run every coding agent in one persistent terminal workspace.",
    long_about = None,
    disable_help_subcommand = true,
    after_help = "Run `desl` with no command inside a repository to attach. Inside, ? shows every key."
)]
struct Cli {
    /// Print machine-readable JSON
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Attach to the workspace; starts the daemon if needed (the default)
    Attach,
    /// The sidebar as text
    Status,
    /// List, show, rename or remove threads
    Thread {
        #[command(subcommand)]
        command: ThreadCmd,
    },
    /// Open a new thread
    New {
        /// A project id or a directory (default: the current directory)
        #[arg(short, long)]
        project: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        /// Not built yet; arrives with worktrees
        #[arg(long)]
        worktree: bool,
        /// A first prompt to send
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Send a prompt and stream the reply to stdout
    Send { thread: String, prompt: String },
    /// Continue a thread with another agent
    Switch { thread: String, agent: String },
    /// Copy a thread up to a point into a new thread
    Fork {
        thread: String,
        agent: Option<String>,
        /// The event index to fork at (default: the newest)
        #[arg(long)]
        at: Option<u64>,
        /// Not built yet; arrives with worktrees
        #[arg(long)]
        worktree: bool,
    },
    /// List the models of a thread's agent, or pick one
    Model { thread: String, name: Option<String> },
    /// Print the handoff brief desl would send now
    Brief { thread: String },
    /// Block until the thread is waiting, idle or failed
    Wait { thread: String },
    /// Allow what the thread is waiting to do
    Approve {
        thread: String,
        /// Allow this tool for the rest of the thread
        #[arg(long)]
        always: bool,
    },
    /// Deny what the thread is waiting to do
    Deny { thread: String },
    /// Every approval of a thread, who answered it and when
    Log { thread: String },
    /// Detected agents and login state
    Agents,
    /// Check binaries, logins, the daemon and the store
    Doctor,
    /// Start, stop or restart the daemon, or read its log
    Daemon {
        #[command(subcommand)]
        command: DaemonCmd,
    },
}

#[derive(Subcommand)]
enum ThreadCmd {
    List,
    Show {
        thread: String,
    },
    Rename {
        thread: String,
        name: String,
    },
    /// Remove a thread and its history
    Rm {
        thread: String,
    },
}

#[derive(Subcommand)]
enum DaemonCmd {
    Start,
    /// Stop the daemon and every agent; threads are kept
    Stop,
    Restart,
    /// The last lines of the daemon's log
    Logs {
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Run the daemon in the foreground (what `start` runs in the background)
    #[command(hide = true)]
    Run,
}

fn attach(paths: &Paths) -> anyhow::Result<()> {
    let cwd = std::env::current_dir()?;
    let conn = match conn::Conn::open(paths, true, "tui")? {
        Some(c) => c,
        None => anyhow::bail!("The desl daemon could not be started. Its log is {}.", paths.log.display()),
    };
    if let Some(line) = tui::run(conn, cwd)? {
        println!("{line}");
        if !line.starts_with("Detached") && !line.starts_with("The desl daemon was stopped") {
            std::process::exit(1);
        }
    }
    Ok(())
}

fn run(cli: Cli, paths: &Paths) -> anyhow::Result<()> {
    let out = cli::Out { json: cli.json };
    let no_worktree = |on: bool| {
        if on {
            anyhow::bail!("--worktree is not built yet; it arrives with worktrees. Run the command without it.")
        }
        Ok(())
    };
    match cli.command.unwrap_or(Cmd::Attach) {
        Cmd::Attach => attach(paths),
        Cmd::Status => cli::status(paths, &out),
        Cmd::Thread { command } => match command {
            ThreadCmd::List => cli::thread_list(paths, &out),
            ThreadCmd::Show { thread } => cli::thread_show(paths, &out, &thread),
            ThreadCmd::Rename { thread, name } => cli::thread_rename(paths, &out, &thread, &name),
            ThreadCmd::Rm { thread } => cli::thread_rm(paths, &out, &thread),
        },
        Cmd::New { project, agent, worktree, message } => {
            no_worktree(worktree)?;
            cli::new(paths, &out, project, agent, message)
        }
        Cmd::Send { thread, prompt } => cli::send(paths, &out, &thread, &prompt),
        Cmd::Switch { thread, agent } => cli::switch(paths, &out, &thread, &agent),
        Cmd::Fork { thread, agent, at, worktree } => {
            no_worktree(worktree)?;
            cli::fork(paths, &out, &thread, agent, at)
        }
        Cmd::Model { thread, name } => cli::model(paths, &out, &thread, name),
        Cmd::Brief { thread } => cli::brief(paths, &out, &thread),
        Cmd::Wait { thread } => cli::wait(paths, &out, &thread),
        Cmd::Approve { thread, always } => cli::answer(paths, &out, &thread, if always { Decision::Always } else { Decision::Allow }),
        Cmd::Deny { thread } => cli::answer(paths, &out, &thread, Decision::Deny),
        Cmd::Log { thread } => cli::log(paths, &out, &thread),
        Cmd::Agents => cli::agents(paths, &out),
        Cmd::Doctor => cli::doctor(paths, &out),
        Cmd::Daemon { command } => match command {
            DaemonCmd::Start => cli::daemon_start(paths, &out),
            DaemonCmd::Stop => cli::daemon_stop(paths, &out),
            DaemonCmd::Restart => cli::daemon_restart(paths, &out),
            DaemonCmd::Logs { lines } => cli::daemon_logs(paths, lines),
            DaemonCmd::Run => desl_daemon::server::serve(paths),
        },
    }
}

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    let paths = Paths::resolve();
    if let Err(e) = run(cli, &paths) {
        // One line, a complete sentence; never a stack trace.
        let mut sentence = format!("{e:#}");
        if !sentence.ends_with(['.', '!', '?']) {
            sentence.push('.');
        }
        if json {
            println!("{}", serde_json::json!({ "error": sentence }));
        } else {
            eprintln!("{sentence}");
        }
        std::process::exit(1);
    }
}
