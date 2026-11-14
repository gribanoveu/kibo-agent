//! A fresh round: the notes the history is owed, the request, and what the reply asks for.

use super::*;

/// How many times one turn answers an empty reply with a nudge rather than
/// ending. One: a model that says nothing twice running is not going to be
/// talked round by a third note, and each costs a round.
// ponytail: fixed at one; a counter per round if a provider's blank replies
// turn out to come in pairs.
pub(super) const MAX_EMPTY_NUDGES: u32 = 1;

/// Said to a model whose reply had neither text nor a call. Without it the
/// turn ended there, as if finished: in `agent_bench`, four of GLM's six
/// failures were a blank reply after a round of reads — the task abandoned
/// with nothing said and nothing done.
pub(super) const EMPTY_REPLY_NOTE: &str = "[Your last reply was empty — no text and no tool call. If the task is not finished, carry on with it; if it is, say what you did.]";

/// The same, when the reply was empty because the response length limit ran
/// out first — usually on thinking.
pub(super) const EMPTY_TRUNCATED_NOTE: &str = "[Your last reply was cut off by the response length limit before it said anything. Carry on, and think more briefly this time.]";

/// A fresh round: the notes the history is owed, the request, the model's
/// reply — and what the reply asks for. Calls refused before running are
/// answered here; calls that need the user pause the round.
pub(super) fn ask_round(turn: &Turn, events: &Events, round: u32, state: &mut State, pass: &mut Pass) -> Result<Next, TurnError> {
    prepare_round(turn, events, round, state, pass);

    let result = match ask_the_model(
        turn,
        &events,
        round,
        &mut state.history,
        Folding { todos: &state.todos, plan: state.written_plan.as_deref().or(turn.plan), tail_percent: RETRY_KEEP_TAIL_PERCENT },
        &mut pass.seen_results,
    )? {
        Some(result) => result,
        // Cancelled during a retry wait.
        None => return Ok(Next::Cancelled(ChatStreamResult::default())),
    };
    report_reply(events, round, pass, &result);

    // Checkpoint two. Before the pause check and before any call runs,
    // so a stop that landed as the round finished pre-empts the write
    // that round asked for — not merely the model's next sentence.
    if (turn.cancelled)() {
        return Ok(Next::Cancelled(result));
    }

    if result.tool_calls.is_empty() {
        return Ok(without_calls(turn, events, round, state, pass, result));
    }
    Ok(with_calls(turn, events, round, state, pass, &result))
}

/// What the history is owed before the request goes out, in the order the
/// transcript shows it: the loop guard's note, a review's wrap-up, what the
/// user typed, processes that ended, then room made and the standing notes.
fn prepare_round(turn: &Turn, events: &Events, round: u32, state: &mut State, pass: &mut Pass) {
    let limits = turn.session.limits;
    // Before the round is announced, so the notes and the boundary
    // land in the transcript in the order the history has them. A
    // round that ran nothing hands in nothing, which ends the streaks.
    if let Some(found) = pass.guard.observe(&std::mem::take(&mut pass.settled)) {
        state.history.push(note(turn, found.note()));
        events.emit(
            round,
            Some(format!("loop:{round}")),
            ChatEventPayload::LoopReminded {
                tool: found.tool().to_string(),
                failing: matches!(found, Loop::SameError { .. }),
            },
        );
    }
    if turn.place.mode() == Some(ConversationMode::Review) {
        let used = crate::domain::review::Budget { rounds: round - 1, weight: state.budget_used };
        let limit = crate::domain::review::Budget { rounds: limits.rounds, weight: limits.budget };
        if let Some(said) = crate::domain::review::wrap_up(used, limit, &state.history) {
            state.history.push(note(turn, said.clone()));
            events.emit(
                round,
                Some(format!("wrap-up:{round}")),
                ChatEventPayload::WrapUpReminded { rounds: round - 1, tokens: pass.spent, text: said },
            );
        }
    }
    apply_steering(&events, round, &mut state.history, (turn.take_steering)());
    report_ended_processes(turn, &events, round, &mut state.history);
    // The window's ladder, cheapest first: old results at 85%, then a
    // summary at 90% if clearing was not enough, then — should the
    // provider still refuse — a harder summary in `ask_the_model`.
    clear_stale_results(turn, state, &mut pass.seen_results);
    let usage = request_usage(turn, &state.history);
    if compaction::should_compact(usage.total, usage.limit) {
        let ahead = Folding { todos: &state.todos, plan: state.written_plan.as_deref().or(turn.plan), tail_percent: KEEP_TAIL_PERCENT };
        match fold(turn, &events, round, &mut state.history, ahead, &mut pass.seen_results) {
            Ok(_) => {}
            // A pass that could not help leaves the history as it was:
            // if the request really does not fit, the provider says
            // so and the pass after a refusal takes over.
            Err(_) => {}
        }
    }
    restore_standing(&mut state.history, turn);
    restore_plan(&mut state.history, state.written_plan.as_deref().or(turn.plan));
    restore_checklist(&mut state.history, &state.todos);
    events.emit(round, Some(format!("round:{round}")), ChatEventPayload::RoundStarted);
    events.emit(round, Some(format!("estimate:{round}")), ChatEventPayload::ContextEstimate(request_usage(turn, &state.history)));
}

