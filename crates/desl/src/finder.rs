//! `prefix o`: open a directory as a project, the way fzf and zoxide find one. Typing filters a
//! list of the projects desl knows, the directories zoxide remembers and the git repositories
//! under the home directory; a typed path (`~/src/x`, `/srv`, `../y`) lists its directory
//! instead, and tab completes it. Enter picks the highlighted directory.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

/// Directories below the home directory never worth descending into.
const SKIP: &[&str] = &["node_modules", "target", "vendor", "dist", "build", "Library", "go", "snap", "venv", "__pycache__"];
/// How deep under the home directory to look for repositories, and how many directories to read.
const DEPTH: usize = 4;
const BUDGET: usize = 40_000;
/// Rows the finder shows at most.
pub const SHOWN: usize = 50;

/// Where a directory in the list comes from, shown dim beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Project,
    Recent,
    Repo,
    Dir,
}

impl Kind {
    pub fn note(self) -> &'static str {
        match self {
            Kind::Project => "project",
            Kind::Recent => "recent",
            Kind::Repo => "git",
            Kind::Dir => "",
        }
    }
}

pub struct Finder {
    pub query: String,
    pub cursor: usize,
    /// Every candidate, best source first, each path once.
    found: Vec<(PathBuf, Kind)>,
    scan: Option<Receiver<Vec<(PathBuf, Kind)>>>,
    home: Option<PathBuf>,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `~/x` for a path under the home directory.
pub fn tilde(p: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// The directories zoxide remembers, most used first; none when it is not installed.
fn zoxide() -> Vec<PathBuf> {
    let Ok(out) = std::process::Command::new("zoxide").args(["query", "--list"]).stderr(std::process::Stdio::null()).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(PathBuf::from).filter(|p| p.is_dir()).collect()
}

/// Git repositories under `root`, breadth first, not inside one another.
fn repos(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut level = vec![root.to_path_buf()];
    let mut read = 0;
    for _ in 0..=DEPTH {
        let mut next = Vec::new();
        for dir in level {
            read += 1;
            if read > BUDGET {
                return out;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let Ok(t) = e.file_type() else { continue };
                if !t.is_dir() {
                    continue;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') || SKIP.contains(&name.as_ref()) {
                    continue;
                }
                let p = e.path();
                if p.join(".git").exists() {
                    out.push(p);
                } else {
                    next.push(p);
                }
            }
        }
        level = next;
    }
    out
}

/// How well `query` matches `text` as an fzf-style subsequence, higher better; None if not.
fn score(text: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let base = t.iter().rposition(|&c| c == '/').map_or(0, |i| i + 1);
    let mut s = 0i64;
    let mut at = 0;
    let mut last: Option<usize> = None;
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let i = (at..t.len()).find(|&i| t[i] == q)?;
        s += 1;
        if last == Some(i.wrapping_sub(1)) {
            s += 8;
        }
        if i >= base {
            s += 4;
        }
        if i == 0 || matches!(t[i - 1], '/' | '-' | '_' | '.') {
            s += 6;
        }
        last = Some(i);
        at = i + 1;
    }
    // Shorter paths win ties: `desl` over `desl/crates/desl`.
    Some(s * 100 - t.len() as i64)
}

impl Finder {
    /// Starts with `known`, the projects' directories; the rest arrives from a scan in the background.
    pub fn new(known: &[String]) -> Finder {
        let home = home();
        let found = known.iter().map(|p| (PathBuf::from(p), Kind::Project)).collect();
        let (tx, rx) = mpsc::channel();
        let root = home.clone();
        std::thread::spawn(move || {
            let mut more: Vec<(PathBuf, Kind)> = zoxide().into_iter().map(|p| (p, Kind::Recent)).collect();
            let _ = tx.send(std::mem::take(&mut more));
            if let Some(root) = root {
                let _ = tx.send(repos(&root).into_iter().map(|p| (p, Kind::Repo)).collect());
            }
        });
        Finder { query: String::new(), cursor: 0, found, scan: Some(rx), home }
    }

