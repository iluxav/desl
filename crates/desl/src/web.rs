//! `desl web` from the terminal (web-spec.md "Opening the page"): `prefix v` in the TUI and
//! `desl web --open` find the desl web that serves this workspace, or start one in the
//! background, and open the browser on it. A running desl web says where it is in a file beside
//! the daemon's socket, which only the user can read, as only the user can reach the socket.

use anyhow::{Context, Result, bail};
use desl_proto::paths::Paths;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Serialize, Deserialize)]
struct Note {
    pid: u32,
    url: String,
}

/// `<socket>.web`: one desl web per workspace, as one daemon per socket.
fn note(paths: &Paths) -> PathBuf {
    let mut p = paths.socket.clone().into_os_string();
    p.push(".web");
    PathBuf::from(p)
}

fn read(paths: &Paths) -> Option<Note> {
    serde_json::from_slice(&std::fs::read(note(paths)).ok()?).ok()
}

/// This process serves the page at `url`: noted for the TUI and the next `desl web`.
pub fn record(paths: &Paths, url: &str) -> Result<()> {
    let body = serde_json::to_vec(&Note { pid: std::process::id(), url: url.to_string() })?;
    let mut tmp = note(paths).into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(&body)?;
    std::fs::rename(&tmp, note(paths))?;
    Ok(())
}

/// Takes the note away, when it is this process's.
pub fn forget(paths: &Paths) {
    if read(paths).is_some_and(|n| n.pid == std::process::id()) {
        let _ = std::fs::remove_file(note(paths));
    }
}

fn port(url: &str) -> Option<u16> {
    url.strip_prefix("http://127.0.0.1:")?.split('/').next()?.parse().ok()
}

fn answers(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(300)).is_ok()
}

/// The page's address, when the desl web noted for this workspace still answers.
pub fn running(paths: &Paths) -> Option<String> {
    let n = read(paths)?;
    answers(port(&n.url)?).then_some(n.url)
}

