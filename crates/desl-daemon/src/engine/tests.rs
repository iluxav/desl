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
    assert_eq!(unknown, "desl drives claude and codex, not gemini.");
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
    let Reply::Created { thread_id: f } = r.cmd(Command::Fork { thread_id: t.clone(), agent: Some("codex".into()), at: Some(3) }).unwrap()
    else {
        panic!()
    };
    assert_eq!(f, format!("{t}/1"));
    let fork = r.thread(&f);
    assert_eq!((fork.parent_id.as_deref(), fork.fork_point, fork.active_agent.as_str()), (Some(t.as_str()), Some(3), "codex"));
    assert!(fork.bindings.is_empty());
    assert_eq!(r.kinds(&f), ["user_message", "assistant_message", "usage", "fork"]);
    assert_eq!(r.events(&t), before, "the parent is untouched");

    r.prompt(&f, "try it another way");
    let brief = r.codex.lock().unwrap().payloads[0].clone();
    assert!(brief.starts_with("# Handoff brief\n") && brief.contains("first task") && !brief.contains("second task"));
    let Reply::Created { thread_id: g } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None }).unwrap() else { panic!() };
    assert_eq!(g, format!("{t}/2"));
    let bad = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: Some(99) }).unwrap_err();
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
                r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: Some(at) }).unwrap();
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
        let Reply::Created { thread_id } = r.cmd(Command::Fork { thread_id: t.clone(), agent: None, at: None }).unwrap() else { panic!() };
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
