import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
// Type-only, and erased: the transcript's block shapes are defined beside the
// reducer that builds them, and a saved chat is where they cross the wire.
import type { Block } from "./chatTurnReducer";

// One typed wrapper per command. Components and hooks call these, never
// `invoke` directly — the command names and payload shapes live here and
// nowhere else.

// ---------------------------------------------------------------- the wire

export type Role = "system" | "user" | "assistant" | "tool";

export type LlmToolCall = { id: string; name: string; arguments: string };

export type LlmMessage = {
  role: Role;
  content: string | null;
  toolCallId?: string | null;
  toolCalls?: LlmToolCall[];
  /** The provider's own blocks for this message (Anthropic's signed thinking). Opaque here: carried, never read. */
  nativeContent?: unknown;
};

export type TodoStatus = "pending" | "inProgress" | "completed" | "cancelled";
export type Task = { id: string; title: string; status: TodoStatus; note?: string | null };

export type PendingToolCall = {
  id: string;
  name: string;
  arguments: string;
  requiresConfirmation: boolean;
  /** Why it asks although its tool is always allowed — `rewrites a remote (git push --force)`. */
  reason?: string | null;
};

/** The whole state of a paused turn. Sent back untouched — see `chat_resume`. */
export type Checkpoint = {
  history: LlmMessage[];
  round: number;
  budgetUsed: number;
  eventSeq: number;
  calls: PendingToolCall[];
  todos: Task[];
  reads: unknown;
};

export type ToolCallDecision = { id: string; approved: boolean; reason?: string | null };

export type ChatUsage = {
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  /** Of `promptTokens`, what the provider read from its prompt cache. */
  cachedTokens?: number;
};

export type TurnResult = {
  text: string;
  reasoning?: string;
  usage?: ChatUsage | null;
  toolCalls?: LlmToolCall[];
  truncated: boolean;
  todos: Task[];
  /** The conversation as the turn left it, calls and results included — what the next message is sent with. */
  history: LlmMessage[];
};

export type Outcome =
  | { status: "done"; value: TurnResult }
  | { status: "pendingApproval"; value: Checkpoint }
  | { status: "cancelled"; value: TurnResult };

export type OutputStream = "stdout" | "stderr";

/** One event from a running turn. Flat: `type` picks the branch. */
export type TurnEvent = { turnId: string; seq: number; round: number; targetId?: string | null } & (
  | { type: "delta"; payload: { delta: string } }
  | { type: "reasoning"; payload: { delta: string } }
  | { type: "retrying"; payload: { attempt: number; maxAttempts: number; delaySeconds: number } }
  | { type: "steeringApplied"; payload: { id: string; text: string } }
  | { type: "historyCompacting" }
  | { type: "historyCompacted"; payload: { folded: number } }
  | { type: "hookFeedback"; payload: { event: string; message: string; blocked: boolean } }
  | { type: "processesEnded"; payload: { processes: ProcessInfo[] } }
  | { type: "loopReminded"; payload: { tool: string; failing: boolean } }
  | { type: "wrapUpReminded"; payload: { rounds: number; tokens: number } }
  | { type: "roundStarted" }
  | { type: "roundCompleted"; payload: { text: string; reasoning?: string; truncated?: boolean } }
  | { type: "toolCallDelta"; payload: LlmToolCall }
  | { type: "toolCall"; payload: LlmToolCall }
  | { type: "toolResult"; payload: { id: string; result?: unknown; error?: string | null; changes?: FileChange[] } }
  | { type: "commandOutput"; payload: { id: string; stream: OutputStream; chunk: string } }
  | { type: "contextUsage"; payload: ChatUsage }
);

/**
 * The other half of a contract whose first half is `CHAT_TURN_EVENT` in
 * `src-tauri/src/commands/chat_events.rs`. Nothing checks that the two match:
 * a mismatch shows up as a window where nothing ever happens.
 */
export const CHAT_TURN_EVENT = "chat:turn-event";

// ------------------------------------------------------------- the commands

const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Outside the app there is no backend. Say so, rather than let `invoke` fail with its own wording. */
function requireBackend() {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
}

export async function openWorkspace(path: string): Promise<string> {
  requireBackend();
  return invoke<string>("workspace_open", { path });
}

export async function currentWorkspace(): Promise<string | null> {
  if (!inTauri()) return null;
  return invoke<string | null>("workspace_current");
}

/** The branch checked out in the open folder; `null` outside a repository. */
export async function currentBranch(): Promise<string | null> {
  if (!inTauri()) return null;
  return invoke<string | null>("workspace_branch");
}

