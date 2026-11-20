# Changelog

All notable changes to desl are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semantic versioning.

## [Unreleased]

### Added

- A task list per project. `prefix c` offers `tasks`, which opens the project's tasks thread
  (or focuses it) and makes `.tasks.md` at the project's root when there is none: a flat list
  of tasks, each with a title, a type, a status (todo, in progress, issue, done, ignored), a
  priority and a description in Markdown. Tabs filter it by status, `n` adds a task (its type
  first, then its title and description), `enter` opens one, `s` `p` `t` change its fields from
  a list, `x` marks it done, `u` undoes. What changes in the file shows at once and lights up.
  Agents keep it with `desl task list|show|add|set|rm`, which the desl skill now teaches them
  to use strictly; a daemon that starts brings an installed copy of the skill up to date.

- Text can be selected with the mouse in a shell pane, and in any pane scrolled back: a drag
  marks it inside that pane only, a double click takes a word and a triple click a line, and
  letting go copies it to the clipboard, through the terminal (OSC 52) and the desktop's own
  tool. Before, desl held the mouse and a shell, which never asks for it, got no selection.
- The projects in the sidebar can be reordered. `prefix w` now stops on a project's row too;
  `shift+↑` and `shift+↓` there move the project up or down, and a click on the row puts the
  cursor on it. The order is the daemon's, so it holds across clients and restarts.
- GitHub Copilot CLI is the third agent desl drives, in its own interface like the other two:
  `prefix c` and `desl new --agent copilot` open it, the sidebar shows its status and its own
  title for the conversation, `desl model` lists its models, and a switch or a fork copies the
  conversation into a Copilot session and out of one. Verified against Copilot CLI 1.0.112.
- With more than two agents installed, `prefix s` opens a list of them with the current one
  marked; with two it switches at once, as before.
- desl looks again for the agents' programs when a client attaches after one was installed or
  removed, so the menus follow what is on the machine without `desl agents`.

- A files thread: with [yazi](https://yazi-rs.github.io) installed, `prefix c` offers it under the
  shell (`desl new --agent files` from the CLI). It opens in the directory of the thread on
  screen, in a thread of its own with a folder mark in the sidebar, and it is yazi as the user
  set it up: the list, the highlighted preview, its keys. `q` closes the thread, and `prefix c`
  from it starts an agent where it is browsing.

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
  uncommitted, `desl ⎇ main ±4  ~/Projects`, the branch in the terminal's magenta and the count
  in its green, read again every five seconds. A worktree thread's header shows its own count
  beside its branch.

### Fixed

- A thread that has run for hours comes back whole when desl is opened again. The daemon kept
  the last 8 MB an agent drew and cut the rest off, so past that point what a client got on
  attach started in the middle of the drawing. An agent sets its full screen, the mouse and
  the form its keys arrive in once, when it starts, and all of that was in the part cut off:
  desl then took the pane for a plain one, the wheel scrolled desl's own scrollback in place
  of the agent's conversation, and the keys came in the plain form, not the one the agent had
  asked for. A Claude Code at work draws about 600 bytes a second, so this began after a few
  hours of work. Now everything older than the last megabyte is folded into what it left
  behind (the screen, 10,000 rows of history, the modes), so attaching shows what a client
  that never left shows. Changing to such a thread is faster too: a megabyte is drawn again,
  not eight.
- A pane opened on a thread whose terminal had other sizes before shows its scrollback as it
  was drawn. What was drawn at each size is now drawn again at that size and wrapped at the
  next; before, all of it was drawn at the pane's size, which put text an agent had placed by
  column in the wrong place.
- Installing a new build no longer ends the agents. `make install` stopped the daemon, which
  killed every agent and shell at once: a turn in the middle of its work was cut, and a server
  started in a shell went down. `desl daemon restart`, which `make install` now runs, starts
  the daemon again from the new binary and hands it every running program's terminal, so a
  turn goes on, nothing drawn is lost, and an open desl reconnects by itself. If the new
  daemon cannot start, the old one carries on and says why. The first install of this build
  still restarts the old way, once: the daemon it replaces cannot hand over, so that restart
  waits until no agent is working.
- Resizing the window, hiding the sidebar or opening a split wraps the text in a pane again at
  its new width, history included, as the terminal itself does. Before, a line stayed wrapped
  where it had been, and a narrower pane cut the end of each line off for good. The screen
  behind an editor or a pager is wrapped again too, so it is right when the program quits.
- desl no longer types a prompt into a question the agent draws near the top of its screen,
  such as Codex's "Trust this folder?" in a new folder. The thread waits for the user instead.
- What runs in a thread is told where it starts: `PWD` was the directory the daemon was
  started in, so a program that trusts it over its working directory opened there.
- A Claude thread no longer shows as working for hours after Claude Code compacts its
  conversation while idle (2.1.325 writes the summary in the user's place). The summary is
  Claude's own record: it opens no turn and is not an event of the thread.

### Changed

- `prefix o` also lists the plain folders beside your projects and repositories, marked
  `folder`: a directory made a minute ago in `~/Projects` is found before it has a `.git`.
  The home folder's own children are still listed only when they are projects, repositories
  or recent.
- The menus list only the agents installed on this machine. One that is missing is no longer
  a dimmed "not installed" row; `desl agents` and `desl doctor` still name it and how to
  install it. One that is installed and logged out stays, dimmed, with the reason.
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
