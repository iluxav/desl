# Keys and commands

Generated from the keymap and the command registry in `crates/desl/src`; do not edit by hand.
`DESL_BLESS=1 cargo test -p desl keys_md` rewrites it. The prefix is `ctrl+b`.

## Prompt

| Key | Action |
| --- | --- |
| `enter` | Send the prompt |
| `shift+enter / alt+enter` | Newline in the prompt |
| `ctrl+c` | Interrupt the agent; twice clears the prompt |
| `esc` | Dismiss, stop following, back out |
| `↑` | Scroll the transcript up |
| `↓` | Scroll the transcript down |
| `pgup` | Page up |
| `pgdn` | Page down |
| `/` | Command palette (prompt empty) |
| `@` | File picker into the prompt |
| `ctrl+r` | Prompt history across threads |

## Transcript

| Key | Action |
| --- | --- |
| `j / ↓` | Select the next item |
| `k / ↑` | Select the previous item |
| `g` | Top |
| `G` | Bottom, and follow again |
| `pgup / ctrl+u` | Page up |
| `pgdn / ctrl+d` | Page down |
| `esc` | Follow again, back to the prompt |
| `enter` | Expand or collapse the selected tool call or brief |
| `o` | Open the selected file in $EDITOR |

## Sidebar

| Key | Action |
| --- | --- |
| `j / ↓` | Next thread |
| `k / ↑` | Previous thread |
| `enter` | Open the thread, focus the prompt |
| `esc` | Back to the prompt |

## Approval

| Key | Action |
| --- | --- |
| `y` | Allow this once, or answer yes |
| `n` | Deny, or answer no |
| `a` | Always allow this tool in this thread |

## Workspace

| Key | Action |
| --- | --- |
| `tab` | Next pane |
| `shift+tab` | Previous pane |
| `?` | Keys for the focused element |
| `ctrl+t` | Jump to any thread |
| `ctrl+n` | New thread |
| `ctrl+1…9` | Jump to thread n in the sidebar |
| `ctrl+b d` | Detach; agents keep running |
| `ctrl+b n` | New thread |
| `ctrl+b t` | Jump to any thread |
| `ctrl+b 1…9` | Jump to thread n |
| `ctrl+b tab` | Next pane |
| `ctrl+b b` | Toggle the sidebar |
| `ctrl+b %` | Split pane right (not built yet) |
| `ctrl+b "` | Split pane below (not built yet) |
| `ctrl+b x` | Close pane (not built yet) |
| `ctrl+b z` | Zoom the focused pane (not built yet) |
| `ctrl+b s` | Switch agent (same as /switch) |
| `ctrl+b f` | Fork thread (same as /fork) |
| `ctrl+b ?` | Keys for the focused element |

## Slash commands

| Command | Action |
| --- | --- |
| `/switch [agent]` | Continue this thread with another agent; alone, pick from a list |
| `/fork [agent] [@n]` | New thread from here or from message n |
| `/brief` | Show the handoff brief desl would send right now |
| `/new` | New thread in this project |
| `/threads` | Fuzzy thread picker |
| `/resume` | Pick a past thread to reopen |
| `/rename <name>` | Rename this thread |
| `/model [name] [effort]` | Change this agent's model and effort; alone, pick from a list |
| `/clear` | Start a fresh context on the same thread |
| `/compact` | Ask the agent to compact its context (not built yet) |
| `/export [file]` | Write the thread as markdown |
| `/diff` | Show the worktree diff in a pane (not built yet) |
| `/pr` | Create a pull request from this worktree (not built yet) |
| `/worktree new\|list\|rm` | Manage worktrees (not built yet) |
| `/info` | Agent, model, native session id, token usage |
| `/help` | The help overlay |
| `/quit` | Detach and close the client; agents keep running |

Anything else that starts with `/` is sent to the agent, so its own commands and skills keep working.
