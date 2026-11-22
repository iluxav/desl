//! The engine against a scripted fake agent: no real processes, no network.

use super::*;
use desl_ir::adapter::{AdapterError, Capabilities, Detection};
use desl_ir::{Effort, ModelInfo, Risk, tool};
use serde_json::json;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

#[derive(Default)]
struct FakeState {
    /// What the next sends answer with; when empty, a plain reply.
    plan: VecDeque<Vec<AdapterEvent>>,
    payloads: Vec<String>,
    /// The resume id each `start` was given.
    starts: Vec<Option<String>>,
    /// The model each `start` was given.
    models: Vec<Option<String>>,
    /// The effort each started session was given.
    efforts: Vec<Option<String>>,
    /// The permission mode each started session was given.
    modes: Vec<PermissionMode>,
    /// How many times the model list was asked for.
    listed: usize,
    answers: Vec<(String, Decision)>,
    interrupts: usize,
    sink: Option<Sink>,
    sessions: usize,
    /// A resumed session dies before saying anything.
    fail_resume: bool,
    /// `interrupt` ends the turn at once, as Codex does.
    interrupt_ends_turn: bool,
    /// `write_session` copies a conversation into a session, as Claude Code's adapter does.
    copies: bool,
    /// The events of each conversation copied into a session.
    copied: Vec<usize>,
}

struct Fake {
    id: &'static str,
    state: Arc<Mutex<FakeState>>,
}

struct FakeSession {
    id: &'static str,
    state: Arc<Mutex<FakeState>>,
    sink: Sink,
    resumed: bool,
    announced: bool,
}

impl Adapter for Fake {
    fn id(&self) -> &'static str {
        self.id
    }
    fn detect(&self) -> Detection {
        Detection {
            installed: true,
            logged_in: Some(true),
            version: Some("1".into()),
            fixtures_version: "1".into(),
            ..Detection::default()
        }
    }
    fn installed(&self) -> bool {
        true
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    /// The fake codex is like the real one: it cannot ask per call.
    fn permission_note(&self, mode: PermissionMode) -> Option<String> {
        (self.id == "codex" && mode == PermissionMode::Ask).then(|| "reads only, because codex cannot ask".to_string())
    }
    fn models(&self, _cwd: &Path) -> Result<Vec<ModelInfo>, AdapterError> {
        self.state.lock().unwrap().listed += 1;
        let model = |id: &str, name: &str, default: bool, levels: &[&str]| ModelInfo {
            id: id.into(),
            name: name.into(),
            description: format!("{name} of {}", self.id),
            default,
            efforts: levels.iter().map(|l| Effort { id: l.to_string(), description: String::new() }).collect(),
            default_effort: default.then(|| "medium".to_string()),
        };
        Ok(vec![
            model("default", "Default", true, &["low", "medium", "high"]),
            model("fast", "Fast One", false, &["low", "medium"]),
            model("deep", "Deep One", false, &["low", "medium", "high", "max"]),
        ])
    }
    fn write_session(&self, cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), AdapterError> {
        let mut s = self.state.lock().unwrap();
        if !s.copies {
            return Err(AdapterError(format!("{} cannot take a copied conversation.", self.id)));
        }
        s.copied.push(events.len());
        Ok(("copied".into(), cwd.join("copied.jsonl"), 0))
    }
    fn start(&self, opts: StartOpts, sink: Sink) -> Result<Box<dyn Session>, AdapterError> {
        let mut s = self.state.lock().unwrap();
        s.starts.push(opts.resume_session_id.clone());
        s.models.push(opts.model.clone());
        s.efforts.push(opts.effort.clone());
        s.modes.push(opts.permission_mode);
        s.sink = Some(sink.clone());
        Ok(Box::new(FakeSession {
            id: self.id,
            state: self.state.clone(),
            sink,
            resumed: opts.resume_session_id.is_some(),
            announced: false,
        }))
    }
}

impl Session for FakeSession {
    fn send(&mut self, turn: Turn) -> Result<(), AdapterError> {
        let payload = turn.payload();
        let (events, fail) = {
            let mut s = self.state.lock().unwrap();
            s.payloads.push(payload.clone());
            if self.resumed && s.fail_resume {
                (Vec::new(), true)
            } else {
                let mut events = Vec::new();
                if !self.announced && !self.resumed {
                    s.sessions += 1;
                    events.push(AdapterEvent::Session { session_id: format!("{}-s{}", self.id, s.sessions), model: Some("m".into()) });
                }
                self.announced = true;
                events.extend(s.plan.pop_front().unwrap_or_else(|| {
                    let first = payload.lines().next().unwrap_or("").to_string();
                    vec![
                        AdapterEvent::TextStart,
                        AdapterEvent::TextDelta { text: "re".into() },
                        AdapterEvent::Event { event: EventKind::AssistantMessage { text: format!("reply to: {first}"), model: None } },
                        AdapterEvent::Event { event: EventKind::Usage { input_tokens: 1, output_tokens: 1, cost_estimate: None } },
                        AdapterEvent::TurnEnd { error: None },
                    ]
                }));
                (events, false)
            }
        };
        if fail {
            (self.sink)(AdapterEvent::Exit { code: Some(1), detail: "No conversation found with that session id".into() });
        }
        for ev in events {
            (self.sink)(ev);
        }
        Ok(())
    }
    fn interrupt(&mut self) -> bool {
        let mut s = self.state.lock().unwrap();
        s.interrupts += 1;
        s.interrupt_ends_turn
    }
    fn answer(&mut self, call_id: &str, decision: Decision) -> Result<(), AdapterError> {
        self.state.lock().unwrap().answers.push((call_id.to_string(), decision));
        Ok(())
    }
    fn close(&mut self) {}
}

struct Rig {
    engine: Engine,
    rx: Receiver<Msg>,
    out: Receiver<Arc<Vec<u8>>>,
    claude: Arc<Mutex<FakeState>>,
    codex: Arc<Mutex<FakeState>>,
    dir: PathBuf,
    next_id: u64,
    /// The thread the rig's project started with.
    t: String,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(name: &str) -> Rig {
    let dir = std::env::temp_dir().join(format!("desl-engine-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("api");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::in_memory(&dir.join("blobs")).unwrap();
    let (tx, rx) = channel();
    let claude = Arc::new(Mutex::new(FakeState::default()));
    let codex = Arc::new(Mutex::new(FakeState { interrupt_ends_turn: true, ..FakeState::default() }));
    let adapters: Vec<Arc<dyn Adapter>> =
        vec![Arc::new(Fake { id: "claude", state: claude.clone() }), Arc::new(Fake { id: "codex", state: codex.clone() })];
    let mut engine = Engine::new(store, adapters, tx).unwrap();
    let (out_tx, out) = channel();
    engine.handle(Msg::Connected { client: 1, out: out_tx });
    let mut rig = Rig { engine, rx, out, claude, codex, dir, next_id: 0, t: String::new() };
    let Ok(Reply::Workspace(ws)) = rig.cmd(Command::Subscribe { cwd: Some(project.to_string_lossy().to_string()) }) else {
        panic!("subscribe failed")
    };
    rig.t = ws.focus.expect("a first thread");
    rig
}

impl Rig {
    /// Everything the agents reported since the last command.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.engine.handle(msg);
        }
    }

    fn cmd(&mut self, command: Command) -> Outcome {
        self.next_id += 1;
        let id = self.next_id;
        self.engine.handle(Msg::Request { client: 1, request: Request { id, command } });
        self.pump();
        let mut found = None;
        while let Ok(bytes) = self.out.try_recv() {
            let frame: ServerFrame = desl_proto::read_frame(&mut &bytes[..]).unwrap().unwrap();
            if let ServerFrame::Reply { id: got, ok, error } = frame
                && got == id
            {
                found = Some(match (ok, error) {
                    (Some(r), _) => Ok(r),
                    (None, Some(e)) => Err(e),
                    (None, None) => panic!("an empty reply"),
                });
            }
        }
        found.expect("a reply")
    }

    /// A command whose reply is worked out off the engine's thread: waits for it, a while.
    fn cmd_waiting(&mut self, command: Command) -> Outcome {
        self.next_id += 1;
        let id = self.next_id;
        self.engine.handle(Msg::Request { client: 1, request: Request { id, command } });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(msg) = self.rx.recv_timeout(std::time::Duration::from_millis(20)) {
                self.engine.handle(msg);
            }
            while let Ok(bytes) = self.out.try_recv() {
                let frame: ServerFrame = desl_proto::read_frame(&mut &bytes[..]).unwrap().unwrap();
                if let ServerFrame::Reply { id: got, ok, error } = frame
                    && got == id
                {
                    return match (ok, error) {
                        (Some(r), _) => Ok(r),
                        (None, Some(e)) => Err(e),
                        (None, None) => panic!("an empty reply"),
                    };
                }
            }
        }
        panic!("no reply in 5 seconds")
    }

    fn prompt(&mut self, thread: &str, text: &str) {
        self.cmd(Command::Prompt { thread_id: thread.to_string(), text: text.to_string() }).unwrap();
    }

    fn thread(&self, id: &str) -> Thread {
        self.engine.threads.iter().find(|t| t.id == id).unwrap().clone()
    }

    fn events(&self, id: &str) -> Vec<Event> {
        self.engine.store.all_events(id).unwrap()
    }

    fn kinds(&self, id: &str) -> Vec<&'static str> {
        self.events(id).iter().map(|e| e.kind.name()).collect()
    }

    fn emit(&mut self, agent: &Arc<Mutex<FakeState>>, events: Vec<AdapterEvent>) {
        let sink = agent.lock().unwrap().sink.clone().unwrap();
        for ev in events {
            sink(ev);
        }
        self.pump();
    }

    /// The invariants every sequence of commands must keep (SPEC.md "Test strategy").
    fn check_invariants(&self) {
        for t in &self.engine.threads {
            let events = self.events(&t.id);
            let indices: Vec<u64> = events.iter().map(|e| e.index).collect();
            assert_eq!(indices, (1..=t.head).collect::<Vec<_>>(), "indices of {} are dense and start at 1", t.id);
            for b in t.bindings.values() {
                assert!(b.seen_up_to <= t.head, "{}: {} has seen {} of {}", t.id, b.agent, b.seen_up_to, t.head);
            }
            if let (Some(parent), Some(point)) = (&t.parent_id, t.fork_point) {
                let theirs = self.engine.store.events(parent, 1, point).unwrap();
                for (a, b) in theirs.iter().zip(&events) {
                    assert_eq!((a.index, &a.kind), (b.index, &b.kind), "a fork starts with its parent's prefix");
                }
            }
        }
    }
}

fn ev(kind: EventKind) -> AdapterEvent {
    AdapterEvent::Event { event: kind }
}

#[test]
fn a_prompt_becomes_a_turn_and_the_agent_is_marked_up_to_date() {
    let mut r = rig("prompt");
    let t = r.t.clone();
    r.prompt(&t, "Read src/auth/middleware.ts and explain it");
    assert_eq!(r.kinds(&t), ["user_message", "assistant_message", "usage"]);
    let thread = r.thread(&t);
    assert_eq!((thread.status, thread.name.as_str(), thread.head), (Status::Idle, "Read src/auth/middleware.ts and explain it", 3));
    let b = &thread.bindings["claude"];
    assert_eq!((b.session_id.as_deref(), b.seen_up_to), (Some("claude-s1"), 3));
    assert_eq!(r.claude.lock().unwrap().payloads, vec!["Read src/auth/middleware.ts and explain it"]);
    r.check_invariants();
}

#[test]
fn a_switch_sends_a_full_brief_once_and_a_switch_back_sends_only_the_delta() {
    let mut r = rig("switch");
    let t = r.t.clone();
    r.prompt(&t, "first task");
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap();
    assert_eq!(r.thread(&t).active_agent, "codex");
    r.prompt(&t, "continue please");
    let to_codex = r.codex.lock().unwrap().payloads[0].clone();
    assert!(to_codex.starts_with("# Handoff brief\n"), "{to_codex}");
    assert!(to_codex.contains("first task") && to_codex.contains("## Next step\ncontinue please"));
    assert!(r.kinds(&t).contains(&"brief") && r.kinds(&t).contains(&"agent_switch"));

    // A second prompt to the same agent carries nothing extra.
    r.prompt(&t, "and again");
    assert_eq!(r.codex.lock().unwrap().payloads[1], "and again");

    // Back to claude: it resumes its session and gets only what it missed.
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "claude".into() }).unwrap();
    r.prompt(&t, "back to you");
    let to_claude = r.claude.lock().unwrap().payloads[1].clone();
    assert!(to_claude.starts_with("# Handoff brief (delta)\n"), "{to_claude}");
    assert!(to_claude.contains("continue please") && to_claude.contains("and again"));
    assert!(!to_claude.contains("first task"), "claude has already seen its own turn");
    assert_eq!(r.claude.lock().unwrap().starts.len(), 1, "the live session is reused, not restarted");

    // Away and back with nothing said in between: nothing to hand over.
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap();
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "claude".into() }).unwrap();
    r.prompt(&t, "still here");
    assert_eq!(r.claude.lock().unwrap().payloads[2], "still here");
    let thread = r.thread(&t);
    assert_eq!(thread.bindings["claude"].seen_up_to, thread.head);
    r.check_invariants();
}

