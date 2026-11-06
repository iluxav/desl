# Adapters

The adapter contract, a worked example, and what was learned about each agent's headless mode.
Everything about the agents was verified by running the installed binaries on 2026-11-04; the
flag names in `SPEC.md` were from memory, these are real. Captured output lives in
`crates/adapters/<agent>/fixtures/<version>-<scenario>.jsonl`, each with the IR it must produce
beside it as `<fixture>.ir.json`.

## The contract

An adapter is the only code that knows an agent's native format. It lives in
`crates/adapters/<id>/` and implements two traits from `desl-ir` (`crates/desl-ir/src/adapter.rs`).

| `Adapter` | |
| --- | --- |
| `id()` | The word the user types after `/switch`: `claude`, `codex`. |
| `detect()` | Installed, version, logged in, binary path, and the sentences to show when it is missing or logged out. |
| `capabilities()` | `resume`, `fork`, `model_switch`, `compact`, `interrupt`, `approvals`, `streaming`. |
| `permission_note(mode)` | A few words when the agent cannot do what a permission mode says; none when the mode holds as written. Shown in the permissions list in place of the mode's own meaning. |
| `start(opts, sink)` | Prepares a native session for a thread: working directory, permission mode, the session id to resume. Nothing reaches the agent until the first `send`. |

| `Session` | |
| --- | --- |
| `send(turn)` | One user turn. A brief after a switch is part of the same turn. |
| `interrupt()` | Stops the turn. Returns true when the turn is over at once. |
| `answer(call_id, decision)` | Answers an approval request. |
| `close()` | Ends the process: SIGTERM to its process group, SIGKILL five seconds later. The native session stays resumable. |

A session reports through the `sink` it was started with, from its own threads:

| `AdapterEvent` | Meaning |
| --- | --- |
| `Session {session_id, model}` | The native session exists. The daemon keeps the id in the thread's binding and never shows it unasked. |
| `TextStart`, `TextDelta {text}` | A block of prose begins and grows. Streamed to viewers, never stored. |
| `Event {event}` | An IR event for the thread's log (`docs/protocol.md` lists the kinds). |
| `TurnEnd {error}` | The turn is over. `error` is a sentence when it ended badly. |
| `Exit {code, detail}` | The process is gone. Mid-turn this fails the thread; between turns it is silent. |

Rules every adapter keeps:

- Tool names and arguments are neutral: `read`, `edit`, `bash`, `search`, `fetch`, `agent`,
  `todo` with the keys `path`, `command`, `pattern`, `query`, `url`, `description`. Paths inside
  the thread's directory are relative to it.
- An edit is a `tool_call`, a `tool_result` and a `file_change` with the unified diff when the
  adapter can tell what changed.
- Tool call ids are unique within a thread, across turns and across daemon restarts.
- Agent processes run in their own process group, without the variables in SPEC.md "Secrets".
- Every sentence a user may read says what happened and what to do next.

### A worked example

Claude Code prints this line when it wants to run a command it may not run unasked:

```json
{"type":"control_request","request_id":"9f1c","request":{"subtype":"can_use_tool","tool_name":"Bash",
 "input":{"command":"touch /tmp/x","description":"Create a file"},"tool_use_id":"toolu_01"}}
```

The Claude adapter's parser (`crates/adapters/claude/src/parse.rs`) turns it into one IR event and
remembers the native request under the call id:

```json
{"type":"event","event":{"kind":"approval_request","call_id":"toolu_01","tool":"bash",
 "summary":"touch /tmp/x","risk":"command"}}
```

The daemon's approval broker answers it from the thread's "always" list or sets the thread
`waiting`. When the user presses `y`, the daemon calls `answer("toolu_01", allow)` and the adapter
writes Claude Code's own reply to the process:

```json
{"type":"control_response","response":{"subtype":"success","request_id":"9f1c",
 "response":{"behavior":"allow","updatedInput":{"command":"touch /tmp/x","description":"Create a file"}}}}
```

Nothing outside the adapter saw `control_request`, `can_use_tool` or `toolu_`-style ids as
anything but opaque strings.

### Fixtures and replay tests

`cargo test -p adapter-claude -p adapter-codex` replays every fixture through the parser and
compares the result with the `.ir.json` beside it, byte for byte. A new agent version adds
fixtures; old ones are never edited. After a deliberate change to the mapping,
`DESL_BLESS=1 cargo test -p adapter-<id>` rewrites the expected files, and the diff is reviewed
like code.

