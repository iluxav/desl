//! A conversation as the transcript renderer draws it: IR events projected into items. The
//! native client uses it for a subagent's conversation; it holds nothing that matters.

use desl_ir::{Decision, Event, EventKind, diff, tool_target, tool_verb};

pub struct Outcome {
    pub ok: bool,
    pub summary: String,
}

/// How much of a tool call is on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Open {
    /// The one line.
    Closed,
    /// An edit as it lands: the first `PREVIEW_LINES` rows of its diff.
    Preview,
}

/// Rows of a diff shown under an edit before the user asks for all of it.
pub const PREVIEW_LINES: usize = 12;

pub enum Item {
    User {
        text: String,
    },
    Assistant {
        text: String,
        streaming: bool,
    },
    Tool {
        id: String,
        verb: String,
        target: String,
        outcome: Option<Outcome>,
        /// "allowed", "denied" or "always": the approval decision, shown on the line.
        decision: Option<&'static str>,
        detail: String,
        /// The change an edit made, as a unified diff. Drawn in place of `detail`.
        diff: Option<String>,
        open: Open,
    },
    /// A thin labelled line: `── switched to codex ──`.
    Divider(String),
    /// The handoff text that was (or would be) sent, collapsed to one line.
    Brief {
        label: String,
        text: String,
        tokens: u64,
        expanded: bool,
    },
    Note(String),
    Error(String),
}

pub struct Thread {
    pub items: Vec<Item>,
    /// The event index behind each user and assistant message, in order.
    pub messages: Vec<u64>,
    pub cost_usd: f64,
    pub turns: u32,
}

impl Thread {
    pub fn new() -> Self {
        Thread { items: Vec::new(), messages: Vec::new(), cost_usd: 0.0, turns: 0 }
    }

    /// Replaces everything this view knows with the daemon's events.
    pub fn load(&mut self, events: &[Event]) {
        self.items.clear();
        self.messages.clear();
        self.cost_usd = 0.0;
        self.turns = 0;
        for e in events {
            self.apply(e);
        }
    }

    /// The block is complete: replace the streamed text with the final text so the two never drift.
    fn finish_text(&mut self, full: &str) {
        // The last assistant item, looking past notes such as "interrupted".
        let last = self.items.iter_mut().rev().find(|i| !matches!(i, Item::Note(_)));
        match last {
            Some(Item::Assistant { text, streaming }) if *streaming => {
                *text = full.to_string();
                *streaming = false;
            }
            // An interrupted block arrives again as a final event; it is already on screen.
            Some(Item::Assistant { text, streaming: false }) if text.as_str() == full => {}
            _ if !full.is_empty() => self.items.push(Item::Assistant { text: full.to_string(), streaming: false }),
            _ => {}
        }
    }

    pub fn close_streaming(&mut self) {
        match self.items.last_mut() {
            // A block that never got a word leaves nothing behind.
            Some(Item::Assistant { text, streaming: true }) if text.is_empty() => {
                self.items.pop();
            }
            Some(Item::Assistant { streaming, .. }) => *streaming = false,
            _ => {}
        }
    }

