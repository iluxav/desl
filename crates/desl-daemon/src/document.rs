//! The `document` widget's file (docs/design/2026-11-21-web-forms.md, "Documents, diffs,
//! images"): a Markdown file inside the thread's directory, of at most 1 MB, whose section is read
//! when the form is pushed and saved on Send, in one write, only over what the user started from.

use crate::files::{Located, open_inside};
use desl_proto::form::MAX_SECTION_BYTES;
use desl_proto::markdown;
use rustix::fs::{Mode, OFlags, fchmod, openat, renameat, unlinkat};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The most a document's file may weigh.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Lines of the diff the agent gets before `more` says how many were left out.
const DIFF_LINES: usize = 400;

/// The file's text, where it is and its mode, checked: inside, Markdown, UTF-8, LF line endings.
fn load(dir: &Path, path: &str) -> Result<(String, Located, u32), String> {
    let (file, at) = open_inside(dir, path, "document", MAX_FILE_BYTES)?;
    // Its real name too: a .md link to another file of the project is not that file's way in.
    let markdown = at.real.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    if !markdown {
        return Err(format!("\"{path}\" leads to {}, which is not a Markdown file.", at.real.display()));
    }
    let mode = file.metadata().map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o644);
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
    Ok((text, at, mode))
}

/// The section under `heading` (the whole file without one), as the card shows it for editing,
/// and the file's real path.
pub fn read(dir: &Path, path: &str, heading: Option<&str>) -> Result<(String, PathBuf), String> {
    let (text, at, _) = load(dir, path)?;
    let range = markdown::section(&text, heading).map_err(|e| format!("In \"{path}\": {e}"))?;
    let part = &text[range];
    if part.len() > MAX_SECTION_BYTES {
        return Err(format!(
            "The section of \"{path}\" is {} KB; desl edits at most 256 KB at once. Name a smaller section.",
            part.len().div_ceil(1024)
        ));
    }
    Ok((part.to_string(), at.real))
}

/// The section as it is on disk now: None when its heading is gone from the file.
/// A sentence when the file cannot be read, the heading is there twice, or the section grew
/// past what the card edits.
pub fn current(dir: &Path, path: &str, heading: Option<&str>) -> Result<Option<String>, String> {
    let (text, _, _) = load(dir, path)?;
    let Some(range) = markdown::find(&text, heading).map_err(|e| format!("In \"{path}\": {e}"))? else { return Ok(None) };
    if range.len() > MAX_SECTION_BYTES {
        return Err(format!("The section of \"{path}\" is now {} KB; desl edits at most 256 KB at once.", range.len().div_ceil(1024)));
    }
    Ok(Some(text[range].to_string()))
}

/// A save worked out and checked, not yet written: the folder it was checked in stays open.
pub struct Plan {
    at: Located,
    mode: u32,
    before: String,
    after: String,
}

impl Plan {
    /// The file's real path when it was checked.
    pub fn real(&self) -> &Path {
        &self.at.real
    }
}

/// The file with the section replaced by `text`, when the section on disk is still `base`, the
/// text the edit started from. None when `text` is `base`: nothing to save.
pub fn plan(dir: &Path, path: &str, heading: Option<&str>, base: &str, text: &str) -> Result<Option<Plan>, String> {
    let text = text.replace("\r\n", "\n");
    if text == base {
        return Ok(None);
    }
    let (before, at, mode) = load(dir, path)?;
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
    Ok(Some(Plan { at, mode, before, after }))
}

/// Writes a planned save in one go, in the folder it was checked in: a new file beside it,
/// given the file's mode, renamed over it.
pub fn write(plan: &Plan, path: &str) -> Result<(), String> {
    let failed = |e: std::io::Error| format!("desl could not save \"{path}\" ({e}). The file is as it was.");
    let dir = &plan.at.dir;
    let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    // A name nobody can guess, and another one when something is there already.
    let mut tries = 0;
    let (tmp, fd) = loop {
        let mut tmp = std::ffi::OsString::from(".");
        tmp.push(&plan.at.name);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        tmp.push(format!(".desl-{}-{}-{nanos:08x}", std::process::id(), unique()));
        match openat(dir, &tmp, flags, Mode::from_raw_mode(0o600)) {
            Ok(fd) => break (tmp, fd),
            Err(e) if e == rustix::io::Errno::EXIST && tries < 8 => tries += 1,
            Err(e) => return Err(failed(e.into())),
        }
    };
    let mut f = std::fs::File::from(fd);
    let done = f
        .write_all(plan.after.as_bytes())
        .and_then(|()| fchmod(&f, Mode::from_raw_mode(plan.mode)).map_err(Into::into))
        .and_then(|()| f.sync_all())
        .and_then(|()| renameat(dir, &tmp, dir, &plan.at.name).map_err(Into::into));
    if let Err(e) = done {
        let _ = unlinkat(dir, &tmp, rustix::fs::AtFlags::empty());
        return Err(failed(e));
    }
    Ok(())
}

