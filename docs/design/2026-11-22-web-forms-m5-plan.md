# desl web forms, milestone 5: preview, table, images. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an agent can push three more widgets: `preview` (HTML and CSS it wrote, tuned with
controls), `table` (cells to edit) and `images` (pictures of the project to pick from). It can also
start a `choice` on an option, and the tool tells it to pre-set what is true now.

**Architecture:** the form language grows in `desl_proto::form`. The page draws each widget in
`page/forms.js`. Images are the one part that reads files. The daemon checks each path when the form is
pushed: it must sit inside the thread's directory, be a picture and weigh at most 10 MB. The daemon
reads a picture again, with the same checks, when the page asks with a new `form_image` command. The
preview is drawn in an `<iframe sandbox="allow-same-origin">` with no scripts allowed. The page fills
it through the DOM and applies the agent's CSS as a constructed stylesheet, since the page's CSP has
no `'unsafe-inline'`.

**Tech stack:** Rust (serde, serde_json, base64 0.22 already in the lock), plain JavaScript modules,
no build step.

**Spec:** `docs/design/2026-11-21-web-forms.md`, sections "The form language", "Documents, diffs,
images" and "Milestones" item 5. The author picked this milestone on 2026-11-22, with a 4-hour
budget, Claude Code building it and commits going to main. With it came one note: "in form, which
agent builds it, the current agent should be pre selected".

## Global constraints

- The tool's description and schemas together stay ≤ 6,000 bytes (≈ 1,500 tokens): the test
  `the_tool_costs_at_most_1500_tokens_a_turn`.
- The schema stays flat: no `oneOf`.
- Nothing an agent writes runs in the page. The preview frame has no `allow-scripts`.
- The page's CSP is unchanged: no `'unsafe-inline'`.
- Images: PNG, JPEG, GIF, WebP or SVG. Each file at most 10 MB, at most 12 per widget. The path
  resolves, links followed, inside the thread's directory (`Thread.cwd`, which is the worktree for
  a worktree thread).
- Every refusal names what was wrong and what to do.
- `make check` passes before each commit. Commits carry no Co-Authored-By line, and go to main and
  are pushed.

## Review focus

1. A preview whose HTML has `<script>`, `onerror=`, `javascript:` links, `<meta refresh>`,
   `<form>` or `<iframe>`: nothing runs, nothing navigates the page, nothing loads from outside.
2. An image path with `..`, an absolute path outside the directory, or a link inside the
   directory that points outside it is refused when the form is pushed and when the page reads
   it. The same goes for a file swapped for such a link after the form was pushed.
3. `form_image` for a path the form does not name is refused, so the page cannot read arbitrary
   files through it.
4. A conversation redraw while the user drags a preview control does not reload the frame or lose
   the drag.
5. A table answer that names an unknown row, a column that cannot be edited, or a select cell
   outside its options is refused.

## Rulings made while planning

- The images widget's list is named `images`, not `items`, because the flat schema already types
  `items` for checklists. The cost if wrong is one field name.
- A table answer lists each changed row as `{id, <column>: value}` with only the changed cells.
  The cost if wrong: the agent maps row ids back to its rows.
- A preview control's `kind` is `range`, `colour` or `select`, and `color` is accepted as
  `colour`. A select's options are CSS values, given as plain strings.
- The preview answer has every control's value: a number for a range, `#rrggbb` for a colour,
  the string for a select. The frame's variable gets the number with its `unit`.

---

### Task 1: a choice starts on a value; the tool says to pre-set what is true now

**Files:** `crates/desl-proto/src/form.rs`, `crates/desl-web/page/forms.js`

- [ ] Test `a_choice_starts_on_its_value`:
  - `{"kind":"choice","id":"a","options":[{"label":"X"},{"label":"Y"}],"value":"Y"}` is accepted.
  - With `"value":"Z"` it is refused with `starts at "Z", which is not one of its options`.
  - A `multi` choice takes `"value":["X","Y"]` and refuses `"value":"X"`, saying a list.
  - `TOOL_DESCRIPTION` contains `value`, telling the agent to pre-set what is true now or what it
    recommends.
- [ ] Make it pass: `Choice.value: Option<Value>`, checked in `sound`; the description line; the
  schema's `value` description mentions choice.
- [ ] Page: `DRAW.choice` falls back to `w.value` when neither a draft nor an answer gives one.
- [ ] Commit.

### Task 2: preview

**Files:** `form.rs`, `forms.js`, `app.js` (a redraw that keeps cards in place), `app.css`

**Produces:** `Widget::Preview { id, label?, html, css?, controls: Vec<Control>, optional }`, and
`Control` tagged by `kind`:
- `range {var, label?, min, max, step?, unit?, value?}`
- `colour {var, label?, value?}`
- `select {var, label?, options: Vec<String>, value?}`

- [ ] Test `a_preview_is_checked_control_by_control`:
  - accepted: a preview with one control of each kind, and `"kind":"color"` too;
  - refused: a `var` without a leading `--`, or with a character other than letters, digits, `-`
    and `_`;
  - refused: a range value outside min to max;
  - refused: a colour value that is not `#rrggbb`;
  - refused: a select value not among its options, and a select with no options;
  - refused: more than 12 controls, or the same var twice;
  - refused: HTML of more than 64 KB, which is already under the form's cap.
