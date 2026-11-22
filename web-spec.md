# desl web — Specification

Nov 21, 2026 · adopted Nov 21, 2026

`desl web` opens the workspace in a browser. It shows the same threads with every indicator the TUI shows, plus three things the terminal shows poorly:

1. A waiting list for every thread.
2. Questions with options to click.
3. A live document: a Markdown file of the project, where the agents' changes appear as they make them, and the user edits it and comments to steer them.

It is a second client of the daemon, beside the TUI, and replaces nothing.

**Adopted 2026-11-21.** `SPEC.md` carries the changes listed below. Where this file and `SPEC.md` disagree, `SPEC.md` wins.

**Revised 2026-11-21**, after a review against the code and a mockup of the page: the user edits documents from version 1 (the author's call); an approval card opens the agent's own screen, because desl sees an agent's dialog but not what is behind it; a thread shown in the TUI keeps the TUI's size; the page gains a Conversation tab and a list of documents; and "Indicators" says what each client shows today.

## Changes made to SPEC.md

| Section | Before | Now |
| --- | --- | --- |
| Non-goals, "No dashboard" | "No web UI, no metrics pages, no boards. The remote client in phase 3 is the same TUI over TCP." | "No metrics pages and no boards. `desl web` is the one browser client: the workspace as the TUI shows it, plus the waiting list, questions, and comments on and edits to a project's documents. It adds no feature the CLI lacks." |
| Non-goals, "No IDE features" | "No file browser, editor or language server of desl's own." | Add: "`desl web` shows a project's Markdown files with what the agents changed in them, takes comments on them, and edits them a paragraph at a time. It opens no other kind of file." |
| Language and stack | "`tokio` … comes back only if remote clients in phase 3 need it." | `desl web` uses `tokio` and `axum`, in its own crate. The daemon and the TUI stay on plain threads and channels. |
| Naming and vocabulary | No words for the new pieces | Add **question**: what a thread's agent asks the user with `desl ask`, with options to choose from (borrowed from Claude Code's `AskUserQuestion`). Add **comment**: a note the user attaches to a passage of a document and sends to a thread (borrowed from GitHub reviews and Google Docs). Add **waiting list**, already used in the walkthrough: every pending approval and question in the workspace, in one place. |
| IR events | No question or comment events | Add `question`, `question_answer`, `comment` and `comment_resolved` (see "Questions" and "The live document") |
| Roadmap | No browser client | Not placed yet: "desl web timing" in `SPEC.md`'s open questions, and open question 1 below |

Everything else in `SPEC.md` holds for the page as written: the vocabulary, the status words and their colour roles, the eight principles, "no ambient AI", "no configuration required", and the task list kept flat.

## Why

desl's only control today is text: a prompt typed into an agent, or a key. With several threads running, often in `yolo` mode, three costs repeat:

1. **Finding who is waiting.** The bell and the title count say that a thread is waiting, but answering means moving to that thread and reading its screen.
2. **Typing decisions.** When an agent asks the user to choose ("Cloudflare or Vercel?"), the choice is typed back as prose. Several open questions end up spread across turns and threads.
3. **Steering a document through prose.** The usual loop is: the user writes ideas into a Markdown file, an agent reworks it, and the user reads the change in the terminal and types what to change next ("no stock photos, update the doc"). The user's note isn't attached to the passage it's about, and the agent's change isn't shown in the document.

The page serves feel-bar moments 5 (waiting is impossible to miss), 6 (the agent is the hero: the page shows its own interface) and 7 (coming back is instant, from a browser too). It must not cost moment 4: the TUI draws exactly as it does without the page.

## Scope

**Version 1**

1. **The thread list** with every indicator the TUI shows (see "Indicators").
2. **The waiting list:** questions from every thread, answered with one click, and approvals, each opening the agent's screen to be answered in its own dialog.
3. **Questions:** `desl ask` for agents, and option cards on the page.
4. **The live document:** a project's Markdown file with the agents' changes marked as they happen, edited by the user a paragraph at a time, and comments sent to threads.
5. **The task list:** a project's open tasks, their status and priority changed from the page, and a task started in a thread.
6. **Any thread's screen:** the agent's own interface, drawn by xterm.js, with keys, paste and resize.
7. **Any thread's conversation:** every prompt, reply and tool call since the thread began, from desl's log, as `prefix [` shows it in the TUI, with search.

