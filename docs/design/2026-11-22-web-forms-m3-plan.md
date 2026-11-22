# desl web forms, milestone 3: the document widget. Implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task by task.

**Goal:** an agent names a Markdown file of the project, and optionally one section of it. The user
edits that text in place on the page and comments on parts of it, then sends. desl saves the
section into the file in one write, but only if the section on disk is still what the card opened
with. The agent gets back whether it was saved, the unified diff, and the comments.

**Spec:** `docs/design/2026-11-21-web-forms.md`: the `document` row of "The form language", "When it
is read", "Documents, diffs, images", and milestone 3. Also `web-spec.md` "Security" item 6. The
author said "go milestone 3" on 2026-11-22.

## Global constraints

- The path resolves, links followed, inside the thread's directory. It names a regular file
  ending in `.md` or `.markdown`, of at most 1 MB, valid UTF-8, with LF line endings.
- The section is read once, when the form is pushed, and kept in the stored form as `original`.
- A save writes only that section. It writes only when the section on disk equals the text the
  edit started from. It writes in one go: a temporary file in the same folder, renamed over the
  file, with the file's mode kept.
- The tool's description and schema stay ≤ 6,000 bytes, and the schema stays flat.
- `make check` passes before each commit. Commits carry no Co-Authored-By line, go to main, and
  are pushed.

## Rulings made while planning

| Ruling | Why | Cost if wrong |
| --- | --- | --- |
| A section is an ATX heading (`#` to `######`) and everything up to the next heading of the same or a higher level. Headings inside fenced code don't count. `section` matches the heading's text with or without its `#`s. A heading that appears twice is refused. | Simple and predictable | Setext headings (underlined) can't be named |
| The page answers `{text, base?, comments}`; the agent gets `{saved, diff, more?, comments[{quote, note}]}`. `base` is left out when it equals `original`. | The daemon computes the diff and does the save, so the agent sees what really happened | none |
| A comment quotes the selected text, or the paragraph at the caret when nothing is selected | A textarea has no paragraph boxes to click | Comments are less visual than a rendered view |
| On a conflict the save is refused and the page asks for the section as it is now (`form_section`). The card shows both versions. "Use the file's version" and "Save mine over it" (with `base` set to the file's text) settle it. | Compare-and-swap: nothing is lost silently | One more command |
| The diff comes from `git diff --no-index` on the whole file, before and after, capped at 400 lines with `more` | No diff library in the build; git is already needed | A machine without git saves but sends an empty diff |
| Sections are capped at 256 KB, and drafts at 1 MB | A draft holds the edited text and the comments | A very long section must be named by a smaller heading |
| Files with CRLF line endings are refused | A textarea turns CRLF into LF, so every line would read as changed | Windows-style files can't be edited here |

## Review focus

1. A path with `..`, an absolute path outside the directory, a link to a file outside it, a FIFO,
   a non-Markdown file or a file over 1 MB is refused, at push time and at save time.
2. A save never writes outside the named section. The rest of the file is byte-for-byte the same.
3. A file edited on disk after the push is not overwritten unless the user chooses so.
4. A draft survives a reload: the text, the comments, and the base after a conflict.
5. An unchanged document answers `saved: false` and leaves the file untouched.

### Task 1: sections, a pure module

`crates/desl-proto/src/markdown.rs`: `pub fn section(text: &str, heading: Option<&str>) -> Result<Range<usize>, String>`.
Tests: the section at level 2 runs to the next `##` or `#` and keeps any `###` inside it; a heading
inside a fence is skipped; the heading matches with or without `#`s; a heading that appears twice
is refused, and so is a missing heading, each with a sentence; with no heading, the whole file.

### Task 2: the widget in the form language

`Widget::Document { id, label?, path, section?, original?, optional }`.
- `check`: the path ends in `.md` or `.markdown`, has no control characters, and appears at most
  once among the form's documents.
- `check_answer` (the page's shape): `text` is a string; `base` is optional; `comments` is a list of
  at most 50, each with `quote` and `note`.
- Also: the description line and the schema fields `path` and `section`.

### Task 3: reading and saving in the daemon

`crates/desl-daemon/src/files.rs` takes the resolve-inside and open logic from `picture.rs`, which
then uses it. `crates/desl-daemon/src/document.rs` has:
- `read(dir, path, section) -> Result<String, String>`
- `current(...) -> Option<String>`
- `save(dir, path, section, base, text) -> Result<Saved, String>`
- the diff

The engine:
- `form_new` fills `original` into the stored form.
- `form_answer` checks every document first, then writes them, and turns the page's answer into
  the agent's.
- A new command, `form_section {form_id, widget}`, returns `section {text?}`.
- `form_draft` takes up to 1 MB.

Engine tests:
- read when pushed, and saved on Send;
- unchanged means not saved;
- a conflict is refused, and saving after one works;
- refused paths.

### Task 4: the page

`DRAW.document`:
- the path, with a Wide button;
- Edit and Preview tabs over a textarea;
- Comment, plus a list of comments;
- a conflict panel.

`read()` returns `{text, comments, base?}`. An answered card shows "Saved" or "No change", the diff
and the comments. `ctx.section(widget)` is backed by `form_section`.

### Task 5: docs, a live run, review

- Docs: `web-spec.md`, `docs/protocol.md`, `SPEC.md` vocabulary, the changelog, and the design doc's
  milestone 3 row.
- Live in isolation: Claude Code pushes a document on a section of a scratch file. Edit, comment
  and send; check the file. Then make a conflict and settle it.
- A fresh review, then the fixes.
