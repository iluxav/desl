//! The client on the native route (SPEC.md "Interface design"): the shown thread's agent draws
//! straight onto this terminal and gets every key, except the prefix and the key after it.

use crate::conn::Conn;
use crate::rawkeys;
use anyhow::{Context, Result, bail};
use desl_proto::{Change, Command, Reply, ServerFrame, b64, read_frame, term};
use ratatui::crossterm::terminal;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

enum Msg {
    Stdin(Vec<u8>),
    Frame(ServerFrame),
    DaemonGone,
}

fn out(b: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

fn title(agent: &str, thread_id: &str) {
    out(format!("\x1b]0;desl · {agent} · {thread_id}\x07").as_bytes());
}

/// The terminal in raw mode for as long as this lives, put back however the client ends.
struct Raw;

impl Raw {
    fn enter() -> Result<Raw> {
        terminal::enable_raw_mode().context("Could not put the terminal in raw mode. Run desl from a terminal")?;
        Ok(Raw)
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        out(term::RESET);
        let _ = terminal::disable_raw_mode();
    }
}

/// Asks the terminal what agents will ask it, so the daemon can answer for it while no client
/// shows an agent. Gives up after half a second; a terminal that answers nothing is fine.
fn probe(rx: &mpsc::Receiver<Msg>) -> (term::Answers, Vec<u8>) {
    let mut answers = term::Answers::default();
    out(term::PROBE);
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut got: Vec<u8> = Vec::new();
    while Instant::now() < deadline {
        if let Ok(Msg::Stdin(b)) = rx.recv_timeout(Duration::from_millis(20)) {
            got.extend_from_slice(&b);
            if answers.take(&got) {
                return (answers, Vec::new());
            }
        }
    }
    (answers, got)
}

/// Shows the workspace's thread for `cwd` until the user detaches. Returns the line to print
/// once the terminal is back to normal.
pub fn run(mut conn: Conn, cwd: PathBuf) -> Result<Option<String>> {
    let thread_id = match conn.call(Command::Subscribe { cwd: Some(cwd.to_string_lossy().into_owned()) })? {
        Reply::Workspace(ws) => match ws.focus {
            Some(t) => t,
            None => bail!("desl found no thread to open here. Run `desl new` to start one"),
        },
        other => bail!("The desl daemon answered with {other:?}. Run `desl daemon restart`"),
    };
    let raw = Raw::enter()?;
    let (tx, rx) = mpsc::channel::<Msg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = [0u8; 8192];
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 || tx.send(Msg::Stdin(buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
    }
    let (answers, _) = probe(&rx);
    let (mut cols, mut rows) = terminal::size().unwrap_or((80, 24));
    let mut agent = match conn.call(Command::Attach { thread_id: thread_id.clone(), rows, cols, answers }) {
        Ok(Reply::Attached { agent, replay, .. }) => {
            out(&term::replace_screen(&b64::decode(&replay).unwrap_or_default()));
            agent
        }
        Ok(other) => {
            drop(raw);
            bail!("The desl daemon answered attach with {other:?}. Run `desl daemon restart`")
        }
        Err(e) => {
            drop(raw);
            return Err(e);
        }
    };
    title(&agent, &thread_id);
    let (mut link, mut reader) = conn.split();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame::<_, ServerFrame>(&mut reader) {
                if tx.send(Msg::Frame(frame)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::DaemonGone);
        });
    }

    let mut armed: Option<Vec<u8>> = None;
    // A switch in flight: its request id, and the keys typed meanwhile, for the agent that wins.
    let mut switching: Option<u64> = None;
    let mut held: Vec<u8> = Vec::new();
    // A sentence shown in the title until the next key.
    let mut notice = false;
    let mut swallow = false;
    let farewell = loop {
        let msg = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
        };
        match msg {
            Some(Msg::Stdin(b)) => {
                if notice {
                    notice = false;
                    title(&agent, &thread_id);
                }
                let mut to_agent: Vec<u8> = Vec::new();
                let mut detach = false;
                let mut switch = false;
                let mut at = 0;
                while at < b.len() {
                    let Some((k, n)) = rawkeys::next(&b[at..]) else {
                        to_agent.extend_from_slice(&b[at..]);
                        break;
                    };
                    let raw_key = &b[at..at + n];
                    at += n;
                    if let Some(prefix) = armed.take() {
                        if k.noise {
                            armed = Some(prefix);
                            continue;
                        }
                        swallow = true;
                        match k.code {
                            _ if rawkeys::is_prefix(&k) => to_agent.extend_from_slice(raw_key),
                            Some(c) if c == u32::from(b'd') => detach = true,
                            Some(c) if c == u32::from(b's') => switch = true,
                            _ => {
                                swallow = false;
                                to_agent.extend_from_slice(&prefix);
                                to_agent.extend_from_slice(raw_key);
                            }
                        }
                        continue;
                    }
                    if k.noise && swallow {
                        continue;
                    }
                    swallow = false;
                    if rawkeys::is_prefix(&k) {
                        armed = Some(raw_key.to_vec());
                        continue;
                    }
                    to_agent.extend_from_slice(raw_key);
                }
                if switching.is_some() {
                    held.extend_from_slice(&to_agent);
                } else if !to_agent.is_empty() {
                    link.request(Command::Input { thread_id: thread_id.clone(), data: b64::encode(&to_agent) });
                }
                if switch && switching.is_none() {
                    let to = if agent == "claude" { "codex" } else { "claude" };
                    out(format!("\x1b]0;desl · switching to {to}…\x07").as_bytes());
                    switching = Some(link.request(Command::Switch { thread_id: thread_id.clone(), agent: to.to_string() }));
                }
                if detach {
                    break Some("Detached. The agents keep working; run desl to come back.".to_string());
                }
            }
            Some(Msg::Frame(ServerFrame::Pty { thread_id: t, data })) if t == thread_id => {
                if let Some(bytes) = b64::decode(&data) {
                    out(&term::without_titles(&bytes));
                    // tmux takes an APC (a kitty graphics query, say) as a title: put desl's back.
                    if bytes.windows(2).any(|w| w == b"\x1b_") {
                        title(&agent, &thread_id);
                    }
                }
            }
            Some(Msg::Frame(ServerFrame::PtyEnded { thread_id: t, .. })) if t == thread_id => {
                break Some(format!("{agent} ended. Run desl to open the thread again; it resumes the same conversation."));
            }
            Some(Msg::Frame(ServerFrame::Event { change: Change::Bye })) => {
                break Some("The desl daemon was stopped. Run desl to start it again; the thread resumes where it was.".to_string());
            }
            Some(Msg::Frame(ServerFrame::PtyReplace { thread_id: t, agent: to, data })) if t == thread_id => {
                out(&term::replace_screen(&b64::decode(&data).unwrap_or_default()));
                agent = to;
                title(&agent, &thread_id);
                switching = None;
                if !held.is_empty() {
                    link.request(Command::Input { thread_id: thread_id.clone(), data: b64::encode(&std::mem::take(&mut held)) });
                }
            }
            Some(Msg::Frame(ServerFrame::Reply { id, error: Some(sentence), .. })) if Some(id) == switching => {
                switching = None;
                out(format!("\x1b]0;desl · {agent} · {sentence}\x07").as_bytes());
                notice = true;
                if !held.is_empty() {
                    link.request(Command::Input { thread_id: thread_id.clone(), data: b64::encode(&std::mem::take(&mut held)) });
                }
            }
            Some(Msg::Frame(ServerFrame::Notice { thread_id: t, text })) if t == thread_id => {
                switching = None;
                out(format!("\x1b]0;desl · {agent} · {text}\x07").as_bytes());
                notice = true;
                if !held.is_empty() {
                    link.request(Command::Input { thread_id: thread_id.clone(), data: b64::encode(&std::mem::take(&mut held)) });
                }
            }
            Some(Msg::Frame(_)) => {}
            Some(Msg::DaemonGone) => {
                break Some("The desl daemon closed the connection. Run desl to reconnect; the thread resumes where it was.".to_string());
            }
            None => {}
        }
        if let Ok((c, r)) = terminal::size()
            && (c, r) != (cols, rows)
            && c > 0
            && r > 0
        {
            (cols, rows) = (c, r);
            link.request(Command::Resize { thread_id: thread_id.clone(), rows, cols });
        }
    };
    drop(raw);
    out(b"\r\n");
    // Give the last keys a moment to reach the daemon before the socket closes.
    std::thread::sleep(Duration::from_millis(30));
    Ok(farewell)
}
