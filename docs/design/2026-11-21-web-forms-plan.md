# desl web forms, milestones 1 and 2: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reshape desl web from a copy of the TUI into a conversation page, then let Claude Code, Codex and Copilot push forms to it through an MCP tool and get the user's answer as their next prompt.

**Architecture:** A shared module `desl_proto::form` defines the form language, checks forms and answers, and words the prompts. The daemon keeps forms in a new store table and in two IR events, sets `waiting` while a form is open, and delivers answers through the existing `prompt` path. `desl mcp` is a stdio MCP server each agent starts from launch flags desl adds; it calls the daemon like any client. The page draws forms inside the conversation from the same JSON.

**Tech Stack:** Rust 2024 workspace (serde, serde_json, rusqlite, clap, tokio and axum in `desl-web` only), plain JavaScript modules with no build step.

**Spec:** `docs/design/2026-11-21-web-forms.md` (approved 2026-11-21). Milestones 3 to 6 of it get their own plans.

## Global Constraints

- Read `AGENTS.md`, `SPEC.md` and `web-spec.md` first; they apply to every task.
- `make check` (clippy `-D warnings`, `fmt --check`, every test) passes after every task.
- `unsafe_code = "forbid"`; no new crate dependency in any task.
- Commits carry no `Co-Authored-By` line. Commit and push each task to `main`.
- Every error a user or agent sees is a complete sentence that names what was wrong and what to do.
- The page sets agent-written text with `textContent` or the existing DOM-only Markdown renderer, never `innerHTML`.
- Test desl in isolation only: `DESL_HOME` in the scratchpad, `DESL_SOCKET=$XDG_RUNTIME_DIR/desl-vf.sock`, `tmux -L desl-vf`; never the user's daemon, never `make install` from a test.
- Never click inside an agent's screen in a browser test; never leave a test thread in `yolo`.
- Form limits: at most 30 widgets, at most 256 KB as JSON. Tool description and input schema together at most 1,500 tokens, checked as at most 6,000 bytes.
- Budgets: `show_form` to the card on an open page < 300 ms; Send to the prompt typed into an idle agent < 1 s.
- Answer prompt, exactly: `desl: answer to f-4b1e "Which milestone next?"` then a fenced `json` block holding the answer.
- Dismiss note, exactly: `desl: the user dismissed f-4b1e "Which milestone next?" without answering.`
- Tool result, exactly: `Shown to the user as f-4b1e. End your turn now; their answer arrives as your next prompt.`

## Review Focus

1. **An answer sent while the agent works** (the user answers one form while the agent is busy on something else) must be queued like any prompt, not dropped or typed into a busy screen. Test: Task 6, `an_answer_while_the_agent_works_is_queued`.
2. **A form open when the daemon restarts, with a half-done draft** must come back open, with its draft, and the thread still `waiting`. Test: Task 6, `an_open_form_and_its_draft_survive_a_restart`.
3. **Two tabs, one form:** the second Send on a form already answered must be refused with a sentence, and the agent must get one prompt, not two. Test: Task 6, `a_form_is_answered_once`.
4. **Agent text that looks like HTML** in a title, label, option or `markdown` widget must show as text. Test: Task 9, the page check with `<img src=x onerror=alert(1)>` as a title.
5. **A fork of a thread with an open form:** the fork shows the card read-only and has no open form; answering it from the parent still reaches the parent. Test: Task 6, `a_fork_copies_no_open_form`.

---

## Milestone 1: the page reshaped

### Task 1: The daemon forgets the page's screen

The page will no longer attach to agents' terminals, so the size rule for pages and the dialog text for cards go.

**Files:**
- Modify: `crates/desl-daemon/src/engine.rs` (`Client`, `Asked`, `stamp`, `attach`, `fit`, `Msg::Disconnected`, the terminal-answer rule, `check_asking`, `deliver`, `trimmed`)
- Modify: `crates/desl-daemon/src/engine/tests.rs` (remove `a_tui_keeps_its_size_and_a_page_draws_at_it`, `what_a_screen_asks_is_its_rows_without_the_blank_ones_around_them`)
- Modify: `crates/desl-proto/src/lib.rs` (`ServerFrame::PtySize`, `Reply::Attached.rows/cols`)
- Modify: `crates/desl-ir/src/lib.rs` (`Thread.asking`), `crates/desl-daemon/src/store.rs`, `crates/desl-proto/src/view.rs` (`ThreadRow.asking` and its test lines)
- Modify: `docs/protocol.md` (`attached` rows/cols, `pty_size`, `thread.asking`)

**Interfaces:**
- Produces: `Client.shows: HashSet<String>` as before commit `4798f4a`; `Reply::Attached { thread_id, agent, replay }`; no `ServerFrame::PtySize`; no `Thread.asking`, no `ThreadRow.asking`.

