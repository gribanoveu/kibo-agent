//! The bench for the agent itself: small repositories with something wrong,
//! a request in the user's words, and a hidden check of the result.
//!
//! Ignored by default — every run is a real model spending real tokens:
//!
//! ```text
//! AGENT_BENCH_API_KEY=… AGENT_BENCH_MODEL=claude-sonnet-5 \
//!   cargo test --release agent_bench -- --ignored --nocapture
//! ```
//!
//! What it measures is the harness with one model held fixed: the loop in
//! `llm_chat`, the prompt, the tools and what they say back. Change one of
//! those, run again with the same model, and compare. A model change is a
//! different experiment.
//!
//! The provider comes from the environment rather than the app's settings, so
//! a run never reads or writes the user's configuration:
//!
//! - `AGENT_BENCH_API_KEY`, `AGENT_BENCH_MODEL` — required.
//! - `AGENT_BENCH_KIND` — `anthropic` (default) or `openai` (any
//!   OpenAI-compatible endpoint).
//! - `AGENT_BENCH_BASE_URL` — defaults to the vendor's own `/v1`.
//! - `AGENT_BENCH_MAX_TOKENS`, `AGENT_BENCH_REASONING_EFFORT` — as in the
//!   provider settings.
//! - `AGENT_BENCH_TRUSTED_CERT` — path to a PEM file, for a gateway with its
//!   own certificate or a proxy with its own CA; replaces the trust store, as
//!   the provider setting does.
//! - `AGENT_BENCH_RUNS` — runs per task, default 1. A model is not
//!   deterministic: compare versions on three or more.
//! - `AGENT_BENCH_TASKS` — comma-separated substrings of task names to run.
//! - `AGENT_BENCH_TIMEOUT_SECS` — a run is stopped after this, default 600.
//!
//! Tasks live in `bench/agent-tasks/` (see its README). Every run gets a fresh
//! copy of the task's `repo/`, committed to a new Git repository as the user's
//! folder would be. A failed run's copy is left in the temp dir and its path
//! printed; each run is also a line in `target/agent-bench/<unix time>.jsonl`.
//!
//! `semanticSearch` works as in the app: the copy is indexed with the bundled
//! model before the turn (so `git lfs pull` first), and synced again before
//! every search, as the app's watcher would after an edit.
//!
//! Not wired in yet: skills, MCP, hooks, background processes and terminals.
//! A task that needs one of them needs it wired here first.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use secrecy::SecretString;
use serde_json::json;

use crate::domain::chunk_index::ChunkBuildOptions;
use crate::domain::command_exec::{Shell, describe_shell};
use crate::domain::embeddings::EmbeddingProvider;
use crate::domain::conversation_mode::ConversationMode;
use crate::domain::hooks::Hooks;
use crate::domain::llm::LlmMessage;
use crate::domain::mcp::McpTools;
use crate::domain::result_clearing::STUB_PREFIX;
use crate::domain::settings::{DEFAULT_CONTEXT_LIMIT, ProviderConfig, ProviderKind};
use crate::domain::tool_call_log::{CallStatus, ToolCallLogEntry};
use crate::domain::tools::{ApprovalPolicy, CodeSearchFn, ToolScope};
use crate::domain::turn::{ChatEventPayload, ChatEventSink, ChatStreamOutcome, ChatTurnEvent};
use crate::domain::workspace_index::IndexEventSink;
use crate::infra::llm_providers::provider_for;
use crate::infra::local_embeddings::{DEFAULT_IDLE_UNLOAD, LocalEmbeddings, bundled_model_dir};
use crate::infra::process_runner::probe_shell;
use crate::services::code_search;
use crate::services::index_sync::RepoIndexer;
use crate::services::llm_chat::{self, SteeringQueue, Turn};
use crate::services::llm_session::LlmSession;
use crate::services::project_rules;
use crate::testing::temp_dir;