    fn tool_mut(&mut self, tool_id: &str) -> Option<&mut Item> {
        self.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id, .. } if id == tool_id))
    }

    /// One event of the thread's log, as the transcript shows it.
    pub fn apply(&mut self, e: &Event) {
        match &e.kind {
            EventKind::UserMessage { text, .. } => {
                self.close_streaming();
                self.items.push(Item::User { text: text.clone() });
                self.messages.push(e.index);
                self.turns += 1;
            }
            EventKind::AssistantMessage { text, .. } => {
                self.finish_text(text);
                self.messages.push(e.index);
            }
            EventKind::ToolCall { id, name, args } => {
                if self.tool_mut(id).is_none() {
                    self.close_streaming();
                    self.items.push(Item::Tool {
                        id: id.clone(),
                        verb: tool_verb(name).to_string(),
                        target: tool_target(name, args),
                        outcome: None,
                        decision: None,
                        detail: String::new(),
                        diff: None,
                        open: Open::Closed,
                    });
                }
            }
            EventKind::ToolResult { call_id, output, is_error, truncated, .. } => {
                if let Some(Item::Tool { verb, outcome, detail, .. }) = self.tool_mut(call_id) {
                    let summary = if *is_error {
                        let first = output.lines().find(|l| !l.trim().is_empty()).unwrap_or("error");
                        format!("✕ {}", first.chars().take(40).collect::<String>())
                    } else if verb == "read" && !output.is_empty() {
                        match output.lines().count() {
                            1 => "1 line".to_string(),
                            n if *truncated => format!("{n}+ lines"),
                            n => format!("{n} lines"),
                        }
                    } else {
                        "✓".to_string()
                    };
                    *outcome = Some(Outcome { ok: !is_error, summary });
                    *detail = output.clone();
                }
            }
            EventKind::FileChange { path, diff: change, call_id, .. } => {
                // What the edit did, the way git and lazygit count it.
                let counted = change.as_deref().map(|d| match diff::stat(d) {
                    (0, 0) => "✓".to_string(),
                    (a, 0) => format!("+{a}"),
                    (0, r) => format!("−{r}"),
                    (a, r) => format!("+{a} −{r}"),
                });
                match call_id.as_deref().and_then(|id| self.tool_mut(id)) {
                    Some(Item::Tool { outcome, diff: shown, open, .. }) => {
                        if let Some(summary) = counted {
                            *outcome = Some(Outcome { ok: true, summary });
                            // An edit shows its diff as it lands; everything else stays one line until asked.
                            if *open == Open::Closed {
                                *open = Open::Preview;
                            }
                            *shown = change.clone();
                        }
                    }
                    // A change no tool call announced still gets its line.
                    _ => self.items.push(Item::Tool {
                        id: format!("change-{}", e.index),
                        verb: "edit".into(),
                        target: path.clone(),
                        outcome: Some(Outcome { ok: true, summary: counted.unwrap_or_else(|| "✓".into()) }),
                        decision: None,
                        detail: String::new(),
                        open: if change.is_some() { Open::Preview } else { Open::Closed },
                        diff: change.clone(),
                    }),
                }
            }
            // The strip above the prompt comes from the thread's pending approval.
            EventKind::ApprovalRequest { .. } => {}
            EventKind::ApprovalResponse { call_id, decision: answer, .. } => {
                if let Some(Item::Tool { decision, .. }) = self.tool_mut(call_id) {
                    *decision = Some(match answer {
                        Decision::Allow => "allowed",
                        Decision::Deny => "denied",
                        Decision::Always => "always",
                    });
                }
            }
            EventKind::AgentSwitch { to, .. } => {
                self.close_streaming();
                self.items.push(Item::Divider(format!("switched to {to}")));
            }
            EventKind::Fork { parent_id, .. } => {
                self.close_streaming();
                self.items.push(Item::Divider(format!("forked from {parent_id}")));
            }
            EventKind::Brief { text, token_estimate, delta, .. } => {
                let label = format!("{}brief sent to {}", if *delta { "delta " } else { "" }, e.agent);
                self.items.push(Item::Brief { label, text: text.clone(), tokens: *token_estimate, expanded: false });
            }
            EventKind::Usage { cost_estimate, .. } => self.cost_usd += cost_estimate.unwrap_or(0.0),
            EventKind::Error { message, .. } => {
                self.close_streaming();
                self.items.push(Item::Error(message.clone()));
            }
            EventKind::Interrupt => {
                self.close_streaming();
                self.items.push(Item::Note("interrupted".into()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desl_ir::ChangeKind;
    use serde_json::json;

    fn ev(index: u64, agent: &str, kind: EventKind) -> Event {
        Event { index, thread_id: "t-7f3a".into(), timestamp: 0, agent: agent.into(), kind }
    }

    #[test]
    fn an_edit_shows_its_diff_and_counts_on_its_own_line() {
        let mut t = Thread::new();
        t.apply(&ev(1, "claude", EventKind::ToolCall { id: "c1".into(), name: "edit".into(), args: json!({ "path": "src/a.ts" }) }));
        t.apply(&ev(
            2,
            "claude",
            EventKind::ToolResult {
                call_id: "c1".into(),
                output: "ok".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
        ));
        t.apply(&ev(
            3,
            "claude",
            EventKind::FileChange {
                path: "src/a.ts".into(),
                diff: Some("@@ -1,1 +1,2 @@\n-a\n+b\n+c\n".into()),
                change: ChangeKind::Modify,
                call_id: Some("c1".into()),
            },
        ));
        assert_eq!(t.items.len(), 1);
        let Item::Tool { verb, target, outcome, open, diff, .. } = &t.items[0] else { panic!() };
        assert_eq!((verb.as_str(), target.as_str(), *open), ("edit", "src/a.ts", Open::Preview));
        assert_eq!(outcome.as_ref().map(|o| o.summary.as_str()), Some("+2 −1"));
        assert!(diff.is_some());
    }

    #[test]
    fn a_command_runs_and_a_failure_leads_with_its_first_line() {
        let mut t = Thread::new();
        t.apply(&ev(1, "codex", EventKind::ToolCall { id: "c1".into(), name: "bash".into(), args: json!({ "command": "ls /nope" }) }));
        t.apply(&ev(
            2,
            "codex",
            EventKind::ToolResult {
                call_id: "c1".into(),
                output: "exit 2\nls: cannot access".into(),
                exit_code: Some(2),
                is_error: true,
                truncated: false,
                blob: None,
            },
        ));
        let Item::Tool { verb, target, outcome, .. } = &t.items[0] else { panic!() };
        assert_eq!((verb.as_str(), target.as_str()), ("run", "ls /nope"));
        let o = outcome.as_ref().unwrap();
        assert_eq!((o.ok, o.summary.as_str()), (false, "✕ exit 2"));
    }

    #[test]
    fn a_final_message_is_one_item() {
        let mut t = Thread::new();
        t.load(&[ev(1, "claude", EventKind::AssistantMessage { text: "Hello.".into(), model: None })]);
        assert_eq!(t.items.len(), 1);
        assert!(matches!(&t.items[0], Item::Assistant { text, streaming: false } if text == "Hello."));
    }

    #[test]
    fn switches_briefs_errors_and_interrupts_have_their_own_rows() {
        let mut t = Thread::new();
        t.load(&[
            ev(1, "claude", EventKind::UserMessage { text: "one".into(), attachments: vec![] }),
            ev(2, "codex", EventKind::AgentSwitch { from: "claude".into(), to: "codex".into(), brief_event_id: None }),
            ev(3, "codex", EventKind::UserMessage { text: "two".into(), attachments: vec![] }),
            ev(4, "codex", EventKind::Brief { text: "# Handoff brief".into(), token_estimate: 617, covers_range: (1, 2), delta: false }),
            ev(5, "codex", EventKind::Interrupt),
            ev(6, "codex", EventKind::Error { message: "codex exited with 1 mid-turn.".into(), recoverable: true }),
        ]);
        let rows: Vec<String> = t
            .items
            .iter()
            .map(|i| match i {
                Item::User { text } => format!("user {text}"),
                Item::Divider(l) => format!("divider {l}"),
                Item::Brief { label, tokens, .. } => format!("brief {label} {tokens}"),
                Item::Note(n) => format!("note {n}"),
                Item::Error(e) => format!("error {e}"),
                _ => "other".into(),
            })
            .collect();
        assert_eq!(
            rows,
            [
                "user one",
                "divider switched to codex",
                "user two",
                "brief brief sent to codex 617",
                "note interrupted",
                "error codex exited with 1 mid-turn."
            ]
        );
        assert_eq!((t.turns, t.messages.clone()), (2, vec![1, 3]));
    }
}