/// What the reply cost and what it said. Said outright rather than left to
/// the deltas that streamed it, and said before the round's exits: a round
/// that is about to be cancelled, or to pause on a confirmation, has still
/// reported what it said.
fn report_reply(events: &Events, round: u32, pass: &mut Pass, result: &ChatStreamResult) {
    if let Some(usage) = result.usage {
        pass.spent += u64::from(usage.prompt_tokens) + u64::from(usage.completion_tokens);
        events.emit(
            round,
            Some(format!("round:{round}")),
            ChatEventPayload::ContextUsage(usage),
        );
    }
    events.emit(
        round,
        Some(format!("round:{round}")),
        ChatEventPayload::RoundCompleted {
            text: result.text.clone(),
            reasoning: result.reasoning.clone(),
            truncated: result.truncated,
        },
    );
}

/// A reply that asked for nothing: the model is done — unless the user said
/// something while it was answering, the reply was empty, or a Stop hook
/// sends it back.
fn without_calls(turn: &Turn, events: &Events, round: u32, state: &mut State, pass: &mut Pass, result: ChatStreamResult) -> Next {
    // Ending the turn here would silently drop what the user typed, and
    // they would have no way to tell it was never seen.
    let waiting = (turn.take_steering)();
    // An empty reply is not an ending, and is never kept: an
    // assistant message with neither text nor calls is one
    // providers refuse. What the user typed meanwhile is reason
    // enough to go on; otherwise the model is sent back once. No
    // Stop hook is asked either way — nothing is finishing.
    if result.text.trim().is_empty() && (!waiting.is_empty() || pass.empty_nudges < MAX_EMPTY_NUDGES) {
        if waiting.is_empty() {
            pass.empty_nudges += 1;
            let said = if result.truncated { EMPTY_TRUNCATED_NOTE } else { EMPTY_REPLY_NOTE };
            state.history.push(note(turn, said.to_string()));
        }
        apply_steering(&events, round, &mut state.history, waiting);
        return Next::Again;
    }
    // Only a turn that is really ending asks its Stop hooks; they
    // run every time — one may be a notification — but past the
    // cap a refusal no longer keeps the turn going.
    // A helper's answer — research, a review — is not the turn
    // ending: that is the calling turn's, and its Stop hooks run then.
    let helper = turn.fork || matches!(turn.place.mode(), Some(ConversationMode::Explore | ConversationMode::Review));
    let refused = if waiting.is_empty() && !helper {
        let fields = serde_json::json!({ "stop_hook_active": pass.stop_blocks > 0 });
        match fire_hook(turn, &events, round, HookEvent::Stop, None, fields) {
            Some(_) if pass.stop_blocks >= MAX_STOP_BLOCKS => {
                report_hook(&events, round, HookEvent::Stop, format!(
                    "Stop hooks kept the turn going {MAX_STOP_BLOCKS} times; it ends here anyway"
                ), false);
                None
            }
            refused => refused,
        }
    } else {
        None
    };
    if waiting.is_empty() && refused.is_none() {
        return Next::Done(result);
    }
    state.history.push(LlmMessage {
        role: LlmRole::Assistant,
        content: (!result.text.is_empty()).then(|| result.text.clone()),
        tool_call_id: None,
        tool_calls: vec![],
        native_content: result.native_content.clone(),
        images: Vec::new(),
    });
    if let Some(reason) = refused {
        pass.stop_blocks += 1;
        state.history.push(note(turn, format!(
            "[A Stop hook did not let the turn end yet. It said:]\n{reason}"
        )));
    }
    apply_steering(&events, round, &mut state.history, waiting);
    Next::Again
}