#[test]
fn switching_is_refused_with_a_sentence_when_it_cannot_happen() {
    let mut r = rig("switch-refused");
    let t = r.t.clone();
    let same = r.cmd(Command::Switch { thread_id: t.clone(), agent: "claude".into() }).unwrap_err();
    assert_eq!(same, "This thread is already on claude.");
    let unknown = r.cmd(Command::Switch { thread_id: t.clone(), agent: "gemini".into() }).unwrap_err();
    assert_eq!(unknown, "desl drives claude and codex, not gemini.", "two agents are joined with and, three with a comma too");
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "a long task");
    let busy = r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap_err();
    assert_eq!(busy, "claude is still working. Wait for it to finish or press ctrl+c, then switch.");
}

#[test]
fn an_approval_waits_for_the_user_and_always_answers_the_next_one_by_policy() {
    let mut r = rig("approval");
    let t = r.t.clone();
    let call = |id: &str| ev(EventKind::ToolCall { id: id.into(), name: tool::BASH.into(), args: json!({ "command": "rm -rf dist/" }) });
    let ask = |id: &str| {
        ev(EventKind::ApprovalRequest { call_id: id.into(), tool: tool::BASH.into(), summary: "rm -rf dist/".into(), risk: Risk::Command })
    };
    r.claude.lock().unwrap().plan.push_back(vec![call("c1"), ask("c1")]);
    r.prompt(&t, "clean up");
    let thread = r.thread(&t);
    assert_eq!(thread.status, Status::Waiting);
    assert_eq!(thread.pending_approval.as_ref().map(|p| p.summary.as_str()), Some("rm -rf dist/"));

    r.cmd(Command::Answer { thread_id: t.clone(), call_id: Some("c1".into()), decision: Decision::Always }).unwrap();
    assert_eq!(r.claude.lock().unwrap().answers, vec![("c1".to_string(), Decision::Always)]);
    assert_eq!(r.thread(&t).status, Status::Working);

    // The same tool asks again: answered at once, logged, the user is not bothered.
    let claude = r.claude.clone();
    r.emit(&claude, vec![call("c2"), ask("c2")]);
    assert_eq!(r.thread(&t).status, Status::Working);
    assert_eq!(r.claude.lock().unwrap().answers.last(), Some(&("c2".to_string(), Decision::Allow)));
    let by: Vec<String> = r
        .events(&t)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ApprovalResponse { by, .. } => Some(by),
            _ => None,
        })
        .collect();
    assert_eq!(by, ["user", "policy"]);

    r.emit(&claude, vec![AdapterEvent::TurnEnd { error: None }]);
    assert_eq!(r.thread(&t).status, Status::Idle);
    let nothing = r.cmd(Command::Answer { thread_id: t.clone(), call_id: None, decision: Decision::Allow }).unwrap_err();
    assert_eq!(nothing, format!("Nothing in {t} is waiting for an answer."));
    r.check_invariants();
}

#[test]
fn a_session_that_cannot_be_resumed_is_replaced_with_a_full_brief() {
    let mut r = rig("resume-fails");
    let t = r.t.clone();
    r.prompt(&t, "first task");
    // The daemon restarted: the process is gone, the native session id is still bound.
    r.engine.live.clear();
    r.claude.lock().unwrap().fail_resume = true;
    r.prompt(&t, "second task");
    let s = r.claude.lock().unwrap();
    assert_eq!(s.starts, vec![None, Some("claude-s1".to_string()), None], "resume was tried, then a fresh session started");
    assert!(s.payloads[2].starts_with("# Handoff brief\n") && s.payloads[2].contains("## Next step\nsecond task"));
    drop(s);
    let prompts = r.events(&t).iter().filter(|e| matches!(&e.kind, EventKind::UserMessage { text, .. } if text == "second task")).count();
    assert_eq!(prompts, 1, "the prompt is in the log once");
    let thread = r.thread(&t);
    assert_eq!((thread.status, thread.bindings["claude"].session_id.as_deref()), (Status::Idle, Some("claude-s2")));
    assert!(r.events(&t).iter().any(
        |e| matches!(&e.kind, EventKind::Error { message, recoverable: true } if message.starts_with("Could not resume the claude session"))
    ));
    r.check_invariants();
}

#[test]
fn a_fork_has_the_parents_history_up_to_the_point_and_a_fresh_agent() {
    let mut r = rig("fork");
    let t = r.t.clone();
    r.prompt(&t, "first task");
    r.prompt(&t, "second task");
    let before = r.events(&t);
    let Reply::Created { thread_id: f } =
        r.cmd(Command::Fork { thread_id: t.clone(), agent: Some("codex".into()), at: Some(3), worktree: false }).unwrap()
    else {
        panic!()
    };
    assert_eq!(f, format!("{t}/1"));
    let fork = r.thread(&f);
    assert_eq!((fork.parent_id.as_deref(), fork.fork_point, fork.active_agent.as_str()), (Some(t.as_str()), Some(3), "codex"));
    assert!(fork.bindings.values().all(|b| b.session_id.is_none() && b.seen_up_to == 0), "choices carry over, sessions do not");
    assert_eq!(r.kinds(&f), ["user_message", "assistant_message", "usage", "fork"]);
    assert_eq!(r.events(&t), before, "the parent is untouched");

    r.prompt(&f, "try it another way");
    let brief = r.codex.lock().unwrap().payloads[0].clone();
    assert!(brief.starts_with("# Handoff brief\n") && brief.contains("first task") && !brief.contains("second task"));
    let Reply::Created { thread_id: g } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None, worktree: false }).unwrap()
    else {
        panic!()
    };
    assert_eq!(g, format!("{t}/2"));
    let bad = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: Some(99), worktree: false }).unwrap_err();
    assert_eq!(bad, format!("{t} has events 1 to 6; 99 is not one of them."));
    r.check_invariants();
}

#[test]
fn prompts_sent_while_the_agent_works_wait_their_turn_in_order() {
    let mut r = rig("queue");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "one");
    assert!(matches!(r.cmd(Command::Prompt { thread_id: t.clone(), text: "two".into() }), Ok(Reply::Sent { queued: true })));
    assert!(matches!(r.cmd(Command::Prompt { thread_id: t.clone(), text: "three".into() }), Ok(Reply::Sent { queued: true })));
    assert_eq!(r.thread(&t).queued, ["two", "three"]);
    let claude = r.claude.clone();
    r.emit(&claude, vec![AdapterEvent::TurnEnd { error: None }]);
    assert_eq!(r.claude.lock().unwrap().payloads, ["one", "two", "three"]);
    let thread = r.thread(&t);
    assert!(thread.queued.is_empty() && thread.status == Status::Idle);
    r.check_invariants();
}

#[test]
fn an_interrupt_is_recorded_and_ends_the_turn() {
    let mut r = rig("interrupt");
    let t = r.t.clone();
    // Claude confirms an interrupt with the end of its turn.
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "a long task");
    r.cmd(Command::Interrupt { thread_id: t.clone() }).unwrap();
    assert_eq!((r.thread(&t).status, r.claude.lock().unwrap().interrupts), (Status::Working, 1));
    let claude = r.claude.clone();
    r.emit(&claude, vec![AdapterEvent::TurnEnd { error: None }]);
    assert_eq!(r.thread(&t).status, Status::Idle);
    assert_eq!(r.kinds(&t), ["user_message", "interrupt"]);

    // Codex is stopped by ending its process: the turn is over at once.
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap();
    r.codex.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "another long task");
    r.cmd(Command::Interrupt { thread_id: t.clone() }).unwrap();
    assert_eq!(r.thread(&t).status, Status::Idle);
    let idle = r.cmd(Command::Interrupt { thread_id: t.clone() }).unwrap_err();
    assert_eq!(idle, "codex is not working, so there is nothing to interrupt.");
    r.check_invariants();
}

#[test]
fn an_agent_that_dies_mid_turn_fails_the_thread_and_the_next_prompt_resumes_it() {
    let mut r = rig("crash");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(vec![ev(EventKind::AssistantMessage { text: "Working on it.".into(), model: None })]);
    r.prompt(&t, "do the thing");
    let claude = r.claude.clone();
    r.emit(&claude, vec![AdapterEvent::Exit { code: Some(137), detail: "killed".into() }]);
    let thread = r.thread(&t);
    assert_eq!(thread.status, Status::Failed);
    let last = r.events(&t).pop().unwrap();
    assert_eq!(
        last.kind,
        EventKind::Error {
            message: "claude exited with 137 mid-turn. It said: killed. Send your prompt again to resume the session.".into(),
            recoverable: true
        }
    );
    r.prompt(&t, "do the thing");
    assert_eq!(r.claude.lock().unwrap().starts.last(), Some(&Some("claude-s1".to_string())), "the native session is resumed");
    assert_eq!(r.thread(&t).status, Status::Idle);
    r.check_invariants();
}

#[test]
fn a_failed_turn_is_an_error_in_the_log_and_the_thread_is_ready_again() {
    let mut r = rig("turn-error");
    let t = r.t.clone();
    r.claude
        .lock()
        .unwrap()
        .plan
        .push_back(vec![AdapterEvent::TurnEnd { error: Some("claude stopped: rate limited. Send your prompt again to retry.".into()) }]);
    r.prompt(&t, "hello");
    assert_eq!(r.kinds(&t), ["user_message", "error"]);
    assert_eq!(r.thread(&t).status, Status::Idle);
}

#[test]
fn wait_answers_when_the_thread_needs_attention() {
    let mut r = rig("wait");
    let t = r.t.clone();
    assert!(matches!(r.cmd(Command::Wait { thread_id: t.clone() }), Ok(Reply::Waited { status: Status::Idle })));
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "a long task");
    r.next_id += 1;
    let id = r.next_id;
    r.engine.handle(Msg::Request { client: 1, request: Request { id, command: Command::Wait { thread_id: t.clone() } } });
    assert!(
        r.out.try_iter().all(
            |b| !matches!(desl_proto::read_frame::<_, ServerFrame>(&mut &b[..]), Ok(Some(ServerFrame::Reply { id: got, .. })) if got == id)
        ),
        "no reply while working"
    );
    let claude = r.claude.clone();
    r.emit(&claude, vec![AdapterEvent::TurnEnd { error: None }]);
    let replied = r.out.try_iter().any(|b| matches!(desl_proto::read_frame::<_, ServerFrame>(&mut &b[..]), Ok(Some(ServerFrame::Reply { id: got, ok: Some(Reply::Waited { status: Status::Idle }), .. })) if got == id));
    assert!(replied, "the waiter hears when the turn ends");
}

#[test]
fn a_daemon_that_restarts_mid_turn_says_so_and_leaves_the_thread_idle() {
    let mut r = rig("restart");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "a long task");
    assert_eq!(r.thread(&t).status, Status::Working);
    // A new engine over the same store, as after a crash.
    let (tx, _rx) = channel();
    let store = std::mem::replace(&mut r.engine.store, Store::in_memory(&r.dir.join("blobs2")).unwrap());
    let engine = Engine::new(store, vec![Arc::new(Fake { id: "claude", state: r.claude.clone() })], tx).unwrap();
    let thread = engine.threads.iter().find(|x| x.id == t).unwrap();
    assert_eq!(thread.status, Status::Idle);
    let last = engine.store.all_events(&t).unwrap().pop().unwrap();
    assert!(matches!(last.kind, EventKind::Error { ref message, recoverable: true } if message.starts_with("The desl daemon restarted")));
}

#[test]
fn clear_starts_a_fresh_session_without_a_brief_of_the_old_history() {
    let mut r = rig("clear");
    let t = r.t.clone();
    r.prompt(&t, "first task");
    r.cmd(Command::Clear { thread_id: t.clone() }).unwrap();
    r.prompt(&t, "something new");
    let s = r.claude.lock().unwrap();
    assert_eq!(s.starts, vec![None, None], "a new native session");
    assert_eq!(s.payloads[1], "something new", "history stays in desl, not in the new context");
}

/// Random sequences of prompt, switch and fork keep the invariants (SPEC.md "Test strategy").
#[test]
fn random_sequences_keep_indices_dense_and_marks_behind_the_head() {
    let mut r = rig("random");
    let mut seed: u64 = 0x5eed_1234_abcd_ef01;
    let mut next = move |n: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % n
    };
    for step in 0..120 {
        let ids: Vec<String> = r.engine.threads.iter().map(|t| t.id.clone()).collect();
        let t = ids[next(ids.len() as u64) as usize].clone();
        match next(10) {
            0..=5 => r.prompt(&t, &format!("prompt {step}")),
            6..=7 => {
                let to = if r.thread(&t).active_agent == "claude" { "codex" } else { "claude" };
                r.cmd(Command::Switch { thread_id: t.clone(), agent: to.into() }).unwrap();
            }
            8 if ids.len() < 6 && r.thread(&t).head > 0 => {
                let at = 1 + next(r.thread(&t).head);
                r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: Some(at), worktree: false }).unwrap();
            }
            _ => {
                // The daemon restarts: every process is gone, every binding stays.
                r.engine.live.clear();
            }
        }
        r.check_invariants();
    }
    assert!(r.engine.threads.len() > 1, "some forks happened");
}