/** The main working tree when the open folder is a git worktree of it; `null` otherwise. */
export async function worktreeOf(): Promise<string | null> {
  if (!inTauri()) return null;
  return invoke<string | null>("workspace_worktree_of");
}

/** Folders opened lately that still exist, the last one first. */
/** Mirrors `commands::chat::RecentFolder`: `worktreeOf` is the main folder when this one is a git worktree of it. */
export type RecentFolder = { path: string; worktreeOf: string | null };

export async function recentWorkspaces(): Promise<RecentFolder[]> {
  if (!inTauri()) return [];
  return invoke<RecentFolder[]>("workspace_recent");
}

// --------------------------------------------------------- the folder index

/**
 * The channel the open folder's index reports on. Pinned on the Rust side by
 * `the_channel_name_is_pinned` in `commands/workspace_events.rs`.
 */
export const INDEX_EVENT = "workspace-index:event";

/** One report of a sync. `root` is the string `openWorkspace` returned. */
export type IndexEvent = { root: string } & (
  | { kind: "syncStarted" }
  | { kind: "keywordsReady"; indexed: number; unchanged: number; removed: number; skipped: number }
  | { kind: "embeddingProgress"; done: number; total: number }
  | { kind: "syncFinished"; embedded: number; embeddingError: string | null }
  | { kind: "failed"; error: string }
);

/** The index as it stands, for a window that was not listening when the sync began. */
export type IndexSnapshot = {
  root: string;
  syncing: boolean;
  embedded: number;
  skipped: number;
  embeddingError: string | null;
};

export async function indexStatus(): Promise<IndexSnapshot | null> {
  if (!inTauri()) return null;
  return invoke<IndexSnapshot | null>("workspace_index_status");
}

/** Every folder's index events: a sync can outlive its folder being open. */
export async function onIndexEvent(handler: (event: IndexEvent) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<IndexEvent>(INDEX_EVENT, ({ payload }) => handler(payload));
}

/** Starts a turn. `messages` is the whole conversation: the transcript is the caller's. */
export async function startChat(
  turnId: string,
  messages: LlmMessage[],
  todos: Task[] = [],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("chat_start", { turnId, messages, todos, plan });
}

/**
 * Shortens the conversation when it is worth shortening, and returns the
 * shorter one. `null` means "leave yours alone" — the decision is the
 * backend's, including how much to fold. Once a summary is being made, a
 * `historyCompacting` event says so under `turnId` (`onTurnEvent`).
 */
export async function compactHistory(
  messages: LlmMessage[],
  force = false,
  plan: string | null = null,
  turnId: string,
): Promise<{ history: LlmMessage[]; folded: number } | null> {
  if (!inTauri()) return null;
  return invoke<{ history: LlmMessage[]; folded: number } | null>("chat_compact", {
    messages,
    force,
    plan,
    turnId,
  });
}

/** Continues a paused turn. `checkpoint` goes back exactly as it arrived. */
export async function resumeChat(
  turnId: string,
  checkpoint: Checkpoint,
  decisions: ToolCallDecision[],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("chat_resume", { turnId, checkpoint, decisions, plan });
}

/** Returns at once; the turn stops at its next checkpoint. */
export async function cancelChat(): Promise<void> {
  requireBackend();
  return invoke<void>("chat_cancel");
}

/**
 * Queues a mid-turn note and returns its id, which is what withdraws it.
 * `shown` is what the transcript shows in place of `text` — a `/` command.
 */
export async function steer(text: string, shown?: string): Promise<string> {
  requireBackend();
  return invoke<string>("chat_steer", { text, shown: shown ?? null });
}

/** `false` means a round already took it — what is said to the model cannot be unsaid. */
export async function cancelSteer(id: string): Promise<boolean> {
  requireBackend();
  return invoke<boolean>("chat_cancel_steer", { id });
}

export async function alwaysAllow(tool: string): Promise<void> {
  requireBackend();
  return invoke<void>("approval_always_allow", { tool });
}

/**
 * Ask or Auto for the turns from now on, remembered for `chatId` or for the
 * open folder — whichever Settings says. A chat not saved yet (`null`) is
 * remembered when it first is.
 */
export async function setUnattended(unattended: boolean, chatId: string | null = null): Promise<void> {
  requireBackend();
  return invoke<void>("approval_set_unattended", { unattended, chatId });
}

