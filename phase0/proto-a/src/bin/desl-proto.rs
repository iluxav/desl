//! The blind launcher for the hallway sessions: picks prototype A or B for a session, logs which,
//! and becomes it. The user sees "desl" either way and is never told what A and B are.
//!
//! `desl-proto`        start or rejoin a session in this directory
//! `desl-proto log`    the sessions so far, for the observer
//! `desl-proto a|b`    force a prototype (logged as forced)

use proto_a::paths;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn host_running(cwd: &Path, proto: &str) -> bool {
    UnixStream::connect(paths::socket_path_for(cwd, proto)).is_ok()
}

fn counts() -> (usize, usize) {
    let text = std::fs::read_to_string(paths::launcher_log()).unwrap_or_default();
    let mut a = 0;
    let mut b = 0;
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v["how"] == "rejoined" {
            continue;
        }
        match v["prototype"].as_str() {
            Some("a") => a += 1,
            Some("b") => b += 1,
            _ => {}
        }
    }
    (a, b)
}

/// Balanced random: the prototype with fewer sessions so far, a coin flip on a tie.
fn pick() -> &'static str {
    let (a, b) = counts();
    if a < b {
        return "a";
    }
    if b < a {
        return "b";
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    if (nanos / 1000) % 2 == 0 { "a" } else { "b" }
}

fn log(cwd: &Path, proto: &str, how: &str) {
    let path = paths::launcher_log();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let entry = serde_json::json!({
        "time": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "cwd": cwd.display().to_string(),
        "prototype": proto,
        "label": proto.to_uppercase(),
        "how": how,
    });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{entry}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| ".".into());
    let mut passthrough: Vec<String> = Vec::new();
    let (proto, how): (&str, &str) = match args.first().map(String::as_str) {
        Some("log") => {
            let text = std::fs::read_to_string(paths::launcher_log()).unwrap_or_default();
            if text.trim().is_empty() {
                println!("No sessions yet. The log is {}.", paths::launcher_log().display());
                return;
            }
            for line in text.lines() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    println!(
                        "{}  {}  {:<8}  {}",
                        v["time"].as_str().unwrap_or(""),
                        v["label"].as_str().unwrap_or("?"),
                        v["how"].as_str().unwrap_or(""),
                        v["cwd"].as_str().unwrap_or("")
                    );
                }
            }
            let (a, b) = counts();
            println!("sessions: A {a}, B {b}");
            return;
        }
        Some("a") => ("a", "forced"),
        Some("b") => ("b", "forced"),
        Some("status") | Some("stop") => {
            // Scriptable commands go to whichever prototype holds this directory's session.
            passthrough = args.clone();
            if host_running(&cwd, "b") && !host_running(&cwd, "a") { ("b", "") } else { ("a", "") }
        }
        Some(other) => {
            eprintln!("Unknown command `{other}`. Run with no arguments to start, or `log`, `status` or `stop`.");
            std::process::exit(2);
        }
        // A session that is still running in this directory is rejoined, never re-rolled.
        None if host_running(&cwd, "a") => ("a", "rejoined"),
        None if host_running(&cwd, "b") => ("b", "rejoined"),
        None => (pick(), "picked"),
    };
    if !how.is_empty() {
        log(&cwd, proto, how);
    }
    let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(format!("proto-{proto}"))));
    let Some(exe) = exe.filter(|p| p.is_file()) else {
        eprintln!("The prototype binary proto-{proto} is not installed next to this launcher. Run `make install`.");
        std::process::exit(1);
    };
    let err = Command::new(exe).args(passthrough).exec();
    eprintln!("Could not start the prototype: {err}.");
    std::process::exit(1);
}
