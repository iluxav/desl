# desl web — Specification

Nov 21, 2026 · adopted Nov 21, 2026 · redirected to forms Nov 21, 2026

`desl web` opens the workspace in a browser, beside the terminal. It is where an agent puts a
**form** in front of the user: a few widgets described in JSON (choices, a checklist, a document
to edit, a diff to review, a look to tune), drawn inside the thread's conversation. The user works
in it and presses Send, and the agent receives the result as data in its next prompt. The page
also shows every project and agent thread, and each thread's whole conversation.

It is a second client of the daemon, beside the TUI, and replaces nothing. The agent's own screen
stays in the terminal.

**Adopted 2026-11-21.** `SPEC.md` carries the changes listed below. Where this file and `SPEC.md`
disagree, `SPEC.md` wins.

**Redirected 2026-11-21**, the author's call: the browser should not repeat the terminal; it
should give the user and the agent richer ways to talk than a terminal prompt. The design and its
reasons are in `docs/design/2026-11-21-web-forms.md` (approved), the build plan for its first two
milestones beside it. An earlier version of this file planned the agent's screen on the page,
questions with `desl ask`, approval cards, a live document and the task list on the page; the
first three were built and removed, the last two moved to Later.

## Changes made to SPEC.md

| Section | Before | Now |
| --- | --- | --- |
| Non-goals, "No dashboard" | "No web UI, no metrics pages, no boards." | "`desl web` is the one browser client: the projects, their agent threads and each thread's conversation, where an agent puts forms in front of the user; the agent's screen stays in the terminal. It adds no feature the CLI lacks." |
| Language and stack | "`tokio` … comes back only if remote clients in phase 3 need it." | `desl web` uses `tokio` and `axum`, in its own crate. The daemon and the TUI stay on plain threads and channels. |
| Naming and vocabulary | No words for the new pieces | **form**: what a thread's agent puts in front of the user on the page, made of widgets, answered with Send. **waiting list**: every open form and every agent dialog in the workspace, in one place. |
| IR events | — | `form` and `form_answer` |
| Agent adapters | `launch(opts)` | `launch(opts)` takes `mcp`, desl's own MCP server, which each adapter adds with its agent's flags |
| CLI | — | `desl form list|show|answer|close|new`; `prefix v` opens the page on a thread's open form |

Everything else in `SPEC.md` holds for the page as written: the vocabulary, the status words and
their colour roles, the eight principles, "no ambient AI", "no configuration required", and the
task list kept flat.

## Why

desl's only control was text: a prompt typed into an agent, or a key. Typing a reply is slow, and
it loses information, when the user should choose between options with trade-offs, edit a passage
of a document, tune a look, approve a plan in parts, or review a change. The five uses the author
agreed to, each a widget an agent can push:

1. **Choosing:** options with their trade-offs, one click and a note.
2. **Editing a document:** a section of a Markdown file of the project, edited in place and sent
   back, so the agent gets the exact edit.
3. **Tuning a look:** a live preview with sliders; the value goes back.
4. **Approving a plan in parts:** steps as a checklist to untick, reorder and annotate.
5. **Reviewing a change:** each part of a diff accepted, rejected or commented on.

Quick choices stay where they are: Claude Code's `AskUserQuestion` and Copilot's `ask_user` keep
asking in the agent's own screen, answered with the arrow keys in the same turn (the author's
call). Forms are for what the terminal does badly.

## Scope

**Version 1**

1. **The thread list:** projects and their agent threads, one line each, with the status the
   TUI shows (see "Indicators").
2. **The conversation:** the selected thread's every prompt, reply and tool call since it began,
   from desl's log, as `prefix [` shows it in the TUI, with search; a prompt box under it, and
   Stop in its header while the agent works.
3. **Forms:** desl's MCP tool for agents, `desl form` for scripts, cards in the conversation,
   and the waiting list (see "Forms").
4. **Widgets, by milestone:** `markdown`, `choice`, `checklist`, `text`, `number`, `toggle`,
   `select` (milestone 2); `document` (3); `diff` (4); `preview`, `table`, `images` (5).

Shell and file-manager threads are left out of the page. The browser adds nothing to a
terminal, and the TUI keeps them.

**Later**

