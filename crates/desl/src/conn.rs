//! The client's connection to the daemon: start it if needed, say hello, send commands, read
//! frames. The TUI reads frames on a thread; the CLI asks one question at a time.

use anyhow::{Context, Result, bail};
use desl_proto::paths::Paths;
use desl_proto::{Command, PROTOCOL, Reply, Request, ServerFrame, read_frame, write_frame};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub struct Conn {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
    next_id: u64,
    /// The daemon's own version and process id, from its hello.
    pub daemon_version: String,
    pub daemon_pid: u32,
}

/// The sending half, for the TUI: commands go out, replies arrive later on the reader thread.
pub struct Link {
    writer: UnixStream,
    next_id: u64,
}

fn socket(paths: &Paths, start: bool) -> Result<Option<UnixStream>> {
    if let Ok(s) = UnixStream::connect(&paths.socket) {
        return Ok(Some(s));
    }
    if !start {
        return Ok(None);
    }
    let exe = std::env::current_exe().context("finding the desl binary")?;
    let mut daemon = desl_daemon::server::spawn_detached(&exe, paths)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(&paths.socket) {
            return Ok(Some(s));
        }
        // A daemon that is already gone will never answer: say what it said.
        if let Ok(Some(_)) = daemon.try_wait() {
            if let Ok(s) = UnixStream::connect(&paths.socket) {
                return Ok(Some(s));
            }
            let said =
                std::fs::read_to_string(&paths.log).ok().and_then(|t| t.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string));
            match said {
                Some(line) => bail!("The desl daemon could not start. {}", line.trim()),
                None => bail!("The desl daemon could not start and left no message. Its log is {}.", paths.log.display()),
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("The desl daemon did not start within five seconds. Its log is {}.", paths.log.display())
}

impl Conn {
    /// Connects and says hello. `Ok(None)` when no daemon is running and `start` is false.
    pub fn open(paths: &Paths, start: bool, client: &str) -> Result<Option<Conn>> {
        let Some(stream) = socket(paths, start)? else { return Ok(None) };
        let reader = BufReader::new(stream.try_clone()?);
        let mut conn = Conn { writer: stream, reader, next_id: 0, daemon_version: String::new(), daemon_pid: 0 };
        match conn.call(Command::Hello { protocol: PROTOCOL, client: client.to_string() })? {
            Reply::Hello { version, pid, .. } => {
                conn.daemon_version = version;
                conn.daemon_pid = pid;
            }
            other => bail!("The desl daemon answered hello with {other:?}. Run `desl daemon restart`."),
        }
        Ok(Some(conn))
    }

    pub fn send(&mut self, command: Command) -> Result<u64> {
        self.next_id += 1;
        write_frame(&mut self.writer, &Request { id: self.next_id, command })
            .context("The desl daemon closed the connection. Run desl again to reconnect.")?;
        Ok(self.next_id)
    }

    /// How long `read` waits for a frame before it gives up.
    pub fn set_read_timeout(&mut self, timeout: Duration) {
        let _ = self.reader.get_ref().set_read_timeout(Some(timeout));
    }

    /// The next frame, or None when the daemon closed the connection.
    pub fn read(&mut self) -> Result<Option<ServerFrame>> {
        read_frame(&mut self.reader).context("reading from the desl daemon")
    }

    /// One command, one reply. Events and stream frames that arrive in between are skipped.
    /// A daemon error comes back as `Err` with the daemon's sentence.
    pub fn call(&mut self, command: Command) -> Result<Reply> {
        let id = self.send(command)?;
        loop {
            match self.read()? {
                Some(ServerFrame::Reply { id: got, ok, error }) if got == id => {
                    return match (ok, error) {
                        (Some(reply), _) => Ok(reply),
                        (None, Some(sentence)) => Err(anyhow::Error::msg(sentence)),
                        (None, None) => bail!("The desl daemon sent an empty reply. Run `desl daemon restart`."),
                    };
                }
                Some(_) => {}
                None => bail!("The desl daemon closed the connection. Run desl again to reconnect."),
            }
        }
    }

    /// Splits into the sending half and the reading half.
    pub fn split(self) -> (Link, BufReader<UnixStream>) {
        (Link { writer: self.writer, next_id: self.next_id }, self.reader)
    }
}

impl Link {
    /// A link whose commands go nowhere, for tests that feed the client frames by hand.
    #[cfg(test)]
    pub fn detached() -> (Link, UnixStream) {
        let (writer, peer) = UnixStream::pair().expect("a socket pair");
        (Link { writer, next_id: 0 }, peer)
    }

    /// Sends a command and returns its request id. A lost connection is reported by the reader.
    pub fn request(&mut self, command: Command) -> u64 {
        self.next_id += 1;
        let _ = write_frame(&mut self.writer, &Request { id: self.next_id, command });
        self.next_id
    }
}
