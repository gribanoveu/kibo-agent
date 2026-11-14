//! Running a round's calls: the preflight, approval, side-by-side reads, the log.

use super::*;

/// What the model reads instead of a file it already has verbatim, earlier in
/// this same turn.
pub(super) const REPEAT_READ_NOTE: &str = "This file was already read in this turn, over the same range, and has not changed since — the result is earlier in the conversation and is not repeated here.";

/// The same, for a search that ranked identically.
pub(super) const REPEAT_SEARCH_NOTE: &str = "This search already ran in this turn with the same parameters and returned the same result — it is earlier in the conversation and is not repeated here.";

/// Appended to a tool error from a round the provider cut off.
pub(super) const TRUNCATED_ROUND_NOTE: &str = "Note: the model's reply was cut off mid-way — the response length limit (max_tokens) ran out; the arguments were not malformed. Retry the call more compactly: shorter arguments, fewer files or lines at a time, splitting the work across several calls if needed.";

/// The cost of one round: [`ToolName::loop_weight`] over every call it
/// contains, since a round can bundle several.
///
/// A name that is not a tool costs `1` — the floor of the cheapest real tool —
/// so a hallucinated tool name still moves the budget forward instead of
/// letting the loop spin for free. An MCP call costs its server's `weight`.
pub fn round_cost(calls: &[LlmToolCall], mcp: &McpTools) -> u32 {
    calls
        .iter()
        .map(|call| match ToolName::from_wire_name(&call.name) {
            Some(ToolName::Mcp) => mcp.weight(&call.name),
            Some(tool) => tool.loop_weight(),
            None => 1,
        })
        .sum()
}

/// Replaces the body of a result the model has already been given, byte for
/// byte, earlier in this turn.
///
/// Not a correctness fix — re-reading is legitimate, and after a write it is
/// required. It is a context fix: the same file read three times is re-sent in
/// full on every subsequent round of the turn, and so is a search that keeps
/// returning the same hits.
///
/// Keyed on the tool name plus its raw arguments, so a different line range or
/// a different query is a different call, and gated on a hash of the payload,
/// so an edited file — or a search that now ranks differently — comes back in
/// full.
///
/// `result` is `None` for a failed call: an error is short and worth repeating
/// as often as it happens.
pub fn dedupe_repeat_result(
    seen: &mut HashMap<String, u64>,
    call: &LlmToolCall,
    result: Option<&ToolResult>,
    content: String,
) -> String {
    let note = match result {
        Some(ToolResult::File { .. } | ToolResult::Files { .. }) => REPEAT_READ_NOTE,
        Some(ToolResult::GrepResults { .. }) => REPEAT_SEARCH_NOTE,
        _ => return content,
    };
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    match seen.insert(format!("{}|{}", call.name, call.arguments), hash) {
        Some(previous) if previous == hash => note.to_string(),
        _ => content,
    }
}

/// Adds [`TRUNCATED_ROUND_NOTE`] to a failed call from a truncated round.
///
/// `finish_reason: "length"` means generation stopped because the response
/// budget ran out — mid-sentence, and just as easily mid-`arguments`. The
/// model is never told that budget exists, so the severed JSON comes back to
/// it as a bare "invalid arguments" error, which invites the obvious wrong
/// repair: send the same oversized call again.
///
/// Only to a **failed** call: one whose arguments happened to close before the
/// cut still executed correctly, and telling the model its successful result
/// was somehow damaged is worse than saying nothing.
///
/// Takes a plain `failed` flag rather than the outcome, because the two places
/// a tool error becomes content for the model do not share a type — the
/// preflight rejects a call before there is any result to inspect, and severed
/// arguments fail exactly there.
pub fn truncated_round_note(round_truncated: bool, failed: bool, content: String) -> String {
    if round_truncated && failed {
        format!("{content}\n\n{TRUNCATED_ROUND_NOTE}")
    } else {
        content
    }
}

