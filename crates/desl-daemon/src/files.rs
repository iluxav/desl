//! Files a form names (docs/design/2026-11-21-web-forms.md, "Documents, diffs, images"): they
//! resolve, links followed, inside the thread's directory. Once checked, the file's folder is
//! opened part by part without following a link and kept open, so that neither a folder nor the
//! file swapped for a link afterwards, nor a FIFO, can lead desl elsewhere or stop it.

use rustix::fs::{Mode, OFlags, openat};
use std::ffi::OsString;
use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::{Component, Path, PathBuf};

/// A checked file: its folder, held open, its name in it, and its real path when checked.
pub struct Located {
    pub dir: OwnedFd,
    pub name: OsString,
    pub real: PathBuf,
}

/// Where `path` is inside `dir`: refused when it is outside or missing. `path` is relative to
/// `dir` or absolute; `what` names it in the sentence ("image", "document").
pub fn locate(dir: &Path, path: &str, what: &str) -> Result<Located, String> {
    let root = dir.canonicalize().map_err(|e| format!("desl could not read the thread's directory {} ({e}).", dir.display()))?;
    let given = Path::new(path);
    let full = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
    let real =
        full.canonicalize().map_err(|_| format!("There is no file \"{path}\" in {}. Name a file of the project.", root.display()))?;
    if !real.starts_with(&root) {
        return Err(format!("The {what} \"{path}\" is outside the thread's directory, {}. Name a file inside it.", root.display()));
    }
    let rel = real.strip_prefix(&root).unwrap_or(Path::new(""));
    let Some(name) = rel.file_name().map(OsString::from) else { return Err(format!("The {what} \"{path}\" is not a file.")) };
    let moved = |e| format!("The {what} \"{path}\" moved while desl opened it ({e}). Try again.");
    let mut fd = openat(rustix::fs::CWD, &root, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).map_err(moved)?;
    for part in rel.parent().map(Path::components).into_iter().flatten() {
        let Component::Normal(part) = part else { continue };
        fd = openat(&fd, part, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty()).map_err(moved)?;
    }
    Ok(Located { dir: fd, name, real })
}

/// The file `path` names inside `dir`, opened for reading in its held folder: refused when it
/// is outside, missing, not a regular file or over `max` bytes.
pub fn open_inside(dir: &Path, path: &str, what: &str, max: u64) -> Result<(File, Located), String> {
    let at = locate(dir, path, what)?;
    // NOFOLLOW: the file is not swapped for a link meanwhile; NONBLOCK: a FIFO or a device is
    // refused below rather than waited on, and a regular file ignores it.
    let fd = openat(&at.dir, &at.name, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())
        .map_err(|e| format!("desl could not open the {what} \"{path}\" ({e})."))?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|e| format!("desl could not read the {what} \"{path}\" ({e})."))?;
    if !meta.is_file() {
        return Err(format!("The {what} \"{path}\" is not a file."));
    }
    if meta.len() > max {
        let mb = max / (1024 * 1024);
        return Err(format!("The {what} \"{path}\" is {} MB; desl takes files of at most {mb} MB.", meta.len().div_ceil(1024 * 1024)));
    }
    Ok((file, at))
}
