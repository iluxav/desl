//! Where the prototype keeps its socket and its log, per project directory.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub fn project_key(cwd: &Path) -> String {
    let canon = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    canon.hash(&mut h);
    format!("{:08x}", h.finish() & 0xffff_ffff)
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
        let uid = std::fs::metadata("/proc/self").map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.uid()
        });
        PathBuf::from(format!("/tmp/desl-proto-a-{}", uid.unwrap_or(0)))
    })
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir)
}

/// `proto` is "a" or "b": each prototype keeps its own hosts, sockets and logs.
pub fn socket_path_for(cwd: &Path, proto: &str) -> PathBuf {
    runtime_dir().join(format!("desl-proto-{proto}")).join(format!("{}.sock", project_key(cwd)))
}

pub fn log_path_for(cwd: &Path, proto: &str) -> PathBuf {
    state_dir().join(format!("desl-proto-{proto}")).join(format!("{}.jsonl", project_key(cwd)))
}

pub fn host_stderr_path_for(cwd: &Path, proto: &str) -> PathBuf {
    state_dir().join(format!("desl-proto-{proto}")).join(format!("{}.host.log", project_key(cwd)))
}

pub fn socket_path(cwd: &Path) -> PathBuf {
    socket_path_for(cwd, "a")
}

pub fn log_path(cwd: &Path) -> PathBuf {
    log_path_for(cwd, "a")
}

pub fn host_stderr_path(cwd: &Path) -> PathBuf {
    host_stderr_path_for(cwd, "a")
}

/// Where the launcher records which prototype each session ran.
pub fn launcher_log() -> PathBuf {
    state_dir().join("desl-proto").join("launches.jsonl")
}
