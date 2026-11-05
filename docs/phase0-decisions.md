# Phase 0 decisions

UI decisions the spec does not settle, with the alternatives considered. These are the real output
of phase 0; the winning ones go back into `SPEC.md` at the gate. Newest at the bottom.

## 2026-11-04, day 1 (Prototype A walking skeleton)

1. **Stack: Rust with ratatui 0.30 for both prototypes.** Prototype B must host the agents' real
   TUIs inside a pane, which needs a terminal emulator widget; the ratatui ecosystem has one
   (`tui-term` over `portable-pty`). Ink and Bubble Tea have nothing comparable I would trust;
   Textual is not installed. Alternatives: Ink, Textual, Bubble Tea. The code stays throwaway.
2. **Sidebar width: 20 columns under 120 columns wide, 24 at 120 and above**, divider included.
   Names truncate with `…`. Alternative: proportional width, rejected because the shape must not
   change between 80 and 300 columns.
3. **The WAITING group is always visible, anchored to the bottom of the sidebar, dim when empty.**
   Alternative: appear only when non-empty, rejected because the sidebar would reflow at the exact
   moment the user is being asked for attention.
4. **Thread name: the first word of the first prompt that is not a stopword, path stem for paths,
   at most 12 characters.** "Read src/auth/middleware.ts and explain…" names the thread
   `middleware`. Alternatives: first two words (too long for the sidebar); an agent-written title
   (costs a turn, and ambient AI in the chrome is a non-goal). `/rename` exists for the misses.
5. **Transcript spacing: one blank line before every user and assistant message, none before
   tool lines**, which reproduces the spec drawing exactly.
6. **A dim `▍` marks where text is still arriving.** Removed when the block completes.
   Alternatives: nothing (the reader cannot tell a pause from the end); a blinking cursor
   (animation without information).
7. **Thinking blocks are not rendered.** The status glyph says `working`. Alternative: a dim
   "thinking…" line, rejected for day 1 as chrome; revisit if users ask "is it stuck?".
8. **Tool line: `▸ verb target`, outcome right-aligned: `✓`, or `✕` plus the first error line.**
   Verbs: `read`, `edit` (Edit, Write, MultiEdit), `run` (Bash), `search` (Grep, Glob, WebSearch),
   `fetch`, `agent`, otherwise the tool's lowercase name. Long targets truncate in the middle so
   the file name survives.
9. **The `↓ n new` pill counts hidden lines, bottom-right of the transcript**, inverted accent.
   Alternative: count messages, rejected because a single streaming message would never change
   the count.
10. **Scrolling to the bottom resumes following**, as in `less` and tmux copy mode; `G` and `esc`
    also resume. New output never moves the view while the user is above the bottom.
11. **Prose wraps at 100 columns at most**, left-aligned, on wide terminals. Tool lines and the
    outcome column follow the same cap. Alternative: full width, rejected for readability at 200
    columns.
12. **The prompt box has one column of margin on each side; its border is always muted; focus
    shows through the cursor and the `›` colour.** Alternative: accent border when focused,
    rejected as louder than the agent's text.
13. **A pending prefix shows as an inverted `ctrl+b` in the status bar hint; it has no timeout
    and `esc` cancels it**, like tmux (with the prefix-highlight plugin).
14. **`ctrl+c`: interrupts when the agent is working; a second press within 1.5 s, or any press
    when idle, clears the prompt; with nothing to do it explains itself in the status bar**
    ("Nothing to interrupt. To leave, type /quit."). It never quits.
15. **The `?` overlay lists the focused element's keys, then approval keys when one is pending,
    then workspace keys.** Unbuilt keys are listed dim with "· not yet" so nothing in the keymap
    is hidden. It scrolls with `j`/`k` when the terminal is short, with a `↓ n more` footer.
16. **Outside a git repository the status bar shows `no git`, muted.** The branch slot is never
    empty because the spec says the branch is always visible.
