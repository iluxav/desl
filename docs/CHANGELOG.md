# Changelog

All notable changes to desl are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semantic versioning.

## [Unreleased]

### Added

- `desl interrupt <thread>` stops a thread's current turn, as `esc` does in the agent's own
  screen, and `desl exit <thread>` ends a thread's agent while the thread stays; its next
  prompt resumes the conversation. The desl skill teaches both, and neither works on the
  thread it runs in.

- `alt+j` / `alt+k` move to the next or previous thread in one press, without the prefix.
- In the sidebar (`prefix w`), the pane shows the thread under the cursor as it moves; `enter`
  or `esc` give the keys back to it.
- `prefix %` and `prefix "` split the pane: a second thread beside or under the one on screen,
  each with its own header. `alt+h` / `alt+l` or a click move the keys between the panes,
  `prefix z` zooms back to one, and the split comes back on the next attach.
- `prefix r` and `desl restart <thread>` end a thread's agent and start it again on the same
  conversation, so it runs the version and settings installed now, such as a new MCP server.
  The daemon keeps agents alive while desl itself is closed and reopened, so this is how one
  is reloaded.
- The sidebar's project row shows the branch checked out there and how many paths are
  uncommitted, `desl ⎇ main ±4  ~/Projects`, read again every five seconds. A worktree thread's
  header shows its own count beside its branch.

### Fixed

- desl no longer types a prompt into a question the agent draws near the top of its screen,
  such as Codex's "Trust this folder?" in a new folder. The thread waits for the user instead.

### Changed

- `prefix ?` is a command list as well as help: typing filters the keys, `↑`/`↓` move, `enter`
  runs the one under the cursor and `esc` closes.
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
