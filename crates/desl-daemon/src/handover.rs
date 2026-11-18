//! One daemon handing its work to the next (SPEC.md "Upgrades"). The daemon that leaves starts
//! the new one with a socket for its standard input and sends it, for every running program,
//! the terminal it runs in and what it has drawn. The new daemon answers once it serves, and
//! only then does the old one go. A program never notices: its terminal stays open throughout.
//!
//! On the wire: a header (four bytes of length, then JSON), then for each terminal one byte
//! that carries the descriptor, the bytes it drew, and the unfinished line of its session file.
//! The answer is a header of its own.

use desl_proto::term::Answers;
use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
use serde::{Deserialize, Serialize};
use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The format below. A daemon that is given another refuses it, and the one that leaves stays.
const FORMAT: u32 = 1;

/// How long the daemon that leaves waits for the new one to serve.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// One running program, as the daemon that leaves describes it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub thread_id: String,
    pub agent: String,
    pub pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
    /// When the program started, in milliseconds since 1970.
    pub since_ms: u64,
    /// How many bytes of what it drew follow its terminal,
    log: u64,
    /// and how many of the line of its session file that was read but not finished.
    partial: u64,
}

impl Kept {
    pub fn new(thread_id: &str, agent: &str, pid: Option<u32>, (rows, cols): (u16, u16), since_ms: u64) -> Kept {
        Kept { thread_id: thread_id.to_string(), agent: agent.to_string(), pid, rows, cols, since_ms, log: 0, partial: 0 }
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    format: u32,
    /// What the user's terminal answers to the questions agents ask it.
    answers: Answers,
    kept: Vec<Kept>,
}

#[derive(Serialize, Deserialize)]
struct Serving {
    pid: u32,
}

/// A program on its way out of the daemon that leaves.
pub struct Giving<'a> {
    pub kept: Kept,
    pub master: BorrowedFd<'a>,
    pub log: &'a [u8],
    pub partial: &'a [u8],
}

/// A program as the new daemon received it.
pub struct Given {
    pub kept: Kept,
    pub master: OwnedFd,
    pub log: Vec<u8>,
    pub partial: Vec<u8>,
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

fn framed<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let json = serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))?;
    let len = u32::try_from(json.len()).map_err(|_| invalid("the handover header is too long"))?;
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(&json);
    Ok(out)
}

