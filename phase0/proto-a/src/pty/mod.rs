//! Prototype B's pane: an agent's own TUI running in a pseudoterminal, parsed by `vt100`.

pub mod keys;
pub mod widget;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Instant;

pub enum PaneMsg {
    Output(usize, Vec<u8>),
    Exited(usize, Option<u32>),
}

pub struct Pane {
    pub parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pub rows: u16,
    pub cols: u16,
    pub last_output: Instant,
    pub last_input: Instant,
    pub exited: Option<Option<u32>>,
    /// Printable characters typed since the last Enter: names the thread, and catches desl's own slash commands.
    pub typed: String,
    /// Text to paste into the agent's prompt once its interface is ready (a brief after a switch).
    pub pending_paste: Option<String>,
    pub spawned_at: std::time::SystemTime,
    pub saw_output: bool,
}

const SCROLLBACK_LINES: usize = 10_000;

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

impl Pane {
    pub fn spawn(idx: usize, program: &str, args: &[&str], cwd: &Path, rows: u16, cols: u16, tx: Sender<PaneMsg>) -> Result<Pane> {
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).context("opening a pty")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("CLAUDE") {
                cmd.env_remove(k);
            }
        }
        let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting `{program}`"))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let writer = pair.master.take_writer().context("pty writer")?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(PaneMsg::Output(idx, buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(PaneMsg::Exited(idx, None));
        });
        Ok(Pane {
            parser: vt100::Parser::new(nz(rows), nz(cols), SCROLLBACK_LINES),
            master: pair.master,
            writer,
            child,
            rows,
            cols,
            last_output: Instant::now(),
            last_input: Instant::now(),
            exited: None,
            typed: String::new(),
            pending_paste: None,
            spawned_at: std::time::SystemTime::now(),
            saw_output: false,
        })
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.last_output = Instant::now();
        self.saw_output = true;
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.last_input = Instant::now();
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if rows == self.rows && cols == self.cols || rows == 0 || cols == 0 {
            return;
        }
        self.rows = rows;
        self.cols = cols;
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.parser.set_size(nz(rows), nz(cols));
    }

    pub fn mark_exited(&mut self) {
        let code = self.child.try_wait().ok().flatten().map(|s| s.exit_code());
        self.exited = Some(code);
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Lines of scrollback currently shown above the live screen.
    pub fn scroll_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let cur = self.scroll_offset() as isize;
        let next = (cur + delta).max(0) as usize;
        self.parser.screen_mut().set_scrollback(next);
    }

    pub fn scroll_to_live(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }
}