/** Puts back what was chosen for this chat or folder, and says whether it is Auto. */
export async function restoreUnattended(chatId: string | null): Promise<boolean> {
  requireBackend();
  return invoke<boolean>("approval_restore", { chatId });
}

/** Where Ask/Auto is remembered: each chat its own, or one for the folder. */
export type RememberScope = "chat" | "repository";

export async function getRememberScope(): Promise<RememberScope> {
  requireBackend();
  return invoke<RememberScope>("approval_remember_get");
}

export async function setRememberScope(remember: RememberScope): Promise<void> {
  requireBackend();
  return invoke<void>("approval_remember_set", { remember });
}

/**
 * What the next request will cost, split into the parts that behave
 * differently: compacting shortens `conversation` and nothing else.
 *
 * Mirrors `domain::compaction::ContextUsage`. An estimate, and the one the
 * backend actually decides on — which is the point of asking for it rather
 * than counting characters here.
 */
export type ContextUsage = {
  /** The system prompt but the skills list: the mode, the project's rules, the plan. */
  instructions: number;
  /** The skills list, and every skill loaded since. */
  skills: number;
  /** The built-in tools' schemas the mode offers. */
  tools: number;
  /** The running MCP servers' schemas, and what their tools returned. */
  mcp: number;
  /** Everything else the two sides have said — the only part compacting shortens. */
  conversation: number;
  total: number;
  limit: number | null;
  /** The total at which a pass starts happening on its own. */
  compactsAt: number | null;
};

export async function contextUsage(messages: LlmMessage[], plan: string | null): Promise<ContextUsage> {
  requireBackend();
  return invoke<ContextUsage>("chat_context_usage", { messages, plan });
}

/** What the agent is allowed to be. Mirrors `domain::conversation_mode`. */
export type ConversationMode = "agent" | "plan" | "ask";

/**
 * Chooses the mode for the turns that follow.
 *
 * Told to the backend rather than sent with each turn, because a paused turn
 * is resumed from a checkpoint that predates the chip — and because the mode
 * is enforced there, not here: the window asking nicely for fewer tools would
 * be a rule the model never sees.
 */
export async function setConversationMode(mode: ConversationMode): Promise<void> {
  requireBackend();
  return invoke<void>("chat_set_mode", { mode });
}

/**
 * Subscribes to one turn's events.
 *
 * The channel is global, so events from another turn arrive here too and are
 * dropped by id. Without that filter two overlapping turns interleave into one
 * message — which is the reason the id is on the wire at all.
 */
export async function onTurnEvent(
  turnId: string,
  handler: (event: TurnEvent) => void,
): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<TurnEvent>(CHAT_TURN_EVENT, ({ payload }) => {
    if (payload.turnId === turnId) handler(payload);
  });
}

// ---------------------------------------------------- provider configuration

/** The wire protocol an endpoint speaks. Absent means OpenAI-compatible. */
export type ProviderKind = "openAiCompatible" | "anthropic";

export type ProviderConfig = {
  id: string;
  kind?: ProviderKind;
  baseUrl: string;
  /** Where the model list is asked for. Unset is `{baseUrl}/models`. */
  modelsUrl?: string | null;
  model?: string | null;
  trustedCertPem?: string | null;
  requestHeaders?: Record<string, string>;
  temperature?: number | null;
  /** Nucleus sampling. Unset sends nothing, leaving the model's default. */
  topP?: number | null;
  maxTokens?: number | null;
  /** The model's context window in tokens. Unset means the app does not know. */
  contextLimit?: number | null;
  reasoningEffort?: string | null;
};

/** A provider as the window sees it: the configuration, plus whether a key is stored. */
export type ProviderView = ProviderConfig & { hasApiKey: boolean };

/** Mirrors `domain::settings::ReplyLanguage`: what the model writes in; `auto` asks nothing. */
export type ReplyLanguage = "auto" | "english" | "russian";

/**
 * A `/` command's prompt with the reply language said at its end, as
 * `domain::prompt::with_language_reminder` does for the app's own notes: the
 * prompt is the app's words, usually English, and what the user typed — the
 * one thing in their language — is not what the model gets.
 */
export function withLanguageReminder(prompt: string, language: ReplyLanguage): string {
  if (language === "auto") return prompt;
  const name = language === "english" ? "English" : "Russian";
  return `${prompt}\n\n[Reply in ${name}.]`;
}

export type LlmSettings = {
  providers: ProviderView[];
  activeProviderId: string | null;
  debugLogging: boolean;
  replyLanguage: ReplyLanguage;
};

