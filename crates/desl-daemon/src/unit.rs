//! The daemon's own systemd scope (SPEC.md "Architecture"). Started from a terminal, the daemon
//! and every agent it runs would sit in that terminal's scope: a logout, or one kill of that
//! scope under memory pressure, would end them all together. So the daemon moves itself, its
//! agents and their processes into a scope of their own, `desl-<pid>.scope`, through the user's
//! systemd, the way `systemd-run --scope` does. A daemon that takes over at an install starts in
//! that scope already and stays. Without systemd, or under a service the user wrote, nothing
//! moves.

use std::collections::HashSet;
use std::process::{Command, Stdio};

/// The unit a process is in, from its `/proc/<pid>/cgroup`: the last part of its path.
fn unit_of(cgroup: &str) -> Option<&str> {
    let line = cgroup.lines().find(|l| l.starts_with("0::"))?;
    line.rsplit('/').next().filter(|u| !u.is_empty())
}

/// Whether a process in `cgroup` should move to a scope of its own: it is in a scope of the
/// user's manager (a terminal's, a session's) that is not desl's already. A service, which the
/// user set up on purpose, keeps it.
fn should_move(cgroup: &str) -> bool {
    let under_user = cgroup.lines().any(|l| l.starts_with("0::") && l.contains("/user@"));
    under_user && unit_of(cgroup).is_some_and(|u| u.ends_with(".scope") && !u.starts_with("desl-"))
}

/// `roots` and every process under them, from `(pid, parent)` pairs: roots first, then each
/// generation in turn.
fn tree(ppids: &[(u32, u32)], roots: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for r in roots {
        if seen.insert(*r) {
            out.push(*r);
        }
    }
    let mut at = 0;
    while at < out.len() {
        let parent = out[at];
        for (pid, pp) in ppids {
            if *pp == parent && seen.insert(*pid) {
                out.push(*pid);
            }
        }
        at += 1;
    }
    out
}

/// Every process's `(pid, parent)`, from /proc.
fn ppids() -> Vec<(u32, u32)> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.filter_map(|e| {
        let pid: u32 = e.ok()?.file_name().to_str()?.parse().ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // `pid (name) state ppid …`: the name may hold spaces and parentheses.
        let after = &stat[stat.rfind(')')? + 1..];
        let ppid = after.split_whitespace().nth(1)?.parse().ok()?;
        Some((pid, ppid))
    })
    .collect()
}

/// busctl's arguments for the systemd call `systemd-run --scope` makes: a transient scope that
/// takes `pids` in.
fn call(name: &str, pids: &[u32]) -> Vec<String> {
    let mut a: Vec<String> = [
        "--user",
        "call",
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        "ssa(sv)a(sa(sv))",
        name,
        "fail",
        "3",
        "PIDs",
        "au",
    ]
    .map(String::from)
    .to_vec();
    a.push(pids.len().to_string());
    a.extend(pids.iter().map(u32::to_string));
    a.extend(["Description", "s", "desl daemon: its agents and shells", "CollectMode", "s", "inactive-or-failed", "0"].map(String::from));
    a
}

/// Moves this daemon, the processes in `agents` and everything under them into a scope of their
/// own, when it sits in another scope of the user's systemd. The scope's name when it moved;
/// None when there was nothing to do or no systemd to ask; a sentence when systemd refused.
pub fn settle_in(agents: &[u32]) -> Result<Option<String>, String> {
    if std::env::var_os("DESL_NO_SCOPE").is_some() {
        return Ok(None);
    }
    let Ok(cgroup) = std::fs::read_to_string("/proc/self/cgroup") else { return Ok(None) };
    if !should_move(&cgroup) {
        return Ok(None);
    }
    let Some(busctl) = desl_ir::adapter::which("busctl") else { return Ok(None) };
    let me = std::process::id();
    let mut roots = vec![me];
    roots.extend_from_slice(agents);
    let pids = tree(&ppids(), &roots);
    let name = format!("desl-{me}.scope");
    let out =
        Command::new(busctl).args(call(&name, &pids)).stdin(Stdio::null()).output().map_err(|e| format!("could not run busctl ({e})"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(Some(name))
}

/// The unit this process runs in, as systemd names it: `desl-4242.scope`.
pub fn current() -> Option<String> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    unit_of(&cgroup).map(str::to_string)
}

fn linger_of(out: &str) -> Option<bool> {
    match out.lines().find_map(|l| l.trim().strip_prefix("Linger="))? {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// Whether the user's systemd keeps running after they log out, and with it this scope: None
/// when loginctl cannot say.
pub fn lingers() -> Option<bool> {
    let loginctl = desl_ir::adapter::which("loginctl")?;
    let user = std::env::var("USER").ok().filter(|u| !u.is_empty())?;
    let out = Command::new(loginctl).args(["show-user", &user, "-p", "Linger"]).stdin(Stdio::null()).output().ok()?;
    linger_of(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scope_a_process_is_in_is_read_from_its_cgroup() {
        let term = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-graphical.slice/app-Hyprland-xdg\\x2dterminal\\x2dexec-e9e22875.scope\n";
        assert_eq!(unit_of(term), Some("app-Hyprland-xdg\\x2dterminal\\x2dexec-e9e22875.scope"));
        assert!(should_move(term), "a terminal's scope: out of it");
        let own = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/desl-4242.scope\n";
        assert!(!should_move(own), "its own already");
        let service = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/desl.service\n";
        assert!(!should_move(service), "a service the user wrote keeps it");
        let system = "0::/system.slice/docker-abc.scope\n";
        assert!(!should_move(system), "no user manager to ask");
    }

    #[test]
    fn linger_is_read_from_loginctl() {
        assert_eq!(linger_of("Linger=yes\n"), Some(true));
        assert_eq!(linger_of("Linger=no\n"), Some(false));
        assert_eq!(linger_of("Failed to get user: No such user\n"), None);
    }

    #[test]
    fn a_tree_is_its_roots_and_everything_under_them() {
        // daemon 10 runs agents 20 and 30; 20 runs 21, which runs 22; 40 is someone else's.
        let ppids = [(20, 10), (30, 10), (21, 20), (22, 21), (40, 1), (41, 40)];
        assert_eq!(tree(&ppids, &[10]), [10, 20, 30, 21, 22]);
        // After a handover the agents' parent is gone: they are roots of their own.
        assert_eq!(tree(&ppids, &[99, 20, 30]), [99, 20, 30, 21, 22]);
    }

    #[test]
    fn the_call_is_the_one_systemd_run_scope_makes() {
        let args = call("desl-10.scope", &[10, 20]);
        assert_eq!(
            args,
            [
                "--user",
                "call",
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                "StartTransientUnit",
                "ssa(sv)a(sa(sv))",
                "desl-10.scope",
                "fail",
                "3",
                "PIDs",
                "au",
                "2",
                "10",
                "20",
                "Description",
                "s",
                "desl daemon: its agents and shells",
                "CollectMode",
                "s",
                "inactive-or-failed",
                "0",
            ]
        );
    }
}