- [ ] **Step 1:** Revert the engine and protocol parts of `4798f4a` and `0a4a0e6` by hand (later commits touch the same lines, so `git revert` will not apply cleanly): attach resizes the terminal to what the client asks, as before; the daemon answers an agent's terminal questions when no client shows it; `hello` no longer marks a client `web`. Keep `check_asking` and `deliver` setting `waiting` from the screen exactly as before `0a4a0e6`.
- [ ] **Step 2:** Run `cargo build 2>&1 | grep -E "^(error|warning)"`. Expected: nothing.
- [ ] **Step 3:** Run `make check`. Expected: passes; the two removed tests are gone, no other test changes.
- [ ] **Step 4:** Remove the same rows from `docs/protocol.md`; add a line to `docs/CHANGELOG.md` only if the removed items were listed there (they were added unreleased: edit the existing desl web entries instead of adding a removal).
- [ ] **Step 5:** Commit: `desl web: the daemon no longer sizes terminals for pages`.

### Task 2: The page is the conversation

**Files:**
- Modify: `crates/desl-web/page/index.html`, `crates/desl-web/page/app.js`, `crates/desl-web/page/app.css`
- Delete: `crates/desl-web/page/vendor/` (all six files)
- Modify: `crates/desl-web/src/lib.rs` (`FILES`, the CSP string)
- Modify: `crates/desl-web/src/tests.rs`
- Modify: `web-spec.md` (Scope, Layout, "The agent's screen on the page", Architecture, Milestones), `SPEC.md` (the three `desl web` budget rows for keys, output and memory; the non-goal sentence), `README.md` ("In a browser"), `docs/CHANGELOG.md`

**Interfaces:**
- Consumes: the bridge's `view` and `conversation` messages, unchanged.
- Produces (for Task 9): in `app.js`, `drawSide(cur)` draws projects and their agent threads; `drawConversation(toEnd)` draws the middle; `drawWaiting()` draws the right column from `state.view.waiting`; the right column's container is `#waitlist`.

- [ ] **Step 1: Write the failing test** in `crates/desl-web/src/tests.rs`:

```rust
#[test]
fn the_page_serves_no_terminal_and_allows_nothing_inline() {
    assert!(FILES.iter().all(|(path, _, _)| !path.starts_with("/vendor/")), "xterm.js is gone");
    let csp = csp(7428);
    assert!(!csp.contains("unsafe-inline"), "{csp}");
    assert!(csp.starts_with("default-src 'self';"), "{csp}");
}
```

(`csp(port: u16) -> String` is the function that builds the header today; extract it from the response code if it is inline.)
- [ ] **Step 2:** Run `cargo test -p desl-web the_page_serves_no_terminal`. Expected: FAIL.
- [ ] **Step 3:** Remove the vendor files and their `FILES` rows; drop `'unsafe-inline'` from `style-src` (setting `element.style` from script is not affected by CSP). In `index.html` remove the tabs, the screen container and `xterm.css`. In `app.js` remove the `screen` object and everything only it uses (`play`, `marks`, `attach`, `drawn`, `sized`, `replaced`, `ended`, `REPLIES`, `typed`, `refit`, `setupScreen`, `showTab`, `dialog`, `openScreen`), and the tab state in `sessionStorage`. The middle column always shows the conversation; the prompt box always shows under it; Stop stays in the header. The left column draws each project with its agent threads, one line each (status mark, title, age), with no subagent rows. The right column, "Waiting", lists each thread in `view.waiting` as one line: title, project, and "asks in its own screen: answer in the terminal".
- [ ] **Step 4:** Run `cargo test -p desl-web` and `node --check crates/desl-web/page/app.js`. Expected: PASS, no output from node.
- [ ] **Step 5:** Check it live: an isolated daemon with a Claude and a Codex thread, `target/debug/desl web --port 7499`, the page in Playwright at 1300×800 and at 390×800. Expected: three columns at desktop width, the conversation of the selected thread, no tab bar, no console errors; at phone width the columns stack with no horizontal scroll. Delete `.playwright-mcp/` afterwards.
- [ ] **Step 6:** Update the docs listed above: the page is the conversation; the screen, the tabs and the waiting cards are gone; the binary-size note in web-spec's Architecture gets the new size from `ls -l target/release/desl` after `cargo build --release`.
- [ ] **Step 7:** `make check`, then commit: `desl web: the page is the conversation, not a copy of the TUI`.

---

## Milestone 2: forms

### Task 3: The form language

**Files:**
- Create: `crates/desl-proto/src/form.rs`
- Modify: `crates/desl-proto/src/lib.rs` (`pub mod form;`)

