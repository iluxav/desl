//! The git service (SPEC.md "Architecture"): branch state for the status bar, the worktree diff
//! for briefs, the file list for the `@` picker, and a worktree per thread. Wraps the `git`
//! binary.

use desl_ir::GitState;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(cwd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// `git` that says why it failed: its first line of stderr.
fn git_or_why(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out =
        Command::new("git").arg("-C").arg(cwd).args(args).stdin(Stdio::null()).output().map_err(|e| format!("could not run git ({e})"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or("git failed").trim().trim_start_matches("fatal: ").to_string())
    }
}

/// Where worktrees go inside a project, and the prefix of their branches (SPEC.md "Configuration").
pub const WORKTREE_DIR: &str = ".worktrees";
pub const BRANCH_PREFIX: &str = "desl/";

/// A new worktree `name` of the repository at `root`, on a new branch `desl/<name>` from the
/// commit `from` is on. `.worktrees/` goes into the repository's `info/exclude`, so the user's
/// tracked files never change.
pub fn worktree_add(root: &Path, name: &str, from: &Path) -> Result<PathBuf, String> {
    let top = git(root, &["rev-parse", "--show-toplevel"])
        .ok_or_else(|| format!("Worktrees need a git repository, and {} is not one.", root.display()))?;
    let base = git(from, &["rev-parse", "HEAD"])
        .ok_or_else(|| "Worktrees need a first commit to branch from. Commit something, then try again.".to_string())?;
    let top = PathBuf::from(top);
    if let Some(common) = git(&top, &["rev-parse", "--git-common-dir"]) {
        let common = if Path::new(&common).is_absolute() { PathBuf::from(common) } else { top.join(common) };
        let exclude = common.join("info").join("exclude");
        let line = format!("/{WORKTREE_DIR}/");
        let now = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !now.lines().any(|l| l.trim() == line) {
            let _ = std::fs::create_dir_all(common.join("info"));
            let sep = if now.is_empty() || now.ends_with('\n') { "" } else { "\n" };
            let _ = std::fs::write(&exclude, format!("{now}{sep}{line}\n"));
        }
    }
    let path = top.join(WORKTREE_DIR).join(name);
    let branch = format!("{BRANCH_PREFIX}{name}");
    let p = path.to_string_lossy().into_owned();
    git_or_why(&top, &["worktree", "add", "-b", &branch, &p, &base]).map_err(|why| format!("Could not make the worktree {p}: {why}."))?;
    Ok(path)
}

/// Removes the worktree at `path`, never with changes in it: git refuses one with uncommitted
/// or untracked files, and the branch always stays.
pub fn worktree_remove(path: &Path) -> Result<(), String> {
    let p = path.to_string_lossy().into_owned();
    git_or_why(path, &["worktree", "remove", &p]).map(|_| ())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git").arg("-C").arg(dir).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn a_worktree_branches_from_the_commit_and_goes_only_when_clean() {
        let dir = std::env::temp_dir().join(format!("desl-git-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]);
        assert!(worktree_add(&dir, "t-1", &dir).unwrap_err().contains("first commit"));
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "one"]);
        let wt = worktree_add(&dir, "t-1", &dir).unwrap();
        assert!(wt.join("a.txt").exists());
        assert_eq!(state(&wt).branch.as_deref(), Some("desl/t-1"));
        assert_eq!(git(&dir, &["status", "--porcelain"]).unwrap(), "", "the main tree stays clean");
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        assert!(worktree_remove(&wt).is_err(), "changes keep it");
        std::fs::remove_file(wt.join("b.txt")).unwrap();
        worktree_remove(&wt).unwrap();
        assert!(!wt.exists());
        assert!(git(&dir, &["rev-parse", "--verify", "desl/t-1"]).is_some(), "the branch stays");
        let plain = std::env::temp_dir().join(format!("desl-git-plain-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        assert!(worktree_add(&plain, "t-2", &plain).unwrap_err().contains("git repository"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }
}