- The live document: a project's Markdown file with the agents' changes marked as they happen,
  and comments on its passages. The `document` widget covers editing a document with an agent.
- The task list on the page. It stays in the TUI's tasks thread.
- Approvals answered on the page: each adapter first learns its agent's dialog keys, with
  fixtures.
- An activity view per thread from its events: tool calls, file changes, usage.
- Remote access, with the pairing rules of phase 3.

**Never:** accounts, sharing, hosted pages, analytics, columns or boards, the agent's screen, or
a model call of desl's own.

## Layout

```
┌ projects ─────────┐┌ web milestone · claude · working ─────┐┌ Waiting · 2 ───────┐
│ desl              ││ you: what next?                       ││ ▣ Next step?  2m   │
│   web milestone ◀ ││ claude: three ways to go…             ││   web milestone    │
│   spec rewrite    ││ ┌ ▣ Which milestone next? ──────────┐ ││ ▣ Which logo? 9m   │
│ rebrand           ││ │ ( ) M4 questions                  │ ││   logo · rebrand   │
│   logo            ││ │ ( ) M5 live document              │ ││ ply · asks in its  │
│                   ││ │ Note: ___________  [Dismiss][Send]│ ││   own screen       │
│                   ││ └───────────────────────────────────┘ ││                    │
│                   │├───────────────────────────────────────┤│                    │
│                   ││ › prompt to web milestone…    [Send]  ││                    │
└───────────────────┘└───────────────────────────────────────┘└────────────────────┘
```

| Area | What it holds |
| --- | --- |
| Left | Projects and their agent threads, one line each: the status mark, the title, the age, `⎇` for a worktree |
| Middle, header | The selected thread's indicators, as the TUI's panel title shows them, plus its permission mode; Stop while it works |
| Middle | The thread's whole conversation, with a search and a box that hides tool calls; each form a card at the point it was pushed |
| Right | Waiting: every open form, oldest first, then each thread whose agent asks in its own screen, one line saying to answer it in the terminal |
| Bottom | The prompt to the selected thread; a sentence from desl, for a few seconds |

The look follows `SPEC.md`'s "Visual language" and borrows the layout from T3 Code, as the TUI
does. The page has a light and a dark version, following the browser's setting. At phone width
the columns stack and the conversation scrolls in a box of its own.

**Resizing.** The left and right columns take the width the user drags the line beside them to
(the author's call on 2026-11-21). A column is never narrower than its least width (180 px on the
left, 220 on the right) nor wider than 45% of the window, and the middle keeps at least 360 px. A
double click on the line gives the usual width back (280 and 300 px). From the keyboard the line
takes the focus with `tab`, and the arrows move it by 16 px, 64 with `shift`. The widths are kept
in the browser, for that browser only.

## Indicators

| Indicator | In the TUI | On the page |
| --- | --- | --- |
| Waiting count | Sidebar header and terminal title (`1 waiting`) | Page header and tab title |
| Project row | Mark in the project's colour, folder name, branch, uncommitted count, directory | The same |
| Thread row | Agent logo as status (pulses while working; warning colour while waiting; critical when failed; faded while idle), title, age, `⎇` for a worktree | The same, with the same timing and colours; no subagents |
| Thread header | Agent mark, title, project, model, effort, status word, thread id | The same, plus the permission mode and `⚠ yolo` |
| A form | `▣` line in the history (`prefix [`): open, answered, withdrawn or dismissed | The card |

- **Source:** every indicator comes from the daemon's events, so it changes on the page when it
  changes in the TUI. `desl_proto::view` words every row for both clients; the parity check is
  `the_page_and_the_terminal_say_the_same` in `crates/desl/src/layout.rs`.
- **Model:** the header shows the model the agent reported for its session, before the one
  picked with `desl model`.

## Forms

The whole design, with the approaches weighed, is `docs/design/2026-11-21-web-forms.md`. In short:

**The tool.** desl serves an MCP server over stdio, `desl mcp --thread <id> --socket <path>`, with
two tools: `show_form {title, widgets[], send_label?}` and `close_form {form_id}`. desl adds it to
each agent when it starts it, with the agent's own flags, its tools allowed, so the user's agent
configuration is never edited (the flags are in `docs/adapters.md`). `show_form` returns at once:
`Shown to the user as f-4b1e. End your turn now; their answer arrives as your next prompt.` A form
it refuses comes back as a tool error naming the widget and the field, so the agent mends it in
the same turn. The tool's description teaches the language, within 1,500 tokens.

