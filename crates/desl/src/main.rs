//! desl (say diesel): one persistent terminal workspace for coding agents.
//!
//! One binary, two roles (SPEC.md "Architecture"): `desl` is the thin client, a TUI or a CLI
//! command; `desl daemon run` is the long-lived daemon the client starts on first use.

mod cli;
mod conn;
mod finder;
mod layout;
mod model;
mod native;
mod rawkeys;
mod theme;
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
enum ConfigCmd {
    /// Print every setting in effect here, with its source
    Show,
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
        /// Give the thread a git worktree of its own, on a branch desl/<thread>
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
        /// Give the thread a git worktree of its own, on a branch desl/<thread>
        #[arg(long)]
        worktree: bool,
    },
    /// List the models of a thread's agent, or pick one
    Model {
        thread: String,
        /// A model from the list, or `default` for the agent's own
        name: Option<String>,
        /// How hard the model thinks: a level from the list, or `default` for the agent's own setting
        #[arg(long)]
        effort: Option<String>,
    },
    /// Show what a thread's agent may do without asking, or change it
    Permissions {
        thread: String,
        /// ask, edits or yolo
        mode: Option<String>,
    },
    /// Print the handoff brief desl would send now
    Brief { thread: String },
    /// Block until the thread is waiting, idle or failed
    Wait { thread: String },
    /// Stop the thread's current turn, as esc does in the agent's own screen
    Interrupt { thread: String },
    /// End the thread's agent; the thread stays, and its next prompt resumes it
    Exit { thread: String },
    /// Start the thread's agent again on the same conversation, with the binary and settings installed now
    Restart { thread: String },
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
    /// desl's settings: what is in effect and where each value comes from
    Config {
        #[command(subcommand)]
        command: Option<ConfigCmd>,
    },
    /// Check binaries, logins, the daemon and the store
    Doctor,
    /// The desl skill, which teaches Claude Code and Codex to drive desl
    Skill {
        #[command(subcommand)]
        command: SkillCmd,
    },
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
enum SkillCmd {
    /// Install the skill into every agent's skill folder, or bring it up to date
    Install,
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

/// A desl drawn inside one of its own threads would draw itself without end. Like tmux with
/// $TMUX, desl refuses; a desl on another socket, such as a test workspace, attaches as usual.
fn not_nested(paths: &Paths) -> anyhow::Result<()> {
    let Some(own) = desl_proto::paths::own_thread() else { return Ok(()) };
    let Some(mut conn) = conn::Conn::open(paths, false, "cli")? else { return Ok(()) };
    if cli::workspace(&mut conn)?.threads.iter().any(|t| t.id == own) {
        anyhow::bail!(
            "This terminal is already inside desl, in thread {own}. Run `unset DESL_THREAD` first to attach desl inside it anyway"
        );
    }
    Ok(())
}

fn attach(paths: &Paths) -> anyhow::Result<()> {
    not_nested(paths)?;
    let cwd = std::env::current_dir()?;
    let conn = match conn::Conn::open(paths, true, "tui")? {
        Some(c) => c,
        None => anyhow::bail!("The desl daemon could not be started. Its log is {}.", paths.log.display()),
    };
    let settings = desl_proto::config::load(&paths.config, desl_proto::config::find_project(&cwd).as_deref());
    // Leaving the client is never a failure: detaching, or the last agent quitting.
    if let Some(line) = native::run(conn, cwd, settings)? {
        println!("{line}");
    }
    Ok(())
}

fn run(cli: Cli, paths: &Paths) -> anyhow::Result<()> {
    let out = cli::Out { json: cli.json };
    match cli.command.unwrap_or(Cmd::Attach) {
        Cmd::Attach => attach(paths),
        Cmd::Status => cli::status(paths, &out),
        Cmd::Thread { command } => match command {
            ThreadCmd::List => cli::thread_list(paths, &out),
            ThreadCmd::Show { thread } => cli::thread_show(paths, &out, &thread),
            ThreadCmd::Rename { thread, name } => cli::thread_rename(paths, &out, &thread, &name),
            ThreadCmd::Rm { thread } => cli::thread_rm(paths, &out, &thread),
        },
        Cmd::New { project, agent, worktree, message } => cli::new(paths, &out, project, agent, message, worktree),
        Cmd::Send { thread, prompt } => cli::send(paths, &out, &thread, &prompt),
        Cmd::Switch { thread, agent } => cli::switch(paths, &out, &thread, &agent),
        Cmd::Fork { thread, agent, at, worktree } => cli::fork(paths, &out, &thread, agent, at, worktree),
        Cmd::Model { thread, name, effort } => cli::model(paths, &out, &thread, name, effort),
        Cmd::Permissions { thread, mode } => cli::permissions(paths, &out, &thread, mode),
        Cmd::Brief { thread } => cli::brief(paths, &out, &thread),
        Cmd::Wait { thread } => cli::wait(paths, &out, &thread),
        Cmd::Interrupt { thread } => cli::interrupt(paths, &out, &thread),
        Cmd::Exit { thread } => cli::exit(paths, &out, &thread),
        Cmd::Restart { thread } => cli::restart(paths, &out, &thread),
        Cmd::Approve { thread, always } => cli::answer(paths, &out, &thread, if always { Decision::Always } else { Decision::Allow }),
        Cmd::Deny { thread } => cli::answer(paths, &out, &thread, Decision::Deny),
        Cmd::Log { thread } => cli::log(paths, &out, &thread),
        Cmd::Agents => cli::agents(paths, &out),
        Cmd::Config { command: None | Some(ConfigCmd::Show) } => cli::config_show(paths, &out),
        Cmd::Doctor => cli::doctor(paths, &out),
        Cmd::Skill { command: SkillCmd::Install } => cli::skill_install(&out),
        Cmd::Daemon { command } => match command {
            DaemonCmd::Start => cli::daemon_start(paths, &out),
            DaemonCmd::Stop => cli::daemon_stop(paths, &out),
            DaemonCmd::Restart => cli::daemon_restart(paths, &out),
            DaemonCmd::Logs { lines } => cli::daemon_logs(paths, lines),
            DaemonCmd::Run => desl_daemon::server::serve(paths),
        },
    }
}

/// `desl status | head` stops reading early. Like any Unix tool, desl then just stops: the
/// print that found the pipe closed ends the process quietly instead of panicking.
fn quiet_on_closed_pipe() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let msg = payload.downcast_ref::<String>().map(String::as_str).or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("");
        if msg.contains("Broken pipe") {
            std::process::exit(0);
        }
        default(info);
    }));
}

fn main() {
    quiet_on_closed_pipe();
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