impl Rig {
    /// A command whose reply arrives later, from another thread.
    fn cmd_async(&mut self, command: Command) -> Outcome {
        self.next_id += 1;
        let id = self.next_id;
        self.engine.handle(Msg::Request { client: 1, request: Request { id, command } });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(msg) = self.rx.recv_timeout(std::time::Duration::from_millis(20)) {
                self.engine.handle(msg);
            }
            while let Ok(bytes) = self.out.try_recv() {
                let frame: ServerFrame = desl_proto::read_frame(&mut &bytes[..]).unwrap().unwrap();
                if let ServerFrame::Reply { id: got, ok, error } = frame
                    && got == id
                {
                    return ok.ok_or_else(|| error.unwrap_or_default());
                }
            }
            assert!(std::time::Instant::now() < deadline, "no reply");
        }
    }
}

#[test]
fn the_model_list_is_asked_once_and_marks_what_the_thread_uses() {
    let mut r = rig("models");
    let t = r.t.clone();
    let Reply::Models { agent, models, current, .. } = r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap() else { panic!() };
    assert_eq!((agent.as_str(), models.len(), current.as_deref()), ("claude", 3, Some("default")));
    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("Fast One".into()) }).unwrap();
    let Reply::Models { current, .. } = r.cmd(Command::Models { thread_id: t.clone() }).unwrap() else { panic!() };
    assert_eq!(current.as_deref(), Some("fast"), "a name is as good as an id");
    assert_eq!(r.claude.lock().unwrap().listed, 1, "the second answer came from the daemon's memory");
}

#[test]
fn a_new_model_reaches_the_agent_with_the_next_prompt_and_keeps_the_session() {
    let mut r = rig("set-model");
    let t = r.t.clone();
    r.prompt(&t, "first");
    r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap();
    let Reply::ModelSet { agent, model, .. } = r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("deep".into()) }).unwrap()
    else {
        panic!()
    };
    assert_eq!((agent.as_str(), model.map(|m| m.name)), ("claude", Some("Deep One".to_string())));
    r.prompt(&t, "second");
    let s = r.claude.lock().unwrap();
    assert_eq!(s.models, vec![None, Some("deep".to_string())]);
    assert_eq!(s.starts, vec![None, Some("claude-s1".to_string())], "the same native session, resumed on the new model");
    assert_eq!(s.payloads[1], "second", "a model change is not a handoff: no brief");
    drop(s);
    assert_eq!(r.thread(&t).bindings["claude"].model_choice.as_deref(), Some("deep"));

    // Back to the default: the choice is cleared, not pinned.
    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("default".into()) }).unwrap();
    assert_eq!(r.thread(&t).bindings["claude"].model_choice, None);
    let unknown = r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("gpt-9".into()) }).unwrap_err();
    assert_eq!(unknown, "claude has no model named gpt-9. It offers default, fast, deep.");
}

#[test]
fn each_agent_of_a_thread_keeps_its_own_model() {
    let mut r = rig("model-per-agent");
    let t = r.t.clone();
    r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap();
    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("fast".into()) }).unwrap();
    r.prompt(&t, "one");
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap();
    let Reply::Models { agent, current, .. } = r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap() else { panic!() };
    assert_eq!((agent.as_str(), current.as_deref()), ("codex", Some("default")), "codex was never told to use claude's pick");
    r.prompt(&t, "two");
    assert_eq!(r.codex.lock().unwrap().models, vec![None]);
    assert_eq!(r.claude.lock().unwrap().models, vec![Some("fast".to_string())]);
}

#[test]
fn an_effort_reaches_the_agent_with_the_next_prompt_and_keeps_the_session() {
    let mut r = rig("set-effort");
    let t = r.t.clone();
    r.prompt(&t, "first");
    r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap();
    let Reply::ModelSet { agent, model, effort } = r.cmd(Command::SetEffort { thread_id: t.clone(), effort: Some("High".into()) }).unwrap()
    else {
        panic!()
    };
    assert_eq!((agent.as_str(), model, effort.as_deref()), ("claude", None, Some("high")), "the default model, thinking harder");
    r.prompt(&t, "second");
    {
        let s = r.claude.lock().unwrap();
        assert_eq!(s.efforts, vec![None, Some("high".to_string())]);
        assert_eq!(s.starts, vec![None, Some("claude-s1".to_string())], "the same native session, resumed at the new effort");
        assert_eq!(s.payloads[1], "second", "an effort change is not a handoff: no brief");
    }
    let Reply::Models { effort, .. } = r.cmd(Command::Models { thread_id: t.clone() }).unwrap() else { panic!() };
    assert_eq!(effort.as_deref(), Some("high"), "the list says what the thread uses");

    // A level the model does not take is refused here, not by the agent in the middle of a turn.
    let unknown = r.cmd(Command::SetEffort { thread_id: t.clone(), effort: Some("ultra".into()) }).unwrap_err();
    assert_eq!(unknown, "Default has no effort level named ultra. It takes low, medium, high.");
    assert_eq!(r.thread(&t).bindings["claude"].effort_choice.as_deref(), Some("high"));

    // Back to the agent's own setting: the choice is cleared, not pinned.
    r.cmd(Command::SetEffort { thread_id: t.clone(), effort: Some("default".into()) }).unwrap();
    assert_eq!(r.thread(&t).bindings["claude"].effort_choice, None);
}

#[test]
fn an_effort_the_new_model_does_not_take_goes_with_the_old_model() {
    let mut r = rig("effort-per-model");
    let t = r.t.clone();
    r.cmd_async(Command::Models { thread_id: t.clone() }).unwrap();
    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("deep".into()) }).unwrap();
    r.cmd(Command::SetEffort { thread_id: t.clone(), effort: Some("low".into()) }).unwrap();
    let Reply::ModelSet { effort, .. } = r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("fast".into()) }).unwrap() else {
        panic!()
    };
    assert_eq!(effort.as_deref(), Some("low"), "a level both models take is kept");

    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("deep".into()) }).unwrap();
    r.cmd(Command::SetEffort { thread_id: t.clone(), effort: Some("max".into()) }).unwrap();
    let Reply::ModelSet { effort, .. } = r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("fast".into()) }).unwrap() else {
        panic!()
    };
    assert_eq!(effort, None, "fast has no max: the thread is back on the agent's own setting");
    assert_eq!(r.thread(&t).bindings["claude"].effort_choice, None);
    r.prompt(&t, "go");
    let s = r.claude.lock().unwrap();
    assert_eq!((s.models.last().unwrap().as_deref(), s.efforts.last().unwrap().as_deref()), (Some("fast"), None));
}

fn rm_call(id: &str) -> Vec<AdapterEvent> {
    vec![
        ev(EventKind::ToolCall { id: id.into(), name: tool::BASH.into(), args: json!({ "command": "rm -rf dist/" }) }),
        ev(EventKind::ApprovalRequest { call_id: id.into(), tool: tool::BASH.into(), summary: "rm -rf dist/".into(), risk: Risk::Command }),
    ]
}

fn answered_by(r: &Rig, thread: &str) -> Vec<(Decision, String)> {
    r.events(thread)
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::ApprovalResponse { decision, by, .. } => Some((decision, by)),
            _ => None,
        })
        .collect()
}

#[test]
fn in_yolo_every_request_is_allowed_by_policy_and_still_logged() {
    let mut r = rig("yolo");
    let t = r.t.clone();
    let Reply::ModeSet { mode, meaning } = r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Yolo }).unwrap() else {
        panic!()
    };
    assert_eq!((mode, meaning.as_str()), (PermissionMode::Yolo, "everything is allowed, nothing asks"));
    r.claude.lock().unwrap().plan.push_back(rm_call("c1"));
    r.prompt(&t, "clean up");
    let thread = r.thread(&t);
    assert_eq!((thread.status, thread.pending_approval), (Status::Working, None), "the user is not asked");
    assert_eq!(r.claude.lock().unwrap().answers, vec![("c1".to_string(), Decision::Allow)]);
    assert_eq!(answered_by(&r, &t), [(Decision::Allow, "policy".to_string())], "what yolo allowed is in the log");
    assert_eq!(r.engine.store.threads().unwrap()[0].permission_mode, PermissionMode::Yolo, "the mode outlives the daemon");
}

#[test]
fn going_yolo_answers_the_question_that_is_on_screen() {
    let mut r = rig("yolo-pending");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(rm_call("c1"));
    r.prompt(&t, "clean up");
    assert_eq!(r.thread(&t).status, Status::Waiting);
    r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Yolo }).unwrap();
    let thread = r.thread(&t);
    assert_eq!((thread.status, thread.pending_approval), (Status::Working, None));
    assert_eq!(r.claude.lock().unwrap().answers, vec![("c1".to_string(), Decision::Allow)]);
    assert_eq!(answered_by(&r, &t), [(Decision::Allow, "policy".to_string())]);

    // Back to asking: the next request waits for the user again.
    r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Edits }).unwrap();
    let claude = r.claude.clone();
    r.emit(&claude, rm_call("c2"));
    assert_eq!(r.thread(&t).status, Status::Waiting);
}

#[test]
fn a_new_mode_reaches_the_agent_with_its_next_turn_and_never_cuts_one_short() {
    let mut r = rig("mode-flags");
    let t = r.t.clone();
    r.prompt(&t, "first");
    r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Ask }).unwrap();
    r.prompt(&t, "second");
    {
        let s = r.claude.lock().unwrap();
        assert_eq!(s.modes, vec![PermissionMode::Edits, PermissionMode::Ask]);
        assert_eq!(s.starts, vec![None, Some("claude-s1".to_string())], "the same native session, resumed under the new mode");
        assert_eq!(s.payloads[1], "second", "a mode change is not a handoff: no brief");
    }

    // The same mode again changes nothing: the agent is not restarted for it.
    r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Ask }).unwrap();
    r.claude.lock().unwrap().plan.push_back(vec![AdapterEvent::TextStart]);
    r.prompt(&t, "third");
    assert_eq!(r.claude.lock().unwrap().starts.len(), 2);

    // A change in the middle of a turn: the turn runs on to its end, the next one starts fresh.
    assert_eq!(r.thread(&t).status, Status::Working);
    r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Edits }).unwrap();
    let claude = r.claude.clone();
    r.emit(
        &claude,
        vec![ev(EventKind::AssistantMessage { text: "finished all the same".into(), model: None }), AdapterEvent::TurnEnd { error: None }],
    );
    assert_eq!(r.thread(&t).status, Status::Idle);
    assert!(r.events(&t).iter().any(|e| matches!(&e.kind, EventKind::AssistantMessage { text, .. } if text == "finished all the same")));
    r.prompt(&t, "fourth");
    let s = r.claude.lock().unwrap();
    assert_eq!((s.starts.len(), s.modes.last()), (3, Some(&PermissionMode::Edits)));
}

#[test]
fn a_fork_keeps_ask_and_never_inherits_yolo() {
    let mut r = rig("mode-fork");
    let t = r.t.clone();
    r.prompt(&t, "one");
    let fork_in = |r: &mut Rig, mode| {
        r.cmd(Command::SetMode { thread_id: t.clone(), mode }).unwrap();
        let Reply::Created { thread_id } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None, worktree: false }).unwrap()
        else {
            panic!()
        };
        r.thread(&thread_id).permission_mode
    };
    assert_eq!(fork_in(&mut r, PermissionMode::Yolo), PermissionMode::Edits, "yolo is set by hand, per thread");
    assert_eq!(fork_in(&mut r, PermissionMode::Ask), PermissionMode::Ask);
}

#[test]
fn an_agent_that_cannot_ask_says_what_it_does_instead() {
    let mut r = rig("mode-notes");
    let t = r.t.clone();
    let meaning = |r: &mut Rig, mode| match r.cmd(Command::SetMode { thread_id: t.clone(), mode }).unwrap() {
        Reply::ModeSet { meaning, .. } => meaning,
        _ => panic!(),
    };
    assert_eq!(meaning(&mut r, PermissionMode::Ask), "every edit and command asks first");
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).unwrap();
    assert_eq!(meaning(&mut r, PermissionMode::Ask), "reads only, because codex cannot ask");
    assert_eq!(meaning(&mut r, PermissionMode::Edits), "file edits are allowed, commands ask");
    let Reply::Agents { agents } = r.cmd(Command::Agents { refresh: false }).unwrap() else { panic!() };
    assert_eq!(agents[1].mode_notes.len(), 1, "the list of agents carries the note, so a client can show it before the pick");

    let refused = r.cmd(Command::SetMode { thread_id: t.clone(), mode: PermissionMode::Worktree }).unwrap_err();
    assert_eq!(refused, "The worktree mode arrives with worktrees. Pick one of ask, edits or yolo.");
}

