# Changelog

All notable changes to desl are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow semantic versioning.

## [Unreleased]

### Added

- Forms on desl web: an agent puts a form in front of you, and your answer is its next
  prompt. Claude Code, Codex and Copilot get desl's MCP tool `show_form` when desl starts them,
  through their own flags, with your configuration untouched. A form is drawn as a card in the
  thread's conversation: text to read, a choice with an optional typed answer, a checklist to
  untick, reorder and annotate, text, a number with a slider, a toggle, a list to pick from. Send
  stays off until the required answers are in; Dismiss closes it, and the agent is told before
  its next prompt. What you fill in is kept as you go, so a reload, an install or a restart finds
  it as you left it. The right column lists every open form; `ctrl+space v` on a thread with one
  open goes straight to it. A thread with a form open shows waiting, and talking to its agent
  meanwhile works as usual. `desl form list`, `show`, `answer`, `close` and `new` do the same
  from a shell. See `web-spec.md` and `docs/design/2026-11-21-web-forms.md`.
- Tasks are desl's first-class record. desl web's right column shows the project's task list,
  live, with filters, and each task starts an agent with one click (**Fix now**, **Work on it**,
  with the agent and the place to pick: its own worktree by default). An agent on its own branch
  marks the task ready, and **Merge** fast-forwards main to it; **Diff** shows the change first.
  Agents get `list_tasks`, `add_task` and `update_task`, and park the ideas you defer. Two new
  statuses, ready and parked. The left column lists agent threads only.
- The daemon and every agent leave the terminal that started them: the daemon moves itself,
  its agents and their processes into a systemd scope of its own, `desl-<pid>.scope`, so
  closing or killing that terminal's scope no longer ends them. A running daemon moves at the
  next install, agents and all, without stopping one. With `loginctl enable-linger` they also
  keep running when you log out; `desl doctor` says whether they do.
- A form an agent pushes while no desl web page is open opens your browser on it, starting desl
  web if it must; `[web] open_on_form = false` turns that off. `desl web --open --thread <id>`
  does it by hand.
- `desl` starts desl web in the background when it attaches, if none is running, without
  opening a browser. `ctrl+space v` then only opens the page.
- A **document** form widget: an agent names a Markdown file of the project, or one section of
  it, and you edit it in place on the page, with a preview and comments on any part. Send saves
  it into the file, but only if nobody changed that section meanwhile; if someone did, the card
  shows both versions and you pick. The agent gets the diff and your comments.
- Three more form widgets: a **preview** of HTML and CSS the agent wrote, tuned with sliders,
  colour pickers and lists that set its CSS variables, drawn in a frame where nothing runs; a
  **table** whose cells you edit, sent back as just the cells you changed; and **pictures** of
  the project to pick from, which desl reads only from inside the thread's directory. A choice
  can now start on the option the agent recommends.
- `desl web`: the workspace in a browser, the first milestone of `web-spec.md`. It prints an
  address on 127.0.0.1 with a key that is new each time, and the page lists every project and
  agent thread with the same marks and words as the terminal's sidebar: the agent logo as the
  status (breathing while it works, amber while it waits), the title, the age, the worktree
  mark, and each thread's header with its model, permission mode and `⚠ yolo`. It shows who is
  waiting, sends a prompt to the thread you pick (queued while it works) and stops its turn. It follows the browser's light or dark setting, works at phone
  width, and comes back by itself after `desl daemon restart`. Another website, a wrong key or
  a rebinding host name gets nothing. Shells and file managers stay in the terminal.
- desl web shows the selected thread's whole conversation, the same one `prefix [` shows in the
  terminal: every prompt with its time, the replies as Markdown, tool calls folded
  to a line you can open, a rule at each new day. A search box finds a word (enter for the next,
  shift+enter for the one before), a box hides the tool calls, and it follows new turns as they
  land.
  The prompt box sits under it, Stop is in the thread's header while it works, and desl's
  sentences show there for a few seconds. The agent's own screen stays in the terminal: the
  right column says which threads wait on you there.
- `ctrl+space v` opens the workspace in your browser: it starts `desl web` in the background
  when none runs for this workspace, and it runs on after desl closes. Over SSH the address goes
  to the clipboard instead. `desl web --open` does the same from a shell, `desl web` says where
  one already runs instead of starting a second, and `desl web --stop` ends it.
- The page's left and right columns resize: drag the line beside one, double-click it for the
  usual width, or give it the focus and use the arrows. The browser keeps the widths.
- A project's task list shows in the sidebar by itself once `.tasks.md` exists, also when an
  agent made it with `desl task add` (at once) or by other means (within five seconds). `q`
  closes it on screen; its row stays while the file does.

- Two panes are easy to tell apart. In the sidebar, a thread on screen has a `▌` at its row's
  edge and a mark of its pane's half of the screen (left or right, top or bottom); the one with
  the keys is tinted with the accent, as its pane's border is. A pane's title now leads with the
  thread's own title, then its project, so two agents of one project read differently. Each
  project has a colour of its own, on its mark in the sidebar and in a pane's title.

