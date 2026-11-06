# desl — Product Specification

Nov 4, 2026 · @Ilya Vinokurov · revised Nov 5, 2026 for the native route

## Vision and positioning

desl (say diesel) is a single-binary terminal workspace that runs many coding agents side by side, keeps them alive after you detach, and lets one conversation move between agents without losing context. It is the terminal-native tool for people who already use Claude Code and Codex and want one place to run, watch, switch and fork them.

**The gap.** Two good tools each solve half the problem. Herdr (Rust, tmux-style) gives persistence, fleet visibility and an attention queue, but every agent owns its own conversation, so there is no way to continue a Codex thread in Claude. T3 Code (TypeScript, web/desktop GUI) owns threads through provider adapters and supports worktrees per thread, but it has no terminal UI and no cross-agent handoff. desl combines both halves in the terminal and adds the piece neither has: portable conversation state.

**The one-line pitch.** Run every agent in one persistent terminal workspace; switch or fork any thread to any agent, mid-conversation.

**Who it is for.**

- Engineers who run 2 to 10 coding agents in parallel across worktrees and lose track of which one needs them.
- Engineers who use more than one agent and want to pick the best model per task without starting over.
- Teams running agents on remote boxes over SSH who need detach, reattach and a mobile glance later.

**What desl is not.** Not a new coding agent, not a model router, not an IDE. It drives the agents the user already has, with the user's own subscriptions and logins, on the user's own machine.

**Success looks like.** A user starts a thread in Claude Code, presses `ctrl+b s`, and Codex opens on the same conversation and continues as if it had been there all along. They close the laptop, SSH in from a phone, and every agent is still running. They never read a manual, because every key and command works the way tmux, lazygit, Claude Code and Codex already taught them.

## User experience walkthrough

These five scenarios are the acceptance script. Each must work exactly as written, with no step added.

**First run.** The user runs `curl -fsSL https://desl.dev/install.sh | sh`, then `desl` inside a repo. desl starts the daemon in the background, detects `claude` and `codex` on PATH and whether each is logged in, registers the repo as a project, and opens one thread on the default agent: the projects and their threads on the left, and on the right the agent's own interface, exactly as if the user had typed `claude`, under a one-line header naming the project, agent and model. The terminal title reads `desl · claude · t-7f3a`. Total time under ten seconds; nothing to configure. If no agent is found, desl says which ones it supports and how to install each, then exits cleanly.

**The daily loop.** The user types into the agent's own prompt and everything on screen is the agent's own: its streaming, its tool calls, its approval dialogs, its `/` commands, skills and MCP servers, its mouse wheel. desl owns only the prefix key. When the agent asks a question or for approval, the thread turns `waiting`: its glyph in the sidebar changes, and if the user is looking at another thread the bell fires once and the title gains `1 waiting`. `prefix c` opens a second thread, `prefix n` / `prefix p` move between threads, a click on a thread in the sidebar opens it, and `prefix w` moves the keys into the sidebar (`j`/`k`, `enter`, `esc`).

**Switch.** Mid-thread, the user presses `prefix s`. desl copies the conversation so far into a new session of the other agent, starts that agent out of sight, and once it has drawn the conversation the screen changes to it in one frame: the same prompts, replies and tool calls, now in Codex's interface. The title says `codex`. The next prompt goes to Codex, which answers it knowing everything that came before. `prefix s` again copies the turns Codex added back into a new Claude session the same way. The user never sees a native session id.

**Fork.** The user presses `prefix f` and picks an agent to try a different approach from the current point. A new thread opens, named `t-7f3a/1`, with the parent's conversation up to here in a fresh session of the chosen agent, and becomes the one on screen. From the CLI, `desl fork t-7f3a claude --at 12 --worktree` forks from message 12 onto its own branch. The parent is untouched.

**Detach and return.** `prefix d` detaches; all agents keep working. From another machine, `ssh box && desl` reattaches to the same workspace with every thread where it was, the screen and its scrollback included. `desl status` from any shell prints the thread list as text; `desl wait t-7f3a` blocks until that thread needs attention, for use in scripts.

Throughout, three things are always true: the current project, agent and model are in the header, every thread and its status are in the sidebar, and the bottom line shows the common keys, with `prefix ?` for all of them.

## The feel bar

Nine named moments. Each is a testable experience target, scored by the author in the hallway protocol below, and each has an owner in the roadmap. They sit beside the performance budgets with equal weight: a release that misses a feel moment does not ship, the same as one that misses a latency number.