- [ ] Test `a_preview_answer_holds_each_control`: `{"--accent":"#3355ff","--radius":8}` is accepted.
  Refused: a var the preview lacks, a range number outside its range, a colour that is not
  `#rrggbb`, a select value not among its options. Every control must be given.
- [ ] Make them pass: the description line, and the schema fields `html`, `css`, `controls`.
- [ ] Page:
  - `DRAW.preview` builds the iframe with `sandbox="allow-same-origin"` and a srcdoc skeleton.
  - On each `load` it fills the frame:
    - parse the HTML with `DOMParser`;
    - drop `script, iframe, frame, object, embed, base, meta, link`, every `on*` attribute and
      every `javascript:` URL;
    - move `<style>` text and `style` attributes into a constructed `CSSStyleSheet` (attributes
      come back through `el.style.cssText`);
    - import the nodes and set each control's variable.
  - The controls are drawn by the page.
  - A "Wide" button hides the right column and makes the frame taller.
- [ ] `drawConversation` removes only the nodes that left, and inserts the rest around the kept
  cards, so a card is never detached and its frame never reloads.
- [ ] Commit.

### Task 3: table

**Files:** `form.rs`, `forms.js`, `app.css`

**Produces:**
- `Widget::Table { id, label?, columns: Vec<Column>, rows: Vec<Map<String, Value>>, optional }`
- `Column { id, label, edit: Option<Edit>, options: Vec<String> }`, where `Edit` is `text`,
  `select` or `check`.

- [ ] Test `a_table_is_checked_row_by_row`:
  - accepted: 1 to 12 columns;
  - refused: 0 or 13 columns, a duplicate column id, a select column with no options, and the
    column `id` set to edit;
  - accepted: 1 to 200 rows, each with a unique string `id`;
  - refused: a row key that is no column and not `id`, and a cell that is an object or a list;
  - refused: a check cell that is not a boolean, and a select cell not among its options.
- [ ] Test `a_table_answer_lists_the_changed_cells`: `[{"id":"r1","prio":"low"}]` is accepted, and
  `[]` too.
  - Refused: an unknown row, the same row twice, a column that cannot be edited, the wrong type
    for a cell, and a select value outside its options.
- [ ] Make them pass: the description line and the schema fields `columns` and `rows`.
- [ ] Page: `DRAW.table` draws a `<table>`, with inputs in the editable cells, so the answer
  holds only the cells that changed. A draft keeps the changes.
- [ ] Commit.

### Task 4: images

**Files:** `form.rs`, `crates/desl-proto/src/lib.rs` (`FormImage`, `Reply::Image`),
`crates/desl-daemon/src/engine.rs`, `crates/desl-daemon/Cargo.toml` (base64),
`engine/tests.rs`, `forms.js`, `app.js`

**Produces:**
- `Widget::Images { id, label?, images: Vec<Pic {path, caption?}>, multi, optional }`
- `Command::FormImage { form_id, path }` → `Reply::Image { mime, data }`, with `data` as base64.

- [ ] Test `images_are_checked_and_answered_by_path` (form.rs):
  - accepted: 1 to 12 images;
  - refused: the same path twice;
  - answer: a listed path, or a list of paths with `multi`.
- [ ] Engine tests:
  - `an_image_outside_the_thread_is_refused`: `../x.png`, an absolute path outside, a link to
    outside, a file that is not a picture, and a file over 10 MB are each refused with a sentence
    naming the path.
  - `the_page_reads_only_the_images_a_form_names`: `form_image` returns `image/png` and the base64
    of a listed PNG. It refuses an unlisted path, and refuses a listed path swapped for a link to
    outside after the push.
- [ ] Make them pass. `form_new` calls `pictures_inside(&form, cwd)` after `form::check`. The type
  comes from the magic bytes. An SVG needs a `.svg` name and an `<svg` in its first 4 KB.
- [ ] Page: `DRAW.images` draws a grid of picks, radio or checkbox. Each image is asked for with
  `ctx.image(path)`. While it reads, and if it fails, the card shows the file name.
- [ ] `docs/protocol.md`: `form_image` and `image`.
- [ ] Commit.

### Task 5: docs and a live check

- [ ] Docs:
  - `web-spec.md`: the widget table and the preview's sandbox.
  - `SPEC.md`: the form vocabulary, if it lists widgets.
  - The skill: the widget list.
  - `docs/CHANGELOG.md`.
- [ ] Live check in isolation, with `DESL_HOME` scratch, `DESL_SOCKET=$XDG_RUNTIME_DIR/desl-vf.sock`
  and `tmux -L desl-vf`:
  - Claude Code pushes a preview, a table and images.
  - Playwright drags a control and edits a cell, then sends.
  - The agent gets the answer.
  - A preview with `<script>` and `<img onerror>` runs nothing: check the console and a `window.top`
    marker.
- [ ] Commit, push.

### Final

- [ ] Run a fresh reviewer on the most capable model over the milestone's range. Fix what is
  Critical or Important, test first, then push.