/// A reply that asked for calls: it goes into the history, each call is
/// checked before anything runs, and the ones left either run now or wait
/// for the user.
fn with_calls(turn: &Turn, events: &Events, round: u32, state: &mut State, pass: &mut Pass, result: &ChatStreamResult) -> Next {
    let round_truncated = result.truncated;
    // The assistant's own turn goes back into the history before its
    // results do, so the next request shows the provider its own prior
    // request. `None` content for a tool-only turn is what the wire
    // actually says.
    state.history.push(LlmMessage {
        role: LlmRole::Assistant,
        content: (!result.text.is_empty()).then(|| result.text.clone()),
        tool_call_id: None,
        tool_calls: sanitize_tool_call_arguments(&result.tool_calls),
        native_content: result.native_content.clone(),
        images: Vec::new(),
    });
    state.budget_used += round_cost(&result.tool_calls, turn.mcp);

    // Containment before approval: a write outside the workspace has
    // to fail as a tool error now, not show the user a card for an
    // operation that cannot happen. Severed arguments fail here too,
    // which is the case the truncation note exists for.
    let mut runnable: Vec<LlmToolCall> = Vec::new();
    for call in &result.tool_calls {
        match preflight(turn, &state.reads, call) {
            Ok(()) => runnable.push(call.clone()),
            Err(e) => {
                report_call(&events, round, call);
                // Refused before running — a path out of the folder,
                // a write without a read — which is exactly what an
                // audit trail is read for.
                let args = parse_tool_call(call).map_or(serde_json::Value::Null, |p| tool_call_log::redact_args(&p));
                log_call(turn, round, call, args, CallStatus::Error, Some(tool_call_log::redact_error(&e)), None, Instant::now());
                let message = format!("{TOOL_ERROR_PREFIX}{e}");
                pass.settled.push(Settled {
                    tool: call.name.clone(),
                    arguments: call.arguments.clone(),
                    content: message.clone(),
                    error: Some(loop_guard::error_kind(&e)),
                });
                report_result(&events, round, &call.id, None, Some(&message), Vec::new());
                state.history.push(tool_message(
                    &call.id,
                    truncated_round_note(round_truncated, true, message),
                ));
            }
        }
    }
    if runnable.is_empty() {
        // Every call in the round was refused before running. Let the
        // model react to the errors on the next round.
        return Next::Again;
    }

    let pending: Vec<PendingToolCall> = runnable
        .iter()
        .map(|call| {
            let (requires_confirmation, reason) = needs_approval(turn, call);
            PendingToolCall {
                requires_confirmation,
                reason,
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            }
        })
        .collect();
    if pending.iter().any(|call| call.requires_confirmation) {
        // The whole round pauses, including the calls that need no
        // decision: with nothing executed there is no partial round to
        // describe to whoever resumes it.
        return Next::Paused(pending);
    }
    Next::Run(runnable, round_truncated)
}