    /// Takes what the scan found since last time; true when the list changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.scan else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(batch) => {
                    for (p, k) in batch {
                        if !self.found.iter().any(|(q, _)| *q == p) {
                            self.found.push((p, k));
                            changed = true;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.scan = None;
                    changed = true;
                    break;
                }
            }
        }
        changed
    }

    pub fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    /// The typed query as a path, when it is one: `~/x`, `/x`, `./x`, `../x`.
    fn typed(&self) -> Option<PathBuf> {
        let q = self.query.trim();
        if let Some(rest) = q.strip_prefix('~') {
            let h = self.home.clone()?;
            return Some(if rest.is_empty() { h } else { h.join(rest.trim_start_matches('/')) });
        }
        if q.starts_with('/') || q.starts_with("./") || q.starts_with("../") {
            return Some(std::env::current_dir().map(|c| c.join(q)).unwrap_or_else(|_| PathBuf::from(q)));
        }
        None
    }

    /// The rows to show, best first.
    pub fn matches(&self) -> Vec<(PathBuf, Kind)> {
        if let Some(path) = self.typed() {
            // A path: the directory itself if it is one, then its subdirectories that start with
            // what follows the last slash.
            let ends = self.query.ends_with('/') || self.query.trim() == "~";
            let (dir, stem) = if ends {
                (path.clone(), String::new())
            } else {
                let stem = path.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                (path.parent().map(Path::to_path_buf).unwrap_or(path.clone()), stem)
            };
            let mut out = Vec::new();
            if path.is_dir() {
                out.push((path.clone(), Kind::Dir));
            }
            let mut subs: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map(|r| {
                    r.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()) && e.path().is_dir())
                        .map(|e| e.path())
                        .filter(|p| {
                            let n = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                            n.starts_with(&stem) && (stem.starts_with('.') || !n.starts_with('.'))
                        })
                        .filter(|p| *p != path)
                        .collect()
                })
                .unwrap_or_default();
            subs.sort();
            out.extend(subs.into_iter().map(|p| {
                let k = self.found.iter().find(|(q, _)| *q == p).map_or(Kind::Dir, |(_, k)| *k);
                (p, k)
            }));
            out.truncate(SHOWN);
            return out;
        }
        let mut scored: Vec<(i64, usize, &PathBuf, Kind)> = self
            .found
            .iter()
            .enumerate()
            .filter_map(|(i, (p, k))| score(&tilde(p, self.home.as_deref()), &self.query).map(|s| (s, i, p, *k)))
            .collect();
        // With nothing typed, the sources keep their order: projects, recent, repositories.
        if !self.query.trim().is_empty() {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        scored.into_iter().take(SHOWN).map(|(_, _, p, k)| (p.clone(), k)).collect()
    }

    /// Tab: the query becomes the highlighted directory, with a slash to go on into it.
    pub fn complete(&mut self) {
        if let Some((p, _)) = self.matches().get(self.cursor) {
            let mut q = tilde(p, self.home.as_deref());
            if !q.ends_with('/') {
                q.push('/');
            }
            self.query = q;
            self.cursor = 0;
        }
    }

    pub fn tilde(&self, p: &Path) -> String {
        tilde(p, self.home.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test, removed when it ends.
    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Temp {
            let p = std::env::temp_dir().join(format!("desl-finder-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Temp(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_name_beats_a_path_that_only_contains_its_letters() {
        let a = score("~/Projects/desl", "desl").unwrap();
        let b = score("~/Projects/desl/crates/desl-daemon", "desl").unwrap();
        let c = score("~/d/e/s/l", "desl").unwrap();
        assert!(a > b && a > c, "{a} {b} {c}");
        assert!(score("~/Projects/desl", "xyz").is_none());
        assert_eq!(score("anything", ""), Some(0));
    }

    #[test]
    fn a_typed_path_lists_its_directory_and_tab_completes() {
        let dir = Temp::new("typed");
        for d in ["alpha", "alps", "beta", ".hidden"] {
            std::fs::create_dir(dir.path().join(d)).unwrap();
        }
        std::fs::write(dir.path().join("alfile"), "").unwrap();
        let mut f = Finder { query: format!("{}/al", dir.path().display()), cursor: 0, found: Vec::new(), scan: None, home: None };
        let names: Vec<String> = f.matches().iter().map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["alpha", "alps"]);
        f.complete();
        assert_eq!(f.query, format!("{}/alpha/", dir.path().display()));
        assert_eq!(f.matches()[0], (dir.path().join("alpha"), Kind::Dir), "the directory itself comes first");
        f.query = format!("{}/", dir.path().display());
        assert_eq!(f.matches().len(), 4, "itself, alpha, alps, beta; not .hidden");
    }

    #[test]
    fn repositories_are_found_but_not_inside_one_another() {
        let home = Temp::new("repos");
        for d in ["src/api/.git", "src/api/sub/.git", "node_modules/x/.git", ".cache/y/.git", "a/b/c/web/.git"] {
            std::fs::create_dir_all(home.path().join(d)).unwrap();
        }
        let mut got = repos(home.path());
        got.sort();
        assert_eq!(got, [home.path().join("a/b/c/web"), home.path().join("src/api")]);
    }
}