/** What a turn still needs before it can start. Asked before sending, not discovered by failing. */
export type Readiness = { workspace: string | null; provider: string | null; hasKey: boolean };

export async function llmSettings(): Promise<LlmSettings> {
  if (!inTauri()) return { providers: [], activeProviderId: null, debugLogging: false, replyLanguage: "auto" };
  return invoke<LlmSettings>("llm_settings_get");
}

export async function saveProvider(provider: ProviderConfig): Promise<void> {
  requireBackend();
  return invoke<void>("llm_provider_save", { provider });
}

export async function removeProvider(id: string): Promise<void> {
  requireBackend();
  return invoke<void>("llm_provider_remove", { id });
}

/**
 * Sends the key to be sealed. There is no command that reads one back — an
 * empty string deletes the stored one.
 */
export async function saveApiKey(id: string, key: string): Promise<void> {
  requireBackend();
  return invoke<void>("llm_api_key_save", { id, key });
}

export async function setActiveProvider(id: string | null): Promise<void> {
  requireBackend();
  return invoke<void>("llm_active_provider_set", { id });
}

export async function setDebugLogging(enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("llm_debug_logging_set", { enabled });
}

export async function setReplyLanguage(language: ReplyLanguage): Promise<void> {
  requireBackend();
  return invoke<void>("llm_reply_language_set", { language });
}

/** A live call, so it is also what proves the URL and the key are both right. */
export async function listModels(id?: string): Promise<string[]> {
  requireBackend();
  return invoke<string[]>("llm_models_list", { id: id ?? null });
}

/**
 * What `provider` serves, as the form has it now — saved or not. `apiKey`
 * `null` uses the stored key.
 */
export async function probeModels(provider: ProviderConfig, apiKey: string | null): Promise<string[]> {
  requireBackend();
  return invoke<string[]>("llm_models_probe", { provider, apiKey });
}

export async function readiness(): Promise<Readiness> {
  if (!inTauri()) return { workspace: null, provider: null, hasKey: false };
  return invoke<Readiness>("agent_readiness");
}

// ------------------------------------------------------------- previewing

export type FileDiffStats = {
  linesAdded: number;
  linesRemoved: number;
  unifiedDiff: string;
  truncated: boolean;
};

/** What a paused call would do, worked out without doing it. */
export type ToolPreview =
  | { kind: "diff"; path: string; diff: FileDiffStats }
  | { kind: "removes"; path: string; files: number }
  | { kind: "command"; command: string; cwd: string }
  | { kind: "failed"; reason: string }
  | { kind: "nothing" };

/** One round trip for the whole card: it shows every call the round asked for. */
export async function previewCalls(calls: PendingToolCall[]): Promise<ToolPreview[]> {
  if (!inTauri()) return calls.map(() => ({ kind: "nothing" }) as ToolPreview);
  return invoke<ToolPreview[]>("chat_preview", { calls });
}

// ------------------------------------------------------------ saved chats

