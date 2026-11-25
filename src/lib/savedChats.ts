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
  /** Held above the list, in the "Pinned" group ahead of the days. */
  pinned?: boolean;
  /** The folder holding the chat — a ChatGroup id; absent for unfiled chats and rows saved before folders. */
  group?: string | null;
  /** Who answers in a Chat mode conversation; absent for the agent's and for chats saved before rows kept it. */
  role?: ChatRoleId | null;
  /** The git branch the chat works on — its folder's when last saved; null outside a repository and for chats saved before rows kept it. */
  gitBranch?: string | null;
};

/** The provider and model a chat speaks with — its own pin; `model: null` is "auto", the provider's own pin. */
export type ChatModel = {
  provider: string;
  model?: string | null;
};

/** A folder the sidebar files chats under — a theme of work, not a date. */
export type ChatGroup = {
  id: string;
  name: string;
  position: number;
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
  /** The branch this chat's worktree was created from. */
  worktreeBase?: string | null;
  /** The commit HEAD pointed at when the chat was first saved — a review's
   * "what this chat did" starts from it. Absent in older chats and in those
   * begun before their folder's first commit. */
  headAtStart?: string | null;
  /** The provider and model this chat speaks with; absent for a chat not
   * pinned yet — one saved before pins existed. The global choice speaks for it. */
  model?: ChatModel | null;
};

/** Chats of the folder `workspace` names, newest first. `null` (Chat mode), no folder, no backend: no rows. */
export async function listChats(workspace: string | null): Promise<ChatSummary[]> {
  if (!inTauri()) return [];
  return invoke<ChatSummary[]>("chat_list", { workspace });
}

export async function loadChat(id: string): Promise<ChatRecord> {
  requireBackend();
  return invoke<ChatRecord>("chat_load", { id });
}

/**
 * The turn's transcript so far — its blocks, checklist and plan — journaled
 * at each round's end, so a crash keeps what the window drew as the loop
 * keeps what the model said. Best-effort: a failed write costs crash
 * protection, never the turn. The turn's normal save folds it away.
 */
export async function journalBlocks(
  id: string,
  turnId: string,
  blocks: Block[],
  todos: Task[],
  plan: string | null,
  from: number,
): Promise<void> {
  requireBackend();
  await invoke("chat_journal_blocks", { id, turnId, blocks, todos, plan, from });
}

export async function saveChat(
  id: string,
  messages: LlmMessage[],
  blocks: Block[],
  todos: Task[],
  plan: string | null = null,
  branchedFrom: string | null = null,
  /** The folder a new chat was typed in; ignored once the chat is saved or its turn has started. */
  workspace: string | null = null,
  /** The turn this save ends: its journal folds, and only its — the next
      turn may already be writing its own. `null` folds every turn's rows. */
  turnId: string | null = null,
): Promise<ChatSummary> {
  requireBackend();
  return invoke<ChatSummary>("chat_save", { id, messages, blocks, todos, plan, branchedFrom, workspace, turnId });
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
 * Journals the turn `turnId` of chat `chatId` for the next-prompt model.
 * `user` is the message that started it
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

/** Moves this saved agent chat to its own clean worktree; resolves to its folder. */
export async function moveChatToWorktree(id: string): Promise<string> {
  requireBackend();
  return invoke<string>("chat_move_to_worktree", { id });
}

export async function setChatArchived(id: string, archived: boolean): Promise<void> {
  requireBackend();
  return invoke("chat_set_archived", { id, archived });
}

/** A pick made in this chat's chip: the chat speaks through it from the next
 * turn. `null` model is "auto" — the provider's own pin decides. */
export async function setChatModel(id: string, provider: string, model: string | null): Promise<void> {
  requireBackend();
  return invoke("chat_model_set", { id, provider, model });
}

/** Holds the chat above the list in its own "Pinned" group, or lets it sink back to its day. */
export async function setChatPinned(id: string, pinned: boolean): Promise<void> {
  requireBackend();
  return invoke("chat_set_pinned", { id, pinned });
}

/** Which set of chat folders: Chat mode's own, or the one every project shares. */
export type GroupList = "chat" | "projects";

/** The chat folders of a set, in the user's order; no backend: none. */
export async function listChatGroups(list: GroupList): Promise<ChatGroup[]> {
  if (!inTauri()) return [];
  return invoke<ChatGroup[]>("chat_group_list", { list });
}

/** Makes a folder at the end of the set's folders; the backend refuses a name another folder of the set carries. */
export async function createChatGroup(list: GroupList, name: string): Promise<ChatGroup> {
  requireBackend();
  return invoke<ChatGroup>("chat_group_create", { list, name });
}

export async function renameChatGroup(list: GroupList, id: string, name: string): Promise<void> {
  requireBackend();
  return invoke("chat_group_rename", { list, id, name });
}

/** Removes the folder; its chats stay, filed under no folder. */
export async function deleteChatGroup(list: GroupList, id: string): Promise<void> {
  requireBackend();
  return invoke("chat_group_delete", { list, id });
}

/** Moves the chat into the folder `group`, or out to no folder with `null`. */
export async function setChatGroup(id: string, group: string | null): Promise<void> {
  requireBackend();
  return invoke("chat_set_group", { id, group });
}

export async function deleteChat(id: string): Promise<void> {
  requireBackend();
  return invoke("chat_delete", { id });
}