17. **Approval keys `y`/`n`/`a` act only while the prompt is empty**; with text in the prompt they
    type. Alternative: a focusable strip, rejected as a modal in disguise.
18. **Queued prompts stay in the transcript under their text with a muted `⏳ queued`.**
19. **An empty thread shows one muted line: "Type a prompt and press enter. ? shows every key."**
    Alternative: nothing, rejected for moment 1 (a new user reads nothing else).
20. **On quit the prototype prints each thread's native session id and the `claude --resume`
    command**, so nothing is lost even without the daemon.
21. **New threads run Claude in `acceptEdits`**, the spec's `edits` default: reads and edits
    allowed, commands ask.
22. **Error wording in the prototype**: every error says what happened and what to do next, e.g.
    "Claude exited with 1 mid-turn. It said: …. Send your prompt again to resume the session."

## 2026-11-04, day 2 (threads, Codex, switch)

23. **Codex runs one `codex exec` process per turn and its text arrives whole.** Exec mode has no
    token deltas and no approval channel (see `docs/adapters.md`). The thread shows `working` with
    no streaming cursor until the message lands. Alternative: `codex app-server` (deltas and
    per-call approvals) deferred to phase 1 so that phase 0 tests the spec's stated path first.
24. **The switch confirmation is a one-line strip above the prompt with `[y]es [n]o`**, the same
    place and shape as an approval, never a modal:
    `Handing 1 turn to codex as a brief (~170 tokens). Continue?`. A switch back with nothing new to
    hand over happens silently with just the divider. `esc` answers no.
25. **The divider is a full-width thin rule with the label at the left**:
    `── switched to codex ───────`. Alternative: the spec's bare `── switched to codex ──`, which
    read as a stray line rather than a boundary at 100 columns.
26. **The brief rides inside the next prompt, and the prompt is its "Next step".** Nothing else is
    appended, so the receiving agent answers the question rather than the brief. The transcript
    records what went out as a collapsed line under the prompt, `▸ brief sent to codex ~164 tokens`,
    expandable with `enter`. `/brief` shows the brief desl would send right now, expanded.
27. **Token estimates are bytes divided by four, always shown with `~`.** No tokenizer in phase 0.
28. **`seen_up_to` is advanced twice**: when a prompt is dispatched to an agent (it will see
    everything up to and including that prompt) and when its turn completes. A switch back sends a
    delta brief of only the items in between; on the test run that was 129 tokens.
