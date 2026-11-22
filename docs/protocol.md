# The daemon protocol

How a client talks to the desl daemon. The definition is `crates/desl-proto/src/lib.rs`; this
page explains it. Version **1**.

One daemon per user per machine listens on a Unix socket, `$XDG_RUNTIME_DIR/desl.sock` by default
(`DESL_SOCKET` and `DESL_HOME` move it; the path must be under 100 bytes). The socket is readable
and writable by its owner only. The TUI, the CLI and any later client all speak this protocol
and nothing else: no client touches the store or an agent process.

## Frames

Every message is one frame: a 4-byte big-endian length, then that many bytes of UTF-8 JSON.
A frame is at most 256 MB. A connection that closes between frames ended cleanly.

There are three families.

| Family | Direction | What |
| --- | --- | --- |
| command | client to daemon | A request with an id. It gets exactly one reply with the same id. |
| event | daemon to client | Every IR event and every workspace change, as it happens. Sent after `subscribe`. |
| stream | daemon to client | Token deltas of the thread the client is viewing. Never stored. |

## Commands

A request is one flat object: an `id` the client chooses, a `command` name, and the command's
fields.

```json
{"id": 7, "command": "prompt", "thread_id": "t-7f3a", "text": "Add refresh-token rotation"}
```

The reply carries the same `id` and exactly one of `ok` and `error`. `error` is a complete
sentence that says what happened and what to do next; clients show it as it is.

```json
{"type": "reply", "id": 7, "ok": {"reply": "sent", "queued": false}, "error": null}
{"type": "reply", "id": 8, "ok": null, "error": "This thread is already on codex."}
```

