# desl — Product Specification

Nov 4, 2026 · @Ilya Vinokurov

## Vision and positioning

desl (say diesel) is a single-binary terminal workspace that runs many coding agents side by side, keeps them alive after you detach, and lets one conversation move between agents without losing context. It is the terminal-native tool for people who already use Claude Code and Codex and want one place to run, watch, switch and fork them.

**The gap.** Two good tools each solve half the problem. Herdr (Rust, tmux-style) gives persistence, fleet visibility and an attention queue, but every agent owns its own conversation, so there is no way to continue a Codex thread in Claude. T3 Code (TypeScript, web/desktop GUI) owns threads through provider adapters and supports worktrees per thread, but it has no terminal UI and no cross-agent handoff. desl combines both halves in the terminal and adds the piece neither has: portable conversation state.

**The one-line pitch.** Run every agent in one persistent terminal workspace; switch or fork any thread to any agent, mid-conversation.

**Who it is for.**

- Engineers who run 2 to 10 coding agents in parallel across worktrees and lose track of which one needs them.
- Engineers who use more than one agent and want to pick the best model per task without starting over.
- Teams running agents on remote boxes over SSH who need detach, reattach and a mobile glance later.

**What desl is not.** Not a new coding agent, not a model router, not an IDE. It drives the agents the user already has, with the user's own subscriptions and logins, on the user's own machine.

**Success looks like.** A user starts a thread in Claude Code, types `/switch codex`, and Codex continues as if it had been there all along. They close the laptop, SSH in from a phone, and every agent is still running. They never read a manual, because every key and command works the way tmux, lazygit, Claude Code and Codex already taught them.

## User experience walkthrough

These five scenarios are the acceptance script. Each must work exactly as written, with no step added.

**First run.** The user runs `curl -fsSL https://desl.dev/install.sh | sh`, then `desl` inside a repo. desl starts the daemon in the background, detects `claude` and `codex` on PATH and whether each is logged in, registers the repo as a project, and opens the workspace with one empty thread on the default agent. The status line reads `claude · main · t-7f3a`. Total time under ten seconds; nothing to configure. If no agent is found, desl says which ones it supports and how to install each, then exits cleanly.

**The daily loop.** The user types a prompt. The transcript streams tokens as they arrive; tool calls collapse to one line each (`read src/auth.ts`, `edit 3 files`, `run pnpm test ✓`) and expand on `enter`. When the agent asks for approval, the thread turns `waiting` in the sidebar, a single-line prompt appears above the input (`Allow: rm -rf dist/ ? [y]es [n]o [a]lways`), and a terminal bell fires if the user is in another pane. `ctrl+n` opens a second thread in a new worktree; `ctrl+1`/`ctrl+2` jump between them. The sidebar shows every thread with its status and age, waiting threads sorted to the top.

**Switch.** Mid-thread, the user types `/switch codex`. desl shows a one-line confirmation with what will be sent (`Handing 41 turns to codex as a brief (2.1k tokens). Continue? [y]`), builds the brief, starts a Codex session with it, and marks the thread `codex`. The next prompt goes to Codex. The transcript shows a thin divider: `── switched to codex ──`. `/switch claude` later resumes the original Claude session and prepends only the delta since Claude last saw the thread. The user never sees a native session id.

**Fork.** The user runs `/fork claude` to try a different approach from the current point, or `/fork claude @12` to fork from message 12 (shown as a gutter number on hover or with `/messages`). A new thread opens beside the old one, named `t-7f3a/1`, with the parent's history up to the fork point and a fresh agent. Optionally `--worktree` puts it on its own branch. The parent is untouched.

**Detach and return.** `prefix d` detaches; all agents keep working. From another machine, `ssh box && desl` reattaches to the same workspace with every thread where it was. `desl status` from any shell prints the sidebar as text; `desl wait t-7f3a` blocks until that thread needs attention, for use in scripts.

Throughout, three things are always true: the current agent name is visible, the current git branch is visible, and `?` shows every key that works right now.

## The feel bar

Nine named moments. Each is a testable experience target, scored by the author in the hallway protocol below, and each has an owner in the roadmap. They sit beside the performance budgets with equal weight: a release that misses a feel moment does not ship, the same as one that misses a latency number.

1. **First reply in ten seconds.** From a finished install to a first agent reply, a new user reads nothing, configures nothing, and is never asked a question they do not understand.
2. **Keys you already know.** A user who knows tmux and Claude Code discovers switch, fork, detach and the waiting list within ten minutes, unprompted, and never hits a key that does something surprising.
3. **Switching feels like changing the model.** `/switch codex` shows no seam except the thin divider; the next reply answers the user's question, not the brief; the user never sees or types a native session id.
4. **Nothing moves under your eyes.** While the user is reading, nothing flickers, jumps or reflows. Streaming never scrolls away the line being read; new output waits behind a `↓ n new` pill.
5. **Waiting is impossible to miss and impossible to be nagged by.** One bell, one sidebar glyph, one line above the input. Never a modal, never a repeat, never a notification for something already answered.
6. **The agent is the hero.** The user's eye lands on the agent's words and diffs, not on desl's chrome. A screenshot reads as a conversation with a border, not as a dashboard.
7. **Coming back is instant.** Reattach from any device restores the exact screen, scroll positions and focus in under a second, and every agent is where it was.
8. **Errors read like a colleague.** Every error says what happened and what to do next, in one line, in plain words, with a key to press. No stack traces, no codes without text.
9. **Leaving is always safe.** Quitting detaches. Closing the terminal detaches. Killing the process loses nothing. The user never has to think about whether it is safe to stop.

**Hallway protocol.** The author is the only tester (decided 2026-11-04; the protocol was first written for ten outside users in observed, recorded sessions). Before each gate the author rates each of the nine moments on the build from 1 to 5 and writes one sentence about what they expected. A moment passes at 4 or higher. Every 1 or 2 becomes an issue. The protocol runs on the phase 1 build and on every release candidate after that; scores may never drop between runs.

**Design review.** Every screen, state and message goes through a two-person review before it ships: one person who built it and one who did not, with the vocabulary table, the keymap and this list open. The reviewer tries to break the moment, not to confirm it.

## Product principles

Every design decision is tested against these eight rules. When two conflict, the earlier one wins.