1. **First reply in ten seconds.** From a finished install to a first agent reply, a new user reads nothing, configures nothing, and is never asked a question they do not understand.
2. **Keys you already know.** A user who knows tmux and Claude Code discovers switch, fork, detach and the waiting list within ten minutes, unprompted, and never hits a key that does something surprising.
3. **Switching feels like changing the model.** `prefix s` shows no seam: the other agent opens on the whole conversation, never on a blank screen; the next reply answers the user's question with that history in mind; the user never sees or types a native session id.
4. **Nothing moves under your eyes.** desl never draws over the agent's pane unasked. A switch, a thread change or a reattach replaces the screen in one frame: no blank frame, no half-drawn screen, no flicker.
5. **Waiting is impossible to miss and impossible to be nagged by.** One bell, one mark in the title, one place at the top of the thread list. Never a modal from desl, never a repeat, never a notification for something already answered.
6. **The agent is the hero.** The pane is the agent's own interface, as large as the chrome allows; desl's chrome is a quiet frame around it: a sidebar, a header line and a key line, and the sidebar hides with `prefix b`.
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
4. **The agent is the star.** The agent's own interface fills the pane. desl's chrome is a frame around it in the style of Herdr and T3 Code: one accent colour, no banners, no animation that does not carry information.
5. **Interrupt the agent, never the app.** Every key without the prefix belongs to the agent, `ctrl+c` included. Quitting desl is a deliberate command, and the daemon keeps running regardless.
6. **Everything is scriptable.** Every action desl owns exists as a CLI subcommand with `--json` output. Agents can drive desl through the same interface a human uses.
7. **Fast by default.** Keys reach the agent with no delay a person could notice; a switch or a thread change lands in one frame. Performance is a feature with a budget, not a cleanup phase.
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
| brief | The handoff summary desl sends when a thread's conversation cannot be copied into the other agent's session | new |
| switch | Continue this thread with a different agent | new |
| fork | Copy a thread up to a point into a new thread | git, Claude Code |
| detach / attach | Leave or rejoin the workspace, agents keep running | tmux |

Status words, used identically in the thread list, the CLI and notifications:

| Status | Meaning | Colour role |
| --- | --- | --- |
| working | The agent is generating or running a tool | accent |
| waiting | The agent asked a question or needs an approval | warning |
| idle | The last turn finished, nothing pending | muted |
| failed | The agent process exited abnormally | critical |
| detached | Pane is running but no client is attached | muted |

Verbs in commands and help follow the agents' own grammar: `resume`, `clear`, `compact`, `export`, `model` keep their Claude Code and Codex meanings. desl adds `switch`, `fork`, `brief`, `worktree`, `attach`, `detach`. Never use "session" for a desl thread or "thread" for an agent's native session.

Identifiers: thread ids are short, typeable and prefixed, in the style of git short SHAs: `t-7f3a`. Native session ids are shown only on request (`prefix i`, `desl thread show`) and never required from the user.

## Interface design

The workspace screen, in the style of Herdr and T3 Code in a terminal: projects and their threads on the left, the shown thread's agent on the right under a one-line header, and the common keys along the bottom. The agent runs its own interface in a terminal the daemon owns; desl draws it through its own terminal emulator into the pane, and passes the user's keys straight to it, so the agent's colours, keys, dialogs and mouse are its own. desl owns one key, the prefix. It must read well at 80×24 and scale to 300×80 without changing shape; the sidebar takes three tenths of the width (32 to 52 columns) and steps aside below 80.

```
╭ Projects ───── 1 waiting ╮╭ Claude · ~/src/api · opus · high ──────────── ○ idle t-7f3a ╮
│                          ││ ▐▛███▛█   Claude Code v2.1.318                               │
│ api  ~/src               ││                                                              │
│ ▌ ● claude  auth         ││ ❯ Add refresh-token rotation to the auth middleware          │
│   ◐ codex   tests        ││   Read 2 files                                               │
│   ○ bash                 ││ ● Rotation is in. Want me to add the revocation list?        │
│                          ││                                                              │
│ web  ~/src               ││ ❯ _                                                          │
│   ○ claude  new          ││                                                              │
╰──────────────────────────╯╰──────────────────────────────────────────────────────────────╯
   ctrl+b   s switch agent   c new   o open dir   w threads   n p next prev   ? help
  terminal title: desl · claude · t-7f3a · 1 waiting
```

**Panels.** Two rounded panels in the style of lazygit, with a column of padding inside; the one that has the keys has an accent border. **Sidebar.** Every project, its folder's name with the directory it lives in dimmer beside it, and under it each thread on two lines: its status glyph and title (the first line of its first prompt, at most 48 characters, user-editable), then dimmer its agent's mark, agent, status and age, and its branch when it is not the main one; in a fixed order (project, then oldest first) that `prefix n`, `prefix p` and `prefix 1`…`9` follow. The shown thread's two lines are tinted with a shade mixed from the terminal's own background (asked with OSC 11 at start), never a bar. Agent marks: with a Nerd Font, the Claude and OpenAI logos (`cod-claude`, `cod-openai`) and a terminal for a shell; without, a one-cell badge in the agent's colour (`✻` on Claude's orange, `›` on light grey, `$` on dark grey). desl uses the Nerd Font marks when one is installed and the terminal is local; `DESL_ICONS=nerd` or `text` decides instead. A click opens a thread; `prefix w` gives the sidebar the keys (`j`/`k` move, `enter` opens, `esc` returns to the agent); `prefix b` hides it and gives the agent the width. A shell thread is titled by its shell's name (`bash`) and shows its directory. The agent panel's title carries the same mark. Under a thread, hanging from it with tree lines (`├─`, `└─`), the newest four of the helpers its agent started in its current session (Claude's subagents from `<session>/subagents/*.meta.json`, Codex's from the spawns in its session file): `●` running, `✓` done, `○` left unfinished by a stopped turn. A click (or `enter` in the sidebar) shows that subagent's conversation in the pane, read-only, laid out like a transcript and read again every second while it runs; the panel's title says `Subagent · <description>` and `● running` or `✓ done`; `j`/`k`, `g`/`G` and the wheel scroll, `esc` returns to the agent.

