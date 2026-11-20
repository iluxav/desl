---
name: desl
description: "Drive desl, the terminal workspace this agent runs in: keep the project's task list (.tasks.md) with `desl task`, open threads on Claude Code, Codex or GitHub Copilot, prompt them and read the reply, fork this conversation to another agent for a second opinion, and wait on threads working in parallel. Use when the user mentions tasks, the task list, a task number such as #12, what to work on next, or desl; when a .tasks.md is in the project; or when the user asks to hand work to another agent or thread, or to run work in parallel. Requires DESL_THREAD to be set, except for `desl task`."
---

# desl

desl (say *diesel*) runs Claude Code, Codex and GitHub Copilot side by side in one terminal workspace. Each conversation is a **thread**; a thread is on one **agent** at a time (`claude`, `codex` or `copilot`) and belongs to a **project** (a repository). desl owns the conversation, so a thread can be **forked** into a copy on another agent with everything said so far. `desl agents` lists the agents installed on this machine.

## Check that you are inside desl

```bash
test -n "${DESL_THREAD:-}"
```

If this fails, say that you are not running inside desl and stop, except for `desl task`, which works anywhere desl is installed. When it passes, `$DESL_THREAD` is the id of your own thread and the `desl` binary on `PATH` talks to the workspace you run in.

## Learn the current CLI

The installed binary is the authority for syntax:

```bash
desl --help
desl <command> --help
```

Never run bare `desl`: it attaches the full-screen interface. Add `--json` to any command to get machine-readable output, and read ids from it instead of guessing them.

## The project's task list

A project can keep a task list: `.tasks.md` at its root, which the user watches and edits in a tasks thread of desl while you work. Each task has an id (`#12`), a title, a type (`feature`, `bug`, `task`, `refactor`, `test`, `docs`, `idea`), a status (`todo`, `in-progress`, `issue`, `done`, `ignored`), a priority (`high`, `medium`, `low`) and a description in Markdown. `desl task` works from any directory of the project, in a worktree too, and inside desl or not.

```bash
desl task list                         # the open tasks: todo, in-progress, issue
desl task list --status all --json     # every task, with its description
desl task show 12                      # one task and its description
desl task add "Fix the flaky login test" --type bug --priority high --body "Fails one run in ten on CI."
desl task set 12 --status in-progress
desl task set 12 --status done --note "Fixed in a1b2c3d: the test waited on a timer."
desl task set 12 --status issue --note "Blocked: the staging database is down."
```

`--body -` and `--note -` read the text from standard input, for anything long. The task list on the user's screen shows each change as you make it.

Work with it strictly:

- **Change it only with `desl task`.** Never edit `.tasks.md` by hand, and never start another list (`TODO.md`, a list in a comment, a file of your own). The commands keep its shape, and change only what they name, so nothing the user or another agent did meanwhile is lost.
- **When the user names a task** ("do #12", "the next task"), read it with `desl task show` first. With no task named, the next one is the open task of the highest priority, in the list's order.
- **Say where you are.** Set a task `in-progress` when you start on it, and `done` only when the work is finished and checked, with a `--note` saying what was done. When you cannot go on, set it `issue` with a `--note` saying why, and tell the user.
- **Write down what is found on the way.** A bug or a piece of work you notice that is outside the task at hand becomes a task (`desl task add`) rather than a change you make unasked.
- **The priority, the order and `ignored` are the user's.** Change them, or remove a task (`desl task rm`), only when the user says so.
- Your own plan for a turn (a todo tool of your own) is separate: the task list holds the project's work, which outlives your session.

## Look around

```bash
desl status --json                    # every project and thread, with status
desl thread show "$DESL_THREAD"       # your own thread: agent, model, transcript
desl thread show <thread>             # what another thread said and did
```

A thread's status is one of `working`, `waiting` (it asks the user something or waits for approval), `idle` (its turn is over), `failed` or `detached`.

## Ask another agent, with this conversation

A fork copies your thread into a new thread on another agent. It answers knowing everything said so far:

```bash
desl fork "$DESL_THREAD" codex --json          # prints {"thread_id": "..."}
desl send <new-thread> "Review the change we just made to the token refresh. What did we miss?"
```

`desl send` types the prompt into the thread's agent and prints the reply when its turn ends. Name an agent other than your own: `claude`, `codex` or `copilot`, whichever `desl agents` shows as installed.

## Run work in parallel

Open a thread with a first prompt; give it its own git worktree when it will edit files at the same time as you:

```bash
desl new --agent codex --worktree -m "Write property tests for src/parser.rs" --json
desl wait <thread>                     # blocks until it is waiting, idle or failed
desl thread show <thread>              # read what it did
```

Run independent threads first, then wait on each. `desl send` to a thread that is already working queues the prompt; it goes out when the current turn ends.

## Stop a thread

```bash
desl interrupt <thread>                # stops its current turn, as esc does in its own screen
desl exit <thread>                     # ends its agent; the thread and its conversation stay
desl restart <thread>                  # ends its agent and starts it again on the same conversation
```

After `desl interrupt`, `desl wait <thread>` returns once the turn has ended. After `desl exit`, the next `desl send` to the thread starts its agent again on the same conversation. Use `exit` when a thread's work is done or it has gone off track, so it stops using the user's limits. Use `restart` when a thread's agent should pick up something installed since it started, such as a new MCP server or a new version of itself; it comes back on the same conversation.

When `desl send` reports that the agent is asking something before it goes on, such as whether to trust a new folder, desl has not typed your prompt; tell the user, who answers in desl's screen.

## Rules

- **Never target your own thread** with `send`, `wait`, `interrupt`, `exit`, `restart`, `switch` or `thread rm`; desl refuses, because each would hang you or end you mid-turn. Fork it instead.
- **Approvals belong to the user.** When a thread you opened is `waiting`, tell the user which thread waits and what it asks. Run `desl approve` or `desl deny` only when the user tells you to.
- **Do not widen permissions.** Leave `desl permissions` and `--worktree` choices to what the user asked for; never set `yolo` on your own.
- **Stop and clean up only what you opened**, or what the user names. `desl exit` is safe to use when a thread you opened is done; `desl thread rm <thread>` removes a thread and its history, so ask the user first.
- Open threads only when the user's request calls for another agent or parallel work, not by default.
