//! Prototype B's continuity source: the agents' own session files, tailed as they grow.
//! The only code that knows where those files live and what their lines mean (AGENTS.md rule 4).

pub mod claude;
pub mod codex;
#[cfg(test)]
mod tests;

use crate::agent::AgentEvent;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a transcript line adds to a thread.
pub enum TranscriptEvent {
    /// The user typed a prompt into the agent's own interface.
    User(String),
    /// The agent began a turn (useful for status; the screen heuristics are the fallback).
    TurnStarted,
    Agent(AgentEvent),
}

/// Incremental reader of a growing JSONL file; whole lines only.
pub struct Tail {
    pub path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    pub fn new(path: PathBuf) -> Self {
        Tail { path, offset: 0, partial: Vec::new() }
    }

    pub fn read_new_lines(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let Ok(mut f) = File::open(&self.path) else { return out };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len <= self.offset {
            return out;
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return out;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_err() {
            return out;
        }
        self.offset = len;
        self.partial.extend_from_slice(&buf);
        while let Some(nl) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=nl).collect();
            if let Ok(s) = String::from_utf8(line) {
                let s = s.trim_end().to_string();
                if !s.is_empty() {
                    out.push(s);
                }
            }
        }
        out
    }
}

/// Newest file under `dir` matching `pred`, modified at or after `since`, not in `taken`.
pub fn newest_since(dir: &Path, since: SystemTime, taken: &[PathBuf], pred: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let Ok(rd) = std::fs::read_dir(dir) else { return None };
    for entry in rd.flatten() {
        let path = entry.path();
        if !pred(&path) || taken.contains(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if modified < since {
            continue;
        }
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, p)| p)
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}