29. **Switching while an agent is working is refused with a hint** ("claude is still working. Wait
    for it to finish or press ctrl+c, then switch."). Alternative: interrupt and switch in one
    step, rejected as surprising.
30. **In the transcript, `j`/`k` select items (lazygit) and `enter` expands tool calls and briefs;
    `↑`/`↓` from the prompt still scroll by line.** The selection shows as a `┃` gutter in the
    accent colour down the item's lines. Expanded detail is a `│`-ruled block of at most 40 lines
    plus `… n more lines`. Selecting an item that does not expand says so in the status bar.
31. **`tab` cycles prompt → transcript → sidebar.** In the sidebar `j`/`k` change the current
    thread immediately (lazygit panels) and the current row inverts while focused; `enter` or `esc`
    returns to the prompt.
32. **Thread picker (`ctrl+t`, `/threads`) is an fzf-style overlay**: a query line, subsequence
    matching with bonuses for word starts and runs, `enter` jumps. The same widget will serve the
    command palette and the file picker.
33. **`ctrl+n` and `/new` open a thread on Claude** (the default agent, no worktree in phase 0),
    named `new` until its first prompt. `ctrl+1…9` jump by sidebar position where the terminal can
    report ctrl with a digit.
34. **A stale agent process that speaks after a switch is recorded as a muted note**
    `claude (late): …` rather than shown as the current agent's reply.
35. **On quit, every binding is printed with its resume command** (`claude --resume <id>`,
    `codex resume <id>`), one line per agent per thread.
36. **`o` (open in `$EDITOR`) stays unbuilt in Prototype A.** The input thread owns the terminal's
    stdin, so a child editor would fight it for keystrokes. Phase 1's client can suspend properly.

## 2026-11-04, day 2 (detach and reattach)

37. **Prototype A is a thin client over a per-project host.** The host owns the agent processes
    and an append-only log of entries (prompt, dispatched, agent event, decision, interrupted,
    switched, failed, note); the client renders a replay of that log and turns keys into
    commands. Both run the same projection code (`workspace.rs`), so they never disagree about
    the screen. This is not the phase 1 daemon, store or protocol; it exists so moments 7 and 9
    can be felt. Alternative: hide the whole TUI inside a tmux session, rejected because it
    cannot restore scroll and focus and would make Prototype A depend on tmux.
38. **The client starts the host on demand** with `setsid`, stdio to a log file, and waits up to
    five seconds for its socket at `$XDG_RUNTIME_DIR/desl-proto-a/<project-key>.sock`. The log
    lives at `$XDG_STATE_HOME/desl-proto-a/<project-key>.jsonl`. One host per project directory.
39. **Quitting detaches.** `/quit`, `prefix d` and closing the terminal all leave the host and
    its agents running; the farewell line says how many threads were kept and how many are still
    working, and names `proto-a stop`. There is no confirmation: leaving is always safe.
40. **Reattach restores the exact view**: current thread, sidebar visibility, focus, and each
    thread's scroll offset and follow state, saved by the client on detach and handed back with
    the snapshot. The `↓ n new` pill came back with the same count in the test.
41. **A host restart replays its log.** Threads, items and bindings come back; a thread that was
    busy gets a note and goes idle; its next prompt resumes the native session (verified with
    Claude recalling the previous turn). Nothing is lost short of deleting the log file.
42. **`proto-a status` prints the sidebar as text** (`● t-8d21 facts working claude`), the
    spec's `desl status`, and works while a client is attached.
43. **Stopping the host tells attached clients why they are leaving**: "The desl host was
    stopped. Its log is kept, so running the prototype here again restores every thread and
    resumes the native sessions."

## 2026-11-04, day 2 (prompt affordances)

44. **`/` on an empty prompt opens the command palette; `/` with text in the prompt types a slash.**
    The palette lists every spec command with its arguments and description, unbuilt ones with
    "· not yet". `enter` runs a command that takes no argument, or leaves `/switch ` in the prompt
    for one that does. A full command line typed into the palette (`/switch codex`) runs as typed,
    so Claude Code muscle memory keeps working. Unknown slash commands still go to the agent.
45. **`@` opens a file picker over `git ls-files` plus untracked files** (capped at 5000), file name
    first and directory as the hint; `enter` inserts `@path ` at the cursor. Outside a git repo it
    says so and types a literal `@`.
46. **`ctrl+r` lists every prompt across threads, newest first, deduplicated**, with the thread id
    as the hint; `enter` replaces the prompt with it.
47. **Markdown rendering is minimal and quiet**: bold, italics and code spans inline; headings in
    bold with no `#`; bullets as `•`; numbered lists as written; quotes with a `▎` bar; fences as a
    dotted rule carrying the language (`┄┄ rust ┄┄┄`) with code lines behind a `│` bar, unwrapped.
    No syntax highlighting in phase 0. Emphasis state carries across wrapped lines so a bold
    phrase broken by the wrap stays bold.
48. **Picker columns**: descriptions align after the longest visible label; both truncate with `…`
    so a row never wraps.

## 2026-11-04, day 2 (Prototype B, first panes)

49. **Prototype B shares every chrome module with A** (frame, sidebar, status bar, help, pickers,
    keymap, theme) through one view struct, so the two cannot drift apart visually. Only the main
    area differs: A renders a transcript, B paints the agent's own screen.
50. **B hosts the agents' TUIs in PTYs parsed by `vt100` and painted cell by cell**; no third-party
    terminal widget, to stay independent of ratatui version churn. 10,000 lines of scrollback per
    pane, because Claude Code's interface relies on the terminal's scrollback.
51. **In B, every unprefixed key belongs to the agent.** The spec's unprefixed workspace keys
    (`ctrl+n`, `ctrl+t`, `ctrl+1…9`, `tab`, `?`) cannot be stolen from a TUI that uses them, so B
    adds prefixed twins: `ctrl+b n`, `ctrl+b t`, `ctrl+b 1…9`, `ctrl+b tab`, `ctrl+b [` for
    scrollback. A accepts the same twins, so both prototypes answer both forms. B's help lists only
    prefixed workspace keys plus one row, "any key: goes to the agent's own interface". This is the
    first structural difference the hallway test should notice; it is written down as a finding.
52. **Status in B is a heuristic, as in Herdr**: `waiting` when the bottom 14 rows of the pane
    contain one of a small set of prompt phrases (Claude Code's "Do you want to proceed", its trust
    dialog's "Enter to confirm", Codex's approval wording); `working` when the pane produced output
    in the last 1.5 s that was not an echo of the user's own typing (output within 0.5 s of a
    keystroke does not count); `idle` otherwise; `failed` when the process exited non-zero. The bell
    rings on the transition to `waiting` only.