/// Replaces old tool results with stubs once the history has grown — the
/// rules, and why they suit a prompt cache, are `domain::result_clearing`'s —
/// and makes the turn forget them as well. A repeat of a cleared call must
/// come back in full rather than as "already above", and a file whose text
/// the model no longer has must be read again before it is replaced whole.
pub(super) fn clear_stale_results(turn: &Turn, state: &mut State, seen_results: &mut HashMap<String, u64>) {
    let scope = turn.place.scope();
    let usage = request_usage(turn, &state.history);
    let kept = |name: &str| turn.mcp.keeps_results(name);
    for cleared in result_clearing::plan(&state.history, usage.total, usage.limit, &kept) {
        state.history[cleared.index].content = Some(cleared.stub);
        seen_results.remove(&format!("{}|{}", cleared.tool, cleared.arguments));
        if ToolName::from_wire_name(&cleared.tool) != Some(ToolName::ReadFile) {
            continue;
        }
        // A path that no longer resolves has nothing left to forget.
        let Some(scope) = scope else { continue };
        let args = serde_json::from_str(&cleared.arguments).unwrap_or_default();
        for path in crate::domain::tools::read_paths(&args) {
            if let Ok(relative) = resolve_existing(scope, &path).and_then(|resolved| relative_to_root(scope, &resolved)) {
                state.reads.forget_whole(&relative);
            }
        }
    }
}

/// Adds queued notes to the conversation and says so, one event per note, so
/// the front end can retire each by id rather than by matching its text.
pub(super) fn apply_steering(
    events: &Events,
    round: u32,
    history: &mut Vec<LlmMessage>,
    notes: Vec<SteeringNote>,
) {
    for note in notes {
        // Neither words nor pictures: `prefixed()` would hand the model the
        // prefix and nothing after its colon, so there is nothing to send.
        if note.text.trim().is_empty() && note.images.is_empty() {
            continue;
        }
        let prefixed = note.prefixed();
        let SteeringNote { id, text, shown, images } = note;
        // The pictures ride with the words: a note made of a screenshot alone
        // is what the model is meant to look at this round.
        history.push(LlmMessage::user_with_images(prefixed, images.clone()));
        events.emit(
            round,
            Some(format!("steer:{id}")),
            ChatEventPayload::SteeringApplied { id, text: shown.unwrap_or(text), images },
        );
    }
}

/// One round against the provider, retried while [`retry_delay`] allows it.
///
/// `Ok(None)` means the turn was cancelled during a wait — the caller turns
/// that into the same cancelled outcome as every other stopping point.
/// One round's request, made once more against a shorter history if the
/// provider says the conversation no longer fits.
///
/// The compaction is reactive on purpose: it happens because a request was
/// actually refused, not because an estimate guessed it would be. Once, and
/// only once — a second refusal after the history has already been summarized
/// is not about the history's length, and summarizing again would spend
/// another request to lose more of the conversation for nothing.
///
/// A pass that cannot help leaves `history` alone and the original refusal is
/// what the turn reports: "this conversation does not fit" is the useful
/// thing to read, and "the summarizer also failed" is not.
pub(super) fn ask_the_model(
    turn: &Turn,
    events: &Events,
    round: u32,
    history: &mut Vec<LlmMessage>,
    folding: Folding,
    seen_results: &mut HashMap<String, u64>,
) -> Result<Option<ChatStreamResult>, TurnError> {
    let mut compacted = false;
    loop {
        let request = ChatRequest {
            messages: request_messages(turn, history),
            tools: offered(turn, history),
            model: turn.session.model.clone(),
        };
        let error = match stream_one_round(turn, events, round, request) {
            Ok(result) => return Ok(result),
            Err(TurnError::Provider(error)) if !compacted && too_long(&error) => error,
            Err(other) => return Err(other),
        };
        compacted = true;

        // Harder than a proactive pass would: the window is not nearly full,
        // it is already over.
        match fold(turn, events, round, history, folding, seen_results) {
            Ok(true) => {}
            // Nothing could be folded, or the summarizer itself failed: report
            // what the model actually refused.
            Ok(false) | Err(_) => return Err(TurnError::Provider(error)),
        }
    }
}