**Opening a project.** Any directory is a project: its git root when it has one, else the directory itself. `prefix o` opens the user's own shell (`$SHELL`, with their whole environment) in the pane, in the directory above the current project; the user `cd`s with the tools they know, and `prefix c` registers where the shell is as a project and starts the default agent there, and the shell moves under that project. A shell that exits is removed. When the shown thread's program ends, the pane moves to the thread shown before; desl ends only when no thread is left to show.

**Header.** The agent panel's top border: the agent, the project, and its model and effort as the thread's session last used them; on the right the thread's status and id, or, while scrolled back, how far.

**Key line.** The common keys after the prefix, lit while the prefix is armed; a notice from desl (why a switch did not happen, say) takes its place until the next key. Never drawn over the agent.

**Scrolling.** The wheel over the pane goes to the agent when it asked for the mouse, and moves an agent on the alternate screen as its arrow keys; otherwise it scrolls desl's own scrollback of the agent's output (10,000 lines), and the next key typed returns to the bottom. Holding shift selects text with the terminal's own selection.

**Terminal title.** Always `desl · <agent> · <thread>`, with `· n waiting` while other threads need the user.

**Changing what is on screen.** A switch, a thread change and a reattach all work the same way: the agent to be shown has been drawing into a hidden terminal kept by the daemon, desl replays that output into a fresh emulator for the pane, and paints only the cells that differ, inside one synchronized-output frame (`CSI ? 2026 h … l`). The old screen stays until the new one is complete; there is no blank or half-drawn frame. While the new agent is getting ready, keys typed are held and delivered to it.

**Approvals and questions** are the agent's own dialogs, answered in the agent's own way. desl only notices them (see Agent adapters) to mark the thread `waiting` in the sidebar and ring the bell for a thread that is not on screen.

**Errors and notices** from desl appear in the key line, never drawn over the agent, until the next key.