1. **Nothing is ever lost.** Thread state is an append-only log on disk. A crash, a killed terminal or a dead agent process never loses a message, a diff or a decision. Recovery is automatic and silent.
2. **Zero to useful in ten seconds.** First run detects installed agents, detects the repo in the current directory, and opens a thread. No config file, no wizard, no account.
3. **Borrow, never invent.** Keys, words and commands come from tmux, lazygit, fzf, Claude Code and Codex. A new concept gets a new word only when no community word fits. Users should guess right the first time.
4. **The agent is the star.** desl's chrome stays quiet so the agent's output reads cleanly. One accent colour, no banners, no animation that does not carry information.
5. **Interrupt the agent, never the app.** `ctrl+c` stops the running agent turn. Quitting desl is a deliberate command, and the daemon keeps running regardless.
6. **Everything is scriptable.** Every action in the TUI exists as a CLI subcommand with `--json` output. Agents can drive desl through the same interface a human uses.
7. **Fast by default.** Sub-16 ms frame redraws, instant thread switching, token streaming without stutter. Performance is a feature with a budget, not a cleanup phase.
8. **Honest status.** The UI never says "done" when a process is still running, never hides an error, and shows which agent and which native session is behind every thread.

## Non-goals

Every tool in this space grew sideways. Staying narrow is a feature, and these are the directions desl will not grow in. A request for any of them is answered with "no" in the issue tracker, with a link here.

- **No new coding agent, model router or API proxy.** desl drives the agents the user already has, under the user's own logins. It never calls a model API itself.
- **No plugin runtime, workflow engine or scheduler.** No marketplaces, no event-driven loops, no cron. Long-running automation belongs in the agents or in CI; desl's CLI and skill file let them drive it.
- **No dashboard.** No web UI, no metrics pages, no boards. The remote client in phase 3 is the same TUI over TCP.
- **No IDE features.** No file browser, no editor, no language server. `o` opens the file in `$EDITOR` and that is the integration.
- **No team or cloud features in the core.** No accounts, no hosted sync, no shared state. `export` and `import` move a thread between the user's own machines.
- **No agent beyond the two until the two are perfect.** Claude Code and Codex are the whole surface until phase 3; more agents come through the third-party adapter protocol, not the core.
- **No ambient AI in the chrome.** desl does not summarise, suggest, rank or auto-complete anything with a model of its own. The one exception is the opt-in agent-written brief, which the outgoing agent writes.
- **No configuration required, ever.** A feature that needs a config key to be useful is not finished.

When a non-goal looks tempting, the test is: would lazygit add this? If lazygit would not, neither does desl.

## Naming and vocabulary

**desl** (say *diesel*) is the product and the binary. The story: coding agents are engines; desl is the fuel that keeps them running after you close the lid, and lets you swap one engine for another without stopping the vehicle. Diesel carries the associations the product wants: workhorse, long-haul, runs for days, torque over flash. Four letters, lowercase, impossible to misspell once heard; the README's first line fixes the pronunciation. A first collision pass on 2026-11-04 found `desl` clean: no CLI, package, product or agent tool of that name on GitHub, npm or PyPI. Before any public artifact, register the GitHub org, `desl.dev` and `desl.sh`, crates.io `desl`, npm `desl` (for a future `npx desl` installer shim) and a Homebrew tap, and do a trademark glance (Diesel the apparel brand is a different word in a different class). Known adjacency: `diesel` is a well-known Rust ORM; the spelling and the space differ, so it is not a blocker, but crate names must not imitate `diesel-*` patterns. The metaphor lives in the brand (tagline, README, logo), never in the UI vocabulary below: agents stay agents, not engines. Rejected names, each already taken by one or more coding-agent tools: drover, muster, corral, wrangle/wrangler, baton, weft, skein, splice, ferry, yoke, tack, continuo, legato, cadenza. Clean runner-up if desl ever has to change: segue.

All user-facing words are fixed here. Code, docs, help text and error messages use these and no synonyms.

| Word | Meaning | Borrowed from |
| --- | --- | --- |
| workspace | Everything the daemon holds on one machine | tmux session, Herdr |
| project | A git repository desl knows about | T3 Code |
| thread | One conversation with an agent, owned by desl, with its own id | T3 Code, Codex |
| agent | A coding agent CLI: Claude Code, Codex, OpenCode | universal |
| session | The agent's own native conversation id, bound to a thread | Claude Code, Codex |
| pane | A region of the screen showing a thread, a shell or a raw agent | tmux, Herdr |
| worktree | A git worktree a thread runs in | git, T3 Code |
| brief | The handoff summary desl builds when a thread changes agent | new |
| switch | Continue this thread with a different agent | new |
| fork | Copy a thread up to a point into a new thread | git, Claude Code |
| detach / attach | Leave or rejoin the workspace, agents keep running | tmux |

Status words, used identically in the sidebar, the CLI and notifications:

| Status | Meaning | Colour role |
| --- | --- | --- |
| working | The agent is generating or running a tool | accent |
| waiting | The agent asked a question or needs an approval | warning |
| idle | The last turn finished, nothing pending | muted |
| failed | The agent process exited abnormally | critical |
| detached | Pane is running but no client is attached | muted |

Verbs in commands and help follow the agents' own grammar: `resume`, `clear`, `compact`, `export`, `model` keep their Claude Code and Codex meanings. desl adds `switch`, `fork`, `brief`, `worktree`, `attach`, `detach`. Never use "session" for a desl thread or "thread" for an agent's native session.

Identifiers: thread ids are short, typeable and prefixed, in the style of git short SHAs: `t-7f3a`. Native session ids are shown only on request (`/info`) and never required from the user.

## Interface design

The default layout is a narrow sidebar on the left, one or more panes on the right, and a one-line status bar at the bottom. It must read well at 80×24 and scale to 300×80 without changing shape.

```
┌ desl ─────────────┬──────────────────────────────────────────────────────┐
│ PROJECTS          │ t-7f3a · claude · main                               │
│ ▾ api (3)         │                                                      │
│   ● t-7f3a  auth  │ › Add refresh-token rotation to the auth middleware  │
│   ◐ t-91c2  tests │                                                      │
│   ○ t-04be  docs  │   I'll start by reading the current middleware.      │
│ ▸ web (1)         │   ▸ read src/auth/middleware.ts                      │
│                   │   ▸ edit src/auth/middleware.ts (+41 −7)             │
│ WAITING (1)       │   ▸ run pnpm test                        ✓ 84 passed │
│   ◐ t-91c2        │                                                      │
│                   │   Rotation is in. Want me to add the revocation list?│
│                   │                                                      │
│                   │ ┌────────────────────────────────────────────────────┐
│                   │ │ ›                                                  │
│                   │ └────────────────────────────────────────────────────┘
├───────────────────┴──────────────────────────────────────────────────────┤
│ ● working  claude  main ↑2  worktree: auth   ctrl+b ? help  │ 12:41  │
└──────────────────────────────────────────────────────────────────────────┘
```

