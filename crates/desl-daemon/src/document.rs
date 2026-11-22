//! The `document` widget's file (docs/design/2026-11-21-web-forms.md, "Documents, diffs,
//! images"): a Markdown file inside the thread's directory, of at most 1 MB, whose section is read
//! when the form is pushed and saved on Send, in one write, only over what the user started from.

use crate::files::open_inside;
use desl_proto::form::MAX_SECTION_BYTES;
use desl_proto::markdown;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The most a document's file may weigh.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Lines of the diff the agent gets before `more` says how many were left out.
const DIFF_LINES: usize = 400;

/// The file's text and real path, checked: inside, Markdown, UTF-8, LF line endings.
fn load(dir: &Path, path: &str) -> Result<(String, PathBuf), String> {
    let (file, real) = open_inside(dir, path, "document", MAX_FILE_BYTES)?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("desl could not read \"{path}\" ({e})."))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("The document \"{path}\" grew over 1 MB; desl takes files of at most 1 MB."));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("The document \"{path}\" is not UTF-8 text."))?;
    if text.contains('\r') {
        return Err(format!(
            "The document \"{path}\" has Windows line endings (CRLF); desl edits files with Unix line endings only. Convert it first."
        ));
    }
    Ok((text, real))
}

/// The section under `heading` (the whole file without one), as the card shows it for editing.
pub fn read(dir: &Path, path: &str, heading: Option<&str>) -> Result<String, String> {
    let (text, _) = load(dir, path)?;
    let range = markdown::section(&text, heading).map_err(|e| format!("In \"{path}\": {e}"))?;
    let part = &text[range];
    if part.len() > MAX_SECTION_BYTES {
        return Err(format!(
            "The section of \"{path}\" is {} KB; desl edits at most 256 KB at once. Name a smaller section.",
            part.len().div_ceil(1024)
        ));
    }
    Ok(part.to_string())
}

/// The section as it is on disk now: None when its heading is gone from the file.
pub fn current(dir: &Path, path: &str, heading: Option<&str>) -> Result<Option<String>, String> {
    let (text, _) = load(dir, path)?;
    Ok(markdown::section(&text, heading).ok().map(|r| text[r].to_string()))
}

/// A save worked out and checked, not yet written.
pub struct Plan {
    real: PathBuf,
    before: String,
    after: String,
}

/// The file with the section replaced by `text`, when the section on disk is still `base`, the
/// text the edit started from. None when `text` is `base`: nothing to save.
pub fn plan(dir: &Path, path: &str, heading: Option<&str>, base: &str, text: &str) -> Result<Option<Plan>, String> {
    let text = text.replace("\r\n", "\n");
    if text == base {
        return Ok(None);
    }
    let (before, real) = load(dir, path)?;
    let changed =
        || format!("\"{path}\" changed on disk since the form was shown: its section is not what you started from. Nothing was saved.");
    let range = markdown::section(&before, heading).map_err(|_| changed())?;
    if before[range.clone()] != *base {
        return Err(changed());
    }
    let mut middle = text;
    // A section that ended its line keeps doing so, or the next heading would join its last line.
    if range.end < before.len() && !middle.is_empty() && !middle.ends_with('\n') {
        middle.push('\n');
    }
    let after = format!("{}{middle}{}", &before[..range.start], &before[range.end..]);
    Ok(Some(Plan { real, before, after }))
}

/// Writes a planned save in one go: a file beside it, renamed over it, with its mode.
pub fn write(plan: &Plan, path: &str) -> Result<(), String> {
    let folder = plan.real.parent().ok_or_else(|| format!("desl could not save \"{path}\": it has no folder."))?;
    let name = plan.real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = folder.join(format!(".{name}.desl-{}", std::process::id()));
    let mode = std::fs::metadata(&plan.real).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
    let failed = |e: std::io::Error| format!("desl could not save \"{path}\" ({e}). The file is as it was.");
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp).map_err(failed)?;
    if let Err(e) = f.write_all(plan.after.as_bytes()).and_then(|()| f.sync_all()) {
        let _ = std::fs::remove_file(&tmp);
        return Err(failed(e));
    }
    if let Err(e) = std::fs::rename(&tmp, &plan.real) {
        let _ = std::fs::remove_file(&tmp);
        return Err(failed(e));
    }
    Ok(())
}

/// The unified diff of a save, by `git diff --no-index`, named after `path`: its first 400 lines
/// and how many more there were. Empty when git cannot run.
pub fn diff(plan: &Plan, path: &str) -> (String, usize) {
    let Some(full) = git_diff(&plan.before, &plan.after) else { return (String::new(), 0) };
    // git names the two scratch files; the agent knows the file by the path it gave.
    let body: Vec<&str> = full.lines().skip_while(|l| !l.starts_with("@@")).collect();
    let mut lines = vec![format!("--- a/{path}"), format!("+++ b/{path}")];
    lines.extend(body.iter().map(|l| l.to_string()));
    let more = lines.len().saturating_sub(DIFF_LINES);
    lines.truncate(DIFF_LINES);
    (lines.join("\n") + "\n", more)
}

fn git_diff(before: &str, after: &str) -> Option<String> {
    let scratch = std::env::temp_dir().join(format!("desl-diff-{}-{}", std::process::id(), unique()));
    std::fs::DirBuilder::new().mode(0o700).create(&scratch).ok()?;
    let put = |name: &str, text: &str| -> Option<()> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(scratch.join(name)).ok()?;
        f.write_all(text.as_bytes()).ok()
    };
    let out = put("a", before).and_then(|()| put("b", after)).and_then(|()| {
        Command::new("git")
            .args(["-c", "core.quotepath=false", "diff", "--no-index", "--no-color", "--no-ext-diff", "-U3", "--", "a", "b"])
            .current_dir(&scratch)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
    });
    let _ = std::fs::remove_dir_all(&scratch);
    out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}
