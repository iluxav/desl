//! The projector (SPEC.md "Cross-agent continuity"): a pure function from a range of a thread's
//! events to the handoff brief. Deterministic, no network, no model. Sections are omitted when
//! empty, filled in priority order and truncated from the bottom within a token budget.
//! Everything it cannot verify from events is left out; it never invents.

use desl_ir::{ChangeKind, Event, EventKind, diff, tool, tool_target, tool_verb};
use std::collections::HashMap;

pub const BUDGET_TOKENS: usize = 4000;

pub struct Brief {
    pub text: String,
    pub token_estimate: u64,
    /// User turns in the range.
    pub turns: u64,
    /// First and last event index summarised.
    pub covers: (u64, u64),
}

/// Byte-ratio heuristic, marked `~` wherever it is shown.
pub fn estimate_tokens(s: &str) -> u64 {
    s.len().div_ceil(4) as u64
}

fn truncate_to(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut cut = max_bytes.saturating_sub(16);
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &s[..cut];
    let head = head.rsplit_once('\n').map(|(a, _)| a).unwrap_or(head);
    format!("{head}\n[truncated]")
}

const DECISION_MARKERS: &[&str] =
    &["i'll use", "i will use", "decided", "instead of", "because", "i'll go with", "i chose", "rather than", "i'm going to"];

fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = para.trim();
        if para.is_empty() || para.starts_with("```") {
            continue;
        }
        let mut cur = String::new();
        for ch in para.chars() {
            cur.push(ch);
            if matches!(ch, '.' | '!' | '?') {
                out.push(cur.trim().to_string());
                cur.clear();
            }
        }
        if !cur.trim().is_empty() {
            out.push(cur.trim().to_string());
        }
    }
    out
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max - 1).collect::<String>()) }
}

