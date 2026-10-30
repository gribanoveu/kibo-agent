import { invoke } from "@tauri-apps/api/core";
import { inTauri, requireBackend } from "./backend";
import type { LlmMessage, Task } from "./chat";
import type { Block } from "./chatTurnReducer";
import type { KubePin } from "./kube";
import type { ChatRoleId } from "./plainChat";

// Saved chats: the sidebar's list, one chat restored, written, exported.

/** A row in the sidebar. Not the whole conversation — see `chat_load`. */
export type ChatSummary = {
  id: string;
  title: string;
  updatedAt: number;
  /** The chat this one was branched from. A branch shares its title. */
  branchedFrom?: string | null;
  /** Filed away: shown under the sidebar's "Archived" filter, not in the list. */
  archived: boolean;
  /** Who answers in a Chat mode conversation; absent for the agent's and for chats saved before rows kept it. */
  role?: ChatRoleId | null;
  /** The git branch the chat works on — its folder's when last saved; null outside a repository and for chats saved before rows kept it. */
  gitBranch?: string | null;
};

/**
 * One saved conversation. Two lists, because they are not the same list: the
 * model reads `messages`, the reader reads `blocks`, and one tool call is two
 * messages and one block.
 *
 * The shape is fixed on the other side in
 * `src-tauri/src/domain/chat_record.rs`, where `schemaVersion` says which
 * spelling of it this is.
 */
export type ChatRecord = {
  schemaVersion: number;
  id: string;
  workspace: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messages: LlmMessage[];
  blocks: Block[];
  todos: Task[];
  /** Absent in chats saved before plans existed. */
  plan?: string | null;
  branchedFrom?: string | null;
  /** Who the model was, in a Chat mode conversation; absent in the agent's. */
  role?: ChatRoleId | null;
  /** The cluster a Kubernetes chat is pinned to. */
  kube?: KubePin | null;
};

/** Chats of the open folder, newest first. No folder, no backend: no rows. */
export async function listChats(): Promise<ChatSummary[]> {
  if (!inTauri()) return [];
  return invoke<ChatSummary[]>("chat_list");
}

export async function loadChat(id: string): Promise<ChatRecord> {
  requireBackend();
  return invoke<ChatRecord>("chat_load", { id });
}

export async function saveChat(
  id: string,
  messages: LlmMessage[],
  blocks: Block[],
  todos: Task[],
  plan: string | null = null,
  branchedFrom: string | null = null,
): Promise<ChatSummary> {
  requireBackend();
  return invoke<ChatSummary>("chat_save", { id, messages, blocks, todos, plan, branchedFrom });
}

/**
 * Names chat `id` with the active model, from its first message saved. Resolves once the name is
 * stored — re-read the list then; rejects when the model could not name it, and the chat keeps its name.
 */
export async function generateChatTitle(id: string): Promise<void> {
  requireBackend();
  await invoke("chat_generate_title", { id });
}

/**
 * Journals the turn `turnId` of chat `chatId` for the next-prompt model
 * (docs/20-next-prompt-suggestions.md). `user` is the message that started it
 * as the transcript shows it. Returns the row's id, or `null` when there was
 * nothing to journal.
 */
export async function logNextPrompt(chatId: string, turnId: string, user: string): Promise<string | null> {
  requireBackend();
  return invoke<string | null>("next_prompt_log", { chatId, turnId, user });
}

/** What the user sent after the journaled turn `id`. */
export async function nextPromptSent(id: string, text: string): Promise<void> {
  requireBackend();
  return invoke<void>("next_prompt_sent", { id, text });
}

/** Writes the saved chat to `path` as Markdown — a transcript to read or analyse elsewhere. */
export async function exportChat(id: string, path: string): Promise<void> {
  requireBackend();
  return invoke("chat_export", { id, path });
}

export async function setChatArchived(id: string, archived: boolean): Promise<void> {
  requireBackend();
  return invoke("chat_set_archived", { id, archived });
}

export async function deleteChat(id: string): Promise<void> {
  requireBackend();
  return invoke("chat_delete", { id });
}