**Visual language.** One accent colour (default: the terminal's blue), one warning, one critical, three greys. Status glyphs: `●` working, `◐` waiting, `○` idle, `✕` failed, `⊝` detached, with the words always available for screen readers and `NO_COLOR`. Rounded box drawing where the terminal supports it, falling back to ASCII. No emoji in chrome. Respect the terminal's own palette by default; built-in themes (`default`, `mono`, `catppuccin`, `gruvbox`, `solarized`) are opt-in.

**Splits (phase 2).** `prefix %` and `prefix "` show two threads side by side in the pane, as in tmux, through the same emulator; `prefix z` zooms one back. Shell panes work the same way.

**Help.** `prefix ?` opens a box over the pane listing the keys desl owns, grouped by purpose, in the style of lazygit, with one row saying that every other key goes to the agent. The overlay is generated from the keymap, never hand-written, so it is always correct.

## Commands, keybindings and CLI

Two surfaces, one command set: prefixed keys and the CLI. desl has no prompt of its own; a `/` command typed into the agent goes to the agent, so its own commands, skills and MCP prompts work unchanged. Every prefixed key has a CLI equivalent, and the help overlay and `desl --help` are generated from the same registry.

**Keys.** The prefix is `ctrl+b` by default (`ctrl+a` and `ctrl+space` offered on first run); pressed twice it sends `ctrl+b` to the agent. desl recognises the prefix in every encoding an agent may switch the terminal into: the plain control byte, the kitty keyboard protocol and xterm's modifyOtherKeys. Every key without the prefix belongs to the agent.

| Key | Action | Source |
| --- | --- | --- |
| `prefix s` | Switch this thread to the other agent; with more than two agents, a list with the current one marked | desl |
| `prefix f` | Fork from here: pick the agent, a new thread opens with the conversation so far | desl |
| `prefix c` | New thread in this project: a menu of the agents and the user's shell, the cursor on the project's default (`j`/`k`, `enter`, `1`…`9`, `esc`); from a shell, in the shell's directory, which becomes a project | tmux |
| `prefix o` | Open a directory: a shell beside this project to `cd` to it, then `prefix c` | desl |
| `prefix w` | Move the keys to the sidebar's thread list | tmux |
| `prefix n` / `prefix p` | Next / previous thread | tmux |
| `prefix 1`…`prefix 9` | Jump to thread n in the list | tmux |
| `prefix l` | Back to the thread shown before | tmux |
| `prefix ,` | Rename this thread | tmux |
| `prefix i` | Agent, native session id, model, worktree | desl |
| `prefix x` | Close this thread after a `y` in the key line: its agent or shell ends and it leaves the sidebar; the agent's own history keeps the conversation (`desl thread rm` from the CLI) | tmux |
| `prefix b` | Hide or show the sidebar | desl |
| `prefix d` | Detach | tmux |
| `prefix ?` | Help overlay | lazygit |
| `prefix prefix` | Send the prefix key to the agent | tmux |

All keys are remappable in `keys.toml` by named command, with `vim` and `emacs` presets. desl detects when it runs inside tmux and offers to change its prefix on first run.

**Non-interactive CLI.** Every command takes `--json` and exits non-zero on failure.

```
desl                         attach to the workspace (starts the daemon if needed)
desl status                  the thread list as text
desl thread list|show|rename|rm
desl new [-p project] [--agent claude] [--worktree] [-m "prompt"]
desl send <thread> "prompt"  type the prompt into the thread's agent and print the reply when the turn ends
desl switch <thread> <agent>
desl model <thread> [name] [--effort level]   the model desl starts the agent with when it opens a session
desl permissions <thread> [mode]   the permission mode desl starts the agent with
desl fork <thread> <agent> [--at N] [--worktree]
desl brief <thread>          print the handoff brief, the fallback for a copy
desl wait <thread>           block until the thread is waiting or idle
desl worktree new|list|rm
desl agents                  detected agents and login state
desl doctor                  check binaries, logins, daemon, permissions
desl daemon start|stop|restart|logs
desl upgrade
```

Inside the agent, its own `/model` and permission keys work as always; `desl model` and `desl permissions` decide what desl passes when it opens a session for the thread (a switch, a fork, a restart). The lists they show come from the agents, as before.

The CLI is how scripts, CI and other agents drive desl. A `desl` skill file for Claude Code and Codex (shipped in the repo, installable with `desl skill install`) teaches agents to open threads, wait on them and fork work, the way Herdr's skill does.

## Core concepts and data model

desl owns the conversation; the agents' native sessions are working copies. Every thread is an append-only list of provider-neutral events, read by the adapters from the session file of whichever agent the thread is on. Each agent that has touched the thread holds a binding that records the native session it has open for the thread and how much of the thread that session holds.

**Thread.**

```
Thread {
  id:          "t-7f3a"            // short, typeable, unique per workspace
  project_id:  "api"
  name:        "Add refresh-token rotation to the auth"  // first line of the first prompt, user-editable
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
  model_choice: "sonnet" | null     // picked with desl model; null follows the agent's default
  effort_choice: "high" | null      // reasoning effort picked with desl model; null follows the agent's own setting
  last_used_at
}
```

`seen_up_to` is the high-water mark. When the thread head is ahead of an agent's mark, that agent's session is missing exactly the events between the two; a switch back gives it a new session holding the whole thread (see Cross-agent continuity), and the brief, when the fallback is used, covers that range alone.

**Events (the IR).** One schema for every agent. Adapters translate the agent's session file into these, and these back into a new session file, and nothing else.

| Kind | Fields | Notes |
| --- | --- | --- |
| user\_message | text, attachments\[\] | What the user sent |
| assistant\_message | text, agent, model | Final prose for a turn; streamed deltas are not stored separately |
| tool\_call | id, name, args, agent | `read`, `edit`, `bash`, `search`, or the agent's own name |
| tool\_result | call\_id, output, exit\_code, is\_error, truncated, blob | Output stored in full on disk, truncated in memory; `blob` names the full text |
| file\_change | path, diff, kind (create/modify/delete), call\_id | Derived from tool results or git, so handoffs know what changed; `call_id` ties it to the edit that made it |
| approval\_request | call\_id, tool, summary | The agent showed its own approval dialog or question; the thread is `waiting` |
| approval\_response | call\_id, decision (allow/deny) | Read back from the session file once the agent records the answer |
| agent\_switch | from, to, session\_id, brief\_event\_id | The thread moved to another agent; `brief_event_id` only when the fallback was used |
| fork | parent\_id, fork\_point | First event of a forked thread |
| brief | text, token\_estimate, covers\_range, delta | The fallback handoff text, when it was sent |
| usage | agent, input\_tokens, output\_tokens, cost\_estimate | Per turn |
| error | message, recoverable | Process crashes, adapter failures |
| interrupt |  | The user stopped the turn |

Every event carries `index`, `thread_id`, `timestamp` and `agent`. Indices start at 1, are dense and are never reused, so a fork point or a high-water mark is always an exact reference; a mark of 0 means the agent has seen nothing. Tool calls use neutral names (`read`, `edit`, `bash`, `search`, `fetch`, `agent`, `todo`) and neutral argument keys (`path`, `command`, `pattern`, `query`, `url`, `description`), so a client can show any agent's work without knowing the agent.

**Project.** A registered git repository: id, path, default agent, default branch, list of worktrees. Projects are discovered from `cwd` on first run and can be added with `desl project add`.

**Workspace.** The set of projects, threads, panes and client layouts the daemon holds for one user on one machine. Each live agent's terminal output is kept by the daemon (capped, see Process hygiene) so reattach restores the exact screen and its scrollback.

**Storage.** SQLite, one file at `$XDG_DATA_HOME/desl/workspace.db`, WAL mode. Events are rows; large tool outputs live in content-addressed files beside the database and are referenced by hash. The schema has a version and every migration is forward-only. A `desl export <thread>` writes a self-contained JSONL that `desl import` can load on another machine, which is the sync path for a later release.

## Architecture

One binary, two roles: a long-lived daemon that owns all state and processes, and a thin client that renders it. The TUI, the CLI and any future remote client are all clients of the same daemon over one protocol.

&#91;embedded content: architecture · clients, daemon, adapters, agent processes\]

Clients never touch agent processes or the store directly; everything goes through the daemon, which is why detach, reattach and remote access cost nothing extra.

**Daemon** (`desl daemon`). Starts on first use, runs as a user-level background process (launchd agent on macOS, systemd user unit on Linux, Windows service later). Owns the SQLite store, every agent process and the PTY it runs in, the git service and the event bus. Listens on a Unix socket at `$XDG_RUNTIME_DIR/desl.sock` (named pipe on Windows), optionally on TCP with a token for remote clients. One daemon per user per machine; projects are a dimension inside it.

**Client** (`desl`, the TUI). Connects to the socket, passes the shown thread's terminal output to the real terminal and the user's keys to its agent, draws the overlays, and sends commands. Holds no state that matters beyond which thread is shown. Can be killed at any time. The CLI subcommands are the same client without the screen.

**Protocol.** Length-prefixed JSON frames over the socket (msgpack if profiling shows JSON on the hot path). Three message families: `command` (client → daemon, with a request id and a typed reply), `event` (daemon → client, every IR event plus workspace changes, as they happen), and `pty` (the raw terminal bytes of the thread a client shows, both ways, sent only to that client). The protocol is versioned and documented in `docs/protocol.md`; the daemon refuses clients with a mismatched major version and tells them to upgrade.

**Thread engine.** The core state machine. Starts each thread's agent in a PTY through its adapter, follows the agent's session file and appends what the adapter reads from it as events, keeps each thread's status from the file and the screen, and carries out switch and fork (see Cross-agent continuity). Serialises all writes to one thread; threads run concurrently.

**Adapters.** One package per agent implementing a single interface: the command line that starts or resumes the agent's interface, a reader from its session file to IR events, a writer from IR events to a new session, and a status reader for its screen. Adapters are the only code that knows a native format. See Agent adapters.

**Projector.** Pure function from an event range to a brief, the fallback when a copy cannot be written. Deterministic, no network. Produces structured text with fixed headings so the receiving agent can rely on its shape. See Cross-agent continuity.

**Git service.** Wraps `git` for worktree create/list/remove, branch state, diff and PR creation (through `gh` when present). Emits `file_change` events by diffing the worktree before and after each agent turn, which makes handoffs accurate even when an agent's tool output is opaque.

**PTY host.** Every agent runs in a PTY the daemon owns. The host keeps each one's output (capped) so a client can show it, answers the terminal queries an agent sends while no client shows it (cursor position, device attributes, colours, keyboard protocol) the way the user's terminal answered them, and keeps a screen model (`vt100`) per PTY for status. Shell panes from phase 2 use the same host.

**Attention.** Receives the adapters' `waiting` readings, rings the bell once for a thread not on screen, updates the title and the thread list, and clears all of it when the thread moves on.

**Event bus.** In-process pub/sub inside the daemon. Every store write publishes; clients, the status line, notifications and the `desl wait` command all subscribe. Nothing reads the store to find out what changed.

## Cross-agent continuity

Switch and fork are both projections of the thread's event log into a different native session. desl never edits a session an agent wrote. It writes a new session in the receiving agent's own format, holding the conversation, and starts the agent's interface on it with the agent's supported resume path. The receiving agent then has the conversation as its own history: every prompt and reply, and every tool call written as one of its own tools with the output (each output capped). Thinking is not carried: Claude's is signed and Codex's reasoning is encrypted, so neither can read the other's.

**Switch algorithm** (`prefix s` on thread T, from agent A to agent B):

1. Read A's session file through A's adapter and append the events it added since the thread's head.
2. Ask B's adapter to write a new session holding events `1..head`. Record it as B's binding with `seen_up_to = head`. Append an `agent_switch` event and set `T.active_agent = B`.
3. Start B's interface on that session in a hidden PTY, at the size of the client's terminal.
4. When B has drawn and gone quiet (output, then 600 ms of none, at least 1.2 s after start; 15 s at most), replace the client's screen with B's output in one synchronized frame. Keys typed meanwhile go to B.
5. End A's process. Its session stays in A's own resume list.
6. If B's adapter cannot write a session (an agent version without fixtures, a failed write) or B quits before it is ready, fall back: start a new session of B with the **brief** for `1..head` pasted into its prompt, and say so on the strip. If that fails too, stay on A and say why.

A switch is refused while A is `working`, with a line that says so and offers to wait; a half-finished turn is never copied.

**Fork algorithm** (`prefix f` or `desl fork <thread> <agent> [--at n] [--worktree]` on thread T):

1. Create thread F with `parent_id = T.id`, `fork_point = n` (default head), `project_id = T.project_id`, name `T.name` plus a `/k` suffix, empty bindings.
2. Copy events `1..n` into F (by reference to the same content blobs, so forks are cheap).
3. If `--worktree`, create a git worktree from T's current commit and set `F.worktree`.
4. If `agent == T.active_agent` and `n == head`, fork the native session (`claude --resume <id> --fork-session`, `codex fork <id>`) and bind it.
5. Otherwise write a new session from events `1..n` as in a switch.
6. Show F.

**Brief format** (the fallback). Deterministic markdown with fixed headings, so agents learn its shape. Sections are omitted when empty.

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

**What is not carried.** The receiving agent does not get the previous agent's thinking, system prompt, permission state, compaction summaries or message ids; tool-call ids are rewritten into the receiver's shape. Only the IR crosses between adapters: tests must assert that a session one adapter writes holds nothing the other adapter's reader did not put into events.

## Agent adapters

An adapter is the only code allowed to know an agent's native format. It knows how to start the agent's own interface, how to read the agent's session file into IR events, how to write IR events out as a new session the agent will resume, and how to tell from the screen that the agent is waiting. Two adapters ship in phase 1, and the contract is public so the community can add more.

**Adapter interface.**

```
interface Adapter {
  id():            "claude" | "codex" | ...
  detect():        { installed: bool, version, logged_in: bool, binary_path }
  capabilities():  { copy_in, fork, model_choice, permission_modes }
  launch(opts: { session: New(id?) | Resume(id) | Fork(id), model?, effort?, permission_mode, cwd }) → argv
  session_file(cwd, session_id?, started_at) → path      // finds a session the agent named itself
  read(path, from) → [IREvent]                           // events after the ones already read
  write(events, cwd) → session_id                        // a new session holding exactly these events
  status(screen, path) → working | waiting | idle        // from the session file first, the screen for waiting
}
```

Every adapter lives in `adapters/<id>/` with a `fixtures/` directory of real session files per supported agent version and replay tests that assert the IR read from each, plus round-trip tests: events written by `write` and read back by `read` are the same events, and a written session resumes in the real agent in the nightly end-to-end run. An adapter that fails its fixture tests cannot ship.

**Claude Code adapter.** Starts `claude --session-id <uuid>` for a new thread and `claude --resume <id>` for a written or existing session, with `--permission-mode` from the thread's mode and `--model`/`--effort` when chosen. Reads `~/.claude/projects/<cwd with every non-alphanumeric character as ->/<id>.jsonl`: user lines (prompts, tool results), assistant lines (text, tool use; thinking ignored), skipping subagent lines and Claude Code's own tagged records. Writes a new file of chained lines (`uuid`, `parentUuid`, `sessionId`, `cwd`), tool ids in the `toolu_` shape. Waiting: the permission dialog or a question on screen while a turn is open in the file. Verify flag names against `claude --help` for the pinned version; they move.

**Codex adapter.** Starts `codex --no-daemon` for a new thread and `codex --no-daemon resume <id>` for a written or existing session, with sandbox and approval flags from the thread's mode and `-m`/`-c model_reasoning_effort` when chosen; `--no-daemon` keeps the thread out of Codex's shared app-server, which would otherwise hold it after the interface quits. Finds a new thread's session as the newest rollout for the cwd begun after the start. Reads `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`: `response_item` messages, tool calls (`exec` scripts unpacked to their commands) and outputs. Writes a rollout with a `session_meta` header carrying `"history_mode": "paginated"`, one turn per user message bracketed by `task_started`/`task_complete`, `item_completed` events for messages and commands (what Codex's screen is built from), and no ids on assistant messages or tool calls (the API rejects ids it did not issue). Waiting: Codex's approval prompt on screen. Verify flag names against `codex --help` for the pinned version.

