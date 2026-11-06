//! The PTY host (SPEC.md "Architecture"): every agent's own interface runs in a pseudoterminal the
//! daemon owns. The host keeps what each one drew, so a client that attaches gets the screen and
//! its scrollback back, and keeps a screen model of it for the agent's cursor and, later, status.

use desl_ir::adapter::{Launch, is_secret};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;

/// How much of an agent's output is kept for a client that attaches (SPEC.md "Process hygiene").
pub const KEEP: usize = 8 * 1024 * 1024;

pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    screen: vt100::Parser,
    /// Everything the agent drew, oldest first, at most [`KEEP`] bytes.
    log: Vec<u8>,
    pub agent: String,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Pty {
    /// Starts `launch` in `cwd` at this size. Output goes to `on_output` as it arrives, from a
    /// thread of its own; `on_exit` is called once when the agent's side closes.
    pub fn spawn(
        agent: &str,
        launch: &Launch,
        cwd: &Path,
        rows: u16,
        cols: u16,
        on_output: impl Fn(Vec<u8>) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Pty, String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Could not open a terminal for {agent} ({e}). Run `desl doctor`."))?;
        let mut cmd = CommandBuilder::new(&launch.program);
        cmd.args(&launch.args);
        cmd.cwd(cwd);
        for var in &launch.env_remove {
            cmd.env_remove(var);
        }
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if is_secret(&name) && !launch.keep.iter().any(|k| *k == name) {
                cmd.env_remove(&key);
            }
        }
        if std::env::var_os("TERM").is_none() {
            cmd.env("TERM", "xterm-256color");
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Could not start {agent} ({e}). Check that `{}` is installed and on your PATH.", launch.program))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|e| format!("Could not read {agent}'s terminal ({e})."))?;
        let writer = pair.master.take_writer().map_err(|e| format!("Could not write to {agent}'s terminal ({e})."))?;
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => on_output(buf[..n].to_vec()),
                }
            }
            on_exit();
        });
        Ok(Pty {
            master: pair.master,
            writer,
            child,
            screen: vt100::Parser::new(nz(rows), nz(cols), 0),
            log: Vec::new(),
            agent: agent.to_string(),
        })
    }

    /// Records what the agent drew.
    pub fn took(&mut self, bytes: &[u8]) {
        self.screen.process(bytes);
        self.log.extend_from_slice(bytes);
        if self.log.len() > KEEP {
            // Drop the oldest quarter at once, so trimming is rare.
            let cut = self.log.len() - KEEP * 3 / 4;
            self.log.drain(..cut);
        }
    }

    pub fn log(&self) -> &[u8] {
        &self.log
    }

    /// The bottom `rows` lines of the agent's screen as text, for telling that it asks something.
    pub fn tail_text(&self, rows: usize) -> String {
        let screen = self.screen.screen();
        let (_, cols) = screen.size();
        let lines: Vec<String> = screen.rows(0, cols.get()).collect();
        lines[lines.len().saturating_sub(rows)..].join("\n")
    }

    /// The agent's terminal size, (rows, columns).
    pub fn size(&self) -> (u16, u16) {
        let (r, c) = self.screen.screen().size();
        (r.get(), c.get())
    }

    /// The agent's cursor on its own screen, (row, column) from 0.
    pub fn cursor(&self) -> (u16, u16) {
        self.screen.screen().cursor_position()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || self.screen.screen().size() == (nz(rows), nz(cols)) {
            return;
        }
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.screen.screen_mut().set_size(nz(rows), nz(cols));
    }

    /// The directory the program is in now: where a shell has `cd`-ed to.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        let pid = self.child.process_id()?;
        if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
            return Some(p);
        }
        // No /proc (macOS): lsof prints the cwd as a line starting with `n`.
        let out = std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix('n')).map(std::path::PathBuf::from)
    }

    /// The agent's exit status, once it has one.
    pub fn exit_code(&mut self) -> Option<u32> {
        self.child.try_wait().ok().flatten().map(|s| s.exit_code())
    }
}

impl Drop for Pty {
    /// Ends the agent: hang-up first, as closing a terminal would, then a kill.
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        // Never block the engine on an agent that will not end.
        for _ in 0..20 {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    #[test]
    fn an_agent_draws_into_the_log_and_hears_what_is_written() {
        let launch = Launch {
            program: "sh".into(),
            args: vec!["-c".into(), "printf 'ready\\n'; read line; printf 'got %s\\n' \"$line\"".into()],
            env_remove: Vec::new(),
            keep: Vec::new(),
            session_id: None,
        };
        let (tx, rx) = channel::<Option<Vec<u8>>>();
        let done = tx.clone();
        let mut pty = Pty::spawn("sh", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(Some(b))), move || drop(done.send(None)))
            .expect("sh starts in a pty");
        let mut seen = Vec::new();
        let mut wrote = false;
        while let Ok(Some(b)) = rx.recv_timeout(Duration::from_secs(5)) {
            pty.took(&b);
            seen.extend_from_slice(&b);
            if !wrote && String::from_utf8_lossy(&seen).contains("ready") {
                pty.write(b"hello\r");
                wrote = true;
            }
            if String::from_utf8_lossy(&seen).contains("got hello") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(pty.log()).contains("got hello"), "{:?}", String::from_utf8_lossy(pty.log()));
        assert!(pty.cursor().0 >= 2);
    }

    #[test]
    fn a_shell_says_where_it_has_gone() {
        let launch = Launch {
            program: "sh".into(),
            args: vec!["-c".into(), "cd /tmp && printf 'there\\n' && sleep 5".into()],
            env_remove: Vec::new(),
            keep: Vec::new(),
            session_id: None,
        };
        let (tx, rx) = channel::<Vec<u8>>();
        let pty = Pty::spawn("shell", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(b)), || {}).expect("sh starts");
        let _ = rx.recv_timeout(Duration::from_secs(5));
        let tmp = Path::new("/tmp").canonicalize().unwrap();
        assert_eq!(pty.cwd().and_then(|p| p.canonicalize().ok()), Some(tmp));
    }
}
