//! desl's settings (SPEC.md "Configuration"): `config.toml` in the user's config directory, and
//! `<project>/.desl/config.toml`, which overrides it. Every key is optional; a missing file is
//! the defaults. The files are TOML, of the flat kind desl needs: `[sections]` of
//! `key = "string"`, `key = true` or `key = 12` lines, `#` comments. A line desl cannot read is
//! reported, never guessed at.

use std::path::{Path, PathBuf};

/// The settings in effect, and where each came from, for `desl config show`.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub prefix: Prefix,
    pub default_agent: Option<String>,
    /// A permission mode word: `ask`, `edits`, `yolo`.
    pub default_mode: Option<String>,
    pub worktree_dir: String,
    pub branch_prefix: String,
    /// `section.key = value  # source` for every key set by a file.
    pub set: Vec<(String, String, String)>,
    /// Lines that were not understood, as sentences.
    pub problems: Vec<String>,
}

/// The key that comes before desl's own keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    /// The unshifted key with ctrl: a letter, or 32 for space.
    pub code: u32,
}

impl Prefix {
    /// `ctrl+space`, `ctrl+a` … `ctrl+z`.
    pub fn parse(s: &str) -> Option<Prefix> {
        let key = s.trim().to_ascii_lowercase();
        let rest = key.strip_prefix("ctrl+").or_else(|| key.strip_prefix("c-"))?;
        match rest {
            "space" => Some(Prefix { code: 32 }),
            r if r.len() == 1 && r.as_bytes()[0].is_ascii_lowercase() => Some(Prefix { code: u32::from(r.as_bytes()[0]) }),
            _ => None,
        }
    }

    /// How the key is written in the interface: `ctrl+space`.
    pub fn label(&self) -> String {
        match self.code {
            32 => "ctrl+space".into(),
            c => format!("ctrl+{}", char::from_u32(c).unwrap_or('?')),
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            prefix: Prefix { code: 32 },
            default_agent: None,
            default_mode: None,
            worktree_dir: ".worktrees".into(),
            branch_prefix: "desl/".into(),
            set: Vec::new(),
            problems: Vec::new(),
        }
    }
}

/// `value` without its quotes, or as written for `true` and numbers.
fn value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        return Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"));
    }
    if raw == "true" || raw == "false" || raw.parse::<f64>().is_ok() || raw.starts_with('[') {
        return Some(raw.to_string());
    }
    None
}

/// The settings from `text`, applied over `into`; `source` names the file in reports.
pub fn apply(into: &mut Config, text: &str, source: &str) {
    let mut section = String::new();
    for (n, line) in text.lines().enumerate() {
        // A `#` outside quotes starts a comment.
        let mut in_str = false;
        let cut = line.char_indices().find(|&(_, c)| {
            if c == '"' {
                in_str = !in_str;
            }
            c == '#' && !in_str
        });
        let line = cut.map_or(line, |(i, _)| &line[..i]).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim().to_string();
            continue;
        }
        let Some((key, raw)) = line.split_once('=') else {
            into.problems.push(format!("{source} line {}: `{line}` is not `key = value`.", n + 1));
            continue;
        };
        let key = key.trim();
        let Some(v) = value(raw) else {
            into.problems.push(format!("{source} line {}: the value of {key} needs quotes, like {key} = \"…\".", n + 1));
            continue;
        };
        let full = format!("{section}.{key}");
        let known = match full.as_str() {
            "general.prefix" => match Prefix::parse(&v) {
                Some(p) => {
                    into.prefix = p;
                    true
                }
                None => {
                    into.problems.push(format!(
                        "{source} line {}: prefix \"{v}\" is not one desl knows; use ctrl+space or ctrl+a to ctrl+z.",
                        n + 1
                    ));
                    continue;
                }
            },
            "general.default_agent" => {
                into.default_agent = Some(v.clone());
                true
            }
            "permissions.default_mode" => {
                if !["ask", "edits", "yolo"].contains(&v.as_str()) {
                    into.problems.push(format!("{source} line {}: default_mode \"{v}\" is not ask, edits or yolo.", n + 1));
                    continue;
                }
                into.default_mode = Some(v.clone());
                true
            }
            "worktrees.dir" => {
                into.worktree_dir = v.clone();
                true
            }
            "worktrees.branch_prefix" => {
                into.branch_prefix = v.clone();
                true
            }
            // Read by later versions; accepted so a full file from SPEC.md is not an error.
            "general.theme"
            | "agents.claude.model"
            | "agents.claude.extra_args"
            | "agents.codex.model"
            | "agents.codex.extra_args"
            | "permissions.destructive_patterns"
            | "brief.token_budget"
            | "brief.agent_summary" => false,
            _ => {
                into.problems.push(format!("{source} line {}: desl has no setting {full}.", n + 1));
                continue;
            }
        };
        let note = if known { source.to_string() } else { format!("{source}, not used yet") };
        into.set.retain(|(k, _, _)| *k != full);
        into.set.push((full, v, note));
    }
}

/// The project's own settings file.
pub fn project_file(project: &Path) -> PathBuf {
    project.join(".desl").join("config.toml")
}

/// The project a directory belongs to, as far as settings go: the nearest directory up from
/// `cwd` with a `.desl/config.toml`, not past the repository's root.
pub fn find_project(cwd: &Path) -> Option<PathBuf> {
    for dir in cwd.ancestors() {
        if project_file(dir).is_file() {
            return Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            return None;
        }
    }
    None
}

/// The user's settings, then the project's over them when `project` is given.
pub fn load(user: &Path, project: Option<&Path>) -> Config {
    let mut c = Config::default();
    for path in std::iter::once(user.to_path_buf()).chain(project.map(project_file)) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            apply(&mut c, &text, &path.to_string_lossy());
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_example_reads_and_the_project_overrides_the_user() {
        let mut c = Config::default();
        apply(
            &mut c,
            r#"
[general]
default_agent = "claude"   # the one c starts on
prefix = "ctrl+a"
theme = "default"

[agents.claude]
extra_args = []

[permissions]
default_mode = "edits"

[worktrees]
dir = ".wt"
"#,
            "user",
        );
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert_eq!((c.prefix.label(), c.default_agent.as_deref(), c.worktree_dir.as_str()), ("ctrl+a".into(), Some("claude"), ".wt"));
        apply(&mut c, "[general]\ndefault_agent = \"codex\"\n", "project");
        assert_eq!(c.default_agent.as_deref(), Some("codex"));
        assert!(c.set.iter().any(|(k, v, s)| k == "general.default_agent" && v == "codex" && s == "project"));
        assert!(c.set.iter().any(|(k, _, s)| k == "general.theme" && s.contains("not used yet")));
    }

    #[test]
    fn a_line_desl_cannot_read_says_so_and_changes_nothing() {
        let mut c = Config::default();
        apply(&mut c, "[general]\nprefix = ctrl+a\nprefix = \"alt+x\"\nwhat = \"x\"\n[permissions]\ndefault_mode = \"maybe\"\n", "f");
        assert_eq!(c.prefix, Prefix { code: 32 }, "ctrl+space stays");
        assert_eq!(c.problems.len(), 4, "{:?}", c.problems);
        assert!(c.problems[0].contains("needs quotes"));
        assert_eq!(Prefix::parse("ctrl+space").map(|p| p.code), Some(32));
        assert_eq!(Prefix::parse("C-g").map(|p| p.label()), Some("ctrl+g".into()));
    }
}