/// Runs a round's calls in order and answers each in the history — `decisions`
/// the user's, for a resumed round. A stop between two calls leaves the rest
/// unrun; the loop's first checkpoint then ends the turn.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_calls(
    turn: &Turn,
    events: &Events,
    round: u32,
    state: &mut State,
    pass: &mut Pass,
    calls: &[LlmToolCall],
    decisions: &[ToolCallDecision],
    round_truncated: bool,
) {
    // Reads asked one after another run side by side when the round
    // reaches the first of them; the loop then takes their results in turn,
    // so the history, the log and the loop guard see one call after another
    // as ever.
    let mut early: HashMap<String, Early> = HashMap::new();

    for (at, call) in calls.iter().enumerate() {
        // The "stopped between two calls of one round" case. Back to the
        // loop rather than ending the turn here: checkpoint one builds the
        // outcome, so there is one place that decides what a cancelled turn
        // looks like.
        if (turn.cancelled)() {
            return;
        }
        if !early.contains_key(&call.id) {
            early.extend(side_by_side(turn, &events, round, &calls[at..], decisions, &state.history));
        }
        let ran = early.remove(&call.id);
        if ran.is_none() {
            report_call(&events, round, call);
        }

        let decision = decisions.iter().find(|d| d.id == call.id);
        let started = ran.as_ref().map_or_else(Instant::now, |ran| ran.started);
        // What the log may keep, gathered on the way: the arguments once
        // they have parsed, the error before it is flattened to text.
        let mut logged_args = serde_json::Value::Null;
        let mut logged_error = None;
        let mut error_kind = None;
        let mut changes = Vec::new();
        let denied = matches!(decision, Some(d) if !d.approved);
        let outcome = match decision {
            Some(d) if !d.approved => Err(denial(d)),
            _ => parse_tool_call(call)
                .and_then(|parsed| {
                    logged_args = tool_call_log::redact_args(&parsed);
                    // Its hook already asked, and it has already run; what
                    // it read joins the round's in the order it was asked.
                    if let Some(ran) = ran {
                        state.reads.merge(ran.reads);
                        return ran.result;
                    }
                    if let Some(reason) = pre_tool_use(turn, &events, round, call) {
                        return Err(crate::domain::tools::ToolError::BlockedByHook(reason));
                    }
                    // Around the call, not inside the tool: one place sees
                    // every write, and no tool has to remember to report.
                    let watched = turn.place.scope().and_then(|scope| file_changes::before(scope, &parsed));
                    let plan = state.written_plan.as_deref().or(turn.plan);
                    let result = execute_call(turn, round, call, &parsed, &state.history, plan, &mut state.reads, &mut state.todos);
                    if let (Ok(_), crate::domain::tools::ToolCall::WritePlan(written)) = (&result, &parsed) {
                        state.written_plan = Some(written.content.trim().to_string());
                    }
                    changes = watched.map(file_changes::after).unwrap_or_default();
                    result
                })
                .map_err(|e| {
                    logged_error = Some(tool_call_log::redact_error(&e));
                    error_kind = Some(loop_guard::error_kind(&e));
                    format!("{TOOL_ERROR_PREFIX}{e}")
                }),
        };
        // What a hook says about a call that ran goes to the model with
        // its result: the call happened, and cannot be refused any more.
        let post_hook = outcome.as_ref().ok().and_then(|result| {
            let fields = serde_json::json!({
                "tool_name": call.name,
                "tool_input": serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default(),
                "tool_response": result,
                "tool_use_id": call.id,
            });
            fire_hook(turn, &events, round, HookEvent::PostToolUse, Some(&call.name), fields)
        });
        let status = match &outcome {
            Ok(_) => CallStatus::Ok,
            Err(_) if denied => CallStatus::Denied,
            Err(_) => CallStatus::Error,
        };
        // A denial's text is the user's own reason, not the tool's.
        let error = if denied { outcome.as_ref().err().cloned() } else { logged_error };
        let result = outcome.as_ref().ok().map(tool_call_log::redact_result);
        log_call(turn, round, call, logged_args, status, error, result, started);

        report_result(
            &events,
            round,
            &call.id,
            outcome.as_ref().ok(),
            outcome.as_ref().err().map(String::as_str),
            changes,
        );

        // What the model reads, as opposed to what the UI was given:
        // plain text in the shape each answer usually takes — see
        // `model_text`.
        let content = match &outcome {
            Ok(result) => for_model(result),
            // The prompt says nothing about approval: what to do with a
            // refusal arrives with the refusal.
            Err(message) if denied => format!("{message}\n{AFTER_DENIAL}"),
            Err(message) => message.clone(),
        };
        // What came back, before any note is added to it. A denial is the
        // user's answer, not the model going round.
        if !denied {
            pass.settled.push(Settled {
                tool: call.name.clone(),
                arguments: call.arguments.clone(),
                content: content.clone(),
                error: error_kind,
            });
        }
        let content = truncated_round_note(round_truncated, outcome.is_err(), content);
        let content =
            dedupe_repeat_result(&mut pass.seen_results, call, outcome.as_ref().ok(), content);
        let content = match post_hook {
            Some(said) => format!("{content}\n\n[A PostToolUse hook said:]\n{said}"),
            None => content,
        };
        state.history.push(tool_message(&call.id, content));
    }
}

