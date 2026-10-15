import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";
import type { Checkpoint, ContextUsage, LlmMessage, Outcome, PendingToolCall, ToolCallDecision, ToolPreview } from "./chat";
import type { KubePin } from "./kube";
import type { ChatSummary } from "./savedChats";

// Chat mode: a role's conversation with no folder (`docs/21-kubernetes-mode.md`).

/** The same for a chat's card: a change to its pinned cluster, dry-run there. */
export async function plainChatPreview(kube: KubePin | null, calls: PendingToolCall[]): Promise<ToolPreview[]> {
  if (!inTauri()) return calls.map(() => ({ kind: "nothing" }) as ToolPreview);
  return invoke<ToolPreview[]>("plain_chat_preview", { kube, calls });
}

/** Mirrors `domain::chat_role::ChatRole`: who the model is in Chat mode, and which tools it has. */
export type ChatRoleId = "assistant" | "kubernetes";
export type ChatRoleView = { id: ChatRoleId; name: string; description: string };

export async function plainChatRoles(): Promise<ChatRoleView[]> {
  if (!inTauri()) return [];
  return invoke<ChatRoleView[]>("plain_chat_roles");
}

/**
 * A turn in `role` on `messages`, the whole conversation so far, outside any
 * folder — the agent's loop with the role's prompt and tools. Its text, calls
 * and results arrive on `onTurnEvent(turnId)` as the agent's do; this resolves
 * with how it ended: done, stopped, or paused on the approval card.
 */
export async function plainChatSend(
  turnId: string,
  role: ChatRoleId,
  kube: KubePin | null,
  messages: LlmMessage[],
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("plain_chat_send", { turnId, role, kube, messages });
}

/** Continues a chat's turn paused on the approval card, with the user's answers. */
export async function plainChatResume(
  turnId: string,
  role: ChatRoleId,
  kube: KubePin | null,
  checkpoint: Checkpoint,
  decisions: ToolCallDecision[],
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("plain_chat_resume", { turnId, role, kube, checkpoint, decisions });
}

/** "Always" on a chat's card: this tool stops asking in Chat mode until the app quits. */
export async function plainChatAlwaysAllow(tool: string): Promise<void> {
  requireBackend();
  return invoke<void>("plain_chat_always_allow", { tool });
}

/** Chat mode's conversations, newest first, whatever folder is open. They open with `loadChat`. */
export async function plainChatList(): Promise<ChatSummary[]> {
  if (!inTauri()) return [];
  return invoke<ChatSummary[]>("plain_chat_list");
}

/** `messages` is what the model is sent again; `blocks` the transcript as drawn — the agent's blocks. */
export async function plainChatSave(
  id: string,
  role: ChatRoleId,
  kube: KubePin | null,
  messages: LlmMessage[],
  blocks: unknown,
): Promise<ChatSummary> {
  requireBackend();
  return invoke<ChatSummary>("plain_chat_save", { id, role, kube, messages, blocks });
}

/** `contextUsage` for a chat in `role`: its prompt and tools. */
export async function plainChatContextUsage(
  role: ChatRoleId,
  kube: KubePin | null,
  messages: LlmMessage[],
): Promise<ContextUsage> {
  requireBackend();
  return invoke<ContextUsage>("plain_chat_context_usage", { role, kube, messages });
}

/** `compactHistory` for a chat in `role`, against its own prompt and tools. */
export async function plainChatCompact(
  role: ChatRoleId,
  kube: KubePin | null,
  messages: LlmMessage[],
  force: boolean,
  turnId: string,
): Promise<{ history: LlmMessage[]; folded: number } | null> {
  requireBackend();
  return invoke<{ history: LlmMessage[]; folded: number } | null>("plain_chat_compact", { role, kube, messages, force, turnId });
}

export async function plainChatCancel(): Promise<void> {
  requireBackend();
  return invoke<void>("plain_chat_cancel");
}