/// What a fold needs of the turn besides its history: what to put back
/// after it, and how much to keep word for word.
#[derive(Clone, Copy)]
pub(super) struct Folding<'a> {
    todos: &'a [Task],
    plan: Option<&'a str>,
    tail_percent: u64,
}

/// One pass over the turn's history, keeping `tail_percent` of the window
/// word for word, said on the turn's channel as the window's own pass is.
/// `false` when there was nothing worth folding or the summary came back
/// empty; the history is then as it was.
pub(super) fn fold(
    turn: &Turn,
    events: &Events,
    round: u32,
    history: &mut Vec<LlmMessage>,
    folding: Folding,
    seen_results: &mut HashMap<String, u64>,
) -> Result<bool, LlmError> {
    let started = || events.emit(round, Some(format!("round:{round}")), ChatEventPayload::HistoryCompacting);
    let tail = compaction::share_of_window(turn.session.context_limit, folding.tail_percent);
    let Some(shorter) = context_compaction::compact(turn.session, history, tail, &started)? else {
        return Ok(false);
    };
    *history = shorter.history;
    // What was read before the cut is gone from the history: a repeat of it
    // must come back in full, not as "already above".
    seen_results.clear();
    // The summary may have folded the last list, and the last plan, away.
    restore_standing(history, turn);
    restore_plan(history, folding.plan);
    restore_checklist(history, folding.todos);
    events.emit(round, Some(format!("round:{round}")), ChatEventPayload::HistoryCompacted { folded: shorter.folded });
    Ok(true)
}

/// What a request of `turn` with `history` weighs, by the estimate the
/// compaction pass uses — for a caller that shows a cost before the provider
/// reports one.
pub fn estimate_request(turn: &Turn, history: &[LlmMessage]) -> usize {
    compaction::estimate_tokens(&request_messages(turn, history))
        + compaction::estimate_tool_schema_tokens(&offered(turn, history))
}

/// The request's messages: what the model is told, then the conversation.
/// Nothing after it — the checklist lives in the history
/// ([`restore_checklist`]), so each request extends the last.
///
/// The system part is rebuilt every round rather than pushed into `history`
/// once: a folder or a date frozen into the stored conversation would be
/// resent, wrong, for as long as the chat exists. It is the same every round
/// of a turn, so a prompt cache keeps it.
pub(super) fn request_messages(turn: &Turn, history: &[LlmMessage]) -> Vec<LlmMessage> {
    let (scope, mode) = match turn.place {
        Place::Folder { scope, mode, .. } => (scope, mode),
        Place::Chat { role, kube, runbooks, web, .. } => {
            let mut messages = prompt::chat_system_messages(role, kube, runbooks, turn.session.reply_language, web.is_some(), &turn.features, &today());
            messages.extend_from_slice(history);
            return messages;
        }
    };
    let today = today();
    let mut messages = prompt::system_messages(&turn_context(turn, scope, mode, &today));
    messages.extend_from_slice(history);
    messages
}

/// What a request in the open folder is told before the conversation.
pub(super) fn turn_context<'a>(turn: &'a Turn, scope: &'a ToolScope, mode: ConversationMode, today: &'a str) -> prompt::TurnContext<'a> {
    let rules = match turn.frozen {
        Some(frozen) => &frozen.rules[..],
        None => turn.rules,
    };
    prompt::TurnContext {
        mode,
        workspace: scope.root(),
        shell: turn.shell_described,
        today,
        skills: turn.skills,
        rules,
        worktree_of: turn.worktree_of,
        parallel: turn.frozen.and_then(|frozen| frozen.parallel.as_deref()),
        language: turn.session.reply_language,
        mcp_servers: turn.mcp.notes(),
    }
}

/// The next request's cost by part, as `context_compaction::usage` reports it
/// to the meter between turns — the same arithmetic, over the turn's own
/// history.
pub(super) fn request_usage(turn: &Turn, history: &[LlmMessage]) -> crate::domain::compaction::ContextUsage {
    let frame = match turn.place {
        Place::Folder { scope, mode, web, .. } => {
            let today = today();
            context_compaction::request_frame(&turn_context(turn, scope, mode, &today), turn.mcp, web.is_some(), &turn.features)
        }
        Place::Chat { role, kube, runbooks, web, .. } => {
            context_compaction::chat_request_frame(role, kube, runbooks, turn.session.reply_language, web.is_some(), &turn.features)
        }
    };
    context_compaction::usage(turn.session, frame, history)
}

