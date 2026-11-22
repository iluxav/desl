# Tasks as desl's first-class record: plan

Spec: `docs/design/2026-11-22-tasks-first-class.md`. Four milestones. For each one:
- Each Rust change gets a test that fails first.
- Page changes are checked live in the browser against an isolated daemon, with Playwright.
- `make check` passes.
- The milestone is committed and pushed to main.

There is no reviewer agent, per the author's choice.

## 1. Tasks in the daemon; the list on the page

**Model:**
- `desl_proto::tasks`: the parser, renderer and `Store`, moved from `crates/desl/src/tasks/list.rs`,
  which re-exports them.
- New statuses `ready` and `parked`; `open()` is todo, in-progress, issue and ready.
- New fields `thread` and `branch` on `Task`, written as field lines.
- `Store::apply` takes an exclusive `flock` on the project's folder around its read, change and
  write.

Tests:
- The new statuses and fields read and render.
- Two writers in a loop lose nothing.

**Daemon:**
- `Command::Tasks { project_id }` replies `Reply::Tasks { project_id, tasks }`.
- On the tick, every second, each project with a `.tasks.md` whose stamp changed sends
  `Change::Tasks { project_id, tasks }`.
- `Command::TaskAdd` and `Command::TaskSet` change the list through the `Store`.

Tests: a file changed on disk is sent once; a change through the command is in the file.

**Page:**
- The left column without tasks threads: `view::on_page` drops `TASKS`.
- The right column: Waiting rows, then the Tasks section, with:
  - the project dropdown, Find, and the segmented status filter with counts;
  - More ▾ for Issue, Parked and Bugs;
  - two-line rows, and folded Parked and Done.
- Live from `Change::Tasks`.

## 2. Starting work

- `Command::TaskStart { project_id, id, agent, place }`, where `place` is `worktree`, `main`, or
  `thread:<id>`:
  - It makes the thread (`NewThread`, with a worktree when `place` is `worktree`) and names it
    `#<id> <title>`.
  - It sends the brief as the first prompt, or as a prompt to the given thread.
  - It sets the task to in-progress, with its thread and branch.

  Test: each place gives the thread, branch, status and prompt it should.
- **Page:**
  - The split button: Fix now for a bug, Work on it for the rest.
  - The ▾ panel, with Agent (from the agents the daemon detects) and Where, remembered in
    `localStorage`.
  - A row in progress shows its thread, which is a link that selects it.

## 3. Merge, Diff, Rebase

- **`Command::TaskMerge { project_id, id }`:**
  - The main branch is what the main checkout has checked out.
  - It needs `merge-base --is-ancestor main branch`, otherwise it refuses with "needs a rebase".
  - It runs `merge --ff-only`.
  - Then the task is done, and `worktree_remove` runs when nothing would be lost.

  Tests, in a scratch repository: a fast-forward merges; a diverged main refuses and changes
  nothing; a dirty main checkout that conflicts refuses with git's sentence.
- **`Command::TaskDiff`:** `git diff main...branch`, capped at 2,000 lines, replies `Reply::Diff
  { text, more }`.
- **`Command::TaskRebase`:** a prompt to the task's thread.
- **Page:**
  - Merge and Diff on a ready row.
  - A refused merge shows "needs a rebase" and **Ask the agent to rebase**.
  - The diff shows in the middle column with a Close.

## 4. Agents' task tools

- `desl mcp` adds `list_tasks {status?}`, `add_task {title, type?, priority?, status?, body?}` and
  `update_task {id, status?, title?, priority?, note?}`, on its thread's project, through the
  daemon's task commands.
- The descriptions are short: record work and ideas here; "not now" means add as parked.
- Claude's `--allowedTools` and Copilot's allow list cover the new tools. Codex already approves
  every tool of desl's server.

Tests: the tool list, an add and an update through `serve`, and the launch flags.

**Docs:**
- web-spec, SPEC's vocabulary for the statuses, `docs/protocol.md`, the skill (tasks are tools
  now) and the changelog.
- A live run: Claude adds a parked idea when told "park it".
