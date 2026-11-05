# phase 0

Prototype A, the winner of phase 0: desl draws the whole conversation itself from the agents'
headless streams. It is throwaway code, kept as the reference the phase 1 client is rebuilt to
match. Prototype B (the agents' own interfaces in panes), the blind launcher and the hallway kit
were removed when A was chosen; they are in the repository's first commit.

- `proto-a/` the crate: host, client, the Claude and Codex adapters, the brief projector.
- `proto-n/` a throwaway test of the native route reopened on 2026-11-05: the agent's own interface passed straight through, and `ctrl+b s` continues the same conversation in the other agent (see below).
- `recordings/` terminal recordings of Prototype A (`.cast` plays in asciinema; `.out` + `.timing` replay with `scriptreplay -t`).
- `tools/` the tmux-driven key tests (`e2e*.sh`), the demo recorders (`record*.sh`) and the `script` to asciicast converter.

The decisions made while building it are in `docs/phase0-decisions.md`; what was learned about
the agents' flags and output is in `docs/adapters.md`.

## Install

From the repository root, `make install` builds a release binary and puts `proto-a` in
`~/.local/bin` (no sudo). `make uninstall` removes it and stops running hosts; `make purge` also
deletes the hosts' logs. `make test` runs the tests.

## Use

```
cd <some git repo you trust>
proto-a           # attach; starts the project's host if needed
proto-a status    # the sidebar as text
proto-a stop      # stop the host; its log is kept
```

Needs `claude` and `codex` on PATH and logged in. `?` lists the keys. `/quit` or `ctrl+b d`
detaches; the agents keep running in the host. The host's log is
`$XDG_STATE_HOME/desl-proto-a/<key>.jsonl`, its stderr is next to it, and its socket is
`$XDG_RUNTIME_DIR/desl-proto-a/<key>.sock`. Delete the log to start the project over.

The test and recorder scripts take `DESL_DEMO` (a small git repo to run in) and `DESL_BIN`.

## proto-n, the native switch test

```
cd <some git repo you trust>
~/Projects/desl/phase0/target/release/proto-n claude    # or: proto-n codex
```

The agent's own interface runs as if desl were not there: its keys, its scrollback, its mouse.
desl owns only `ctrl+b`:

- `ctrl+b s` switch: the conversation so far is written into a new session of the other agent,
  which starts out of sight; once it has drawn that history the screen changes to it in one frame.
- `ctrl+b q` leave: both agents stop, and the line printed names the session to resume.
- `ctrl+b ctrl+b` sends `ctrl+b` to the agent.

The terminal title says which agent is showing (`desl · claude`). Every switch leaves a new session
in that agent's own `--resume` list. `PROTO_N_CLAUDE_ARGS` and `PROTO_N_CODEX_ARGS` add flags to
every start (the tests use `--model haiku` and `-m gpt-6-luna -c model_reasoning_effort=low`).
Build it with `cargo build -p proto-n --release` in `phase0/`.