**Sidebar.** Projects collapse like a file tree. Each thread shows a status glyph, its id and a short name derived from its first prompt (editable with `/rename`). A `WAITING` group at the bottom repeats every thread that needs the user, newest first, so attention never requires scrolling. Hidden with `prefix b` for a full-width transcript.

**Transcript pane.** The agent's prose renders as markdown with syntax-highlighted code blocks and visible fenced boundaries. Tool calls are one collapsed line each with a verb, a target and an outcome; an edit also shows the first rows of its diff under the line. `enter` on a line expands its full output in place or collapses it again, `o` opens the file in `$EDITOR`. Diffs render as unified diffs with gutter colours. Switch and fork events appear as thin labelled dividers. The pane auto-follows while streaming and stops following the moment the user scrolls up, with a `↓ 3 new` pill to jump back.

**Input.** A bordered single-line box that grows to five lines. `/` opens a fuzzy command palette with inline descriptions; a command whose argument is a choice (`/switch`, `/model`) opens a second list of the choices instead of asking the user to type one from memory; `@` opens a fuzzy file picker scoped to the project; `ctrl+r` searches prompt history across all threads. `shift+enter` inserts a newline; `enter` sends. While the agent is working, the box stays usable and queued prompts show as `⏳ queued`.

**Approvals.** A one-line strip between transcript and input, never a modal: `Allow codex: rm -rf dist/  [y]es [n]o [a]lways for this thread [e]dit`. The bell fires and the sidebar glyph changes at the same moment.

**Status bar.** Left to right: status glyph and word, agent, git branch with ahead/behind counts, worktree name, the help hint, the clock. Nothing else. Errors replace the left half in critical colour until dismissed with `esc`.

