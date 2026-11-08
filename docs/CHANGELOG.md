# Changelog

All notable changes to desl are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semantic versioning.

## [Unreleased]

### Changed

- A thread takes its agent's own title for the conversation, Claude Code's or Codex's, in place
  of the first sentence of its first prompt, and follows it when the agent retitles it. A name
  given with `prefix ,` or `desl thread rename` stays. Threads renamed before this keep their
  names.

## [0.1.2]

### Added

- Agents can drive desl. Every terminal desl starts has `DESL_THREAD` set to its own thread,
  and `desl skill install` teaches Claude Code and Codex to open threads, prompt them, wait on
  them and fork their own conversation to the other agent.
- `desl send`, `wait`, `switch` and `thread rm` refuse the thread they run in, and `desl`
  refuses to attach inside one of its own threads, as tmux does.

## [0.1.1]

### Changed

- `prefix o` opens a directory finder: type part of a name to filter the known projects,
  zoxide's directories and the git repositories under the home directory, or type a path (`tab`
  completes it). Picking one opens a shell there; `prefix c` then starts an agent in it.
- The sidebar puts threads row on row, with one blank row between projects.

## [0.1.0] — first public build

desl on the native route: each agent runs its own interface, and desl adds a workspace around it.

### Added

- A daemon that owns every agent in a terminal of its own, a SQLite log of every thread, and a
  Unix socket for clients; agents keep running after the client detaches.
- The workspace screen: projects and threads in a sidebar (one row per thread, the agent's logo
  as its status), the agent's own interface in the pane through desl's terminal emulator, a
  header with the agent, project, model and effort, and a key line.
- `ctrl+space` as the prefix, recognised in every keyboard encoding; `s` switch, `c` new (a menu
  of Claude, Codex and the user's shell), `o` open a directory, `f` fork, `w` sidebar, `n`/`p`,
  `1`…`9`, `l`, `,` rename, `i` info, `x` close, `b` hide the sidebar, `?` help, `d` detach.
- Switching a thread between Claude Code and Codex by copying the conversation into a new native
  session of the other agent, swapped in one frame; refused mid-turn.
- Forks onto either agent, and git worktrees per thread (`w` in the menus, `--worktree`).
- Subagents (Claude's and Codex's) under their thread, with their conversation one click away.
- Waiting threads ring the bell once and are counted in the title.
- The CLI with `--json`: `status`, `thread list|show|rename|rm`, `new`, `send` (typed into the
  agent's own interface), `switch`, `fork`, `wait`, `model`, `permissions`, `brief`, `agents`,
  `doctor`, `config show`, `daemon start|stop|restart|logs`.
- `config.toml`, per user and per project: prefix, default agent, default permission mode,
  worktree directory and branch prefix.
- `install.sh` and release builds for Linux and macOS, x86_64 and arm64.