### Detection

| Agent | Version | Logged in |
| --- | --- | --- |
| Claude Code | `claude --version` prints `2.1.312 (Claude Code)` | `claude auth status --json` prints an object with `loggedIn` (verified; it also holds the account's email, which desl does not read) |
| Codex | `codex --version` prints `codex-cli 0.165.0` | `codex login status` prints `Logged in using ChatGPT` and exits 0 (verified; the logged-out output was not observed) |

### Models

`Adapter::models` returns what the agent's own `/model` picker shows, so desl's picker is the same list.

| Agent | How the list is read | Notes |
| --- | --- | --- |
| Claude Code | Start `claude -p --output-format stream-json --input-format stream-json --verbose`, write `{"type":"control_request","request_id":"…","request":{"subtype":"initialize"}}`, read the `control_response`. `response.response.models[]` has `value`, `displayName`, `description`, `resolvedModel` and effort levels. | Verified: answers in about 0.2 s, no prompt is sent, nothing is billed. The same reply carries `account` (email, organization); desl does not read it and the fixture leaves it out. The entry with `value: "default"` is "whatever Claude Code would use"; desl passes no flag for it. |
| Codex | `codex debug models` prints `{"models":[…]}` with `slug`, `display_name`, `description`, `visibility` (`list` or `hide`) and `priority`. | Verified: 22 ms. Hidden entries are left out. The default is the `model` in `~/.codex/config.toml` when it is on the list, else the first entry. `codex app-server` answers `model/list` with the same models and an `isDefault` flag; not used. |

The pick reaches the agent as a flag on its next process: `claude --resume <id> --model sonnet`
and `codex exec -s … -m gpt-6-luna resume <id> …`. Both verified: the native session is kept and
answers on the new model. Codex then emits an `error` item, "This session was recorded with model
… but is resuming with …"; the adapter drops it, because the user asked for exactly that.

The model fixtures (`2.1.312-initialize-models.json`, `0.165.0-debug-models.json`,
`0.165.0-debug-models-levels.json`) are real output cut down to the fields desl reads.

### Launching the interface (the native route)

`launch(opts)` gives the command line of the agent's own interface, run by the daemon's PTY host.
Verified 2026-11-05 against Claude Code 2.1.318 and Codex CLI 0.166.0, each started, given a
turn, detached from and reattached to under the daemon; Claude Code also quit and reopened on
the same session. Codex names a new session itself, and until the adapter finds it in Codex's
session files a Codex thread that quits reopens on a new session. Codex asks whether to trust a
folder it has not seen, in its own interface, before anything else.

| Agent | New thread | Existing session | Mode flags |
| --- | --- | --- | --- |
| Claude Code | `claude --session-id <uuid>` (desl picks the id, so the session file is known) | `claude --resume <id>` | `--permission-mode manual` (ask), `acceptEdits` (edits, worktree), `bypassPermissions` (yolo); `--model`, `--effort` when picked |
| Codex | `codex --no-daemon` (Codex names the session) | `codex --no-daemon … resume <id>` | `-s read-only -a on-request` (ask: Codex asks before anything that writes), `-s workspace-write -a on-request` (edits, worktree; asks before anything outside the project). 0.166 takes only `on-request` and `never` for `-a`; `untrusted` fails at start, `--dangerously-bypass-approvals-and-sandbox` (yolo); `-m`, `-c model_reasoning_effort="…"` when picked; all before `resume` |

Claude Code's interface sends a kitty graphics query (`ESC _ G … a=q … ESC \`) at start; tmux
takes any APC as a pane title, so the client puts desl's title back after one. On `/exit` Claude
Code prints its own `claude --resume <id>` line.

### Reading the session files (the native route)

While an agent's interface runs, the daemon follows its session file (`session_file`,
`session_reader`): every line the agent appends becomes events in the thread's log, and turn
markers set the thread's status. The offset read so far is kept per binding, so nothing is read
twice; a thread whose log already held the session's history (from the headless route) starts at
the end of the file. Verified 2026-11-05 on Claude Code 2.1.318 and Codex CLI 0.166.0.

