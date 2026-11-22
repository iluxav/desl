# Highlights: a thread's story in short questions and answers

2026-11-22. Agreed with the author in conversation; they picked approach A.

## Intent

The conversation on desl web already leaves out what is cryptic: tool output, the agent's
machinery. The author wants something smaller still, as informative: a feed where each entry is a
question and its answer, two sentences at most, in plain words, so a person can follow what an
agent does as it runs, and catch up on any other agent afterwards. The questions are what started
the work (the user's question or instruction, a task, a form answer) and the questions the agent
asks itself on the way.

## Goals

- **Highlights**, a second view of each thread on desl web: **Conversation | Highlights**, live.
- **An entry:** who asked (you, a task, a form, the agent), when, a question and an answer, each
  at most two sentences. Each entry links to its moment in the conversation.
- **Catching up:** the left column shows each thread's latest entry under its name.
- **Kept:** entries live in the thread's log, so they survive restarts and leaving.

## Non-goals

- The TUI (later).
- Summarising with a model. SPEC.md's "no ambient AI" holds: desl runs no model of its own.
- The agents' thinking. Claude Code stores it empty (1,494 of 1,500 thinking blocks in one of the
  author's sessions); desl's Codex adapter drops it.

## Approaches

| | Approach | Trade-off |
| --- | --- | --- |
| **A, chosen** | Two writers, no model. desl derives one entry per turn from the log: the question is what started the turn, the answer the first two sentences of the reply. The agent writes its own questions with a desl tool, `highlight`. | Free, faithful. The agent's entries depend on it calling the tool; old threads have only desl's entries. |
| B | A small model summarises each step | Uniform, works on old threads; costs model calls (the user's limits), can invent, and breaks "no ambient AI" |
| C | desl's entries only | Cheapest; loses the agent's own questions, the heart of the idea |

## How (the builder's calls, stated as proposals)

- **The agent's tool** is served by `desl mcp` beside `show_form`: `highlight(question, answer)`.
  The daemon appends a `highlight` event to the thread's log, as `show_form` appends `form`: the
  same for every agent, and on the page at once. Each part at most two sentences and 300
  characters; longer is refused with a sentence saying so, so the agent learns the length.
- **desl's entries** come from the log: a prompt (its first two sentences), a task's brief ("Task
  #n: title"), a form's answer ("You answered “title”"). The answer is the first two sentences of
  the turn's last reply, Markdown taken out; while the turn runs, the agent's latest words so far.
- **One derivation**, in `desl-proto` (`highlights`), used by the bridge for the page and by the
  daemon for the left column's latest entry.
- **The conversation view** hides the `highlight` tool call, as it hides `show_form`'s.

## Milestones

1. desl's entries and the Highlights view on the page, live.
2. The agent's `highlight` tool: the MCP tool, the command, the event, its description.
3. The left column's latest entry under each thread.