pub(super) fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub(super) fn session() -> LlmSession {
    let required = |name: &str| var(name).unwrap_or_else(|| panic!("{name} is not set — see the top of agent_bench.rs"));
    let kind = match var("AGENT_BENCH_KIND").as_deref() {
        None | Some("anthropic") => ProviderKind::Anthropic,
        Some("openai") => ProviderKind::OpenAiCompatible,
        Some(other) => panic!("AGENT_BENCH_KIND is anthropic or openai, not {other}"),
    };
    let base_url = var("AGENT_BENCH_BASE_URL").unwrap_or_else(|| match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com/v1".to_string(),
        ProviderKind::OpenAiCompatible => "https://api.openai.com/v1".to_string(),
    });
    let model = required("AGENT_BENCH_MODEL");
    let config = ProviderConfig {
        id: "agent-bench".to_string(),
        kind,
        base_url,
        model: Some(model.clone()),
        max_tokens: var("AGENT_BENCH_MAX_TOKENS").map(|n| n.parse().expect("AGENT_BENCH_MAX_TOKENS is a number")),
        reasoning_effort: var("AGENT_BENCH_REASONING_EFFORT"),
        trusted_cert_pem: var("AGENT_BENCH_TRUSTED_CERT")
            .map(|path| std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("AGENT_BENCH_TRUSTED_CERT {path}: {e}"))),
        ..ProviderConfig::default()
    };
    let key = SecretString::from(required("AGENT_BENCH_API_KEY"));
    LlmSession {
        provider: Arc::from(provider_for(&config, Some(key)).expect("a provider from the AGENT_BENCH_* settings")),
        provider_id: config.id,
        model,
        debug_logging: false,
        context_limit: Some(DEFAULT_CONTEXT_LIMIT),
        reply_language: None,
        limits: Default::default(),
    }
}

struct Task {
    name: String,
    dir: PathBuf,
    prompt: String,
}