Below the three columns are a prompt to the selected thread (queued while its agent is working) and a stop button.

Shell and file-manager threads are left out of the page. The browser adds nothing to a terminal, and the TUI keeps them.

**Later**

- A Changes tab: each thread's file changes as diffs, with comments on a line that reuse the comment events.
- Approvals answered on the page: Allow, Deny and Always, with the diff of an edit or the command and its risk shown first. Each adapter first learns its agent's dialog keys, with fixtures.
- A prompt with context: a section of a document, a task or a screenshot dragged into the prompt.
- An activity view per thread from its events: tool calls, file changes, usage.
- Two forks of one point shown side by side.
- Remote access, with the pairing rules of phase 3.

**Never:** accounts, sharing, hosted pages, analytics, columns or boards, or a model call of desl's own.

## Layout

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│ desl                                                                         2 waiting │
├──────────────────────┬────────────────────────────────────────────┬────────────────────┤
│ PROJECTS             │ ✻ Proxy design · rebrand · opus · high     │ WAITING            │
│ ● rebrand ⎇ main ±4  │   edits · working · t-7f3a                 │                    │
│   ✻ Proxy design  2m │ [SPEC.md]  Screen                          │ Proxy design       │
│   › Template v0   ⎇  │                                            │ Which host?        │
│     └─ ● Explore     │ ## 6.3 Proxy                               │ (•) Cloudflare D1  │
│   Tasks · 3 open     │ One MCP server at one address.             │ ( ) Vercel + DB    │
│     ◐ #4 Template v0 │ ┃ Host: Cloudflare Workers with D1         │ [Answer]           │
│     ○ #5 Audit agent │ ┃ claude · 1 min ago     [keep] [undo]     │                    │
│ ● dnom ⎇ main        │                                            │ Template v0        │
│ ● desl ⎇ main ±2     │ [comment] no stock photos                  │ Run: npm test      │
│                      │   you → Proxy design · resolved            │ [Open its screen]  │
├──────────────────────┴────────────────────────────────────────────┴────────────────────┤
│ › Prompt to Proxy design (claude)                                        [Send] [Stop] │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

| Area | What it holds |
| --- | --- |
| Left | Projects, their agent threads and subagents, as the sidebar shows them, and under each project its open tasks and its documents: the ones opened on the page before and any Markdown file an agent changed in the last day, at most five, newest first |
| Middle, header | The selected thread's indicators, as the TUI's panel title shows them, plus its permission mode |
| Middle, tabs | The open document (named after its file), the thread's whole conversation (Conversation) and its own screen (Screen) |
| Right | The waiting list: every pending approval and open question, oldest first |
| Bottom | The prompt to the selected thread, and the stop button |

The look follows `SPEC.md`'s "Visual language" and borrows the layout from T3 Code, as the TUI does. The page has a light and a dark version, following the browser's setting.