/// Asks the PreToolUse hooks about `call`; the reason, when one refused it.
pub(super) fn pre_tool_use(turn: &Turn, events: &Events, round: u32, call: &LlmToolCall) -> Option<String> {
    let fields = serde_json::json!({
        "tool_name": call.name,
        "tool_input": serde_json::from_str::<serde_json::Value>(&call.arguments).unwrap_or_default(),
        "tool_use_id": call.id,
    });
    fire_hook(turn, events, round, HookEvent::PreToolUse, Some(&call.name), fields)
}

/// Runs one parsed call with what the turn has.
#[allow(clippy::too_many_arguments)]
pub(super) fn execute_call(
    turn: &Turn,
    round: u32,
    call: &LlmToolCall,
    parsed: &crate::domain::tools::ToolCall,
    history: &[LlmMessage],
    plan: Option<&str>,
    reads: &mut ReadFiles,
    todos: &mut Vec<Task>,
) -> Result<ToolResult, crate::domain::tools::ToolError> {
    let tree_change = match parsed {
        crate::domain::tools::ToolCall::RunCommand(request) => {
            crate::domain::command_risk::git_tree_change(&request.command, &turn.approval.git_aliases)
        }
        _ => None,
    };
    // Approval may have paused for long enough for a neighbour to start. Check
    // again at the point of effect; for a tree mutation, hold the same lock as
    // turn admission so a neighbour cannot appear between this check and exec.
    let _tree = tree_change.as_ref().map(|_| crate::domain::command_risk::git_tree_lock());
    preflight(turn, reads, call)?;

    // Built per call, because the id is what pairs a line of output with the
    // call that produced it — a round may have started more than one.
    let output = command_output_sink(turn.events, round, &call.id);
    let explore = |args: &crate::domain::tools::ExploreArgs| run_explore(turn, args, history, &output);
    let review_run = |args: &crate::domain::tools::ReviewArgs| run_review_tool(turn, args, &output);
    let check = |prompt: &str, text: &str| check_with(turn.session, prompt, text);
    let ask = |server: &str, question: &crate::domain::mcp::McpQuestion| ask_user(turn, round, &call.id, server, question);
    let deps = ToolDeps {
        shell: turn.shell.clone(),
        output: Some(output.clone()),
        search: turn.search.clone().map(|search| crate::services::search_rerank::reranking(search, Arc::new(turn.session.clone()))),
        skills: turn.skills.to_vec(),
        mcp: turn.mcp.clone(),
        cancelled: Some(turn.cancelled),
        notes_typed: Some(turn.notes_typed),
        ask: turn.questions.is_some().then_some(&ask as _),
        processes: turn.processes.clone(),
        tree_mutation: tree_change.is_some(),
        terminals: turn.terminals.clone(),
        review: turn.review.clone(),
        explore: Some(&explore),
        review_run: Some(&review_run),
        check: Some(&check),
        kube: pinned_cluster(turn),
        runbooks: match turn.place {
            Place::Chat { runbooks, .. } => runbooks.to_vec(),
            Place::Folder { .. } => Vec::new(),
        },
        web: match turn.place {
            Place::Chat { web, .. } | Place::Folder { web, .. } => web.cloned(),
        },
        deepseek: turn.deepseek.clone(),
        history,
        board: turn.board.clone(),
        mongo: turn.mongo.clone(),
        parallel: match turn.place {
            Place::Folder { parallel, .. } => Some(parallel),
            Place::Chat { .. } => None,
        },
        worktree_base: match turn.place {
            Place::Folder { worktree_base, .. } => worktree_base,
            Place::Chat { .. } => None,
        },
        chat_head: turn.chat_head,
        waiting: match turn.place {
            Place::Folder { mode, web, .. } => waiting_tools(mode, web.is_some(), &turn.features),
            Place::Chat { .. } => Vec::new(),
        },
        has_plan: plan.is_some_and(|plan| !plan.trim().is_empty()),
    };
    dispatch(turn.place.scope(), parsed, reads, todos, &deps)
}