#[test]
fn a_thread_with_a_worktree_works_in_it_and_closing_keeps_its_branch() {
    let mut r = rig("worktree");
    let project = r.dir.join("api");
    let cwd = project.to_string_lossy().to_string();
    let new = |worktree| Command::NewThread { cwd: cwd.clone(), agent: Some("claude".into()), prompt: None, beside: None, worktree };
    let refused = r.cmd(new(true)).unwrap_err();
    assert!(refused.contains("git repository"), "{refused}");
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git").arg("-C").arg(&project).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project.join("a.txt"), "a").unwrap();
    git(&["add", "."]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "one"]);
    let Ok(Reply::Created { thread_id }) = r.cmd(new(true)) else { panic!("no thread") };
    let t = r.thread(&thread_id);
    let wt = project.join(".worktrees").join(&thread_id);
    assert_eq!((t.worktree.as_deref(), t.cwd.as_str()), (Some(thread_id.as_str()), wt.to_string_lossy().as_ref()));
    assert_eq!(t.git.branch.as_deref(), Some(format!("desl/{thread_id}").as_str()));
    let Ok(Reply::Removed { note: Some(note) }) = r.cmd(Command::RemoveThread { thread_id: thread_id.clone() }) else {
        panic!("not removed")
    };
    assert!(note.contains("Removed its worktree") && !wt.exists(), "{note}");
    // A worktree with work in it stays.
    let Ok(Reply::Created { thread_id: kept }) = r.cmd(new(true)) else { panic!("no thread") };
    let kept_dir = project.join(".worktrees").join(&kept);
    std::fs::write(kept_dir.join("draft.txt"), "wip").unwrap();
    let Ok(Reply::Removed { note: Some(note) }) = r.cmd(Command::RemoveThread { thread_id: kept }) else { panic!("not removed") };
    assert!(note.contains("uncommitted") && kept_dir.join("draft.txt").exists(), "{note}");
}

#[test]
fn the_agents_own_title_names_the_thread_until_the_user_names_it() {
    let mut r = rig("title");
    let t = r.t.clone();
    r.prompt(&t, "ok let me ask you this. herdr when it runs agents");
    let i = r.engine.idx(&t).unwrap();
    assert!(r.engine.retitle(i, "Herdr API awareness in desl"));
    assert_eq!(r.thread(&t).name, "Herdr API awareness in desl");
    assert!(!r.engine.retitle(i, "Herdr API awareness in desl"), "the same title changes nothing");
    r.cmd(Command::Rename { thread_id: t.clone(), name: "skills".into() }).unwrap();
    assert!(r.thread(&t).named);
    assert!(!r.engine.retitle(i, "Something else"), "a name the user gave wins");
    assert_eq!(r.thread(&t).name, "skills");
    let stored = r.engine.store.threads().unwrap().into_iter().find(|x| x.id == t).unwrap();
    assert!(stored.named, "the store keeps that the user named it");
}

#[test]
fn exit_ends_only_a_running_agent_and_never_a_shell() {
    let mut r = rig("exit");
    let t = r.t.clone();
    let err = r.cmd(Command::Exit { thread_id: t.clone() }).unwrap_err();
    assert!(err.contains("is not running") && err.contains("next prompt starts it"), "{err}");
    let Ok(Reply::Created { thread_id: shell }) = r.cmd(Command::NewThread {
        cwd: r.thread(&t).cwd,
        agent: Some(SHELL.into()),
        prompt: None,
        beside: Some(t.clone()),
        worktree: false,
    }) else {
        panic!("a shell thread")
    };
    let err = r.cmd(Command::Exit { thread_id: shell.clone() }).unwrap_err();
    assert!(err.contains("is a shell") && err.contains("desl thread rm"), "{err}");
}

#[test]
fn restart_needs_a_running_agent_and_never_a_shell() {
    let mut r = rig("restart");
    let t = r.t.clone();
    let err = r.cmd(Command::Restart { thread_id: t.clone() }).unwrap_err();
    assert!(err.contains("is not running") && err.contains("next prompt starts it"), "{err}");
    let Ok(Reply::Created { thread_id: shell }) = r.cmd(Command::NewThread {
        cwd: r.thread(&t).cwd,
        agent: Some(SHELL.into()),
        prompt: None,
        beside: Some(t.clone()),
        worktree: false,
    }) else {
        panic!("a shell thread")
    };
    let err = r.cmd(Command::Restart { thread_id: shell.clone() }).unwrap_err();
    assert!(err.contains("is a shell"), "{err}");
}

#[test]
fn a_files_thread_opens_where_the_thread_beside_it_is_and_has_no_conversation() {
    let mut r = rig("files");
    let t = r.t.clone();
    // A program every machine has stands in for yazi.
    r.engine.file_manager = "sh".into();
    let Ok(Reply::Workspace(ws)) = r.cmd(Command::Workspace) else { panic!("the workspace") };
    assert_eq!(ws.file_manager.as_deref(), Some("sh"), "an installed file manager is offered");
    // The thread on screen works in a directory of its own, as one with a worktree does.
    let project = r.thread(&t).cwd;
    let sub = PathBuf::from(&project).join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let i = r.engine.idx(&t).unwrap();
    r.engine.threads[i].cwd = sub.to_string_lossy().into_owned();
    let Ok(Reply::Created { thread_id: files }) =
        r.cmd(Command::NewThread { cwd: project, agent: Some(FILES.into()), prompt: None, beside: Some(t.clone()), worktree: false })
    else {
        panic!("a files thread")
    };
    let f = r.thread(&files);
    assert_eq!((f.active_agent.as_str(), f.name.as_str()), (FILES, "sh"), "titled by its program, as a shell is");
    assert_eq!(f.cwd, sub.to_string_lossy(), "in the directory of the thread it was opened from");
    assert_eq!(f.project_id, r.thread(&t).project_id);
    for (command, says) in [
        (Command::Prompt { thread_id: files.clone(), text: "hello".into() }, "A file manager thread has no agent to prompt"),
        (Command::Switch { thread_id: files.clone(), agent: "codex".into() }, "A file manager has no conversation to switch"),
        (Command::Fork { thread_id: files.clone(), agent: None, at: None, worktree: false }, "A file manager has no conversation to fork"),
        (Command::Restart { thread_id: files.clone() }, "is a file manager"),
    ] {
        let err = r.cmd(command).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
}

#[test]
fn a_file_manager_that_is_not_installed_is_not_offered() {
    let mut r = rig("nofiles");
    let t = r.t.clone();
    r.engine.file_manager = "desl-no-such-file-manager".into();
    let Ok(Reply::Workspace(ws)) = r.cmd(Command::Workspace) else { panic!("the workspace") };
    assert_eq!(ws.file_manager, None);
    let err = r
        .cmd(Command::NewThread { cwd: r.thread(&t).cwd, agent: Some(FILES.into()), prompt: None, beside: Some(t), worktree: false })
        .unwrap_err();
    assert!(err.contains("is not installed") && err.contains("Install desl-no-such-file-manager"), "{err}");
    assert!(r.engine.threads.iter().all(|t| t.active_agent != FILES), "no thread is left behind");
}

#[test]
fn with_three_agents_a_switch_must_name_one_and_sentences_list_them_all() {
    let dir = std::env::temp_dir().join(format!("desl-engine-test-{}-three", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let project = dir.join("api");
    std::fs::create_dir_all(&project).unwrap();
    let store = Store::in_memory(&dir.join("blobs")).unwrap();
    let (tx, _rx) = channel();
    let fake = |id: &'static str| -> Arc<dyn Adapter> { Arc::new(Fake { id, state: Arc::new(Mutex::new(FakeState::default())) }) };
    let engine = Engine::new(store, vec![fake("claude"), fake("codex"), fake("copilot")], tx).unwrap();
    assert_eq!(engine.usable("gemini").unwrap_err(), "desl drives claude, codex and copilot, not gemini.");
    assert_eq!(engine.other_agent("claude").unwrap_err(), "Say which agent: codex or copilot.");
    assert_eq!(engine.default_agent("none").unwrap(), "claude", "the first that is ready, in the order the menus list them");
}

#[test]
fn a_project_moves_past_the_ones_the_sidebar_lists_and_the_order_is_kept() {
    let mut r = rig("move-project");
    // Three more folders: `web` and `docs` get a thread, `empty` is only registered.
    let dir = |name: &str| {
        let p = r.dir.join(name);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().into_owned()
    };
    let (web, empty, docs) = (dir("web"), dir("empty"), dir("docs"));
    r.cmd(Command::Subscribe { cwd: Some(web) }).unwrap();
    r.engine.project_for(Path::new(&empty)).unwrap();
    r.cmd(Command::Subscribe { cwd: Some(docs) }).unwrap();
    let order = |r: &Rig| r.engine.projects.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    assert_eq!(order(&r), ["api", "web", "empty", "docs"]);
    // Up from the last row lands above `web`: `empty` has no row to stop at.
    r.cmd(Command::MoveProject { project_id: "docs".into(), up: true }).unwrap();
    assert_eq!(order(&r), ["api", "docs", "web", "empty"]);
    r.cmd(Command::MoveProject { project_id: "docs".into(), up: true }).unwrap();
    assert_eq!(order(&r), ["docs", "api", "web", "empty"]);
    let err = r.cmd(Command::MoveProject { project_id: "docs".into(), up: true }).unwrap_err();
    assert_eq!(err, "docs is already the first project.");
    let err = r.cmd(Command::MoveProject { project_id: "web".into(), up: false }).unwrap_err();
    assert_eq!(err, "web is already the last project.", "nothing listed is below it");
    assert!(r.cmd(Command::MoveProject { project_id: "nope".into(), up: true }).unwrap_err().starts_with("There is no project nope."));
    let saved = r.engine.store.meta_get(PROJECT_ORDER).unwrap().unwrap();
    assert_eq!(saved, r#"["docs","api","web","empty"]"#);
}

#[test]
fn the_projects_come_back_in_the_order_the_user_gave_them_and_new_ones_last() {
    let dir = std::env::temp_dir().join(format!("desl-engine-test-{}-order", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = Store::in_memory(&dir.join("blobs")).unwrap();
    for (n, id) in ["api", "web", "docs", "late"].iter().enumerate() {
        let project =
            Project { id: (*id).into(), path: format!("/src/{id}"), default_agent: None, created_at: n as i64, git: GitState::default() };
        store.put_project(&project).unwrap();
    }
    // Saved before `late` was registered.
    store.meta_set(PROJECT_ORDER, r#"["docs","api","web"]"#).unwrap();
    let (tx, _rx) = channel();
    let engine = Engine::new(store, vec![Arc::new(Fake { id: "claude", state: Arc::new(Mutex::new(FakeState::default())) })], tx).unwrap();
    assert_eq!(engine.projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["docs", "api", "web", "late"]);
}

/// Handles what arrives until `done` says so, for up to five seconds.
fn until(r: &mut Rig, what: &str, done: impl Fn(&Engine) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(&r.engine) {
        assert!(Instant::now() < deadline, "{what}");
        if let Ok(msg) = r.rx.recv_timeout(Duration::from_millis(50)) {
            r.engine.handle(msg);
        }
    }
}

fn drew(engine: &Engine, thread: &str, text: &str) -> bool {
    engine.ptys.get(thread).is_some_and(|(_, p)| String::from_utf8_lossy(p.log()).contains(text))
}

#[test]
fn the_task_list_is_one_thread_per_project_opened_at_its_root() {
    let mut r = rig("tasks");
    let t = r.t.clone();
    // A program every machine has stands in for `desl task ui`, and says which root it was given.
    r.engine.tasks_program = vec!["sh".into(), "-c".into(), "echo \"the list of $2\"; cat".into(), "sh".into()];
    let project = r.thread(&t).cwd;
    // The thread beside it works in a directory of its own, as one with a worktree does.
    let sub = PathBuf::from(&project).join("src");
    std::fs::create_dir_all(&sub).unwrap();
    let i = r.engine.idx(&t).unwrap();
    r.engine.threads[i].cwd = sub.to_string_lossy().into_owned();
    let open = |beside: &str| Command::NewThread {
        cwd: sub.to_string_lossy().into_owned(),
        agent: Some(TASKS.into()),
        prompt: None,
        beside: Some(beside.to_string()),
        worktree: false,
    };
    let Ok(Reply::Created { thread_id: tasks }) = r.cmd(open(&t)) else { panic!("a tasks thread") };
    let k = r.thread(&tasks);
    assert_eq!((k.active_agent.as_str(), k.name.as_str()), (TASKS, "tasks"));
    assert_eq!((k.cwd.as_str(), k.project_id.as_str()), (project.as_str(), r.thread(&t).project_id.as_str()), "at the project's root");
    let Ok(Reply::Created { thread_id: again }) = r.cmd(open(&tasks)) else { panic!("the same list") };
    assert_eq!(again, tasks, "asked again, from anywhere in the project, it is the same thread");
    assert_eq!(r.engine.threads.iter().filter(|t| t.active_agent == TASKS).count(), 1);
    r.cmd(Command::Attach { thread_id: tasks.clone(), rows: 10, cols: 60, answers: term::Answers::default() }).unwrap();
    until(&mut r, "the list starts for the project's root", |e| drew(e, &tasks, &format!("the list of {project}")));
    let err = r.cmd(Command::Prompt { thread_id: tasks.clone(), text: "hello".into() }).unwrap_err();
    assert!(err.contains("A task list thread has no agent to prompt"), "{err}");
    let err = r.cmd(Command::Fork { thread_id: tasks, agent: None, at: None, worktree: false }).unwrap_err();
    assert!(err.contains("A task list has no conversation to fork"), "{err}");
}

#[test]
fn a_task_list_made_by_anyone_shows_in_the_sidebar_and_stays_while_its_file_does() {
    let mut r = rig("tasklist");
    let t = r.t.clone();
    r.engine.tasks_program = vec!["cat".into()];
    let project = r.thread(&t).cwd;
    let lists = |r: &Rig| r.engine.threads.iter().filter(|t| t.active_agent == TASKS).map(|t| t.id.clone()).collect::<Vec<_>>();
    r.cmd(Command::Workspace).unwrap();
    assert!(lists(&r).is_empty(), "no file, no list");
    // An agent starts the list with `desl task add`: the file appears behind the daemon's back.
    std::fs::write(PathBuf::from(&project).join(desl_ir::TASK_LIST), "## first\n").unwrap();
    let Ok(Reply::Workspace(ws)) = r.cmd(Command::Workspace) else { panic!("the workspace") };
    let id = lists(&r).pop().expect("the project's list is a thread now");
    assert!(
        ws.threads.iter().any(|x| x.id == id && x.name == "tasks" && x.project_id == r.thread(&t).project_id),
        "a client attaching sees it"
    );
    assert!(!r.engine.ptys.contains_key(&id), "nothing runs until it is shown");
    r.cmd(Command::Workspace).unwrap();
    assert_eq!(lists(&r), vec![id.clone()], "once");
    // q in the list ends its program; the row stays while the file is there.
    r.cmd(Command::Attach { thread_id: id.clone(), rows: 10, cols: 40, answers: term::Answers::default() }).unwrap();
    until(&mut r, "the list runs", |e| e.ptys.contains_key(&id));
    r.engine.handle(Msg::Request {
        client: 1,
        request: Request { id: 990, command: Command::Input { thread_id: id.clone(), data: b64::encode(b"\x04") } },
    });
    until(&mut r, "its program ends", |e| !e.ptys.contains_key(&id));
    assert_eq!(lists(&r), vec![id.clone()], "the row stays");
    let err = r.cmd(Command::RemoveThread { thread_id: id.clone() }).unwrap_err();
    assert!(err.contains(".tasks.md") && err.contains("stays in the sidebar"), "{err}");
    // Without the file, it closes as any thread does.
    std::fs::remove_file(PathBuf::from(&project).join(desl_ir::TASK_LIST)).unwrap();
    r.cmd(Command::RemoveThread { thread_id: id }).unwrap();
    r.cmd(Command::Workspace).unwrap();
    assert!(lists(&r).is_empty());
}

#[test]
fn a_daemon_hands_its_running_programs_to_the_next_one() {
    use std::os::unix::net::UnixStream;
    let mut r = rig("handover");
    let t = r.t.clone();
    // A program every machine has, which repeats what is typed, runs in a thread of its own.
    r.engine.file_manager = "cat".into();
    let project = r.thread(&t).cwd;
    let Ok(Reply::Created { thread_id: id }) =
        r.cmd(Command::NewThread { cwd: project, agent: Some(FILES.into()), prompt: None, beside: Some(t.clone()), worktree: false })
    else {
        panic!("a thread")
    };
    r.cmd(Command::Attach { thread_id: id.clone(), rows: 10, cols: 40, answers: term::Answers::default() }).unwrap();
    let typed = |text: &str| Command::Input { thread_id: id.clone(), data: b64::encode(text.as_bytes()) };
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 900, command: typed("before\r") } });
    until(&mut r, "the program repeats what was typed", |e| drew(e, &id, "before\r\nbefore"));
    let pid = r.engine.ptys[&id].1.pid().expect("a process");

    // A client asks; the daemon is ready at once, with nothing half done.
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 901, command: Command::Handover { exe } } });
    assert!(r.engine.handover_due().is_some());
    r.engine.hold_terminals();
    r.pump();
    assert!(!r.engine.reading_terminals());
    // Typed while nobody reads: it waits in the terminal for the next daemon.
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 902, command: typed("between\r") } });

    let (mut ours, theirs) = UnixStream::pair().unwrap();
    let taker = std::thread::spawn(move || crate::handover::take(&theirs).unwrap());
    {
        let (answers, all) = r.engine.giving();
        assert_eq!(all.len(), 1);
        crate::handover::give(&mut ours, &answers, &all).unwrap();
    }
    let (answers, given) = taker.join().unwrap();
    assert_eq!((given[0].kept.thread_id.as_str(), given[0].kept.agent.as_str(), given[0].kept.pid), (id.as_str(), FILES, Some(pid)));

    // The next daemon: a new engine over the same store.
    let (tx, rx) = channel();
    let store = std::mem::replace(&mut r.engine.store, Store::in_memory(&r.dir.join("blobs2")).unwrap());
    let adapters: Vec<Arc<dyn Adapter>> = vec![Arc::new(Fake { id: "claude", state: r.claude.clone() })];
    let mut next = Engine::taking_over(store, adapters, tx, answers, given).unwrap();
    assert_eq!(next.terminals(), 1);
    assert!(!next.reading_terminals(), "it reads nothing before it has said that it serves");

    // The one that leaves says so, and ends nothing when it goes.
    r.engine.handed_over(4242);
    let mut said = Vec::new();
    while let Ok(bytes) = r.out.try_recv() {
        said.push(desl_proto::read_frame::<_, ServerFrame>(&mut &bytes[..]).unwrap().unwrap());
    }
    assert!(
        said.iter().any(|f| matches!(f, ServerFrame::Reply { id: 901, ok: Some(Reply::HandedOver { pid: 4242, kept: 1 }), .. })),
        "{said:?}"
    );
    assert!(matches!(said.last(), Some(ServerFrame::Event { change: Change::Restarting })), "{said:?}");
    assert_eq!(r.engine.terminals(), 0);
    let (out_tx, out) = channel();
    next.handle(Msg::Connected { client: 1, out: out_tx });
    let old = std::mem::replace(&mut r.engine, next);
    drop(old);
    (r.rx, r.out) = (rx, out);

    r.engine.read_terminals();
    until(&mut r, "what was typed in between reaches the next daemon", |e| drew(e, &id, "between\r\nbetween"));
    let Ok(Reply::Attached { replay, .. }) =
        r.cmd(Command::Attach { thread_id: id.clone(), rows: 10, cols: 40, answers: term::Answers::default() })
    else {
        panic!("attached")
    };
    assert!(String::from_utf8_lossy(&b64::decode(&replay).unwrap()).contains("before\r\nbefore"), "what it drew before came along");
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 903, command: typed("after\r") } });
    until(&mut r, "the same program answers the next daemon", |e| drew(e, &id, "after\r\nafter"));
    assert_eq!(r.engine.ptys[&id].1.pid(), Some(pid));
    // Its end is noticed without it being this daemon's child: the thread closes, as a shell's does.
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 904, command: typed("\x04") } });
    until(&mut r, "the program's end closes its thread", |e| !e.threads.iter().any(|x| x.id == id));
}

