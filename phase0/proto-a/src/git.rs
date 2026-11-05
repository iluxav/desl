//! Branch and ahead/behind for the status bar. Read-only, cheap, refreshed on a timer.

use std::path::Path;
use std::process::Command;

#[derive(Default, Clone)]
pub struct GitInfo {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn read(cwd: &Path) -> GitInfo {
    let mut info = GitInfo::default();
    // A repository with no commits yet has a branch name but nothing for rev-parse to resolve.
    let Some(branch) = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).or_else(|| git(cwd, &["symbolic-ref", "--short", "HEAD"])) else {
        return info;
    };
    info.branch = Some(if branch == "HEAD" { "detached HEAD".to_string() } else { branch });
    if let Some(counts) = git(cwd, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
        let mut it = counts.split_whitespace();
        info.ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        info.behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    }
    info
}
