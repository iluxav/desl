# phase 0

Prototype A, the winner of phase 0: desl draws the whole conversation itself from the agents'
headless streams. It is throwaway code, kept as the reference the phase 1 client is rebuilt to
match. Prototype B (the agents' own interfaces in panes), the blind launcher and the hallway kit
were removed when A was chosen; they are in the repository's first commit.

- `proto-a/` the crate: host, client, the Claude and Codex adapters, the brief projector.
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