#[test]
fn a_turn_whose_agent_was_handed_over_is_not_called_interrupted() {
    let mut r = rig("handover-turn");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "a long task");
    assert_eq!(r.thread(&t).status, Status::Working);
    // The agent's terminal, here a file, comes along to the next daemon.
    let file = std::fs::File::create(r.dir.join("terminal")).unwrap();
    let given =
        vec![Given { kept: Kept::new(&t, "claude", None, (24, 80), 0), master: file.into(), log: b"drawn".to_vec(), partial: Vec::new() }];
    let (tx, _rx) = channel();
    let store = std::mem::replace(&mut r.engine.store, Store::in_memory(&r.dir.join("blobs2")).unwrap());
    let mut engine =
        Engine::taking_over(store, vec![Arc::new(Fake { id: "claude", state: r.claude.clone() })], tx, term::Answers::default(), given)
            .unwrap();
    assert_eq!(engine.threads.iter().find(|x| x.id == t).unwrap().status, Status::Working);
    assert!(!matches!(engine.store.all_events(&t).unwrap().pop().unwrap().kind, EventKind::Error { .. }));
    assert_eq!(engine.ptys[&t].1.log(), b"drawn");
    // A daemon that cannot serve after all lets go of it: nothing here ends the agent.
    engine.let_go();
    assert_eq!(engine.terminals(), 0);
}

fn pick_form() -> serde_json::Value {
    json!({"title": "Which milestone next?", "widgets": [{"kind": "choice", "id": "next", "options": [{"label": "M4"}, {"label": "M5"}]}]})
}

fn show(r: &mut Rig, thread: &str) -> String {
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: thread.to_string(), form: pick_form() }) else {
        panic!("shown")
    };
    form_id
}

#[test]
fn a_form_waits_for_the_user_and_its_answer_is_the_next_prompt() {
    let mut r = rig("form");
    let t = r.t.clone();
    let form_id = show(&mut r, &t);
    assert!(form_id.starts_with("f-") && form_id.len() == 6, "{form_id}");
    assert_eq!(r.thread(&t).status, Status::Waiting, "idle with an open form");
    assert_eq!(r.thread(&t).open_forms.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["Which milestone next?"]);
    assert_eq!(r.kinds(&t), ["form"]);
    let Ok(Reply::Form { form, draft, answer, closed_by, thread_id, .. }) = r.cmd(Command::FormGet { form_id: form_id.clone() }) else {
        panic!("the form")
    };
    assert_eq!((form, draft, answer, closed_by, thread_id), (pick_form(), None, None, None, t.clone()));
    let wrong = r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M6"}) }).unwrap_err();
    assert!(wrong.contains("not one of the options"), "{wrong}");
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M5"}) }).unwrap();
    let sent = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert_eq!(sent, desl_proto::form::answer_prompt(&form_id, "Which milestone next?", &json!({"next": "M5"})));
    let after = r.thread(&t);
    assert!(after.open_forms.is_empty());
    assert_eq!(after.status, Status::Idle, "the fake agent answered the prompt and is idle again");
    assert!(r.kinds(&t).starts_with(&["form", "form_answer"]));
    let Ok(Reply::Form { answer, closed_by, .. }) = r.cmd(Command::FormGet { form_id }) else { panic!("the form") };
    assert_eq!((answer, closed_by.as_deref()), (Some(json!({"next": "M5"})), Some("user")));
    r.check_invariants();
}

#[test]
fn a_form_is_answered_once() {
    let mut r = rig("form-once");
    let t = r.t.clone();
    let form_id = show(&mut r, &t);
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M5"}) }).unwrap();
    let again = r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M4"}) }).unwrap_err();
    assert_eq!(again, format!("{form_id} was answered already. Nothing more was sent."));
    assert_eq!(r.claude.lock().unwrap().payloads.len(), 1);
    let unknown = r.cmd(Command::FormAnswer { form_id: "f-0000".into(), answer: json!({}) }).unwrap_err();
    assert!(unknown.starts_with("There is no form f-0000."), "{unknown}");
}

#[test]
fn an_answer_while_the_agent_works_is_queued() {
    let mut r = rig("form-queue");
    let t = r.t.clone();
    r.claude.lock().unwrap().plan.push_back(vec![]);
    r.prompt(&t, "one");
    let form_id = show(&mut r, &t);
    assert_eq!(r.thread(&t).status, Status::Working, "working while its turn goes on");
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M4"}) }).unwrap();
    let answer = desl_proto::form::answer_prompt(&form_id, "Which milestone next?", &json!({"next": "M4"}));
    assert_eq!(r.thread(&t).queued, std::slice::from_ref(&answer));
    let claude = r.claude.clone();
    r.emit(&claude, vec![AdapterEvent::TurnEnd { error: None }]);
    assert_eq!(r.claude.lock().unwrap().payloads, ["one".to_string(), answer]);
    assert_eq!(r.thread(&t).status, Status::Idle);
}

#[test]
fn a_dismissal_is_told_before_the_next_prompt() {
    let mut r = rig("form-dismiss");
    let t = r.t.clone();
    let form_id = show(&mut r, &t);
    let bad = r.cmd(Command::FormClose { form_id: form_id.clone(), by: "user".into() }).unwrap_err();
    assert!(bad.contains("dismissed or withdrawn"), "{bad}");
    r.cmd(Command::FormClose { form_id: form_id.clone(), by: "dismissed".into() }).unwrap();
    assert!(r.claude.lock().unwrap().payloads.is_empty(), "dismissing starts no turn");
    assert_eq!(r.thread(&t).status, Status::Idle);
    r.prompt(&t, "go on");
    let note = desl_proto::form::dismissed_note(&form_id, "Which milestone next?");
    assert_eq!(r.claude.lock().unwrap().payloads.last().unwrap(), &format!("{note}\n\ngo on"));
    r.prompt(&t, "again");
    assert_eq!(r.claude.lock().unwrap().payloads.last().unwrap(), "again", "told once");
}

