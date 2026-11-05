# desl

Say *diesel*. One persistent terminal workspace for your coding agents: run Claude Code and Codex
side by side, keep them working after you detach, and move a conversation from one to the other
without starting over.

```
╭ desl ─────────────┬──────────────────────────────────────────────────────────╮
│ PROJECTS          │ t-7f3a · claude · main                                   │
│ ▾ api (3)         │   ▾ edit src/auth/middleware.ts                    +2 −1 │
│ ▸ ○ t-7f3a  auth  │   │  9 -   const token = header.slice(7);                │
│   ◐ t-91c2  tests │   │  9 +   const token = header.startsWith("Bearer ") ?  │
│   ○ t-04be  docs  │   │      header.slice(7) : null;                         │
│ ▾ web (1)         │   ▸ run pnpm test                                      ✓ │
│   ○ t-5d10  landi…│                                                          │
│                   │   Rotation is in. Want me to add the revocation list?    │
│                   │                                                          │
│ WAITING (1)       │ ╭──────────────────────────────────────────────────────╮ │
│   ◐ t-91c2        │ │ ›                                                    │ │
│                   │ ╰──────────────────────────────────────────────────────╯ │
├───────────────────┴──────────────────────────────────────────────────────────┤
│ ○ idle  claude  main ↑2                               ctrl+b ? help  │ 12:41 │
╰──────────────────────────────────────────────────────────────────────────────╯
```

## First reply in a minute

You need `claude` or `codex` on your PATH, logged in the way you already use them. desl drives
them under your own logins and never calls a model itself.

```
make install        # builds and puts desl in ~/.local/bin
cd your-repo
desl
```

Type a prompt and press enter. That is all there is to set up.

## What to try next

| Do this | And this happens |
| --- | --- |
| `/switch` | Shows the agents, with the one you are on marked. Pick Codex and it continues the thread: desl hands it a brief of what happened, and switching back later sends Claude only what it missed. `/switch codex` skips the list. |
| `/model` | Shows the current agent's own models, with the one in use marked, then that model's reasoning effort levels. The pick takes effect with your next prompt and keeps the conversation. `/model sonnet high` skips both lists. |
| `/fork` | A new thread with this one's history, to try another way. `/fork codex @4` forks from message 4 to Codex. |
| `ctrl+n` | A new thread. `ctrl+t` jumps to any thread; `ctrl+1` to `ctrl+9` jump by position. |
| `y`, `n`, `a` | Answer when an agent asks before running something. `a` allows that tool for the rest of the thread. |
| `ctrl+c` | Stops the agent's turn. It never quits desl. |
| `/quit` or `ctrl+b d` | Detaches. The agents keep working; `desl` brings you back to the same screen. |
| `?` | Every key that works right now. |

Keys come from tmux, lazygit, fzf, Claude Code and Codex, so most of them you already know.
The full list is in [docs/keys.md](docs/keys.md).

## From a script

Everything the screen does, the command line does, with `--json` when you want to parse it.

```
desl status                      # the sidebar as text
desl new -m "Fix the flaky test" # a new thread with a first prompt; prints its id
desl send t-7f3a "And the docs"  # send a prompt, stream the reply to stdout
desl wait t-7f3a                 # block until the thread needs you or is done
desl switch t-7f3a codex
desl model t-7f3a                # the agent's models; add a name to pick one, --effort to set how hard it thinks
desl fork t-7f3a claude --at 12
desl approve t-7f3a              # or: desl deny t-7f3a
desl brief t-7f3a                # the handoff brief desl would send now
desl doctor                      # agents, logins, the daemon, the store
desl daemon stop                 # ends the agents; every thread is kept
```

## How it works

`desl` is one binary with two roles. A daemon owns everything that matters: an append-only log
of every thread in SQLite, the agent processes, and the approvals. The screen is a thin client
that can be closed, killed or reattached at any time without losing a word.

desl owns the conversation; the agents' own sessions are caches. Each agent that has touched a
thread has a mark for how far it has read. When you switch, desl builds a brief from exactly the
part the other agent has not seen and sends it with your next prompt.

- [SPEC.md](SPEC.md) is the product specification and the source of truth.
- [docs/protocol.md](docs/protocol.md) is the daemon protocol.
- [docs/adapters.md](docs/adapters.md) is the adapter contract and what is known about each agent.
- [docs/CHANGELOG.md](docs/CHANGELOG.md) is what changed.

## Building

```
make check          # clippy, formatting and every test
make install        # release build into ~/.local/bin; stops a running daemon of the old build
make uninstall
```

desl keeps its store in `$XDG_DATA_HOME/desl`, its log in `$XDG_STATE_HOME/desl` and its socket in
`$XDG_RUNTIME_DIR`. `DESL_HOME` moves all three into one directory.

`phase0/` holds the experience prototype the interface was tuned on. It is kept as a reference
and is not part of the build.