**Resizing.** The left and right columns take the width the user drags the line beside them to (the author's call on 2026-11-21), as a code editor's side panels do. A column is never narrower than its least width (180 px on the left, 220 on the right) nor wider than 45% of the window, and the middle keeps at least 360 px. A double click on the line gives the usual width back (280 and 300 px). From the keyboard the line takes the focus with `tab`, and the arrows move it by 16 px, 64 with `shift`. The widths are kept in the browser, for that browser only. At phone width the columns stack and there is nothing to drag.

## Indicators

Every indicator the TUI shows appears on the page, with the same words and the same colour roles. The page adds none of its own beyond the permission mode in the header, the waiting list, the document marks, and each project's open tasks and documents in the left column.

| Indicator | In the TUI | On the page |
| --- | --- | --- |
| Waiting count | Sidebar header and terminal title (`1 waiting`) | Page header and tab title (`desl · claude · t-7f3a · 1 waiting`) |
| Project row | Mark in the project's colour, folder name, branch, uncommitted count, directory | The same |
| Thread row | Agent logo as status (pulses while working, one breath every 1.6 s; warning colour while waiting; critical colour when failed; faded while idle), title, age, `⎇` for a worktree | The same, with the same timing and colours |
| Subagents | The newest four under their thread: `●` running, `✓` done, `○` left unfinished | The same. A click shows the subagent's conversation, read-only |
| Thread header | Agent mark, title, project, model, effort, status word, thread id | The same, plus the permission mode (`ask`, `edits`, `worktree`, `yolo`) |
| `yolo` | Not shown today. Milestone 1 adds `⚠ yolo` to the header while the agent runs with it | `⚠ yolo` in the header and the tab title |
| Tasks | A tasks thread under its project once `.tasks.md` exists | The tasks thread, and under the project its open tasks with their marks, ids, titles and priorities |

- **Source:** every indicator comes from the daemon's events (`workspace`, `thread`, `project`, `appended`), so it changes on the page when it changes in the TUI. The page polls nothing of its own.
- **Model:** the header shows the model the agent reported for its session, not only the one picked with `desl model`, following principle 8, honest status. The TUI shows the picked model first today (`model_choice` before `model` in `crates/desl/src/layout.rs`); milestone 1 changes both clients to the reported one.
- **No drift:** an indicator computed inside the TUI's code today moves to the daemon or to a crate both clients use. A parity check, in which each row of this table appears in both clients, is a release gate. Done in milestone 1: `desl_proto::view` orders the projects and threads and words every row and header; the TUI draws from it, and the bridge sends it to the page as it is. The parity check is `the_page_and_the_terminal_say_the_same` in `crates/desl/src/layout.rs`: each word of the page's view is on the TUI's screen.

## The waiting list

- **Contents:** every thread whose status is `waiting`: each open question, and each dialog an agent's screen shows.
- **Approvals:** the daemon knows that an agent's screen shows a dialog (each adapter reads its agent's screen for one), not the tool, command or risk behind it, and the `answer` command reaches only an agent desl runs headless. So in version 1 an approval card shows the dialog as the agent drew it (the bottom rows of the daemon's screen model) and an **Open its screen** button, and the user answers in the agent's own dialog. Allow, Deny and Always on the card wait until each adapter knows its agent's dialog keys (see Later).
- **Claude Code's own questions:** Claude Code asks with its built-in `AskUserQuestion` tool unless told not to, and its dialog is answered on its screen like an approval. Open question 7 decides how it joins `desl ask`.
- **Notification:** one mark in the tab title, the same count as the terminal title. Never a modal and never a repeat. Desktop notifications wait for the phase 3 notifications work.
- **Answered elsewhere:** something answered in the TUI, in the agent's own screen or from the CLI leaves the list at once.

## Questions

An agent asks the user to decide something with `desl ask`. The command returns at once, the agent ends its turn, and the user's answer arrives as the agent's next prompt. This works the same for Claude Code, Codex and Copilot, needs no MCP server, and has no tool timeout, because nothing waits.

**CLI**

```bash
desl ask "Which host for the proxy?" \
  --option "Cloudflare Workers with D1: $0–5 a month, SQLite" \
  --option "Vercel with a hosted database: same platform as the sites" \
  --text                                  # also allow a free-text answer
# → q-4b1e   (with --json: {"question_id": "q-4b1e", "thread_id": "t-7f3a"})

desl answer q-4b1e 1 --text "Keep it under $5 a month."   # for scripts and for users in the terminal
```

- `desl ask` asks on behalf of the thread in `DESL_THREAD` and refuses without it. Like `desl comment resolve`, it acts on its own thread. It can't hang the caller, because it returns immediately.
- Options are 2 to 4. `--multi` allows several choices. Each option is a label, with an optional description after the first `:`.

**Events.** Both are appended to the asking thread's log, so a question and its answer are in the history, in an export and in a brief, and are never lost (principle 1).

| Kind | Fields |
| --- | --- |
| `question` | `question_id`, `text`, `options[] {label, description?}`, `multi`, `allow_text` |
| `question_answer` | `question_id`, `choices[]`, `text?`, `by` (`user`, `superseded`) |

**Protocol.** Two new commands within major version 1: `ask {thread_id, text, options, multi, allow_text}` replies `asked {question_id}`, and `answer_question {question_id, choices, text?}` replies `done`. A `thread` event carries the thread's open questions, as it carries a pending approval.

**Status.** A thread with an open question is `waiting`, even though its agent is idle. That matches the status word as defined: "the agent asked a question or needs an approval". The rule becomes: `waiting` while the agent's screen shows a dialog or the thread has an open question. Today the screen check alone sets `waiting` and clears it within a second (`check_asking` in `crates/desl-daemon/src/engine.rs`), so an open question is a second input to it, not a status set once.

**Delivery.** The answer goes to the thread through the existing `prompt` path: queued while the agent works, typed when it is idle, and held while the agent's screen shows a question of its own. The prompt names the question and the choice:

```
desl: answer to q-4b1e "Which host for the proxy?"
Chosen: Cloudflare Workers with D1
Note: Keep it under $5 a month.
```

**Superseded.** A prompt the user sends to the thread, read from the agent's session as a `user_message`, closes the thread's open questions with `by: superseded`. The page then stops offering them. A prompt desl typed itself (an answer, a comment, a task started from the page) supersedes nothing: desl knows what it typed, and without this rule answering one question would close another open question in the same thread.

## The live document

A document is any Markdown file of the project. The file stays the record: it lives in the repository, under git, and agents read and change it with their own tools, as they do today. The page shows it, marks what changed and who changed it, lets the user edit it a paragraph at a time, and takes comments.

**Opening one.** The document tab opens the file last opened for that project. With none, it opens `SPEC.md`, then `AGENTS.md`, then `README.md`, whichever exists first. A picker lists the project's Markdown files, from the existing `files` command. The left column lists, under each project, the documents opened on the page before and any Markdown file an agent changed in the last day, at most five, newest first.

**Live changes.** `desl web` watches the open file by checking its modification time twice a second, which needs no new dependency. A change shows on the page within a second.

**Who changed what.**
- **Agents' edits:** each change is matched to the `file_change` event with the same path whose hunks it contains; two edits that land between two checks arrive as one change. desl records such an event for every edit an agent makes with its own edit tool, with the agent and the thread. The changed passage gets a bar in the margin and a line under it: `claude · Proxy design · 1 min ago`.
- **Timing:** the event comes from the agent's session file up to a second after the file changes, so a change is first marked `changed` and gets its author when the event arrives.
- **Shell edits:** an edit made with a shell command (`sed`, a script, `cat >`) records no `file_change`. When exactly one thread of the project was working at that moment, the change reads `while Template v0 was working`; otherwise it reads `outside desl`, as an edit from the user's own editor does.
- **The user's saves:** a block saved on the page reads `you · just now`.

**Keep and undo.**
- **Keep:** clears the mark.
- **Undo:** writes the reverse of that change to the file as it is at that moment, in one rename, the way the task list writes. When the passage has changed again since, undo is refused, with a sentence saying why and pointing to `git diff`. The user's own saves are undone the same way.
- **Where marks are kept:** the daemon keeps the marks not yet kept or undone, per document, so they survive a reload and a daemon restart: `marks {project_id, path}` replies `marks {marks}`, and `save_marks {project_id, path, marks}` replies `done`. They stay out of `save_view`, which holds one value, the TUI's layout, and would be overwritten.

**Editing.** The user edits a document on the page from version 1 (the author's call on 2026-11-21). Agents never type into a file character by character; each edit replaces a passage in one write. So the page works a block at a time, with no live cursors and no merging of keystrokes:

1. **Open a block:** a click on a paragraph opens it as its Markdown source: one block as the renderer parses it, a paragraph, a heading, a list, a table or a code block. The rest of the document stays rendered and keeps changing live.
2. **Type:** nothing is written while the user types. When an agent changes the same block meanwhile, a line above the text says so (`claude changed this paragraph while you were typing`), and the typing is never replaced.
3. **Save** (the button, or `ctrl+enter`): desl reads the file as it is at that moment, finds the block by its old text under its heading, replaces that block alone, and writes the file in one rename, keeping its line endings. What an agent wrote elsewhere a second before stays. `esc` cancels.
4. **When the old text is gone:** when the block's old text is no longer in the file exactly once, nothing is written. The page shows the user's version and the file's side by side, and the user picks one or edits on.
5. **Add and remove:** a `+` between two blocks adds one; a block saved empty is removed.
6. **Telling an agent:** a save tells no agent, so it starts no turn the user did not ask for. A line under the save offers `Tell <thread>`, which sends one prompt through the `prompt` path to the thread that last changed that section, or the selected one:

```
desl: the user changed SPEC.md, section "6.3 Proxy"
Was: "Cache the menu JSON for five minutes."
Now: "Cache the menu JSON at the edge for five minutes, and drop it on publish."
```

What the agents do when the user saves:
- **Nobody is interrupted.** An agent reads the new text the next time it opens the file.
- **They read again before editing.** Claude Code refuses to edit a file that changed since it last read it, and reads it again. A Codex patch fails when its context lines have moved, and Codex reads the file again. Copilot CLI's behaviour is checked in milestone 5.
- **A shell overwrite is caught.** An agent that rewrites the file with a shell command has no such check and can overwrite the user's block. The page sees the user's marked change disappear and says so, with **Restore**, which writes it back by the same rules as a save. The skill tells agents to edit documents with their own edit tool for this reason.

**Comments.**
1. The user selects a passage and writes a comment.
2. The comment goes to a thread: by default the thread that last changed that section, otherwise the selected thread. The user can pick another.
3. It arrives through the `prompt` path, like an answer, as below.
4. The comment stays beside its passage, open, until the agent resolves it or the user does.

```
desl: comment c-12 on SPEC.md, section "6.3 Proxy"
On: "Host: Cloudflare Workers with D1"
Comment: Keep costs under $5 a month.
When it is done: desl comment resolve c-12 --note "<what changed>"
```

**Anchoring.** A comment holds the quoted text and the heading of its section, and the page finds it again by that text. When the quote is gone from the file, the comment moves under its heading, marked `text changed`. When the heading is gone too, it moves to the end of the document.

**Events and commands.**

| Kind | Fields |
| --- | --- |
| `comment` | `comment_id`, `path`, `quote`, `heading?`, `text` |
| `comment_resolved` | `comment_id`, `by` (`user`, `agent`), `note?` |

Both are appended to the log of the thread the comment went to. In the protocol, `comment {thread_id, path, quote, heading?, text}` replies `created {comment_id}`, and `resolve_comment {comment_id, note?}` replies `done`. The CLI has the same pieces, so everything stays scriptable:

```bash
desl comment add SPEC.md --quote "Host: Cloudflare Workers with D1" --text "Keep costs under $5." --thread t-7f3a
desl comment list --path SPEC.md --open --json
desl comment resolve c-12 --note "Added the cost limit to 6.3."
```

## The task list

- **Left column:** each project's open tasks under its threads, with each task's status mark, id, title and priority, using the tasks thread's marks.
- **A click opens a task:** its fields, each a list to change (status, priority, type), and its description rendered as Markdown.
- **"Start in…":** sends `Work on task #4.` to a thread the user picks. The desl skill already teaches agents what to do with a task number.
- **Reading and writing:** `desl web` reads and writes `.tasks.md` with the same code `desl task` uses, so every rule of "Tasks" in `SPEC.md` holds. A task someone else changed lights up for five seconds. That code moves first from the `desl` binary (`crates/desl/src/tasks/list.rs`) to a library crate: `desl` depends on `desl-web`, so `desl-web` cannot reach into `desl`.
- **Still flat:** status tabs as in the tasks thread. No columns, assignees or estimates.

## The agent's screen on the page

- **Attach:** `attach` with the browser terminal's rows and columns, and the answers xterm.js gives to terminal queries. Then come `pty` frames, and `input` and `resize` going the other way. A tab that closes or shows another thread sends `leave`.
- **Keys:** with the screen focused, every key goes to the agent except the prefix, as in the TUI.
- **Known limit:** the browser keeps some keys for itself (`ctrl+w`, `ctrl+t`, `ctrl+n`) and the page can't take them.
- **One thread on two screens:** while a TUI shows a thread, the terminal keeps the TUI's size, and the page draws that screen at that size, with blank space around it or scrolled. The daemon tells the clients apart by the name each gives in `hello`; it ignores the page's `attach` size and `resize` for such a thread, and sends the page the terminal's size in `attached` and in a `pty_size` frame when it changes. Today the client that attached or resized last sets the size and nothing tells the other, so a TUI would draw a screen of another size wrongly, against moment 4 (open question 2).

## Architecture

```
browser ──WebSocket, JSON frames──▶ desl web (tokio, axum) ──Unix socket, protocol v1──▶ daemon (threads)
                                          │
                                          └── reads the open document and .tasks.md
```

- **A client, not part of the daemon.** `desl web` connects to the daemon's socket the way the TUI does, speaks the documented protocol, and holds no state that matters. Killing it loses nothing (moment 9), and a fault in web code cannot end the daemon that holds the agents.
- **A thin bridge.** Every WebSocket text message is one protocol frame's JSON, both ways, so the page speaks the protocol in `docs/protocol.md` directly: `hello`, `subscribe`, `attach`, `input`, `resize`, `leave`, `prompt`, `answer`. The bridge adds the token check, the origin check, one daemon connection per browser tab, and what the protocol doesn't carry: the open document's text and its saves, undos and restores, and the task list, read and written with desl's own code.
- **One binary.** The page's files are compiled into the binary (`include_bytes!`, or `rust-embed` for a folder), pre-compressed, and served from memory. Version 1 has no frontend build step: plain JavaScript modules, plus vendored copies of xterm.js (MIT) and a Markdown renderer with raw HTML turned off. `cargo build` stays the only build.
- **tokio stays in its own crate.** A new crate, `crates/desl-web`, holds the async code, and the runtime starts only when `desl web` runs. One WebSocket carries terminal output out and keys in at the same time, which tokio handles directly.
- **Upgrades.** When the daemon restarts (`desl daemon restart`), its socket closes. `desl web` connects to the new daemon, sends `hello` and `subscribe` again, re-attaches the threads each tab shows, and the tabs carry on. When the binary changes, `desl web` starts again from the new binary, as an open TUI does, keeping its port and its token, so the page reconnects by itself without a new URL.

**Cost, measured in milestone 1:** the stripped release binary grew from 8.2 MB to 10.1 MB, and a clean release build from 42 s to 44 s, on the author's machine.

**The bridge's own messages.** Besides the daemon's frames, the bridge sends the page:
- `{"type": "view", "view": {waiting, projects}}`: the sidebar as `desl_proto::view::Sidebar` words it, each project with its threads, each thread with its status, glyph, word, age, worktree mark, subagents and header (project, branch, count, model, permission mode, `yolo`). It is sent when the workspace changes, and every 15 seconds when an age moved on; never twice the same.
- `{"type": "bridge", "state": "no_daemon" | "daemon_gone"}` when the daemon cannot be reached or closed the connection; the page connects again, with growing waits up to 5 seconds.
- A refused request is answered as the daemon would, `{"type": "reply", "id", "error"}`, and never reaches the daemon: `shutdown`, `handover` and `hello`.

The bridge says `hello` as client `web` and subscribes without a directory, so it registers no project; request ids from 2^52 up are its own, and their replies stay with it. The page sends the key as `{"token": "…"}`; a wrong or late key closes the socket with code 4401, and the page asks for the address desl web printed. A new address pasted into an open tab loads it with the new key.

## Budgets

Proposed, to join `SPEC.md`'s table with milestone 1, measured on the reference machine with the page on localhost. The TUI's budgets do not move: the daemon serves the page's connection on a reader of its own, as it serves a TUI's.

| Metric | Budget |
| --- | --- |
| A key typed on the page to the agent's terminal | < 10 ms p99 |
| The agent's output to the page's screen | < 20 ms p99 |
| Page load to a usable thread list, 20 threads | < 500 ms |
| An agent's edit to its mark in the open document | < 1.5 s |
| A save to the file on disk | < 100 ms |
| `desl web`'s memory with three tabs open | < 50 MB |

## Security

The page has the same powers as the TUI. It can type into agents and approve their commands, so whoever holds its URL controls the agents. Every rule below follows from that.

1. **Localhost only.** `desl web` binds to 127.0.0.1. `--host` waits for phase 3 and its remote-access rules: a token, localhost unless `--host`, and a pairing URL and QR code as `t3 serve` prints them.
2. **A token per start.** `desl web` makes a random token of at least 128 bits and prints `http://127.0.0.1:<port>/#t=<token>`. The token is in the part after `#`, which the browser never sends in a request or writes to a server log. The page sends it as its first WebSocket message, and the bridge closes any connection that hasn't sent a valid token within 5 seconds.
3. **Origin and Host checks.** A WebSocket is accepted only when `Origin` is the page's own address and `Host` is `127.0.0.1:<port>` or `localhost:<port>`. Browsers let any website open a WebSocket to localhost, and these checks are what stop another site from driving the agents, including through DNS rebinding.
4. **Agent text is never HTML.** Agent output reaches the page only through xterm.js, as text nodes, or through the Markdown renderer with raw HTML turned off. A Content-Security-Policy of `default-src 'self'` blocks inline scripts, foreign scripts and images from other servers. An agent, or a repository it reads, can write `<script>` or an image link that carries secrets in its address. If the page ran or loaded either, one could drive every agent and the other could leak secrets. Links in a document open in a new tab, with `noopener` and `noreferrer`. Milestone 2 checks that xterm.js draws under this policy; an exception it needs is written here, and never one for scripts.
5. **Secrets.** The environment denylist and the key scan in `SPEC.md` apply unchanged. The bridge logs no frame contents.
6. **Writes stay in the project's documents.** The bridge writes a file only through a save, an undo or a restore, and only a file that already exists, ends in `.md` or `.markdown`, and lies inside the project's root once links are resolved. It creates and deletes no file, and refuses a path with `..`.

## Milestones

| # | Milestone | Done when |
| --- | --- | --- |
| 0 | SPEC change | Done 2026-11-21: the two non-goals, the stack paragraph, the vocabulary, the IR events and the open questions in `SPEC.md`; rule 3 and the layout in `AGENTS.md` |
| 1 | Page, thread list and indicators | Done 2026-11-21. `desl web` prints a URL. The page shows every project and thread, with every indicator in "Indicators" changing within a second of the TUI. The TUI's header shows the reported model and `⚠ yolo`. The parity check passes. The binary is still one file, and its size and build time are recorded. Measured: 8.2 MB to 10.1 MB and 42 s to 44 s (see Architecture); the first view drew 18 ms after the page began to load, with four threads on localhost. The prompt box and Stop came with it |
| 2 | Agent screen and conversation | Claude Code, Codex, Copilot and a tasks thread are usable from the page. A thread shown in the TUI and on the page at once keeps the TUI's size and draws right in both. The Conversation tab shows a two-week thread from its first prompt and finds a word in it. A `desl daemon restart` loses nothing on the page |
| 3 | Waiting list and approvals | For each of the three agents, an approval card shows its dialog, Open its screen shows it, and answering it there clears the card and the TUI's marks |
| 4 | Questions | `desl ask`, the two events, `desl answer` and the skill move exist. Each of the three agents asks in a test project and gets its answer as the next prompt. A typed prompt supersedes an open question |
| 5 | Live document | An agent's edit to `SPEC.md` shows within a second, with its agent and thread. Undo works, and is refused when the passage has changed again. A block saved on the page lands in the file while an agent edits another section, and an agent's edit to the same block meanwhile brings up both versions. Claude Code, Codex and Copilot each read a document again after the user changed it, before editing it. A shell overwrite of the user's block shows with Restore. A comment reaches the thread, the agent resolves it, and the comment moves when its quote is gone |
| 6 | Task list | A status change on the page shows in the tasks thread at once, and the reverse. "Start in…" gets a thread working on the task |
| 7 | Security review | Someone who didn't write the code tries to drive an agent from another website, a wrong token, a rebinding host name, injected HTML, an outside image in a document and a save outside the project's documents, and fails each time |

Each milestone follows the definition of done in `SPEC.md`: spec match, tests, help text, error paths tried by hand, and budgets.

## The skill

`skills/desl/SKILL.md` gains three moves.

**Ask the user with `desl ask`**
- Ask only for decisions that are the user's.
- Give 2 to 4 options, with the recommended one first and a one-line trade-off for each.
- After asking, end the turn, unless there is work that doesn't depend on the answer.
- Never ask the same question twice, and never answer your own question.

**Act on a comment**
- A prompt that starts `desl: comment` carries a comment.
- Make the change it asks for, then `desl comment resolve <id> --note "<what changed>"`.
- When you disagree or need more, say so in your reply and leave the comment open.

**Share a document with the user**
- A Markdown document can change while you work: the user edits it on the page.
- Edit it with your own edit tool, never a shell command, so desl can tell who changed what and your edit cannot overwrite the user's.
- When an edit fails because the file changed, read it again and build on what is there.
- A prompt that starts `desl: the user changed` reports the user's change; take it into account.

## Open questions

1. **When.** Phase 3, beside remote TCP mode, or right after the phase 1 gate?
2. **One thread on two screens.** Version 1 keeps the TUI's size while a TUI shows the thread. Should a later version follow the client that typed last, as tmux's `window-size latest` does? It needs the TUI to draw a screen of another size.
3. **Questions and comments in the TUI.** How does the TUI show and answer a desl question without a modal (moment 5)? One option is a line under the thread in the sidebar, answered with a key. Does the TUI show comments at all, or only their prompts?
4. **Port.** Settled in milestone 1: 7428, else the next free one of the 19 after it, or `--port`. Still open: keeping the port and the token when `desl web` starts again from a new binary (milestone 2).
5. **Prompt wording.** The exact text that answers and comments arrive as. It is tested with each agent, like the agent preamble in `SPEC.md`'s open questions.
6. **Default document.** Is the order `SPEC.md`, `AGENTS.md`, `README.md` right, or should a project pin its document?
7. **Claude Code's own questions.** Start Claude with `--disallowedTools AskUserQuestion` (present in Claude Code 2.1.329) so it asks with `desl ask` in every client, which needs question 3 answered for the TUI first; or have the Claude adapter read `AskUserQuestion` from the session file as a `question`, shown on the page and answered on Claude's own screen. Proposed: the second for version 1, the first once the TUI can answer a desl question.

## Decisions so far

- The page is a client of the daemon over the existing protocol, not code inside the daemon.
- `tokio` and `axum` are used in `desl web` only. The daemon and the TUI stay on threads.
- The page's files are compiled into the single binary, with no frontend build step in version 1.
- Answers, comments and wake-ups use the existing `prompt` path: no tmux, no MCP server, no channels.
- Agents ask through `desl ask` and the skill. An MCP server is not part of version 1.
- Every indicator the TUI shows is shown on the page, with the same words and colour roles.
- Documents: comments and editing, both in version 1 (the author's call on 2026-11-21). Editing is a block at a time, on Markdown files only, and a save tells no agent unless the user presses `Tell <thread>`.
- Approvals in version 1 open the agent's own screen; the page presses no keys in an agent's dialog.
- A thread shown in the TUI keeps the TUI's size; the page draws it at that size.
- Phone: the page lays out at phone width, but it listens on 127.0.0.1, so a phone reaches it only with phase 3's remote access.
- Shell and file-manager threads stay in the TUI only.