53. **B names a thread from the first line the user typed into the agent** (printable keys up to
    Enter, with backspaces honoured), using the same rule as A.
54. **A new thread in B asks which agent** through the picker (`claude` or `codex`), since B has no
    prompt of its own for a `/new --agent` form.
55. **`ctrl+b [` enters scroll mode**: `j`/`k`/`pgup`/`pgdn`/`g` move through the pane's
    scrollback, the title says "scrolling, esc returns", the `↓ n new` pill shows the lines below,
    and `esc`, `q` or `G` return to the live screen. Any key typed to the agent returns to live first.
56. **Detach is not in B yet.** `ctrl+b d` leaves honestly: "Detach is not built in prototype B
    yet, so leaving stopped the agents", with the native resume commands named.
57. **Finding, not a decision**: Claude Code's interactive trust dialog appears on first use of a
    directory in B but never in A, because `-p` skips it. Both are the agent's own behaviour; the
    hallway task script must either pre-trust the demo repository or treat the dialog as part of
    moment 1.

## 2026-11-04, day 3 (Prototype B continuity, B host, launcher)

58. **B's continuity source is the agents' own session files, tailed.** Claude Code's
    `~/.claude/projects/<cwd with / as ->/<session>.jsonl` and Codex's
    `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` are parsed into the same thread items A builds
    from its streams, so the same projector produces the brief. A pane claims the newest file
    written after it started (Codex files are also matched on their recorded cwd). Replay tests run
    over real files in `phase0/fixtures/transcripts/`.
59. **Status in B now comes from the session file first.** A user line or Codex's `task_started`
    means `working`; Claude's `end_turn` or Codex's `task_complete` means `idle`. The screen text
    still decides `waiting`, and output activity is only the fallback before a session file exists.
    A pane that is still booting, or waiting to receive a pasted brief, is never shown as working.
60. **desl's slash commands typed into the agent's own input are taken back at Enter.** B tracks the
    printable keys since the last Enter; if the line is `/switch`, `/brief`, `/info`, `/rename`,
    `/threads`, `/new`, `/help` or `/quit`, it erases the line with backspaces and runs the command
    itself. Any other slash command goes to the agent untouched. Consequence: the agent's own
    `/help` and `/quit` are unreachable in B; the agent's `/exit` still works.
61. **The switch confirmation in B is the same one-line strip, drawn over the pane's last row**,
    so the agent is never resized. `y` continues, `n` or `esc` stays.