/// The unified diff of a save, by `git diff --no-index`, named after `path`: its first 400 lines
/// and how many more there were. Empty when git cannot run.
pub fn diff(plan: &Plan, path: &str) -> (String, usize) {
    let Some(full) = git_diff(&plan.before, &plan.after) else { return (String::new(), 0) };
    shaped(&full, path)
}

/// git's diff of the two scratch files, named after `path`, and capped.
fn shaped(full: &str, path: &str) -> (String, usize) {
    // git names the two scratch files; the agent knows the file by the path it gave.
    let body: Vec<&str> = full.lines().skip_while(|l| !l.starts_with("@@")).collect();
    if body.is_empty() {
        return (String::new(), 0);
    }
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
    let out = put("a", before).and_then(|()| put("b", after)).and_then(|()| run_git(&scratch));
    let _ = std::fs::remove_dir_all(&scratch);
    out
}

/// git's diff of `a` and `b` in `scratch`, given 3 seconds: it runs on the engine's thread, and a
/// git that hangs (a wrapper, a stuck filesystem) must not stop desl. Its output is read on the
/// side so a long diff cannot fill the pipe and stall it.
fn run_git(scratch: &Path) -> Option<String> {
    let mut child = Command::new("git")
        .args(["-c", "core.quotepath=false", "diff", "--no-index", "--no-color", "--no-ext-diff", "--no-textconv", "-U3", "--", "a", "b"])
        .current_dir(scratch)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok().map(|out| String::from_utf8_lossy(&out).into_owned())
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const DOC: &str = "# Guide\n\n## Install\n\nRun it.\n\n## Use\n\nGo.\n";

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("desl-doc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proj/sub")).unwrap();
        std::fs::create_dir_all(d.join("outside")).unwrap();
        d
    }

    #[test]
    fn a_folder_swapped_for_a_link_between_check_and_write_is_not_written_through() {
        let d = scratch("swap");
        let proj = d.join("proj");
        std::fs::write(proj.join("sub/a.md"), DOC).unwrap();
        std::fs::write(d.join("outside/a.md"), DOC).unwrap();
        let plan = plan(&proj, "sub/a.md", Some("Install"), "## Install\n\nRun it.\n\n", "## Install\n\nMine.\n\n").unwrap().unwrap();
        // Between the check and the write, the folder becomes a link to one outside.
        std::fs::rename(proj.join("sub"), proj.join("held")).unwrap();
        std::os::unix::fs::symlink(d.join("outside"), proj.join("sub")).unwrap();
        let _ = write(&plan, "sub/a.md");
        assert_eq!(std::fs::read_to_string(d.join("outside/a.md")).unwrap(), DOC, "nothing written outside");
        assert!(std::fs::read_to_string(proj.join("held/a.md")).unwrap().contains("Mine."), "written where it was checked");
    }

    #[test]
    fn a_save_keeps_the_files_mode() {
        let d = scratch("mode");
        let proj = d.join("proj");
        std::fs::write(proj.join("a.md"), DOC).unwrap();
        std::fs::set_permissions(proj.join("a.md"), std::fs::Permissions::from_mode(0o664)).unwrap();
        let p = plan(&proj, "a.md", None, DOC, "# Guide\n").unwrap().unwrap();
        write(&p, "a.md").unwrap();
        assert_eq!(std::fs::metadata(proj.join("a.md")).unwrap().permissions().mode() & 0o777, 0o664);
    }

    #[test]
    fn a_markdown_name_linked_to_another_file_is_refused() {
        let d = scratch("ext");
        let proj = d.join("proj");
        std::fs::write(proj.join(".env"), "TOKEN=x\n").unwrap();
        std::os::unix::fs::symlink(proj.join(".env"), proj.join("notes.md")).unwrap();
        let err = read(&proj, "notes.md", None).unwrap_err();
        assert!(err.contains("Markdown"), "{err}");
    }
}

#[cfg(test)]
mod shaping {
    #[test]
    fn git_output_with_no_hunk_is_no_diff() {
        // git that failed, or printed only its header, says nothing of the change.
        assert_eq!(super::shaped("diff --git a/a b/b\n--- a/a\n+++ b/b\n", "x.md"), (String::new(), 0));
        let (d, more) = super::shaped("diff --git a/a b/b\n--- a/a\n+++ b/b\n@@ -1 +1 @@\n-a\n+b\n", "x.md");
        assert_eq!((d.as_str(), more), ("--- a/x.md\n+++ b/x.md\n@@ -1 +1 @@\n-a\n+b\n", 0));
    }
}