- With a Nerd Font, desl draws its Material Design icons across the screen, one family: a
  repository before each project, a branch and a plus-minus for git, the status in each panel's
  title, subagents' marks, a chip before the model, the agents' marks in the menus, an icon per
  kind of directory in `prefix o`, and an icon per line of `prefix i`. Without one, nothing
  changes.

- History: `prefix [` shows a thread's whole conversation in its pane, from the first prompt.
  After the daemon ended (a stop, a restart of the machine) an agent is started again on its
  session and draws what it chooses to: Claude Code shows nothing from before the last time it
  compacted its context, so a long conversation came back without most of itself. desl has logged
  every prompt, reply and tool call all along; this view reads that log, across compactions,
  restarts, switches and forks. `j`/`k`, the wheel and the page keys scroll, `{` and `}` step
  from prompt to prompt, `/` and `?` find with `n`/`N` to go on, `t` hides the tool calls, a
  rule marks each new day and the header says when the prompt on screen was sent. A drag, a
  double click or a triple click copies, as in a shell's pane. `esc` returns to the agent,
  which went on behind the view. A subagent's conversation takes the same keys now.
- A task list per project. `prefix c` offers `tasks`, which opens the project's tasks thread
  (or focuses it) and makes `.tasks.md` at the project's root when there is none: a flat list
  of tasks, each with a title, a type, a status (todo, in progress, issue, done, ignored), a
  priority and a description in Markdown. Tabs filter it by status, `n` adds a task (its type
  first, then its title and description), `enter` opens one, `s` `p` `t` change its fields from
  a list, `x` marks it done, `u` undoes. What changes in the file shows at once and lights up.
  With a Nerd Font, statuses, types and priorities are its Material Design icons, priority as
  signal bars; without, plain Unicode marks and `!!!`.
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

- A Codex thread whose agent asked "Allow the … MCP server to run tool?" for an MCP server
  of your own showed working, with no mark and no bell; it shows waiting now.
- The TUI's history line for a form named the key as "prefix v" and said nothing of the answer;
  it now names your prefix, shows the answer on one line, and says a form copied into a fork
  was asked in the thread it was forked from. `desl send` names your prefix too.
- Removing a thread broke the form cards of its forks: forms a fork still shows now stay,
  closed, and an open one reads "closed: the thread that asked it was removed".
- desl web's waiting column hid a dialog in an agent's own screen when that thread also had a
  form open; it lists both now.
- Forms: a field or list you emptied came back filled after a reload, and an answered card showed
  the agent's starting value for what you left empty. A comment with no note yet was lost on a
  reload. A table column named `constructor` broke its cells. A document's file that could no
  longer be read after a refused Send left the card silent; it now says why. The document
  preview shows `~~~` code fences.
- Forms, in the daemon: pictures are read off the engine's thread; a save's diff gives up after
  3 seconds; a save that fails after another one tells the agent which were saved; a section's
  headings are found as Markdown reads them (front matter, comments, `## C#`, inline code).
- An agent never started in a thread whose only history was something other than a
  conversation, such as a form pushed from a shell before the first prompt: desl copied the
  empty conversation into a session the agent could not resume. It now opens a new session, on
  start and on a switch.
- An install that upgraded the store lost what the new daemon wrote until its next restart.
  Before migrating, the daemon copied the store's file outside SQLite, which drops every lock
  the process holds on it; the daemon being replaced then took itself for the last one on its
  way out and deleted the write-ahead log under the new one. The backup is now made through
  SQLite (`VACUUM INTO`).
- `desl web` runs the new build after an install. It kept running the binary it was started
  from, so the page stayed the old one, and `ctrl+space v` reopened it. Now it notices its
  binary was replaced, starts again from the new one on the same address and key, and an open
  page reloads itself. `ctrl+c` and `desl web --stop` also stop it at once with a page open;
  they waited for the page to go.
- A Claude thread whose prompt held a pasted image stayed idle in the sidebar for the whole
  turn: Claude Code writes such a prompt as a list of blocks, which desl did not read as a
  prompt. It now does, and the lines Claude Code adds in the user's place (`isMeta`: a skill's
  text, where an image came from) are no longer taken for prompts.
- A key or a mouse report that reaches desl in two reads is read as one. A read from the
  terminal can end anywhere, with a fast wheel or over a slow link; desl passed the first half
  on to the agent, never saw the report, and the second half followed as text: a wheel step
  in a shell left `64;80;15M` at its prompt and scrolled nothing. The prefix as a kitty report
  was missed the same way, and a letter of several bytes typed into a rename was lost. The
  unfinished end of a read now waits for the next one. An escape pressed by itself is still
  passed on at once; one read together with other keys waits up to 50 ms.
- `ctrl+n`, `ctrl+p` and `ctrl+u` work in the key list and in the directory finder. A list
  got the letter instead, so `ctrl+n` typed an "n" into the filter.
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

- The thread's header in the terminal names the model the agent reported for its session,
  before one picked with `desl model` that it has not started on yet, and says `⚠ yolo` while
  the thread's permission mode is yolo. The sidebar's order and words now come from
  `desl_proto::view`, which `desl web` sends to the page, so the two cannot disagree.
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
