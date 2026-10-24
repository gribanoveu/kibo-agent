//! A turn put together, then run: what the app holds (the open folder's index,
//! the MCP servers, the processes, the board…) and what the user left on disk
//! (the provider, skills, rules, hooks, web search, features), read per turn so
//! a change made a minute ago counts. The command layer takes the app's handles
//! out of Tauri's state, hands them over with the window's sink, and does
//! nothing else (`docs/34-architecture-audit.md`, #5).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

use crate::domain::agents::Agents;
use crate::domain::background::BackgroundProcesses;
use crate::domain::board::TaskBoard;
use crate::domain::chat_role::ChatRole;
use crate::domain::conversation_mode::ConversationMode;
use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::features::Feature;
use crate::domain::hooks::{Hooks, HooksConfigError};
use crate::domain::kube::{KubeApi, KubePin, KubeSetup};
use crate::domain::llm::LlmError;
use crate::domain::mcp::McpTools;
use crate::domain::review::ReviewDesk;
use crate::domain::settings::{SettingsError, WebSearchPlace};
use crate::domain::terminal::UserTerminals;
use crate::domain::tool_call_log::ToolCallLogEntry;
use crate::domain::tools::{ApprovalPolicy, CodeSearchFn, ToolError, ToolScope};
use crate::domain::turn::{ChatEventSink, ChatStreamOutcome, SteeringNote};
use crate::domain::web_search::WebSetupError;
use crate::infra::kube_changes::ChangeStore;
use crate::infra::kube_client::{ClusterApi, Clusters};
use crate::infra::prototype_store;
use crate::services::llm_chat::{Place, Turn, TurnError};
use crate::services::mcp_questions::McpQuestions;
use crate::services::mcp_servers::McpServers;
use crate::services::{features, kubeconfigs, llm_session, plain_chat, project_rules, prototype_versions, skills, web_search};