**Interfaces:**
- Produces:
  - `pub struct Form { pub title: String, pub widgets: Vec<Widget>, pub send_label: Option<String> }` (serde, `deny_unknown_fields`)
  - `pub enum Widget` tagged by `kind`, M2 kinds only: `Markdown { id, text }`, `Choice { id, options: Vec<Opt>, multi: bool, other: bool, optional: bool }`, `Checklist { id, items: Vec<Step>, reorder: bool, optional: bool }`, `Text { id, label, lines: Option<u8>, placeholder: Option<String>, value: Option<String>, optional: bool }`, `Number { id, label, min: f64, max: f64, step: Option<f64>, unit: Option<String>, value: Option<f64>, optional: bool }`, `Toggle { id, label, value: bool, optional: bool }`, `Select { id, label, options: Vec<String>, value: Option<String>, optional: bool }`; `Opt { label, detail: Option<String> }`, `Step { id, label, detail: Option<String>, checked: bool }` (`checked` defaults to true)
  - `pub fn check(form: &serde_json::Value) -> Result<Form, String>`
  - `pub fn check_answer(form: &Form, answer: &serde_json::Value) -> Result<(), String>`
  - `pub fn answer_prompt(form_id: &str, title: &str, answer: &serde_json::Value) -> String`
  - `pub fn dismissed_note(form_id: &str, title: &str) -> String`
  - `pub fn shown(form_id: &str) -> String` (the tool result)
  - `pub fn is_form_prompt(text: &str) -> bool` (true for an answer prompt)
  - `pub fn strip_notes(text: &str) -> &str` (a prompt without the dismiss notes before it)
  - `pub const TOOL_DESCRIPTION: &str`, `pub fn input_schema() -> serde_json::Value`, `pub const CLOSE_DESCRIPTION: &str`, `pub fn close_schema() -> serde_json::Value`

- [ ] **Step 1: Write the failing tests** in `form.rs`:

```rust
#[test]
fn a_form_is_checked_widget_by_widget() {
    let ok = json!({"title": "Which milestone next?", "widgets": [
        {"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5", "detail": "the biggest"}]},
        {"kind": "text", "id": "note", "label": "Note", "optional": true}]});
    assert_eq!(check(&ok).unwrap().widgets.len(), 2);
    let twice = json!({"title": "t", "widgets": [{"kind": "toggle", "id": "a", "label": "A"}, {"kind": "toggle", "id": "a", "label": "B"}]});
    assert_eq!(check(&twice).unwrap_err(), "Widget 2 has the id \"a\", which widget 1 has already. Give each widget its own id.");
    let unknown = json!({"title": "t", "widgets": [{"kind": "slider", "id": "a"}]});
    assert!(check(&unknown).unwrap_err().starts_with("Widget 1 has the kind \"slider\", which desl does not know. Use one of: markdown, choice, checklist, text, number, toggle, select."));
    let many = json!({"title": "t", "widgets": (0..31).map(|i| json!({"kind": "toggle", "id": format!("w{i}"), "label": "x"})).collect::<Vec<_>>()});
    assert_eq!(check(&many).unwrap_err(), "The form has 31 widgets; desl shows at most 30. Split it into two forms.");
    let range = json!({"title": "t", "widgets": [{"kind": "number", "id": "n", "label": "N", "min": 5, "max": 1}]});
    assert_eq!(check(&range).unwrap_err(), "Widget 1 (\"n\") has min 5 above max 1. Swap them.");
    assert!(check(&json!({"title": "", "widgets": []})).unwrap_err().contains("title"));
}

#[test]
fn an_answer_is_checked_against_its_form() {
    let f = check(&json!({"title": "t", "widgets": [
        {"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5"}]},
        {"kind": "markdown", "id": "why", "text": "Pick one."},
        {"kind": "text", "id": "note", "label": "Note", "optional": true}]})).unwrap();
    assert!(check_answer(&f, &json!({"next": "M5"})).is_ok(), "an optional widget may be left out, a markdown one has no answer");
    assert_eq!(check_answer(&f, &json!({})).unwrap_err(), "The answer has nothing for \"next\", which is not optional.");
    assert_eq!(check_answer(&f, &json!({"next": "M6"})).unwrap_err(), "\"M6\" is not one of the options of \"next\".");
    assert_eq!(check_answer(&f, &json!({"next": "M5", "x": 1})).unwrap_err(), "The answer has \"x\", which is no widget of the form.");
}

#[test]
fn the_prompts_are_worded_once() {
    let p = answer_prompt("f-4b1e", "Which milestone next?", &json!({"next": "M5"}));
    assert_eq!(p, "desl: answer to f-4b1e \"Which milestone next?\"\n```json\n{\"next\":\"M5\"}\n```");
    assert!(is_form_prompt(&p));
    let note = dismissed_note("f-4b1e", "Which milestone next?");
    assert_eq!(note, "desl: the user dismissed f-4b1e \"Which milestone next?\" without answering.");
    assert_eq!(strip_notes(&format!("{note}\n\nand now the tests")), "and now the tests");
    assert_eq!(shown("f-4b1e"), "Shown to the user as f-4b1e. End your turn now; their answer arrives as your next prompt.");
}

