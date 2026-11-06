# desl

Read `AGENTS.md` first, then `SPEC.md`. Both apply in full to every task in this repository.

Short version: desl (say *diesel*) is a terminal workspace for coding agents, judged on experience, not technology. The vocabulary, keys, non-goals, adapter boundary and performance budgets in `SPEC.md` are fixed; a change that needs them to move updates `SPEC.md` in the same commit.

Current phase: **phase 1, the real thing**, on the native route: each agent runs its own interface in a terminal the daemon owns, desl draws it beside a sidebar through its own emulator, and a switch copies the conversation into a new session of the other agent. The daemon, store, adapters and client live in `crates/`. The phase 0 prototypes were removed from the tree and remain in the git history.