| Command | Fields | Reply | Notes |
| --- | --- | --- | --- |
| `hello` | `protocol`, `client` | `hello {protocol, version, pid}` | First frame on every connection. A different major `protocol` is refused. `client` names the client: `tui`, `cli`, or `web` for `desl web`'s bridge. |
| `workspace` | | `workspace` | Projects, threads, agents, and `file_manager`: the program a files thread runs (`yazi`) when it is on the daemon's `PATH` now, absent otherwise. No subscription. |
| `subscribe` | `cwd?` | `workspace` | As `workspace`, and from now on every change arrives as an event. `cwd` registers that directory's repository as a project, makes sure it has a thread, and sets `project_id` and `focus` in the reply. |
| `open_thread` | `thread_id` | `thread {thread, events, partial}` | The thread's whole log, and its token stream from now on. `partial` is the text of a block still being generated. |
| `view` | `thread_id` | `viewing {thread_id, partial}` | Moves the token stream to a thread whose events the client already has. |
| `events` | `thread_id` | `events {events}` | The log, without viewing. |
| `prompt` | `thread_id`, `text` | `sent {queued}` | `queued` when the agent was busy; the prompt goes out when the turn ends. |
| `interrupt` | `thread_id` | `done` | Stops the current turn. An agent in its own interface gets its own interrupt key; the end of the turn arrives as an event. |
| `exit` | `thread_id` | `done` | Ends the thread's agent; the thread stays idle, and its next prompt or view resumes the agent's session. |
| `restart` | `thread_id` | `done` | Ends the thread's agent and, once it has exited, starts it again on the same session, with the binary and settings installed now; clients showing the thread get `pty_replace`. |
| `answer` | `thread_id`, `call_id?`, `decision` | `done` | Answers the pending approval: `allow`, `deny` or `always`. |
| `switch` | `thread_id`, `agent` | `done` | The brief goes out with the next prompt. |
| `fork` | `thread_id`, `agent?`, `at?` | `created {thread_id}` | A new thread with the history up to event `at` (the newest when absent). |
| `subagent` | `thread_id`, `subagent_id` | `events {events}` | One of the thread's subagents' conversation, read-only, numbered from 1 |
| `new_thread` | `cwd`, `agent?`, `prompt?`, `beside?` | `created {thread_id}` | `agent` `shell` opens the user's shell in `beside`'s project at `cwd`, `files` their file manager in the directory `beside` is in (an error when none is installed), and `tasks` the project's task list: the tasks thread of `beside`'s project when it has one (its id comes back), else a new one at the project's root; from a shell or file manager `beside`, the agent opens where it is, as its own project, and it joins it |
| `rename` | `thread_id`, `name` | `done` | |
| `remove_thread` | `thread_id` | `done` | Ends its agents and deletes its history. |
| `move_project` | `project_id`, `up` | `done` | Moves the project one place up or down among the projects that have a thread, the ones a sidebar lists, and keeps the order across restarts. An error sentence when it is already first or last. |
| `brief` | `thread_id`, `to?` | `brief {to, text, token_estimate, turns, delta, needed}` | What a switch to `to` would send now. `needed` is false when `to` has seen everything. |
| `clear` | `thread_id` | `done` | A fresh native session on the same thread; the history stays. |
| `models` | `thread_id` | `models {agent, models, current, effort?}` | The models the thread's active agent offers, each with `id`, `name`, `description`, `default`, and where the model has them `efforts[] {id, description?}` and `default_effort`. `current` is the id this thread uses; `effort` is the thread's own pick of a reasoning effort, absent on the agent's own setting. The first ask per agent may take a second: the daemon asks the agent, then remembers. |
| `set_model` | `thread_id`, `model?` | `model_set {agent, model, effort?}` | Picks a model by id or name; absent or `default` goes back to the agent's default. It reaches the agent with the next prompt and keeps the native session. An effort the new model does not take is dropped with the old model; `effort` in the reply is what is left. |
| `set_effort` | `thread_id`, `effort?` | `model_set {agent, model, effort?}` | Picks a reasoning effort for the model in use, by level id; absent or `default` goes back to the agent's own setting. A level the model does not take is refused with a sentence that lists its levels. It reaches the agent like a model does. |
| `set_mode` | `thread_id`, `mode` | `mode_set {mode, meaning}` | What the thread's agents may do without asking: `ask`, `edits` or `yolo`. The broker follows at once, and in `yolo` it answers a question that is already waiting; an agent's own flags follow with its next turn. `meaning` is the mode in words for the thread's active agent. |
| `wait` | `thread_id` | `waited {status}` | Replies when the thread is waiting, idle or failed. The only reply that can take long. |
| `agents` | `refresh` | `agents {agents}` | Detection results and capabilities. `mode_notes[] {mode, note}` lists the permission modes an agent cannot hold as written, with what it does instead. |
| `history` | `limit` | `history {prompts}` | Recent prompts across threads, newest first. |
| `files` | `thread_id` | `files {files}` | The project's files, for the `@` picker. |
| `blob` | `hash` | `blob {text}` | The full text of a truncated tool output. |
| `save_view` | `view` | `done` | What the client wants back on its next attach. Opaque to the daemon. |
| `attach` | `thread_id`, `rows`, `cols`, `answers` | `attached {thread_id, agent, replay}` | Shows the thread's agent in this client's terminal (the native route). The agent's own interface starts at this size if it is not running, resuming its session when the thread has one. `replay` draws its screen as it is now, base64: what it drew, with a mark wherever its terminal was given a size (`ESC _ desl;size;<rows>;<cols> ESC \`, an application program command, which a terminal ignores), and with everything older than the last megabyte or so folded into the screen, the history and the modes it left behind. The client draws each part at its size (`desl_proto::replay::play`) and puts the result on screen in one frame, and `pty` frames follow. `answers` is what the client's terminal replied to the questions agents ask (`fg`, `bg`, `cursor_color`, `da1`, `kitty`, each the reply verbatim); the daemon gives the same replies while no client shows the agent. |
| `input` | `thread_id`, `data` | none | Keys for the shown agent, base64, exactly as the terminal sent them. Never replied to. |
| `resize` | `thread_id`, `rows`, `cols` | none | The client's terminal changed size. Never replied to. |
| `leave` | `thread_id` | none | This client no longer shows the thread: its pane closed or moved to another thread. A client may show several threads at once, one per pane. Never replied to. |
| `doctor` | | `doctor {checks}` | Store, agents and daemon checks. |
| `form_new` | `thread_id`, `form` | `form_shown {form_id}` | Puts a form in front of the user on desl web's page for the thread's agent (`docs/design/2026-11-21-web-forms.md`). The form is checked; a refusal names the first thing wrong, by the widget's place and id. At most 256 KB and 30 widgets. The thread is `waiting` while a form is open and its agent idle. |
| `form_answer` | `form_id`, `answer` | `done` | The user's answer, checked against the form: it closes the form and goes to the agent as its next prompt, through the `prompt` path. A `document` widget's answer is `{text, base?, comments}`: desl saves the section into the file only if it still equals `base` (the text the card opened with when left out), else refuses and nothing is saved; the agent gets `{saved, diff, more?, comments}` instead. A form closes once. |
| `form_close` | `form_id`, `by` | `done` | Closes a form unanswered: `withdrawn` (the agent) or `dismissed` (the user). A dismissal is told to the agent in a line before its next prompt. |
| `form_draft` | `form_id`, `draft` | `done` | The page's state of an open form, kept until it closes, so a reload or a restart finds it as it was. |
| `form_get` | `form_id` | `form {form_id, thread_id, form, draft?, answer?, closed_by?}` | A form, with its draft while open and its answer once answered. |
| `form_image` | `form_id`, `path` | `image {mime, data}` | A picture an `images` widget of the form names, read again from inside its thread's directory (links followed; PNG, JPEG, GIF, WebP or SVG; at most 10 MB), `data` in base64. A path the form does not name is refused, so a client cannot read other files through it. |
| `form_section` | `form_id`, `widget` | `section {text?}` | The section a `document` widget of the form edits, as it is on disk now, from inside its thread's directory; no `text` when its heading is gone. The page asks when a save was refused because the file changed, to show both versions. |
| `shutdown` | | `done` | Ends every agent, then the daemon. The connection closes when it has finished. |
| `handover` | `exe` | `handed_over {pid, kept}` | The daemon starts again from the desl binary at `exe` and hands it the terminal of every running agent and shell, so nothing ends. The reply comes from the daemon that leaves, once the new one serves on the same socket: `pid` is the new daemon's, `kept` how many terminals it took. An error means the daemon that was running stays as it was. A daemon from a build without this command closes the connection instead of answering. |

