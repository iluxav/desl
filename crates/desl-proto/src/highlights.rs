//! Highlights (docs/design/2026-11-22-highlights.md): a thread's story as short questions and
//! answers, each part at most two sentences in plain words. desl writes one entry a turn from the
//! log: the question is what started the turn (a prompt, a task's brief, a form's answer), the
//! answer the first two sentences of the turn's last reply, or of the agent's words so far while
//! it runs. desl runs no model for it.

use crate::form;
use desl_ir::{Event, EventKind};
use serde::Serialize;

/// The most characters a question or an answer keeps.
pub const MAX_CHARS: usize = 300;

/// Who asked.
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum By {
    You,
    Task,
    Form,
    /// A prompt desl typed itself, such as asking for a rebase.
    Desl,
    /// The agent, with desl's `highlight` tool: a question it settled while working.
    Agent,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub by: By,
    /// When it was asked, in milliseconds.
    pub at: i64,
    /// The event it starts at, for a link to its moment in the conversation.
    pub event: u64,
    pub question: String,
    /// Empty until the agent says something.
    pub answer: String,
}

/// The thread's entries, oldest first.
pub fn highlights(events: &[Event]) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let mut turn: Option<usize> = None;
    for e in events {
        match &e.kind {
            EventKind::UserMessage { text, .. } => {
                let (by, question) = asked(text);
                if question.is_empty() {
                    continue;
                }
                out.push(Entry { by, at: e.timestamp, event: e.index, question, answer: String::new() });
                turn = Some(out.len() - 1);
            }
            EventKind::AssistantMessage { text, .. } => {
                let said = two_sentences(text);
                if let (Some(t), false) = (turn, said.is_empty()) {
                    out[t].answer = said;
                }
            }
            EventKind::Highlight { question, answer } => {
                out.push(Entry { by: By::Agent, at: e.timestamp, event: e.index, question: question.clone(), answer: answer.clone() });
            }
            _ => {}
        }
    }
    out
}

/// Who asked, and the question in at most two sentences.
fn asked(prompt: &str) -> (By, String) {
    let text = form::strip_notes(prompt);
    if form::is_form_prompt(text) {
        let title = text.split('"').nth(1).unwrap_or("a form");
        return (By::Form, format!("You answered “{title}”."));
    }
    if let Some(rest) = text.strip_prefix("desl: you are on task #") {
        let n = rest.split_whitespace().next().unwrap_or("");
        let title = rest.lines().next().and_then(|l| l.split_once("): ")).map_or("", |(_, t)| t.trim());
        return (By::Task, format!("Task #{n}: {title}"));
    }
    match text.strip_prefix("desl: ") {
        Some(rest) => (By::Desl, two_sentences(rest)),
        None => (By::You, two_sentences(text)),
    }
}

/// Why an agent's highlight is refused, as a sentence for the agent; `None` when it is kept.
pub fn refused(question: &str, answer: &str) -> Option<String> {
    for (what, text) in [("question", question), ("answer", answer)] {
        let text = text.trim();
        if text.is_empty() {
            return Some(format!("The {what} is empty. Give a question you settled and its answer."));
        }
        if text.chars().count() > MAX_CHARS {
            return Some(format!("The {what} is longer than {MAX_CHARS} characters. Say it in at most two short sentences."));
        }
        if units(text).iter().map(|u| sentences(u).len()).sum::<usize>() > 2 {
            return Some(format!("The {what} has more than two sentences. Keep each part to two sentences at most."));
        }
    }
    None
}

/// The first two sentences of `text`, its Markdown taken out: code, tables and headings left
/// out, a list item or a paragraph a sentence of its own when it has no full stop. A sentence
/// that ends in a colon leads into what follows, and is left out. At most [`MAX_CHARS`].
pub fn two_sentences(text: &str) -> String {
    let mut kept: Vec<String> = Vec::new();
    'units: for unit in units(text) {
        for s in sentences(&unit) {
            if s.ends_with(':') {
                continue;
            }
            kept.push(s);
            if kept.len() == 2 {
                break 'units;
            }
        }
    }
    capped(&kept.join(" "))
}