fn tasks(root: &Path) -> Vec<Task> {
    let filter: Vec<String> = var("AGENT_BENCH_TASKS")
        .map(|f| f.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let mut tasks: Vec<Task> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|dir| dir.join("task.md").is_file())
        .map(|dir| Task {
            name: dir.file_name().unwrap().to_string_lossy().into_owned(),
            prompt: std::fs::read_to_string(dir.join("task.md")).unwrap().trim().to_string(),
            dir,
        })
        .filter(|t| filter.is_empty() || filter.iter().any(|f| t.name.contains(f.as_str())))
        .collect();
    tasks.sort_by(|a, b| a.name.cmp(&b.name));
    tasks
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// The fixture as the user's folder would be: a repository with one commit,
/// so that `git diff` shows the agent's work and nothing else. Line endings
/// are stored as they are, whatever the user's global `core.autocrlf` says:
/// `crlf-edit` is about exactly those bytes.
///
/// The folder is not named after the task: the agent sees its path, and a
/// model that read "crlf-edit" in it went around `editFile` on the hint alone.
fn workspace(task: &Task, run: usize) -> PathBuf {
    let work = temp_dir(&format!("agent-bench-{run}")).canonicalize().unwrap();
    copy_dir(&task.dir.join("repo"), &work);
    for args in [
        &["init", "-q"][..],
        &["config", "core.autocrlf", "false"],
        &["add", "-A"],
        &["-c", "user.name=bench", "-c", "user.email=bench@localhost", "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"],
    ] {
        let status = Command::new("git").args(args).current_dir(&work).status().unwrap();
        assert!(status.success(), "git {args:?} failed in {}", work.display());
    }
    work
}

/// Where a run's index lives: beside the copy, not in it, where `listFiles`
/// would show it to the agent.
fn index_dir(work: &Path) -> PathBuf {
    PathBuf::from(format!("{}-index", work.display()))
}

/// `semanticSearch` over the copy, as the app's `WorkspaceIndex::searcher`
/// builds it.
fn searcher(work: &Path, model: Arc<dyn EmbeddingProvider>) -> CodeSearchFn {
    let indexer = Arc::new(RepoIndexer::open(work, &index_dir(work), model, ChunkBuildOptions::default()).unwrap());
    let quiet: IndexEventSink = Arc::new(|_| {});
    indexer.sync(&quiet).unwrap();
    Arc::new(move |queries, fts, top_k, filter| {
        // Only what changed is read again, so on these folders it is cheap.
        indexer.sync(&quiet).map_err(|e| e.to_string())?;
        code_search::search_many(&indexer, queries, fts, top_k, filter).map_err(|e| e.to_string())
    })
}

/// Everything one run is judged by.
struct Run {
    task: String,
    passed: bool,
    /// `done`, `cancelled` (the time limit), `paused` (asked for approval,
    /// which it must not with everything allowed) or the turn's error.
    ended: String,
    check_output: String,
    answer: String,
    rounds: usize,
    calls: Vec<ToolCallLogEntry>,
    /// Calls with the same tool and arguments as an earlier one in the run —
    /// a sign the model did not understand, or lost, the first result.
    repeats: usize,
    /// Old results the turn replaced with stubs (`domain::result_clearing`).
    cleared: usize,
    /// Whether the loop guard told the model it was going round in circles
    /// (`domain::loop_guard`) — at most once a turn.
    loop_reminded: bool,
    tokens_in: u64,
    /// The part of `tokens_in` the provider served from its prompt cache —
    /// billed at a fraction, so `tokens_in` alone overstates the cost.
    tokens_cached: u64,
    tokens_out: u64,
    /// Each round: tokens in, cached and out, whether the length limit cut it,
    /// and the start of what it said and thought. A clearing or a changed
    /// checklist shows as the round where the cached share drops; a blank
    /// reply shows why it was blank.
    per_round: Vec<serde_json::Value>,
    seconds: f64,
    workspace: PathBuf,
}

impl Run {
    fn errors(&self) -> impl Iterator<Item = &ToolCallLogEntry> {
        self.calls.iter().filter(|c| c.status == CallStatus::Error)
    }

    fn json(&self) -> serde_json::Value {
        let clip = |s: &str| s.chars().take(400).collect::<String>();
        json!({
            "task": self.task,
            "passed": self.passed,
            "ended": self.ended,
            "rounds": self.rounds,
            "calls": self.calls.len(),
            "toolErrors": self.errors().map(|c| json!({
                "tool": c.tool,
                "args": c.args,
                "error": clip(c.error.as_deref().unwrap_or("")),
            })).collect::<Vec<_>>(),
            // Every call in order, as the call log redacts it (edit texts are
            // hidden, commands are not) — what a pass that took twice the
            // rounds actually did.
            "trace": self.calls.iter().map(|c| json!({
                "round": c.round,
                "tool": c.tool,
                "args": c.args,
                "status": c.status.as_str(),
            })).collect::<Vec<_>>(),
            "repeats": self.repeats,
            "cleared": self.cleared,
            "loopReminded": self.loop_reminded,
            "tokensIn": self.tokens_in,
            "tokensCached": self.tokens_cached,
            "tokensOut": self.tokens_out,
            "perRound": self.per_round,
            "seconds": self.seconds,
            "check": clip(&self.check_output),
            "answer": clip(&self.answer),
            "workspace": self.workspace,
        })
    }
}

fn run_task(session: &LlmSession, model: &Arc<dyn EmbeddingProvider>, task: &Task, run: usize, limit: Duration) -> Run {
    let work = workspace(task, run);
    let scope = ToolScope::new(&work).unwrap();
    let search = searcher(&work, Arc::clone(model));
    let rules = project_rules::load(&work);

    let events_seen: Arc<Mutex<Vec<ChatTurnEvent>>> = Arc::default();
    let sink = Arc::clone(&events_seen);
    let events: ChatEventSink = Arc::new(move |e| sink.lock().unwrap().push(e));
    let calls: Arc<Mutex<Vec<ToolCallLogEntry>>> = Arc::default();
    let logged = Arc::clone(&calls);
    let log_call = move |entry: ToolCallLogEntry| logged.lock().unwrap().push(entry);

    let started = Instant::now();
    let cancelled = || started.elapsed() > limit;
    let sleep = |d: Duration| std::thread::sleep(d);
    let steering = SteeringQueue::default();
    let take_steering = || steering.take();
    let notes_typed = || steering.typed();
    let shell = Shell::default();
    let shell_described = describe_shell(&shell.program, probe_shell(&shell).as_ref());
    let approval = ApprovalPolicy { skip_all: true, ..ApprovalPolicy::default() };
    let mcp = McpTools::default();
    let hooks = Hooks::default();
    let agents = Arc::new(crate::domain::agents::Agents::default());

    let turn = Turn {
        events: &events,
        session,
        place: llm_chat::Place::Folder { scope: &scope, mode: ConversationMode::Agent },
        approval: &approval,
        cancelled: &cancelled,
        sleep: &sleep,
        shell: &shell,
        shell_described: &shell_described,
        take_steering: &take_steering,
        notes_typed: &notes_typed,
        search: Some(search),
        skills: &[],
        rules: &rules,
        log_call: &log_call,
        plan: None,
        worktree_of: None,
        mcp: &mcp,
        hooks: &hooks,
        processes: None,
        terminals: None,
        review: None,
        agents: Some(Arc::clone(&agents)),
        questions: None,
    };
    let outcome = llm_chat::stream(&turn, vec![LlmMessage::user(task.prompt.clone())], Vec::new());
    let seconds = started.elapsed().as_secs_f64();

    // Stubs left in the history: how many results the turn cleared.
    let stubs = |history: &[LlmMessage]| {
        history.iter().filter(|m| m.content.as_deref().is_some_and(|c| c.starts_with(STUB_PREFIX))).count()
    };
    let (ended, answer, cleared) = match outcome {
        Ok(ChatStreamOutcome::Done(done)) if done.limit_reached.is_some() => ("exhausted".to_string(), done.result.text, stubs(&done.history)),
        Ok(ChatStreamOutcome::Done(done)) => ("done".to_string(), done.result.text, stubs(&done.history)),
        Ok(ChatStreamOutcome::Cancelled(done)) => ("cancelled".to_string(), done.result.text, stubs(&done.history)),
        Ok(ChatStreamOutcome::PendingApproval(_)) => ("paused".to_string(), String::new(), 0),
        Err(e) => (format!("error: {e}"), String::new(), 0),
    };

    let check = Command::new("sh")
        .arg(task.dir.join("check.sh"))
        .current_dir(&work)
        .env("WORKSPACE", &work)
        .env("ORIG", task.dir.join("repo"))
        .env("TASK", &task.dir)
        .output()
        .unwrap();
    let check_output = format!("{}{}", String::from_utf8_lossy(&check.stdout), String::from_utf8_lossy(&check.stderr));

    let events = events_seen.lock().unwrap();
    let rounds = events.iter().filter(|e| matches!(e.event, ChatEventPayload::RoundCompleted { .. })).count();
    let (tokens_in, tokens_cached, tokens_out) = tokens_spent(&events, &agents.list());
    let mut by_round: BTreeMap<u32, serde_json::Map<String, serde_json::Value>> = BTreeMap::new();
    let clip = |s: &str| s.chars().take(300).collect::<String>();
    for e in events.iter() {
        let entry = by_round.entry(e.round).or_default();
        match &e.event {
            ChatEventPayload::ContextUsage(u) => {
                entry.insert("in".into(), json!(u.prompt_tokens));
                entry.insert("cached".into(), json!(u.cached_tokens));
                entry.insert("out".into(), json!(u.completion_tokens));
            }
            ChatEventPayload::LoopReminded { tool, failing } => {
                entry.insert("loopReminded".into(), json!({ "tool": tool, "failing": failing }));
            }
            ChatEventPayload::RoundCompleted { text, reasoning, truncated } => {
                entry.insert("text".into(), json!(clip(text)));
                entry.insert("reasoning".into(), json!(clip(reasoning)));
                entry.insert("reasoningChars".into(), json!(reasoning.chars().count()));
                entry.insert("truncated".into(), json!(truncated));
            }
            _ => {}
        }
    }
    let per_round: Vec<serde_json::Value> = by_round
        .into_iter()
        .filter(|(_, entry)| entry.contains_key("text"))
        .map(|(round, mut entry)| {
            entry.insert("round".into(), json!(round));
            serde_json::Value::Object(entry)
        })
        .collect();
    let loop_reminded = events.iter().any(|e| matches!(e.event, ChatEventPayload::LoopReminded { .. }));
    let calls = std::mem::take(&mut *calls.lock().unwrap());
    let mut seen = HashSet::new();
    let repeats = calls.iter().filter(|c| !seen.insert((c.tool.clone(), c.args.to_string()))).count();

    Run {
        task: task.name.clone(),
        passed: check.status.success() && ended == "done",
        ended,
        check_output,
        answer,
        rounds,
        calls,
        repeats,
        cleared,
        loop_reminded,
        tokens_in,
        tokens_cached,
        tokens_out,
        per_round,
        seconds,
        workspace: work,
    }
}

/// What a run cost — in, of it cached, out: every request the turn made,
/// and its helpers' too. Delegating is not free because the chat's meter
/// does not show it.
fn tokens_spent(events: &[ChatTurnEvent], helpers: &[crate::domain::agents::AgentInfo]) -> (u64, u64, u64) {
    let helped = helpers.iter().fold((0u64, 0u64, 0u64), |(i, c, o), run| {
        (i + run.tokens.prompt, c + run.tokens.cached, o + run.tokens.completion)
    });
    events.iter().fold(helped, |(i, c, o), e| match &e.event {
        ChatEventPayload::ContextUsage(u) => {
            (i + u64::from(u.prompt_tokens), c + u64::from(u.cached_tokens), o + u64::from(u.completion_tokens))
        }
        _ => (i, c, o),
    })
}

/// A run that delegated is charged for its helpers.
#[test]
fn a_run_costs_its_own_requests_and_its_helpers() {
    use crate::domain::agents::{AgentInfo, AgentState, AgentTokens};
    use crate::domain::llm::ChatUsage;
    let usage = |prompt: u32, cached: u32, completion: u32| ChatTurnEvent {
        seq: 0,
        round: 1,
        target_id: None,
        event: ChatEventPayload::ContextUsage(ChatUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: cached,
        }),
    };
    let helper = AgentInfo {
        id: 1,
        task: "t".into(),
        state: AgentState::Done,
        steps: vec![],
        tokens: AgentTokens { prompt: 5000, cached: 4000, completion: 300 },
        answer: None,
    };
    let events = [usage(1000, 800, 50), usage(1200, 1000, 70)];
    assert_eq!(tokens_spent(&events, &[]), (2200, 1800, 120));
    assert_eq!(tokens_spent(&events, &[helper]), (7200, 5800, 420));
}