/** A row in the sidebar. Not the whole conversation — see `chat_load`. */
export type ChatSummary = {
  id: string;
  title: string;
  updatedAt: number;
  /** The chat this one was branched from. A branch shares its title. */
  branchedFrom?: string | null;
  /** Filed away: shown under the sidebar's "Archived" filter, not in the list. */
  archived: boolean;
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

// ---------------------------------------------------------------- skills

/**
 * Mirrors `domain::skills::SkillListItem`: one skill folder, the open
 * folder's (`.claude/skills`, `.agents/skills`) or the user's (the app's,
 * `~/.agents/skills`, `~/.claude/skills`). `error` is set when its SKILL.md did
 * not parse. The switch is by name — two rows can share one — and `path` tells
 * rows apart. `shadowedBy` is the folder of the skill used instead of this one.
 */
export type SkillListItem = {
  name: string;
  description: string;
  enabled: boolean;
  error: string | null;
  source: "project" | "user";
  path: string;
  shadowedBy: string | null;
};

/**
 * Mirrors `domain::skills::SkillSourceItem`: a folder skills are read from —
 * `project` (the repository's, `path` its root, empty with no folder open),
 * `app`, `agents`, `claude` — and whether it is read at all.
 */
export type SkillSourceItem = { id: "project" | "app" | "agents" | "claude"; path: string; enabled: boolean };

/** `dir` is where the user's own skills go, so an empty list can say so. */
export type SkillsView = { dir: string; skills: SkillListItem[]; sources: SkillSourceItem[] };

export async function skillsList(): Promise<SkillsView> {
  if (!inTauri()) return { dir: "", skills: [], sources: [] };
  return invoke<SkillsView>("skills_list");
}

/** A command the user wrote in `.kibo/commands`. Mirrors `domain::slash_commands::CommandFile`. */
export type CommandFile = {
  name: string;
  description: string;
  argumentHint: string | null;
  /** The prompt; `$ARGUMENTS` stands for what is typed after the name. */
  template: string;
  source: "project" | "user";
};

/** The open folder's command files and the user's, the folder's first on a shared name. */
export async function commandFilesList(): Promise<CommandFile[]> {
  if (!inTauri()) return [];
  return invoke<CommandFile[]>("slash_commands_list");
}

/** A skills folder read or not, for every repository. */
export async function setSkillSourceEnabled(id: SkillSourceItem["id"], enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("skills_set_source_enabled", { id, enabled });
}

/** By name: off in every folder and every repository. */
export async function setSkillEnabled(name: string, enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("skills_set_enabled", { name, enabled });
}

// ---------------------------------------------------------------- MCP servers

/** Mirrors `domain::mcp::McpToolInfo`: one tool a running server offers. */
export type McpToolInfo = { name: string; description: string };

/** Mirrors `domain::mcp::McpServerState`: what the server's process is doing. */
export type McpServerState =
  | { state: "notStarted" }
  | { state: "starting" }
  | { state: "running"; tools: McpToolInfo[] }
  | { state: "exited"; error: string }
  | { state: "failed"; error: string };

/**
 * Mirrors `domain::mcp::McpServerItem`. `command` is the URL for an HTTP server. `error` is why it
 * will not start, when the entry alone says; `warning` is something wrong that does not stop it.
 */
export type McpServerItem = {
  name: string;
  command: string;
  enabled: boolean;
  error: string | null;
  warning: string | null;
  state: McpServerState;
};
/** The file (`text`, for the editor), where it is, and its servers as rows. */
export type McpView = { path: string; text: string; servers: McpServerItem[] };

export async function mcpConfig(): Promise<McpView> {
  if (!inTauri()) return { path: "", text: "", servers: [] };
  return invoke<McpView>("mcp_config_get");
}

/** Refused, and the file left alone, when the text is not a valid `mcpServers` config. */
export async function saveMcpConfig(text: string): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_config_save", { text });
}

/** Starts the server if it is not running, so the tab can show its tools without an Agent turn. */
export async function connectMcpServer(name: string): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_server_connect", { name });
}

export async function setMcpServerEnabled(name: string, enabled: boolean): Promise<McpView> {
  requireBackend();
  return invoke<McpView>("mcp_server_set_enabled", { name, enabled });
}

// ---------------------------------------------------------------- plain chat

/** Mirrors `domain::chat_role::ChatRole`: who the model is in Chat mode, and which tools it has. */
export type ChatRoleId = "assistant";
export type ChatRoleView = { id: ChatRoleId; name: string };

export async function plainChatRoles(): Promise<ChatRoleView[]> {
  if (!inTauri()) return [];
  return invoke<ChatRoleView[]>("plain_chat_roles");
}

/**
 * The model's reply to `messages`, outside any folder. Its text arrives on
 * `onTurnEvent(turnId)` as `delta` events while it is written; this resolves
 * with all of it — what was said so far, after a stop.
 */
export async function plainChatSend(
  turnId: string,
  role: ChatRoleId,
  messages: LlmMessage[],
): Promise<{ text: string; truncated?: boolean }> {
  requireBackend();
  return invoke("plain_chat_send", { turnId, role, messages });
}

/** Chat mode's conversations, newest first, whatever folder is open. They open with `loadChat`. */
export async function plainChatList(): Promise<ChatSummary[]> {
  if (!inTauri()) return [];
  return invoke<ChatSummary[]>("plain_chat_list");
}

export async function plainChatSave(id: string, role: ChatRoleId, messages: LlmMessage[]): Promise<ChatSummary> {
  requireBackend();
  return invoke<ChatSummary>("plain_chat_save", { id, role, messages });
}

export async function plainChatCancel(): Promise<void> {
  requireBackend();
  return invoke<void>("plain_chat_cancel");
}

// ---------------------------------------------------------------- review

/**
 * `/review`: a turn in Review mode over the working tree against HEAD. The
 * backend adds the change to `messages` as the turn's message, so the
 * outcome's history carries it; otherwise it is `startChat`.
 */
export async function reviewStart(
  turnId: string,
  messages: LlmMessage[],
  todos: Task[] = [],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("review_start", { turnId, messages, todos, plan });
}