**Visual language.** One accent colour (default: the terminal's blue), one warning, one critical, three greys. Status glyphs: `●` working, `◐` waiting, `○` idle, `✕` failed, `⊝` detached, with the words always available for screen readers and `NO_COLOR`. Rounded box drawing where the terminal supports it, falling back to ASCII. No emoji in chrome. Respect the terminal's own palette by default; built-in themes (`default`, `mono`, `catppuccin`, `gruvbox`, `solarized`) are opt-in.

**Panes.** `prefix %` and `prefix "` split as in tmux. A pane holds a thread, a plain shell, or a raw agent TUI (see Agent adapters). Raw and shell panes get the same border, title and status glyph as thread panes so the workspace feels uniform.

**Help.** `?` anywhere opens a context overlay listing the keys that work in the focused element, grouped by purpose, in the style of lazygit. The overlay is generated from the keymap, never hand-written, so it is always correct.

## Commands, keybindings and CLI

Three surfaces, one command set. Every slash command has a CLI equivalent and a keymap entry; the help overlay, the command palette and `desl --help` are all generated from the same registry.

**Slash commands** (typed in the input, fuzzy-matched in the palette):

| Command | Action | Source of the name |
| --- | --- | --- |
| `/switch [agent]` | Continue this thread with another agent. Alone, it opens a list of the agents with the current one marked | desl |
| `/fork <agent> [@n] [--worktree]` | New thread from here or from message n | desl, git |
| `/brief` | Show the handoff brief desl would send right now | desl |
| `/new [--worktree]` | New thread in this project | T3 Code |
| `/threads` | Fuzzy thread picker | T3 Code |
| `/resume` | Pick a past thread to reopen | Claude Code, Codex |
| `/rename <name>` | Rename this thread | tmux |
| `/model [name]` | Change the model for this agent. Alone, it opens the agent's own list of models with the current one marked | Claude Code, Codex |
| `/clear` | Start a fresh context on the same thread (new native session, history kept) | Claude Code |
| `/compact` | Ask the agent to compact its context | Claude Code |
| `/export [file]` | Write the thread as markdown | Claude Code |
| `/diff` | Show the worktree diff in a pane | lazygit, T3 Code |
| `/pr` | Create a pull request from this worktree | T3 Code |
| \`/worktree new | list | rm\` |
| `/info` | Agent, model, native session id, token usage | desl |
| `/help` | The help overlay | universal |
| `/quit` | Detach and close the client; daemon keeps running | universal |

Unknown slash commands are forwarded to the agent, so agent-specific commands and user skills keep working.

**Keys.** The prefix is `ctrl+b` by default (`ctrl+a` and `ctrl+space` offered on first run). Keys without a prefix act on the focused element; prefixed keys act on the workspace.

| Key | Action | Source |
| --- | --- | --- |
| `enter` | Send prompt, or expand the selected tool call | universal |
| `shift+enter` | Newline in prompt | Claude Code |
| `ctrl+c` | Interrupt the agent's current turn; twice to clear input | Claude Code, Codex |
| `esc` | Dismiss overlay, stop following, back out | universal |
| `j` / `k`, `↓` / `↑` | Move in lists and the transcript | vim, lazygit |
| `g` / `G` | Top / bottom of the transcript | vim |
| `/` | Command palette (input empty) or filter (in a list) | fzf, lazygit |
| `@` | File picker into the prompt | Claude Code |
| `ctrl+r` | Prompt history search | shell |
| `ctrl+t` | Fuzzy jump to any thread | fzf |
| `ctrl+n` | New thread | T3 Code |
| `ctrl+1`…`ctrl+9` | Jump to thread n in the sidebar | browsers |
| `tab` / `shift+tab` | Next / previous pane | tmux |
| `y` / `n` / `a` | Answer an approval | Claude Code |
| `o` | Open selected file in `$EDITOR` | lazygit |
| `?` | Help overlay for the focused element | lazygit |
| `prefix d` | Detach | tmux |
| `prefix %` / `prefix "` | Split pane right / below | tmux |
| `prefix x` | Close pane | tmux |
| `prefix b` | Toggle sidebar | desl |
| `prefix z` | Zoom the focused pane | tmux |
| `prefix s` | Switch agent (same as `/switch`) | desl |
| `prefix f` | Fork thread (same as `/fork`) | desl |

All keys are remappable in `keys.toml` by named command, with `vim` and `emacs` presets. desl detects when it runs inside tmux and offers to change its prefix on first run.

**Non-interactive CLI.** Every command takes `--json` and exits non-zero on failure.

```
desl                         attach to the workspace (starts the daemon if needed)
desl status                  the sidebar as text
desl thread list|show|rename|rm
desl new [-p project] [--agent claude] [--worktree] [-m "prompt"]
desl send <thread> "prompt"  send and stream the reply to stdout
desl switch <thread> <agent>
desl model <thread> [name]   list the agent's models, or pick one
desl fork <thread> <agent> [--at N] [--worktree]
desl brief <thread>          print the handoff brief
desl wait <thread>           block until the thread is waiting or idle
desl approve|deny <thread>
desl worktree new|list|rm
desl agents                  detected agents and login state
desl doctor                  check binaries, logins, daemon, permissions
desl daemon start|stop|restart|logs
desl upgrade
```

The CLI is how scripts, CI and other agents drive desl. A `desl` skill file for Claude Code and Codex (shipped in the repo, installable with `desl skill install`) teaches agents to open threads, wait on them and fork work, the way Herdr's skill does.

## Core concepts and data model

desl owns the conversation; the agents' native sessions are caches. Every thread is an append-only list of provider-neutral events, and each agent that has touched the thread holds a binding that records its native session id and how much of the thread it has seen.

**Thread.**

```
Thread {
  id:          "t-7f3a"            // short, typeable, unique per workspace
  project_id:  "api"
  name:        "auth"              // derived from first prompt, user-editable
  cwd:         "/Users/me/api"
  worktree:    "auth" | null       // git worktree name, if any
  parent_id:   "t-91c2" | null     // set on forks
  fork_point:  18 | null           // event index in the parent
  active_agent:"claude"
  status:      working | waiting | idle | failed | detached
  created_at, updated_at
  bindings:    { agent → Binding }
}

Binding {
  agent:        "claude"
  session_id:   "session-id-1"    // the agent's own id, resumable
  seen_up_to:   41                 // index of the last event this agent has in context
  model:        "claude-opus-5-5"   // what the agent reported for its session
  model_choice: "sonnet" | null     // picked with /model; null follows the agent's default
  last_used_at
}
```

`seen_up_to` is the high-water mark. When the thread head is ahead of an agent's mark, that agent is missing exactly the events between the two, and the brief for a switch back to it is built from that range alone.

**Events (the IR).** One schema for every agent. Adapters translate native output into these and nothing else.

| Kind | Fields | Notes |
| --- | --- | --- |
| user\_message | text, attachments\[\] | What the user sent |
| assistant\_message | text, agent, model | Final prose for a turn; streamed deltas are not stored separately |
| tool\_call | id, name, args, agent | `read`, `edit`, `bash`, `search`, or the agent's own name |
| tool\_result | call\_id, output, exit\_code, is\_error, truncated, blob | Output stored in full on disk, truncated in memory; `blob` names the full text |
| file\_change | path, diff, kind (create/modify/delete), call\_id | Derived from tool results or git, so handoffs know what changed; `call_id` ties it to the edit that made it |
| approval\_request | call\_id, tool, summary, risk | Surfaced in the UI as `waiting`; `tool` is the key of the thread's "always" list |
| approval\_response | call\_id, decision (allow/deny/always), by (user/policy) |  |
| agent\_switch | from, to, brief\_event\_id | Rendered as a divider |
| fork | parent\_id, fork\_point | First event of a forked thread |
| brief | text, token\_estimate, covers\_range, delta | The handoff text that was actually sent |
| usage | agent, input\_tokens, output\_tokens, cost\_estimate | Per turn |
| error | message, recoverable | Process crashes, adapter failures |
| interrupt |  | The user stopped the turn |

Every event carries `index`, `thread_id`, `timestamp` and `agent`. Indices start at 1, are dense and are never reused, so a fork point or a high-water mark is always an exact reference; a mark of 0 means the agent has seen nothing. Tool calls use neutral names (`read`, `edit`, `bash`, `search`, `fetch`, `agent`, `todo`) and neutral argument keys (`path`, `command`, `pattern`, `query`, `url`, `description`), so a client can show any agent's work without knowing the agent.

**Project.** A registered git repository: id, path, default agent, default branch, list of worktrees. Projects are discovered from `cwd` on first run and can be added with `desl project add`.

**Workspace.** The set of projects, threads, panes and client layouts the daemon holds for one user on one machine. Pane layout is persisted so reattach restores the exact screen.

**Storage.** SQLite, one file at `$XDG_DATA_HOME/desl/workspace.db`, WAL mode. Events are rows; large tool outputs live in content-addressed files beside the database and are referenced by hash. The schema has a version and every migration is forward-only. A `desl export <thread>` writes a self-contained JSONL that `desl import` can load on another machine, which is the sync path for a later release.

## Architecture

One binary, two roles: a long-lived daemon that owns all state and processes, and a thin client that renders it. The TUI, the CLI and any future remote client are all clients of the same daemon over one protocol.

&#91;embedded content: architecture · clients, daemon, adapters, agent processes\]

Clients never touch agent processes or the store directly; everything goes through the daemon, which is why detach, reattach and remote access cost nothing extra.

**Daemon** (`desl daemon`). Starts on first use, runs as a user-level background process (launchd agent on macOS, systemd user unit on Linux, Windows service later). Owns the SQLite store, every agent subprocess, every PTY, the git service and the event bus. Listens on a Unix socket at `$XDG_RUNTIME_DIR/desl.sock` (named pipe on Windows), optionally on TCP with a token for remote clients. One daemon per user per machine; projects are a dimension inside it.

**Client** (`desl`, the TUI). Connects to the socket, subscribes to the workspace, renders the store and sends commands. Holds no state that matters beyond scroll position and focus. Can be killed at any time. The CLI subcommands are the same client without the screen.

**Protocol.** Length-prefixed JSON frames over the socket (msgpack if profiling shows JSON on the hot path). Three message families: `command` (client → daemon, with a request id and a typed reply), `event` (daemon → client, every IR event plus workspace changes, as they happen), and `stream` (token deltas for the thread a client is viewing, sent only to subscribed clients). The protocol is versioned and documented in `docs/protocol.md`; the daemon refuses clients with a mismatched major version and tells them to upgrade.

**Thread engine.** The core state machine. Takes a user prompt, appends the event, resolves the thread's active adapter, checks the high-water mark, asks the projector for a delta brief if needed, sends the turn, consumes the adapter's event stream, appends events, updates status, bumps the mark. Serialises all writes to one thread; threads run concurrently.

**Adapters.** One package per agent implementing a single interface: `start(thread, resume?) → session`, `send(session, turn) → stream<IR event>`, `interrupt(session)`, `answer(session, approval)`, `close(session)`, and `capabilities()` (supports resume, fork, model switch, compaction). Adapters are the only code that knows a native format. See Agent adapters.

**Projector.** Pure function from an event range to a brief. Deterministic, no network. Produces structured text with fixed headings so the receiving agent can rely on its shape. See Cross-agent continuity.

**Git service.** Wraps `git` for worktree create/list/remove, branch state, diff and PR creation (through `gh` when present). Emits `file_change` events by diffing the worktree before and after each agent turn, which makes handoffs accurate even when an agent's tool output is opaque.

**PTY host.** For shell panes and raw agent panes. Based on the platform PTY library, with scrollback kept by the daemon so reattach restores it.

**Approval broker.** Receives `approval_request` events from adapters, applies per-thread and per-project policy (`ask`, `allow-reads`, `allow-all-in-worktree`), surfaces what still needs a human, and routes the answer back to the right adapter.

**Event bus.** In-process pub/sub inside the daemon. Every store write publishes; clients, the status line, notifications and the `desl wait` command all subscribe. Nothing reads the store to find out what changed.

## Cross-agent continuity

Switch and fork are both projections of the same event log into a different agent. Neither ever writes to an agent's native session files; everything goes through the agent's supported input path (a prompt or a resumed session).

**Switch algorithm** (`/switch <agent>` on thread T):

1. Set `T.active_agent = agent`. Append an `agent_switch` event.
2. Look up `T.bindings[agent]`.
3. No binding: build a **full brief** from events `0..head`, start a new native session with the brief as the first user turn, store the returned session id with `seen_up_to = head`.
4. Binding exists and `seen_up_to < head`: build a **delta brief** from `seen_up_to+1..head`, resume the native session, prepend the delta to the user's next prompt, set `seen_up_to = head`.
5. Binding exists and `seen_up_to == head`: resume with nothing extra.
6. If resume fails (session purged, agent updated, cwd moved), log a recoverable `error`, drop the binding and fall back to step 3.

The brief is sent as part of the next user turn, not as a separate turn, so the agent answers the user's actual question in the same response. Before sending, the UI shows a one-line summary and the token estimate; `/brief` shows the full text.

**Fork algorithm** (`/fork <agent> [@n] [--worktree]` on thread T):

1. Create thread F with `parent_id = T.id`, `fork_point = n` (default head), `project_id = T.project_id`, name `T.name` plus a `/k` suffix, empty bindings.
2. Copy events `0..n` into F (by reference to the same content blobs, so forks are cheap).
3. If `--worktree`, create a git worktree from T's current commit and set `F.worktree`.
4. If `agent == T.active_agent` and the adapter reports `fork` capability and `n == head`, fork the native session (`claude --fork-session`, `codex fork`) and bind it with `seen_up_to = n`.
5. Otherwise treat it as a switch with no binding: full brief from `0..n`, new session.
6. Open F in a new pane beside T.

**Brief format.** Deterministic markdown with fixed headings, so agents learn its shape. Sections are omitted when empty.

```
# Handoff brief
You are continuing work that another assistant started. Treat this as your
own history. Do not re-explain it; continue from "Next step".

## Task
<first user prompt, verbatim, plus any later reframing>

## Decisions
- <decision> — <one-line reason>       (extracted from assistant messages)

## Files touched
- src/auth/middleware.ts (+41 −7)
- src/auth/tokens.ts (new)

## Current diff
<git diff of the worktree, or --stat plus hunks for files above, capped>

## Recent conversation
<last 5 turns verbatim, user and assistant, tool calls as one line each>

## Open questions
- <unanswered questions the previous assistant asked>

## Next step
<the last assistant message's final paragraph, or the user's pending prompt>
```

Budget: 4k tokens by default, configurable per project. The projector fills sections in priority order (task, next step, diff, recent conversation, decisions, files, open questions) and truncates from the bottom of each section. Token counts use a local tokenizer approximation; the estimate shown to the user is marked `~`.

**Projector extraction rules.** Decisions are sentences in assistant messages that match an explicit pattern ("I'll use", "decided", "instead of", "because") or precede a file\_change. Files touched come from `file_change` events, never from parsing prose. The diff comes from the git service, not the agent. Everything the projector cannot verify from events is left out; it never invents.

**Agent-written brief (phase 3).** When the outgoing agent is still alive, desl can ask it for a summary with a fixed prompt and merge it into the Decisions and Next step sections. This is opt-in per project because it costs a turn.

**What is not carried.** The receiving agent does not get the previous agent's tool-call ids, its system prompt, its permission state or its compaction summaries. The brief is the whole interface. Tests must assert that no native data crosses the boundary.

## Agent adapters

An adapter is the only code allowed to know an agent's native format. It runs the agent in headless mode, turns its event stream into IR events, and exposes a uniform permission hook. Two adapters ship in phase 1, one escape hatch in phase 2, and the contract is public so the community can add more.

**Adapter interface.**

```
interface Adapter {
  id():            "claude" | "codex" | ...
  detect():        { installed: bool, version, logged_in: bool, binary_path }
  capabilities():  { resume, fork, model_switch, compact, interrupt, approvals }
  start(thread, opts: { resume_session_id?, model?, cwd, permission_mode }) → Session
  send(session, turn: { text, attachments[], prefix? }) → AsyncStream<IREvent>
  interrupt(session)
  answer(session, approval: { call_id, decision })
  compact(session)
  close(session)
}
```

Every adapter lives in `adapters/<id>/` with a `fixtures/` directory of real captured native output per supported agent version and a replay test that asserts the IR it produces. An adapter that fails its fixture tests cannot ship.

**Claude Code adapter.** Spawns `claude -p --output-format stream-json --input-format stream-json` with `--permission-prompt-tool` style control so approvals come through the stream rather than a terminal prompt, plus `--resume <id>` or `--session-id <id>` for continuity and `--fork-session` for native forks. Reads the stream-json events (system, assistant, user, tool\_use, tool\_result, result) and maps them to IR. Uses `--append-system-prompt` for a short desl preamble that names the thread and worktree. Verify flag names against `claude --help` for the pinned version; they move. Where the Claude Agent SDK gives a cleaner interface than the raw subprocess, the adapter may wrap a small Node sidecar in later phases, but phase 1 is subprocess-only to keep the single binary.

**Codex adapter.** Spawns `codex exec --json` (and `codex exec resume <id>` for continuity) with sandbox and approval flags set from the thread's permission mode, working directory set with `--cd`. Reads the JSON event stream (thread started, turn started, item added/completed for messages, commands, file changes, approvals, turn completed, usage) and maps to IR. Uses `codex fork` for native forks. Verify flag names against `codex exec --help` for the pinned version.

**Raw PTY adapter (phase 2).** Runs any agent's interactive TUI, or a shell, in a pane without owning the conversation. The pane gets a status glyph from output heuristics (prompt detection, idle time) the way Herdr does, but no IR events, so switch and fork are disabled on it and the UI says why. This is the escape hatch for agents without a headless mode and for users who want an agent's native features. After the process exits, desl offers to import the agent's native session file into a thread on a best-effort basis.

**Permission modes.** Adapters receive one of `ask` (every tool call needs a human), `edits` (reads and edits allowed, commands ask), `worktree` (everything allowed inside the thread's worktree, nothing outside), `yolo` (everything allowed; the UI shows a persistent warning). Each adapter maps the mode to its agent's own flags and reports what it could not enforce.

**Version pinning.** `desl doctor` reports each agent's version and whether the adapter has fixtures for it. A newer agent version than any fixture triggers a non-blocking warning. A CI job captures fresh fixtures from the latest agent releases weekly and opens a PR when the IR mapping changes.

**Extensibility.** A third-party adapter is a binary on PATH named `desl-adapter-<id>` speaking the adapter interface over stdio as JSON. desl discovers it at startup. This keeps the core binary small and lets the community cover OpenCode, Gemini CLI, Cursor Agent and others without forking.

## Permissions and safety

Headless agents cannot show their own approval prompts, so desl is the approval loop. The rule is simple: the agent proposes, the broker decides by policy, the human decides what policy left open, and every decision is an event in the log.

**Flow.** An adapter emits `approval_request` with the tool, its arguments and a risk class (`read`, `edit`, `command`, `network`, `destructive`). The broker checks the thread's permission mode and its `always` list. Allowed requests are answered at once and logged. Everything else sets the thread to `waiting`, fires the bell, raises the thread in the sidebar and shows the one-line strip. The answer (`y`, `n`, `a`, or `e` to edit the command first) becomes an `approval_response` event and goes back to the adapter.

**Defaults.** New threads start in `edits` mode: reads and file edits inside the project are allowed, commands ask. A thread created with `--worktree` defaults to `worktree` mode. `yolo` is never a default and must be set explicitly per thread; the status bar shows `⚠ yolo` for as long as it is on.

**Destructive classification.** Commands matching a built-in list (`rm -rf`, `git push --force`, `git reset --hard`, `drop table`, package publish commands, anything touching paths outside the worktree) are classed `destructive` and always ask, even in `worktree` mode. The list is extensible in `config.toml` and the agent is told, in the preamble, that these will prompt.

**Worktree isolation.** When a thread has a worktree, the broker rejects file edits and commands whose paths resolve outside it, regardless of mode. This is the main reason worktrees are one keystroke away: they make broad permissions safe.

**Secrets.** desl never stores agent credentials; it relies on each agent's own login. Environment variables passed to agent processes are the user's shell environment minus a denylist (`AWS_SECRET_ACCESS_KEY`, `*_TOKEN`, `*_SECRET`, configurable). Briefs are scanned for strings that look like keys before being sent to another agent and the user is warned.

**Audit.** `desl log <thread>` prints every approval, who answered it and when. The export format includes them.

**Remote access.** When the daemon listens on TCP, it requires a token, binds to localhost unless `--host` is given, and prints the pairing URL and a QR code the way `t3 serve` does. Clients over TCP get the same permissions as local ones; there is no read-only mode in phase 1.

## Performance and reliability requirements

These are release gates, measured in CI on a reference machine (4-core laptop, 16 GB) with 20 threads and 50k events in the store. A regression fails the build.

| Metric | Budget |
| --- | --- |
| Cold start to usable TUI, daemon already running | < 150 ms |
| Cold start including daemon launch | < 1.5 s |
| Frame redraw during token streaming | < 16 ms p99, no dropped tokens |
| Thread switch in the sidebar (20 threads, 2k events each) | < 50 ms |
| Prompt send to first token on screen (excluding agent latency) | < 30 ms |
| Brief generation, 4k-token budget, 500-event thread | < 200 ms |
| Client memory, 10 open threads | < 100 MB |
| Daemon memory, 20 live agents idle | < 150 MB plus agent processes |
| Daemon CPU at idle | < 0.5% |
| Binary size, stripped | < 25 MB |
| Store growth | < 1 KB per event excluding tool output blobs |

**Rendering.** Token deltas are batched per animation frame; the transcript is a virtual list that renders only the visible window plus a margin; markdown is parsed incrementally and cached per message; diffs are rendered lazily on expand. Resize is debounced at 50 ms. No full-screen redraw on a single event.

**Crash safety.** The store is written in WAL mode with one transaction per event, so nothing that reached the screen as an event is lost; a daemon crash mid-turn loses at most the in-flight assistant delta, and the thread says so and resumes its native session on the next prompt. Agent process crashes are detected within one second, marked `failed` with the exit reason, and offered a one-key restart that resumes the native session. Client crashes lose nothing. `desl doctor` checks store integrity and can rebuild indexes.

**Process hygiene.** Every agent process is in its own process group; `desl daemon stop` and daemon restart send SIGTERM, wait 5 s, then SIGKILL, and never leave orphans. Idle agent processes are reaped after a configurable timeout (default 30 min) and transparently resumed on the next prompt. PTY scrollback is capped (default 10k lines) and persisted.

**Backpressure.** An agent that emits output faster than the client can render is buffered in the daemon, never blocked; the client shows `↓ n new` and catches up. Tool outputs over 1 MB are truncated in the IR with the full text in a blob.

**Upgrades.** `desl upgrade` replaces the binary atomically, restarts the daemon, and reconnects clients; running agent turns are allowed to finish first. Store migrations run on daemon start, forward-only, with a backup of the previous database kept for one version.

**Observability.** `desl daemon logs` tails structured logs with per-thread and per-adapter filters. `DESL_DEBUG=1` adds native event dumps to the logs for adapter debugging. No telemetry leaves the machine unless the user opts in with `desl telemetry on`, which sends crash reports and anonymous performance counters only.

## Distribution, install and configuration

desl ships as one static binary per platform with no runtime dependencies, installed by a one-line script, and configured by files that are optional on day one.

**Language and stack.** Rust, for the production build from phase 1 on. The product is judged on rendering (streaming without stutter, nothing moving under the reader's eyes, sub-16 ms frames with many threads live), and that is won with cell-level control of the terminal and no runtime pauses. Ratatui's immediate-mode model draws exactly the cells wanted each frame and emits the minimal diff, which is what "only the new tokens change" needs; the tools in this space whose feel is worth copying (Herdr, tutti, Codex CLI) are all Rust. Chosen crates: `ratatui` and `crossterm` (TUI), `rusqlite` with bundled SQLite (store), `clap` (CLI), `serde` and `serde_json` (protocol and adapters), `tracing` with `tracing-subscriber` (logs), `pulldown-cmark` (markdown in the transcript), `sha2` (content addresses for blobs), `chrono` (local clock time), `unicode-width` (column widths), `anyhow` (errors at the edges), `portable-pty` (raw panes, from phase 2), `cargo-dist` (signed releases, installer, Homebrew tap), `cargo-zigbuild` or `cross` (targets). The daemon runs on plain threads and channels: one engine thread owns all state, and each agent stream and each client has its own reader. `tokio` was the first choice and is not used; it comes back only if remote clients in phase 3 need it. Git is read through the `git` binary; `git2` is not used. Phase 0 prototypes are throwaway and may be written in whatever is fastest to tune by hand (Ink, Textual, Bubble Tea); nothing from them is kept except the measurements and the spec changes they produce. Go was considered for its iteration speed while adapter protocols are unstable; that argument is removed by settling the adapter contract in phase 0 before the Rust build starts.

**Targets.** macOS arm64 and amd64, Linux amd64 and arm64 (glibc and musl), Windows amd64 (phase 3, with named pipes and ConPTY). FreeBSD on a best-effort basis.

**Install.**

```
curl -fsSL https://desl.dev/install.sh | sh     # detects OS and arch, verifies checksum, installs to ~/.local/bin
brew install desl-dev/tap/desl                # macOS and Linuxbrew
cargo install desl                              # from source
```

The install script never needs sudo, prints what it will do before doing it, and supports `DESL_INSTALL_DIR` and `DESL_VERSION`. Releases are signed and carry SBOMs. `desl upgrade` uses the same channel.

**Paths** (XDG on Linux and macOS, `%APPDATA%` on Windows):

| Path | Holds |
| --- | --- |
| `$XDG_CONFIG_HOME/desl/config.toml` | User settings |
| `$XDG_CONFIG_HOME/desl/keys.toml` | Key remaps |
| `$XDG_DATA_HOME/desl/workspace.db` | The store |
| `$XDG_DATA_HOME/desl/blobs/` | Large tool outputs, content-addressed |
| `$XDG_STATE_HOME/desl/daemon.log` | Logs |
| `$XDG_RUNTIME_DIR/desl.sock` | Daemon socket |
| `<repo>/.desl/config.toml` | Per-project settings, safe to commit |

**Configuration.** TOML, every key optional, every default sane. Project config overrides user config. `desl config show` prints the effective configuration with the source of each value.

```
[general]
default_agent = "claude"
prefix = "ctrl+b"
theme = "default"

[agents.claude]
model = "claude-opus-5-5"
extra_args = []

[agents.codex]
model = "gpt-5-codex"

[permissions]
default_mode = "edits"
destructive_patterns = ["rm -rf", "git push --force"]

[brief]
token_budget = 4000
agent_summary = false

[worktrees]
dir = ".worktrees"
branch_prefix = "desl/"
```

**Environment.** `DESL_HOME` overrides all paths at once (for CI and tests). `NO_COLOR`, `TERM`, `COLORTERM` and `$EDITOR` are honoured. `DESL_SOCKET` points a client at a non-default daemon.

**Shell integration.** `desl completion bash|zsh|fish` prints completions; the installer offers to add them. A `desl` skill file is installable into Claude Code and Codex with `desl skill install`.

## Roadmap

Four phases, each shippable on its own, each gated by a test the previous phase must pass. Phase 0 is a prototype of the experience, not of the technology: the one decision that is expensive to reverse is how the product feels, so that is settled first, hands-on, before the engineering that serves it begins.

&#91;embedded content: roadmap · four phases, a gate after each\]

Each diamond is a gate: the phase after it does not start until the gate's test passes.

**Phase 0, the experience prototype (weeks 1 to 3).** Two throwaway prototypes of the same experience, built for feel and not for reuse: a sidebar with a few threads, one transcript pane, the approval strip, `/switch` between Claude Code and Codex, and detach/reattach. Prototype A renders the conversation itself from headless adapters (`claude -p` stream-json, `codex exec --json`). Prototype B runs the agents' own TUIs in PTY panes and keeps continuity by tailing their transcript files, the way tandem does. The same ten target users (people who already use both agents and tmux or lazygit daily) try both in the hallway protocol, blind to which is which. Every pixel, keystroke, sound and timing in the walkthrough and the feel bar is tuned here, on whichever prototype is being tested, and the winning details are written back into this spec. Gate: one approach wins on the feel bar with a median of 4 or better on moments 1, 2, 3, 4 and 6, and the losing prototype is deleted. No daemon, no store, no adapter contract is built in this phase; anything reusable is a bonus, not a goal. **Outcome (2026-11-04):** Prototype A won. The author decided alone after using both prototypes, in place of the ten-user sessions; no scores were taken. Prototype B was deleted. Prototype A stays in `phase0/` as the reference the phase 1 client is rebuilt to match.

**Phase 1, the real thing (weeks 4 to 8).** The core built properly under the winning approach: daemon with socket protocol, SQLite store, event IR, Claude and Codex adapters with fixtures, projector, thread engine, approval broker; then the Ratatui client rebuilt to match the phase 0 winner pixel for pixel, with sidebar, transcript, input, approvals strip, status bar, help overlay, command palette, keymap and themes. Full CLI with `--json`. Detach and reattach. `desl doctor`, installer, cargo-dist pipeline. Gate: all five walkthrough scenarios pass on macOS and Linux; a 40-turn Claude thread switched to Codex continues correctly in three scripted scenarios and a fork from message 12 reproduces the parent's state, verified by an independent reviewer; every performance budget is met in CI; the author runs the hallway protocol on the build and every moment passes.

**Phase 2, panes and worktrees (weeks 9 to 11).** Pane splits, zoom, PTY host, raw agent and shell panes, worktree-per-thread with `/diff` and `/pr`, destructive-command classification, idle process reaping, the desl skill for agents. Gate: 20 concurrent threads across 5 worktrees for 8 hours with no orphaned processes, no lost events and memory within budget.

**Phase 3, reach (weeks 12 to 15).** Remote TCP mode with pairing, Windows support, `desl export`/`import` for moving threads between machines, agent-written briefs, third-party adapter protocol with one community adapter (OpenCode) built against it, notifications (desktop and terminal). Gate: a thread started on a laptop is continued on a server from a phone browser's SSH client; an external contributor ships an adapter without touching core.

After phase 3: team features (shared briefs, thread links), a web client on the same protocol, and whatever the community asks for loudest.

## Quality bar and acceptance criteria

desl is done when a demanding terminal user cannot tell which parts were written by an agent. That means tests that run the real thing, review by someone who did not write the code, and a definition of done that includes the help text and the error messages.

**Test strategy.**

- Adapter replay tests: every adapter has fixtures of real native output per agent version; a test replays each fixture and asserts the exact IR. New agent versions add fixtures, never edit old ones.
- Projector golden tests: a set of event logs with expected briefs, byte for byte. Any change to extraction rules updates the goldens in the same PR with a reviewer's sign-off.
- Thread engine property tests: random sequences of prompt, switch, fork and crash against a model of the expected high-water marks and bindings. Invariants: indices are dense, `seen_up_to ≤ head` always, a fork's events equal its parent's prefix, no event is ever lost.
- Daemon integration tests: a fake agent binary that speaks each native protocol, so the full daemon runs in CI without API keys. Covers crash, restart, reap, resume, orphan detection and socket reconnect.
- TUI snapshot tests: `Ratatui's TestBackend` renders each screen at 80×24 and 200×50 and compares against approved snapshots; keymap tests assert every documented key does what the table says.
- End-to-end tests against the real `claude` and `codex` binaries, run nightly with real keys, executing the five walkthrough scenarios.
- Performance tests: the budgets table, run in CI on every PR, with results posted as a comment.

**Code quality rules.** `cargo clippy` with `-D warnings` and the pedantic group enabled, `rustfmt`, `cargo deny` for licences and advisories, `cargo audit`, `#![forbid(unsafe_code)]` everywhere except the PTY crate boundary, and `cargo test` under Miri for the few crates that touch raw memory. No crate may depend on a native agent format except its own adapter. Public items have doc comments and `cargo doc` builds without warnings. Error messages are complete sentences that say what happened and what to do next. No `unwrap` outside tests; errors carry context with `anyhow` at the edges and typed errors inside. No TODOs in `main`.

**UX quality rules.** Every state visible on screen has a name in the vocabulary table. Every error the user can see has been read aloud once. Every key in the keymap appears in the `?` overlay. Every command's `--help` fits on one screen. Colours pass contrast checks on the five built-in themes in light and dark terminals. The app is usable with `NO_COLOR=1` and with a screen reader reading the status line.

**Independent review.** Each phase gate is verified by a reviewer who did not write the phase's code, running the gate script from a clean machine using only the README. They file what they tripped on, and those filings are fixed before the gate passes.

**Definition of done for a feature.**

- [ ] Behaviour matches this spec, or the spec was updated in the same PR
- [ ] Unit, integration and, where it touches the screen, snapshot tests added
- [ ] Help overlay, `--help` and docs updated
- [ ] Error paths tried by hand and messages checked
- [ ] Performance budgets still met
- [ ] Reviewed by someone who did not write it

**Documentation.** A README that gets someone to a first reply in under a minute of reading; `docs/protocol.md` for the daemon protocol; `docs/adapters.md` for the adapter contract with a worked example; `docs/keys.md` generated from the keymap; a CHANGELOG in Keep a Changelog format. The agents building desl read this spec as `SPEC.md` in the repo root and keep `CLAUDE.md` and `AGENTS.md` pointing to it.

## Open questions

Decisions the building agent should raise before phase 0 ends, with the current lean for each.

- [ ] **Name collision check.** Confirm `desl` is free on npm, crates.io, Homebrew, GitHub and as a `.dev` domain; the first pass on 2026-11-04 found it clean. Fallback: segue, not `muster`.
- [x] **Exact headless flags.** Pinned to Claude Code 2.1.312 and Codex CLI 0.165.0; the real flags are recorded in `docs/adapters.md`.
- [x] **Approval transport for Claude Code.** Approvals arrive in the stream as `control_request` lines with `--permission-prompts host --permission-prompt-tool stdio`, and are answered on stdin. A plain subprocess drives it; no Node sidecar.
- [x] **Codex approval policy.** `codex exec` has no per-call approvals; its sandbox decides. `ask` degrades to `edits` for Codex (the UI does not say so yet). `codex app-server` is the route to per-call approvals and to streamed replies.
- [x] **Token estimation.** Bytes divided by four, always shown with `~`.
- [ ] **Store sync.** Append-only event log makes later sync possible; decide whether phase 3 ships one-shot `export`/`import` only or a live sync over the remote protocol.
- [ ] **Agent preamble wording.** The short system-prompt addition that tells an agent it is inside desl (thread name, worktree, what will prompt for approval); draft and test with both agents for unintended behaviour changes.
- [ ] **Raw PTY import.** How far to go in importing a raw pane's native session file into a thread after the fact; proposal is best-effort, clearly labelled, never blocking.
- [ ] **Prefix inside tmux.** Default to `ctrl+a` when `$TMUX` is set, or always ask on first run?
- [ ] **Default theme.** Terminal palette versus a shipped theme; proposal is terminal palette, with the accent colour taken from the terminal's blue.
- [ ] **Windows timing.** Phase 3 as written, or earlier if the first users ask.
