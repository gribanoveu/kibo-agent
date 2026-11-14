//! Helper agents a call starts: explore, a fork, a review.

use super::*;

/// `remember`'s check: `text` alone under `prompt`, to
/// the turn's model with no tools and no history, as a chat's name is asked
/// for (`services::chat_title`). The verdict is SAVE or REFUSE either way.
pub(super) fn check_with(session: &LlmSession, prompt: &str, text: &str) -> Result<(), String> {
    use crate::domain::project_rules::memory_verdict;
    let request = ChatRequest {
        messages: vec![LlmMessage::system(prompt), LlmMessage::user(text)],
        tools: Vec::new(),
        model: session.model.clone(),
    };
    llm_debug_log::log_request(session.debug_logging, &session.provider_id, 0, &request);
    let response = session.provider.chat(request);
    llm_debug_log::log_response(session.debug_logging, &session.provider_id, 0, &response);
    let reply = response.map_err(|e| format!("the check could not run ({e})"))?;
    memory_verdict(&reply.content.unwrap_or_default())
}

/// A helper: `brief` as a turn of its own in `mode`, from an empty history —
/// `explore`'s research, `/review`'s review.
///
/// It shares what the calling turn has — the model, the folder, the stop
/// button, the hooks, the log — and nothing of its conversation: no history,
/// no plan, no checklist. Of its events only the calls go out, as lines of
/// the calling card's output, and its usage goes to its record: its text is
/// the result, and its tokens are not this chat's context.
///
/// Recorded in the Agents tab from start to end, and stopped from there too;
/// the caller says how it ended (`Agents::finish`). The chat's own Stop ends
/// both.
///
/// `fork`: the conversation so far comes before `brief`, and the helper is
/// the calling turn's place, MCP tools and features — what its prompt and
/// tool list are made of — so its requests start as the calling turn's last
/// one did.
pub(super) fn run_helper(
    turn: &Turn,
    brief: &str,
    fork: Option<&[LlmMessage]>,
    mode: ConversationMode,
    review: Option<Arc<crate::domain::review::ReviewDesk>>,
    approval: &ApprovalPolicy,
    progress: &CommandSink,
) -> Result<(Arc<crate::domain::agents::Agents>, u32, Result<ChatStreamOutcome, TurnError>), crate::domain::tools::ToolError> {
    // Offered only in a folder; the chat's preflight refuses it first.
    let Some(scope) = turn.place.scope() else {
        let tool = if mode == ConversationMode::Review { "review" } else { "explore" };
        return Err(crate::domain::tools::ToolError::NoFolder(tool.to_string()));
    };
    // A throwaway record where the app keeps none, so a run is counted the
    // same way either way.
    let agents = turn.agents.clone().unwrap_or_default();
    let id = agents.start(brief);
    let events: ChatEventSink = {
        let agents = Arc::clone(&agents);
        let progress = progress.clone();
        Arc::new(move |event: ChatTurnEvent| match event.event {
            ChatEventPayload::ToolCall(call) => {
                let step = explore_step(&call.name, &call.arguments);
                progress(CommandEvent {
                    stream: crate::domain::command_exec::OutputStream::Stdout,
                    chunk: format!("{step}\n"),
                });
                agents.step(id, step);
            }
            // The budget note is the card's own line, not a notice in the
            // chat: it belongs where the review's steps are.
            ChatEventPayload::WrapUpReminded { text, .. } => {
                progress(CommandEvent {
                    stream: crate::domain::command_exec::OutputStream::Stdout,
                    chunk: format!("{text}\n"),
                });
            }
            ChatEventPayload::ContextUsage(usage) => agents.spent(id, &usage),
            _ => {}
        })
    };
    let stopped = || (turn.cancelled)() || agents.stop_asked(id);
    let no_steering = Vec::new;
    let no_parallel = Vec::new;
    let parallel = match turn.place {
        Place::Folder { parallel, .. } => parallel,
        Place::Chat { .. } => &no_parallel,
    };
    let (mode, web, mcp, features, messages) = match fork {
        // A fork does not search the web either: offered for the cache, refused when called.
        Some(history) => {
            let web = match turn.place {
                Place::Folder { web, .. } => web,
                Place::Chat { .. } => None,
            };
            let mut messages = history.to_vec();
            messages.push(LlmMessage::user(prompt::explore_fork_brief(brief)));
            (turn.place.mode().unwrap_or(mode), web, turn.mcp, turn.features.clone(), messages)
        }
        // A helper reads; no feature of the user's is for it. Several run side
        // by side, and each searching the web would multiply the queries — and
        // Tavily's credits.
        None => (mode, None, &McpTools::default(), Features::default(), vec![LlmMessage::user(brief)]),
    };
    let helper = Turn {
        events: &events,
        session: turn.session,
        place: Place::Folder { scope, mode, web, parallel, worktree_base: None },
        approval,
        cancelled: &stopped,
        sleep: turn.sleep,
        shell: turn.shell,
        shell_described: turn.shell_described,
        take_steering: &no_steering,
        notes_typed: &|| 0,
        search: turn.search.clone(),
        skills: turn.skills,
        rules: turn.rules,
        // A fork's prompt is the calling turn's, for the cache; a helper's
        // own starts fresh and has nothing to be told changed.
        frozen: turn.frozen.filter(|_| fork.is_some()),
        log_call: turn.log_call,
        plan: None,
        worktree_of: turn.worktree_of,
        // A helper's base is its brief's, not its own: a review's marker is
        // the calling turn's fact.
        chat_head: None,
        mcp,
        hooks: turn.hooks,
        // The calling turn's to report when they end.
        processes: None,
        terminals: turn.terminals.clone(),
        review,
        agents: None,
        questions: None,
        board: None,
        // A fork reads the databases the calling turn may; a helper has no feature on, and is not offered it.
        mongo: turn.mongo.clone(),
        deepseek: None,
        features,
        fork: fork.is_some(),
    };
    let outcome = stream(&helper, messages, Vec::new());
    Ok((agents, id, outcome))
}