// ---------------------------------------------------------------- rewind

/** Mirrors `domain::rewind::FileState`: a file's content at one moment, by hash. */
export type FileState = { kind: "absent" } | { kind: "stored"; hash: string } | { kind: "unstored" };
/** Mirrors `domain::rewind::FileChange`: what one call did to one file. */
export type FileChange = { path: string; before: FileState; after: FileState };
/** Why a file is left as it is — `domain::rewind::Skip`. */
export type RewindSkip = "notKept" | "expired" | "changedBetween" | "changedSince" | "unsafePath" | "writeFailed";
/** One file's part of a rewind — `domain::rewind::FileRewind`. */
export type FileRewind = {
  path: string;
  action: "created" | "modified" | "deleted";
  expected: FileState;
  target: FileState;
  skip: RewindSkip | null;
};

/** What putting `changes` back would do, file by file. Touches nothing. */
export async function rewindPreview(changes: FileChange[]): Promise<FileRewind[]> {
  requireBackend();
  return invoke<FileRewind[]>("rewind_preview", { changes });
}

/** Puts them back, each file checked again right before it is written. */
export async function rewindApply(changes: FileChange[]): Promise<FileRewind[]> {
  requireBackend();
  return invoke<FileRewind[]>("rewind_apply", { changes });
}

// ---------------------------------------------------------------- background processes

/** Mirrors `domain::background::ProcessState`. */
export type ProcessState = { state: "running" } | { state: "exited"; code: number | null } | { state: "stopped" };
/** Mirrors `domain::background::ProcessInfo`. */
export type ProcessInfo = { id: number; command: string; cwd: string; state: ProcessState };
/** A row of the Terminal tab: the process and the end of what it wrote. */
export type ProcessView = ProcessInfo & { tail: string };

/**
 * A background process started, wrote, ended or was stopped. Pinned on the
 * Rust side in `commands/processes.rs`.
 */
export const PROCESS_EVENT = "processes:changed";

export async function onProcessChanged(handler: (id: number) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ id: number }>(PROCESS_EVENT, ({ payload }) => handler(payload.id));
}

export async function processesList(): Promise<ProcessView[]> {
  if (!inTauri()) return [];
  return invoke<ProcessView[]>("processes_list");
}

/** The running ones, newest first, without output — cheap to read on every change. */
export async function processesRunning(): Promise<ProcessInfo[]> {
  if (!inTauri()) return [];
  return invoke<ProcessInfo[]>("processes_running");
}

/** The model is told at its next round that the user stopped it. */
export async function stopProcess(id: number): Promise<ProcessView[]> {
  requireBackend();
  return invoke<ProcessView[]>("process_stop", { id });
}

/** How a process stands, in a few words: `running`, `exit 1`, `stopped`. */
export function processStatus(state: ProcessState): string {
  switch (state.state) {
    case "running":
      return "running";
    case "stopped":
      return "stopped";
    case "exited":
      return state.code === null ? "killed by a signal" : `exit ${state.code}`;
  }
}

// ---------------------------------------------------------------- helper agents

/** Mirrors `domain::agents::AgentState`. */
export type AgentState = { state: "running" } | { state: "done" } | { state: "failed"; reason: string } | { state: "stopped" };
/** Mirrors `domain::agents::AgentTokens`: what the provider reported, summed over the run. */
export type AgentTokens = { prompt: number; cached: number; completion: number };
/** Mirrors `domain::agents::AgentInfo`: one `explore` run, as the Agents tab shows it. */
export type AgentInfo = {
  id: number;
  task: string;
  state: AgentState;
  steps: string[];
  tokens: AgentTokens;
  answer: string | null;
};

/**
 * A helper agent started, took a step, spent tokens or ended. Pinned on the
 * Rust side in `commands/agents.rs`.
 */
export const AGENT_EVENT = "agents:changed";

export async function onAgentChanged(handler: (id: number) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ id: number }>(AGENT_EVENT, ({ payload }) => handler(payload.id));
}

/** Newest first. */
export async function agentsList(): Promise<AgentInfo[]> {
  if (!inTauri()) return [];
  return invoke<AgentInfo[]>("agents_list");
}

/** The helper ends at its next check; the turn that started it is told and carries on. */
export async function stopAgent(id: number): Promise<AgentInfo[]> {
  requireBackend();
  return invoke<AgentInfo[]>("agent_stop", { id });
}