#[test]
fn the_tool_costs_at_most_1500_tokens_a_turn() {
    let size = TOOL_DESCRIPTION.len() + input_schema().to_string().len() + CLOSE_DESCRIPTION.len() + close_schema().to_string().len();
    assert!(size <= 6_000, "{size} bytes");
    assert!(TOOL_DESCRIPTION.contains("ask a quick question in your own way"), "design: quick choices stay in the agent's own dialog");
}
```

A form over 256 KB is refused by the caller before `check` (Task 6), since `check` receives parsed JSON.
- [ ] **Step 2:** Run `cargo test -p desl-proto form::`. Expected: FAIL (module missing).
- [ ] **Step 3:** Implement. Parse with serde into `Form`, then check ids (non-empty, unique, at most 64 characters), counts (2 to 12 options for `choice`, 1 to 50 items for `checklist`, 1 to 50 for `select`), ranges, and the title (1 to 120 characters). Map serde errors to sentences that name the widget by position and id. `TOOL_DESCRIPTION` says, in at most a dozen lines: what a form is for (edit, review, reorder, compare, choose with a note), that the tool returns at once and the answer arrives as the next prompt, to end the turn after calling it, and "ask a quick question in your own way". The schema lists the seven kinds and their fields.
- [ ] **Step 4:** Run `cargo test -p desl-proto form::`. Expected: PASS.
- [ ] **Step 5:** Commit: `desl_proto::form: the form language, checked`.

### Task 4: Forms in the record and the conversation

**Files:**
- Modify: `crates/desl-ir/src/lib.rs` (`EventKind`, `name`, `conversational`)
- Modify: `crates/desl-proto/src/conversation.rs` (`Item::Form`, `apply`)
- Modify: `crates/desl/src/ui/transcript.rs` (draw `Item::Form`)
- Modify: `crates/desl-web/src/bridge.rs` (`items`)
- Test: `crates/desl-proto/src/conversation.rs` tests, `crates/desl-web/src/tests.rs`

**Interfaces:**
- Produces:
  - `EventKind::Form { form_id: String, title: String, blob: String }` named `"form"`; `EventKind::FormAnswer { form_id: String, by: String, blob: Option<String> }` named `"form_answer"` (`by` is `user`, `withdrawn` or `dismissed`; `blob` holds the answer JSON for `user`). Both are not `conversational()`: a switch copy already carries the agent's tool call and the answer prompt.
  - `Item::Form { form_id: String, title: String, by: Option<String> }`
  - The bridge's conversation item `{"kind": "form", "form_id", "title", "by"}`.

- [ ] **Step 1: Write the failing test** in `conversation.rs`:

```rust
#[test]
fn a_form_is_one_card_and_its_tool_call_and_answer_prompt_fold_into_it() {
    let events = [
        ev(1, "claude", EventKind::UserMessage { text: "what next?".into(), images: vec![] }),
        ev(2, "claude", EventKind::Form { form_id: "f-4b1e".into(), title: "Which milestone next?".into(), blob: "ab".into() }),
        ev(3, "claude", EventKind::ToolCall { id: "c1".into(), name: "mcp__desl__show_form".into(), args: json!({}) }),
        ev(4, "claude", EventKind::ToolResult { call_id: "c1".into(), output: "Shown".into(), exit_code: None, is_error: false, truncated: false, blob: None }),
        ev(5, "claude", EventKind::FormAnswer { form_id: "f-4b1e".into(), by: "user".into(), blob: Some("cd".into()) }),
        ev(6, "claude", EventKind::UserMessage { text: answer_prompt("f-4b1e", "Which milestone next?", &json!({})), images: vec![] }),
    ];
    let (t, _) = conversation(&events, true);
    let kinds: Vec<&str> = t.items.iter().filter_map(|i| match i {
        Item::User { .. } => Some("user"), Item::Form { .. } => Some("form"), Item::Tool { .. } => Some("tool"), _ => None }).collect();
    assert_eq!(kinds, ["user", "form"]);
    assert!(matches!(&t.items.last().unwrap(), Item::Form { by: Some(b), .. } if b == "user"));
}
```

(Match `UserMessage`'s real fields; the test above assumes `text` and `images`.) The tool-call rule: a call whose name ends in `show_form` or `close_form` and its result are left out. The prompt rule: a `UserMessage` for which `form::is_form_prompt` holds is left out; any other `UserMessage` shows `form::strip_notes(text)`.
- [ ] **Step 2:** Run `cargo test -p desl-proto a_form_is_one_card`. Expected: FAIL.
- [ ] **Step 3:** Implement the events, `Item::Form`, the folding, the TUI's drawing of a form (a framed block: the title, then `open: answer it in the browser, prefix v`, or `answered`, `withdrawn` or `dismissed`), and the bridge item.
- [ ] **Step 4:** Run `make check`. Expected: PASS (every `match` on `Item` and `EventKind` covers the new variants; the copy writers in `crates/adapters/*` skip them).
- [ ] **Step 5:** Commit: `Forms in the thread's record and its conversation`.

### Task 5: The store keeps forms

**Files:**
- Modify: `crates/desl-daemon/src/store.rs` (migration 7, form functions, `delete_thread`)
- Modify: `crates/desl-ir/src/lib.rs` (`Thread.open_forms`, `OpenForm`)

**Interfaces:**
- Produces:
  - `pub struct OpenForm { pub id: String, pub title: String, pub at: i64 }` (serde) and `Thread.open_forms: Vec<OpenForm>` (`#[serde(default, skip_serializing_if = "Vec::is_empty")]`)
  - Table `forms (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, title TEXT NOT NULL, created_at INTEGER NOT NULL, blob TEXT NOT NULL, closed_by TEXT, answer_blob TEXT, told INTEGER NOT NULL DEFAULT 0, draft TEXT)`, `PRAGMA user_version = 7`
  - `Store::form_put(&self, id: &str, thread_id: &str, title: &str, at: i64, blob: &str) -> Result<()>`
  - `Store::form_close(&self, id: &str, by: &str, answer_blob: Option<&str>) -> Result<bool>` (false when it was already closed)
  - `Store::form_draft(&self, id: &str, draft: &str) -> Result<bool>` (false when closed or unknown)
  - `Store::form(&self, id: &str) -> Result<Option<StoredForm>>` with `pub struct StoredForm { pub id, pub thread_id, pub title, pub at: i64, pub blob, pub closed_by: Option<String>, pub answer_blob: Option<String>, pub draft: Option<String> }`
  - `Store::open_forms(&self) -> Result<Vec<(String, OpenForm)>>` (thread id, form), oldest first
  - `Store::untold_dismissals(&self, thread_id: &str) -> Result<Vec<(String, String)>>` (id, title) and `Store::mark_told(&self, ids: &[String]) -> Result<()>`

- [ ] **Step 1: Write the failing test** in `store.rs` tests:

```rust
#[test]
fn a_form_is_kept_until_it_closes_once() {
    let s = store();
    s.form_put("f-1", "t-1", "Pick", 10, "ab").unwrap();
    assert!(s.form_draft("f-1", "{\"next\":\"M5\"}").unwrap());
    assert_eq!(s.open_forms().unwrap(), [("t-1".to_string(), OpenForm { id: "f-1".into(), title: "Pick".into(), at: 10 })]);
    assert!(s.form_close("f-1", "dismissed", None).unwrap());
    assert!(!s.form_close("f-1", "user", Some("cd")).unwrap(), "closed once");
    assert!(!s.form_draft("f-1", "{}").unwrap());
    assert_eq!(s.untold_dismissals("t-1").unwrap(), [("f-1".to_string(), "Pick".to_string())]);
    s.mark_told(&["f-1".into()]).unwrap();
    assert!(s.untold_dismissals("t-1").unwrap().is_empty());
    assert!(s.open_forms().unwrap().is_empty());
}
```

Extend the existing migration test so a version-6 database migrates to 7 with its threads intact.
- [ ] **Step 2:** Run `cargo test -p desl-daemon a_form_is_kept`. Expected: FAIL.
- [ ] **Step 3:** Implement; `delete_thread` deletes the thread's forms; `threads()` leaves `open_forms` empty (the engine fills it from `open_forms()` at start).
- [ ] **Step 4:** Run `cargo test -p desl-daemon store::`. Expected: PASS.
- [ ] **Step 5:** Commit: `The store keeps forms and their drafts`.

### Task 6: The daemon runs forms

**Files:**
- Modify: `crates/desl-proto/src/lib.rs` (commands and replies)
- Modify: `crates/desl-daemon/src/engine.rs`
- Test: `crates/desl-daemon/src/engine/tests.rs`
- Modify: `docs/protocol.md`

**Interfaces:**
- Consumes: Task 3's `form::*`, Task 4's events, Task 5's store functions.
- Produces:
  - `Command::FormNew { thread_id: String, form: Value }` → `Reply::FormShown { form_id: String }`
  - `Command::FormAnswer { form_id: String, answer: Value }` → `Reply::Done`
  - `Command::FormClose { form_id: String, by: String }` → `Reply::Done` (`by` is `withdrawn` or `dismissed`)
  - `Command::FormDraft { form_id: String, draft: Value }` → `Reply::Done`
  - `Command::FormGet { form_id: String }` → `Reply::Form { form_id, thread_id, form: Value, draft: Option<Value>, answer: Option<Value>, closed_by: Option<String> }`
  - The engine field `asked_on_screen: HashSet<String>`: threads whose `waiting` comes from a dialog on their screen.

- [ ] **Step 1: Write the failing tests** with the existing `rig` and fake adapter (prompts reach `FakeState.payloads` through the headless path):

```rust
#[test]
fn a_form_waits_for_the_user_and_its_answer_is_the_next_prompt() {
    let mut r = rig("form");
    let t = r.t.clone();
    let form = json!({"title": "Which milestone next?", "widgets": [{"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5"}]}]});
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t.clone(), form }) else { panic!("shown") };
    assert!(form_id.starts_with("f-") && form_id.len() == 6);
    assert_eq!(r.thread(&t).status, Status::Waiting, "idle with an open form");
    assert_eq!(r.thread(&t).open_forms.len(), 1);
    assert!(r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M6"}) }).unwrap_err().contains("not one of the options"));
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M5"}) }).unwrap();
    let sent = r.state.lock().unwrap().payloads.last().cloned().unwrap();
    assert_eq!(sent, form::answer_prompt(&form_id, "Which milestone next?", &json!({"next": "M5"})));
    assert!(r.thread(&t).open_forms.is_empty());
}

#[test]
fn a_form_is_answered_once() { /* answer twice: the second is Err("f-xxxx was already answered. …"), one payload */ }

