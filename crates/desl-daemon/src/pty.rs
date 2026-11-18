//! The PTY host (SPEC.md "Architecture"): every agent's own interface runs in a pseudoterminal the
//! daemon owns. The host keeps what each one drew, so a client that attaches gets the screen and
//! its scrollback back, and keeps a screen model of it for the agent's cursor and, later, status.

use desl_ir::adapter::{Launch, is_secret};
use filedescriptor::{FileDescriptor, POLLIN, Pipe, pollfd};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use rustix::process::{Pid, Signal};
use std::fs::File;
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// How much of an agent's output is kept for a client that attaches (SPEC.md "Process hygiene").
pub const KEEP: usize = 8 * 1024 * 1024;

/// How many rows above the screen the daemon's own model of it keeps. They are what comes back
/// onto the screen when it grows wider, so the model stays what a client shows.
const HISTORY: usize = 1000;

type OnOutput = Arc<dyn Fn(Vec<u8>) + Send + Sync>;
type OnExit = Arc<dyn Fn() + Send + Sync>;

/// The thread that reads what the program in one terminal draws.
struct Reading {
    /// Set to make the thread stop without reading again. What the program draws from then on
    /// stays in the terminal's own buffer, for whoever reads it next.
    stop: Arc<AtomicBool>,
    /// Written to once `stop` is set: the thread waits on this as well as on the terminal.
    wake: Option<FileDescriptor>,
    thread: std::thread::JoinHandle<()>,
}