/** What a run cost: `38k tokens · 90% cached`; nothing while nothing is reported. */
export function tokensSpent(tokens: AgentTokens): string {
  const total = tokens.prompt + tokens.completion;
  if (!total) return "";
  const shown = total >= 1000 ? `${Math.round(total / 1000)}k` : `${total}`;
  const cached = tokens.prompt ? Math.round((tokens.cached / tokens.prompt) * 100) : 0;
  return cached ? `${shown} tokens · ${cached}% cached` : `${shown} tokens`;
}

/** How a run stands, in a word or two. */
export function agentStatus(state: AgentState): string {
  return state.state === "failed" ? "failed" : state.state;
}

// ---------------------------------------------------------------- git changes

/** Mirrors `domain::git_changes::ChangedFile`; `path` is relative to the repository. */
export type ChangedFile = { path: string; add: number; del: number };
export type WorkingChanges = { staged: ChangedFile[]; unstaged: ChangedFile[] };

/**
 * The open folder's repository changed outside the tree — a `git add` or a
 * commit made elsewhere. Pinned on the Rust side in `commands/git.rs`.
 */
export const GIT_EVENT = "workspace-git:changed";

export async function onGitChanged(handler: (root: string) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<{ root: string }>(GIT_EVENT, ({ payload }) => handler(payload.root));
}

/** Mirrors `domain::git_changes::ChangeTotals`: the open folder against the last commit. */
export type ChangeTotals = { files: number; add: number; del: number };

/** `null` outside a repository, and outside the desktop app. */
export async function gitTotals(): Promise<ChangeTotals | null> {
  if (!inTauri()) return null;
  return invoke<ChangeTotals | null>("git_totals");
}

export async function gitChanges(): Promise<WorkingChanges> {
  requireBackend();
  return invoke<WorkingChanges>("git_changes");
}

export async function gitStage(paths: string[]): Promise<void> {
  requireBackend();
  return invoke("git_stage", { paths });
}

export async function gitUnstage(paths: string[]): Promise<void> {
  requireBackend();
  return invoke("git_unstage", { paths });
}

/** Commits what is staged; resolves to the new commit's short id. */
export async function gitCommit(message: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_commit", { message });
}

/** A message for what is staged, written by the model; `draft` is what the box already holds. */
export async function gitCommitMessage(draft: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_commit_message", { draft });
}

/** Mirrors `domain::git_branches::BranchRef`: `origin/x` is remote, listed only with no local `x`. */
export type BranchRef = { name: string; remote: boolean };
/** Mirrors `domain::git_branches::CheckoutOutcome`: a conflict left the folder exactly as it was. */
export type CheckoutOutcome = { kind: "switched" } | { kind: "conflicts"; paths: string[] };

/** Local branches first, then remote ones. */
export async function gitBranches(): Promise<BranchRef[]> {
  requireBackend();
  return invoke<BranchRef[]>("git_branches");
}

/** Switches the open folder, or names the files that stand in the way — never overwrites them. */
export async function gitCheckout(branch: string): Promise<CheckoutOutcome> {
  requireBackend();
  return invoke<CheckoutOutcome>("git_checkout", { branch });
}

/** A new worktree on a new branch from `base`; resolves to its folder, not yet opened. */
export async function gitWorktreeAdd(base: string): Promise<string> {
  requireBackend();
  return invoke<string>("git_worktree_add", { base });
}

/** Mirrors `commands::git::WorktreeCheck`: a worktree as removing it would find it. */
export type WorktreeCheck = {
  /** The main folder it was made from. */
  main: string;
  branch: string | null;
  /** Uncommitted changes and untracked files: removal is refused while there are any. */
  dirty: string[];
  /** Commits only this worktree's branch has. */
  ownCommits: number;
  keepsBranch: boolean;
  chats: number;
};
/** Mirrors `commands::git::WorktreeRemoved`. */
export type WorktreeRemoved = { branchKept: string | null; chatsRemoved: number };

/** `path` is a worktree from the folder list, not the open folder. */
export async function gitWorktreeCheck(path: string): Promise<WorktreeCheck> {
  requireBackend();
  return invoke<WorktreeCheck>("git_worktree_check", { path });
}

/** Removes the worktree at `path` — from the folder list, not the open folder — and its chats. */
export async function gitWorktreeRemove(path: string): Promise<WorktreeRemoved> {
  requireBackend();
  return invoke<WorktreeRemoved>("git_worktree_remove", { path });
}

