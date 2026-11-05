# Changelog

All notable changes to desl are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semantic versioning
once there is a release.

## [Unreleased]

Phase 1, "the real thing", in progress. Nothing has been released.

### Added

- The `desl` binary: a daemon that owns all state and a thin client, over one Unix socket.
- The event IR and the Thread and Binding types (`desl-ir`), and the daemon protocol
  (`desl-proto`, documented in `docs/protocol.md`).
- The store: SQLite in WAL mode, one row per event, large tool outputs as content-addressed
  blobs, forward-only schema versions.
- The thread engine: prompts, queued prompts, interrupt, switch with full and delta briefs,
  fork, `/clear`, recovery when a native session cannot be resumed, and honest status after an
  agent or the daemon dies mid-turn.
- The approval broker with a per-thread "always" list rebuilt from the log.
- The projector: the handoff brief as a pure function of events, with byte-for-byte golden tests.
- The Claude Code adapter (stream-json in and out, approvals through the stream) and the Codex
  adapter (`codex exec --json`, one process per turn), each with fixtures of real output and
  replay tests.
- The TUI, matching the phase 0 prototype: sidebar across projects, transcript with markdown,
  tool lines, diffs under edits, the approval strip, the working line, the prompt with history,
  file picker and command palette, the help overlay, detach and reattach.
- `o` opens the selected read or edit in `$EDITOR`.
- The CLI with `--json`: `status`, `thread list|show|rename|rm`, `new`, `send`, `switch`,
  `fork`, `brief`, `wait`, `approve`, `deny`, `log`, `agents`, `doctor`,
  `daemon start|stop|restart|logs`.
- `docs/keys.md`, generated from the keymap and kept current by a test.

### Not built yet

- Panes, worktrees, `/diff`, `/pr`, raw agent panes, idle process reaping and destructive-command
  classification (phase 2).
- `config.toml`, `keys.toml`, themes, `/compact`, changing the model from desl, native session
  forks, the installer and the release pipeline (the rest of phase 1).
- Remote access, Windows, `export` and `import`, agent-written briefs, third-party adapters
  (phase 3).