## Events

```json
{"type": "event", "change": {"what": "appended", "event": {"index": 3, "thread_id": "t-7f3a",
  "timestamp": 1794000000000, "agent": "claude", "kind": "tool_call", "id": "c1", "name": "read",
  "args": {"path": "src/auth/middleware.ts"}}}}
```

| `what` | Carries | When |
| --- | --- | --- |
| `project` | `project` | A project was registered, or the branch checked out in its folder or its count of uncommitted paths changed (read every five seconds while a client is attached). |
| `thread` | `thread` | A thread was created, or its name, status, agent, bindings, queue, pending approval, branch or open forms changed. `thread.open_forms` lists its forms not answered yet, `{id, title, at}`, oldest first. `thread.screen_asks` is true while its agent asks something in its own screen, a form open beside it or not. |
| `project_order` | `ids` | The user moved a project: every project's id, in the order to list them now. |
| `thread_removed` | `thread_id` | |
| `appended` | `event` | An IR event was appended to a thread's log. |
| `agents` | `agents` | Detection ran again. |
| `bye` | | The daemon is stopping. |
| `restarting` | | The daemon handed its agents to a new daemon and is leaving. The client connects to the same socket again and finds every thread as it was. |

A subscribed client hears the events of every thread, so a thread it has loaded stays current
without asking again.

### The IR event

