//! The socket side of the daemon: accepts clients, turns their frames into engine messages and
//! writes the engine's frames back. One reader thread and one writer thread per client, so a
//! slow client never holds up the engine or another client.

use crate::engine::{ClientId, Engine, Msg};
use crate::handover;
use crate::store::Store;
use adapter_claude::Claude;
use adapter_codex::Codex;
use adapter_copilot::Copilot;
use anyhow::{Context, Result, bail};
use desl_ir::adapter::Adapter;
use desl_proto::paths::Paths;
use desl_proto::{Request, read_frame};
use std::io::{BufReader, Write};
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A Unix socket address holds about 104 bytes on macOS and 108 on Linux.
const MAX_SOCKET_PATH: usize = 100;

fn init_log(paths: &Paths) -> Result<()> {
    if let Some(dir) = paths.log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("opening {}", paths.log.display()))?;
    let level = if std::env::var_os("DESL_DEBUG").is_some_and(|v| !v.is_empty() && v != "0") {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    // A second daemon in the same process (tests) keeps the first one's log.
    let _ = tracing_subscriber::fmt().with_writer(Mutex::new(file)).with_ansi(false).with_max_level(level).try_init();
    Ok(())
}

/// The next client's id. One count for the whole daemon: it may come to listen a second time.
static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

fn accept(listener: UnixListener, tx: Sender<Msg>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let client: ClientId = NEXT_CLIENT.fetch_add(1, Ordering::SeqCst);
        let Ok(mut writer) = stream.try_clone() else { continue };
        let (out, outbox) = channel::<Arc<Vec<u8>>>();
        if tx.send(Msg::Connected { client, out }).is_err() {
            return;
        }
        std::thread::spawn(move || {
            for bytes in outbox {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            while let Ok(Some(request)) = read_frame::<_, Request>(&mut reader) {
                if tx.send(Msg::Request { client, request }).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Disconnected { client });
        });
    }
}

/// Runs the daemon in this process until a client sends `shutdown`.
/// Every agent desl has an adapter for, in the order lists show them.
pub fn adapters() -> Vec<Arc<dyn Adapter>> {
    vec![Arc::new(Claude), Arc::new(Codex), Arc::new(Copilot)]
}

/// Creates the socket and gives it to its owner alone: it carries the user's prompts and approvals.
fn listen(at: &Path) -> Result<UnixListener> {
    let listener = UnixListener::bind(at).with_context(|| {
        format!("Could not create the socket {}. Check that its directory is writable, or set DESL_SOCKET", at.display())
    })?;
    let _ = std::fs::set_permissions(at, std::fs::Permissions::from_mode(0o600));
    Ok(listener)
}

fn socket_fits(paths: &Paths) -> Result<()> {
    if let Some(dir) = paths.socket.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let len = paths.socket.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        bail!(
            "The socket path {} is too long for a Unix socket ({len} bytes; the limit is {MAX_SOCKET_PATH}). Set DESL_SOCKET to a shorter path.",
            paths.socket.display()
        );
    }
    Ok(())
}

/// Which file the socket path leads to: the daemon's own while nobody has taken its place.
fn socket_file(paths: &Paths) -> Option<(u64, u64)> {
    std::fs::metadata(&paths.socket).ok().map(|m| (m.dev(), m.ino()))
}

/// Runs the daemon in this process until a client sends `shutdown`, or until it has handed
/// its agents to the next daemon. With `takeover` it is that next daemon: started by the one
/// that leaves, which sends what it has on this process's standard input.
pub fn serve(paths: &Paths, takeover: bool) -> Result<()> {
    socket_fits(paths)?;
    if takeover {
        return take_over(paths);
    }
    if UnixStream::connect(&paths.socket).is_ok() {
        bail!("A desl daemon is already running on {}. `desl daemon stop` stops it.", paths.socket.display());
    }
    let _ = std::fs::remove_file(&paths.socket);
    let listener = listen(&paths.socket)?;
    init_log(paths)?;

    let (tx, rx) = channel::<Msg>();
    let store = Store::open(&paths.db, &paths.blobs)?;
    let mut engine = Engine::new(store, adapters(), tx.clone())?;
    engine.set_config_file(paths.config.clone());
    engine.set_socket(paths.socket.clone());
    tracing::info!(socket = %paths.socket.display(), store = %paths.db.display(), pid = std::process::id(), "desl daemon {} started", env!("CARGO_PKG_VERSION"));
    run(engine, rx, tx, listener, paths)
}