**The language.** A form is a one-line title, 1 to 30 widgets, and an optional label for its Send
button. Each widget has a `kind` and an `id` unique in the form, and may be `optional`; the answer
is an object keyed by widget id. `desl_proto::form` checks forms and answers for the daemon, the
tool and the tests.

| Kind | The agent gives | The answer |
| --- | --- | --- |
| `markdown` | `text` | none |
| `choice` | `label?`, `options[] {label, detail?}` (2 to 12), `multi?`, `other?` | the label, or a list; `other` allows typed text |
| `checklist` | `label?`, `items[] {id, label, detail?, checked?}` (1 to 50), `reorder?` | `[{id, checked, note?}]` in the user's order |
| `text` | `label`, `lines?`, `placeholder?`, `value?` | the text |
| `number` | `label`, `min`, `max`, `step?`, `unit?`, `value?` | the number |
| `toggle` | `label`, `value?` | true or false |
| `select` | `label`, `options[] {label, detail?}` (1 to 50), `value?` | the label |

`document`, `diff`, `preview`, `table` and `images` follow in milestones 3 to 5, as the design
describes them: the agent names files and desl reads them, so a form stays small and the user
sees the file as it is.

**Lifecycle.**
1. `show_form` (or `desl form new`): the daemon checks the form, keeps its JSON as a blob, gives it
   an id, appends a `form` event, and opens it on the thread.
2. A thread with an open form and an idle agent is `waiting`, with the same marks as an agent's
   own dialog: amber, the bell, the title count. While the agent works it stays `working`. A wait
   on a form is not a wait on the screen: the agent takes a prompt meanwhile, and the user may
   talk to it.
3. **Send:** the answer is checked against the form, kept as a blob, a `form_answer` event
   appended, and the agent gets it through the `prompt` path (queued while it works, held while
   its screen asks something):

   ````
   desl: answer to f-4b1e "Which milestone next?"
   ```json
   {"next": "M5 live document", "note": "start with comments"}
   ```
   ````

4. **Dismiss** closes it unanswered; the agent is told in a line before its next prompt,
   `desl: the user dismissed f-4b1e "Which milestone next?" without answering.`, and no turn
   starts. `close_form` withdraws one.
5. **Drafts:** what the user fills in goes to the daemon a second after the last change and is
   kept beside the open form, so a reload, an install, a daemon restart or another tab finds the
   card as it was.

A form closes once. Open forms live in the store, so a restart of anything keeps them. After a
switch the answer goes to whichever agent the thread runs then; a fork shows its parent's forms
read-only and has none open. A prompt the user types closes no form.

**On the page.** A form is a card at its place in the conversation, drawn by the page's own code
(`page/forms.js`): agent text goes in as text nodes or through the page's Markdown renderer. The
agent's `show_form` call and the answer prompt fold into the card, so nothing shows twice. Send
stays off until every required widget is answered. An answered card stays, read-only, with the
answer. The right column lists every open form; a click, or `prefix v` in the TUI on a thread with
one open, goes to the card and focuses its first field.

**In the terminal.** The thread shows `waiting`. The history (`prefix [`) shows a `▣` line for
each form with how it ended. `desl send` that ends on a form names it and where to answer it.
`desl form list|show|answer|close` does from a shell what the page does.

## Architecture

```
agent ──stdio MCP──▶ desl mcp ─┐
                               ├──Unix socket, protocol v1──▶ daemon (threads) ──▶ store (forms, blobs)
browser ──WebSocket──▶ desl web (tokio, axum) ─┘
```

- **Clients, not part of the daemon.** `desl web` and `desl mcp` connect to the daemon's socket
  the way the TUI does, speak the documented protocol, and hold no state that matters. Killing
  either loses nothing, and a fault in them cannot end the daemon that holds the agents.
