//! The socket side of the daemon: accepts clients, turns their frames into engine messages and
//! writes the engine's frames back. One reader thread and one writer thread per client, so a
//! slow client never holds up the engine or another client.

use crate::engine::{ClientId, Engine, Msg};
use crate::store::Store;
use adapter_claude::Claude;
use adapter_codex::Codex;
use anyhow::{Context, Result, bail};
use desl_ir::adapter::Adapter;
use desl_proto::paths::Paths;
use desl_proto::{Request, read_frame};
use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{Sender, channel};
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

fn accept(listener: UnixListener, tx: Sender<Msg>) {
    let mut next: ClientId = 1;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let client = next;
        next += 1;
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
pub fn serve(paths: &Paths) -> Result<()> {
    if let Some(dir) = paths.socket.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    if UnixStream::connect(&paths.socket).is_ok() {
        bail!("A desl daemon is already running on {}. `desl daemon stop` stops it.", paths.socket.display());
    }
    let _ = std::fs::remove_file(&paths.socket);
    let len = paths.socket.as_os_str().len();
    if len > MAX_SOCKET_PATH {
        bail!(
            "The socket path {} is too long for a Unix socket ({len} bytes; the limit is {MAX_SOCKET_PATH}). Set DESL_SOCKET to a shorter path.",
            paths.socket.display()
        );
    }
    let listener = UnixListener::bind(&paths.socket).with_context(|| {
        format!("Could not create the socket {}. Check that its directory is writable, or set DESL_SOCKET", paths.socket.display())
    })?;
    // The socket carries the user's prompts and approvals: owner only.
    let _ = std::fs::set_permissions(&paths.socket, std::fs::Permissions::from_mode(0o600));
    init_log(paths)?;

    let (tx, rx) = channel::<Msg>();
    let store = Store::open(&paths.db, &paths.blobs)?;
    let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(Claude), Arc::new(Codex)];
    let mut engine = Engine::new(store, adapters, tx.clone())?;
    tracing::info!(socket = %paths.socket.display(), store = %paths.db.display(), pid = std::process::id(), "desl daemon {} started", env!("CARGO_PKG_VERSION"));

    {
        let tx = tx.clone();
        std::thread::spawn(move || accept(listener, tx));
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Msg::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }
    while let Ok(msg) = rx.recv() {
        engine.handle(msg);
        if engine.stopping {
            break;
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

/// Starts the daemon as a background process in its own session, so closing the terminal that
/// first ran `desl` never reaches it. `exe` is the desl binary. The child is returned so the
/// caller can tell a daemon that died at startup from one that is slow.
pub fn spawn_detached(exe: &std::path::Path, paths: &Paths) -> Result<std::process::Child> {
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
    cmd.args(["daemon", "run"]).stdin(Stdio::null()).stdout(out).stderr(err);
    cmd.spawn().context("Could not start the desl daemon")
}