/** Mirrors `domain::git_changes::CommitSummary`; `time` is in seconds since the epoch. */
export type CommitSummary = { id: string; summary: string; author: string; time: number; head: boolean; refs: string[] };
/** Mirrors `domain::git_changes::GitHistory`. */
export type GitHistory = {
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  commits: CommitSummary[];
  more: boolean;
};

/** Where HEAD is and the newest `limit` commits from it, newest first. */
export async function gitHistory(limit: number): Promise<GitHistory> {
  requireBackend();
  return invoke<GitHistory>("git_history", { limit });
}

/**
 * Mirrors `domain::git_changes::FileSide`: which two versions the viewer
 * compares. `unstaged` and `staged` take the repository's paths, as Changes
 * lists them; `worktree` takes the open folder's, as Files does.
 */
export type FileSide = "unstaged" | "staged" | "worktree";
/** A file for the viewer to show, and against what. */
export type FileTarget = { path: string; side: FileSide };
/** Mirrors `domain::git_changes::FileView`: `null` on a side the file is not on. */
export type FileView = { old: string | null; new: string | null; unviewable: "binary" | "tooLarge" | null };

export async function fileView(target: FileTarget): Promise<FileView> {
  requireBackend();
  return invoke<FileView>("file_view", target);
}

// ---------------------------------------------------------------- file tree

/** Mirrors `domain::file_tree`. A deleted file is not listed: it is not on disk. */
export type FileStatus = "modified" | "added" | "untracked" | "conflicted";
export type TreeEntry = { name: string; path: string; isDir: boolean; status: FileStatus | null; changed: boolean };
export type FolderListing = { entries: TreeEntry[]; more: number };

/** One folder of the open one; `""` is the open folder itself. */
export async function workspaceList(dir: string): Promise<FolderListing> {
  requireBackend();
  return invoke<FolderListing>("workspace_list", { dir });
}

// ---------------------------------------------------------------- hooks

/** Mirrors `domain::hooks::HookItem`. `problem` is why it will not run as written. */
export type HookItem = {
  event: string;
  matcher: string;
  command: string;
  timeoutSecs: number;
  problem: string | null;
};
export type HooksView = { path: string; text: string; hooks: HookItem[] };

export async function hooksConfig(): Promise<HooksView> {
  if (!inTauri()) return { path: "", text: "", hooks: [] };
  return invoke<HooksView>("hooks_config_get");
}

/** Refused, and the file left alone, when the text is not a valid hooks config. */
export async function saveHooksConfig(text: string): Promise<HooksView> {
  requireBackend();
  return invoke<HooksView>("hooks_config_save", { text });
}

// ---------------------------------------------------------------- project rules

/** An instruction file at the open folder's root. `error` when it cannot be sent; then it has no switch. */
export type RuleListItem = {
  name: string;
  path: string;
  enabled: boolean;
  content: string;
  truncated: boolean;
  error: string | null;
};

export async function rulesList(): Promise<RuleListItem[]> {
  if (!inTauri()) return [];
  return invoke<RuleListItem[]>("rules_list");
}

export async function setRuleEnabled(path: string, enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("rules_set_enabled", { path, enabled });
}

// ---------------------------------------------------------------- tool-call log

export type CallStatus = "ok" | "error" | "denied";

/** One settled call, as stored: arguments and result have their file content replaced by `<redacted>`. */
export type ToolLogRow = {
  id: number;
  tsMs: number;
  repoRoot: string;
  round: number;
  providerId: string;
  model: string;
  tool: string;
  /** `null` when the model's arguments did not parse. */
  args: { tool?: string; args?: Record<string, unknown> } | null;
  status: CallStatus;
  error: string | null;
  result: unknown;
  durationMs: number;
};

export type ToolLogFilter = {
  tool?: string;
  status?: CallStatus;
  search?: string;
  limit?: number;
  offset?: number;
};

export type ToolLogPage = { rows: ToolLogRow[]; total: number };

export async function toolLogQuery(filter: ToolLogFilter): Promise<ToolLogPage> {
  if (!inTauri()) return { rows: [], total: 0 };
  return invoke<ToolLogPage>("tool_log_query", { filter });
}

export async function toolLogClear(): Promise<number> {
  requireBackend();
  return invoke<number>("tool_log_clear");
}

export async function toolLogEnabled(): Promise<boolean> {
  if (!inTauri()) return true;
  return invoke<boolean>("tool_log_enabled_get");
}

export async function setToolLogEnabled(enabled: boolean): Promise<void> {
  requireBackend();
  return invoke<void>("tool_log_enabled_set", { enabled });
}
