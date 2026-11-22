# Tasks as desl's first-class record: design

2026-11-22. Agreed with the author in conversation, with mockups (`.superpowers/brainstorm/`,
not committed). The author said "looks right, implement".

## Intent

Tasks are the unit of work agents pick up and the place ideas wait. Today they are one more
item among the threads in desl web's left column (the tasks thread), and agents reach them only
through the skill, which they read sometimes. The author's call:
- Tasks get desl web's right column, always visible and live.
- Each task can start an agent on it in one click.
- Agents get tools to read and change the list, so they use it and park ideas there.
- The left column shows agent threads only.

## Goals

- **The right column:**
  - "Waiting" first (forms, screen dialogs), as slim rows.
  - Then "Tasks": a filter bar and the task list as rows, updated as the file changes.
- **Filters:**
  - The project: it starts on the selected thread's project, and "All projects" groups them.
  - A Find box.
  - Status as one segmented control: **Open · To do · In progress · Ready · Done**, each with a count.
  - **More ▾**: Issue, Parked, Bugs only.
- **Rows:**
  - Two lines: the id, a kind tag and the title; then priority, status, and for running work the
    thread, agent and branch.
  - The action sits on the right of the row.
  - Parked and Done fold to one line each at the bottom.
- **Starting work:**
  - The action is a split button: **Fix now** for a bug, **Work on it** for the rest.
  - Its main part starts at once with the defaults: a new thread, in its own worktree, on the
    default agent.
  - **▾** opens a panel that asks two things apart:
    - **Agent:** every agent desl finds.
    - **Where:** a new thread in its own worktree (the default), a new thread in the main
      checkout, or the selected thread.
  - The panel remembers the last pick.
- **The lifecycle:**
  - Starting marks the task **in progress** and links it to its thread (and branch).
  - The agent's first prompt carries the task and a brief:
    1. Work on the branch and commit there.
    2. Run the tests.
    3. Rebase onto main.
    4. Mark the task **ready**, then stop.
  - Stuck: the agent marks it **issue**, with a note.
  - In the main checkout or the selected thread, there is no branch to merge: the agent marks
    the task **done** itself.
- **Ready:**
  - **Merge** fast-forwards main, in the main checkout, to the task's branch. Then the task is
    done, the worktree goes (when nothing in it would be lost), the branch stays, and the thread
    stays.
  - When main moved meanwhile, the row says it needs a rebase and offers **Ask the agent to
    rebase**: one prompt to the task's thread.
  - **Diff** shows the branch against main, read-only, in the middle column.
- **Agents:** every agent desl starts gets the tools `list_tasks`, `add_task` and `update_task` on
  its project's list, beside `show_form`. Their descriptions say: record work and ideas there;
  when the user says "not now" or "park it", add the idea as **parked**.
- **Statuses:** two new ones, **ready** (done on a branch, waiting for a merge) and **parked**
  (kept for later; not open). Two new fields on a task, **thread** and **branch**.

## Non-goals

- Task boards, assignees, due dates, sub-tasks.
- Changing the TUI's tasks thread (`desl task ui`); it stays as it is, beside agents.
- Answering forms in the terminal (task #4, parked).

## Constraints

- `.tasks.md` stays the record: Markdown a person reads anywhere. The new fields are lines of a
  task's own field list (`- thread: t-7f3a`, `- branch: desl/t-7f3a`).
- The CLI (`desl task`) and the daemon write the file the same way. A change is made under an
  exclusive lock on the project's folder, so two writers never lose one another's change. Today
  a read-modify-write without a lock can.
- Merges are fast-forward only. desl never makes a merge commit and never forces.

## Approaches

| | Approach | Trade-off |
| --- | --- | --- |
| **A, chosen** | The daemon reads and watches each project's `.tasks.md` (the parser moves from the `desl` crate to `desl-proto`), sends the list to clients as events, and is where the page's and the agents' changes are made | One source for the page, the tools and the TUI; live; one writer behind a lock |
| B | The bridge (desl web) reads the file itself and polls | The agents' tools and the page would each write the file their own way; no events for other clients |
| C | The page runs `desl task` through the daemon as a program | Slow, and a text interface where a typed one exists |

The author's calls are the layout, the filters, list rows over cards, agent and place as two
choices, worktrees by default, and Merge as a button. The rest are the builder's, stated above
as proposals and accepted.

## Left to the builder

- The brief's exact wording and the tools' descriptions.
- How the page shows the diff, and what the merge's refusals say.
- Which branch counts as main: the branch the main checkout has checked out.
