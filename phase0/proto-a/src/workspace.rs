//! The projection of the log into threads and items. Host and client run the same code, so
//! they always agree on what the screen should show.

use crate::agent::AgentEvent;
use crate::agent::claude::describe_tool;
use crate::log::Entry;
use crate::model::{Agent, Approval, Item, Open, Outcome, PendingBrief, Status, Thread, derive_name};
use std::path::PathBuf;

pub struct Workspace {
    pub cwd: PathBuf,
    pub threads: Vec<Thread>,
}

impl Workspace {
    pub fn new(cwd: PathBuf) -> Self {
        Workspace { cwd, threads: Vec::new() }
    }

    /// Applies one entry. Returns the dequeued prompt text for `Dequeued`, else None.
    pub fn apply(&mut self, e: &Entry) -> Option<String> {
        let idx = e.idx();
        if let Entry::ThreadCreated { id, agent, .. } = e {
            let mut t = Thread::new(*agent);
            t.id = id.clone();
            if idx == self.threads.len() {
                self.threads.push(t);
            }
            return None;
        }
        let cwd = self.cwd.clone();
        let Some(t) = self.threads.get_mut(idx) else { return None };
        match e {
            Entry::ThreadCreated { .. } => {}
            Entry::Renamed { name, .. } => t.name = name.chars().take(12).collect(),
            Entry::Prompt { text, queued, .. } => {
                if t.name.is_empty() && !*queued {
                    t.name = derive_name(text);
                }
                t.items.push(Item::User { text: text.clone(), queued: *queued });
                t.follow = true;
            }
            Entry::Dequeued { .. } => {
                let text = t.first_queued();
                if t.name.is_empty()
                    && let Some(text) = &text
                {
                    t.name = derive_name(text);
                }
                return text;
            }
            Entry::Dispatched { agent, brief, .. } => {
                if let Some(b) = brief {
                    t.items.push(Item::Brief { label: b.label.clone(), text: b.text.clone(), tokens: b.tokens, expanded: false });
                }
                t.pending_brief = None;
                t.status = Status::Working;
                t.turns += 1;
                let len = t.items.len();
                t.binding_mut(*agent).seen_up_to = len;
            }
            Entry::Agent { agent, ev, .. } => apply_agent(t, *agent, ev, &cwd),
            Entry::Decision { request_id, allow, always, .. } => {
                if let Some(a) = t.approval.take_if(|a| &a.request_id == request_id) {
                    if *always {
                        t.always.insert(a.tool.clone());
                    }
                    let word = match (allow, always) {
                        (true, true) => "always",
                        (true, false) => "allowed",
                        _ => "denied",
                    };
                    if let Some(Item::Tool { decision, .. }) = t.approval_tool_mut(a.tool_use_id.as_deref(), &a.summary) {
                        *decision = Some(word);
                    }
                    t.status = Status::Working;
                }
            }
            Entry::Interrupted { ended, .. } => {
                t.approval = None;
                t.close_streaming();
                t.items.push(Item::Note("interrupted".into()));
                if *ended {
                    t.status = Status::Idle;
                }
            }
            Entry::Switched { to, brief_from, delta, .. } => {
                t.items.push(Item::Divider(format!("switched to {}", to.name())));
                t.agent = *to;
                t.pending_brief = brief_from.map(|from| PendingBrief { from, delta: *delta });
                t.follow = true;
            }
            Entry::Failed { message, .. } => {
                t.close_streaming();
                t.approval = None;
                t.items.push(Item::Error(message.clone()));
                t.status = Status::Failed;
            }
            Entry::Note { text, brief, .. } => {
                match brief {
                    Some(b) => t.items.push(Item::Brief { label: text.clone(), text: b.text.clone(), tokens: b.tokens, expanded: true }),
                    None => t.items.push(Item::Note(text.clone())),
                }
                t.follow = true;
            }
            Entry::HostRestarted { .. } => {
                if t.busy() {
                    t.close_streaming();
                    t.approval = None;
                    t.status = Status::Idle;
                    t.items.push(Item::Note(
                        "the desl host restarted while this agent was working; send your prompt again to resume".into(),
                    ));
                }
            }
        }
        None
    }
}