#[test]
fn an_answer_while_the_agent_works_is_queued() { /* status Working via a plan that holds the turn; answer → Reply::Done and thread.queued has the prompt */ }

#[test]
fn a_dismissal_is_told_before_the_next_prompt() {
    /* FormClose by dismissed → no payload; then Command::Prompt "go on" → payload == dismissed_note + "\n\n" + "go on"; a second prompt carries no note */
}

#[test]
fn an_open_form_and_its_draft_survive_a_restart() { /* FormNew, FormDraft, rebuild the engine on the same store: open_forms has it, status Waiting, FormGet's draft matches */ }

#[test]
fn a_fork_copies_no_open_form() { /* FormNew on t, fork t: the fork's open_forms is empty; FormAnswer still reaches t */ }

#[test]
fn a_form_too_big_or_for_no_agent_is_refused() { /* 300 KB title padding → "The form is 300 KB; desl takes at most 256 KB. …"; FormNew on a shell thread → a sentence */ }
```

The bodies in comments are the assertions to write, in the style of the first test.
- [ ] **Step 2:** Run `cargo test -p desl-daemon form`. Expected: FAIL.
- [ ] **Step 3:** Implement. `FormNew`: refuse over 256 KB (`serde_json::to_vec(&form)?.len()`), `form::check`, a new id `f-` and four hex digits unique in the store (as `new_thread_id`), the form JSON into a blob, `form_put`, append `EventKind::Form`, push `OpenForm` onto the thread, `persist`. `FormAnswer`: load the row, refuse when closed, `check_answer`, the answer into a blob, `form_close(.., "user", ..)`, append `FormAnswer`, remove the `OpenForm`, set `Idle` when the thread is `Waiting` and not in `asked_on_screen`, `persist`, then `self.prompt(thread_id, &form::answer_prompt(..))`. `FormClose`: as answer without the prompt. `FormDraft`: `form_draft`, refused when closed. `Command::Prompt`: before queueing or typing, prepend `untold_dismissals` as notes joined by newlines, a blank line, then the text; `mark_told`. Status: in `persist`, an `Idle` thread with open forms becomes `Waiting`; `check_asking` and `deliver` add the thread to `asked_on_screen` when they set `Waiting` from the screen, and `check_asking` turns `Waiting` back into `Working` only for a thread in that set (removing it). At start, fill each thread's `open_forms` from `open_forms()`.
- [ ] **Step 4:** Run `cargo test -p desl-daemon`. Expected: PASS.
- [ ] **Step 5:** Document the five commands, `Reply::FormShown`, `Reply::Form`, `thread.open_forms` and the two events in `docs/protocol.md` and `SPEC.md`'s IR events table (replacing `question` and `question_answer`).
- [ ] **Step 6:** Commit: `The daemon runs forms: shown, drafted, answered, dismissed`.

### Task 7: `desl mcp`, and each agent starts with it

**Files:**
- Create: `crates/desl/src/mcp.rs`
- Modify: `crates/desl/src/main.rs` (`Cmd::Mcp { thread: String, socket: PathBuf }`, hidden from `--help`)
- Modify: `crates/desl-ir/src/adapter.rs` (`LaunchOpts.mcp`, `McpServer`)
- Modify: `crates/adapters/claude/src/lib.rs`, `crates/adapters/codex/src/lib.rs`, `crates/adapters/copilot/src/lib.rs` (`launch`, tests)
- Modify: `crates/desl-daemon/src/engine.rs` (`start_pty` fills `mcp`)
- Modify: `docs/adapters.md`, `SPEC.md` (adapter boundary, each adapter's launch line)

**Interfaces:**
- Consumes: Task 6's `FormNew`, `FormClose`; Task 3's `TOOL_DESCRIPTION`, `input_schema`, `CLOSE_DESCRIPTION`, `close_schema`, `shown`.
- Produces:
  - `pub struct McpServer { pub command: String, pub args: Vec<String> }` and `LaunchOpts.mcp: Option<McpServer>`
  - `pub fn serve(input: impl BufRead, output: impl Write, call: impl FnMut(Command) -> Result<Reply, String>, thread: &str) -> io::Result<()>` in `mcp.rs`, and `pub fn run(socket: &Path, thread: &str) -> anyhow::Result<()>` that connects with `Conn` and calls `serve` on stdin/stdout.
  - Tool names: `show_form`, `close_form`; server name `desl`.

- [ ] **Step 1: Write the failing tests.** In `mcp.rs`:

```rust
#[test]
fn the_server_lists_its_tools_and_shows_a_form() {
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "show_form", "arguments": {"title": "T", "widgets": []}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "nope"}),
    ].iter().map(|v| v.to_string() + "\n").collect::<String>();
    let mut out = Vec::new();
    let mut asked = Vec::new();
    serve(input.as_bytes(), &mut out, |c| { asked.push(c); Ok(Reply::FormShown { form_id: "f-4b1e".into() }) }, "t-7f3a").unwrap();
    let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4, "a notification gets no reply");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "desl");
    let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["show_form", "close_form"]);
    assert_eq!(replies[2]["result"]["content"][0]["text"], form::shown("f-4b1e"));
    assert!(matches!(&asked[0], Command::FormNew { thread_id, .. } if thread_id == "t-7f3a"));
    assert_eq!(replies[3]["error"]["code"], -32601);
}
```

A refusal from the daemon returns `{"content": [{"type": "text", "text": <sentence>}], "isError": true}`, so the agent can fix its form in the same turn; add that case. In each adapter's tests, assert `launch` with `mcp: Some(McpServer { command: "/bin/desl".into(), args: vec!["mcp".into(), "--thread".into(), "t-1".into(), "--socket".into(), "/run/d.sock".into()] })` produces:
  - Claude: `--mcp-config` followed by one argument `{"mcpServers":{"desl":{"type":"stdio","command":"/bin/desl","args":[…]}}}`, then `--allowedTools` followed by `mcp__desl__show_form,mcp__desl__close_form`;
  - Codex: `-c` `mcp_servers.desl.command="/bin/desl"` and `-c` `mcp_servers.desl.args=["mcp","--thread","t-1","--socket","/run/d.sock"]`, before `resume` when resuming;
  - Copilot: `--additional-mcp-config` followed by `{"mcpServers":{"desl":{"type":"local","command":"/bin/desl","args":[…],"tools":["*"]}}}`, then `--allow-tool` `desl`.
- [ ] **Step 2:** Run `cargo test -p desl mcp::` and `cargo test -p desl-adapter-claude -p desl-adapter-codex -p desl-adapter-copilot` (use the crates' real names). Expected: FAIL.
- [ ] **Step 3:** Implement. `serve` reads one JSON object per line, answers `initialize` with the client's `protocolVersion`, `capabilities: {"tools": {}}` and `serverInfo {name: "desl", version}`, answers `ping` with `{}`, ignores notifications, and answers unknown methods with error `-32601`. `start_pty` fills `mcp` for agent threads with the daemon's own binary (the first word of `tasks_program`), `mcp --thread <id> --socket <socket path>`.
- [ ] **Step 4:** Run `make check`. Expected: PASS.
- [ ] **Step 5:** Check each agent by hand in the isolated workspace: start a thread, ask it "Use the desl show_form tool to ask me which of two colours I like." Expected for each of Claude Code, Codex and Copilot: the tool runs without an approval dialog of the agent's own (if Codex asks, find its per-tool setting and add it to the Codex flags with a test), and `desl form list` (Task 8) or `sqlite3 $DESL_HOME/…/workspace.db 'select id,title from forms'` shows the form. Record each agent's version in `docs/adapters.md`.
- [ ] **Step 6:** Commit: `desl mcp: agents push forms, started with desl's tool`.

