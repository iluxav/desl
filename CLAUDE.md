# desl

Read `AGENTS.md` first, then `SPEC.md`. Both apply in full to every task in this repository.

Short version: desl (say *diesel*) is a terminal workspace for coding agents, judged on experience, not technology. The vocabulary, keys, non-goals, adapter boundary and performance budgets in `SPEC.md` are fixed; a change that needs them to move updates `SPEC.md` in the same commit.

Current phase: **phase 1, the real thing.** The daemon, store, adapters and client live in `crates/`. The client matches Prototype A in `phase0/`, which is kept only as that reference. Phase 0 ended on 2026-11-04: Prototype A won, decided by the author's own testing.