| | Claude Code | Codex |
| --- | --- | --- |
| Found by | the id desl gave it (`--session-id`) | the newest rollout for the cwd created after the start; its id becomes the binding's |
| Working | a user line whose content is a string (not a tag) | `event_msg` `task_started` |
| Idle | `system` `turn_duration`; `[Request interrupted…]` | `task_complete`; `turn_aborted` |
| Events | user, assistant and tool-result lines through the stream parser (`toolUseResult` is the stream's `tool_use_result`); `isSidechain` lines skipped | `response_item` messages, `custom_tool_call` (`exec` scripts unpacked to their command, `apply_patch` to an edit), `function_call`, outputs |
| Waiting (bottom 14 rows, only mid-turn) | `Do you want to` with `1. Yes`; the trust question | `Would you like to run the following command?` (seen live), `Trust this folder` |
| Model and effort chosen inside | each reply's `message.model`; `/effort`'s "Set effort level to …" (Claude Code also saves both as the user's default) | `event_msg` `thread_settings_applied` `{model, reasoning_effort}`; Codex's `/model` with `s` changes only the session, and its file holds nothing until the first turn |

In `ask` mode (`-s read-only -a on-request`) Codex asks only when the model decides to request
more: asked plainly to create a file it reported the sandbox refusal; asked to request
permission, it showed the approval above.

### Writing a copied session (the switch)

`write_session(cwd, events)` writes the thread's conversation as a new session of the
receiving agent and returns its id, file and length; the daemon starts reading the file at that
length, so the copy is never read back into the log. Built on the `phase0/proto-n` findings
(see "Copying a conversation into the other agent's session file"); both writers have
round-trip tests (written, then read back by the same adapter's reader).

| Neutral call | Claude Code gets | Codex gets |
| --- | --- | --- |
| `bash` | `Bash {command}` with its `tool_result` | `exec_command` with a `CommandExecution` item |
| `read` | `Read {file_path}` (absolute) | `cat <path>` |
| `search` | `Grep {pattern}` or `WebSearch {query}` | `rg <pattern>` |
| `fetch` | `WebFetch {url}` | told as a comment, with its output |
| `edit` and anything else | told in the assistant's words, an edit with its diff | a commented `exec` call whose output is the diff |

Outputs are cut at 4,000 bytes. Driven 2026-11-05 on a thread of two Claude turns (a `cat`, a
`touch`): Codex opened on both, showed the commands with their output, and answered from memory;
switched back, Claude showed Codex's turn as its own. Measured in tmux every 200 ms, the screen
went from one agent to the other with no blank frame, 1.4 to 2 seconds after the key, plus
Codex's folder-trust question the first time it sees a directory.

### Permission modes

`StartOpts.permission_mode` is the thread's mode. Each adapter maps it to its agent's flags.

| Mode | Claude Code | Codex |
| --- | --- | --- |
| `ask` | `--permission-mode manual`. Verified: an edit arrives as a `can_use_tool` request and waits for the answer; reads inside the project do not ask. | `-s read-only`. Verified: asked to create a file, Codex answers that the filesystem is read-only and nothing is written. Exec mode cannot ask, so this is the mode that changes nothing unasked. |
| `edits` | `--permission-mode acceptEdits`. Edits pass, commands arrive as requests. | `-s workspace-write`. Edits and commands stay inside the workspace (and `/tmp`). |
| `yolo` | `--permission-mode acceptEdits`, and desl's broker allows every request at once. Verified: a command that asks in `edits` runs without a question, and the log holds "policy answered Allow". `bypassPermissions` is not used: it would leave nothing in the log. | `-s danger-full-access`. Verified: a write outside the workspace succeeds. |

A mode change replaces the agent's process before its next turn and resumes the native
session; both agents were verified to keep the conversation across it. `Adapter::permission_note`
is how Codex says what `ask` and `edits` mean for it.

### Reasoning effort

Each model lists its effort levels, and `/model` offers them after a model is picked.

| Agent | Where the levels are | The flag | Notes |
| --- | --- | --- | --- |
| Claude Code | `supportedEffortLevels` on each entry of `models[]`, when `supportsEffort` is set. Names only. | `--effort <level>` | Verified: accepted with `--resume`, and a different level on the next process keeps the session. An unknown level is a warning on stderr and Claude Code falls back to its default, so desl checks the level first. Claude Code does not report which level a session uses when none is passed; the list says "claude's own setting" and no more. |
| Codex | `supported_reasoning_levels[] {effort, description}` and `default_reasoning_level` on each entry of `codex debug models`. | `-c model_reasoning_effort="<level>"`, before `resume` | Verified: accepted on a new and on a resumed session. A level the model does not take fails the turn with a 400 from the API, so desl checks the level first. With no pick the level is `model_reasoning_effort` from `~/.codex/config.toml` when the model takes it, else the model's default; the list names it. |

`StartOpts.effort` carries the pick. `ModelInfo.efforts` and `ModelInfo.default_effort` carry
the lists; an adapter whose agent has no such setting leaves both empty and the second list
never opens.

## Agent notes

| Agent | Version | Installed via |
| --- | --- | --- |
| Claude Code | 2.1.312 | mise (`~/.local/share/mise/installs/claude/latest/claude`) |
| Codex CLI | 0.165.0 | mise (`~/.local/share/mise/installs/codex/latest/bin/codex`) |

## Claude Code 2.1.312

### Flags, from `claude --help`

Prototype A spawns:

```
claude -p --output-format stream-json --input-format stream-json --verbose \
       --include-partial-messages --permission-prompts host --permission-prompt-tool stdio \
       --permission-mode acceptEdits [--resume <session-id>]
```

| Flag | Status | Notes |
| --- | --- | --- |
| `-p, --print` | verified | Non-interactive mode. Skips the workspace trust dialog. |
| `--output-format stream-json` | verified | One JSON object per line on stdout. |
| `--input-format stream-json` | verified | User turns are JSON lines on stdin; the process stays alive between turns until stdin closes. |
| `--verbose` | verified | Always passed; not yet tested whether stream-json works without it. |
| `--include-partial-messages` | verified | Adds `stream_event` lines with token deltas. Without it, text arrives whole per block. |
| `--permission-prompts host\|none` | verified | `none` auto-denies anything that would prompt. |
| `--permission-prompt-tool stdio` | verified, **not listed in `--help`** | Mentioned only in the `--permission-prompts` text. Required: without it, a tool call that needs approval is auto-denied (fixture `approval-denied-no-prompt-tool`). With it, a `control_request` of subtype `can_use_tool` arrives on stdout (fixture `approval-can-use-tool`). |
| `--permission-mode` | verified | Choices: `acceptEdits`, `auto`, `bypassPermissions`, `manual`, `dontAsk`, `plan`. The `init` event reported `permissionMode: default` when `manual` was passed. Spec mode `edits` maps to `acceptEdits`. |
| `--resume <id>` | exists | Not yet exercised. |
| `--session-id <uuid>` | exists | Not yet exercised. |
| `--session-id <uuid>` | verified, interactive | "Use a specific session ID for the conversation (must be a valid UUID)". Prototype B starts `claude --session-id <fresh v4 uuid>`; the session file is then `<uuid>.jsonl`, known before it exists. Without it B took the newest file in the project directory, which is another Claude Code's file whenever one is working in the same directory. |
| `--fork-session` | exists | "When resuming, create a new session ID"; use with `--resume`. Not yet exercised. |
| `--append-system-prompt` | exists | For the desl preamble; not yet exercised. |
| `--replay-user-messages` | exists | Echoes our stdin messages back on stdout. Not used. |
| `--include-hook-events` | exists | Not used. Hook events `hook_started`/`hook_response` arrive anyway as `system` lines. |
| `--no-session-persistence` | exists | Must never be used: resume depends on persistence. |
| `--max-budget-usd` | exists | Possible safety net for the hallway sessions. |

### Output events observed

Every line has `type`. Those with `parent_tool_use_id` non-null belong to a subagent and are
ignored by the prototype.

| `type` | `subtype` / inner `event.type` | What the prototype does |
| --- | --- | --- |
| `system` | `init` | Takes `session_id`, `model`. Also lists `tools`, `mcp_servers`, `slash_commands`, `permissionMode`. |
| `system` | `hook_started`, `hook_response`, `status`, `thinking_tokens` | Ignored. |
| `system` | `permission_denied` | Only without `--permission-prompt-tool`; a matching `is_error` tool_result follows. |
| `stream_event` | `content_block_start` with `content_block.type == text` | Opens a new assistant item. |
| `stream_event` | `content_block_delta` with `delta.type == text_delta` | Appends `delta.text`. |
| `stream_event` | `thinking_delta`, `signature_delta`, `input_json_delta`, `message_*` | Ignored. Thinking is not shown. |
| `assistant` | one content block per line | `text` replaces the streamed text with the final text; `tool_use` adds a collapsed tool line (`id`, `name`, `input`). |
| `user` | `tool_result` blocks | Marks the tool line done (`is_error`, `content` as string or text parts). Also carries `[Request interrupted by user]` after an interrupt. |
| `user` | top-level `tool_use_result` | The structured twin of the result, one per line. An edit: `filePath`, `oldString`, `newString`, `originalFile`, `structuredPatch[] {oldStart, oldLines, newStart, newLines, lines[]}` with each line prefixed by a space, `+` or `-`; the prototype turns the hunks into the unified diff shown under the edit. A new file: `type: "create"`, `content`, `structuredPatch: []`, `originalFile: null`; shown as added lines. A read: `type: "text"`, `file {filePath, content, startLine, numLines, totalLines}`; the prototype numbers `content` from `startLine` instead of using the `cat -n` text of the result. A command: `stdout`, `stderr`, `interrupted`; not used. A failed call carries a string. |
| `result` | `success`, `error_during_execution`, ... | Turn done. `total_cost_usd`, `duration_ms`, `is_error`, `result` text. |
| `control_request` | `request.subtype == can_use_tool` | Thread turns `waiting`; see below. |
| `control_response` | | Acknowledges our requests (`interrupt`: `{"still_queued": []}`). |
| `rate_limit_event` | | Ignored. |

### Input protocol

User turn:

```json
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
```

Interrupt (verified: acknowledged with `control_response`, then a `user` line
`[Request interrupted by user]` and a `result` with subtype `error_during_execution`):

```json
{"type":"control_request","request_id":"desl-1","request":{"subtype":"interrupt"}}
```

Approval. Incoming:

```json
{"type":"control_request","request_id":"<uuid>","request":{"subtype":"can_use_tool",
 "tool_name":"Bash","display_name":"Bash","input":{"command":"touch /tmp/x","description":"..."},
 "description":"...","permission_suggestions":[{"type":"addRules","rules":[...],"behavior":"allow","destination":"localSettings"}]}}
```

Our answer (verified: the tool runs; answering 12 s later also works, no timeout seen):

```json
{"type":"control_response","response":{"subtype":"success","request_id":"<uuid>",
 "response":{"behavior":"allow","updatedInput":{...the input...}}}}
{"type":"control_response","response":{"subtype":"success","request_id":"<uuid>",
 "response":{"behavior":"deny","message":"The user denied this in desl."}}}
```

No `initialize` handshake is needed for `can_use_tool` to arrive (tested with and without).
If the host never answers, Claude eventually fails the call with
`Tool permission request failed: AbortError: Tool permission stream closed before response received`.

### Other findings

- The user's own Claude Code settings apply inside the headless session. On this machine
  `echo` ran without asking under the user's permission rules, while `touch /tmp/...` asked
  because the path is outside the working directory. desl's approval strip therefore sees only
  what the user's settings leave open, which is the right behaviour for "the user's own logins".
- Running `claude -p` from inside another Claude Code session works. The prototype still strips
  every `CLAUDE*` environment variable from the child so it never inherits the parent's
  messaging socket or session id.
- After an interrupted turn the process exited with code 1 once stdin closed. The prototype
  respawns with `--resume <session-id>` on the next prompt when the process is gone.
- Answers to the SPEC open questions "Exact headless flags" and "Approval transport for Claude Code":
  stream-json in and out, `--permission-prompt-tool stdio`, drivable from a plain subprocess, no Node sidecar.

## Codex CLI 0.165.0

Verified by running `codex exec --json`; fixtures in `crates/adapters/codex/fixtures/0.165.0-*.jsonl`.
Login state: `codex login status` printed "Logged in using ChatGPT". The user's `~/.codex/config.toml`
sets the model and reasoning effort; probes passed `-c model_reasoning_effort="low"` for speed.

### Flags and commands

Prototype A runs one process per turn:

```
codex exec -s workspace-write [resume <thread-id>] --json --skip-git-repo-check -     # prompt on stdin
```

| Flag or command | Status | Notes |
| --- | --- | --- |
| `codex exec [PROMPT]`, `-` for stdin | verified | When stdin is not a TTY and no prompt argument is given, Codex waits on stdin ("Reading additional input from stdin..."); the prototype always writes the prompt and closes stdin. |
| `--json` | verified | One JSON object per line on stdout. Works with `exec resume` too. |
| `-C, --cd <DIR>` | verified | `exec resume` has no `-C`; the prototype sets the working directory on the process instead. |
| `-s, --sandbox read-only\|workspace-write\|danger-full-access` | verified | `workspace-write` lets commands write inside the workspace and `/tmp`; writes elsewhere fail inside the sandbox. |
| `--skip-git-repo-check` | verified | |
| `-c approval_policy="on-request"` | verified, no effect in exec | No approval event appeared; the blocked command produced **no item at all**, only the agent's own sentence saying it failed (fixture `approval-home`). |
| `codex exec resume <id> --json [PROMPT]` | verified | Keeps the context; `thread.started` repeats the same `thread_id`. |
| `-s` with `resume` | verified | `exec resume` has no `-s`: after `resume` it fails with `error: unexpected argument '-s' found`, exit 2. Before it (`codex exec -s workspace-write resume <id> …`) it is accepted, and the resumed turn wrote a file in the workspace. `-c sandbox_mode="workspace-write"` after `resume` also works, and so did no sandbox argument at all on a session that began with `workspace-write`. |
| `codex exec fork`, `codex fork` | exist | Not yet exercised. |
| `--ephemeral` | exists | Never use: resume depends on session files. |
| `codex app-server` | exists, experimental | JSON-RPC over stdio with per-item approval requests and message deltas; the phase 1 candidate if exec mode's limits below matter. |

### Output events observed

| `type` | `item.type` | What the prototype does |
| --- | --- | --- |
| `thread.started` | | Takes `thread_id` as the session id. |
| `turn.started` | | Ignored. |
| `item.completed` | `agent_message` | Opens and closes an assistant item with `text`. **Text arrives whole; there are no token deltas in exec mode.** |
| `item.started` / `item.completed` | `command_execution` | A tool line `run <command>`; `command` is `/usr/bin/bash -lc '...'` and is unwrapped. Completion carries `aggregated_output`, `exit_code`, `status`. |
| `item.started` / `item.completed` | `file_change` | One `edit <path>` line per entry of `changes[] {path (absolute), kind: add\|update\|delete}`. **The text of the change is not in the stream.** In every run observed, `item.started` arrived before the file was touched, so the prototype reads the file at `item.started` and again at `item.completed` and diffs the two. Files over 512 KB or not UTF-8 get no diff. |
| any item | `id` | **`item_0`, `item_1`, … start again in every turn**, and each turn is a new process. The prototype prefixes the ids with the turn's start time; before that, a tool call in a later turn was taken for an earlier one and never shown. |
| `item.completed` | `reasoning` | Ignored. |
| `turn.completed` | | Turn done; `usage` has token counts, no cost. |
| `turn.failed`, `error` | | Turn done with an error message. (Shapes assumed from the schema, not yet observed.) |

### Consequences for the spec

- **Codex approval policy** (open question): in exec mode there is no per-call approval channel. The
  sandbox policy decides; a blocked command leaves no trace in the event stream. desl's `ask` mode
  degrades to the sandbox's `workspace-write` for Codex and the UI must say so. `codex app-server`
  is the route to real per-call approvals.
- **Streaming**: Codex's reply appears all at once after the turn, so moment 4 cannot be tuned for
  Codex in Prototype A unless app-server is used.
- Session files: `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread-id>.jsonl`, which is
  what Prototype B will tail.

## Session files (Prototype B)

Prototype B read the agents' own transcripts instead of driving a headless stream. It was not
chosen and was removed on 2026-11-04; its parsers (`phase0/proto-a/src/transcript/`) and fixtures
(`phase0/fixtures/transcripts/`) are in the repository's first commit. The findings stay here
because they describe the agents, not the prototype.

### Claude Code 2.1.312

Path: `~/.claude/projects/<cwd with every / replaced by ->/<session-id>.jsonl`, one JSON object per
line, appended as the session runs. The file appears with the first prompt.

| `type` | Read as |
| --- | --- |
| `user` with `message.content` a string | The prompt the user typed (`origin.kind` is `human`, `turnOrigin` is `human`), unless the text starts with a tag. |
| `user`, string starting with a tag | Claude Code's own record, not a prompt. A local command such as `/model` writes three lines and no turn follows: `<local-command-caveat>` (`isMeta: true`), `<command-name>`, `<local-command-stdout>`; none has `turnOrigin`. Bash mode (`!echo hi`) writes `<bash-input>` and then `<bash-stdout>`; the second has `turnOrigin: "human"` and Claude answers it, so it opens a turn. `/cost` writes no user line at all. |
| `user` with a `tool_result` block | The result of a tool call: `tool_use_id`, `is_error`, `content`. |
| `user` with a text block `[Request interrupted by user]` or `[Request interrupted by user for tool use]` | Esc. Ends the turn; neither `end_turn` nor `turn_duration` follows. An interrupted tool also gets an error `tool_result` first. |
| `assistant` | One line per content block: `text`, `tool_use` (`id`, `name`, `input`), `thinking` (ignored). Lines are written when the message is complete, not as it streams. `message.stop_reason == "end_turn"` ends the turn; it repeats on every line of the same `message.id`, so it is counted once, and the thinking line carries it before the text line arrives. |
| `system` with `subtype: "turn_duration"` | Written once after every finished turn, after the last line of its last message. The dependable end of a turn. |
| other `system`, `summary` | Ignored. |
| `attachment`, `api-request-blob`, `api-request-shape`, `file-history-snapshot`, `mode`, `permission-mode` | Ignored. `api-request-blob` holds the full request including the system prompt. |

`isSidechain: true` lines belong to subagents and are skipped. A pending permission dialog is not
recorded in the file, which is why `waiting` in B comes from the screen.

The permission dialog on screen (2.1.312): a question that varies (`Do you want to proceed?` for a
command, `Do you want to create probe.txt?` for a new file), then `❯ 1. Yes`, a second option that
widens the permission (`Yes, and don't ask again for …`, or `Yes, and switch to accept edits …`),
`3. No`, and `Esc to cancel · Tab to amend`. Enter takes option 1, Down then Enter option 2, Esc
cancels. B treats the screen as a dialog only when both the question and `1. Yes` are present.

### Codex CLI 0.165.0

Path: `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread-id>.jsonl`. The first line is
`session_meta` with `id` and `cwd`. An interactive session writes nothing until its first turn.

| `type` / `payload.type` | Read as |
| --- | --- |
| `session_meta` | Session id; `cwd` identifies which project the file belongs to. |
| `event_msg` / `task_started`, `task_complete`, `turn_aborted` | Turn boundaries: working, idle, interrupted. |
| `response_item` / `message`, role `user` | The prompt, unless the text starts with a tag such as `<environment_context>`. |
| `response_item` / `message`, role `assistant` | The reply (`output_text`). Role `developer` is instructions and is ignored. |
| `response_item` / `custom_tool_call` | `name: "exec"` with a script in `input`: `tools.exec_command({cmd:"..."})` is a command, `tools.apply_patch("*** Update File: path ...")` is an edit. |
| `response_item` / `custom_tool_call_output`, `function_call_output` | The result, matched by `call_id`. |
| `response_item` / `function_call` | A named tool with JSON `arguments`. |
| `token_usage_record`, `turn_context`, `world_state` | Ignored. |

## Copying a conversation into the other agent's session file (spike, 2026-11-05)

Question: can a switch carry the whole conversation instead of a brief, by writing it into the
receiving agent's own session file and resuming that? Tested on Claude Code 2.1.318 and Codex CLI
0.166.0, both directions, with a real source session (one turn: a codename given in the prompt, a
fact that appears only in a tool's output, a rejected approach, one edit) and a fresh target session
of the receiving agent (one "ok" turn). The converter was a throwaway script; it is not in the repo.

| | Claude reads a copied Codex conversation | Codex reads a copied Claude conversation |
| --- | --- | --- |
| Headless resume (`-p --resume`, `exec resume`) | 4 of 4 recalled, including the tool-output fact | 4 of 4 recalled, including the tool-output fact |
| Native TUI resume | History shows as if native; Codex `cat`s are summarised as `Read 2 files, ran 1 shell command` | 4 of 4 recalled; the copied turn shows its prompt and reply, not its tool calls |

What it takes:

- **Thinking is dropped** both ways: Claude's is signed, Codex's reasoning is encrypted.
- **Tool calls become the receiver's own tools.** Codex `exec_command` → Claude `Bash` with a
  `tool_result`; Claude `Read`/`Bash` → Codex `exec` running `cat`/the command, `Edit` →
  `apply_patch`. Outputs were capped at 4,000 bytes each.
- **Claude**: lines spliced before the target's first prompt, each with its own `uuid`, chained
  through `parentUuid`, and the target's first prompt re-parented onto the last copied line. Copied
  `tool_use` ids need the `toolu_` shape. No other check bit.
- **Codex**: copied `response_item`s must carry **no `id`** on assistant messages and tool calls; with
  made-up ids the API answers 400 `This request does not support user-supplied input item IDs`. The
  copy must be a whole turn appended at the end, `task_started` … `task_complete`, with
  `item_completed` events (`UserMessage`, `AgentMessage`, carrying `thread_id` and timestamps) and
  `ordinal` renumbered. Lines placed inside an existing turn reach the model but not the screen.
- **Codex's screen comes from a cache**, `~/.codex/thread_history_1.sqlite`, projected from the
  rollout up to a stored byte offset. Appending at the end is picked up; rewriting earlier lines is
  not, until that thread's rows are cleared.
- **The Codex TUI runs through a shared background daemon** (`codex app-server`, from
  `~/.codex/packages/app-server-daemon/`). It keeps a resumed thread open after the TUI quits, holding
  `~/.codex/thread-writer-locks/<id>.lock`; `codex exec resume` then fails with `thread already has an
  active writer`, and a new `codex resume` forks the thread. desl must write a Codex file before the
  daemon has the thread open, or go through the app-server itself.

Later the same day, for `phase0/proto-n`:

- **A session written from nothing resumes in both agents**, so no throwaway turn is needed: a
  Claude file of chained lines with `parentUuid: null` on the first, and a Codex rollout whose
  only header is `session_meta` (`id`, `cwd`, `cli_version`, `source`, `model_provider`).
- **Codex shows such a session on screen only with `"history_mode": "paginated"` in
  `session_meta`.** Without it the model has the conversation and the screen shows a bare
  `Worked for <1s`. With it, one turn per user message and a `CommandExecution` item per command
  (`command`, `aggregated_output`, `exit_code`), the screen shows prompts, replies and commands
  with their output.
- **`codex --no-daemon`** runs the TUI without the shared app-server daemon, so quitting it frees
  the thread.
- Starting an agent in a hidden PTY needs answers to its terminal queries (cursor position, DA1,
  OSC 10/11/12 colours, kitty keyboard `CSI ? u`); desl asks the real terminal once at startup
  and answers with that. Replaying the hidden agent's output onto the real terminal inside
  synchronized output (`CSI ? 2026 h/l`) gives a switch with no blank frame: captured every
  200 ms in tmux, the screen went from one agent's history straight to the other's, 1.4 to 2.2 s
  after the key.

Not tested: long threads (context size), Codex's own
`apply_patch` and MCP calls copied into Claude, and how either format moves in the next release.

## Prototype dependencies

The crates the phase 0 prototype in `phase0/` uses. The crates of desl itself are named in the
stack paragraph of `SPEC.md`.

| Crate | Version | Why |
| --- | --- | --- |
| `ratatui` (with its `crossterm`) | 0.30 | The interface. In the spec's stack. |
| `serde`, `serde_json` | 1 | Agent streams, the host protocol, the log. In the spec's stack. |
| `pulldown-cmark` | 0.13, default features off | Parses an answer's markdown (tables, nested lists, links). Added 2026-11-04 after the hand-written renderer was judged poor. Not in the spec's stack; phase 1 needs a markdown parser named there. |
| `chrono` | 0.4, `clock` only | The status-bar clock. |
| `unicode-width` | 0.2 | Column widths for wrapping. |
| `anyhow` | 1 | Error context in the binaries. |

`portable-pty` 0.9 and `vt100` 0.18 served Prototype B's panes and left with it; `proto-n` uses them again (with `crossterm` 0.29 for raw mode and `chrono` for timestamps).


## Subagents (Claude Code 2.1.318)

Claude keeps each subagent of session `<id>` beside the session file, in `<id>/subagents/`:
`agent-<agentId>.jsonl` (its own transcript, every line `isSidechain: true` with `agentId`) and
`agent-<agentId>.meta.json`. The meta file is written at launch with `agentType`,
`description`, `toolUseId`, `spawnDepth`, `requestShape` (`foreground` or `background`) and
`pickedEffortModel`; `"finished": true` is added when the subagent is done (watched live
2026-11-06). A subagent whose turn was stopped stays unfinished, so desl shows one as running
only while its thread is busy.

Codex (0.166) keeps a subagent as a session of its own: a rollout whose `session_meta` has
`parent_thread_id` and `source.subagent.thread_spawn` (`parent_thread_id`, `depth`,
`agent_nickname` such as `Feynman`, `agent_role`). The parent records the spawn as an
`item_completed` event whose item is `CollabAgentToolCall` with `tool: spawn_agent`, `prompt`
(the task), and `receiver_agents` (`thread_id`, `agent_nickname`); later `wait` items report
`agents_states`. desl reads spawns from the parent's file as it grows, and a subagent is done
when its own file's last `task_started` is followed by `task_complete` or `turn_aborted`.