#[test]
fn an_open_form_and_its_draft_survive_a_restart() {
    let mut r = rig("form-restart");
    let t = r.t.clone();
    let form_id = show(&mut r, &t);
    r.cmd(Command::FormDraft { form_id: form_id.clone(), draft: json!({"next": "M5"}) }).unwrap();
    let (tx, _rx) = channel();
    let store = std::mem::replace(&mut r.engine.store, Store::in_memory(&r.dir.join("blobs2")).unwrap());
    let engine = Engine::new(store, vec![Arc::new(Fake { id: "claude", state: r.claude.clone() })], tx).unwrap();
    let thread = engine.threads.iter().find(|x| x.id == t).unwrap();
    assert_eq!(thread.status, Status::Waiting, "still waiting on the user");
    assert_eq!(thread.open_forms.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [form_id.as_str()]);
    let last = engine.store.all_events(&t).unwrap().pop().unwrap();
    assert!(matches!(last.kind, EventKind::Form { .. }), "no word of an interrupted turn: {last:?}");
    assert_eq!(engine.store.form(&form_id).unwrap().unwrap().draft.as_deref(), Some("{\"next\":\"M5\"}"));
}

#[test]
fn a_fork_copies_no_open_form() {
    let mut r = rig("form-fork");
    let t = r.t.clone();
    r.prompt(&t, "first");
    let form_id = show(&mut r, &t);
    let Reply::Created { thread_id: g } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None, worktree: false }).unwrap()
    else {
        panic!("a fork")
    };
    assert!(r.thread(&g).open_forms.is_empty());
    assert_ne!(r.thread(&g).status, Status::Waiting);
    r.cmd(Command::FormAnswer { form_id, answer: json!({"next": "M5"}) }).unwrap();
    assert!(r.kinds(&t).contains(&"form_answer"));
    assert!(!r.kinds(&g).contains(&"form_answer"), "the answer goes to the thread that asked");
}

#[test]
fn a_form_too_big_or_for_no_agent_is_refused() {
    let mut r = rig("form-refused");
    let t = r.t.clone();
    let big = json!({"title": "t", "widgets": [{"kind": "markdown", "id": "m", "text": "x".repeat(300_000)}]});
    let err = r.cmd(Command::FormNew { thread_id: t.clone(), form: big }).unwrap_err();
    assert!(err.contains("at most 256 KB"), "{err}");
    let Ok(Reply::Created { thread_id: shell }) = r.cmd(Command::NewThread {
        cwd: r.thread(&t).cwd,
        agent: Some(SHELL.into()),
        prompt: None,
        beside: Some(t.clone()),
        worktree: false,
    }) else {
        panic!("a shell thread")
    };
    let err = r.cmd(Command::FormNew { thread_id: shell, form: pick_form() }).unwrap_err();
    assert!(err.contains("no agent"), "{err}");
    let err = r.cmd(Command::FormNew { thread_id: t.clone(), form: json!({"title": "t", "widgets": []}) }).unwrap_err();
    assert!(err.contains("no widgets"), "{err}");
    assert!(r.thread(&t).open_forms.is_empty() && r.kinds(&t).is_empty(), "a refused form leaves nothing");
}

#[test]
fn a_threads_latest_highlight_follows_its_turns_and_reaches_the_clients() {
    let mut r = rig("latest");
    let t = r.t.clone();
    r.prompt(&t, "Why is the pane empty?");
    assert_eq!(r.thread(&t).latest.as_deref(), Some("reply to: Why is the pane empty?"));
    // Sent as a client does, so the changes it makes are read here and not by `cmd`.
    r.changes();
    let highlight = Command::Highlight { thread_id: t.clone(), question: "Who asks?".into(), answer: "Only prefix t.".into() };
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 900, command: highlight } });
    assert_eq!(r.thread(&t).latest.as_deref(), Some("Only prefix t."));
    let told =
        r.changes().into_iter().any(|c| matches!(c, Change::Thread { thread } if thread.latest.as_deref() == Some("Only prefix t.")));
    assert!(told, "clients hear of it");
}

#[test]
fn an_agents_highlight_goes_in_the_log_and_a_long_one_is_refused() {
    let mut r = rig("highlight");
    let t = r.t.clone();
    let ok = r.cmd(Command::Highlight {
        thread_id: t.clone(),
        question: "Why does the pane show no tasks on start?".into(),
        answer: "Only prefix t asked the daemon for the list. Now the pane asks as soon as it shows.".into(),
    });
    assert!(matches!(ok, Ok(Reply::Done)), "{ok:?}");
    assert_eq!(r.kinds(&t), ["highlight"]);
    let three = "One. Two. Three.";
    let err = r.cmd(Command::Highlight { thread_id: t.clone(), question: "Why?".into(), answer: three.into() }).unwrap_err();
    assert!(err.contains("two sentences"), "{err}");
    let err = r.cmd(Command::Highlight { thread_id: t.clone(), question: "x".repeat(301), answer: "Yes.".into() }).unwrap_err();
    assert!(err.contains("300 characters"), "{err}");
    let err = r.cmd(Command::Highlight { thread_id: t.clone(), question: " ".into(), answer: "Yes.".into() }).unwrap_err();
    assert!(err.contains("question"), "{err}");
    assert_eq!(r.kinds(&t), ["highlight"], "a refused one leaves nothing");
}

#[test]
fn an_answer_reaches_the_agent_while_another_form_is_open() {
    let mut r = rig("form-two");
    let t = r.t.clone();
    let first = show(&mut r, &t);
    let _second = show(&mut r, &t);
    r.cmd(Command::FormAnswer { form_id: first.clone(), answer: json!({"next": "M5"}) }).unwrap();
    let answer = desl_proto::form::answer_prompt(&first, "Which milestone next?", &json!({"next": "M5"}));
    assert_eq!(r.claude.lock().unwrap().payloads, [answer], "typed, not queued behind the other form");
    assert!(r.thread(&t).queued.is_empty());
    assert_eq!(r.thread(&t).status, Status::Waiting, "the other form is still open");
}

#[test]
fn a_prompt_reaches_the_agent_while_a_form_is_open() {
    let mut r = rig("form-talk");
    let t = r.t.clone();
    show(&mut r, &t);
    assert!(matches!(r.cmd(Command::Prompt { thread_id: t.clone(), text: "hello".into() }), Ok(Reply::Sent { queued: false })));
    assert_eq!(r.claude.lock().unwrap().payloads, ["hello"]);
    assert_eq!(r.thread(&t).status, Status::Waiting, "after the turn it waits on the form again");
    assert_eq!(r.thread(&t).open_forms.len(), 1, "talking to the agent closes no form");
}

#[test]
fn a_thread_waiting_on_a_form_switches_and_its_answer_reaches_the_new_agent() {
    let mut r = rig("form-switch");
    let t = r.t.clone();
    r.prompt(&t, "first");
    let form_id = show(&mut r, &t);
    r.cmd(Command::SetModel { thread_id: t.clone(), model: Some("fast".into()) }).expect("a model can be picked meanwhile");
    r.cmd(Command::Switch { thread_id: t.clone(), agent: "codex".into() }).expect("a wait on a form is no turn under way");
    assert_eq!(r.thread(&t).open_forms.len(), 1, "the form stays open across the switch");
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: json!({"next": "M4"}) }).unwrap();
    let answer = desl_proto::form::answer_prompt(&form_id, "Which milestone next?", &json!({"next": "M4"}));
    assert!(r.codex.lock().unwrap().payloads.last().is_some_and(|p| p.contains(&answer)), "{:?}", r.codex.lock().unwrap().payloads);
    let err = r.cmd(Command::Interrupt { thread_id: t.clone() });
    assert!(err.is_err(), "nothing to interrupt: {err:?}");
}

#[test]
fn a_stop_says_nothing_of_an_interrupted_turn_for_a_thread_waiting_on_a_form() {
    let mut r = rig("form-stop");
    let t = r.t.clone();
    show(&mut r, &t);
    r.engine.shutdown();
    assert!(!r.events(&t).iter().any(|e| matches!(e.kind, EventKind::Error { .. })), "{:?}", r.kinds(&t));
}

#[test]
fn a_thread_waiting_on_its_screen_after_a_handover_queues_a_prompt() {
    // A daemon that took over knows nothing of the dialog the one before it saw: the thread's
    // row says waiting, it has no form, so its screen asks something.
    let mut r = rig("screen-wait");
    let t = r.t.clone();
    let i = r.engine.idx(&t).unwrap();
    r.engine.threads[i].status = Status::Waiting;
    assert!(matches!(r.cmd(Command::Prompt { thread_id: t.clone(), text: "later".into() }), Ok(Reply::Sent { queued: true })));
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

fn pictures(paths: &[&str]) -> serde_json::Value {
    let images: Vec<_> = paths.iter().map(|p| json!({ "path": p })).collect();
    json!({"title": "Which logo?", "widgets": [{"kind": "images", "id": "logo", "images": images}]})
}

#[test]
fn an_image_outside_the_thread_is_refused() {
    let mut r = rig("form-images-out");
    let t = r.t.clone();
    let cwd = std::path::PathBuf::from(r.thread(&t).cwd);
    std::fs::create_dir_all(cwd.join("art")).unwrap();
    std::fs::write(cwd.join("art/logo.png"), PNG).unwrap();
    std::fs::write(r.dir.join("secret.png"), PNG).unwrap();
    std::os::unix::fs::symlink(r.dir.join("secret.png"), cwd.join("art/link.png")).unwrap();
    std::fs::write(cwd.join("notes.txt"), "not a picture").unwrap();
    std::fs::write(cwd.join("big.png"), [PNG, &vec![0u8; 10 * 1024 * 1024]].concat()).unwrap();
    let secret = r.dir.join("secret.png").to_string_lossy().into_owned();
    let inside = cwd.join("art/logo.png").to_string_lossy().into_owned();
    let ok = r.cmd(Command::FormNew { thread_id: t.clone(), form: pictures(&["art/logo.png", &inside]) });
    assert!(matches!(ok, Ok(Reply::FormShown { .. })), "{ok:?}");
    for (path, says) in [
        ("../secret.png", "outside"),
        (secret.as_str(), "outside"),
        ("art/link.png", "outside"),
        ("art/none.png", "no file"),
        ("notes.txt", "not a PNG, JPEG, GIF, WebP or SVG"),
        ("big.png", "10 MB"),
    ] {
        let err = r.cmd(Command::FormNew { thread_id: t.clone(), form: pictures(&[path]) }).unwrap_err();
        assert!(err.contains(path) && err.contains(says), "{path}: {err}");
    }
}

#[test]
fn the_page_reads_only_the_images_a_form_names() {
    let mut r = rig("form-images-read");
    let t = r.t.clone();
    let cwd = std::path::PathBuf::from(r.thread(&t).cwd);
    std::fs::write(cwd.join("a.png"), PNG).unwrap();
    std::fs::write(cwd.join("b.png"), PNG).unwrap();
    std::fs::write(cwd.join("other.png"), PNG).unwrap();
    std::fs::write(r.dir.join("secret.png"), PNG).unwrap();
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t.clone(), form: pictures(&["a.png", "b.png"]) }) else {
        panic!("shown")
    };
    let Ok(Reply::Image { mime, data }) = r.cmd_waiting(Command::FormImage { form_id: form_id.clone(), path: "a.png".into() }) else {
        panic!("an image")
    };
    assert_eq!((mime.as_str(), desl_proto::b64::decode(&data).unwrap()), ("image/png", PNG.to_vec()));
    let err = r.cmd_waiting(Command::FormImage { form_id: form_id.clone(), path: "other.png".into() }).unwrap_err();
    assert!(err.contains("other.png"), "{err}");
    // Swapped for a link to outside after the form was shown.
    std::fs::remove_file(cwd.join("b.png")).unwrap();
    std::os::unix::fs::symlink(r.dir.join("secret.png"), cwd.join("b.png")).unwrap();
    let err = r.cmd_waiting(Command::FormImage { form_id, path: "b.png".into() }).unwrap_err();
    assert!(err.contains("outside"), "{err}");
}

#[test]
fn a_thread_whose_only_history_is_forms_opens_a_new_session() {
    // A script pushed a form to a thread before its agent ever ran: there is no conversation to
    // copy, and a copy of none is a session the agent cannot resume.
    let mut r = rig("form-fresh");
    let t = r.t.clone();
    r.claude.lock().unwrap().copies = true;
    show(&mut r, &t);
    let i = r.engine.idx(&t).unwrap();
    let fake = Fake { id: "claude", state: r.claude.clone() };
    assert!(matches!(r.engine.open_for(i, &fake), Open::New { session_id: Some(_) }), "{:?}", r.engine.open_for(i, &fake));
    assert!(r.claude.lock().unwrap().copied.is_empty(), "nothing copied");
    // With a conversation, as a fork has, it is copied.
    r.engine.append(&t, EventKind::UserMessage { text: "hello".into(), attachments: vec![] });
    assert_eq!(r.engine.open_for(i, &fake), Open::Resume { session_id: "copied".into() });
}

const GUIDE: &str = "# Guide\n\nIntro.\n\n## Install\n\nRun it.\n\n## Use\n\nThen go.\n";

fn document_form(path: &str, section: Option<&str>) -> serde_json::Value {
    let mut w = json!({"kind": "document", "id": "doc", "path": path});
    if let Some(s) = section {
        w["section"] = json!(s);
    }
    json!({"title": "Edit the install section", "widgets": [w]})
}

fn pushed_document(r: &mut Rig, name: &str) -> (String, PathBuf) {
    let t = r.t.clone();
    let file = PathBuf::from(r.thread(&t).cwd).join(name);
    std::fs::write(&file, GUIDE).unwrap();
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t, form: document_form(name, Some("Install")) }) else {
        panic!("shown")
    };
    (form_id, file)
}

