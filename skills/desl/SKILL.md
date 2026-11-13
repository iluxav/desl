---
name: desl
description: "Drive desl, the terminal workspace this agent runs in: open threads on Claude Code or Codex, prompt them and read the reply, fork this conversation to the other agent for a second opinion, and wait on threads working in parallel. Use when the user mentions desl, or asks to hand work to another agent or thread, to ask Codex or Claude with this conversation's context, or to run work in parallel threads. Requires DESL_THREAD to be set."
---

# desl

desl (say *diesel*) runs Claude Code and Codex side by side in one terminal workspace. Each conversation is a **thread**; a thread is on one **agent** at a time and belongs to a **project** (a repository). desl owns the conversation, so a thread can be **forked** into a copy on the other agent with everything said so far.

## Check that you are inside desl

```bash
test -n "${DESL_THREAD:-}"
```

If this fails, say that you are not running inside desl and stop. When it passes, `$DESL_THREAD` is the id of your own thread and the `desl` binary on `PATH` talks to the workspace you run in.

## Learn the current CLI

The installed binary is the authority for syntax:

```bash
desl --help
desl <command> --help
```

Never run bare `desl`: it attaches the full-screen interface. Add `--json` to any command to get machine-readable output, and read ids from it instead of guessing them.

## Look around

```bash
desl status --json                    # every project and thread, with status
desl thread show "$DESL_THREAD"       # your own thread: agent, model, transcript
desl thread show <thread>             # what another thread said and did
```

A thread's status is one of `working`, `waiting` (it asks the user something or waits for approval), `idle` (its turn is over), `failed` or `detached`.

## Ask the other agent, with this conversation

A fork copies your thread into a new thread on the other agent. It answers knowing everything said so far:

```bash
desl fork "$DESL_THREAD" codex --json          # prints {"thread_id": "..."}
desl send <new-thread> "Review the change we just made to the token refresh. What did we miss?"
```

`desl send` types the prompt into the thread's agent and prints the reply when its turn ends. Use `claude` instead of `codex` when you are Codex.

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
