//! The client's connection to the host: start it if needed, attach, stream frames in, send commands out.

use crate::app::Msg;
use crate::host;
use crate::log::{Command, Frame};
use crate::paths;
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub struct HostLink {
    writer: UnixStream,
}

/// Connects to the project's host, or returns None when there is none and `start` is false.
pub fn connect(cwd: &Path, start: bool) -> Result<Option<UnixStream>> {
    let sock = paths::socket_path(cwd);
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(Some(s));
    }
    if !start {
        return Ok(None);
    }
    host::spawn_detached(cwd)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(&sock) {
            return Ok(Some(s));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    anyhow::bail!(
        "the desl host did not start within five seconds; see {}",
        paths::host_stderr_path(cwd).display()
    )
}

impl HostLink {
    pub fn attach(cwd: &Path, tx: Sender<Msg>) -> Result<HostLink> {
        let stream = connect(cwd, true)?.context("no host")?;
        let reader = stream.try_clone()?;
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else { break };
                if let Ok(frame) = serde_json::from_str::<Frame>(&line)
                    && tx.send(Msg::Host(frame)).is_err()
                {
                    return;
                }
            }
            let _ = tx.send(Msg::HostGone);
        });
        let mut link = HostLink { writer: stream };
        link.send(&Command::Attach);
        Ok(link)
    }

    pub fn send(&mut self, cmd: &Command) {
        if let Ok(mut line) = serde_json::to_string(cmd) {
            line.push('\n');
            let _ = self.writer.write_all(line.as_bytes());
        }
    }
}

/// One request, one reply: used by `stop` and `status`.
pub fn one_shot(cwd: &Path, cmd: &Command, want_reply: bool) -> Result<Option<Frame>> {
    let Some(mut stream) = connect(cwd, false)? else { return Ok(None) };
    let mut line = serde_json::to_string(cmd)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    if !want_reply {
        return Ok(None);
    }
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut buf = String::new();
    reader.read_line(&mut buf)?;
    Ok(serde_json::from_str::<Frame>(&buf).ok())
}