#[test]
fn a_document_section_is_read_when_pushed_and_saved_on_send() {
    let mut r = rig("form-doc");
    let (form_id, file) = pushed_document(&mut r, "guide.md");
    let Ok(Reply::Form { form, .. }) = r.cmd(Command::FormGet { form_id: form_id.clone() }) else { panic!("the form") };
    assert_eq!(form["widgets"][0]["original"], json!("## Install\n\nRun it.\n\n"), "read once, when pushed");
    let answer =
        json!({"doc": {"text": "## Install\n\nRun `make install`.\n\n", "comments": [{"quote": "make install", "note": "and then?"}]}});
    r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer }).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Guide\n\nIntro.\n\n## Install\n\nRun `make install`.\n\n## Use\n\nThen go.\n");
    let sent = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(sent.contains("\"saved\":true"), "{sent}");
    assert!(sent.contains("-Run it.") && sent.contains("+Run `make install`.") && sent.contains("--- a/guide.md"), "{sent}");
    assert!(sent.contains("\"note\":\"and then?\""), "{sent}");
    let Ok(Reply::Form { answer, .. }) = r.cmd(Command::FormGet { form_id }) else { panic!("the form") };
    assert_eq!(answer.unwrap()["doc"]["saved"], json!(true), "the answered card shows what the agent got");
}

#[test]
fn an_unchanged_document_is_not_saved() {
    let mut r = rig("form-doc-same");
    let (form_id, file) = pushed_document(&mut r, "guide.md");
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    r.cmd(Command::FormAnswer { form_id, answer: json!({"doc": {"text": "## Install\n\nRun it.\n\n"}}) }).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before, "not written");
    let sent = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(sent.contains("\"saved\":false") && sent.contains("\"diff\":\"\""), "{sent}");
}

#[test]
fn a_document_changed_on_disk_is_saved_only_over_what_the_user_saw() {
    let mut r = rig("form-doc-conflict");
    let (form_id, file) = pushed_document(&mut r, "guide.md");
    let theirs = GUIDE.replace("Run it.", "Run it now.");
    std::fs::write(&file, &theirs).unwrap();
    let mine = json!({"doc": {"text": "## Install\n\nMine.\n\n"}});
    let err = r.cmd(Command::FormAnswer { form_id: form_id.clone(), answer: mine }).unwrap_err();
    assert!(err.contains("changed") && err.contains("Nothing was saved"), "{err}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), theirs, "untouched");
    assert!(r.thread(&r.t.clone()).open_forms.iter().any(|f| f.id == form_id), "still open");
    let Ok(Reply::Section { text }) = r.cmd(Command::FormSection { form_id: form_id.clone(), widget: "doc".into() }) else {
        panic!("the section as it is now")
    };
    assert_eq!(text.as_deref(), Some("## Install\n\nRun it now.\n\n"));
    // The user saw both and keeps theirs over the file's.
    let over = json!({"doc": {"text": "## Install\n\nMine.\n\n", "base": "## Install\n\nRun it now.\n\n"}});
    r.cmd(Command::FormAnswer { form_id, answer: over }).unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), GUIDE.replace("Run it.", "Mine."));
}

#[test]
fn a_document_outside_the_thread_or_not_markdown_is_refused() {
    let mut r = rig("form-doc-refused");
    let t = r.t.clone();
    let cwd = PathBuf::from(r.thread(&t).cwd);
    std::fs::write(r.dir.join("secret.md"), GUIDE).unwrap();
    std::os::unix::fs::symlink(r.dir.join("secret.md"), cwd.join("link.md")).unwrap();
    std::fs::write(cwd.join("crlf.md"), "# A\r\n\r\nx\r\n").unwrap();
    std::fs::write(cwd.join("big.md"), "x".repeat(1024 * 1024 + 1)).unwrap();
    std::fs::write(cwd.join("guide.md"), GUIDE).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(cwd.join("pipe.md")).status().unwrap().success());
    let secret = r.dir.join("secret.md").to_string_lossy().into_owned();
    for (path, section, says) in [
        ("../secret.md", None, "outside"),
        (secret.as_str(), None, "outside"),
        ("link.md", None, "outside"),
        ("none.md", None, "no file"),
        ("pipe.md", None, "not a file"),
        ("crlf.md", None, "line endings"),
        ("big.md", None, "1 MB"),
        ("guide.md", Some("Uninstall"), "no heading"),
    ] {
        let err = r.cmd(Command::FormNew { thread_id: t.clone(), form: document_form(path, section) }).unwrap_err();
        assert!(err.contains(says), "{path}: {err}");
    }
    assert!(r.thread(&t).open_forms.is_empty());
}

#[test]
fn one_file_named_twice_in_a_form_is_refused() {
    // Two sections of one file saved one after the other: the second would undo the first.
    let mut r = rig("form-doc-twice");
    let t = r.t.clone();
    let cwd = PathBuf::from(r.thread(&t).cwd);
    std::fs::write(cwd.join("a.md"), GUIDE).unwrap();
    std::os::unix::fs::symlink(cwd.join("a.md"), cwd.join("b.md")).unwrap();
    for other in ["./a.md", "b.md"] {
        let form = json!({"title": "t", "widgets": [
            {"kind": "document", "id": "one", "path": "a.md", "section": "Install"},
            {"kind": "document", "id": "two", "path": other, "section": "Use"}]});
        let err = r.cmd(Command::FormNew { thread_id: t.clone(), form }).unwrap_err();
        assert!(err.contains("same file"), "{other}: {err}");
    }
}

#[test]
fn a_form_closed_unanswered_keeps_what_the_user_wrote() {
    // An hour's edit to a document must not vanish because the agent withdrew the form.
    let mut r = rig("form-doc-withdrawn");
    let (form_id, _) = pushed_document(&mut r, "guide.md");
    let draft = json!({"doc": {"text": "## Install\n\nAn hour of work.\n\n", "comments": []}});
    r.cmd(Command::FormDraft { form_id: form_id.clone(), draft: draft.clone() }).unwrap();
    r.cmd(Command::FormClose { form_id: form_id.clone(), by: "withdrawn".into() }).unwrap();
    let Ok(Reply::Form { draft: kept, closed_by, .. }) = r.cmd(Command::FormGet { form_id }) else { panic!("the form") };
    assert_eq!((kept, closed_by.as_deref()), (Some(draft), Some("withdrawn")));
}

#[test]
fn a_form_edits_at_most_256_kb_of_sections_in_all() {
    // Its draft holds each text as edited and as it started: that must fit what desl keeps.
    let mut r = rig("form-doc-much");
    let t = r.t.clone();
    let cwd = PathBuf::from(r.thread(&t).cwd);
    let big = format!("# A\n\n{}\n", "x".repeat(150 * 1024));
    std::fs::write(cwd.join("a.md"), &big).unwrap();
    std::fs::write(cwd.join("b.md"), &big).unwrap();
    let form = json!({"title": "t", "widgets": [
        {"kind": "document", "id": "one", "path": "a.md"}, {"kind": "document", "id": "two", "path": "b.md"}]});
    let err = r.cmd(Command::FormNew { thread_id: t.clone(), form }).unwrap_err();
    assert!(err.contains("256 KB") && err.contains("in all"), "{err}");
}

#[test]
fn a_picture_is_read_off_the_engines_thread() {
    // Twelve pictures of 10 MB, read and encoded, would hold every key and every agent's screen.
    let mut r = rig("form-images-async");
    let t = r.t.clone();
    let cwd = PathBuf::from(r.thread(&t).cwd);
    std::fs::write(cwd.join("a.png"), PNG).unwrap();
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t, form: pictures(&["a.png"]) }) else { panic!("shown") };
    r.engine
        .handle(Msg::Request { client: 1, request: Request { id: 999, command: Command::FormImage { form_id, path: "a.png".into() } } });
    let replied_at_once = std::iter::from_fn(|| r.out.try_recv().ok())
        .any(|bytes| matches!(desl_proto::read_frame::<_, ServerFrame>(&mut &bytes[..]), Ok(Some(ServerFrame::Reply { id: 999, .. }))));
    assert!(!replied_at_once, "the engine went on; a worker replies");
}

#[test]
fn a_section_that_cannot_be_read_again_says_why() {
    let mut r = rig("form-doc-why");
    let (form_id, file) = pushed_document(&mut r, "guide.md");
    std::fs::write(&file, format!("{GUIDE}\n## Install\n\nAgain.\n")).unwrap();
    let err = r.cmd(Command::FormSection { form_id: form_id.clone(), widget: "doc".into() }).unwrap_err();
    assert!(err.contains("twice"), "{err}");
    std::fs::remove_file(&file).unwrap();
    let err = r.cmd(Command::FormSection { form_id, widget: "doc".into() }).unwrap_err();
    assert!(err.contains("no file"), "{err}");
}

#[test]
fn a_save_that_fails_after_another_tells_the_agent_what_was_saved() {
    let mut r = rig("form-doc-partial");
    let t = r.t.clone();
    let cwd = PathBuf::from(r.thread(&t).cwd);
    std::fs::create_dir_all(cwd.join("locked")).unwrap();
    std::fs::write(cwd.join("a.md"), GUIDE).unwrap();
    std::fs::write(cwd.join("locked/b.md"), GUIDE).unwrap();
    let form = json!({"title": "t", "widgets": [
        {"kind": "document", "id": "one", "path": "a.md", "section": "Install"},
        {"kind": "document", "id": "two", "path": "locked/b.md", "section": "Use"}]});
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t, form }) else { panic!("shown") };
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(cwd.join("locked"), std::fs::Permissions::from_mode(0o555)).unwrap();
    let answer = json!({"one": {"text": "## Install\n\nA.\n\n"}, "two": {"text": "## Use\n\nB.\n"}});
    let sent = r.cmd(Command::FormAnswer { form_id, answer });
    std::fs::set_permissions(cwd.join("locked"), std::fs::Permissions::from_mode(0o755)).unwrap();
    sent.unwrap();
    let prompt = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(std::fs::read_to_string(cwd.join("a.md")).unwrap().contains("A."), "the first was saved");
    assert!(prompt.contains("\"saved\":true") && prompt.contains("could not save"), "{prompt}");
    assert!(prompt.contains("## Use\\n\\nB."), "the text that was not saved reaches the agent: {prompt}");
}

#[test]
fn a_thread_whose_only_messages_are_blank_opens_a_new_session() {
    let mut r = rig("blank-only");
    let t = r.t.clone();
    r.claude.lock().unwrap().copies = true;
    r.engine.append(&t, EventKind::UserMessage { text: "  ".into(), attachments: vec![] });
    let i = r.engine.idx(&t).unwrap();
    let fake = Fake { id: "claude", state: r.claude.clone() };
    assert!(matches!(r.engine.open_for(i, &fake), Open::New { .. }));
    assert!(r.claude.lock().unwrap().copied.is_empty());
}

#[test]
fn an_answer_over_1_mb_is_refused() {
    let mut r = rig("form-answer-big");
    let t = r.t.clone();
    let form = json!({"title": "t", "widgets": [{"kind": "text", "id": "a", "label": "A"}]});
    let Ok(Reply::FormShown { form_id }) = r.cmd(Command::FormNew { thread_id: t, form }) else { panic!("shown") };
    let err = r.cmd(Command::FormAnswer { form_id, answer: json!({"a": "x".repeat(1024 * 1024 + 1)}) }).unwrap_err();
    assert!(err.contains("1 MB"), "{err}");
}

/// What the engine asked to open in the browser, instead of a browser.
fn watch_opens(r: &mut Rig) -> Arc<Mutex<Vec<String>>> {
    let opened = Arc::new(Mutex::new(Vec::new()));
    let seen = opened.clone();
    r.engine.set_page_opener(Box::new(move |thread: &str| seen.lock().unwrap().push(thread.to_string())));
    opened
}

#[test]
fn a_form_pushed_with_no_page_open_opens_the_browser_on_it() {
    let mut r = rig("form-opens");
    let t = r.t.clone();
    let opened = watch_opens(&mut r);
    show(&mut r, &t);
    assert_eq!(*opened.lock().unwrap(), std::slice::from_ref(&t), "on the thread, at its form");
    show(&mut r, &t);
    assert_eq!(opened.lock().unwrap().len(), 1, "once: the page it opened is still on its way");
}

#[test]
fn a_form_pushed_while_a_page_is_open_opens_nothing() {
    let mut r = rig("form-page-open");
    let t = r.t.clone();
    let opened = watch_opens(&mut r);
    let (out, _keep) = channel();
    r.engine.handle(Msg::Connected { client: 2, out });
    let hello = Command::Hello { protocol: PROTOCOL, client: "web".into() };
    r.engine.handle(Msg::Request { client: 2, request: Request { id: 1, command: hello } });
    show(&mut r, &t);
    assert!(opened.lock().unwrap().is_empty(), "the page shows it already");
    r.engine.handle(Msg::Disconnected { client: 2 });
    show(&mut r, &t);
    assert_eq!(*opened.lock().unwrap(), [t], "the tab was closed");
}

