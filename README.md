# desl

Say *diesel*. One persistent terminal workspace for your coding agents. Run Claude Code and Codex
side by side in their own interfaces, keep them working after you detach, and move a
conversation from one to the other without starting over.

```
╭ Projects ───────── 1 waiting ╮╭ ✻ Claude · ~/src/api · opus · high ───────── ○ idle t-7f3a ╮
│                              ││ ▐▛███▛█   Claude Code                                     │
│ api  ~/src                   ││                                                           │
│  ✻ Add refresh-token rot… 2m ││ ❯ Add refresh-token rotation to the auth middleware       │
│   └─ ✓ Explore · find auth   ││   Read 2 files                                            │
│                              ││ ● Rotation is in. Want me to add the revocation list?     │
│  ◎ Fix the flaky test   now  ││                                                           │
│                              ││ ❯ _                                                       │
│  $ bash                      ││                                                           │
╰──────────────────────────────╯╰───────────────────────────────────────────────────────────╯
   ctrl+space   s switch agent   c new   o open dir   w threads   n p next prev   ? help
```

Every thread runs the agent's own interface, so its streaming, tool calls, approval dialogs,
`/` commands, skills and MCP servers work exactly as they do without desl. desl adds the
sidebar, the keys after its prefix, and the switch.

## Install

You need `claude` or `codex` on your PATH, logged in the way you already use them. desl runs
them under your own logins and never calls a model itself.

```
curl -fsSL https://raw.githubusercontent.com/iluxav/desl/main/install.sh | sh
```

The script picks the release build for your system (Linux or macOS, x86_64 or arm64), checks
its SHA-256 and installs `desl` into `~/.local/bin`. It never needs sudo.
`DESL_INSTALL_DIR=/some/dir` puts it elsewhere; `DESL_VERSION=v0.1.0` pins a release.

From source, with a recent stable Rust:

```
make install        # release build into ~/.local/bin; stops a running daemon of the old build
```

Then, inside a repository:

```
desl
```

## Keys

desl owns one key, the prefix: `ctrl+space`. Every other key goes to the agent.

| After `ctrl+space` | |
| --- | --- |
| `s` | Switch this thread to the other agent. desl copies the conversation into a new session of Claude or Codex and the screen changes in one frame. |
| `c` | New thread: a menu of Claude, Codex and your shell. `w` in the menu gives it a git worktree of its own. |
| `o` | Open another directory: a shell to `cd` to it, then `c` starts an agent there. |
| `f` | Fork this conversation into a new thread, on either agent. |
| `w` | Move into the sidebar (`j`/`k`, `enter`, `esc`). A click works too. |
| `n` `p` `1`…`9` `l` | Next, previous, by number, the one before. |
| `,` `i` `x` | Rename, info, close. |
| `b` `?` `d` | Hide the sidebar, help, detach (the agents keep working). |

A thread's logo in the sidebar is its status: it pulses while the agent works, turns yellow
when it asks you something, and fades when idle. Subagents hang under their thread; click one
to read its conversation.

## From a script

```
desl status                       # the thread list as text
desl new -m "Fix the flaky test"  # a new thread with a first prompt
desl send t-7f3a "And the docs"   # type a prompt into the thread's agent, print the reply
desl wait t-7f3a                  # block until the thread needs you or is done
desl switch t-7f3a codex
desl fork t-7f3a claude --worktree
desl config show                  # the settings in effect and where each comes from
desl doctor                       # agents, logins, the daemon, the store
desl daemon stop                  # ends the agents; every thread is kept
```

Every command takes `--json`.

## Settings

All optional. `~/.config/desl/config.toml`, and `<repo>/.desl/config.toml` over it:

```toml
[general]
prefix = "ctrl+space"      # or ctrl+a … ctrl+z
default_agent = "claude"

[permissions]
default_mode = "edits"     # ask, edits or yolo

[worktrees]
dir = ".worktrees"
branch_prefix = "desl/"
```

## How it works

`desl` is one binary with two roles. A daemon owns the agents, each in a terminal of its own,
and a SQLite log of every thread; it listens only on a Unix socket in your runtime directory.
The screen is a client that draws the agent through its own terminal emulator and can be
closed or reattached at any time without losing a word. The agents' own session files are
read as they grow, and a switch writes the conversation into a new session file of the other
agent before starting it.

- [SPEC.md](SPEC.md) is the product specification.
- [docs/protocol.md](docs/protocol.md) is the daemon protocol.
- [docs/adapters.md](docs/adapters.md) is what is known about each agent's files and flags.

desl keeps its store in `$XDG_DATA_HOME/desl`, its log in `$XDG_STATE_HOME/desl` and its socket in
`$XDG_RUNTIME_DIR`. `DESL_HOME` moves all three into one directory.

## Developing

```
make check          # clippy with warnings denied, formatting, every test
make build          # release build only
```