pub struct Pty {
    /// The terminal's own end: what is written here the program reads, and what it draws is
    /// read here.
    master: OwnedFd,
    writer: File,
    /// The program, when this daemon started it. One taken over from the daemon before is not
    /// this one's child: it is known by its process id alone.
    child: Option<Box<dyn Child + Send + Sync>>,
    pid: Option<u32>,
    on_output: OnOutput,
    on_exit: OnExit,
    reading: Option<Reading>,
    screen: vt100::Parser,
    /// Everything the agent drew, oldest first, at most [`KEEP`] bytes.
    log: Vec<u8>,
    pub agent: String,
    started: Instant,
    /// When the program started, on the wall clock, for the daemon it is handed to.
    since: SystemTime,
    last_output: Option<Instant>,
    /// Handed to another daemon: the program is no longer this one's to end.
    handed: bool,
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

/// Reads what the program draws until its side closes or `stop` is set; a byte on `woken` ends
/// the wait at once. `stop` is looked at after every wait and before every read, and what was
/// read is always passed on: once the thread has ended, nothing it took from the terminal is
/// missing.
fn read_loop(mut from: File, woken: Option<FileDescriptor>, stop: &AtomicBool, on_output: &OnOutput, on_exit: &OnExit) {
    let mut buf = vec![0u8; 64 * 1024];
    let mut failed = 0;
    loop {
        let mut wait = vec![pollfd { fd: from.as_raw_fd(), events: POLLIN, revents: 0 }];
        // Without a way to be woken, look at `stop` every so often instead.
        let patience = match &woken {
            Some(w) => {
                wait.push(pollfd { fd: w.as_raw_fd(), events: POLLIN, revents: 0 });
                None
            }
            None => Some(Duration::from_millis(100)),
        };
        let ready = filedescriptor::poll(&mut wait, patience);
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match ready {
            Ok(_) if wait[0].revents == 0 => continue,
            Ok(_) => failed = 0,
            // A signal cut the wait short. Anything that keeps failing ends the reading.
            Err(_) if failed < 50 => {
                failed += 1;
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => break,
        }
        match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => on_output(buf[..n].to_vec()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if !stop.load(Ordering::SeqCst) {
        on_exit();
    }
}

fn process(pid: u32) -> Option<Pid> {
    Pid::from_raw(i32::try_from(pid).ok()?)
}

impl Pty {
    /// Starts `launch` in `cwd` at this size, for thread `thread`. Output goes to `on_output` as
    /// it arrives, from a thread of its own; `on_exit` is called when the agent's side closes.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        agent: &str,
        thread: &str,
        launch: &Launch,
        cwd: &Path,
        rows: u16,
        cols: u16,
        on_output: impl Fn(Vec<u8>) + Send + Sync + 'static,
        on_exit: impl Fn() + Send + Sync + 'static,
    ) -> Result<Pty, String> {
        let pair = native_pty_system()
            .openpty(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
            .map_err(|e| format!("Could not open a terminal for {agent} ({e}). Run `desl doctor`."))?;
        let mut cmd = CommandBuilder::new(&launch.program);
        cmd.args(&launch.args);
        cmd.cwd(cwd);
        // The daemon's own PWD is where it was started. A program that trusts PWD over the
        // directory it runs in, as yazi does, would open there.
        cmd.env("PWD", cwd);
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
        // What runs here, an agent or the user's shell, knows it is inside desl and which
        // thread is its own (SPEC.md "Agents drive desl"), as tmux sets $TMUX and Herdr its ids.
        cmd.env(desl_proto::paths::THREAD_VAR, thread);
        // The terminal's own end as a descriptor of this daemon's, which another daemon can be
        // given. Taken before the program starts, so nothing is left running if it fails.
        let no_terminal = |e: &dyn std::fmt::Display| format!("Could not read {agent}'s terminal ({e}). Run `desl doctor`.");
        let raw = pair.master.as_raw_fd().ok_or_else(|| no_terminal(&"it has no descriptor"))?;
        let master = FileDescriptor::dup(&raw).map_err(|e| no_terminal(&e))?.as_fd().try_clone_to_owned().map_err(|e| no_terminal(&e))?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Could not start {agent} ({e}). Check that `{}` is installed and on your PATH.", launch.program))?;
        drop(pair);
        let pid = child.process_id();
        let mut pty = Pty::over(agent, master, Some(child), pid, rows, cols, Arc::new(on_output), Arc::new(on_exit))
            .map_err(|e| format!("Could not write to {agent}'s terminal ({e})."))?;
        pty.read();
        Ok(pty)
    }

    /// A program the daemon before this one started, with its terminal and what it has drawn
    /// so far. Nothing is read from it until [`Pty::read`].
    #[allow(clippy::too_many_arguments)]
    pub fn adopt(
        agent: &str,
        master: OwnedFd,
        pid: Option<u32>,
        rows: u16,
        cols: u16,
        since: SystemTime,
        log: Vec<u8>,
        on_output: impl Fn(Vec<u8>) + Send + Sync + 'static,
        on_exit: impl Fn() + Send + Sync + 'static,
    ) -> Result<Pty, String> {
        let mut pty = Pty::over(agent, master, None, pid, rows, cols, Arc::new(on_output), Arc::new(on_exit))
            .map_err(|e| format!("Could not take over {agent}'s terminal ({e})."))?;
        pty.screen.process(&log);
        pty.log = log;
        pty.since = since;
        // It started long ago: ready for keys.
        pty.started = Instant::now().checked_sub(Duration::from_secs(15)).unwrap_or(pty.started);
        Ok(pty)
    }

    #[allow(clippy::too_many_arguments)]
    fn over(
        agent: &str,
        master: OwnedFd,
        child: Option<Box<dyn Child + Send + Sync>>,
        pid: Option<u32>,
        rows: u16,
        cols: u16,
        on_output: OnOutput,
        on_exit: OnExit,
    ) -> std::io::Result<Pty> {
        let writer = File::from(master.try_clone()?);
        Ok(Pty {
            master,
            writer,
            child,
            pid,
            on_output,
            on_exit,
            reading: None,
            screen: vt100::Parser::new(nz(rows), nz(cols), HISTORY),
            log: Vec::new(),
            agent: agent.to_string(),
            started: Instant::now(),
            since: SystemTime::now(),
            last_output: None,
            handed: false,
        })
    }

    /// Reads what the program draws, on a thread of its own, from now or again after
    /// [`Pty::hold`].
    pub fn read(&mut self) {
        if self.reading.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, on_output, on_exit) = (stop.clone(), self.on_output.clone(), self.on_exit.clone());
        let (woken, wake) = match Pipe::new() {
            Ok(pipe) => (Some(pipe.read), Some(pipe.write)),
            Err(_) => (None, None),
        };
        let thread = match self.master.try_clone() {
            Ok(from) => std::thread::spawn(move || read_loop(File::from(from), woken, &flag, &on_output, &on_exit)),
            // Nothing can be read: the program is as good as gone.
            Err(_) => std::thread::spawn(move || on_exit()),
        };
        self.reading = Some(Reading { stop, wake, thread });
    }

    /// Tells the reader to stop. [`Pty::held`] waits until it has.
    pub fn hold(&mut self) {
        if let Some(r) = &mut self.reading {
            r.stop.store(true, Ordering::SeqCst);
            if let Some(wake) = &mut r.wake {
                let _ = wake.write_all(b"s");
            }
        }
    }

    /// Waits for the reader told to stop. From then on everything it read has been passed on,
    /// and what the program draws stays in the terminal until someone reads again.
    pub fn held(&mut self) {
        if let Some(r) = self.reading.take() {
            let _ = r.thread.join();
        }
    }

    pub fn reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Gives the program up to another daemon: dropping this ends nothing.
    pub fn hand_over(mut self) {
        self.handed = true;
    }

    /// The terminal's own end, for the daemon the program is handed to.
    pub fn master(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn since(&self) -> SystemTime {
        self.since
    }

    /// Records what the agent drew.
    pub fn took(&mut self, bytes: &[u8]) {
        self.last_output = Some(Instant::now());
        self.screen.process(bytes);
        self.log.extend_from_slice(bytes);
        if self.log.len() > KEEP {
            // Drop the oldest quarter at once, so trimming is rare.
            let cut = self.log.len() - KEEP * 3 / 4;
            self.log.drain(..cut);
        }
    }

    /// Started, drawn and quiet: ready for keys. Never longer than 15 seconds.
    pub fn ready(&self) -> bool {
        (self.started.elapsed() >= Duration::from_millis(1200)
            && self.last_output.is_some_and(|t| t.elapsed() >= Duration::from_millis(600)))
            || self.started.elapsed() >= Duration::from_secs(15)
    }

    /// The program asked for pastes to be marked (`CSI ? 2004 h`).
    pub fn bracketed_paste(&self) -> bool {
        self.screen.screen().bracketed_paste()
    }

    pub fn log(&self) -> &[u8] {
        &self.log
    }

    /// The bottom `rows` lines of the agent's screen as text, for telling that it asks something.
    pub fn tail_text(&self, rows: usize) -> String {
        let screen = self.screen.screen();
        let (_, cols) = screen.size();
        let lines: Vec<String> = screen.rows(0, cols.get()).collect();
        drawn_tail(&lines, rows)
    }

    fn alive(&mut self) -> bool {
        match &mut self.child {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => self.pid.and_then(process).is_some_and(|p| rustix::process::test_kill_process(p).is_ok()),
        }
    }

    /// Ends the agent now, as closing its terminal would. Its exit arrives as for any other.
    pub fn end(&mut self) {
        if !self.alive() {
            return;
        }
        match &mut self.child {
            // A hang-up, and a kill when it is still there a quarter of a second later.
            Some(child) => drop(child.kill()),
            None => {
                let Some(pid) = self.pid.and_then(process) else { return };
                let _ = rustix::process::kill_process(pid, Signal::HUP);
                for _ in 0..5 {
                    std::thread::sleep(Duration::from_millis(50));
                    if !self.alive() {
                        return;
                    }
                }
                let _ = rustix::process::kill_process(pid, Signal::KILL);
            }
        }
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
        let size = rustix::termios::Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        let _ = rustix::termios::tcsetwinsize(&self.master, size);
        desl_proto::reflow::resize(&mut self.screen, rows, cols);
    }

    /// The directory the program is in now: where a shell has `cd`-ed to.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        let pid = self.pid?;
        if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
            return Some(p);
        }
        // No /proc (macOS): lsof prints the cwd as a line starting with `n`.
        let out = std::process::Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix('n')).map(std::path::PathBuf::from)
    }

    /// The agent's exit status, once it has one. Only a program this daemon started has one
    /// it can read.
    pub fn exit_code(&mut self) -> Option<u32> {
        self.child.as_mut()?.try_wait().ok().flatten().map(|s| s.exit_code())
    }
}

/// The last `rows` rows of what is drawn: blank rows below it do not count. An agent that draws
/// a short dialog near the top of a tall screen (Codex's folder trust) is asking all the same.
fn drawn_tail(lines: &[String], rows: usize) -> String {
    let end = lines.iter().rposition(|l| !l.trim().is_empty()).map_or(0, |i| i + 1);
    lines[end.saturating_sub(rows)..end].join("\n")
}

impl Drop for Pty {
    /// Ends the agent: hang-up first, as closing a terminal would, then a kill. One handed to
    /// another daemon is left alone.
    fn drop(&mut self) {
        // The reader ends by itself, and says nothing more.
        self.hold();
        if self.handed {
            return;
        }
        self.end();
        // Never block the engine on an agent that will not end.
        for _ in 0..20 {
            if !self.alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
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
        let mut pty =
            Pty::spawn("sh", "t-test", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(Some(b))), move || drop(done.send(None)))
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
        let pty = Pty::spawn("shell", "t-test", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(b)), || {}).expect("sh starts");
        let _ = rx.recv_timeout(Duration::from_secs(5));
        let tmp = Path::new("/tmp").canonicalize().unwrap();
        assert_eq!(pty.cwd().and_then(|p| p.canonicalize().ok()), Some(tmp));
    }