/// Starts desl web in the background, in a session of its own so that closing this terminal
/// does not end it, and returns its address once it serves the page.
pub fn start() -> Result<String> {
    let exe = std::env::current_exe().context("desl could not find its own binary to start desl web")?;
    let mut cmd = match desl_ir::adapter::which("setsid") {
        // A session leader with no terminal, in place, as the daemon is started.
        Some(setsid) => {
            let mut c = Command::new(setsid);
            c.arg(&exe);
            c
        }
        None => {
            let mut c = Command::new(&exe);
            c.process_group(0);
            c
        }
    };
    cmd.args(["web", "--json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    // It inherits DESL_HOME and DESL_SOCKET, so it serves the workspace this process talks to.
    let mut child = cmd.spawn().context("desl could not start desl web")?;
    let mut line = String::new();
    if let Some(out) = child.stdout.take() {
        let _ = BufReader::new(out).read_line(&mut line);
    }
    let url = serde_json::from_str::<serde_json::Value>(&line).ok().and_then(|v| v["url"].as_str().map(str::to_string));
    match url {
        Some(url) => {
            // It runs on after this process ends; until then, it is not left a zombie.
            std::thread::spawn(move || child.wait());
            Ok(url)
        }
        None => {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let _ = child.wait();
            match err.lines().map(str::trim).find(|l| !l.is_empty()) {
                Some(why) => bail!("desl web did not start: {}", why.trim_start_matches("Error: ")),
                None => bail!("desl web stopped before it served the page. Run `desl web` in a terminal to see why."),
            }
        }
    }
}

/// The desl web that serves this workspace, started when there is none.
pub fn ensure(paths: &Paths) -> Result<String> {
    match running(paths) {
        Some(url) => Ok(url),
        None => start(),
    }
}

/// Opens `url` in the user's browser. When it cannot, the sentence says why.
pub fn open(url: &str) -> std::result::Result<(), String> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if set("SSH_CONNECTION") || set("SSH_TTY") {
        return Err("desl runs over SSH here, so this machine's browser is not yours.".to_string());
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let Some(bin) = desl_ir::adapter::which(opener) else {
        return Err(format!("desl found no `{opener}` to open a browser with."));
    };
    let child = Command::new(bin).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    match child {
        Ok(mut c) => {
            std::thread::spawn(move || c.wait());
            Ok(())
        }
        Err(e) => Err(format!("desl could not run `{opener}` ({e}).")),
    }
}

/// Stops the desl web that serves this workspace. False when none was running.
pub fn stop(paths: &Paths) -> Result<bool> {
    let Some(n) = read(paths) else { return Ok(false) };
    let Some(port) = port(&n.url).filter(|p| answers(*p)) else {
        let _ = std::fs::remove_file(note(paths));
        return Ok(false);
    };
    // As ctrl+c would: it stops serving and takes its note away.
    let status = Command::new("kill").args(["-INT", &n.pid.to_string()]).status().context("desl could not run `kill`")?;
    if !status.success() {
        bail!("desl could not stop desl web (process {}). Stop it with ctrl+c where it runs.", n.pid);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while answers(port) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(true)
}

/// The thread the page opens on when `prefix v` is pressed on `on`: that thread when it is an
/// agent's, else its project's agent thread used last, so the page lands on the project the
/// user is in with a conversation to show. None when that project has no agent thread.
pub fn page_thread(threads: &[desl_ir::Thread], on: &str) -> Option<String> {
    let here = threads.iter().find(|t| t.id == on)?;
    if desl_proto::view::talks(here) {
        return Some(here.id.clone());
    }
    threads
        .iter()
        .filter(|t| t.project_id == here.project_id && desl_proto::view::talks(t))
        .max_by_key(|t| t.updated_at)
        .map(|t| t.id.clone())
}

/// The page's address, opened on one thread: the page selects it and goes to its open form.
pub fn url_for(url: &str, thread: &str) -> String {
    format!("{url}&thread={thread}")
}

/// Where the page is, without its key: what a sentence on screen names.
pub fn place(url: &str) -> &str {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    rest.split('/').next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            db: dir.join("workspace.db"),
            blobs: dir.join("blobs"),
            log: dir.join("daemon.log"),
            socket: dir.join("d.sock"),
            config: dir.join("config.toml"),
        }
    }

    fn thread(id: &str, project: &str, agent: &str, updated: i64) -> desl_ir::Thread {
        serde_json::from_value(serde_json::json!({
            "id": id, "project_id": project, "name": "", "cwd": "/src", "worktree": null, "parent_id": null,
            "fork_point": null, "active_agent": agent, "status": "idle", "permission_mode": "edits",
            "created_at": 0, "updated_at": updated, "head": 0, "bindings": {}
        }))
        .unwrap()
    }

    #[test]
    fn the_page_opens_on_the_thread_on_screen_or_its_projects_latest_agent() {
        let threads = [
            thread("t-1", "desl", "claude", 10),
            thread("t-2", "desl", "codex", 30),
            thread("t-3", "desl", desl_ir::SHELL, 40),
            thread("t-4", "desl", desl_ir::TASKS, 50),
            thread("t-5", "rebrand", desl_ir::FILES, 60),
        ];
        assert_eq!(page_thread(&threads, "t-1").as_deref(), Some("t-1"), "an agent's thread: itself");
        assert_eq!(page_thread(&threads, "t-3").as_deref(), Some("t-2"), "a shell: its project's latest agent");
        assert_eq!(page_thread(&threads, "t-4").as_deref(), Some("t-2"), "the task list too");
        assert_eq!(page_thread(&threads, "t-5"), None, "no agent in that project: the page picks");
    }

    #[test]
    fn a_noted_desl_web_is_found_while_it_answers_and_only_its_own_note_is_taken_away() {
        let dir = std::env::temp_dir().join(format!("desl-web-note-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = paths(&dir);
        assert_eq!(running(&p), None, "no note");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/#t=0123456789abcdef0123456789abcdef", listener.local_addr().unwrap().port());
        record(&p, &url).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(note(&p)).unwrap().permissions().mode() & 0o777, 0o600, "the key is the user's alone");
        assert_eq!(running(&p).as_deref(), Some(url.as_str()));
        drop(listener);
        assert_eq!(running(&p), None, "noted, but nothing answers there any more");
        // Another process's note stays.
        std::fs::write(note(&p), serde_json::to_vec(&Note { pid: 1, url: url.clone() }).unwrap()).unwrap();
        forget(&p);
        assert!(note(&p).exists());
        record(&p, &url).unwrap();
        forget(&p);
        assert!(!note(&p).exists(), "this process's note goes");
        assert!(!stop(&p).unwrap(), "nothing to stop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_address_can_name_the_thread_to_open() {
        assert_eq!(
            url_for("http://127.0.0.1:7428/#t=0123456789abcdef0123456789abcdef", "t-7f3a"),
            "http://127.0.0.1:7428/#t=0123456789abcdef0123456789abcdef&thread=t-7f3a"
        );
    }

    #[test]
    fn a_sentence_names_the_place_without_the_key() {
        assert_eq!(place("http://127.0.0.1:7428/#t=0123456789abcdef0123456789abcdef"), "127.0.0.1:7428");
    }
}