/// The half of the gate the model cannot see: in a folder, the mode and
/// containment; in a chat, the role's own tools and nothing else.
pub(super) fn preflight(turn: &Turn, reads: &ReadFiles, call: &LlmToolCall) -> Result<(), crate::domain::tools::ToolError> {
    // Before the place's own gate: a call from memory of a turn when the
    // feature was on is told where to turn it on, not that the tool is unknown.
    if let Some(tool) = ToolName::from_wire_name(&call.name).filter(|&tool| !turn.features.offers(tool)) {
        let owner = crate::domain::features::Feature::owning(tool);
        let (tool, feature) = (call.name.clone(), owner.map(|f| f.manifest().name).unwrap_or_default().to_string());
        return Err(match owner.is_some_and(|f| turn.features.is_on(f)) {
            true => crate::domain::tools::ToolError::ToolsKept { tool, feature },
            false => crate::domain::tools::ToolError::FeatureOff { tool, feature },
        });
    }
    match turn.place {
        Place::Folder { scope, mode, parallel, .. } => {
            if call.name == ToolName::RunCommand.wire_name() && !parallel().is_empty() {
                if let Ok(crate::domain::tools::ToolCall::RunCommand(request)) = parse_tool_call(call) {
                    if let Some(subcommand) = crate::domain::command_risk::git_tree_change(&request.command, &turn.approval.git_aliases) {
                        return Err(crate::domain::tools::ToolError::GitTreeDuringParallel(subcommand));
                    }
                }
            }
            // A fork is offered its parent's tools for the parent's cache,
            // and runs only what an `explore` helper would: the folder, not
            // the web — several run side by side, and the credits are the user's.
            // Before the mode's own gate, so a write is told it is not for a
            // fork, not that it needs a read first.
            let reads_only = |tool: ToolName| {
                conversation_mode::offers(ConversationMode::Explore, tool)
                    && !tool.is_web()
                    && !matches!(tool, ToolName::DeepseekSearch | ToolName::DeepseekFetch)
            };
            if turn.fork && !ToolName::from_wire_name(&call.name).is_some_and(reads_only) {
                return Err(crate::domain::tools::ToolError::Command(format!(
                    "`{}` is not for a fork, which only searches and reads. Answer from what you can read",
                    call.name
                )));
            }
            preflight_tool_call(scope, mode, reads, call)?;
            // A review runs as a helper, with nobody to answer a card: what
            // would ask is refused rather than paused on.
            if mode == ConversationMode::Review && needs_approval(turn, call).0 {
                return Err(crate::domain::tools::ToolError::Command(
                    "this needs the user's approval, and a review runs on its own with nobody to ask. Leave it, and say in your answer what it would have settled".to_string(),
                ));
            }
            Ok(())
        }
        Place::Chat { role, scope, .. } => {
            let parsed = preflight_chat_call(role, call)?;
            if let Some(scope) = scope {
                preflight_paths(scope, reads, &parsed)?;
            }
            // A change the cluster would refuse — read-only, RBAC, a
            // webhook — is refused here, before a card asks about it.
            crate::services::ai_tools::tools::cluster::preflight(pinned_cluster(turn), &parsed)
        }
    }
}

