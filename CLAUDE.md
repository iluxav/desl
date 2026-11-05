# desl

Read `AGENTS.md` first, then `SPEC.md`. Both apply in full to every task in this repository.

Short version: desl (say *diesel*) is a terminal workspace for coding agents, judged on experience, not technology. The vocabulary, keys, non-goals, adapter boundary and performance budgets in `SPEC.md` are fixed; a change that needs them to move updates `SPEC.md` in the same commit.

Current phase: **phase 1, the real thing.** The daemon, store, adapters and client live in `crates/`. Phase 0 ended on 2026-11-04 with Prototype A (headless agents, desl draws the conversation); on 2026-11-05 the author reversed it for the native route: the agents' own interfaces passed straight through, and a switch that copies the conversation into a new session of the other agent. `phase0/proto-n` is the reference for that; Prototype A in `phase0/proto-a` remains the reference for the overlays' look. `crates/` is being moved from the headless route to the native one.
