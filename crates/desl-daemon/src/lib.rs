//! The desl daemon (SPEC.md "Architecture"): owns the store, every agent process and all
//! thread state, and serves clients over one Unix socket.

pub mod engine;
pub mod git;
pub mod projector;
pub mod pty;
pub mod server;
pub mod store;