**Agents without an adapter (phase 2).** Any other agent's interface, or a shell, runs in a thread the same way, with status from output heuristics (prompt detection, idle time) as Herdr does, but no events: switch and fork are disabled on it and `prefix s` says why.

**Permission modes.** `ask` (every edit and command needs a human; reads inside the project do not), `edits` (reads and edits allowed, commands ask), `worktree` (everything allowed inside the thread's worktree, nothing outside; from phase 2), `yolo` (everything allowed). Each adapter maps the mode to its agent's own launch flags, and the agent enforces it and asks in its own interface; in their interactive interfaces both agents can hold every mode as written. Inside the agent, its own mode keys change it for that session.

**Version pinning.** `desl doctor` reports each agent's version and whether the adapter has fixtures for it. A newer agent version than any fixture triggers a non-blocking warning, and switches into that agent use the brief until its fixtures exist. A CI job captures fresh fixtures from the latest agent releases weekly and opens a PR when the IR mapping changes.

**Extensibility.** A third-party adapter is a binary on PATH named `desl-adapter-<id>` speaking the adapter interface over stdio as JSON. desl discovers it at startup. This keeps the core binary small and lets the community cover OpenCode, Gemini CLI, Cursor Agent and others without forking.

## Permissions and safety

The agent runs its own interface, so the agent is the approval loop: it asks in its own dialog and enforces its own mode. desl decides what mode an agent starts in, makes sure a question is never missed, and keeps a record of what was asked and answered.

**Flow.** The adapter sees the agent's dialog or question on screen while a turn is open and reports `waiting`; desl appends an `approval_request`, rings the bell once if the thread is not on screen, adds it to the title count and the top of the thread list. When the agent records the answer in its session file, desl appends the `approval_response` and clears the mark.

**Defaults.** New threads start in `edits` mode. A thread created with `--worktree` defaults to `worktree` mode. `yolo` is never a default and must be set explicitly per thread with `desl permissions`; a fork does not inherit it, and the title shows `⚠ yolo` for as long as a thread's agent was started with it. A mode chosen with `desl permissions` takes effect when desl next opens a session for the thread; inside the agent, its own mode keys change it at once.

**Worktree isolation.** When a thread has a worktree, its agent starts with the worktree as its working directory and, for Codex, its sandbox root. desl does not sit between the agent and its tools, so it cannot veto a single call; isolation is the agent's own sandbox and permission mode, and the permissions list says which of the two each agent gives.

**Secrets.** desl never stores agent credentials; it relies on each agent's own login. Environment variables passed to agent processes are the user's shell environment minus a denylist (`AWS_SECRET_ACCESS_KEY`, `*_TOKEN`, `*_SECRET`, configurable). Conversations are scanned for strings that look like keys before they are copied into another agent's session, and the user is warned.

**Audit.** `desl log <thread>` prints every approval the agents asked for and how it was answered, as read from their session files. The export format includes them.

**Remote access.** When the daemon listens on TCP, it requires a token, binds to localhost unless `--host` is given, and prints the pairing URL and a QR code the way `t3 serve` does. Clients over TCP get the same permissions as local ones; there is no read-only mode in phase 1.

## Performance and reliability requirements

These are release gates, measured in CI on a reference machine (4-core laptop, 16 GB) with 20 threads and 50k events in the store. A regression fails the build.

| Metric | Budget |
| --- | --- |
| Cold start to usable TUI, daemon already running | < 150 ms |
| Cold start including daemon launch | < 1.5 s |
| Delay desl adds between a key and the agent, and between the agent's output and the terminal | < 2 ms p99 |
| Thread change to a live agent, key to new screen (20 threads, 2k events each) | < 100 ms, one frame |
| Switch, key to the other agent's screen (excluding the agent's own start) | < 300 ms of desl's own time; < 3 s in total on the reference agents |
| Copy of a 500-event thread into a new session | < 200 ms |
| Brief generation (the fallback), 4k-token budget, 500-event thread | < 200 ms |
| Client memory, 10 open threads | < 100 MB |
| Daemon memory, 20 live agents idle | < 150 MB plus agent processes |
| Daemon CPU at idle | < 0.5% |
| Binary size, stripped | < 25 MB |
| Store growth | < 1 KB per event excluding tool output blobs |