### Task 8: `desl form` from a shell, and `prefix v` to a form

**Files:**
- Modify: `crates/desl/src/main.rs` (`Cmd::Form { command: FormCmd }`), `crates/desl/src/cli.rs` (the five commands)
- Modify: `crates/desl/src/native.rs` (`open_web` with the thread), `crates/desl/src/web.rs` (an address with the thread)
- Modify: `crates/desl-web/page/app.js` (`takeKey` reads `thread=` from the fragment and selects it)
- Modify: `SPEC.md` (CLI block, `prefix v` row), `README.md` (From a script)

**Interfaces:**
- Produces: `desl form new` (form JSON on stdin, on behalf of `DESL_THREAD`, prints the id; `--json` prints `{"form_id"}`), `desl form list` (open forms: id, thread, title, age), `desl form show <id>` (the form, draft and answer as JSON), `desl form answer <id> --json '<answer>'`, `desl form close <id>` (closes as `dismissed`); `web::url_for(url: &str, thread: &str) -> String` appending `&thread=<id>` to the fragment.

- [ ] **Step 1: Write the failing test** in `web.rs` tests:

```rust
#[test]
fn an_address_can_name_the_thread_to_open() {
    assert_eq!(url_for("http://127.0.0.1:7428/#t=0123456789abcdef0123456789abcdef", "t-7f3a"),
        "http://127.0.0.1:7428/#t=0123456789abcdef0123456789abcdef&thread=t-7f3a");
}
```
- [ ] **Step 2:** Run `cargo test -p desl an_address_can_name`. Expected: FAIL.
- [ ] **Step 3:** Implement the commands with `Conn::call` and the replies of Task 6; `prefix v` passes the focused thread when it has an open form; the page selects that thread after taking the key and scrolls to its oldest open form.
- [ ] **Step 4:** Run `make check`, then by hand: `echo '{"title":"T","widgets":[{"kind":"toggle","id":"a","label":"A"}]}' | DESL_THREAD=<id> target/debug/desl form new`, `desl form list`, `desl form answer <id> --json '{"a":true}'`. Expected: an id, one row, then the agent receives the answer prompt.
- [ ] **Step 5:** Commit: `desl form from a shell; prefix v opens the page on the form`.

