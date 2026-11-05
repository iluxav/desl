# desl — working rules for coding agents

You are building **desl** (say *diesel*): a single-binary terminal workspace that runs many coding agents side by side, keeps them alive after the user detaches, and lets one conversation move between agents without losing context.

The product specification is `SPEC.md` in this directory. It is the source of truth. Read it fully before your first change, and re-read the section you are working in before each task. When the code and the spec disagree, the spec wins unless this task explicitly changes the spec; in that case update `SPEC.md` in the same change.

## What matters most

The experience is the product. Every tool in this space already has the technology (Herdr, T3 Code, baton, tandem, switchboard, tutti, rvr, corral, continuo). desl wins on the first thirty seconds, on keystrokes the user never notices, and on moments where it anticipated them. Before you add anything, ask which of the nine moments in "The feel bar" it serves. If none, do not add it.

## Rules

1. **Vocabulary is fixed.** Use only the words in the "Naming and vocabulary" table: workspace, project, thread, agent, session, pane, worktree, brief, switch, fork, attach, detach. Status words are working, waiting, idle, failed, detached. Never say "session" for a desl thread or "thread" for an agent's native session. "Engine" and "fuel" appear only in brand copy, never in the UI.
2. **Borrow, never invent.** Keys, commands and layout come from tmux, lazygit, fzf, Claude Code and Codex, as listed in "Commands, keybindings and CLI". A new concept gets a new word only when no community word fits.
3. **Non-goals are final.** No model API calls, no plugin runtime, no workflow engine, no dashboard, no IDE features, no cloud, no agents beyond Claude Code and Codex before phase 3, no configuration required for anything. If a task asks for one of these, stop and say so.
4. **Adapters are the only code that knows a native format.** Nothing outside `adapters/<id>/` may parse Claude Code or Codex output, flags or session files. Each adapter ships with `fixtures/` of real captured output per agent version and a replay test; an adapter that fails its fixtures does not merge.
5. **Verify every CLI flag.** Flag names in `SPEC.md` for `claude` and `codex` are from memory. Before using one, run `claude --help` or `codex exec --help` against the pinned version and record what you found in `docs/adapters.md`.
6. **Every error is a complete sentence** that says what happened and what to do next, with a key to press where one applies. No stack traces or bare codes reach the user.
7. **Performance budgets are release gates**, not goals. The table in "Performance and reliability requirements" runs in CI; a regression fails the build.
8. **Nothing is ever lost.** Thread state is an append-only log. Never write a change that could drop an event, a diff or an approval, even on crash.
9. **Review before done.** The definition of done in "Quality bar and acceptance criteria" applies to every change: spec match, tests, help text, error paths tried by hand, budgets, and a review by someone who did not write it.

## Phases

- **Phase 0 (done 2026-11-04, reversed 2026-11-05): the experience prototype.** Prototype A (desl renders the conversation from headless agents) won first; the author reversed it for the native route after `phase0/proto-n` showed a seamless switch between the agents' own interfaces. The reasons are in the phase 0 outcome in `SPEC.md`.
- **Phase 1 (now): the real thing**, in Rust (ratatui, crossterm, portable-pty, vt100, rusqlite, clap, serde, tracing; the stack paragraph of `SPEC.md` has the full list), on the native route: the switch behaves as in `phase0/proto-n`, the overlays look like Prototype A. The author is the only tester.
- **Phase 2: panes and worktrees. Phase 3: reach.** See the roadmap in `SPEC.md`.

## Layout

```
crates/
  desl/            binary: client TUI + CLI (thin; talks to the daemon over the socket)
  desl-daemon/     thread engine, store, projector, approval broker, git service, PTY host, event bus
  desl-proto/      the daemon protocol (versioned, documented in docs/protocol.md)
  desl-ir/         the event IR and Thread/Binding types
  adapters/
    claude/        fixtures/, replay tests, the Claude Code adapter
    codex/         fixtures/, replay tests, the Codex adapter
    pty/           the raw PTY adapter (phase 2)
docs/
  protocol.md  adapters.md  keys.md (generated)  CHANGELOG.md  phase0-decisions.md
phase0/            proto-n, the reference for the native switch; proto-a, the look of the overlays; its own workspace, not part of the build
SPEC.md  AGENTS.md  CLAUDE.md  README.md
```

## How to work

- Start each task by stating which spec section it serves and which feel-bar moment it protects.
- Keep changes small and reviewable. One behaviour per change.
- Run `make check` (`cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test`) before declaring anything done; the performance suite joins it when it exists.
- Expected outputs are files: adapter IR (`fixtures/*.ir.json`), briefs (`goldens/`), screens (`snapshots/`), `docs/keys.md`. `DESL_BLESS=1 cargo test` rewrites them after a deliberate change, and the diff is reviewed like code.
- When a decision in "Open questions" comes up, resolve it in `SPEC.md` first, then implement.
- Never add a dependency without noting it in `docs/adapters.md` or the stack paragraph of `SPEC.md`.
- Never mention "Drover" anywhere; it was a working label that collided with existing tools.

## The spec's two diagrams

`SPEC.md` exported from the design doc replaces two drawings with placeholders. They say:

- **Architecture.** Three clients (the TUI, the CLI and scripts, a later remote client) talk over a Unix socket in JSON frames to one daemon. Inside the daemon: a thread engine (the core), a projector for the fallback brief and attention in the first row; a SQLite store, a git service, a PTY host and an event bus in the second; Claude and Codex adapters in the third. Each agent runs its own interface in a PTY the daemon owns (`claude --session-id|--resume`, `codex --no-daemon [resume]`); its adapter reads and writes the agent's session files. The drawing predates the native route of 2026-11-05 and still shows headless processes (`claude -p`, `codex exec --json`).
- **Roadmap.** Four bands left to right — Feel prototype (weeks 1–3), The real thing (weeks 4–8), Panes and worktrees (weeks 9–11), Reach (weeks 12–15) — with a gate after each: users pick the feel; five scenarios and budgets met; twenty threads for eight hours with no leaks; phone-to-server handoff.