**Rendering.** The agent draws into desl's emulator; desl paints the changed cells at most once per 12 ms, each paint one synchronized-output frame, and passes the agent's key-mode requests (kitty keyboard, modifyOtherKeys, bracketed paste, focus reports) to the real terminal. desl answers the agent's questions to its terminal (cursor position, colours, attributes) from the user's terminal's answers, probed once. Resize goes to the agent's PTY at once and to hidden PTYs debounced at 50 ms.

**Crash safety.** The store is written in WAL mode with one transaction per event, so nothing that reached the screen as an event is lost; a daemon crash mid-turn loses at most the in-flight assistant delta, and the thread says so and resumes its native session on the next prompt. Agent process crashes are detected within one second, marked `failed` with the exit reason, and offered a one-key restart that resumes the native session. Client crashes lose nothing. `desl doctor` checks store integrity and can rebuild indexes.

**Process hygiene.** Every agent process is in its own process group; `desl daemon stop` and daemon restart send SIGTERM, wait 5 s, then SIGKILL, and never leave orphans. Idle agent processes are reaped after a configurable timeout (default 30 min) and resumed with their native session when the thread is next shown. Each PTY's kept output is capped (default 8 MB) and persisted; past the cap the oldest output is dropped and the screen is rebuilt from the agent's own resume on reattach.