pub(super) fn too_long(error: &LlmError) -> bool {
    compaction::is_context_length_error(&error.to_string())
}

pub(super) fn stream_one_round(
    turn: &Turn,
    events: &Events,
    round: u32,
    request: ChatRequest,
) -> Result<Option<ChatStreamResult>, TurnError> {
    let mut attempt = 0;
    loop {
        llm_debug_log::log_request(turn.session.debug_logging, &turn.session.provider_id, round, &request);

        // Set by anything the stream reports: once a byte of this attempt has
        // reached us, the round is no longer repeatable.
        let produced_output = Cell::new(false);
        // How much of each call has been reported, and whether its name had.
        // Providers hand over a call's arguments whole on every chunk — and
        // OpenAI's every call of the round with it — so sending them on as
        // they come costs the square of a written file on the way to the
        // window. Only what is new goes, and a call nothing was added to is
        // not repeated.
        let reported: RefCell<HashMap<String, (usize, bool)>> = RefCell::default();
        let on_event = |event: StreamEvent| {
            produced_output.set(true);
            match event {
                StreamEvent::Text(delta) => events.emit(
                    round,
                    Some(format!("round:{round}:text")),
                    ChatEventPayload::Delta { delta: delta.to_string() },
                ),
                StreamEvent::Reasoning(delta) => events.emit(
                    round,
                    Some(format!("round:{round}:reasoning")),
                    ChatEventPayload::Reasoning { delta: delta.to_string() },
                ),
                StreamEvent::ToolCall { id, name, arguments } => {
                    let mut reported = reported.borrow_mut();
                    let before = reported.get(id).copied();
                    let (sent, named) = before.unwrap_or((0, false));
                    // The arguments only ever grow; `get` rather than slicing keeps a
                    // provider that broke that from panicking the turn.
                    let new = arguments.get(sent..).unwrap_or("");
                    if before.is_some() && new.is_empty() && (named || name.is_empty()) {
                        return;
                    }
                    reported.insert(id.to_string(), (arguments.len().max(sent), named || !name.is_empty()));
                    events.emit(
                        round,
                        Some(format!("round:{round}:tool:{id}")),
                        ChatEventPayload::ToolCallDelta(ToolCallEvent {
                            id: id.to_string(),
                            name: name.to_string(),
                            arguments: new.to_string(),
                        }),
                    );
                }
            }
        };

        let result = turn.session.provider.chat_stream(request.clone(), &on_event, turn.cancelled);
        llm_debug_log::log_response(turn.session.debug_logging, &turn.session.provider_id, round, &result);

        let error = match result {
            Ok(result) => return Ok(Some(result)),
            Err(error) => error,
        };
        let Some(delay) = retry_delay(&error, attempt, produced_output.get()) else {
            return Err(TurnError::Provider(error));
        };
        attempt += 1;
        events.emit(
            round,
            Some(format!("round:{round}")),
            ChatEventPayload::Retrying {
                attempt,
                max_attempts: MAX_ATTEMPTS,
                delay_seconds: delay.as_secs(),
            },
        );
        if !wait(turn, delay) {
            return Ok(None);
        }
    }
}

/// Sleeps in one-second slices, checking for a stop between them. `false` if
/// the turn was cancelled before the wait was over — a minute-long wait that
/// ignored the stop button would look exactly like a hang.
pub(super) fn wait(turn: &Turn, delay: Duration) -> bool {
    let slice = Duration::from_secs(1);
    let mut left = delay;
    while !left.is_zero() {
        if (turn.cancelled)() {
            return false;
        }
        let step = left.min(slice);
        (turn.sleep)(step);
        left -= step;
    }
    !(turn.cancelled)()
}
