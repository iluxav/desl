//! The daemon over its real socket: hello, a refused protocol version, and a clean stop.

use desl_proto::paths::Paths;
use desl_proto::{Command, PROTOCOL, Reply, Request, ServerFrame, read_frame, write_frame};
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn paths(name: &str) -> (Paths, PathBuf) {
    // Short on purpose: a Unix socket path has about a hundred bytes.
    let dir = std::env::temp_dir().join(format!("desl-srv-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = Paths {
        db: dir.join("workspace.db"),
        blobs: dir.join("blobs"),
        log: dir.join("daemon.log"),
        socket: dir.join("s.sock"),
        config: dir.join("config.toml"),
    };
    (p, dir)
}

fn connect(p: &Paths) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(s) = UnixStream::connect(&p.socket) {
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            return s;
        }
        assert!(Instant::now() < deadline, "the daemon did not come up");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn call(s: &mut UnixStream, id: u64, command: Command) -> (Option<Reply>, Option<String>) {
    write_frame(s, &Request { id, command }).unwrap();
    let mut reader = BufReader::new(s.try_clone().unwrap());
    loop {
        match read_frame::<_, ServerFrame>(&mut reader).unwrap() {
            Some(ServerFrame::Reply { id: got, ok, error }) if got == id => return (ok, error),
            Some(_) => {}
            None => panic!("the daemon hung up"),
        }
    }
}

#[test]
fn a_client_says_hello_and_a_stop_removes_the_socket_before_the_daemon_ends() {
    let (p, dir) = paths("stop");
    let serving = {
        let p = p.clone();
        std::thread::spawn(move || desl_daemon::server::serve(&p))
    };
    let mut s = connect(&p);
    let (ok, _) = call(&mut s, 1, Command::Hello { protocol: PROTOCOL, client: "test".into() });
    assert!(matches!(ok, Some(Reply::Hello { protocol: PROTOCOL, .. })));

    // A client from another major version is refused with a sentence that says what to do.
    let mut old = connect(&p);
    let (ok, error) = call(&mut old, 1, Command::Hello { protocol: PROTOCOL + 1, client: "test".into() });
    assert!(ok.is_none());
    assert!(error.unwrap().contains("desl daemon restart"));

    let (ok, _) = call(&mut s, 2, Command::Workspace);
    assert!(matches!(ok, Some(Reply::Workspace(_))));

    write_frame(&mut s, &Request { id: 3, command: Command::Shutdown }).unwrap();
    // The connection closes when the daemon has finished, never before.
    let mut reader = BufReader::new(s.try_clone().unwrap());
    while let Ok(Some(_)) = read_frame::<_, ServerFrame>(&mut reader) {}
    assert!(!p.socket.exists(), "the socket is gone once the daemon has stopped");
    serving.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_second_daemon_on_the_same_socket_is_refused_with_a_sentence() {
    let (p, dir) = paths("second");
    let serving = {
        let p = p.clone();
        std::thread::spawn(move || desl_daemon::server::serve(&p))
    };
    let mut s = connect(&p);
    let err = desl_daemon::server::serve(&p).unwrap_err().to_string();
    assert!(err.starts_with("A desl daemon is already running on"), "{err}");
    write_frame(&mut s, &Request { id: 1, command: Command::Shutdown }).unwrap();
    serving.join().unwrap().unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_socket_path_too_long_for_the_system_is_refused_with_a_sentence() {
    let (mut p, dir) = paths("long");
    p.socket = dir.join("x".repeat(120)).join("s.sock");
    let err = desl_daemon::server::serve(&p).unwrap_err().to_string();
    assert!(err.contains("is too long for a Unix socket") && err.contains("DESL_SOCKET"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}