/// The daemon that takes over (SPEC.md "Upgrades"). It serves on the same socket path, moved
/// there in one step, so there is never a moment without a daemon behind it. Until it has
/// answered the daemon that leaves, it reads nothing from the terminals it was given and ends
/// nothing: whatever fails before that, the other daemon still has everything and stays.
fn take_over(paths: &Paths) -> Result<()> {
    init_log(paths)?;
    let from = std::io::stdin();
    let (answers, given) = handover::take(&from).context("The daemon that hands over did not send its terminals")?;
    let (tx, rx) = channel::<Msg>();
    let store = Store::open(&paths.db, &paths.blobs)?;
    let mut engine = Engine::taking_over(store, adapters(), tx.clone(), answers, given)?;
    engine.set_config_file(paths.config.clone());
    engine.set_socket(paths.socket.clone());
    let mut beside = paths.socket.clone().into_os_string();
    beside.push(".next");
    let beside = std::path::PathBuf::from(beside);
    let _ = std::fs::remove_file(&beside);
    let listener = match listen(&beside).and_then(|l| {
        std::fs::rename(&beside, &paths.socket).with_context(|| format!("Could not move the socket to {}", paths.socket.display()))?;
        Ok(l)
    }) {
        Ok(l) => l,
        Err(e) => {
            engine.let_go();
            let _ = std::fs::remove_file(&beside);
            return Err(e);
        }
    };
    tracing::info!(
        socket = %paths.socket.display(),
        pid = std::process::id(),
        terminals = engine.terminals(),
        "desl daemon {} started in place of the one before it, whose agents keep running",
        env!("CARGO_PKG_VERSION")
    );
    if let Err(e) = handover::serving(&from, std::process::id()) {
        tracing::warn!("the daemon that handed over did not hear that this one serves: {e}");
    }
    engine.read_terminals();
    run(engine, rx, tx, listener, paths)
}

fn run(mut engine: Engine, rx: Receiver<Msg>, tx: Sender<Msg>, listener: UnixListener, paths: &Paths) -> Result<()> {
    let own_socket = socket_file(paths);
    // Out of the terminal's scope that started it, with every agent: a logout or a kill of that
    // scope no longer ends them (crate::unit). A daemon that took over is in it already.
    let agents = engine.agent_pids();
    std::thread::spawn(move || match crate::unit::settle_in(&agents) {
        Ok(Some(scope)) => tracing::info!(scope, agents = agents.len(), "moved into a scope of its own, with its agents"),
        Ok(None) => {}
        Err(e) => tracing::warn!("could not move into a scope of its own, so it stays in the one that started it: {e}"),
    });
    {
        let tx = tx.clone();
        std::thread::spawn(move || accept(listener, tx));
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Msg::Tick).is_ok() {
                std::thread::sleep(Duration::from_millis(crate::engine::TICK_MS));
            }
        });
    }
    while let Ok(msg) = rx.recv() {
        engine.handle(msg);
        if engine.stopping {
            break;
        }
        let Some(exe) = engine.handover_due() else { continue };
        match hand_over(&mut engine, &rx, &exe, paths) {
            Ok(pid) => {
                // The socket path is the next daemon's now. Let the writer threads hand the
                // last frames to their sockets; the clients connect again when this one is gone.
                std::thread::sleep(Duration::from_millis(100));
                tracing::info!(pid, "handed every agent's terminal to the next daemon; this one is done");
                return Ok(());
            }
            Err(e) => {
                tracing::error!("the handover did not happen, so this daemon stays: {e:#}");
                // A daemon that came as far as taking the socket path and then failed left it dead.
                if socket_file(paths) != own_socket {
                    let _ = std::fs::remove_file(&paths.socket);
                    match listen(&paths.socket) {
                        Ok(listener) => {
                            let tx = tx.clone();
                            std::thread::spawn(move || accept(listener, tx));
                        }
                        Err(e) => tracing::error!("could not listen again: {e:#}"),
                    }
                }
            }
        }
    }
    // The socket goes first and only here: from this moment no client can reach this daemon, and
    // one started while the agents below are still ending owns whatever socket it binds.
    let _ = std::fs::remove_file(&paths.socket);
    tracing::info!("stopping: ending the agents");
    engine.shutdown();
    // Let the writer threads hand the last frames to their sockets. Clients read until the
    // connection closes, which happens when this process ends: that is how they know it is over.
    std::thread::sleep(Duration::from_millis(100));
    tracing::info!("desl daemon stopped");
    Ok(())
}

