//! One turn in Chat mode: the agent's own loop (`services::llm_chat`) in a
//! chat's place — the role's prompt and tools, and no folder
//! (`docs/21-kubernetes-mode.md`, K-1). Rounds, approval, a stop, retries and
//! the loop guard are the agent's; what a chat does not have — a folder, a
//! shell, skills, rules, hooks, MCP servers — is left empty.

use std::time::Duration;

use crate::domain::chat_role::ChatRole;
use crate::domain::kube::{KubeApi, KubeSetup};
use crate::domain::command_exec::Shell;
use crate::domain::hooks::Hooks;
use crate::domain::llm::LlmMessage;
use crate::domain::mcp::McpTools;
use crate::domain::tool_call_log::ToolCallLogEntry;
use crate::domain::tools::ApprovalPolicy;
use crate::domain::turn::{ChatEventSink, ChatStreamOutcome, PendingApproval, SteeringNote, ToolCallDecision};
use crate::services::llm_chat::{self, Place, Turn, TurnError};
use crate::services::llm_session::LlmSession;

/// What a chat's turn runs with, handed over by the command layer.
pub struct ChatTurn<'a> {
    pub session: &'a LlmSession,
    pub role: ChatRole,
    /// What the role is told of the user's cluster.
    pub kube: &'a KubeSetup,
    /// What its tools read that cluster through, when one is pinned.
    pub cluster: Option<&'a dyn KubeApi>,
    pub approval: &'a ApprovalPolicy,
    pub events: &'a ChatEventSink,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub log_call: &'a (dyn Fn(ToolCallLogEntry) + Sync),
}

/// A fresh turn on `messages`, the whole conversation so far.
pub fn start(chat: &ChatTurn, messages: Vec<LlmMessage>) -> Result<ChatStreamOutcome, TurnError> {
    in_place(chat, |turn| llm_chat::stream(turn, messages, Vec::new()))
}

/// Continues a turn that paused on the approval card.
pub fn resume(
    chat: &ChatTurn,
    checkpoint: PendingApproval,
    decisions: Vec<ToolCallDecision>,
) -> Result<ChatStreamOutcome, TurnError> {
    in_place(chat, |turn| llm_chat::resume(turn, checkpoint, decisions))
}