#[test]
fn opening_the_browser_for_forms_can_be_turned_off() {
    let mut r = rig("form-opens-off");
    let t = r.t.clone();
    let file = r.dir.join("config.toml");
    std::fs::write(&file, "[web]\nopen_on_form = false\n").unwrap();
    r.engine.set_config_file(file);
    let opened = watch_opens(&mut r);
    show(&mut r, &t);
    assert!(opened.lock().unwrap().is_empty());
}

#[test]
fn a_fork_keeps_its_parents_forms_when_the_parent_is_removed() {
    let mut r = rig("form-fork-removed");
    let t = r.t.clone();
    r.prompt(&t, "first");
    let answered = show(&mut r, &t);
    r.cmd(Command::FormAnswer { form_id: answered.clone(), answer: json!({"next": "M5"}) }).unwrap();
    let open = show(&mut r, &t);
    let Reply::Created { thread_id: fork } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None, worktree: false }).unwrap()
    else {
        panic!("a fork")
    };
    r.cmd(Command::RemoveThread { thread_id: t }).unwrap();
    // The fork's conversation still shows both cards: their forms are kept.
    let Ok(Reply::Form { closed_by, answer, .. }) = r.cmd(Command::FormGet { form_id: answered }) else { panic!("kept") };
    assert_eq!((closed_by.as_deref(), answer), (Some("user"), Some(json!({"next": "M5"}))));
    let Ok(Reply::Form { closed_by, .. }) = r.cmd(Command::FormGet { form_id: open }) else { panic!("kept") };
    assert_eq!(closed_by.as_deref(), Some("removed"), "nobody can answer it any more");
    assert!(r.thread(&fork).open_forms.is_empty());
}

#[test]
fn a_removed_thread_with_no_fork_takes_its_forms_with_it() {
    let mut r = rig("form-removed");
    let t = r.t.clone();
    let form_id = show(&mut r, &t);
    let Ok(Reply::Created { thread_id: other }) =
        r.cmd(Command::NewThread { cwd: r.thread(&t).cwd, agent: None, prompt: None, beside: Some(t.clone()), worktree: false })
    else {
        panic!("another thread")
    };
    assert_ne!(other, t);
    r.cmd(Command::RemoveThread { thread_id: t }).unwrap();
    assert!(r.cmd(Command::FormGet { form_id }).unwrap_err().contains("no form"));
}

#[test]
fn a_dialog_on_the_screen_of_a_thread_with_a_form_is_told_to_the_clients() {
    let mut r = rig("form-and-screen");
    let t = r.t.clone();
    show(&mut r, &t);
    assert!(!r.thread(&t).screen_asks);
    r.engine.asked_on_screen.insert(t.clone());
    r.engine.persist(&t);
    assert!(r.thread(&t).screen_asks, "the waiting column lists it beside its form");
    r.engine.asked_on_screen.remove(&t);
    r.engine.persist(&t);
    assert!(!r.thread(&t).screen_asks);
}

impl Rig {
    /// The changes the daemon told the clients since the last look.
    fn changes(&mut self) -> Vec<Change> {
        let mut out = Vec::new();
        while let Ok(bytes) = self.out.try_recv() {
            if let Ok(Some(ServerFrame::Event { change })) = desl_proto::read_frame::<_, ServerFrame>(&mut &bytes[..]) {
                out.push(change);
            }
        }
        out
    }

    fn project_of(&self, thread: &str) -> (String, PathBuf) {
        let pid = self.thread(thread).project_id;
        let p = self.engine.projects.iter().find(|p| p.id == pid).unwrap();
        (pid, PathBuf::from(&p.path))
    }

    /// A second of ticks.
    fn second(&mut self) {
        for _ in 0..TICKS_PER_SECOND {
            self.engine.handle(Msg::Tick);
        }
    }
}

#[test]
fn a_task_list_changed_on_disk_reaches_the_clients_once() {
    let mut r = rig("tasks-watch");
    let t = r.t.clone();
    let (project, root) = r.project_of(&t);
    std::fs::write(root.join(".tasks.md"), "# Tasks\n\n## Fix the wheel\n- id: 1\n- type: bug\n- status: todo\n- priority: high\n")
        .unwrap();
    r.changes();
    r.second();
    let sent: Vec<Change> = r.changes().into_iter().filter(|c| matches!(c, Change::Tasks { .. })).collect();
    assert!(
        matches!(&sent[..], [Change::Tasks { project_id, tasks }] if *project_id == project && tasks[0].title == "Fix the wheel"),
        "{sent:?}"
    );
    r.second();
    assert!(!r.changes().iter().any(|c| matches!(c, Change::Tasks { .. })), "nothing changed since");
    let Ok(Reply::Tasks { tasks, .. }) = r.cmd(Command::Tasks { project_id: project }) else { panic!("the list") };
    assert_eq!(tasks.len(), 1);
}

#[test]
fn a_task_added_through_the_daemon_is_in_the_file_and_told() {
    let mut r = rig("tasks-add");
    let t = r.t.clone();
    let (project, root) = r.project_of(&t);
    let add = Command::TaskAdd {
        project_id: project.clone(),
        title: "Forms in the terminal".into(),
        kind: desl_proto::tasks::Kind::Idea,
        status: desl_proto::tasks::Status::Parked,
        priority: desl_proto::tasks::Priority::Low,
        body: "Not now.".into(),
    };
    let Ok(Reply::Task { task, .. }) = r.cmd(add) else { panic!("the task") };
    assert_eq!((task.id, task.status), (1, desl_proto::tasks::Status::Parked));
    assert!(std::fs::read_to_string(root.join(".tasks.md")).unwrap().contains("- status: parked"));
    let edit = desl_proto::tasks::Edit { status: Some(desl_proto::tasks::Status::Todo), ..Default::default() };
    let set = Command::TaskSet { project_id: project.clone(), task: 1, edit };
    r.engine.handle(Msg::Request { client: 1, request: Request { id: 500, command: set } });
    let told = r.changes().into_iter().find_map(|c| match c {
        Change::Tasks { project_id, tasks } if project_id == project => Some(tasks),
        _ => None,
    });
    assert_eq!(told.map(|t| t[0].status), Some(desl_proto::tasks::Status::Todo), "told at once");
    let missing = r.cmd(Command::TaskSet { project_id: project, task: 9, edit: Default::default() }).unwrap_err();
    assert!(missing.contains("#9"), "{missing}");
}

/// A git repository at the project's root with one commit, and a task list of one bug.
fn task_repo(r: &mut Rig) -> (String, PathBuf) {
    let t = r.t.clone();
    let (project, root) = r.project_of(&t);
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["-c", "user.email=t@t", "-c", "user.name=t"])
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(root.join("README.md"), "# api\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-q", "-m", "first"]);
    std::fs::write(
        root.join(".tasks.md"),
        "# Tasks\n\n## Fix the wheel\n- id: 1\n- type: bug\n- status: todo\n- priority: high\n\nA wheel report is typed as text.\n",
    )
    .unwrap();
    (project, root)
}

fn started(r: &mut Rig, project: &str, place: &str) -> desl_proto::tasks::Task {
    let start = Command::TaskStart { project_id: project.to_string(), task: 1, agent: Some("claude".into()), place: place.into() };
    match r.cmd(start) {
        Ok(Reply::Task { task, .. }) => task,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_task_started_in_its_own_worktree_gets_a_thread_a_branch_and_the_brief() {
    let mut r = rig("task-start-wt");
    let (project, _) = task_repo(&mut r);
    let task = started(&mut r, &project, "worktree");
    assert_eq!(task.status, desl_proto::tasks::Status::InProgress);
    let thread = task.thread.clone().expect("linked to its thread");
    let branch = task.branch.clone().expect("on a branch of its own");
    let t = r.thread(&thread);
    assert_eq!(t.name, "#1 Fix the wheel");
    assert!(t.worktree.is_some(), "in a worktree");
    assert_eq!(t.git.branch.as_deref(), Some(branch.as_str()));
    let brief = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(brief.contains("task #1") && brief.contains("Fix the wheel") && brief.contains("A wheel report is typed as text."), "{brief}");
    assert!(brief.contains(&branch) && brief.contains("rebase") && brief.contains("main") && brief.contains("ready"), "{brief}");
}

#[test]
fn a_task_started_in_the_main_checkout_or_a_thread_is_marked_done_by_its_agent() {
    let mut r = rig("task-start-main");
    let (project, _) = task_repo(&mut r);
    let task = started(&mut r, &project, "main");
    assert_eq!((task.status, task.branch.as_deref()), (desl_proto::tasks::Status::InProgress, None));
    let brief = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(brief.contains("--status done") && !brief.contains("rebase"), "{brief}");
    // The selected thread: the brief goes to it, and the task names it.
    let here = r.t.clone();
    let edit = desl_proto::tasks::Edit { status: Some(desl_proto::tasks::Status::Todo), thread: Some(String::new()), ..Default::default() };
    r.cmd(Command::TaskSet { project_id: project.clone(), task: 1, edit }).unwrap();
    let task = started(&mut r, &project, &format!("thread:{here}"));
    assert_eq!(task.thread.as_deref(), Some(here.as_str()));
    let refused = r.cmd(Command::TaskStart { project_id: project, task: 1, agent: None, place: "nowhere".into() }).unwrap_err();
    assert!(refused.contains("worktree, main"), "{refused}");
}

fn git_in(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Task #1 started in its own worktree, one commit made there, and marked ready.
fn ready_task(r: &mut Rig) -> (String, PathBuf, desl_proto::tasks::Task) {
    let (project, root) = task_repo(r);
    let task = started(r, &project, "worktree");
    let wt = PathBuf::from(r.thread(task.thread.as_deref().unwrap()).cwd);
    std::fs::write(wt.join("wheel.rs"), "// fixed\n").unwrap();
    git_in(&wt, &["add", "wheel.rs"]);
    git_in(&wt, &["commit", "-q", "-m", "Fix the wheel"]);
    let edit = desl_proto::tasks::Edit { status: Some(desl_proto::tasks::Status::Ready), ..Default::default() };
    r.cmd(Command::TaskSet { project_id: project.clone(), task: 1, edit }).unwrap();
    (project, root, task)
}

#[test]
fn merging_a_ready_task_fast_forwards_main_and_ends_its_worktree() {
    let mut r = rig("task-merge");
    let (project, root, task) = ready_task(&mut r);
    let Ok(Reply::Diff { text, .. }) = r.cmd(Command::TaskDiff { project_id: project.clone(), task: 1 }) else { panic!("a diff") };
    assert!(text.contains("+// fixed"), "{text}");
    let thread = task.thread.clone().unwrap();
    let wt = PathBuf::from(r.thread(&thread).cwd);
    let Ok(Reply::Task { task: done, .. }) = r.cmd(Command::TaskMerge { project_id: project, task: 1 }) else { panic!("merged") };
    assert_eq!(done.status, desl_proto::tasks::Status::Done);
    assert_eq!(git_in(&root, &["log", "-1", "--format=%s"]), "Fix the wheel", "main has the work");
    assert!(root.join("wheel.rs").is_file());
    assert!(!wt.exists(), "the worktree went");
    assert_eq!(
        git_in(&root, &["branch", "--list", task.branch.as_deref().unwrap()]).trim_start_matches(['*', ' ']),
        task.branch.unwrap(),
        "the branch stays"
    );
    let t = r.thread(&thread);
    assert_eq!((t.cwd, t.worktree), (root.to_string_lossy().into_owned(), None), "the thread stays, in the main checkout");
}

#[test]
fn a_task_whose_main_moved_on_is_not_merged_and_its_agent_can_be_asked_to_rebase() {
    let mut r = rig("task-merge-moved");
    let (project, root, _) = ready_task(&mut r);
    std::fs::write(root.join("other.rs"), "// main moved\n").unwrap();
    git_in(&root, &["add", "other.rs"]);
    git_in(&root, &["commit", "-q", "-m", "Main moved"]);
    let Ok(Reply::MergeRefused { reason, rebase }) = r.cmd(Command::TaskMerge { project_id: project.clone(), task: 1 }) else {
        panic!("refused")
    };
    assert!(rebase && reason.contains("rebase"), "{reason}");
    assert_eq!(git_in(&root, &["log", "-1", "--format=%s"]), "Main moved", "main untouched");
    let Ok(Reply::Task { task, .. }) = r.cmd(Command::TaskRebase { project_id: project, task: 1 }) else { panic!("asked") };
    assert_eq!(task.status, desl_proto::tasks::Status::InProgress);
    let asked = r.claude.lock().unwrap().payloads.last().cloned().unwrap();
    assert!(asked.contains("Rebase") && asked.contains("main") && asked.contains("--status ready"), "{asked}");
}

#[test]
fn a_thread_names_its_projects_task_list() {
    // `desl mcp` knows its thread, not its project.
    let mut r = rig("tasks-by-thread");
    let t = r.t.clone();
    let (_, root) = r.project_of(&t);
    let add = Command::TaskAdd {
        project_id: t.clone(),
        title: "Park this".into(),
        kind: desl_proto::tasks::Kind::Idea,
        status: desl_proto::tasks::Status::Parked,
        priority: desl_proto::tasks::Priority::Low,
        body: String::new(),
    };
    assert!(matches!(r.cmd(add), Ok(Reply::Task { .. })));
    assert!(std::fs::read_to_string(root.join(".tasks.md")).unwrap().contains("## Park this"));
}