/// `0` when nothing was sent — and when the provider does not report its
/// cache, which reads the same: no evidence of a hit.
pub(super) fn cached_percent(cached: u64, total: u64) -> u64 {
    (cached * 100).checked_div(total).unwrap_or(0)
}

#[test]
#[ignore = "runs a real model on every task; see the module docs"]
fn agent_bench() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let session = session();
    let runs: usize = var("AGENT_BENCH_RUNS").map_or(1, |n| n.parse().expect("AGENT_BENCH_RUNS is a number"));
    let limit = Duration::from_secs(var("AGENT_BENCH_TIMEOUT_SECS").map_or(600, |n| n.parse().expect("a number of seconds")));
    let tasks = tasks(&manifest.join("bench/agent-tasks"));
    // One model for every run: loading it is the expensive part.
    let model: Arc<dyn EmbeddingProvider> = Arc::new(LocalEmbeddings::new(bundled_model_dir(None), DEFAULT_IDLE_UNLOAD));
    assert!(!tasks.is_empty(), "no task matches AGENT_BENCH_TASKS");

    let out_dir = manifest.join("target/agent-bench");
    std::fs::create_dir_all(&out_dir).unwrap();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let out_path = out_dir.join(format!("{stamp}.jsonl"));
    let mut out = String::new();

    println!("\n=== {} · {} task(s) × {runs}", session.model, tasks.len());
    let mut all: Vec<Run> = Vec::new();
    for task in &tasks {
        for n in 0..runs {
            let run = run_task(&session, &model, task, n, limit);
            println!(
                "{:<22} #{n} {:<4} {:<9} rounds {:>2}  calls {:>2}  errors {:>2}  repeats {:>2}  cleared {:>2}{}  tokens {:>7} ({:>3}% cached)/{:<6} {:>5.0}s",
                run.task,
                if run.passed { "PASS" } else { "FAIL" },
                run.ended,
                run.rounds,
                run.calls.len(),
                run.errors().count(),
                run.repeats,
                run.cleared,
                if run.loop_reminded { "  loop" } else { "" },
                run.tokens_in,
                cached_percent(run.tokens_cached, run.tokens_in),
                run.tokens_out,
                run.seconds,
            );
            if !run.passed {
                println!("    kept: {}", run.workspace.display());
                for line in run.check_output.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev() {
                    println!("    | {line}");
                }
            } else {
                // Best effort: a copy left behind costs disk, not correctness.
                let _ = std::fs::remove_dir_all(&run.workspace);
                let _ = std::fs::remove_dir_all(index_dir(&run.workspace));
            }
            out.push_str(&run.json().to_string());
            out.push('\n');
            // Written after every run, so an interrupted bench keeps what it paid for.
            std::fs::write(&out_path, &out).unwrap();
            all.push(run);
        }
    }

    let passed = all.iter().filter(|r| r.passed).count();
    let mean = |f: &dyn Fn(&Run) -> f64| all.iter().map(f).sum::<f64>() / all.len() as f64;
    println!(
        "\nPASS {passed}/{}  ·  mean rounds {:.1}, calls {:.1}, tool errors {:.1}, repeats {:.1}, loop reminders {}, tokens in {:.0} ({}% cached), {:.0}s",
        all.len(),
        mean(&|r| r.rounds as f64),
        mean(&|r| r.calls.len() as f64),
        mean(&|r| r.errors().count() as f64),
        mean(&|r| r.repeats as f64),
        all.iter().filter(|r| r.loop_reminded).count(),
        mean(&|r| r.tokens_in as f64),
        cached_percent(all.iter().map(|r| r.tokens_cached).sum(), all.iter().map(|r| r.tokens_in).sum()),
        mean(&|r| r.seconds),
    );
    let mut by_tool: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for call in all.iter().flat_map(|r| &r.calls) {
        let entry = by_tool.entry(call.tool.as_str()).or_default();
        entry.0 += 1;
        entry.1 += usize::from(call.status == CallStatus::Error);
    }
    println!("calls by tool (errors):");
    for (tool, (calls, errors)) in by_tool {
        println!("  {tool:<18} {calls:>4} ({errors})");
    }
    println!("runs: {}", out_path.display());
}
