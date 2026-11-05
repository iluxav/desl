# Adapter notes

Phase 0 findings about the agents' headless modes. Everything here was verified by running the
installed binaries on 2026-11-04. The flag names in `SPEC.md` were from memory; these are real.
Captured output lives in `phase0/fixtures/<agent>/<version>-<scenario>.jsonl`.

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

Verified by running `codex exec --json`; fixtures in `phase0/fixtures/codex/0.165.0-*.jsonl`.
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

Prototype B reads the agents' own transcripts instead of driving a headless stream. Fixtures in
`phase0/fixtures/transcripts/`; parsers in `phase0/proto-a/src/transcript/`.

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

## Prototype dependencies

Noted here as `AGENTS.md` requires. The phase 0 crate is throwaway; this list is not the phase 1
stack, which stays in the stack paragraph of `SPEC.md`.

| Crate | Version | Why |
| --- | --- | --- |
| `ratatui` (with its `crossterm`) | 0.30 | The interface. In the spec's stack. |
| `serde`, `serde_json` | 1 | Agent streams, the host protocol, the log. In the spec's stack. |
| `portable-pty` | 0.9 | Prototype B's panes. In the spec's stack. |
| `vt100` | 0.18 | Prototype B: turns a pane's bytes into a screen of cells. Not in the spec's stack. |
| `pulldown-cmark` | 0.13, default features off | Prototype A: parses an answer's markdown (tables, nested lists, links). Added 2026-11-04 after the hand-written renderer was judged poor. Not in the spec's stack; phase 1 needs a markdown parser named there. |
| `chrono` | 0.4, `clock` only | The status-bar clock, log timestamps, Codex rollout dates. |
| `unicode-width` | 0.2 | Column widths for wrapping. |
| `anyhow` | 1 | Error context in the binaries. |