**Backpressure.** An agent that emits output faster than the client can write it is buffered in the daemon, never blocked. Tool outputs over 1 MB are truncated in the IR with the full text in a blob.

**Upgrades.** `desl upgrade` replaces the binary atomically, restarts the daemon, and reconnects clients; running agent turns are allowed to finish first. Store migrations run on daemon start, forward-only, with a backup of the previous database kept for one version.

**Observability.** `desl daemon logs` tails structured logs with per-thread and per-adapter filters. `DESL_DEBUG=1` adds native event dumps and raw key bytes to the logs for adapter and key debugging. No telemetry leaves the machine unless the user opts in with `desl telemetry on`, which sends crash reports and anonymous performance counters only.

## Distribution, install and configuration

desl ships as one static binary per platform with no runtime dependencies, installed by a one-line script, and configured by files that are optional on day one.

**Language and stack.** Rust, for the production build from phase 1 on. The product is judged on what it adds to the agent's own interface: nothing between a key and the agent, screens that change in one frame, many live agents with no runtime pauses. That is won with byte-level control of the terminal; the tools in this space whose feel is worth copying (Herdr, tutti, Codex CLI) are all Rust. Chosen crates: `ratatui` and `crossterm` (overlays, raw mode), `portable-pty` (every agent's PTY), `vt100` (a screen model per PTY, for status and splits), `rusqlite` with bundled SQLite (store), `clap` (CLI), `serde` and `serde_json` (protocol and adapters), `tracing` with `tracing-subscriber` (logs), `sha2` (content addresses for blobs), `chrono` (timestamps, local clock time), `unicode-width` (column widths), `anyhow` (errors at the edges), `cargo-dist` (signed releases, installer, Homebrew tap), `cargo-zigbuild` or `cross` (targets); `pulldown-cmark` left with desl's own transcript. The daemon runs on plain threads and channels: one engine thread owns all state, and each agent PTY, each session file and each client has its own reader. `tokio` was the first choice and is not used; it comes back only if remote clients in phase 3 need it. Git is read through the `git` binary; `git2` is not used. Phase 0 prototypes are throwaway and may be written in whatever is fastest to tune by hand (Ink, Textual, Bubble Tea); nothing from them is kept except the measurements and the spec changes they produce. Go was considered for its iteration speed while adapter protocols are unstable; that argument is removed by settling the adapter contract in phase 0 before the Rust build starts.

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

[brief]            # the fallback when a conversation cannot be copied
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

