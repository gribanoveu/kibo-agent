//! Which tools a request offers, per mode, role and feature.

use super::*;

/// What this mode advertises. Leaving a tool out of the request is the half
/// of the gate the model can see; [`preflight_tool_call`] is the half it
/// cannot, and both are needed — a model that used `writeFile` earlier in a
/// conversation calls it again from memory when the mode narrows.
///
/// `history` is the conversation so far: the deferred MCP tools a `toolSearch`
/// in it found are declared with the rest. `toolSearch` itself only while
/// some tool waits for it — offered with nothing to find, it costs every
/// request its schema for nothing.
/// `web`: whether the agent's web search is on — off, `webSearch` and
/// `webFetch` are left out, as in a chat.
/// `features`: the ones the user has on — a tool of one that is off is left out.
/// The app's own tools wait for `toolSearch` as a deferred server's do: one
/// not `loads_up_front` is declared once a search in `history` found it.
pub(crate) fn tool_definitions_for(
    mode: ConversationMode,
    mcp: &McpTools,
    history: &[LlmMessage],
    web: bool,
    features: &Features,
) -> Vec<LlmToolDefinition> {
    let loaded = crate::domain::mcp::loaded_tools(history);
    let waiting = waiting_tools(mode, web, features);
    let searched = mcp.has_deferred() || !waiting.is_empty();
    tool_definitions()
        .into_iter()
        .filter(|definition| loaded.contains(&definition.name) || !waiting.iter().any(|w| w.name == definition.name))
        .chain(mcp.definitions(&loaded))
        .filter(|definition| {
            ToolName::from_wire_name(&definition.name).is_some_and(|tool| {
                offered_here(mode, web, features, tool) && (tool != ToolName::ToolSearch || searched)
            })
        })
        .map(|definition| match ToolName::from_wire_name(&definition.name) {
            Some(ToolName::ToolSearch) => crate::services::ai_tools::tools::tool_search::definition_for(&waiting, mcp.has_deferred()),
            _ => definition,
        })
        .collect()
}

/// Whether `mode` offers `tool` with the web and the features as they are.
pub(super) fn offered_here(mode: ConversationMode, web: bool, features: &Features, tool: ToolName) -> bool {
    conversation_mode::offers(mode, tool) && (web || !tool.is_web()) && features.offers(tool)
}

/// The app's own tools this turn offers but does not declare until
/// `toolSearch` finds them — none where there is no `toolSearch`.
pub(crate) fn waiting_tools(mode: ConversationMode, web: bool, features: &Features) -> Vec<LlmToolDefinition> {
    if !offered_here(mode, web, features, ToolName::ToolSearch) {
        return Vec::new();
    }
    tool_definitions()
        .into_iter()
        .filter(|definition| {
            ToolName::from_wire_name(&definition.name)
                .is_some_and(|tool| !tool.loads_up_front() && offered_here(mode, web, features, tool))
        })
        .collect()
}

/// What the turn advertises: the mode's tools in a folder, the role's in a chat.
pub(super) fn offered(turn: &Turn, history: &[LlmMessage]) -> Vec<LlmToolDefinition> {
    match turn.place {
        Place::Folder { mode, web, .. } => tool_definitions_for(mode, turn.mcp, history, web.is_some(), &turn.features),
        Place::Chat { role, web, .. } => tool_definitions_for_role(role, web.is_some(), &turn.features),
    }
}

/// The tools a chat in `role` is offered.
/// `web`: whether web search is on. Off, `webSearch` and `webFetch` are left
/// out — offered, they could only fail, and their schemas cost every request.
pub(crate) fn tool_definitions_for_role(role: ChatRole, web: bool, features: &Features) -> Vec<LlmToolDefinition> {
    tool_definitions()
        .into_iter()
        .filter(|definition| {
            ToolName::from_wire_name(&definition.name)
                .is_some_and(|tool| role.tools().contains(&tool) && (web || !tool.is_web()) && features.offers(tool))
        })
        .collect()
}
