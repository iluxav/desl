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
- `/switch` alone opens a list of the agents with the current one marked, and says why an
  agent cannot be chosen when it is not installed or not logged in.
- `/model` alone opens the active agent's own list of models with the current one marked;
  `/model <name>` and `desl model <thread> [name]` pick one. The pick takes effect with the next
  prompt and keeps the native session. Each agent of a thread keeps its own model.
- Picking a model goes on to its reasoning effort levels, with the one in use marked and the
  agent's own setting as the first row. `/model <name> <effort>` and
  `desl model <thread> [name] --effort <level>` set it directly. A level the model does not
  take is refused before it reaches the agent.
- `/permissions` alone opens the permission modes (`ask`, `edits`, `yolo`) with the current one
  marked; `/permissions <mode>` and `desl permissions <thread> [mode]` set one. Each row says
  what the mode means with the thread's agent: Codex cannot ask before a call, so `ask` runs it
  read-only. `yolo` shows `⚠ yolo` in the status bar, answers a question that is already
  waiting, and logs everything it allowed. A fork does not inherit it.
- The store migrates forward on start and keeps a backup of the previous version beside it.

### Fixed

- Typing a command's name in the palette now finds that command. `/model` used to select
  `/info`, whose description mentions the word.
- A long list in a picker scrolls with the selection instead of leaving it off screen.
- CLI commands that read or change threads start the daemon when it is not running.
- Codex's notice about resuming a session on another model is no longer shown as an error.

### Not built yet

- Panes, worktrees, `/diff`, `/pr`, raw agent panes, idle process reaping and destructive-command
  classification (phase 2).
- `config.toml`, `keys.toml`, themes, `/compact`, reasoning effort, native session forks, the
  installer and the release pipeline (the rest of phase 1).
- Remote access, Windows, `export` and `import`, agent-written briefs, third-party adapters
  (phase 3).