62. **In B the brief is pasted into the other agent's prompt and left there**, once its interface
    is quiet and not asking anything; the user types the question after it and presses enter, so
    one turn carries both, as in A. Claude Code shows the paste as a chip
    (`[Pasted text #1 +9 lines]`); **Codex shows it expanded in its input**, which is a wall of text
    for a first-time user. The brief's "Next step" is the previous agent's last paragraph, because
    B cannot know the prompt in advance.
63. **B cannot draw the `── switched to codex ──` divider inside the agent's interface.** The title
    row and status bar change agent and a status-bar notice says what happened. This is a visible
    difference from A that the spec's "same divider on switch" cannot remove.
64. **Each agent that has touched a thread keeps its own interface alive.** Switching shows the
    other one; switching back shows the first with only a delta pasted.
65. **Approvals in B**: for Claude Code's permission dialog desl shows its strip
    (`claude is asking for approval  [y]es [n]o [a]lways`) and maps `y` to Enter on the default
    "Yes", `n` to Esc and `a` to the second option. Codex's prompt answers `y` and `n` itself, so
    desl only shows a strip saying where to answer. The strip cannot show the command, because
    reading it out of the agent's screen reliably is not possible.
66. **`/brief` and `/info` open a scrollable popup in B**, since B has no transcript of its own to
    put them in.
67. **B has its own host**, mirroring A's shape: it owns the PTYs, the session-file tails, status
    and the switch logic; the client keeps a `vt100` screen per pane, fed by the host's byte
    stream, and sends keys back. On attach the host replays up to 2 MB of each pane's output so
    scrollback returns too, and resizes the panes to the attaching terminal. `ctrl+b d` and `/quit`
    detach; `proto-b status` and `proto-b stop` exist.
68. **Stopping B's host ends its agents**, unlike A where sessions resume from the log. The farewell
    says so and names `claude --resume` and `codex resume`. B's thread list is not restored after a
    host restart; that is the honest limit of the native approach in a prototype.
69. **The blind launcher is `desl-proto`.** With no session in the directory it picks the prototype
    with fewer sessions so far (a coin flip on a tie), logs time, directory, label and how
    (`picked`, `rejoined`, `forced`) to `$XDG_STATE_HOME/desl-proto/launches.jsonl`, and becomes
    that prototype. A directory with a running session is rejoined, never re-rolled.
    `desl-proto log` prints the table for the observer; `desl-proto a|b` forces one;
    `desl-proto status|stop` go to whichever prototype holds the session. The user is never told
    what A and B are.

## Day 4: Prototype B status fixes (found by the spec author's own first session)

70. **A thread's status in B can no longer stick on `working` or `waiting`**, which made `/switch`
    answer "claude is still working" forever. Three causes. Claude Code's own `/model` was read as
    a prompt whose turn never ended (seen in the spec author's session). An answer ending in "Do
    you want to proceed?" was read as a permission dialog (reproduced on the old build). Esc left
    a turn open (read from the code). Now a tagged user line opens a turn only when Claude Code
    marks it as answered, the interruption marker and `turn_duration` end one, and dialog words
    count only in the middle of a turn and only with the dialog's first option on screen. All
    three are covered by `phase0/tools/e2e_b_status.sh` and `e2e_b_prose.sh` against the real
    agents, except `/model` itself, which would change the user's default model and is covered by
    a parser test. Alternatives: take status from the screen alone (the heuristic decision 59
    replaced); let `/switch` proceed while working (hands over half a turn).
71. **A screen that stands still for 5 seconds overrules a `working` status.** Both interfaces
    animate while they work (checked over a 22-second foreground command), so stillness means the
    turn ended in a way the session file did not record. Alternative: trust the file only, which
    is what stuck.
72. **B starts Claude Code with `--session-id`**, so the session file's name is known up front.
    Before, B took the newest session file in the project directory and could follow another
    Claude Code working in the same directory, handing its conversation over as the brief. Codex
    has no such flag; its rollout is matched by directory and must have begun after the pane did.
73. **desl's approval strip in B covers Claude Code's file dialogs too** ("Do you want to create
    …?"), not only commands, and `y`, `n`, `a` are mapped only while the thread is `waiting`.
    Before, the same words in an answer would have turned every typed y, n and a into Enter, Esc
    and Down-Enter. `a` takes the dialog's second option, whose meaning differs by dialog (never
    ask again for this command; accept edits for the session). Finding: B cannot say which.
74. **B's host log lists every status change with its cause** (session file, or the screen text
    that read as a question), in `$XDG_STATE_HOME/desl-proto-b/<project>.host.log`. It is for the
    observer; nothing in the interface points to it.