Every event has `index` (starting at 1, dense, never reused), `thread_id`, `timestamp`
(milliseconds since the Unix epoch), `agent` (the thread's active agent at that moment) and a
`kind` with its fields in the same object.

| `kind` | Fields |
| --- | --- |
| `user_message` | `text`, `attachments[]` |
| `assistant_message` | `text`, `model?` |
| `tool_call` | `id`, `name`, `args` |
| `tool_result` | `call_id`, `output`, `exit_code?`, `is_error`, `truncated`, `blob?` |
| `file_change` | `path`, `diff?`, `change` (`create`, `modify`, `delete`), `call_id?` |
| `approval_request` | `call_id`, `tool`, `summary`, `risk` (`read`, `edit`, `command`, `network`, `destructive`) |
| `approval_response` | `call_id`, `decision` (`allow`, `deny`, `always`), `by` (`user`, `policy`) |
| `agent_switch` | `from`, `to`, `brief_event_id?` |
| `fork` | `parent_id`, `fork_point` |
| `brief` | `text`, `token_estimate`, `covers_range`, `delta` |
| `usage` | `input_tokens`, `output_tokens`, `cost_estimate?` |
| `error` | `message`, `recoverable` |
| `interrupt` | |
| `form` | `form_id`, `title`, `blob` (the form's JSON, by the `blob` command) |
| `form_answer` | `form_id`, `by` (`user`, `withdrawn`, `dismissed`), `blob?` (the answer's JSON, for `user`) |

`tool_call.name` is `read`, `edit`, `bash`, `search`, `fetch`, `agent`, `todo`, or the agent's
own name for anything else. `args` uses the neutral keys `path`, `command`, `pattern`, `query`,
`url` and `description` where they apply, so a client can show a tool call without knowing the
agent. `tool_result.output` is cut at 16 KB; when `truncated`, the `blob` command returns all of it.

## Stream

```json
{"type": "stream", "thread_id": "t-7f3a", "delta": {"kind": "start"}}
{"type": "stream", "thread_id": "t-7f3a", "delta": {"kind": "text", "text": "Rotation is "}}
```

`start` opens a new block of prose; `text` adds to it. The block ends when its
`assistant_message` event arrives with the final text, which replaces what was streamed.
Codex in exec mode sends no deltas: its messages arrive whole.

## PTY frames

```json
{"type": "pty", "thread_id": "t-7f3a", "data": "G1szMm3il48bWzBtIFJvdGF0aW9u"}
{"type": "pty_ended", "thread_id": "t-7f3a", "code": 0}
```

On the native route a client that attached to a thread gets every byte its agent draws, base64,
in order, after the `attached` reply. `pty_ended` says the agent's interface quit; the next
`attach` starts it again on the same session.

```json
{"type": "pty_replace", "thread_id": "t-7f3a", "agent": "codex", "data": "G1s/MjAyNmg…"}
{"type": "notice", "thread_id": "t-7f3a", "text": "codex quit before it was ready, so the thread stays on claude. Press ctrl+b s to try again."}
```

`switch` on a thread whose agent runs in a PTY replies `done` at once and works out of sight:
the daemon copies the conversation into a new session of the other agent and starts it hidden.
When it has drawn, every client showing the thread gets `pty_replace` with its whole output, to
put on screen in one frame; `pty` frames after it are the new agent's. A switch that fails after
the reply ends in a `notice`. A switch is refused while the agent is working or waiting.

## A first exchange

```
client                                   daemon
hello {protocol: 1}                  ->
                                     <-  reply hello {protocol: 1, version, pid}
subscribe {cwd: "/home/me/api"}      ->
                                     <-  reply workspace {projects, threads, agents, focus: "t-7f3a"}
open_thread {thread_id: "t-7f3a"}    ->
                                     <-  reply thread {thread, events, partial}
prompt {thread_id, text}             ->
                                     <-  event appended user_message
                                     <-  event thread (status working)
                                     <-  reply sent {queued: false}
                                     <-  stream start, stream text ...
                                     <-  event appended assistant_message
                                     <-  event appended usage
                                     <-  event thread (status idle)
```

## Versioning

`PROTOCOL` in `desl-proto` is the major version. The daemon refuses a client whose major
differs and tells it to run `desl daemon restart`. Fields may be added within a major version;
clients ignore fields they do not know.