/// Prose in units: a paragraph, a list item, a quote; inline Markdown taken out.
fn units(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    let mut code = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            code = !code;
            open = false;
            continue;
        }
        if code || t.is_empty() || t.starts_with('|') || t.starts_with('#') || t.chars().all(|c| matches!(c, '-' | '*' | '_' | '=')) {
            open = false;
            continue;
        }
        let (item, rest) = match list_marker(t) {
            Some(rest) => (true, rest),
            None => (false, t.trim_start_matches('>').trim_start()),
        };
        let rest = inline(rest);
        if rest.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if open && !item => {
                last.push(' ');
                last.push_str(&rest);
            }
            _ => out.push(rest),
        }
        open = true;
    }
    out
}

/// What follows a list item's marker: `- `, `* `, `+ `, `1. ` or `1) `.
fn list_marker(line: &str) -> Option<&str> {
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(m) {
            return Some(rest.trim_start());
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some(rest.trim_start());
        }
    }
    None
}

/// A line without emphasis, code marks and link targets: `[text](url)` keeps its text.
fn inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c == '['
            && let Some(close) = rest.find("](")
            && let Some(end) = rest[close..].find(')')
        {
            out.push_str(&rest[1..close]);
            rest = &rest[close + end + 1..];
            continue;
        }
        if !matches!(c, '*' | '`') && !(c == '_' && (rest.starts_with("__"))) {
            out.push(c);
        } else if c == '_' {
            rest = &rest[2..];
            continue;
        }
        rest = &rest[c.len_utf8()..];
    }
    out.trim().to_string()
}

/// A unit's sentences: each ends at `.`, `?` or `!` before a space, or at the unit's end. A few
/// abbreviations do not end one.
fn sentences(unit: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0;
    let chars: Vec<(usize, char)> = unit.char_indices().collect();
    for (k, &(i, c)) in chars.iter().enumerate() {
        let next_space = chars.get(k + 1).is_some_and(|&(_, n)| n == ' ');
        if matches!(c, '.' | '?' | '!') && next_space {
            let s = unit[start..=i].trim();
            let word = s.rsplit(' ').next().unwrap_or("");
            if matches!(word.to_ascii_lowercase().as_str(), "e.g." | "i.e." | "vs." | "etc.") && c == '.' {
                continue;
            }
            if !s.is_empty() {
                out.push(s.to_string());
            }
            start = i + 1;
        }
    }
    let tail = unit[start..].trim();
    if !tail.is_empty() {
        out.push(tail.to_string());
    }
    out
}

