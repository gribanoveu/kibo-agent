//! What a turn says outward while it runs: calls, results, hooks, command output.

use super::*;

/// Turns a command's output into turn events as it arrives.
///
/// Deliberately not routed through [`Events`]: that cursor is owned by the
/// loop's own thread, and output arrives on the runner's reader threads. These
/// events carry no sequence number of their own and are ordered by the call
/// they belong to, which is what a listener uses to append them to the right
/// card. The call's `ToolResult` remains the authoritative text.
pub(super) fn command_output_sink(events: &ChatEventSink, round: u32, call_id: &str) -> CommandSink {
    let events = events.clone();
    let target_id = format!("round:{round}:tool:{call_id}");
    let call_id = call_id.to_string();
    Arc::new(move |event: CommandEvent| {
        events(ChatTurnEvent {
            seq: 0,
            round,
            target_id: Some(target_id.clone()),
            event: ChatEventPayload::CommandOutput {
                id: call_id.clone(),
                stream: event.stream,
                chunk: event.chunk,
            },
        });
    })
}

/// Tells the model which background processes ended since it last looked —
/// a dev server that died is otherwise invisible until something fails
/// against its port. Into the history rather than a tail note: a retried
/// request must not lose it, and it is said once.
pub(super) fn report_ended_processes(turn: &Turn, events: &Events, round: u32, history: &mut Vec<LlmMessage>) {
    let Some(processes) = &turn.processes else { return };
    let ended = processes.take_ended();
    let Some(said) = background::ended_note(&ended) else { return };
    history.push(note(turn, said));
    events.emit(round, None, ChatEventPayload::ProcessesEnded { processes: ended });
}

/// Runs the hooks of one event and reports what they said; returns the
/// reason when they refused.
pub(super) fn fire_hook(
    turn: &Turn,
    events: &Events,
    round: u32,
    event: HookEvent,
    tool: Option<&str>,
    fields: serde_json::Value,
) -> Option<String> {
    // The user's hooks run in the folder a turn works in; a chat has none, and
    // a guard written for the agent's files is not the chat's to answer.
    let scope = turn.place.scope()?;
    let verdict = turn.hooks.fire(event, tool, fields, scope.root());
    for warning in verdict.warnings {
        report_hook(events, round, event, warning, false);
    }
    if let Some(reason) = &verdict.blocked {
        report_hook(events, round, event, reason.clone(), true);
    }
    verdict.blocked
}

pub(super) fn report_hook(events: &Events, round: u32, event: HookEvent, message: String, blocked: bool) {
    events.emit(
        round,
        None,
        ChatEventPayload::HookFeedback { event: event.name().to_string(), message, blocked },
    );
}

pub(super) fn report_call(events: &Events, round: u32, call: &LlmToolCall) {
    events.emit(
        round,
        Some(format!("round:{round}:tool:{}", call.id)),
        ChatEventPayload::ToolCall(ToolCallEvent {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        }),
    );
}

pub(super) fn report_result(
    events: &Events,
    round: u32,
    call_id: &str,
    result: Option<&ToolResult>,
    error: Option<&str>,
    changes: Vec<FileChange>,
) {
    events.emit(
        round,
        Some(format!("round:{round}:tool:{call_id}")),
        ChatEventPayload::ToolResult(ToolResultEvent {
            id: call_id.to_string(),
            result: result.cloned(),
            error: error.map(str::to_string),
            changes,
        }),
    );
}