/// How many calls run at once. More helpers would be as many streams to one
/// provider, and a model that asks for ten reads gets them four at a time.
// ponytail: fixed batches, not a pool — a slow call (a helper, a page) holds
// its batch back; a pool if rounds with more than four turn out to be common.
pub const MAX_PARALLEL_CALLS: usize = 4;

/// A call already run, for the round's loop to take in its turn.
pub(super) struct Early {
    result: Result<ToolResult, crate::domain::tools::ToolError>,
    started: Instant,
    /// What it read, for the round to gather when it takes the result.
    reads: ReadFiles,
}

/// The reads `calls` starts with — tools that [`ToolName::runs_alongside`] —
/// run side by side when there are two or more in a row.
///
/// In a row, and from where the round has got to: a call asked before them
/// has run, and one asked after them has not — "write the file, then have a
/// helper check it" must find the file written. Each is shown as started and
/// asked of the PreToolUse hooks here, in order, before any runs; a refused
/// one is kept as refused. They never write, and each reports to its own card
/// — its call id — and its own record of what it read; the history they see
/// is the round's as it stood before them. Anything else, and a lone read, is
/// left to the loop.
pub(super) fn side_by_side(
    turn: &Turn,
    events: &Events,
    round: u32,
    calls: &[LlmToolCall],
    decisions: &[ToolCallDecision],
    history: &[LlmMessage],
) -> HashMap<String, Early> {
    let mut early = HashMap::new();
    let reads: Vec<(&LlmToolCall, crate::domain::tools::ToolCall)> = calls
        .iter()
        .map_while(|call| match parse_tool_call(call) {
            _ if decisions.iter().any(|d| d.id == call.id && !d.approved) => None,
            Ok(parsed) if parsed.name().runs_alongside() => Some((call, parsed)),
            _ => None,
        })
        .collect();
    if reads.len() < 2 || (turn.cancelled)() {
        return early;
    }
    let mut runnable = Vec::new();
    for (call, parsed) in reads {
        report_call(events, round, call);
        let started = Instant::now();
        match pre_tool_use(turn, events, round, call) {
            Some(reason) => {
                let result = Err(crate::domain::tools::ToolError::BlockedByHook(reason));
                early.insert(call.id.clone(), Early { result, started, reads: ReadFiles::default() });
            }
            None => runnable.push((call, parsed, started)),
        }
    }
    for batch in runnable.chunks(MAX_PARALLEL_CALLS) {
        let done: Vec<(String, Early)> = std::thread::scope(|scope| {
            let running: Vec<_> = batch
                .iter()
                .map(|(call, parsed, started)| {
                    scope.spawn(move || {
                        // The todo list is not touched by a read; the round's
                        // own stays with the loop.
                        let mut reads = ReadFiles::default();
                        let result = execute_call(turn, round, call, parsed, history, turn.plan, &mut reads, &mut Vec::new());
                        (call.id.clone(), Early { result, started: *started, reads })
                    })
                })
                .collect();
            running
                .into_iter()
                .zip(batch)
                .map(|(handle, (call, parsed, started))| {
                    handle.join().unwrap_or_else(|_| {
                        let result = Err(match parsed {
                            crate::domain::tools::ToolCall::Explore(_) => {
                                crate::domain::tools::ToolError::Explore("the helper failed".to_string())
                            }
                            _ => crate::domain::tools::ToolError::Io(std::io::Error::other("the tool failed")),
                        });
                        (call.id.clone(), Early { result, started: *started, reads: ReadFiles::default() })
                    })
                })
                .collect()
        });
        early.extend(done);
    }
    early
}

