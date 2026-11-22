//! The pictures an `images` widget names (docs/design/2026-11-21-web-forms.md, "Documents,
//! diffs, images"): files inside the thread's directory, links followed, PNG, JPEG, GIF, WebP
//! or SVG, at most 10 MB. Checked when the form is pushed and again each time the page reads
//! one, since the file may have changed meanwhile.

use desl_proto::form::MAX_IMAGE_BYTES;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How much of a file is read to tell its type.
const SNIFF: usize = 4096;

fn open(dir: &Path, path: &str) -> Result<(File, PathBuf), String> {
    crate::files::open_inside(dir, path, "image", MAX_IMAGE_BYTES).map(|(file, at)| (file, at.real))
}

/// The media type of a picture, from its first bytes; an SVG also needs its name to say so.
fn kind(head: &[u8], real: &Path) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if real.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) && String::from_utf8_lossy(head).contains("<svg") {
        Some("image/svg+xml")
    } else {
        None
    }
}

fn not_a_picture(path: &str) -> String {
    format!("The image \"{path}\" is not a PNG, JPEG, GIF, WebP or SVG picture.")
}

/// Whether `path` names a picture desl shows, inside `dir`. A sentence naming it otherwise.
pub fn check(dir: &Path, path: &str) -> Result<(), String> {
    let (file, real) = open(dir, path)?;
    let mut head = Vec::with_capacity(SNIFF);
    file.take(SNIFF as u64).read_to_end(&mut head).map_err(|e| format!("desl could not read the image \"{path}\" ({e})."))?;
    kind(&head, &real).map(|_| ()).ok_or_else(|| not_a_picture(path))
}

/// The picture `path` names inside `dir`: its media type and its bytes.
pub fn read(dir: &Path, path: &str) -> Result<(&'static str, Vec<u8>), String> {
    let (file, real) = open(dir, path)?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("desl could not read the image \"{path}\" ({e})."))?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err(format!("The image \"{path}\" grew over 10 MB; desl shows pictures of at most 10 MB."));
    }
    let mime = kind(&bytes[..bytes.len().min(SNIFF)], &real).ok_or_else(|| not_a_picture(path))?;
    Ok((mime, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn a_fifo_named_as_an_image_is_refused_without_waiting() {
        // An open of a FIFO waits for a writer; on the engine's one thread that would stop desl.
        let dir = std::env::temp_dir().join(format!("desl-picture-fifo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(dir.join("x.png")).status().unwrap().success());
        let (tx, rx) = channel();
        let d = dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send((check(&d, "x.png"), read(&d, "x.png").map(|_| ())));
        });
        let (checked, read) = rx.recv_timeout(Duration::from_secs(3)).expect("refused at once, not waiting on the FIFO");
        assert!(checked.unwrap_err().contains("not a file"));
        assert!(read.unwrap_err().contains("not a file"));
    }
}
