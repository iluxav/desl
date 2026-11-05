//! The git service (SPEC.md "Architecture"): branch state for the status bar, the worktree diff
//! for briefs, the file list for the `@` picker. Wraps the `git` binary; read-only in phase 1.

use desl_ir::GitState;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The repository a directory belongs to, or the directory itself when it is not in one.
pub fn toplevel(cwd: &Path) -> PathBuf {
    git(cwd, &["rev-parse", "--show-toplevel"]).filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| cwd.to_path_buf())
}

pub fn state(cwd: &Path) -> GitState {
    let mut state = GitState::default();
    // A repository with no commits yet has a branch name but nothing for rev-parse to resolve.
    let Some(branch) = git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).or_else(|| git(cwd, &["symbolic-ref", "--short", "HEAD"])) else {
        return state;
    };
    state.branch = Some(if branch == "HEAD" { "detached HEAD".to_string() } else { branch });
    if let Some(counts) = git(cwd, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
        let mut it = counts.split_whitespace();
        state.ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        state.behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    }
    state
}

/// The worktree's diff against HEAD, for a brief's "Current diff". Empty outside a repository.
pub fn diff_head(cwd: &Path) -> String {
    git(cwd, &["diff", "HEAD"]).unwrap_or_default()
}

/// Tracked files and untracked files that are not ignored, capped.
pub fn files(cwd: &Path, cap: usize) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    for args in [&["ls-files"][..], &["ls-files", "--others", "--exclude-standard"][..]] {
        if let Some(out) = git(cwd, args) {
            files.extend(out.lines().map(str::to_string));
        }
    }
    files.truncate(cap);
    files
}