/// Puts an MCP server's question to the user and waits for the answer — the
/// call it came from waits with it. Said out of the turn's sequence, like a
/// command's output, from inside the call. Auto does not answer for the
/// user: a question is the server asking a person.
pub(super) fn ask_user(
    turn: &Turn,
    round: u32,
    call_id: &str,
    server: &str,
    question: &crate::domain::mcp::McpQuestion,
) -> crate::domain::mcp::McpAnswer {
    let Some(questions) = &turn.questions else { return crate::domain::mcp::McpAnswer::Decline };
    let id = uuid::Uuid::new_v4().to_string();
    let say = |event: ChatEventPayload| {
        (turn.events)(ChatTurnEvent { seq: 0, round, target_id: Some(format!("round:{round}:tool:{call_id}")), event })
    };
    say(ChatEventPayload::McpQuestion {
        id: id.clone(),
        call: call_id.to_string(),
        server: server.to_string(),
        question: question.clone(),
    });
    let answer = questions.wait(&id, turn.cancelled);
    say(ChatEventPayload::McpQuestionClosed { id, action: answer.action().to_string() });
    answer
}

/// Whether this call has to be shown to a human first.
///
/// An unparseable call is never risky: it cannot run, and asking about a call
/// that is going to fail either way spends the user's attention on nothing.
///
/// A change to a cluster the user marked as production asks whatever "Always
/// allow" says (`docs/21-kubernetes-mode.md`, K-5e): there, a card skipped is
/// the accident.
pub(super) fn needs_approval(turn: &Turn, call: &LlmToolCall) -> (bool, Option<String>) {
    let policy = turn.approval;
    let Ok(parsed) = parse_tool_call(call) else { return (false, None) };
    let production = matches!(turn.place, Place::Chat { kube: KubeSetup::Pinned(target), .. } if target.config.production);
    if production && parsed.name().is_mutating() && parsed.name().wire_name().starts_with("kube") {
        return (true, Some("a production cluster — every change to it asks".to_string()));
    }
    // What a server says about its own tool is its word (`McpToolHints`):
    // "destructive" asks whatever "Always allow" says, "only reads" is put
    // on the card and lifts nothing.
    if let crate::domain::tools::ToolCall::Mcp(args) = &parsed {
        let hints = turn.mcp.hints(&args.name);
        if hints.destructive && !policy.skip_all {
            return (true, Some("its server marks this tool as destructive — every call to it asks".to_string()));
        }
        let asks = policy.requires_approval_for(&parsed);
        let said = hints.read_only.then(|| "its server says this tool only reads — the server's word, not checked".to_string());
        return (asks, said.filter(|_| asks));
    }
    if policy.requires_approval_for(&parsed) {
        return (true, policy.approval_reason(&parsed));
    }
    (false, None)
}

/// What a refused call tells the model. The reason is the point: a model told
/// only "denied" tries the same call again, then a near variant of it.
/// One settled call, as the log keeps it. `args`, `error` and `result`
/// arrive already redacted.
#[allow(clippy::too_many_arguments)]
pub(super) fn log_call(
    turn: &Turn,
    round: u32,
    call: &LlmToolCall,
    args: serde_json::Value,
    status: CallStatus,
    error: Option<String>,
    result: Option<serde_json::Value>,
    started: Instant,
) {
    (turn.log_call)(ToolCallLogEntry {
        ts_ms: crate::infra::tool_call_log::now_ms(),
        // Empty for a chat, which works in no folder.
        repo_root: turn.place.scope().map(|scope| scope.root().display().to_string()).unwrap_or_default(),
        round,
        provider_id: turn.session.provider_id.clone(),
        model: turn.session.model.clone(),
        tool: call.name.clone(),
        args,
        status,
        error,
        result,
        duration_ms: started.elapsed().as_millis() as i64,
    });
}

pub(super) fn denial(decision: &ToolCallDecision) -> String {
    match &decision.reason {
        Some(reason) if !reason.trim().is_empty() => {
            format!("{TOOL_DENIED_PREFIX}: {reason}")
        }
        _ => format!("{TOOL_DENIED_PREFIX}."),
    }
}
