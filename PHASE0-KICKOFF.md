# Phase 0 kickoff — the experience prototype

Paste this as the first message to the agent that will build phase 0. It assumes `SPEC.md`, `AGENTS.md` and `CLAUDE.md` are in the repo root.

---

You are starting phase 0 of desl. Read `AGENTS.md`, then `SPEC.md` end to end, before doing anything else. Then confirm back to me, in your own words and under 200 words: what desl is, the nine feel-bar moments, and what phase 0 is and is not.

## Goal of this phase

Two throwaway prototypes of the same experience, so ten real users can tell us which approach *feels* right before any production code is written. We are prototyping the UI and the moments, not the architecture. Nothing you build here is kept except measurements, recordings and changes to `SPEC.md`.

## The experience to prototype (identical in both)

From "User experience walkthrough" and "Interface design":

- The default layout: narrow sidebar with a few threads and a WAITING group, one transcript pane, the bordered input, the one-line status bar. Must read well at 80×24 and 200×50.
- One thread on Claude Code and one on Codex, with real agents (use the user's own logins; no API keys in the prototype).
- The daily loop: streaming transcript, tool calls collapsed to one line each and expandable with `enter`, the `↓ n new` pill when the user scrolls up, the approval strip above the input with `y`/`n`/`a`, the bell.
- `/switch codex` and `/switch claude` with the one-line confirmation and the thin divider, continuing the conversation in the other agent via a deterministic brief in the exact format in "Cross-agent continuity".
- Detach and reattach (a background process the prototype can be closed and reopened against is enough; it does not need to be the real daemon).
- `?` help overlay generated from the keymap, `/` command palette, `ctrl+c` interrupting the agent and never the app.
- Every key in the keymap table, with the tmux prefix.

Build nothing else. No panes, no worktrees, no forks, no config files, no themes beyond the terminal palette.

## The two prototypes

**Prototype A — rendered.** Drive the agents headless (`claude -p --output-format stream-json --input-format stream-json`, `codex exec --json`) and render the conversation yourself. You own every pixel.

**Prototype B — native.** Run the agents' own TUIs inside PTY panes and keep continuity by tailing their transcript files (`~/.claude/projects/<slug>/<id>.jsonl`, `~/.codex/sessions/.../rollout-*.jsonl`), the way tandem does. Switching injects the brief as the next user turn in the other agent's real TUI.

Both must look the same from the outside: same sidebar, same status bar, same keys, same divider on switch. The user must not be able to tell which is which from the chrome.

Use whatever stack is fastest to tune by hand: Ink, Textual, Bubble Tea, or raw Rust if you are quicker there. Pick one stack for both prototypes so the comparison is about the approach, not the toolkit. Verify every `claude` and `codex` flag against `--help` on the installed versions before using it, and record the versions and flags you confirmed in `docs/adapters.md`.

## Deliverables, in this order

1. **Day 1:** a walking skeleton of Prototype A — sidebar, transcript, input, one real Claude Code turn streaming. Record a 30-second terminal recording and send it to me before going further.
2. **End of week 1:** Prototype A complete for the experience above; Prototype B's PTY panes running the real TUIs.
3. **End of week 2:** both prototypes complete, blind-labelled "A" and "B" in a launcher that picks one at random per session and logs which.
4. **Week 3:** the hallway protocol from "The feel bar" with ten users: 45-minute sessions, screen and audio recorded, tasks given as goals not instructions, scores 1–5 per moment, one sentence of expectation per task. You prepare the task script, the scoring sheet and the recording setup; I run the sessions.
5. **Gate report:** per prototype, the median and minimum per feel-bar moment, every 1 or 2 with a recording timestamp, and a one-page recommendation. Then a list of concrete changes to `SPEC.md` ("Interface design", "Commands, keybindings and CLI", "Cross-agent continuity") that the winning prototype taught us, as a PR against the spec.

## Rules for this phase

- Optimise for feel over code quality; this code is deleted after the gate. But never cut the corners that affect feel: no flicker, no reflow under the reader, no lag on keypress, no missed bell.
- Any time you are about to make a UI decision the spec does not settle (spacing, a glyph, a timing, the wording of a line), write it down in `docs/phase0-decisions.md` with the alternatives you considered. Those decisions are the real output of this phase.
- If either agent's headless mode cannot do something the walkthrough needs (per-call approvals, forking, resume), say so immediately with the exact command you ran and its output. Do not work around it silently.
- Ask me before spending more than half a day on anything not listed above.

Start with the 200-word confirmation.