**Phase 0, the experience prototype (weeks 1 to 3).** Two throwaway prototypes of the same experience, built for feel and not for reuse: a sidebar with a few threads, one transcript pane, the approval strip, `/switch` between Claude Code and Codex, and detach/reattach. Prototype A renders the conversation itself from headless adapters (`claude -p` stream-json, `codex exec --json`). Prototype B runs the agents' own TUIs in PTY panes and keeps continuity by tailing their transcript files, the way tandem does. The same ten target users (people who already use both agents and tmux or lazygit daily) try both in the hallway protocol, blind to which is which. Every pixel, keystroke, sound and timing in the walkthrough and the feel bar is tuned here, on whichever prototype is being tested, and the winning details are written back into this spec. Gate: one approach wins on the feel bar with a median of 4 or better on moments 1, 2, 3, 4 and 6, and the losing prototype is deleted. No daemon, no store, no adapter contract is built in this phase; anything reusable is a bonus, not a goal. **Outcome (2026-11-04):** Prototype A won. The author decided alone after using both prototypes, in place of the ten-user sessions; no scores were taken. Prototype B was deleted. **Reopened and reversed (2026-11-05):** headless `claude -p`, A's route, was measured to drain the five-hour limit about 2.4 times faster than Claude Code's own interface for the same work, and A had to rebuild every native feature (skills, MCP, plan mode, questions, approvals) through its adapters. What the author had disliked in B was the switch: it flickered and opened the other agent blank, with an empty terminal history. A third prototype, `phase0/proto-n`, fixed that by writing the conversation into a new session of the other agent and swapping the screen in one frame, and passes the agent straight through so the terminal's own scrollback stays. The author judged its switch seamless; the native route is the phase 1 approach. `phase0/proto-n` is the reference for the switch, Prototype A for the overlays' look.

**Phase 1, the real thing (weeks 4 to 8).** The core built properly under the native route: daemon with socket protocol, SQLite store, event IR, PTY host, Claude and Codex adapters that read and write session files with fixtures, projector for the fallback brief, thread engine, attention; then the client: pass-through of the shown thread, the switch of `phase0/proto-n`, the thread list, help and info overlays in Prototype A's visual language, keymap and themes. Full CLI with `--json`. Detach and reattach. `desl doctor`, installer, cargo-dist pipeline. Gate: all five walkthrough scenarios pass on macOS and Linux; a 40-turn Claude thread switched to Codex continues correctly in three scripted scenarios and a fork from message 12 reproduces the parent's state, verified by an independent reviewer; every performance budget is met in CI; the author runs the hallway protocol on the build and every moment passes.

**Phase 2, panes and worktrees (weeks 9 to 11).** Splits and zoom, threads for agents without an adapter, shell panes, worktree-per-thread with `desl diff` and `desl pr`, idle process reaping, the desl skill for agents. Gate: 20 concurrent threads across 5 worktrees for 8 hours with no orphaned processes, no lost events and memory within budget.

**Phase 3, reach (weeks 12 to 15).** Remote TCP mode with pairing, Windows support, `desl export`/`import` for moving threads between machines, agent-written briefs for the fallback, third-party adapter protocol with one community adapter (OpenCode) built against it, notifications (desktop and terminal). Gate: a thread started on a laptop is continued on a server from a phone browser's SSH client; an external contributor ships an adapter without touching core.

After phase 3: team features (shared briefs, thread links), a web client on the same protocol, and whatever the community asks for loudest.

## Quality bar and acceptance criteria

desl is done when a demanding terminal user cannot tell which parts were written by an agent. That means tests that run the real thing, review by someone who did not write the code, and a definition of done that includes the help text and the error messages.

**Test strategy.**

- Adapter replay tests: every adapter has fixtures of real session files per agent version; a test reads each fixture and asserts the exact IR, and a round-trip test writes IR out as a session and reads it back to the same events. New agent versions add fixtures, never edit old ones.
- Projector golden tests: a set of event logs with expected briefs, byte for byte. Any change to extraction rules updates the goldens in the same PR with a reviewer's sign-off.
- Thread engine property tests: random sequences of prompt, switch, fork and crash against a model of the expected high-water marks and bindings. Invariants: indices are dense, `seen_up_to ≤ head` always, a fork's events equal its parent's prefix, no event is ever lost.
- Daemon integration tests: a fake agent binary that draws a screen, asks the terminal what real agents ask and writes a session file in each native format, so the full daemon runs in CI without API keys. Covers crash, restart, reap, resume, orphan detection and socket reconnect.
- TUI snapshot tests: `Ratatui's TestBackend` renders each overlay at 80×24 and 200×50 and compares against approved snapshots; key tests feed the prefix in every keyboard encoding and assert every documented key does what the table says and every other key reaches the agent byte for byte.
- End-to-end tests against the real `claude` and `codex` binaries, run nightly with real keys, executing the five walkthrough scenarios, with the screen sampled during each switch to assert no blank or half-drawn frame.
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
- [x] **Codex approval policy.** Settled by the native route: Codex's own interface asks before calls, as in any Codex session. (Under the headless route, `codex exec` had no per-call approvals.)
- [x] **Token estimation.** Bytes divided by four, always shown with `~`.
- [ ] **Store sync.** Append-only event log makes later sync possible; decide whether phase 3 ships one-shot `export`/`import` only or a live sync over the remote protocol.
- [ ] **Agent preamble wording.** The short system-prompt addition that tells an agent it is inside desl (thread name, worktree, what will prompt for approval); draft and test with both agents for unintended behaviour changes.
- [x] **Raw PTY import.** Settled by the native route: every thread is an agent in a PTY whose session file the adapter reads.
- [ ] **Session formats move.** The copy depends on the agents' undocumented session files. Proposal: per-version fixtures, the weekly fixture job, and the brief as the fallback for any version without fixtures; decide whether to also ask Anthropic and OpenAI whether writing a session for resume is a supported path.
- [ ] **Prefix inside tmux.** Default to `ctrl+a` when `$TMUX` is set, or always ask on first run?
- [ ] **Default theme.** Terminal palette versus a shipped theme; proposal is terminal palette, with the accent colour taken from the terminal's blue.
- [ ] **Windows timing.** Phase 3 as written, or earlier if the first users ask.