/// Sends everything to the new daemon.
pub fn give(to: &mut UnixStream, answers: &Answers, all: &[Giving<'_>]) -> io::Result<()> {
    let kept = all.iter().map(|g| Kept { log: g.log.len() as u64, partial: g.partial.len() as u64, ..g.kept.clone() }).collect();
    to.write_all(&framed(&Header { format: FORMAT, answers: answers.clone(), kept })?)?;
    for g in all {
        // The descriptor rides on one byte of its own, so the reader knows which read carries it.
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = SendAncillaryBuffer::new(&mut space);
        let fds = [g.master];
        if !ancillary.push(SendAncillaryMessage::ScmRights(&fds)) {
            return Err(invalid("the terminal did not fit in the message"));
        }
        rustix::net::sendmsg(&*to, &[IoSlice::new(b"T")], &mut ancillary, SendFlags::empty())?;
        to.write_all(g.log)?;
        to.write_all(g.partial)?;
    }
    to.flush()
}

/// The new daemon's answer: its process id, once it serves. An error when it went away
/// instead, or said nothing for [`PATIENCE`].
pub fn served(from: &mut UnixStream) -> io::Result<u32> {
    from.set_read_timeout(Some(PATIENCE))?;
    let mut len = [0u8; 4];
    from.read_exact(&mut len)?;
    let mut json = vec![0u8; u32::from_be_bytes(len) as usize];
    from.read_exact(&mut json)?;
    let serving: Serving = serde_json::from_slice(&json).map_err(|e| invalid(e.to_string()))?;
    Ok(serving.pid)
}

fn read_exact(from: BorrowedFd<'_>, mut buf: &mut [u8]) -> io::Result<()> {
    while !buf.is_empty() {
        match rustix::io::read(from, &mut *buf) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the daemon that hands over went away")),
            Ok(n) => buf = &mut buf[n..],
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn sized(len: u64, what: &str) -> io::Result<Vec<u8>> {
    // No program's record is larger than this: a length beyond it is not a length.
    const MOST: u64 = 256 * 1024 * 1024;
    if len > MOST {
        return Err(invalid(format!("the handover names {len} bytes of {what}")));
    }
    Ok(vec![0u8; usize::try_from(len).map_err(|_| invalid("a length does not fit"))?])
}

/// Receives everything the daemon that leaves sends, on the socket this daemon was started with.
pub fn take(from: impl AsFd) -> io::Result<(Answers, Vec<Given>)> {
    let from = from.as_fd();
    let mut len = [0u8; 4];
    read_exact(from, &mut len)?;
    let mut json = sized(u64::from(u32::from_be_bytes(len)), "header")?;
    read_exact(from, &mut json)?;
    let header: Header = serde_json::from_slice(&json).map_err(|e| invalid(format!("the handover header does not read ({e})")))?;
    if header.format != FORMAT {
        return Err(invalid(format!("the handover is in format {} and this desl reads format {FORMAT}", header.format)));
    }
    let mut all = Vec::new();
    for kept in header.kept {
        let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
        let mut ancillary = RecvAncillaryBuffer::new(&mut space);
        let mut byte = [0u8; 1];
        let got = rustix::net::recvmsg(from, &mut [IoSliceMut::new(&mut byte)], &mut ancillary, RecvFlags::empty())?;
        if got.bytes == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the daemon that hands over went away"));
        }
        let master = ancillary
            .drain()
            .find_map(|m| match m {
                RecvAncillaryMessage::ScmRights(mut fds) => fds.next(),
                _ => None,
            })
            .ok_or_else(|| invalid(format!("no terminal came for {}", kept.thread_id)))?;
        // A program started later must not inherit another one's terminal.
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        let mut log = sized(kept.log, "output")?;
        read_exact(from, &mut log)?;
        let mut partial = sized(kept.partial, "session file")?;
        read_exact(from, &mut partial)?;
        all.push(Given { kept, master, log, partial });
    }
    Ok((header.answers, all))
}

/// Tells the daemon that leaves that this one serves.
pub fn serving(to: impl AsFd, pid: u32) -> io::Result<()> {
    let bytes = framed(&Serving { pid })?;
    let mut rest = &bytes[..];
    while !rest.is_empty() {
        match rustix::io::write(to.as_fd(), rest) {
            Ok(n) => rest = &rest[n..],
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn a_terminal_and_what_it_drew_cross_to_the_new_daemon() {
        let (mut old, new) = UnixStream::pair().unwrap();
        // Two files stand in for two terminals: what is written to the descriptor that arrives
        // must land in the file that was sent.
        let dir = std::env::temp_dir().join(format!("desl-handover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<File> = (0..2).map(|i| File::options().create(true).append(true).open(dir.join(i.to_string())).unwrap()).collect();
        let big = vec![b'x'; 300_000];
        let answers = Answers { fg: Some("fg".into()), ..Answers::default() };
        let sender = {
            let (a, b) = (files[0].try_clone().unwrap(), files[1].try_clone().unwrap());
            let (answers, big) = (answers.clone(), big.clone());
            std::thread::spawn(move || {
                let all = [
                    Giving {
                        kept: Kept::new("t-1", "claude", Some(41), (24, 80), 1000),
                        master: a.as_fd(),
                        log: &big,
                        partial: b"{\"half",
                    },
                    Giving { kept: Kept::new("t-2", "shell", None, (10, 20), 2000), master: b.as_fd(), log: b"", partial: b"" },
                ];
                give(&mut old, &answers, &all).unwrap();
                served(&mut old).unwrap()
            })
        };
        let (got_answers, given) = take(&new).unwrap();
        assert_eq!(got_answers, answers);
        assert_eq!(given.len(), 2);
        assert_eq!(
            (given[0].kept.thread_id.as_str(), given[0].kept.pid, given[0].kept.rows, given[0].kept.cols),
            ("t-1", Some(41), 24, 80)
        );
        assert_eq!(given[0].log, big);
        assert_eq!(given[0].partial, b"{\"half");
        assert_eq!((given[1].kept.agent.as_str(), given[1].kept.since_ms, given[1].log.len()), ("shell", 2000, 0));
        for (i, g) in given.into_iter().enumerate() {
            File::from(g.master).write_all(format!("terminal {i}").as_bytes()).unwrap();
        }
        assert_eq!(std::fs::read_to_string(dir.join("0")).unwrap(), "terminal 0");
        assert_eq!(std::fs::read_to_string(dir.join("1")).unwrap(), "terminal 1");
        serving(&new, 77).unwrap();
        assert_eq!(sender.join().unwrap(), 77);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_daemon_that_goes_away_is_an_error_not_a_wait() {
        let (mut old, new) = UnixStream::pair().unwrap();
        drop(new);
        assert!(served(&mut old).is_err());
    }

    #[test]
    fn another_format_is_refused() {
        let (mut old, new) = UnixStream::pair().unwrap();
        old.write_all(&framed(&Header { format: FORMAT + 1, answers: Answers::default(), kept: Vec::new() }).unwrap()).unwrap();
        let e = take(&new).err().expect("refused");
        assert!(e.to_string().contains("format"), "{e}");
    }
}