fn capped(text: &str) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(index: u64, kind: EventKind) -> Event {
        Event { index, thread_id: "t-7f3a".into(), timestamp: index as i64 * 1000, agent: "claude".into(), kind }
    }
    fn user(index: u64, text: &str) -> Event {
        ev(index, EventKind::UserMessage { text: text.into(), attachments: vec![] })
    }
    fn reply(index: u64, text: &str) -> Event {
        ev(index, EventKind::AssistantMessage { text: text.into(), model: None })
    }

    #[test]
    fn a_turn_is_its_prompt_and_the_first_two_sentences_of_its_last_reply() {
        let events = [
            user(1, "ok when i start the desl it shows the right pane but no tasks, only when i prefix t it refreshes"),
            reply(2, "Checking where the TUI draws."),
            ev(3, EventKind::ToolCall { id: "c1".into(), name: "Bash".into(), args: serde_json::json!({}) }),
            reply(
                4,
                "Fixed and pushed (ddfd72d, 335 tests pass). To get it:\n\n1. Run `make install` in `~/Projects/desl`.\n2. Quit desl.\n\n**Cause:** only `prefix t` asked.",
            ),
        ];
        let h = highlights(&events);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].by, By::You);
        assert_eq!((h[0].event, h[0].at), (1, 1000));
        assert_eq!(h[0].question, "ok when i start the desl it shows the right pane but no tasks, only when i prefix t it refreshes");
        assert_eq!(h[0].answer, "Fixed and pushed (ddfd72d, 335 tests pass). Run make install in ~/Projects/desl.");
    }

    #[test]
    fn a_highlight_longer_than_two_sentences_or_300_characters_is_refused() {
        assert_eq!(refused("Why is the pane empty?", "Only prefix t asked. Now it asks on its own."), None);
        assert!(refused("Why?", "One. Two. Three.").is_some_and(|s| s.contains("two sentences")));
        assert!(refused(&"x".repeat(301), "Yes.").is_some_and(|s| s.contains("300 characters")));
        assert!(refused("Why?", "  ").is_some_and(|s| s.contains("answer is empty")));
    }

    #[test]
    fn an_agents_own_highlight_is_an_entry_of_its_own_after_its_turns() {
        let events = [
            user(1, "Why is the pane empty?"),
            ev(2, EventKind::Highlight { question: "Who asks for the list?".into(), answer: "Only prefix t.".into() }),
            reply(3, "Fixed."),
        ];
        let h = highlights(&events);
        let got: Vec<(By, &str, &str, u64)> = h.iter().map(|e| (e.by, e.question.as_str(), e.answer.as_str(), e.event)).collect();
        assert_eq!(got, [(By::You, "Why is the pane empty?", "Fixed.", 1), (By::Agent, "Who asks for the list?", "Only prefix t.", 2)]);
    }

    #[test]
    fn while_the_turn_runs_the_answer_is_the_agents_words_so_far() {
        let h = highlights(&[user(1, "Why is the pane empty? It shows no tasks."), reply(2, "Checking where the TUI draws.")]);
        assert_eq!(h[0].question, "Why is the pane empty? It shows no tasks.");
        assert_eq!(h[0].answer, "Checking where the TUI draws.");
        assert_eq!(highlights(&[user(1, "Hello")])[0].answer, "", "nothing said yet");
    }

    #[test]
    fn a_task_a_form_answer_and_desls_own_prompt_say_who_asked() {
        let brief =
            "desl: you are on task #12 of this project's task list (.tasks.md): Wheel report typed as text\n\nYou work in a git worktree.";
        let form = form::answer_prompt("f-4b1e", "Which milestone next?", &serde_json::json!({"pick": "m3"}));
        let dismissed = format!("{}\nWhat now?", form::dismissed_note("f-1111", "Old"));
        let h = highlights(&[
            user(1, brief),
            user(2, &form),
            user(3, "desl: main moved on. Rebase the branch onto main."),
            user(4, &dismissed),
        ]);
        let got: Vec<(By, &str)> = h.iter().map(|e| (e.by, e.question.as_str())).collect();
        assert_eq!(
            got,
            [
                (By::Task, "Task #12: Wheel report typed as text"),
                (By::Form, "You answered “Which milestone next?”."),
                (By::Desl, "main moved on. Rebase the branch onto main."),
                (By::You, "What now?"),
            ]
        );
    }

    #[test]
    fn two_sentences_leave_out_code_tables_headings_and_links_targets() {
        let text = "## Done\n\nThe pane fills on start. See [the design](docs/design.md), e.g. its goals.\n\n```rust\nlet x = 1;\n```\n\n| a | b |\n\nA third one.";
        assert_eq!(two_sentences(text), "The pane fills on start. See the design, e.g. its goals.");
        assert_eq!(two_sentences("**What changed:**\n- **Filters:** every filter shows."), "Filters: every filter shows.");
        assert_eq!(two_sentences("A soft-wrapped\nparagraph goes on. And ends."), "A soft-wrapped paragraph goes on. And ends.");
        let long = "word ".repeat(100);
        let cut = two_sentences(&long);
        assert!(cut.chars().count() == MAX_CHARS && cut.ends_with('…'), "{cut}");
    }
}