fn in_place<T>(chat: &ChatTurn, run: impl FnOnce(&Turn) -> T) -> T {
    let sleep = |delay: Duration| std::thread::sleep(delay);
    // ponytail: no steering — the chat's box is disabled while a turn runs;
    // the agent's queue when a role's turns get long enough to want it.
    let take_steering = Vec::<SteeringNote>::new;
    let shell = Shell::default();
    let mcp = McpTools::default();
    let hooks = Hooks::default();
    let turn = Turn {
        events: chat.events,
        session: chat.session,
        place: Place::Chat { role: chat.role, kube: chat.kube, cluster: chat.cluster },
        approval: chat.approval,
        cancelled: chat.cancelled,
        sleep: &sleep,
        shell: &shell,
        shell_described: "",
        take_steering: &take_steering,
        search: None,
        skills: &[],
        rules: &[],
        log_call: chat.log_call,
        plan: None,
        worktree_of: None,
        mcp: &mcp,
        hooks: &hooks,
        processes: None,
        terminals: None,
        review: None,
        agents: None,
    };
    run(&turn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::llm::{
        ChatRequest, ChatResponse, ChatStreamResult, LlmError, LlmModelInfo, LlmProvider, LlmRole, LlmToolCall,
    };
    use crate::domain::kube::{Access, KubeError, KubeKind, KubeTarget, ListPage, ListQuery, LogQuery, Reach};
    use crate::domain::turn::{ChatEventPayload, ChatTurnEvent};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Answers each round with the next scripted reply, text in two deltas, and
    /// records what it was asked.
    struct Script {
        replies: Mutex<VecDeque<ChatStreamResult>>,
        asked: Mutex<Vec<ChatRequest>>,
    }

    impl LlmProvider for Script {
        fn chat(&self, _: ChatRequest) -> Result<ChatResponse, LlmError> {
            unreachable!("a chat turn is streamed")
        }

        fn chat_stream(
            &self,
            request: ChatRequest,
            on_delta: &dyn Fn(&str),
            _: &dyn Fn(&str),
            _: &dyn Fn(&str, &str, &str),
            _: &dyn Fn() -> bool,
        ) -> Result<ChatStreamResult, LlmError> {
            self.asked.lock().unwrap().push(request);
            let reply = self.replies.lock().unwrap().pop_front().expect("a reply left in the script");
            let (head, tail) = reply.text.split_at(reply.text.len() / 2);
            on_delta(head);
            on_delta(tail);
            Ok(reply)
        }

        fn list_models(&self) -> Result<Vec<LlmModelInfo>, LlmError> {
            unreachable!("a chat turn never lists models")
        }
    }

    fn said(text: &str) -> ChatStreamResult {
        ChatStreamResult { text: text.to_string(), ..Default::default() }
    }

    fn calls(name: &str, arguments: &str) -> ChatStreamResult {
        ChatStreamResult {
            tool_calls: vec![LlmToolCall { id: "c1".into(), name: name.into(), arguments: arguments.into() }],
            ..said("")
        }
    }

    struct Chat {
        session: LlmSession,
        script: Arc<Script>,
        kube: KubeSetup,
        cluster: Option<Arc<dyn KubeApi>>,
        approval: ApprovalPolicy,
        seen: Arc<Mutex<Vec<ChatTurnEvent>>>,
    }

    fn chat(replies: Vec<ChatStreamResult>, language: Option<&'static str>) -> Chat {
        let script = Arc::new(Script { replies: Mutex::new(replies.into()), asked: Mutex::default() });
        Chat {
            session: LlmSession {
                provider: script.clone(),
                provider_id: "test".to_string(),
                model: "m".to_string(),
                debug_logging: false,
                context_limit: None,
                reply_language: language,
            },
            script,
            kube: KubeSetup::NotSet,
            cluster: None,
            approval: ApprovalPolicy::default(),
            seen: Arc::default(),
        }
    }

    impl Chat {
        fn run<T>(&self, role: ChatRole, cancelled: bool, f: impl FnOnce(&ChatTurn) -> T) -> T {
            let seen = self.seen.clone();
            let events: ChatEventSink = Arc::new(move |event| seen.lock().unwrap().push(event));
            let stop = move || cancelled;
            let log = |_: ToolCallLogEntry| {};
            f(&ChatTurn {
                session: &self.session,
                role,
                kube: &self.kube,
                cluster: self.cluster.as_deref(),
                approval: &self.approval,
                events: &events,
                cancelled: &stop,
                log_call: &log,
            })
        }

        fn start(&self, role: ChatRole, messages: Vec<LlmMessage>) -> ChatStreamOutcome {
            self.run(role, false, |turn| start(turn, messages)).unwrap()
        }

        fn asked(&self) -> Vec<ChatRequest> {
            self.script.asked.lock().unwrap().clone()
        }
    }

    fn done(outcome: ChatStreamOutcome) -> crate::domain::turn::ChatDone {
        match outcome {
            ChatStreamOutcome::Done(done) => done,
            other => panic!("the turn did not finish: {other:?}"),
        }
    }

    /// The last tool result the model was given.
    fn tool_result(history: &[LlmMessage]) -> String {
        history.iter().rev().find(|m| m.role == LlmRole::Tool).and_then(|m| m.content.clone()).unwrap_or_default()
    }

    #[test]
    fn the_role_speaks_first_the_conversation_follows_and_no_tool_is_offered() {
        let chat = chat(vec![said("hello")], Some("Russian"));
        let done = done(chat.start(ChatRole::Assistant, vec![LlmMessage::user("hi")]));

        assert_eq!(done.result.text, "hello");
        let asked = chat.asked();
        let messages = &asked[0].messages;
        assert_eq!(messages[0].role, LlmRole::System);
        let system = messages[0].content.as_deref().unwrap();
        assert!(system.starts_with(ChatRole::Assistant.prompt()) && system.ends_with("Reply in Russian."), "{system}");
        assert_eq!(messages[1], LlmMessage::user("hi"));
        assert!(asked[0].tools.is_empty());
        assert_eq!(done.history.last(), Some(&LlmMessage::assistant("hello")), "the answer closes the history");

        let deltas: Vec<String> = chat
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match &event.event {
                ChatEventPayload::Delta { delta } => Some(delta.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, ["he", "llo"]);
    }

    /// The second turn's request is the first's plus what was said since — the
    /// prefix a provider's prompt cache reuses. Anything rebuilt per turn (a
    /// date, a reordered note) would break it at the system prompt.
    #[test]
    fn a_second_turn_extends_the_first_request() {
        let chat = chat(vec![said("hello"), said("more")], Some("Russian"));
        let first = done(chat.start(ChatRole::Kubernetes, vec![LlmMessage::user("hi")])).history;
        chat.start(ChatRole::Kubernetes, [first, vec![LlmMessage::user("and?")]].concat());

        let asked = chat.asked();
        let (a, b) = (&asked[0].messages, &asked[1].messages);
        assert_eq!(&b[..a.len()], &a[..], "the second request does not start with the first");
        assert_eq!(asked[0].tools, asked[1].tools);
        let system = a[0].content.as_deref().unwrap();
        assert!(system.contains(&ChatRole::Kubernetes.setup_note(&chat.kube).unwrap()), "the setup was not told: {system}");
    }

    /// The role's tools and nothing else — none of the agent's, however the
    /// model remembers them.
    #[test]
    fn a_role_is_offered_its_own_tools() {
        let chat = chat(vec![said("ok")], None);
        chat.start(ChatRole::Tester, vec![LlmMessage::user("hi")]);
        let offered: Vec<String> = chat.asked()[0].tools.iter().map(|t| t.name.clone()).collect();
        assert_eq!(offered, ["deleteFile", "todo"]);
    }

    /// A tool that changes nothing runs without a card, and the turn goes on.
    #[test]
    fn a_harmless_tool_runs_and_the_turn_carries_on() {
        let todo = r#"{"op":"write","tasks":["look"]}"#;
        let chat = chat(vec![calls("todo", todo), said("listed")], None);
        let done = done(chat.start(ChatRole::Tester, vec![LlmMessage::user("plan")]));

        assert_eq!(done.result.text, "listed");
        assert!(tool_result(&done.history).contains("look"), "{:?}", done.history);
        assert_eq!(chat.asked().len(), 2);
    }

    /// A tool the role does not have is refused before it runs, and the model
    /// is told why — not handed a folder that does not exist.
    #[test]
    fn a_tool_outside_the_role_is_refused_and_the_model_told() {
        let chat = chat(vec![calls("readFile", r#"{"path":"a.txt"}"#), said("sorry")], None);
        let done = done(chat.start(ChatRole::Assistant, vec![LlmMessage::user("read it")]));

        let refused = tool_result(&done.history);
        assert!(refused.contains("not available in this chat"), "{refused}");
        assert_eq!(done.result.text, "sorry");
    }

    /// A tool that asks pauses the turn with nothing run; the answer resumes it.
    #[test]
    fn a_risky_tool_waits_for_the_card_and_runs_on_approval() {
        let chat = chat(vec![calls("deleteFile", r#"{"path":"a.txt"}"#), said("could not")], None);
        let paused = match chat.start(ChatRole::Tester, vec![LlmMessage::user("delete it")]) {
            ChatStreamOutcome::PendingApproval(paused) => paused,
            other => panic!("expected a card: {other:?}"),
        };
        assert!(paused.calls[0].requires_confirmation);

        let approve = vec![ToolCallDecision { id: "c1".into(), approved: true, reason: None }];
        let done = done(chat.run(ChatRole::Tester, false, |turn| resume(turn, paused, approve)).unwrap());
        // Approved and run — and a chat has no folder to delete from.
        let ran = tool_result(&done.history);
        assert!(ran.contains("works in a folder, and this chat has none"), "{ran}");
        assert_eq!(done.result.text, "could not");
    }

    #[test]
    fn a_denied_call_tells_the_model_the_users_reason() {
        let chat = chat(vec![calls("deleteFile", r#"{"path":"a.txt"}"#), said("ok, kept")], None);
        let ChatStreamOutcome::PendingApproval(paused) = chat.start(ChatRole::Tester, vec![LlmMessage::user("x")]) else {
            panic!("expected a card");
        };
        let deny = vec![ToolCallDecision { id: "c1".into(), approved: false, reason: Some("keep it".into()) }];
        let done = done(chat.run(ChatRole::Tester, false, |turn| resume(turn, paused, deny)).unwrap());
        assert_eq!(tool_result(&done.history), "Denied by the user: keep it");
    }

    /// A cluster of one pod, in whatever namespace it is asked about.
    struct OnePod;

    impl KubeApi for OnePod {
        fn kinds(&self) -> Result<Vec<KubeKind>, KubeError> {
            Ok(vec![KubeKind::core("Pod", "pods")])
        }

        fn list(&self, _: &KubeKind, query: &ListQuery) -> Result<ListPage, KubeError> {
            let pod = serde_json::json!({"metadata": {"name": "api-1", "namespace": query.namespace}});
            Ok(ListPage { items: vec![pod], ..Default::default() })
        }

        fn get(&self, _: &KubeKind, _: &str, _: &str) -> Result<serde_json::Value, KubeError> {
            unreachable!("only listed")
        }

        fn logs(&self, _: &str, _: &str, _: &LogQuery) -> Result<String, KubeError> {
            unreachable!("only listed")
        }
    }

    fn pinned(namespace: &str) -> KubeSetup {
        KubeSetup::Pinned(KubeTarget {
            config: crate::domain::settings::Kubeconfig { name: "prod".into(), path: "/k/prod".into() },
            context: "eks".into(),
            cluster: "eks".into(),
            namespace: namespace.into(),
            reach: Reach::Answered { version: "v1.33.1".into(), access: Access::ReadOnly },
        })
    }

    /// K-3 end to end: the role's read reaches the pinned cluster, in the
    /// pinned namespace, and the model gets the table.
    #[test]
    fn a_pinned_chat_reads_its_cluster_in_its_namespace() {
        let mut chat = chat(vec![calls("kubeList", r#"{"kind":"pods"}"#), said("one pod")], None);
        chat.kube = pinned("payments");
        chat.cluster = Some(Arc::new(OnePod));
        let done = done(chat.start(ChatRole::Kubernetes, vec![LlmMessage::user("pods?")]));
        let read = tool_result(&done.history);
        assert!(read.starts_with("1 pods in namespace payments:") && read.contains("api-1"), "{read}");
        assert_eq!(done.result.text, "one pod");
    }

    /// A cluster the setup does not vouch for is not read: the model is told
    /// there is none, and says what to set up.
    #[test]
    fn a_chat_with_no_cluster_pinned_reads_nothing() {
        let mut chat = chat(vec![calls("kubeList", r#"{"kind":"pods"}"#), said("pick one")], None);
        chat.cluster = Some(Arc::new(OnePod));
        let done = done(chat.start(ChatRole::Kubernetes, vec![LlmMessage::user("pods?")]));
        assert!(tool_result(&done.history).contains("no cluster is pinned"), "{:?}", done.history);
    }

    /// Stopped before the first round: nothing asked, nothing run.
    #[test]
    fn a_stop_ends_the_turn() {
        let chat = chat(vec![], None);
        let outcome = chat.run(ChatRole::Tester, true, |turn| start(turn, vec![LlmMessage::user("x")])).unwrap();
        assert!(matches!(outcome, ChatStreamOutcome::Cancelled(_)), "{outcome:?}");
        assert!(chat.asked().is_empty());
    }
}