    #[test]
    fn what_runs_in_a_thread_knows_its_thread() {
        let launch = Launch {
            program: "sh".into(),
            args: vec!["-c".into(), "printf 'thread=%s;\\n' \"$DESL_THREAD\"; sleep 5".into()],
            env_remove: Vec::new(),
            keep: Vec::new(),
            session_id: None,
        };
        let (tx, rx) = channel::<Vec<u8>>();
        let _pty = Pty::spawn("shell", "t-7f3a", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(b)), || {}).expect("sh starts");
        let mut seen = String::new();
        while let Ok(b) = rx.recv_timeout(Duration::from_secs(5)) {
            seen.push_str(&String::from_utf8_lossy(&b));
            if seen.contains(';') {
                break;
            }
        }
        assert!(seen.contains("thread=t-7f3a;"), "{seen:?}");
    }

    #[test]
    fn what_runs_in_a_thread_is_told_where_it_starts() {
        let launch = Launch { program: "env".into(), args: Vec::new(), env_remove: Vec::new(), keep: Vec::new(), session_id: None };
        let (tx, rx) = channel::<Option<Vec<u8>>>();
        let done = tx.clone();
        let _pty =
            Pty::spawn("files", "t-7f3a", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(Some(b))), move || drop(done.send(None)))
                .expect("env starts");
        let mut seen = String::new();
        while let Ok(Some(b)) = rx.recv_timeout(Duration::from_secs(5)) {
            seen.push_str(&String::from_utf8_lossy(&b));
        }
        assert!(seen.lines().any(|l| l.trim_end() == "PWD=/"), "PWD is the daemon's own, not the thread's: {seen:?}");
    }

    #[test]
    fn a_new_width_wraps_what_was_drawn_again() {
        let launch = Launch {
            program: "sh".into(),
            args: vec!["-c".into(), "printf 'aaaaaaaaaabbbbbbbbbbcccccccccc\\n'; sleep 5".into()],
            env_remove: Vec::new(),
            keep: Vec::new(),
            session_id: None,
        };
        let (tx, rx) = channel::<Vec<u8>>();
        let mut pty = Pty::spawn("shell", "t-test", &launch, Path::new("/"), 24, 20, move |b| drop(tx.send(b)), || {}).expect("sh starts");
        while let Ok(b) = rx.recv_timeout(Duration::from_secs(5)) {
            pty.took(&b);
            if pty.tail_text(4).contains("cccccccccc") {
                break;
            }
        }
        assert_eq!(pty.tail_text(4), "aaaaaaaaaabbbbbbbbbb\ncccccccccc");
        pty.resize(24, 10);
        assert_eq!(pty.size(), (24, 10));
        assert_eq!(pty.tail_text(4), "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc", "the row that no longer fits is not cut off");
        pty.resize(24, 40);
        assert_eq!(pty.tail_text(4), "aaaaaaaaaabbbbbbbbbbcccccccccc");
    }

    #[test]
    fn a_reader_that_is_held_leaves_what_comes_next_in_the_terminal() {
        let launch = Launch { program: "cat".into(), args: Vec::new(), env_remove: Vec::new(), keep: Vec::new(), session_id: None };
        let (tx, rx) = channel::<Vec<u8>>();
        let mut pty = Pty::spawn("files", "t-test", &launch, Path::new("/"), 24, 80, move |b| drop(tx.send(b)), || {}).expect("cat starts");
        let mut seen = String::new();
        let hear = |pty: &mut Pty, seen: &mut String, what: &str| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !seen.contains(what) {
                assert!(Instant::now() < deadline, "never drew {what:?}: {seen:?}");
                if let Ok(b) = rx.recv_timeout(Duration::from_millis(50)) {
                    pty.took(&b);
                    seen.push_str(&String::from_utf8_lossy(&b));
                }
            }
        };
        pty.write(b"one\r");
        hear(&mut pty, &mut seen, "one\r\none");
        pty.hold();
        pty.held();
        assert!(!pty.reading());
        // The program draws this while nobody reads.
        pty.write(b"two\r");
        std::thread::sleep(Duration::from_millis(300));
        assert!(rx.try_recv().is_err(), "nothing is read while the reader is held");
        pty.read();
        hear(&mut pty, &mut seen, "two\r\ntwo");
        // The same terminal, as another daemon holds it: it writes, and this one's reader hears.
        let other = Pty::adopt(
            "files",
            pty.master().try_clone_to_owned().unwrap(),
            pty.pid(),
            24,
            80,
            pty.since(),
            pty.log().to_vec(),
            |_| {},
            || {},
        )
        .expect("adopted");
        assert!(other.tail_text(4).contains("two"), "what it drew came along: {:?}", other.tail_text(4));
        let mut other = other;
        other.write(b"three\r");
        hear(&mut pty, &mut seen, "three\r\nthree");
        // Given up, it ends nothing.
        other.hand_over();
        pty.write(b"four\r");
        hear(&mut pty, &mut seen, "four\r\nfour");
    }

    #[test]
    fn the_tail_is_the_bottom_of_what_is_drawn() {
        let mut lines: Vec<String> =
            ["", "  Trust this folder?", "  1. Trust and continue", "  2. Back"].iter().map(|s| s.to_string()).collect();
        lines.extend(std::iter::repeat_n(String::new(), 30));
        assert_eq!(drawn_tail(&lines, 2), "  1. Trust and continue\n  2. Back");
        assert!(drawn_tail(&lines, 14).contains("Trust this folder?"));
        assert_eq!(drawn_tail(&[String::new(), "  ".into()], 14), "");
    }
}