75. **Known limit, not fixed:** a turn that Claude Code starts without a prompt and without tools
    (its answer to bash mode output) shows `working` only from the line it marks as answered; a
    native command that puts the agent to work some other way shows `idle` until its first
    message is written.
76. **Fixed: every Codex turn after the first failed in A** ("codex exited with 2 mid-turn"). The
    sandbox flag sat after `resume`, where Codex refuses it; the command line had been written
    down as verified but only its parts were, and no test sent Codex two prompts in a row.
    `phase0/tools/e2e_codex_twice.sh` now does. The failure message now quotes the agent's own
    error line instead of its "try --help" hint.
77. **Finding for the spec, not changed: the brief makes the new agent own the old agent's
    words.** Asked "what model are you" after a switch, Codex answered and added "The earlier
    claim that I was Claude Opus was incorrect". The brief says "Treat this as your own
    history", as `SPEC.md` prescribes, so Claude's statement about itself became Codex's. This
    is a seam in moment 3. Proposed change to the brief's opening lines: name the agent that
    wrote the earlier turns and say that anything they state about their own identity, model or
    tools describes that agent. Alternative: label each assistant turn in "Recent conversation"
    with its agent.
78. **A repository with no commits shows its branch name**, not `no git`.

## Day 4: Prototype A, after the spec author's first sessions

79. **A working line under the last message** (supersedes decision 7):
    `⠹ working 12s · ctrl+c to interrupt`, a spinner in the accent colour, the status word, the
    time since the turn began, and the one key that matters. The author's words after trying A:
    "it doesn't show any indication that it's working or thinking". The status glyph in the bar
    was not enough, least of all on Codex, whose headless mode does not stream. Borrowed from
    Claude Code and Codex, which both show a spinner, elapsed time and the interrupt key under
    the conversation. Alternatives: the line in the strip row above the prompt (far from where
    the eye is); the word "thinking" (not a desl status, and desl cannot tell thinking from
    waiting on the network).
80. **Two lines stay free under the last item at all times.** The working line fills the second
    one, which is where the answer's first line will appear, and when the turn ends nothing
    above moves (moment 4). The timer is the client's: after a reattach in mid-turn it counts
    from the reattach, because the log carries no times.
81. **Markdown is parsed by `pulldown-cmark`** (supersedes decision 47). The author found the
    hand-written renderer poor; their answers held links and lists it showed raw. Now: headings
    without `#` (the first level in the accent colour); bold, italics, strikethrough; code spans
    in the accent colour; nested lists with `•`, `◦`, `▪` and hanging wraps; task boxes; quotes
    behind a `▎` bar; tables in light box lines, columns shrunk and cells wrapped to fit; fenced
    code as before but wrapped instead of cut, so no character is lost.
82. **A link shows its text underlined with a number, and the addresses are listed under the
    answer** (`the guide[1]` … `[1] https://…`), as lynx and mutt do. An address in the middle of
    a sentence broke the line wherever it fell. Alternative: `text (url)` inline, as glow does.
    Terminal hyperlinks (OSC 8) would hide the address entirely; ratatui has no support for them.
83. **Blank lines follow the source.** A list written straight under "Sources:" stays tight, and a
    single newline stays a line break, since agents write short lines on purpose. A heading
    always gets a blank line above it.