/// `explore`: `task` as a helper in `ConversationMode::Explore`, returning
/// its closing answer. A helper the user stops is an error the calling turn
/// reads and carries on from.
///
/// Nothing it is offered asks for approval, so it never pauses; if it did,
/// there would be no card to answer, and it ends as an error instead.
///
/// A fork starts from `history` as it was before the reply that called it:
/// what the calling turn's last request sent, so the provider has it cached,
/// and no call of that reply left without its result.
pub(super) fn run_explore(
    turn: &Turn,
    args: &crate::domain::tools::ExploreArgs,
    history: &[LlmMessage],
    progress: &CommandSink,
) -> Result<ToolResult, crate::domain::tools::ToolError> {
    use crate::domain::agents::AgentState;
    use crate::domain::tools::ToolError;
    let task = args.task.as_str();
    let before_reply = history.iter().rposition(|m| m.role == LlmRole::Assistant && !m.tool_calls.is_empty()).unwrap_or(history.len());
    let fork = args.fork.then(|| &history[..before_reply]);
    if let Some(start) = fork {
        fork_can_start(turn, start, &history[before_reply..])?;
    }
    let (agents, id, outcome) = run_helper(turn, task, fork, ConversationMode::Explore, None, &ApprovalPolicy::default(), progress)?;
    let failed = |reason: String| (AgentState::Failed { reason: reason.clone() }, Err(reason));
    let (state, answer) = match outcome {
        Ok(ChatStreamOutcome::Done(ChatDone { limit_reached: Some(rounds), .. })) => failed(format!(
            "the helper used up its {rounds} rounds without answering. Give it a narrower task, or look yourself."
        )),
        Ok(ChatStreamOutcome::Done(done)) if !done.result.text.trim().is_empty() => (AgentState::Done, Ok(done.result.text)),
        Ok(ChatStreamOutcome::Done(_)) => failed("the helper finished without an answer".to_string()),
        Ok(ChatStreamOutcome::Cancelled(_)) if (turn.cancelled)() => {
            (AgentState::Stopped, Err("stopped before it answered".to_string()))
        }
        Ok(ChatStreamOutcome::Cancelled(_)) => (
            AgentState::Stopped,
            Err("the user stopped the helper before it answered. Carry on without it; do not start it again for the same question unless the user asks".to_string()),
        ),
        Ok(ChatStreamOutcome::PendingApproval(_)) => failed("the helper asked for a call that needs approval".to_string()),
        Err(e) => failed(e.to_string()),
    };
    agents.finish(id, state, answer.as_ref().ok().cloned());
    let run = agents.get(id);
    let tokens = run.as_ref().map(|run| run.tokens).unwrap_or_default();
    let read = run.map(|run| helper_reads(&run.steps)).unwrap_or_default();
    let lines_of = |path: &str| {
        let scope = turn.place.scope()?;
        // Bytes, not text: a file in Latin-1 has its lines all the same.
        let bytes = std::fs::read(resolve_existing(scope, path).ok()?).ok()?;
        Some(bytes.iter().filter(|&&b| b == b'\n').count() + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n")))
    };
    answer
        .map(|text| ToolResult::Explored { text: explore_answer::checked(&text, &read, lines_of), agent: id, tokens })
        .map_err(ToolError::Explore)
}

/// Whether a fork may start from `start`, the conversation before the reply
/// that asked for it; `reply` is that reply and what of it has run.
///
/// Not after a change in the same reply: the fork would read the folder as
/// its conversation last saw it. And not from a conversation already half
/// way to where old results are cleared: the fork's own reads would push its
/// requests past it, and the clearing would cut into the conversation it was
/// started for — and into the cached start that made it cheap.
/// ponytail: half the trigger is a guess; a fork that reads more than the
/// rest still clears into its start. A floor under `result_clearing::plan`
/// is the way past.
pub(super) fn fork_can_start(turn: &Turn, start: &[LlmMessage], reply: &[LlmMessage]) -> Result<(), crate::domain::tools::ToolError> {
    use crate::domain::tools::ToolError;
    let ran = |id: &str| reply.iter().any(|m| m.tool_call_id.as_deref() == Some(id));
    let changed: Vec<&str> = reply
        .first()
        .map(|asked| asked.tool_calls.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|call| ran(&call.id) && ToolName::from_wire_name(&call.name).is_some_and(ToolName::is_mutating))
        .map(|call| call.name.as_str())
        .collect();
    if !changed.is_empty() {
        return Err(ToolError::Explore(format!(
            "a fork starts from before this reply, and `{}` in it has already run, so it would read the folder as it was. Give the helper a brief instead (no fork), or fork in a reply of its own",
            changed.join("`, `")
        )));
    }
    let usage = request_usage(turn, start);
    let trigger = usage
        .limit
        .map(|limit| u64::from(limit) * result_clearing::TRIGGER_PERCENT / 100)
        .unwrap_or(result_clearing::FALLBACK_TRIGGER_TOKENS as u64);
    if usage.total as u64 * 2 >= trigger {
        return Err(ToolError::Explore(
            "this conversation is too long to fork: the helper's reading would clear what it was forked for. Give it a brief instead (no fork)".to_string(),
        ));
    }
    Ok(())
}

/// The files a helper opened, once each, from its steps (`explore_step`).
pub(super) fn helper_reads(steps: &[String]) -> Vec<String> {
    let mut read: Vec<String> = Vec::new();
    for path in steps.iter().filter_map(|step| step.strip_prefix("readFile ")).flat_map(|paths| paths.split(", ")) {
        if !read.iter().any(|seen| seen == path) {
            read.push(path.to_string());
        }
    }
    read
}

/// The review helper: Review mode over `desk`, under the chat's approval —
/// what would ask is refused (`preflight`), since a helper has no card.
///
/// Whatever becomes of the run, its findings are the result: the round
/// ceiling, a stop, a call that would have asked, a provider failure — each
/// ends with what `reportFinding` had kept, marked with why the review did
/// not finish (`docs/49-review-as-a-tool.md`, R-6). Nothing a review found
/// is thrown away with the run.
pub(super) fn run_review(
    turn: &Turn,
    brief: &str,
    desk: Arc<crate::domain::review::ReviewDesk>,
    range: crate::domain::review::ReviewRange,
    progress: &CommandSink,
) -> Result<ToolResult, crate::domain::tools::ToolError> {
    use crate::domain::agents::AgentState;
    enum Close {
        Clean,
        Rounds,
        Stopped,
        Failed(String),
    }
    let (agents, id, outcome) = run_helper(turn, brief, None, ConversationMode::Review, Some(Arc::clone(&desk)), turn.approval, progress)?;
    let (state, answer, close) = match outcome {
        Ok(ChatStreamOutcome::Done(done)) if done.limit_reached.is_some() => (AgentState::Done, Some(done.result.text), Close::Rounds),
        Ok(ChatStreamOutcome::Done(done)) => (AgentState::Done, Some(done.result.text), Close::Clean),
        Ok(ChatStreamOutcome::Cancelled(_)) => (AgentState::Stopped, None, Close::Stopped),
        Ok(ChatStreamOutcome::PendingApproval(_)) => {
            let reason = "it asked for a call that needs approval, and a review runs on its own with nobody to ask".to_string();
            (AgentState::Failed { reason: reason.clone() }, None, Close::Failed(reason))
        }
        Err(e) => (AgentState::Failed { reason: e.to_string() }, None, Close::Failed(e.to_string())),
    };
    agents.finish(id, state, answer.as_deref().map(str::trim).filter(|text| !text.is_empty()).map(str::to_string));
    let tokens = agents.get(id).map(|run| run.tokens).unwrap_or_default();
    // The card's last line says what was read and what came of it, a cut
    // short included — the reader should not have to open the result to know.
    let findings = desk.findings();
    let verdict = match findings.len() {
        0 => "no findings".to_string(),
        1 => "1 finding".to_string(),
        n => format!("{n} findings"),
    };
    let mut line = format!("{} — {verdict}", range.description());
    let note = |what: &str| format!("(The review {what}.)");
    let text = match close {
        Close::Clean => answer.unwrap_or_default().trim().to_string(),
        Close::Rounds => {
            line.push_str(" — ran out of rounds before it finished");
            format!("{}\n\n{}", answer.unwrap_or_default().trim(), note("ran out of rounds before it finished"))
        }
        Close::Stopped => {
            line.push_str(" — stopped before it finished");
            note("was stopped before it finished")
        }
        Close::Failed(reason) => {
            line.push_str(&format!(" — failed: {reason}"));
            note(&format!("failed: {reason}"))
        }
    };
    progress(CommandEvent { stream: crate::domain::command_exec::OutputStream::Stdout, chunk: format!("{line}\n") });
    Ok(ToolResult::Reviewed { findings, text: text.trim().to_string(), range: range.description(), agent: id, tokens })
}

/// `review` (the tool): the range `from`..`to` — planned against this turn's
/// folder and its chat's start marker — read by a helper in Review mode.
/// The helper's findings are the call's result; the calling turn goes on
/// with them in the same round, nothing handed sideways.
pub(super) fn run_review_tool(
    turn: &Turn,
    args: &crate::domain::tools::ReviewArgs,
    progress: &CommandSink,
) -> Result<ToolResult, crate::domain::tools::ToolError> {
    // The turn's folder, never the app's global one: a chat working in a
    // worktree reviews the worktree (`docs/49-review-as-a-tool.md`).
    let scope = turn.place.scope().ok_or_else(|| crate::domain::tools::ToolError::NoFolder("review".to_string()))?;
    let facts = crate::infra::git_changes::review_facts(scope.root(), args.to.as_deref(), turn.chat_head)
        .map_err(|e| crate::domain::tools::ToolError::Git(e.to_string()))?;
    let planned = crate::domain::review::plan(args.from.as_deref(), args.to.as_deref(), &facts)
        .map_err(|e| crate::domain::tools::ToolError::Command(e.to_string()))?;
    let files = crate::infra::git_changes::review_diffs(scope.root(), planned.from.as_deref(), planned.to.as_deref())
        .map_err(|e| crate::domain::tools::ToolError::Git(e.to_string()))?;
    let (kept, excluded) = crate::domain::review::select(files);
    if kept.is_empty() {
        // An empty answer to a range the caller named is a different refusal
        // from "the tree is clean": the ends met, and only the caller can
        // widen them.
        let why = if args.from.is_some() || args.to.is_some() {
            crate::domain::review::empty_range()
        } else {
            crate::domain::review::nothing_to_review(excluded.len())
        };
        return Err(crate::domain::tools::ToolError::Command(why));
    }
    let range = crate::domain::review::ReviewRange {
        root: scope.root().display().to_string(),
        from: planned.from.clone().unwrap_or_else(|| "HEAD".to_string()),
        to: planned.to.clone().unwrap_or_else(|| "working tree".to_string()),
        files: kept.len(),
        excluded,
    };
    let brief = crate::domain::review::review_message(&kept, &range, crate::services::llm_session::reply_language(), args.focus.as_deref());
    let desk = Arc::new(crate::domain::review::ReviewDesk::new(kept));
    run_review(turn, &brief, desk, range, progress)
}

/// One line of a helper's progress: the tool, and what it was pointed at.
/// A review's helper runs commands too (`cargo test`, `git diff`), and a step
/// with no command named is the one that says the least.
pub(super) fn explore_step(name: &str, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let paths = crate::domain::tools::read_paths(&args).join(", ");
    let target = (!paths.is_empty()).then_some(paths.as_str()).or_else(|| {
        ["pattern", "query", "command"].iter().find_map(|key| args.get(key).and_then(|v| v.as_str()))
    });
    match target {
        Some(target) => format!("{name} {target}"),
        None => name.to_string(),
    }
}