/// Starts the next daemon from `exe` and gives it every running program. On an error nothing
/// has changed: the readers read again and the client that asked is told why.
fn hand_over(engine: &mut Engine, rx: &Receiver<Msg>, exe: &Path, paths: &Paths) -> Result<u32> {
    // Stop reading the terminals, and take in what the readers had already read. A request
    // handled meanwhile may start a program, whose reader then stops in the next round.
    loop {
        engine.hold_terminals();
        while let Ok(msg) = rx.try_recv() {
            engine.handle(msg);
        }
        if !engine.reading_terminals() {
            break;
        }
    }
    let given = (|| -> Result<u32> {
        let (mut ours, theirs) = UnixStream::pair().context("Could not open a socket to the new daemon")?;
        // What the new daemon writes to the log from here on is what it has to say for itself.
        let logged = std::fs::metadata(&paths.log).map_or(0, |m| m.len());
        let mut next = spawn(exe, paths, Some(theirs.into()))?;
        let (answers, all) = engine.giving();
        let served = handover::give(&mut ours, &answers, &all).and_then(|()| handover::served(&mut ours));
        served.map_err(|e| {
            // It never said it serves: it must not be left reading what this daemon reads.
            // One that ended is a moment away from saying so.
            let ended = (0..20).any(|_| {
                let gone = matches!(next.try_wait(), Ok(Some(_)));
                if !gone {
                    std::thread::sleep(Duration::from_millis(10));
                }
                gone
            });
            let _ = next.kill();
            let _ = next.wait();
            let how = if ended { "ended before it took over".to_string() } else { format!("did not take over ({e})") };
            let said = std::fs::read(&paths.log).ok().and_then(|log| {
                let new = String::from_utf8_lossy(log.get(usize::try_from(logged).ok()?..)?).into_owned();
                new.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())
            });
            match said {
                Some(line) => anyhow::anyhow!("The desl at {} {how}. It said: {line}", exe.display()),
                None => anyhow::anyhow!("The desl at {} {how}, and said nothing.", exe.display()),
            }
        })
    })();
    match given {
        Ok(pid) => {
            while let Ok(msg) = rx.try_recv() {
                engine.last_keys(msg);
            }
            engine.handed_over(pid);
            Ok(pid)
        }
        Err(e) => {
            let mut why = format!("{e:#}");
            if !why.ends_with(['.', '!', '?']) {
                why.push('.');
            }
            engine.handover_failed(&format!("{why} The daemon that was running keeps running, and so does every agent."));
            Err(e)
        }
    }
}

/// Starts the daemon as a background process in its own session, so closing the terminal that
/// first ran `desl` never reaches it. `exe` is the desl binary. The child is returned so the
/// caller can tell a daemon that died at startup from one that is slow.
pub fn spawn_detached(exe: &std::path::Path, paths: &Paths) -> Result<std::process::Child> {
    spawn(exe, paths, None)
}

/// With `from`, the daemon takes over from the one that starts it, which talks to it there.
fn spawn(exe: &Path, paths: &Paths, from: Option<OwnedFd>) -> Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    if let Some(dir) = paths.log.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.log)
        .with_context(|| format!("opening {}", paths.log.display()))?;
    let err = out.try_clone()?;
    let mut cmd = match desl_ir::adapter::which("setsid") {
        // `setsid` makes the daemon a session leader with no terminal. It does that in place,
        // without forking, because the child is not a process group leader.
        Some(setsid) => {
            let mut c = Command::new(setsid);
            c.arg(exe);
            c
        }
        // No setsid (macOS): its own process group keeps the terminal's signals away.
        None => {
            let mut c = Command::new(exe);
            c.process_group(0);
            c
        }
    };
    cmd.args(["daemon", "run"]).stdout(out).stderr(err);
    match from {
        Some(from) => cmd.arg("--takeover").stdin(Stdio::from(from)),
        None => cmd.stdin(Stdio::null()),
    };
    cmd.spawn().with_context(|| format!("Could not start the desl daemon from {}", exe.display()))
}