/// Why a turn did not start, or how it failed. Each reads as it did when the
/// command layer flattened them one by one.
#[derive(Debug, Error)]
pub enum TurnSetupError {
    #[error(transparent)]
    Session(#[from] LlmError),
    #[error(transparent)]
    Folder(#[from] ToolError),
    /// A file that does not parse stops the turn rather than running it
    /// without the hooks: a guard the user wrote, silently off, is worse than
    /// a turn that says why it did not start.
    #[error("{error} — fix or remove {path}")]
    Hooks { error: HooksConfigError, path: String },
    #[error(transparent)]
    Web(#[from] WebSetupError),
    #[error(transparent)]
    Settings(#[from] SettingsError),
    #[error(transparent)]
    Turn(#[from] TurnError),
    /// A Prototype chat's folder could not be made.
    #[error(transparent)]
    ChatFolder(#[from] crate::domain::prototype::PrototypeError),
}

/// What a turn in a folder gets from the running app — each `None` where the
/// app has none of it, as under a mock runtime.
#[derive(Default)]
pub struct FolderHandles {
    /// The open folder's search, for `semanticSearch`.
    pub search: Option<CodeSearchFn>,
    /// The knowledge base's, for `knowledgeSearch`.
    pub knowledge: Option<CodeSearchFn>,
    /// The index's model, which web search reads pages with.
    pub embeddings: Option<Arc<dyn EmbeddingProvider>>,
    pub mcp_servers: Option<Arc<McpServers>>,
    pub processes: Option<Arc<dyn BackgroundProcesses>>,
    pub agents: Option<Arc<Agents>>,
    pub questions: Option<Arc<McpQuestions>>,
    pub terminals: Option<Arc<dyn UserTerminals>>,
    pub board: Option<Arc<dyn TaskBoard>>,
}

/// What a turn in a folder is, from the window and the agent's state.
pub struct FolderTurn<'a> {
    pub workspace: &'a Path,
    pub mode: ConversationMode,
    /// A review worker's files and findings; `None` outside a review.
    pub review: Option<Arc<ReviewDesk>>,
    pub approval: ApprovalPolicy,
    pub plan: Option<&'a str>,
    pub events: &'a ChatEventSink,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub take_steering: &'a (dyn Fn() -> Vec<SteeringNote> + Sync),
    pub notes_typed: &'a (dyn Fn() -> u64 + Sync),
}

/// Puts a turn in the open folder together and hands it to `run`. Blocking:
/// the caller is already off the event loop.
pub fn run_in_folder(
    folder: FolderTurn,
    app: FolderHandles,
    run: impl FnOnce(&Turn) -> Result<ChatStreamOutcome, TurnError>,
) -> Result<ChatStreamOutcome, TurnSetupError> {
    let workspace = folder.workspace;
    let approval = with_git_aliases(folder.approval, workspace);
    let session = llm_session::resolve(None)?;
    let scope = ToolScope::new(workspace)?;
    let sleep = |d: Duration| std::thread::sleep(d);
    let shell = crate::infra::shells::for_commands();
    let shell_described = crate::domain::command_exec::describe_shell(
        &shell.program,
        crate::infra::process_runner::probe_shell(&shell).as_ref(),
    );
    // A skills folder that cannot be read costs the turn its skills, not
    // the turn itself: nothing the user asked for depends on it existing.
    let skills = skills::enabled_catalog(Some(workspace)).unwrap_or_default();
    let rules = project_rules::load(workspace);
    let record = crate::infra::tool_call_log::recorder();
    let log_call = |entry: ToolCallLogEntry| record(&entry);
    let hooks = crate::infra::hooks::load()
        .map(|config| Hooks::new(config, Arc::new(crate::infra::hooks::run)))
        .map_err(|error| TurnSetupError::Hooks {
            error,
            path: crate::infra::hooks::path().map(|p| p.display().to_string()).unwrap_or_default(),
        })?;
    let mcp = mcp_for_turn(app.mcp_servers.as_deref(), folder.mode, workspace, folder.cancelled);
    let worktree_of = crate::infra::git_head::worktree_of(workspace);
    // Read each turn, as the chat's is: a search turned on in Settings is
    // there for the next answer.
    let web = match app.embeddings.as_ref() {
        Some(model) => web_search::web_for(WebSearchPlace::Agent, model)?,
        None => None,
    };
    let features = features::enabled();
    let board = app.board.filter(|_| features.is_on(Feature::Board));
    // A base that cannot be read costs the prompt its block; the tools say why when called.
    let knowledge_overview = match features.is_on(Feature::Knowledge) {
        true => crate::services::knowledge::overview(workspace).ok(),
        false => None,
    };
    let deepseek = app.embeddings.as_ref().and_then(|model| web_search::deepseek_for(&features, model));

    let turn = Turn {
        events: folder.events,
        session: &session,
        place: Place::Folder { scope: &scope, mode: folder.mode, web: web.as_ref() },
        approval: &approval,
        cancelled: folder.cancelled,
        sleep: &sleep,
        take_steering: folder.take_steering,
        notes_typed: folder.notes_typed,
        shell: &shell,
        shell_described: &shell_described,
        search: app.search,
        knowledge: app.knowledge,
        knowledge_overview: knowledge_overview.as_ref(),
        skills: &skills,
        rules: &rules,
        log_call: &log_call,
        plan: folder.plan,
        worktree_of: worktree_of.as_deref(),
        mcp: &mcp,
        hooks: &hooks,
        processes: app.processes,
        terminals: app.terminals,
        review: folder.review,
        agents: app.agents,
        questions: app.questions,
        board,
        deepseek,
        features,
    };
    Ok(run(&turn)?)
}

/// What a chat turn is: its role and cluster from the window, the rest from
/// the app.
pub struct ChatTurnInput<'a> {
    /// The chat the turn is of: a role that works in a folder finds its folder by it.
    pub chat_id: String,
    pub role: ChatRole,
    pub kube: Option<KubePin>,
    pub clusters: Arc<Clusters>,
    pub board: Arc<dyn TaskBoard>,
    pub embeddings: Arc<dyn EmbeddingProvider>,
    pub approval: ApprovalPolicy,
    pub events: &'a ChatEventSink,
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
}

/// Puts a chat's turn together and hands it to `run`. Blocking, as
/// [`run_in_folder`].
pub fn run_in_chat(
    chat: ChatTurnInput,
    run: impl FnOnce(&plain_chat::ChatTurn) -> Result<ChatStreamOutcome, TurnError>,
) -> Result<ChatStreamOutcome, TurnSetupError> {
    // The chat's own folder, for a role that works in one; made on its first turn.
    // Held for the turn, so a restore cannot swap the folder out under a write.
    let (scope, _claim) = match chat.role.workspace() {
        true => {
            let claim = prototype_versions::claim(&chat.chat_id)?;
            (Some(ToolScope::new(&prototype_store::files_dir(&chat.chat_id)?)?), Some(claim))
        }
        false => (None, None),
    };
    // A role with a folder writes in it without a card (`ChatRole::approval`).
    let approval = chat.role.approval(&chat.approval);
    let session = llm_session::resolve(None)?;
    // Asked every turn: a cluster that stopped answering, or a login that
    // expired, is what the model most needs to know. The client is kept,
    // so it is two quick requests, not a new login.
    let kube = kubeconfigs::setup(chat.kube.as_ref(), &chat.clusters)?;
    // Read through the pin alone: the tools cannot name another cluster.
    let api = match &kube {
        KubeSetup::Pinned(target) => {
            Some(ClusterApi::new(chat.clusters.clone(), Path::new(&target.config.path), &target.context))
        }
        _ => None,
    };
    let record = crate::infra::tool_call_log::recorder();
    let log_call = |entry: ToolCallLogEntry| record(&entry);
    // Read each turn: a runbook the user just wrote is there for this answer.
    let runbooks = crate::domain::runbooks::merged(crate::infra::runbooks_store::own());
    let web = web_search::web_for(WebSearchPlace::Chat, &chat.embeddings)?;
    let features = features::enabled();
    let board = Some(chat.board).filter(|_| features.is_on(Feature::Board));
    let deepseek = web_search::deepseek_for(&features, &chat.embeddings);
    let turn = plain_chat::ChatTurn {
        session: &session,
        role: chat.role,
        kube: &kube,
        runbooks: &runbooks,
        web: web.as_ref(),
        scope: scope.as_ref(),
        cluster: api.as_ref().map(|api| api as &dyn KubeApi),
        changes: Some(&ChangeStore),
        approval: &approval,
        events: chat.events,
        cancelled: chat.cancelled,
        log_call: &log_call,
        board,
        deepseek,
        features,
    };
    Ok(run(&turn)?)
}

/// Only an Agent turn starts servers: the others cannot offer their tools,
/// and a server's start is already something done on the user's machine.
/// A configuration that does not parse costs the turn its servers, not the
/// turn — the tab says what is wrong with the file.
fn mcp_for_turn(
    servers: Option<&McpServers>,
    mode: ConversationMode,
    workspace: &Path,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> McpTools {
    match servers {
        Some(servers) if mode == ConversationMode::Agent => {
            servers.for_turn(&crate::infra::mcp_config::load().unwrap_or_default(), workspace, cancelled)
        }
        _ => McpTools::default(),
    }
}

/// The turn's policy with the workspace's git aliases in it. Read per turn:
/// an alias added in a terminal a minute ago counts.
fn with_git_aliases(mut policy: ApprovalPolicy, workspace: &Path) -> ApprovalPolicy {
    policy.git_aliases = crate::infra::git_aliases::read(workspace);
    policy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tools::ToolName;
    use crate::testing::temp_dir;

    /// A chat's turn is given the board only while its feature is on, as
    /// the agent's is.
    #[test]
    fn a_chat_turn_gets_the_board_only_while_it_is_on() {
        crate::testing::with_app_dir("turn-setup-chat", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.llm.providers = vec![crate::domain::settings::ProviderConfig {
                id: "local".into(),
                base_url: "https://local.example/v1".into(),
                model: Some("qwen".into()),
                ..Default::default()
            }];
            crate::infra::settings_store::save(&settings).unwrap();
            crate::infra::llm_credentials_store::save_api_key("local", "sk").unwrap();

            struct NoModel;
            impl EmbeddingProvider for NoModel {
                fn embed(&self, _: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
                    Err(crate::domain::embeddings::EmbeddingError::Invalid("no model in this test".into()))
                }
                fn dimensions(&self) -> usize {
                    2
                }
            }
            let events: ChatEventSink = Arc::new(|_| {});
            let no = || false;
            let has_board = |on: bool| {
                crate::infra::settings_store::update(|s| s.features.insert("board".into(), on)).unwrap();
                let chat = ChatTurnInput {
                    chat_id: "board-chat".into(),
                    role: ChatRole::Assistant,
                    kube: None,
                    clusters: Arc::new(Clusters::default()),
                    board: Arc::new(crate::infra::board_store::Board::new(None, Arc::new(|_| {}))),
                    embeddings: Arc::new(NoModel),
                    approval: ApprovalPolicy::default(),
                    events: &events,
                    cancelled: &no,
                };
                let mut seen = None;
                run_in_chat(chat, |turn| {
                    seen = Some(turn.board.is_some());
                    Ok(ChatStreamOutcome::Done(crate::domain::turn::ChatDone {
                        result: Default::default(),
                        todos: Vec::new(),
                        history: Vec::new(),
                        limit_reached: None,
                    }))
                })
                .unwrap();
                seen.expect("run was called")
            };
            assert!(!has_board(false));
            assert!(has_board(true));
        });
    }

    /// A Prototype chat's turn is handed its own folder — made on the spot —
    /// and writes there without a card; another role gets no folder and
    /// asks as it did.
    #[test]
    fn a_prototype_chat_turn_gets_its_folder_and_writes_without_a_card() {
        crate::testing::with_app_dir("turn-setup-prototype", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.llm.providers = vec![crate::domain::settings::ProviderConfig {
                id: "local".into(),
                base_url: "https://local.example/v1".into(),
                model: Some("qwen".into()),
                ..Default::default()
            }];
            crate::infra::settings_store::save(&settings).unwrap();
            crate::infra::llm_credentials_store::save_api_key("local", "sk").unwrap();

            struct NoModel;
            impl EmbeddingProvider for NoModel {
                fn embed(&self, _: &[&str]) -> Result<Vec<crate::domain::embeddings::Embedding>, crate::domain::embeddings::EmbeddingError> {
                    Err(crate::domain::embeddings::EmbeddingError::Invalid("no model in this test".into()))
                }
                fn dimensions(&self) -> usize {
                    2
                }
            }
            let events: ChatEventSink = Arc::new(|_| {});
            let no = || false;
            let id = "7a1c3e5f-2b4d-4f6a-8c0e-9d1b3f5a7c9e";
            let handed = |role: ChatRole| {
                let chat = ChatTurnInput {
                    chat_id: id.into(),
                    role,
                    kube: None,
                    clusters: Arc::new(Clusters::default()),
                    board: Arc::new(crate::infra::board_store::Board::new(None, Arc::new(|_| {}))),
                    embeddings: Arc::new(NoModel),
                    approval: ApprovalPolicy::default(),
                    events: &events,
                    cancelled: &no,
                };
                let mut seen = None;
                run_in_chat(chat, |turn| {
                    let asks = turn.approval.requires_approval(ToolName::WriteFile, true);
                    // The folder is held while the turn runs: a restore now would be refused.
                    let held = prototype_versions::claim(id).is_err();
                    seen = Some((turn.scope.map(|scope| scope.root().to_path_buf()), asks, held));
                    Ok(ChatStreamOutcome::Done(crate::domain::turn::ChatDone {
                        result: Default::default(),
                        todos: Vec::new(),
                        history: Vec::new(),
                        limit_reached: None,
                    }))
                })
                .unwrap();
                seen.expect("run was called")
            };
            let (folder, asks, held) = handed(ChatRole::Prototype);
            let folder = folder.expect("a folder");
            assert!(folder.is_dir() && folder.ends_with(std::path::Path::new("prototypes").join(id).join("files")), "{folder:?}");
            assert!(!asks);
            assert!(held, "the turn holds its folder");
            assert!(prototype_versions::claim(id).is_ok(), "and lets go of it when it ends");
            assert_eq!(handed(ChatRole::Assistant), (None, true, false));
        });
    }

    /// What a folder's turn is handed, seen by `run` before any provider
    /// call: the session the settings name, the folder's git aliases on the
    /// policy, the window's plan and mode, no web without a model — and the
    /// board only while its feature is on. Hooks that do not parse stop it.
    #[test]
    fn a_folder_turn_carries_what_the_folder_and_settings_say() {
        crate::testing::with_app_dir("turn-setup-folder", || {
            let mut settings = crate::infra::settings_store::load().unwrap();
            settings.llm.providers = vec![crate::domain::settings::ProviderConfig {
                id: "local".into(),
                base_url: "https://local.example/v1".into(),
                model: Some("qwen".into()),
                ..Default::default()
            }];
            crate::infra::settings_store::save(&settings).unwrap();
            crate::infra::llm_credentials_store::save_api_key("local", "sk").unwrap();
            let dir = temp_dir("turn-setup-folder-root");
            git2::Repository::init(&dir).unwrap().config().unwrap().set_str("alias.pf", "push --force").unwrap();

            let events: ChatEventSink = Arc::new(|_| {});
            let (no, none, zero) = (|| false, Vec::new, || 0);
            let turn = |board_on: bool| {
                let mut settings = crate::infra::settings_store::load().unwrap();
                settings.features.insert("board".into(), board_on);
                crate::infra::settings_store::save(&settings).unwrap();
                let folder = FolderTurn {
                    workspace: &dir,
                    mode: ConversationMode::Plan,
                    review: None,
                    approval: ApprovalPolicy::default(),
                    plan: Some("the plan"),
                    events: &events,
                    cancelled: &no,
                    take_steering: &none,
                    notes_typed: &zero,
                };
                let board: Arc<dyn TaskBoard> = Arc::new(crate::infra::board_store::Board::new(None, Arc::new(|_| {})));
                let mut seen = None;
                run_in_folder(folder, FolderHandles { board: Some(board), ..Default::default() }, |turn| {
                    assert_eq!(turn.session.provider_id, "local");
                    assert_eq!(turn.approval.git_aliases.get("pf").map(String::as_str), Some("push --force"));
                    assert_eq!(turn.plan, Some("the plan"));
                    assert!(matches!(turn.place, Place::Folder { mode: ConversationMode::Plan, web: None, .. }));
                    seen = Some(turn.board.is_some());
                    Ok(ChatStreamOutcome::Done(crate::domain::turn::ChatDone {
                        result: Default::default(),
                        todos: Vec::new(),
                        history: Vec::new(),
                        limit_reached: None,
                    }))
                })
                .map(|_| seen.expect("run was called"))
            };
            assert!(!turn(false).unwrap(), "the board is off until its feature is");
            assert!(turn(true).unwrap());

            std::fs::write(crate::infra::hooks::path().unwrap(), "{ not json").unwrap();
            let refused = turn(true).expect_err("hooks that do not parse").to_string();
            assert!(refused.contains("fix or remove"), "{refused}");
        });
    }

    #[test]
    fn only_an_agent_turn_starts_mcp_servers() {
        crate::testing::with_app_dir("cmd-mcp-mode", || {
            crate::infra::mcp_config::save_text(r#"{"mcpServers":{"a":{"command":"x"}}}"#).unwrap();
            let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let counted = Arc::clone(&starts);
            let servers = McpServers::new(Arc::new(move |_, _, _| {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(crate::domain::mcp::McpError::NotStarted("test".into()))
            }), Arc::new(|_| {}));
            let root = temp_dir("cmd-mcp-mode-root");
            for mode in [ConversationMode::Plan, ConversationMode::Ask] {
                mcp_for_turn(Some(&servers), mode, &root, &|| false);
            }
            assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 0);
            mcp_for_turn(Some(&servers), ConversationMode::Agent, &root, &|| false);
            assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    /// Each turn reads the folder's git aliases, keeping what the session
    /// already allowed.
    #[test]
    fn a_turn_s_policy_carries_the_workspace_s_git_aliases() {
        let dir = temp_dir("cmd-git-aliases");
        git2::Repository::init(&dir).unwrap().config().unwrap().set_str("alias.pf", "push --force").unwrap();
        let mut base = ApprovalPolicy::default();
        base.always_allowed.insert(ToolName::RunCommand);

        let policy = with_git_aliases(base, &dir);
        assert_eq!(policy.git_aliases.get("pf").map(String::as_str), Some("push --force"));
        assert!(policy.always_allowed.contains(&ToolName::RunCommand));
    }
}
