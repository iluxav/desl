# phase 0

Throwaway prototypes of the desl experience. Nothing here is kept after the gate except
`recordings/`, `fixtures/` and the changes they cause in `SPEC.md` and `docs/`.

- `proto-a/` the shared crate: Prototype A (`proto-a`, rendered: drives `claude -p` stream-json and `codex exec --json` and draws everything itself) and Prototype B (`proto-b`, native: the agents' own TUIs in PTY panes under the same chrome).
- `fixtures/{claude,codex}/` real captured native output per agent version and scenario.
- `recordings/` terminal recordings for the kickoff deliverables (`.cast` plays in asciinema; `.out` + `.timing` replay with `scriptreplay -t`).
- `tools/` the tmux-driven key tests (`e2e*.sh`), the demo recorders (`record*.sh`), the `script` to asciicast converter, and the hallway kit: `hallway-setup.sh` (demo repository), `hallway-session.sh` (records a session), `hallway-break.sh` (task 6) and `gate_report.py` (the gate numbers). The protocol itself is in `docs/hallway/`.

## Install for hands-on testing

From the repository root, `make install` builds release binaries and puts `proto-a` and `proto-b`
and the launcher `desl-proto` in `~/.local/bin` (no sudo). `make uninstall` removes them and stops running hosts; `make purge`
also deletes the hosts' logs. `make test` runs the tests.

## Prototype A

```
cd <some git repo you trust>
proto-a           # attach; starts the project's host if needed
proto-a status    # the sidebar as text
proto-a stop      # stop the host; its log is kept
```

Needs `claude` and `codex` on PATH and logged in. Quitting detaches; agents keep running in the
host. The host's log is `$XDG_STATE_HOME/desl-proto-a/<key>.jsonl`, its stderr is next to it, and
its socket is `$XDG_RUNTIME_DIR/desl-proto-a/<key>.sock`. Delete the log to start the project over.

The test and recorder scripts take `DESL_DEMO` (a small git repo to run in) and `DESL_BIN`.

## Prototype B

```
cd <some git repo you trust>
proto-b           # attach; starts the project's host, which runs Claude Code's own TUI in a pane
proto-b status    # the sidebar as text
proto-b stop      # stop the host; this ends its agents
```

Every unprefixed key goes to the agent. `ctrl+b ?` lists desl's keys; `ctrl+b n` opens a thread on
Claude or Codex; `/switch codex` typed into the agent's input is taken by desl; `ctrl+b d` or
`/quit` detaches and leaves the agents running.

## The blind launcher

```
desl-proto        # picks A or B for a new session, rejoins a running one, logs which
desl-proto log    # the sessions so far, for the observer
desl-proto a      # force a prototype (also `b`)
```