84. **Code is coloured lightly**: keywords in the accent colour, strings green, comments muted
    italics, diffs by their `+` and `-`. One small lexer for every language, no grammar, plain
    for a language it does not know. The spec asks for syntax-highlighted code; a real
    highlighter is a phase 1 choice. Green is the one hue added to the palette.
85. **An edit shows its diff under its line as it lands** (accepted by the author on 2026-11-04;
    the Transcript pane paragraph of `SPEC.md` now says so).
    The author on 2026-11-04: Prototype A "looks good", the native view "obviously looks
    better". Side by side, the largest difference was that both native interfaces show what an
    edit changed, while A showed `edit src/auth/tokens.ts ✓` and, on `enter`, the agent's
    sentence "The file … has been updated". Now the first 12 rows of the diff sit under the edit;
    `enter` on the line shows all of it when there is more, and closes it otherwise. Reads,
    commands and searches stay one line until asked. Alternatives: closed until `enter`, as the
    spec was written (the diff is two keys away from every edit); the whole diff always, as the
    native interfaces do (a large edit pushes the conversation off the screen). The spec said
    "Tool calls are one collapsed line each … `enter` on a line expands its full output in
    place"; it now adds "an edit also shows the first rows of its diff under the line" and
    "or collapses it again". The walkthrough's daily-loop sentence ("tool calls collapse to one
    line each … and expand on `enter`") is unchanged and still true of the line itself.
86. **A diff has line numbers and signs in a gutter; added lines are green, removed red, context
    dim.** Hunks are separated by `⋯`. A long line wraps at a space, so the changed end of a
    line is never cut. Removed lines carry the old file's number, the rest the new file's.
    Alternatives: coloured backgrounds as Claude Code draws them (they depend on the terminal
    theme and fail under NO_COLOR, where the sign alone still works); syntax colours inside
    diffs (phase 1, with the real highlighter).
87. **The outcome on a tool line says what happened** (extends decision 8): `+2 −1` for an edit, as git and lazygit
    count; `18 lines` for a read; `✓` for a command; `✕` and the first line of the failure,
    which for a Codex command is `exit 2`. Before, everything that worked showed `✓`.
88. **A tool call still running shows the spinner where its outcome will land**, in step with
    the working line, and after `allowed` when the user approved it. It names what the agent is
    waiting on when several lines are on screen.
89. **One column stays free at the right edge of the transcript.** Prose, code fences and
    dividers used to touch the pane border.
90. **Codex diffs come from the file itself.** `codex exec --json` names a changed file and never
    says what changed, so the adapter keeps the file's text from the moment the change started
    and diffs it against the file when the change completes. This is the spec's own fallback
    ("derived from tool results or git"). If Codex ever applies a change before announcing it,
    the edit shows no diff rather than a wrong one.
91. **Fixed: on Codex, tool calls disappeared from the second turn on.** Codex numbers its items
    from `item_0` in every turn; the second turn's `item_1` was taken for the first turn's and
    dropped. Tool ids now carry the turn. Found while checking decision 88, never reported.
92. **An expanded read shows the file's lines with aligned numbers**, taken from the structured
    result instead of the `cat -n` text, whose tabs a terminal cell cannot hold (`1import`).
    Tabs anywhere in tool output become spaces to the next multiple of four.
93. **The gate was decided by the author alone, and Prototype A won** (2026-11-04). No hallway
    sessions will be run; in the author's words, "no people will test it only myself" and "i
    think we should go with A". The basis is their own use of both prototypes the same day: A
    "looks good"; the native view "obviously looks better but it misses the point"; switching in
    B "shows that it pastes the text from claude". Not measured, and so not claimed: the
    per-moment medians the spec's gate asks for. **Needs the spec to move**: the phase 0 gate
    in the roadmap, and every later place the hallway protocol gates a release. Open until the
    author says: whether their own testing replaces the protocol in those later gates too.
