//! Where desl keeps things (SPEC.md "Distribution, install and configuration", Paths).
//! `DESL_HOME` moves everything into one directory; `DESL_SOCKET` points at another daemon.

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    /// The store: `workspace.db`.
    pub db: PathBuf,
    /// Large tool outputs, content-addressed.
    pub blobs: PathBuf,
    pub log: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

impl Paths {
    pub fn resolve() -> Paths {
        let mut p = match std::env::var_os("DESL_HOME").filter(|v| !v.is_empty()).map(PathBuf::from) {
            Some(dir) => Paths {
                db: dir.join("workspace.db"),
                blobs: dir.join("blobs"),
                log: dir.join("daemon.log"),
                socket: dir.join("desl.sock"),
                config: dir.join("config.toml"),
            },
            None => {
                let data = xdg("XDG_DATA_HOME", ".local/share").join("desl");
                let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
                    // No runtime directory (macOS, bare shells): a private directory under the state path.
                    .unwrap_or_else(|| xdg("XDG_STATE_HOME", ".local/state").join("desl"));
                Paths {
                    db: data.join("workspace.db"),
                    blobs: data.join("blobs"),
                    log: xdg("XDG_STATE_HOME", ".local/state").join("desl").join("daemon.log"),
                    socket: runtime.join("desl.sock"),
                    config: xdg("XDG_CONFIG_HOME", ".config").join("desl").join("config.toml"),
                }
            }
        };
        if let Some(sock) = std::env::var_os("DESL_SOCKET").filter(|v| !v.is_empty()) {
            p.socket = PathBuf::from(sock);
        }
        p
    }
}
