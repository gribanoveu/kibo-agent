use super::*;

use crate::domain::llm::{
    ChatResponse, ChatUsage, LlmModelInfo, LlmProvider, LlmToolDefinition,
};
use crate::domain::tools::{ToolScope, ToolName};
use crate::domain::turn::{ChatTurnEvent, STEERING_PREFIX};
use crate::testing::temp_dir;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- doubles

/// One scripted answer from the provider.
enum Step {
    Reply(ChatStreamResult),
    Fail(LlmError),
    /// Streams text and *then* fails — the shape that must never be
    /// retried, since half the round has already been reported.
    StreamThenFail(&'static str, LlmError),
    /// Answers, and the user types while it does. The only way to queue a
    /// note *during* a turn when the provider is synchronous.
    ReplyWhileTheUserTypes(ChatStreamResult, SteeringNote),
    /// Streams calls as providers do — each chunk every call of the
    /// round, its arguments whole so far — then answers.
    ReplyStreamingCalls(Vec<Vec<(&'static str, &'static str, &'static str)>>, ChatStreamResult),
    /// Thinks aloud, then answers: the reasoning streams as its own kind.
    ReplyThinking(&'static str, ChatStreamResult),
}

fn text_while_typing(answer: &str, note: &'static str) -> Step {
    Step::ReplyWhileTheUserTypes(
        ChatStreamResult {
            text: answer.to_string(),
            ..Default::default()
        },
        SteeringNote::user(note),
    )
}

fn asks_while_typing(calls: Vec<LlmToolCall>, note: &'static str) -> Step {
    Step::ReplyWhileTheUserTypes(
        ChatStreamResult {
            tool_calls: calls,
            ..Default::default()
        },
        SteeringNote::user(note),
    )
}

fn text(answer: &str) -> Step {
    Step::Reply(ChatStreamResult {
        text: answer.to_string(),
        ..Default::default()
    })
}

fn asks(calls: Vec<LlmToolCall>) -> Step {
    Step::Reply(ChatStreamResult {
        tool_calls: calls,
        ..Default::default()
    })
}

fn wants(id: &str, name: &str, arguments: &str) -> LlmToolCall {
    LlmToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<ChatRequest>>,
    steering: Mutex<Option<Arc<SteeringQueue>>>,
    /// Summarizing requests, which arrive unstreamed and out of band —
    /// kept apart from `requests` so a test can say how many rounds there
    /// were without counting them.
    summaries: Mutex<Vec<ChatRequest>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
            steering: Mutex::new(None),
            summaries: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl LlmProvider for Scripted {
    /// Only compaction and the check of `remember` gets here: the loop
    /// itself always streams. A check passes what it
    /// is shown unless that says "approve".
    fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let note = request.messages.last().and_then(|m| m.content.clone()).unwrap_or_default();
        let checking = request.messages[0].content.as_deref() == Some(crate::domain::project_rules::MEMORY_CHECK_PROMPT);
        self.summaries.lock().unwrap().push(request);
        let reply = match (checking, note.contains("approve")) {
            (true, false) => "SAVE",
            (true, true) => "REFUSE: asks to skip approvals",
            (false, _) => "they were fixing the parser",
        };
        Ok(ChatResponse {
            content: Some(reply.to_string()),
            tool_calls: Vec::new(),
            usage: None,
        })
    }

    fn chat_stream(
        &self,
        request: ChatRequest,
        on_event: &dyn Fn(StreamEvent),
        _: &dyn Fn() -> bool,
    ) -> Result<ChatStreamResult, LlmError> {
        self.requests.lock().unwrap().push(request);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| panic!("the loop asked for more rounds than the script has"));
        match step {
            Step::Reply(result) => {
                if !result.text.is_empty() {
                    on_event(StreamEvent::Text(&result.text));
                }
                Ok(result)
            }
            Step::Fail(error) => Err(error),
            Step::StreamThenFail(chunk, error) => {
                on_event(StreamEvent::Text(chunk));
                Err(error)
            }
            Step::ReplyStreamingCalls(chunks, result) => {
                for chunk in chunks {
                    for (id, name, arguments) in chunk {
                        on_event(StreamEvent::ToolCall { id, name, arguments });
                    }
                }
                Ok(result)
            }
            Step::ReplyThinking(thinking, result) => {
                on_event(StreamEvent::Reasoning(thinking));
                on_event(StreamEvent::Text(&result.text));
                Ok(result)
            }
            Step::ReplyWhileTheUserTypes(result, note) => {
                if let Some(queue) = self.steering.lock().unwrap().as_ref() {
                    queue.push(note);
                }
                if !result.text.is_empty() {
                    on_event(StreamEvent::Text(&result.text));
                }
                Ok(result)
            }
        }
    }

    fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
        unreachable!("the loop never lists models")
    }
}

/// Everything a turn needs, with the pieces a test wants to reach back
/// into kept out here.
struct Harness {
    provider: Arc<Scripted>,
    session: LlmSession,
    scope: ToolScope,
    root: PathBuf,
    events: ChatEventSink,
    log: Arc<Mutex<Vec<ChatTurnEvent>>>,
    approval: ApprovalPolicy,
    mode: ConversationMode,
    /// A chat's role instead of the folder and the mode: `Place::Chat`.
    chat: Option<ChatRole>,
    kube: KubeSetup,
    cancel_after: Arc<Mutex<Option<usize>>>,
    polls: Arc<Mutex<usize>>,
    slept: Arc<Mutex<Vec<Duration>>>,
    steering: Arc<SteeringQueue>,
    search: Option<CodeSearchFn>,
    skills: Vec<Skill>,
    rules: Vec<RuleFile>,
    frozen: Option<Frozen>,
    logged: Arc<Mutex<Vec<ToolCallLogEntry>>>,
    plan: Option<String>,
    worktree_of: Option<PathBuf>,
    mcp: McpTools,
    hooks: Hooks,
    processes: Option<Arc<dyn BackgroundProcesses>>,
    terminals: Option<Arc<dyn crate::domain::terminal::UserTerminals>>,
    shell_described: String,
    agents: Option<Arc<crate::domain::agents::Agents>>,
    questions: Option<Arc<crate::services::mcp_questions::McpQuestions>>,
    /// The agent's web, as Settings would give it; `None` is off.
    web: Option<Web>,
    features: Features,
    parallel: Vec<String>,
    parallel_calls: Arc<Mutex<usize>>,
}

fn harness(label: &str, steps: Vec<Step>) -> Harness {
    let root = temp_dir(label);
    let provider = Scripted::new(steps);
    let log: Arc<Mutex<Vec<ChatTurnEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    let steering = Arc::new(SteeringQueue::default());
    *provider.steering.lock().unwrap() = Some(steering.clone());
    Harness {
        session: LlmSession {
            provider: provider.clone(),
            provider_id: "test".to_string(),
            model: "m".to_string(),
            debug_logging: false,
            context_limit: None,
            reply_language: None,
            limits: Default::default(),
        },
        provider,
        scope: ToolScope::new(&root).expect("a scope over the temp root"),
        root,
        events: Arc::new(move |event| sink.lock().unwrap().push(event)),
        log,
        // Unattended by default: the approval gate has its own tests, and
        // every other test would otherwise pause on its first write.
        approval: ApprovalPolicy {
            skip_all: true,
            ..ApprovalPolicy::default()
        },
        mode: ConversationMode::Agent,
        chat: None,
        kube: KubeSetup::NotSet,
        cancel_after: Arc::new(Mutex::new(None)),
        polls: Arc::new(Mutex::new(0)),
        slept: Arc::new(Mutex::new(Vec::new())),
        steering,
        search: None,
        skills: Vec::new(),
        rules: Vec::new(),
        frozen: None,
        logged: Arc::new(Mutex::new(Vec::new())),
        plan: None,
        worktree_of: None,
        mcp: McpTools::default(),
        hooks: Hooks::default(),
        processes: None,
        terminals: None,
        shell_described: "/bin/sh".to_string(),
        agents: None,
        questions: None,
        web: None,
        features: Features::default(),
        parallel: Vec::new(),
        parallel_calls: Arc::new(Mutex::new(0)),
    }
}

impl Harness {
    /// Reports "cancelled" from the `n`-th poll onwards, which is how a
    /// test picks the checkpoint it wants to stop at.
    fn cancel_at_poll(&self, n: usize) {
        *self.cancel_after.lock().unwrap() = Some(n);
    }

    fn events(&self) -> Vec<ChatTurnEvent> {
        self.log.lock().unwrap().clone()
    }

    fn run<T>(&self, f: impl FnOnce(&Turn) -> T) -> T {
        let cancel_after = self.cancel_after.clone();
        let polls = self.polls.clone();
        let cancelled = move || {
            let mut polls = polls.lock().unwrap();
            *polls += 1;
            matches!(*cancel_after.lock().unwrap(), Some(n) if *polls >= n)
        };
        let slept = self.slept.clone();
        let sleep = move |d: Duration| slept.lock().unwrap().push(d);
        let shell = Shell::default();
        let queue = self.steering.clone();
        let typed = self.steering.clone();
        let take_steering = move || queue.take();
        let notes_typed = move || typed.typed();
        let logged = self.logged.clone();
        let log_call = move |entry: ToolCallLogEntry| logged.lock().unwrap().push(entry);
        let parallel = || {
            *self.parallel_calls.lock().unwrap() += 1;
            self.parallel.clone()
        };
        let turn = Turn {
            events: &self.events,
            session: &self.session,
            place: match self.chat {
                Some(role) => Place::Chat { role, kube: &self.kube, cluster: None, changes: None, runbooks: &[], web: None, scope: None },
                None => Place::Folder { scope: &self.scope, mode: self.mode, web: self.web.as_ref(), parallel: &parallel, worktree_base: None },
            },
            approval: &self.approval,
            cancelled: &cancelled,
            sleep: &sleep,
            take_steering: &take_steering,
            notes_typed: &notes_typed,
            search: self.search.clone(),
            shell: &shell,
            shell_described: &self.shell_described,
            skills: &self.skills,
            rules: &self.rules,
            frozen: self.frozen.as_ref(),
            log_call: &log_call,
            plan: self.plan.as_deref(),
            worktree_of: self.worktree_of.as_deref(),
            chat_head: None,
            mcp: &self.mcp,
            hooks: &self.hooks,
            processes: self.processes.clone(),
            terminals: self.terminals.clone(),
            review: None,
            agents: self.agents.clone(),
            questions: self.questions.clone(),
            board: None,
            mongo: None,
            deepseek: None,
            features: self.features.clone(),
            fork: false,
        };
        f(&turn)
    }
}

fn payloads(events: &[ChatTurnEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| match &e.event {
            ChatEventPayload::Delta { .. } => "delta".to_string(),
            ChatEventPayload::Reasoning { .. } => "reasoning".to_string(),
            ChatEventPayload::Retrying { .. } => "retrying".to_string(),
            ChatEventPayload::RoundStarted => "roundStarted".to_string(),
            ChatEventPayload::RoundCompleted { .. } => "roundCompleted".to_string(),
            ChatEventPayload::ToolCallDelta(_) => "toolCallDelta".to_string(),
            ChatEventPayload::ToolCall(c) => format!("toolCall:{}", c.id),
            ChatEventPayload::ToolResult(r) => format!("toolResult:{}", r.id),
            ChatEventPayload::ContextUsage(_) => "contextUsage".to_string(),
            ChatEventPayload::ContextEstimate(_) => "estimate".to_string(),
            ChatEventPayload::SteeringApplied { id, .. } => format!("steering:{id}"),
            ChatEventPayload::CommandOutput { id, .. } => format!("commandOutput:{id}"),
            ChatEventPayload::HistoryCompacted { folded } => format!("compacted:{folded}"),
            ChatEventPayload::HistoryCompacting => "compacting".to_string(),
            ChatEventPayload::HookFeedback { event, blocked, .. } => format!("hook:{event}:{blocked}"),
            ChatEventPayload::ProcessesEnded { processes } => format!("ended:{}", processes.len()),
            ChatEventPayload::LoopReminded { tool, failing } => format!("loop:{tool}:{failing}"),
            ChatEventPayload::WrapUpReminded { rounds, .. } => format!("wrap-up:{rounds}"),
            ChatEventPayload::McpQuestion { call, .. } => format!("question:{call}"),
            ChatEventPayload::McpQuestionClosed { action, .. } => format!("questionClosed:{action}"),
        })
        .collect()
}

fn tool_contents(request: &ChatRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .filter(|m| m.role == LlmRole::Tool)
        .filter_map(|m| m.content.clone())
        .collect()
}


fn call(name: &str, arguments: &str) -> LlmToolCall {
    LlmToolCall {
        id: "call_1".to_string(),
        name: name.to_string(),
        arguments: arguments.to_string(),
    }
}

fn file(content: &str) -> ToolResult {
    ToolResult::File {
        content: content.to_string(),
        start_line: 1,
        end_line: 1,
        total_lines: 1,
        clamped: false,
        truncated: false,
    }
}

fn grep() -> ToolResult {
    ToolResult::GrepResults { matches: vec![], truncated: false, total: 0, total_files: 0, total_is_floor: false, skipped: vec![] }
}

mod rules;
mod system_prompt;
mod turn_loop;
mod approval;
mod stopping;
mod model_reads;
mod commands;
mod hooks;
mod mcp;
mod steering;
mod stage;
mod explore;
mod parallel;

use approval::*;
use explore::*;
use hooks::*;
use mcp::*;
use rules::*;
use steering::*;
use system_prompt::*;
use turn_loop::*;