pub fn apply_agent(t: &mut Thread, agent: Agent, ev: &AgentEvent, cwd: &std::path::Path) {
    let is_active = t.agent == agent;
    match ev {
        AgentEvent::Started { session_id, model } => {
            let b = t.binding_mut(agent);
            b.session_id = Some(session_id.clone());
            b.model = Some(model.clone());
        }
        _ if !is_active && !matches!(ev, AgentEvent::Exited { .. }) => {
            // A stale process talking after a switch: keep the record, do not disturb the view.
            if let AgentEvent::TextFinal(s) = ev {
                t.items.push(Item::Note(format!("{} (late): {}", agent.name(), s)));
            }
        }
        AgentEvent::TextStart => t.start_text(),
        AgentEvent::TextDelta(s) => t.push_text_delta(s),
        AgentEvent::TextFinal(s) => t.finish_text(s),
        AgentEvent::ToolUse { id, name, input } => {
            if !t.has_tool(id) {
                t.close_streaming();
                let (verb, target) = describe_tool(name, input, cwd);
                t.items.push(Item::Tool {
                    id: id.clone(),
                    verb,
                    target,
                    outcome: None,
                    decision: None,
                    detail: String::new(),
                    diff: None,
                    open: Open::Closed,
                });
            }
        }
        AgentEvent::ToolResult { tool_use_id, is_error, content, diff } => {
            if let Some(Item::Tool { verb, outcome, detail, diff: shown, open, .. }) = t.tool_mut(tool_use_id) {
                let summary = if *is_error {
                    let first = content.lines().find(|l| !l.trim().is_empty()).unwrap_or("error");
                    format!("✕ {}", first.chars().take(40).collect::<String>())
                } else if let Some(d) = diff {
                    // What the edit did, the way git and lazygit count it.
                    match crate::diff::stat(d) {
                        (0, 0) => "✓".to_string(),
                        (a, 0) => format!("+{a}"),
                        (0, r) => format!("−{r}"),
                        (a, r) => format!("+{a} −{r}"),
                    }
                } else if verb == "read" && !content.is_empty() {
                    match content.lines().count() {
                        1 => "1 line".to_string(),
                        n => format!("{n} lines"),
                    }
                } else {
                    "✓".to_string()
                };
                *outcome = Some(Outcome { ok: !is_error, summary });
                *detail = content.clone();
                // An edit shows its diff as it lands; everything else stays one line until asked.
                if diff.is_some() && *open == Open::Closed {
                    *open = Open::Preview;
                }
                *shown = diff.clone();
            }
        }
        AgentEvent::ApprovalRequest { request_id, tool_name, input, tool_use_id } => {
            let (verb, target) = describe_tool(tool_name, input, cwd);
            let summary = format!("{verb} {target}");
            if t.always.contains(tool_name) {
                if let Some(Item::Tool { decision, .. }) = t.approval_tool_mut(tool_use_id.as_deref(), &summary) {
                    *decision = Some("always");
                }
                return;
            }
            t.approval = Some(Approval {
                request_id: request_id.clone(),
                tool: tool_name.clone(),
                summary,
                input: input.clone(),
                tool_use_id: tool_use_id.clone(),
            });
            t.status = Status::Waiting;
        }
        AgentEvent::TurnDone { is_error, cost_usd, result, subtype, .. } => {
            t.close_streaming();
            if *cost_usd > 0.0 {
                t.cost_usd = *cost_usd;
            }
            t.approval = None;
            if *is_error && subtype != "error_during_execution" {
                let detail = result.clone().unwrap_or_else(|| subtype.clone());
                t.items.push(Item::Error(format!("{} stopped: {detail}. Send your prompt again to retry.", agent.name())));
            }
            t.status = Status::Idle;
            let len = t.items.len();
            t.binding_mut(agent).seen_up_to = len;
        }
        AgentEvent::Exited { .. } => {
            if is_active && t.busy() {
                t.close_streaming();
                t.approval = None;
                t.status = Status::Failed;
            }
        }
    }
}
