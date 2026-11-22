//! Files a form names (docs/design/2026-11-21-web-forms.md, "Documents, diffs, images"): they
//! resolve, links followed, inside the thread's directory, and are opened so that neither a
//! later swap of the last part for a link nor a FIFO can lead desl elsewhere or stop it.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The file `path` names inside `dir`, opened for reading, with its real path: refused when it
/// is outside, missing, not a regular file or over `max` bytes. `path` is relative to `dir` or
/// absolute; `what` names it in the sentence ("image", "document").
pub fn open_inside(dir: &Path, path: &str, what: &str, max: u64) -> Result<(File, PathBuf), String> {
    let root = dir.canonicalize().map_err(|e| format!("desl could not read the thread's directory {} ({e}).", dir.display()))?;
    let given = Path::new(path);
    let full = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
    let real =
        full.canonicalize().map_err(|_| format!("There is no file \"{path}\" in {}. Name a file of the project.", root.display()))?;
    if !real.starts_with(&root) {
        return Err(format!("The {what} \"{path}\" is outside the thread's directory, {}. Name a file inside it.", root.display()));
    }
    // The last part may not be swapped for a link between the check above and the read, and a
    // FIFO or a device is refused below rather than waited on: a regular file ignores NONBLOCK.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32)
        .open(&real)
        .map_err(|e| format!("desl could not open the {what} \"{path}\" ({e})."))?;
    let meta = file.metadata().map_err(|e| format!("desl could not read the {what} \"{path}\" ({e})."))?;
    if !meta.is_file() {
        return Err(format!("The {what} \"{path}\" is not a file."));
    }
    if meta.len() > max {
        let mb = max / (1024 * 1024);
        return Err(format!("The {what} \"{path}\" is {} MB; desl takes files of at most {mb} MB.", meta.len().div_ceil(1024 * 1024)));
    }
    Ok((file, real))
}