### Task 9: Form cards on the page

**Files:**
- Create: `crates/desl-web/page/forms.js` (drawing and reading widgets), and its `FILES` row in `crates/desl-web/src/lib.rs`
- Modify: `crates/desl-web/page/app.js`, `crates/desl-web/page/app.css`
- Modify: `crates/desl-proto/src/view.rs` (`Sidebar.forms: Vec<FormRow>`), `crates/desl-web/src/tests.rs`

**Interfaces:**
- Consumes: the conversation item `{"kind": "form", …}` (Task 4); `form_get`, `form_answer`, `form_close`, `form_draft` through the bridge (it forwards any command but `shutdown`, `handover`, `hello`); `thread.open_forms` (Task 5).
- Produces:
  - `view::FormRow { pub form_id: String, pub thread_id: String, pub title: String, pub project: String, pub age: String }` and `Sidebar.forms`, oldest first, from every thread's `open_forms`.
  - In `forms.js`: `export function card(form, state, { onDraft, onSend, onDismiss, readOnly }) -> HTMLElement` and `export function answerOf(form, cardEl) -> object | null` (null while a required widget is unanswered).

- [ ] **Step 1: Write the failing test** in `view.rs` tests: two threads with one open form each, created at 10 and 20 → `sidebar(..).forms` lists them in that order with their project's name and ages from `age(now, at)`.
- [ ] **Step 2:** Run `cargo test -p desl-proto sidebar`. Expected: FAIL.
- [ ] **Step 3:** Implement `FormRow` and the page: a `form` conversation item asks `form_get` and draws the card in place. An open card is live: each widget drawn from `forms.js` with `textContent` and the Markdown renderer only; Send disabled until `answerOf` returns an object; `onDraft` sends `form_draft` one second after the last change; Send sends `form_answer` and shows a refusal sentence under the card; Dismiss sends `form_close` with `by: "dismissed"`. A closed card is read-only and shows the answer (or "dismissed", "withdrawn") and when. A card whose form belongs to another thread (a fork) is read-only and says "in <parent thread>". The right column lists `view.forms` above the screen-waiting lines; a click selects the thread and scrolls to the card.
- [ ] **Step 4:** Run `make check` and `node --check` on both JS files. Expected: PASS.
- [ ] **Step 5:** Check it live in Playwright on the isolated workspace with `desl form new` (no agent needed): a form with one widget of each of the seven kinds and the title `<img src=x onerror=alert(1)>`. Expected: the title shows as text, no console error; Send stays disabled until the required widgets are answered; a reload keeps a half-filled draft; Send makes the card read-only with the answer; the right column empties. Repeat at 390 px width. Delete `.playwright-mcp/`.
- [ ] **Step 6:** Commit: `desl web: forms as cards in the conversation`.

### Task 10: Each agent, end to end, and the documents

**Files:**
- Modify: `web-spec.md` (rewritten from the design: scope, layout, forms, milestones with 1 and 2 marked done), `SPEC.md` (vocabulary: **form** replaces **question**; the waiting list is open forms plus dialogs; the non-goal sentence; budgets `show_form` to card < 300 ms and Send to prompt < 1 s), `README.md`, `docs/CHANGELOG.md`, `skills/desl/SKILL.md` (one line: forms come from the desl MCP tool)

- [ ] **Step 1:** In the isolated workspace, for each of Claude Code, Codex and Copilot: prompt "Ask me with a desl form which of these two plans to follow, with a note field." Expected: the card appears in the conversation and the right column within 300 ms of the tool call (time it from the daemon log); the agent ends its turn; Send types the answer into the idle agent within 1 s; the agent's next reply uses the choice. Then a daemon restart with a form open: it comes back open with its draft.
- [ ] **Step 2:** Update the documents listed above.
- [ ] **Step 3:** `make check`, then commit and push: `desl web forms: milestones 1 and 2`.
