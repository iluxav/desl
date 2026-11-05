//! The brief: a deterministic projection of a thread's items into the handoff text in
//! SPEC.md "Cross-agent continuity". No network, no model. Sections are omitted when empty,
//! filled in priority order and truncated from the bottom within a token budget.

use crate::model::Item;
use std::path::Path;
use std::process::Command;

pub const BUDGET_TOKENS: usize = 4000;

pub struct Brief {
    pub text: String,
    pub token_estimate: usize,
    pub turns: usize,
}

/// Byte-ratio heuristic, marked `~` wherever it is shown.
pub fn estimate_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn git(cwd: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
        .unwrap_or_default()
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

/// `items` is the range to brief (all items for a full brief, the unseen tail for a delta).
/// `pending_prompt` is the user's next prompt, which becomes "Next step".
pub fn build(items: &[Item], cwd: &Path, pending_prompt: Option<&str>, delta: bool) -> Brief {
    let turns = items.iter().filter(|i| matches!(i, Item::User { queued: false, .. })).count();

    // Task: the first user prompt, verbatim.
    let task = items.iter().find_map(|i| match i {
        Item::User { text, .. } => Some(text.clone()),
        _ => None,
    });

    // Decisions: sentences with an explicit marker, or the sentence right before an edit.
    let mut decisions: Vec<String> = Vec::new();
    let mut last_sentence: Option<String> = None;
    for item in items {
        match item {
            Item::Assistant { text, .. } => {
                for s in sentences(text) {
                    let lower = s.to_lowercase();
                    if DECISION_MARKERS.iter().any(|m| lower.contains(m)) {
                        decisions.push(one_line(&s, 160));
                    }
                    last_sentence = Some(s);
                }
            }
            Item::Tool { verb, .. } if verb == "edit" => {
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
    decisions.dedup();

    // Files touched: edits the agent made, with git's counts when it knows the file.
    let numstat = git(cwd, &["diff", "--numstat", "HEAD"]);
    let mut files: Vec<String> = Vec::new();
    for item in items {
        if let Item::Tool { verb, target, .. } = item
            && verb == "edit"
            && !target.is_empty()
            && !files.iter().any(|f| f.starts_with(target.as_str()))
        {
            let counts = numstat.lines().find_map(|l| {
                let mut p = l.split('\t');
                let (a, d, f) = (p.next()?, p.next()?, p.next()?);
                (f == target).then(|| format!(" (+{a} −{d})"))
            });
            files.push(format!("{target}{}", counts.unwrap_or_default()));
        }
    }
    let untracked = git(cwd, &["ls-files", "--others", "--exclude-standard"]);
    for f in untracked.lines() {
        if items.iter().any(|i| matches!(i, Item::Tool { verb, target, .. } if verb == "edit" && target == f))
            && !files.iter().any(|x| x.starts_with(f))
        {
            files.push(format!("{f} (new)"));
        }
    }

    // Current diff: from git, never from the agent.
    let diff = if delta { String::new() } else { git(cwd, &["diff", "HEAD"]) };

    // Recent conversation: the last five turns, tool calls as one line each.
    let mut turn_starts: Vec<usize> =
        items.iter().enumerate().filter(|(_, i)| matches!(i, Item::User { queued: false, .. })).map(|(k, _)| k).collect();
    if turn_starts.is_empty() {
        turn_starts.push(0);
    }
    let from = turn_starts[turn_starts.len().saturating_sub(5)];
    let mut recent = String::new();
    for item in &items[from..] {
        match item {
            Item::User { text, queued: false } => recent.push_str(&format!("User: {}\n\n", text.trim())),
            Item::Assistant { text, .. } if !text.trim().is_empty() => {
                recent.push_str(&format!("Assistant: {}\n\n", text.trim()))
            }
            Item::Tool { verb, target, outcome, .. } => {
                let o = outcome.as_ref().map(|o| if o.ok { " ✓" } else { " ✕" }).unwrap_or("");
                recent.push_str(&format!("- {verb} {target}{o}\n"));
            }
            _ => {}
        }
    }

    // Open questions: questions in the last assistant message.
    let last_assistant = items.iter().rev().find_map(|i| match i {
        Item::Assistant { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    });
    let questions: Vec<String> = last_assistant
        .as_deref()
        .map(|t| sentences(t).into_iter().filter(|s| s.ends_with('?')).map(|s| one_line(&s, 200)).collect())
        .unwrap_or_default();

    // Next step: the pending prompt, else the last assistant message's final paragraph.
    let next_step = pending_prompt.map(str::to_string).or_else(|| {
        last_assistant.as_deref().and_then(|t| t.trim().rsplit("\n\n").next().map(|p| p.trim().to_string()))
    });

    // Assemble within budget, in priority order; each later section gets what is left.
    let budget_bytes = BUDGET_TOKENS * 4;
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
    take(3, "Current diff", diff, &mut remaining);
    take(4, "Recent conversation", recent, &mut remaining);
    take(1, "Decisions", decisions.iter().map(|d| format!("- {d}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    take(2, "Files touched", files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    take(5, "Open questions", questions.iter().map(|q| format!("- {q}")).collect::<Vec<_>>().join("\n"), &mut remaining);
    sections.sort_by_key(|(pos, _)| *pos);

    let mut text = header.to_string();
    for (_, s) in sections {
        text.push_str(&s);
    }
    let token_estimate = estimate_tokens(&text);
    Brief { text, token_estimate, turns }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Open, Outcome};

    fn sample() -> Vec<Item> {
        vec![
            Item::User { text: "Add refresh-token rotation to the auth middleware".into(), queued: false },
            Item::Assistant { text: "I'll start by reading the current middleware.".into(), streaming: false },
            Item::Tool { id: "1".into(), verb: "read".into(), target: "src/auth/middleware.ts".into(), outcome: Some(Outcome { ok: true, summary: "✓".into() }), decision: None, detail: String::new(), diff: None, open: Open::Closed },
            Item::Assistant { text: "I'll use a rotating refresh token stored server-side because revocation needs a list anyway.".into(), streaming: false },
            Item::Tool { id: "2".into(), verb: "edit".into(), target: "src/auth/middleware.ts".into(), outcome: Some(Outcome { ok: true, summary: "✓".into() }), decision: None, detail: String::new(), diff: None, open: Open::Closed },
            Item::Assistant { text: "Rotation is in.\n\nWant me to add the revocation list?".into(), streaming: false },
        ]
    }

    #[test]
    fn full_brief_has_sections_in_spec_order() {
        let b = build(&sample(), Path::new("/nonexistent"), Some("Yes, add the revocation list.".into()), false);
        println!("{}", b.text);
        let order = ["# Handoff brief", "## Task", "## Decisions", "## Files touched", "## Recent conversation", "## Open questions", "## Next step"];
        let mut last = 0;
        for h in order {
            let pos = b.text.find(h).unwrap_or_else(|| panic!("missing {h}"));
            assert!(pos >= last, "{h} out of order");
            last = pos;
        }
        assert!(!b.text.contains("## Current diff"), "no git, so no diff section");
        assert!(b.text.contains("- I'll use a rotating refresh token"));
        assert!(b.text.contains("- src/auth/middleware.ts"));
        assert!(b.text.contains("- Want me to add the revocation list?"));
        assert!(b.text.trim_end().ends_with("Yes, add the revocation list."));
        assert_eq!(b.turns, 1);
    }

    #[test]
    fn budget_truncates_from_the_bottom() {
        let mut items = sample();
        items.push(Item::Assistant { text: "x ".repeat(20_000), streaming: false });
        let b = build(&items, Path::new("/nonexistent"), None, false);
        assert!(b.token_estimate <= BUDGET_TOKENS + 64, "estimate {} over budget", b.token_estimate);
        assert!(b.text.contains("[truncated]"));
        assert!(b.text.contains("## Next step"), "next step is filled before the long section");
    }
}