/// `events` is the range to brief: the whole thread for a full brief, the unseen tail for a
/// delta. `pending_prompt` is the user's next prompt, which becomes "Next step". `current_diff`
/// is the worktree's diff from the git service; a delta brief leaves it out.
pub fn build(events: &[Event], pending_prompt: Option<&str>, delta: bool, current_diff: &str, budget_tokens: usize) -> Brief {
    let turns = events.iter().filter(|e| matches!(e.kind, EventKind::UserMessage { .. })).count() as u64;
    let covers = (events.first().map(|e| e.index).unwrap_or(0), events.last().map(|e| e.index).unwrap_or(0));

    // Task: the first user prompt, verbatim.
    let task = events.iter().find_map(|e| match &e.kind {
        EventKind::UserMessage { text, .. } => Some(text.clone()),
        _ => None,
    });

    // Decisions: sentences with an explicit marker, or the sentence right before an edit.
    let mut decisions: Vec<String> = Vec::new();
    let mut last_sentence: Option<String> = None;
    for e in events {
        match &e.kind {
            EventKind::AssistantMessage { text, .. } => {
                for s in sentences(text) {
                    let lower = s.to_lowercase();
                    if DECISION_MARKERS.iter().any(|m| lower.contains(m)) {
                        let line = one_line(&s, 160);
                        if !decisions.contains(&line) {
                            decisions.push(line);
                        }
                    }
                    last_sentence = Some(s);
                }
            }
            EventKind::ToolCall { name, .. } if name == tool::EDIT => {
                if let Some(s) = last_sentence.take() {
                    let line = one_line(&s, 160);
                    if !decisions.contains(&line) {
                        decisions.push(line);
                    }
                }
            }
            _ => {}
        }
    }

    // Files touched: from file_change events, never from prose.
    let mut order: Vec<String> = Vec::new();
    let mut touched: HashMap<String, (usize, usize, bool, bool, bool)> = HashMap::new(); // added, removed, counted, new, deleted
    for e in events {
        if let EventKind::FileChange { path, diff: d, change, .. } = &e.kind {
            let entry = touched.entry(path.clone()).or_insert_with(|| {
                order.push(path.clone());
                (0, 0, false, false, false)
            });
            if let Some(d) = d {
                let (a, r) = diff::stat(d);
                entry.0 += a;
                entry.1 += r;
                entry.2 = true;
            }
            match change {
                ChangeKind::Create => entry.3 = true,
                ChangeKind::Delete => entry.4 = true,
                ChangeKind::Modify => {}
            }
        }
    }
    let files: Vec<String> = order
        .iter()
        .map(|path| {
            let (a, r, counted, new, deleted) = touched[path];
            if deleted {
                format!("{path} (deleted)")
            } else if new {
                format!("{path} (new)")
            } else if counted {
                format!("{path} (+{a} −{r})")
            } else {
                path.clone()
            }
        })
        .collect();

    // Recent conversation: the last five turns, tool calls as one line each.
    let ok: HashMap<&str, bool> = events
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ToolResult { call_id, is_error, .. } => Some((call_id.as_str(), !is_error)),
            _ => None,
        })
        .collect();
    let mut turn_starts: Vec<usize> =
        events.iter().enumerate().filter(|(_, e)| matches!(e.kind, EventKind::UserMessage { .. })).map(|(k, _)| k).collect();
    if turn_starts.is_empty() {
        turn_starts.push(0);
    }
    let from = turn_starts[turn_starts.len().saturating_sub(5)];
    let mut recent = String::new();
    // A run of tool lines is set off from the message after it by a blank line.
    let gap = |recent: &mut String| {
        if !recent.is_empty() && !recent.ends_with("\n\n") {
            recent.push('\n');
        }
    };
    for e in events.iter().skip(from) {
        match &e.kind {
            EventKind::UserMessage { text, .. } => {
                gap(&mut recent);
                recent.push_str(&format!("User: {}\n\n", text.trim()));
            }
            EventKind::AssistantMessage { text, .. } if !text.trim().is_empty() => {
                gap(&mut recent);
                recent.push_str(&format!("Assistant: {}\n\n", text.trim()));
            }
            EventKind::ToolCall { id, name, args } => {
                let outcome = match ok.get(id.as_str()) {
                    Some(true) => " ✓",
                    Some(false) => " ✕",
                    None => "",
                };
                recent.push_str(format!("- {} {}", tool_verb(name), tool_target(name, args)).trim_end());
                recent.push_str(outcome);
                recent.push('\n');
            }
            _ => {}
        }
    }

    // Open questions: questions in the last assistant message.
    let last_assistant = events.iter().rev().find_map(|e| match &e.kind {
        EventKind::AssistantMessage { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    });
    let questions: Vec<String> = last_assistant
        .as_deref()
        .map(|t| sentences(t).into_iter().filter(|s| s.ends_with('?')).map(|s| one_line(&s, 200)).collect())
        .unwrap_or_default();

    // Next step: the pending prompt, else the last assistant message's final paragraph.
    let next_step = pending_prompt
        .map(str::to_string)
        .or_else(|| last_assistant.as_deref().and_then(|t| t.trim().rsplit("\n\n").next().map(|p| p.trim().to_string())));

    // Assemble within budget, in priority order; each later section gets what is left.
    let budget_bytes = budget_tokens * 4;
    let header = if delta {
        "# Handoff brief (delta)\nYou are continuing a thread you have seen before. Below is only what happened\nsince you last saw it. Treat it as your own history and continue from \"Next step\".\n"
    } else {
        "# Handoff brief\nYou are continuing work that another assistant started. Treat this as your\nown history. Do not re-explain it; continue from \"Next step\".\n"
    };
    let mut remaining = budget_bytes.saturating_sub(header.len());
    let mut sections: Vec<(usize, String)> = Vec::new(); // (position, text)
    let mut take = |pos: usize, title: &str, body: String, remaining: &mut usize| {
        if body.trim().is_empty() {
            return;
        }
        let allowed = remaining.saturating_sub(title.len() + 8);
        if allowed < 64 {
            return;
        }
        let body = truncate_to(body.trim_end(), allowed);
        *remaining = remaining.saturating_sub(body.len() + title.len() + 8);
        sections.push((pos, format!("\n## {title}\n{body}\n")));
    };
    take(0, "Task", task.unwrap_or_default(), &mut remaining);
    take(6, "Next step", next_step.unwrap_or_default(), &mut remaining);
    take(3, "Current diff", if delta { String::new() } else { current_diff.to_string() }, &mut remaining);
    take(4, "Recent conversation", recent, &mut remaining);
    take(1, "Decisions", decisions.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    take(2, "Files touched", files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    take(5, "Open questions", questions.iter().map(|q| format!("- {q}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    sections.sort_by_key(|(pos, _)| *pos);

    let mut text = header.to_string();
    for (_, s) in sections {
        text.push_str(&s);
    }
    Brief { token_estimate: estimate_tokens(&text), text, turns, covers }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn ev(index: u64, kind: EventKind) -> Event {
        Event { index, thread_id: "t-7f3a".into(), timestamp: index as i64, agent: "claude".into(), kind }
    }

    fn sample() -> Vec<Event> {
        vec![
            ev(1, EventKind::UserMessage { text: "Add refresh-token rotation to the auth middleware".into(), attachments: vec![] }),
            ev(2, EventKind::AssistantMessage { text: "I'll start by reading the current middleware.".into(), model: None }),
            ev(3, EventKind::ToolCall { id: "c1".into(), name: "read".into(), args: json!({ "path": "src/auth/middleware.ts" }) }),
            ev(
                4,
                EventKind::ToolResult {
                    call_id: "c1".into(),
                    output: "…".into(),
                    exit_code: None,
                    is_error: false,
                    truncated: false,
                    blob: None,
                },
            ),
            ev(
                5,
                EventKind::AssistantMessage {
                    text: "I'll use a rotating refresh token because a stolen one then stops working after one use.".into(),
                    model: None,
                },
            ),
            ev(6, EventKind::ToolCall { id: "c2".into(), name: "edit".into(), args: json!({ "path": "src/auth/middleware.ts" }) }),
            ev(
                7,
                EventKind::ToolResult {
                    call_id: "c2".into(),
                    output: "".into(),
                    exit_code: None,
                    is_error: false,
                    truncated: false,
                    blob: None,
                },
            ),
            ev(
                8,
                EventKind::FileChange {
                    path: "src/auth/middleware.ts".into(),
                    diff: Some("@@ -1,2 +1,3 @@\n a\n-b\n+c\n+d\n".into()),
                    change: ChangeKind::Modify,
                    call_id: Some("c2".into()),
                },
            ),
            ev(9, EventKind::ToolCall { id: "c3".into(), name: "edit".into(), args: json!({ "path": "src/auth/tokens.ts" }) }),
            ev(
                10,
                EventKind::FileChange {
                    path: "src/auth/tokens.ts".into(),
                    diff: Some("@@ -0,0 +1,1 @@\n+x\n".into()),
                    change: ChangeKind::Create,
                    call_id: Some("c3".into()),
                },
            ),
            ev(11, EventKind::ToolCall { id: "c4".into(), name: "bash".into(), args: json!({ "command": "pnpm test" }) }),
            ev(
                12,
                EventKind::ToolResult {
                    call_id: "c4".into(),
                    output: "exit 1".into(),
                    exit_code: Some(1),
                    is_error: true,
                    truncated: false,
                    blob: None,
                },
            ),
            ev(13, EventKind::Usage { input_tokens: 10, output_tokens: 20, cost_estimate: None }),
            ev(
                14,
                EventKind::AssistantMessage {
                    text: "Rotation is in, one test fails.\n\nWant me to add the revocation list?".into(),
                    model: None,
                },
            ),
        ]
    }

    /// Byte-for-byte goldens. `DESL_BLESS=1 cargo test -p desl-daemon` rewrites them after a
    /// deliberate change to the extraction rules.
    fn golden(name: &str, got: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens").join(name);
        if std::env::var_os("DESL_BLESS").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, got).unwrap();
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no golden at {}; run with DESL_BLESS=1 once", path.display()));
        assert_eq!(got, want, "the brief {name} changed");
    }

    #[test]
    fn a_full_brief_matches_its_golden() {
        let b = build(
            &sample(),
            Some("Add the revocation list too"),
            false,
            "diff --git a/src/auth/tokens.ts b/src/auth/tokens.ts\n+x",
            BUDGET_TOKENS,
        );
        golden("full.md", &b.text);
        assert_eq!((b.turns, b.covers), (1, (1, 14)));
    }

    #[test]
    fn a_delta_brief_matches_its_golden_and_carries_no_diff() {
        let events = sample();
        let b = build(&events[4..], None, true, "never shown", BUDGET_TOKENS);
        golden("delta.md", &b.text);
        assert!(!b.text.contains("Current diff"));
    }

    #[test]
    fn sections_keep_their_order_and_empty_ones_are_left_out() {
        let b = build(&sample(), Some("next"), false, "", BUDGET_TOKENS);
        let order = [
            "# Handoff brief",
            "## Task",
            "## Decisions",
            "## Files touched",
            "## Recent conversation",
            "## Open questions",
            "## Next step",
        ];
        let mut at = 0;
        for h in order {
            let i = b.text[at..].find(h).unwrap_or_else(|| panic!("{h} missing or out of order"));
            at += i;
        }
        assert!(!b.text.contains("## Current diff"), "an empty diff has no section");
    }

    #[test]
    fn files_touched_come_from_file_change_events_with_their_counts() {
        let b = build(&sample(), None, false, "", BUDGET_TOKENS);
        assert!(b.text.contains("- src/auth/middleware.ts (+2 −1)"), "{}", b.text);
        assert!(b.text.contains("- src/auth/tokens.ts (new)"));
    }

    #[test]
    fn the_budget_holds_and_the_task_and_next_step_survive() {
        let mut events = sample();
        events.push(ev(15, EventKind::AssistantMessage { text: "word ".repeat(20_000), model: None }));
        let b = build(&events, Some("the next thing"), false, &"+line\n".repeat(5000), 1000);
        assert!(b.token_estimate <= 1000, "{} tokens", b.token_estimate);
        assert!(b.text.contains("## Task") && b.text.contains("the next thing"));
    }

    #[test]
    fn the_same_events_give_the_same_brief() {
        let a = build(&sample(), Some("x"), false, "d", BUDGET_TOKENS).text;
        let b = build(&sample(), Some("x"), false, "d", BUDGET_TOKENS).text;
        assert_eq!(a, b);
    }
}