- **A thin bridge.** Every WebSocket text message is one protocol frame's JSON, both ways, so the
  page speaks the protocol in `docs/protocol.md` directly: `prompt`, `interrupt`, `form_get`,
  `form_answer`, `form_close`, `form_draft`. The bridge adds the token check, the origin check,
  one daemon connection per browser tab, the view and the conversation.
- **One binary.** The page's files are compiled into the binary and served from memory. No
  frontend build step and no vendored code: plain JavaScript modules, with the page's own
  Markdown renderer, which makes elements and text nodes, never HTML. `cargo build` stays the only
  build.
- **tokio stays in its own crate,** `crates/desl-web`, and its runtime starts only when `desl web`
  runs. `desl mcp` is plain blocking code in the `desl` binary.
- **Upgrades.** When the daemon restarts, `desl web` and `desl mcp` connect to the new one and
  carry on. When the binary changes, `desl web` starts again from the new binary, keeping its
  port and its token: it looks at its binary every 2 seconds, and once the file is another one
  and has stayed the same for one more look, it runs the new binary in the same process
  (`exec`), handing it the key in `DESL_WEB_KEY`. On connecting, the bridge says which build of
  the page it serves (a hash of the page's files), and a page loaded from another build loads
  itself again.

**Cost:** the stripped release binary grew from 8.2 MB to 10.1 MB with xterm.js in milestone 1;
without it, 9.8 MB.

**Opening the page.** One `desl web` serves each workspace. While it runs it notes its process
and address in `<socket>.web`, beside the daemon's socket and readable by the user alone. `prefix
v` in the TUI opens the browser on it, on the thread's open form when it has one, and starts it
in the background when none runs, in a session of its own. Over SSH, or with no `xdg-open`
(`open` on macOS), the address goes to the clipboard instead. From a shell, `desl web --open`
does the same; `desl web` while one runs says where it is; `desl web --stop` ends it.

**The bridge's own messages.** Besides the daemon's frames, the bridge sends the page:
- `{"type": "view", "view": {waiting, projects, forms}}`: the sidebar as `desl_proto::view::Sidebar`
  words it, and every open form (`form_id`, `thread_id`, `title`, `project`, `age`), oldest
  first. Sent when the workspace changes, and every 15 seconds when an age moved on; never twice
  the same.
- `{"type": "bridge", "state": "ready", "build"}` on connecting; `"no_daemon"` or
  `"daemon_gone"` when the daemon cannot be reached; the page connects again, with growing
  waits up to 5 seconds.
- A refused request is answered as the daemon would, and never reaches the daemon: `shutdown`,
  `handover` and `hello`.
- `{"type": "conversation", "id", "thread_id", "items"}`, the answer to the page's own
  `{"type": "conversation", "id", "thread_id"}`: the thread's conversation as
  `desl_proto::conversation` projects it for the TUI's history, each item with its `kind`
  (`user`, `assistant`, `tool`, `rule`, `brief`, `note`, `error`, `form` with its id, title and
  how it closed). The page asks again when an event is added to that thread: at once for a form
  shown or closed, within 600 ms for the rest.

The bridge says `hello` as client `web` and subscribes without a directory; request ids from 2^52
up are its own. The page sends the key as `{"token": "…"}`; a wrong or late key closes the socket
with code 4401.

## Budgets

| Metric | Budget | Measured (2026-11-22, localhost) |
| --- | --- | --- |
| Page load to a usable thread list, 20 threads | < 500 ms | 18 ms with four threads |
| `show_form` to the card on an open page | < 300 ms | 5 to 9 ms |
| Send to the prompt typed into an idle agent | < 1 s | 3 ms to `working` |
| The tool's description and schemas, counted in tokens | ≤ 1,500 | under 6,000 bytes, by test |
| A form, as JSON | ≤ 256 KB | refused above, by test |
| `desl web`'s memory with three tabs open | < 50 MB | |

## Security

The page has the same powers as the TUI: it can type into agents. Whoever holds its URL controls
the agents. Every rule below follows from that.

1. **Localhost only.** `desl web` binds to 127.0.0.1. `--host` waits for phase 3.
2. **A token per start.** 128 random bits in the part of the address after `#`, which the browser
   never sends; the page sends it as its first WebSocket message, within 5 seconds.
3. **Origin and Host checks** on every request and on the WebSocket, which is what stops another
   site from driving the agents, including through DNS rebinding.
4. **Agent text is never HTML.** Forms, replies and titles reach the page only as text nodes or
   through the page's Markdown renderer, which makes elements and never HTML. The
   Content-Security-Policy is `default-src 'self'` with nothing inline, styles included. The
   `preview` widget (milestone 5) shows agent HTML and CSS in a sandboxed frame with scripts off;
   that is the one place agent markup is drawn.
5. **The MCP server acts on its own thread only:** `desl mcp` is started with the thread's id and
   can push and withdraw that thread's forms, nothing else.
6. **Files stay in the project.** A `document`, `diff` or `images` widget (milestones 3 to 5)
   reads only files inside the thread's directory once links are resolved, of the kinds and sizes
   the design lists; a document save writes only the section the card opened, and only if it is
   unchanged on disk.
7. **Secrets.** The environment denylist and the key scan in `SPEC.md` apply unchanged. The
   bridge logs no frame contents.

## Milestones

| # | Milestone | Done when |
| --- | --- | --- |
| 1 | The page reshaped | Done 2026-11-21: the screen, xterm.js and the page's size rule removed; the left column as projects and threads; the conversation as the middle; the right column ready for forms |
| 2 | Forms | Done 2026-11-22: `desl mcp`, the launch flags for Claude Code 2.1.330, Codex 0.171.0 and Copilot 1.0.114, the events, the protocol, `desl form`, and the seven widgets of milestone 2 in the conversation and the right column. Each of the three agents called `show_form` without a dialog of its own and ended its turn; answers sent from the page and the shell reached Claude, Codex and Copilot as their next prompt; open forms and drafts survived daemon restarts. Two defects found on the way are fixed with tests: a migrating upgrade lost the new daemon's writes (`VACUUM INTO` for the backup), and a thread waiting on a form queued every prompt |
| 3 | Document | A section of a Markdown file edited in place with comments; the conflict on save |
| 4 | Diff review | Each hunk accepted, rejected or commented on |
| 5 | Preview, table, images | The sandboxed preview with controls; a table to edit; pictures to choose from |
| 6 | Security review | Someone who didn't write it tries to run script from a form, read a file outside the project through a document, image or diff, and drive an agent from another site, and fails each time |

Each milestone follows the definition of done in `SPEC.md`: spec match, tests, help text, error
paths tried by hand, and budgets.

## The skill

`skills/desl/SKILL.md` says, in one line, that forms come from desl's MCP tool `show_form`, whose
description teaches the rest: an agent needs nothing else to use it.

## Open questions

1. **When.** Phase 3, beside remote TCP mode, or right after the phase 1 gate?
2. **Answering in the TUI.** The TUI shows that a thread waits on a form and sends the user to
   the page. Should simple forms (one `choice`) be answerable in the TUI too, without a modal?
3. **Codex's own MCP approval dialog.** The Codex adapter does not recognise Codex's "Allow the …
   MCP server to run tool …?" dialog, so such a thread shows `working`. desl's own tools no longer
   ask; other servers' calls still do.
4. **Prompt wording.** The answer and dismissal lines are tested with each agent; are they the
   words that serve agents best over time?

## Decisions so far

- The page is a client of the daemon over the existing protocol, not code inside the daemon.
- `tokio` and `axum` are used in `desl web` only. The daemon, the TUI and `desl mcp` stay on
  threads.
- The page's files are compiled into the single binary, with no frontend build step.
- The page does not show the agent's screen (the author's call, 2026-11-21).
- Forms: desl's own widget set, as JSON, drawn by the page's own code; not A2UI, not agent HTML
  (the author's call, 2026-11-21). The tool returns at once and the answer arrives as the next
  prompt (the author's call). Forms show inside the conversation (the author's call). The agents'
  own question tools stay on (the author's call).
- desl's MCP server is added at each agent's launch, never written into the user's configuration.
- Answers and dismissals use the existing `prompt` path.
- Every indicator the TUI shows is shown on the page, with the same words and colour roles.
- Phone: the page lays out at phone width, but it listens on 127.0.0.1, so a phone reaches it only
  with phase 3's remote access.
- Shell and file-manager threads stay in the TUI only.
