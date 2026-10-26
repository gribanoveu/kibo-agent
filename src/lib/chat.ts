import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { inTauri, requireBackend } from "./backend";
import type { ImagePart } from "./images";
import type { McpAnswer, McpQuestion } from "./mcp";
import type { ProcessInfo } from "./processes";

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
  /** Pictures the user attached, sanitized by `prepareImage`. Only on a user message. */
  images?: ImagePart[];
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
  /** Rounds run, when the turn stopped at the turn limits instead of finishing. */
  limitReached?: number | null;
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
  | { type: "mcpQuestion"; payload: { id: string; call: string; server: string; question: McpQuestion } }
  | { type: "mcpQuestionClosed"; payload: { id: string; action: McpAnswer["action"] } }
  | { type: "contextUsage"; payload: ChatUsage }
  | { type: "contextEstimate"; payload: ContextUsage }
);

/**
 * The other half of a contract whose first half is `CHAT_TURN_EVENT` in
 * `src-tauri/src/commands/chat_events.rs`; `architecture.rs` checks the two
 * match — a mismatch would show as a window where nothing ever happens.
 */
export const CHAT_TURN_EVENT = "chat:turn-event";

// ------------------------------------------------------------- the commands

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

/** Drops a folder from the list offered at launch; the folder itself stays. */
export async function forgetWorkspace(path: string): Promise<void> {
  requireBackend();
  return invoke<void>("workspace_forget", { path });
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

/** Starts a turn. `messages` is the whole conversation: the transcript is the caller's.
    What it spends is counted under `chatId` (`chatTokens`). */
export async function startChat(
  turnId: string,
  chatId: string,
  messages: LlmMessage[],
  todos: Task[] = [],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("chat_start", { turnId, chatId, messages, todos, plan });
}

/**
 * Shortens the conversation when it is worth shortening, and returns the
 * shorter one. `null` means "leave yours alone" — the decision is the
 * backend's, including how much to fold. Once a summary is being made, a
 * `historyCompacting` event says so under `turnId` (`onTurnEvent`).
 */
export async function compactHistory(
  messages: LlmMessage[],
  force: boolean,
  turnId: string,
  chatId: string,
): Promise<{ history: LlmMessage[]; folded: number } | null> {
  if (!inTauri()) return null;
  return invoke<{ history: LlmMessage[]; folded: number } | null>("chat_compact", {
    messages,
    force,
    turnId,
    chatId,
  });
}

/** Continues a paused turn. `checkpoint` goes back exactly as it arrived. */
export async function resumeChat(
  turnId: string,
  chatId: string,
  checkpoint: Checkpoint,
  decisions: ToolCallDecision[],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("chat_resume", { turnId, chatId, checkpoint, decisions, plan });
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
  /** The system prompt but the skills list: the mode, the project's rules. */
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

export async function contextUsage(messages: LlmMessage[]): Promise<ContextUsage> {
  requireBackend();
  return invoke<ContextUsage>("chat_context_usage", { messages });
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

/** What a tool call came back with, in any turn. */
export type ToolResultEvent = Extract<TurnEvent, { type: "toolResult" }>["payload"];

/**
 * Subscribes to every turn's tool results — for a view of what a turn
 * changes that knows no turn by id: one is running under the page, in a chat
 * left mounted. The other events on the channel are dropped here.
 */
export async function onAnyToolResult(handler: (result: ToolResultEvent) => void): Promise<UnlistenFn> {
  if (!inTauri()) return () => {};
  return listen<TurnEvent>(CHAT_TURN_EVENT, ({ payload }) => {
    if (payload.type === "toolResult") handler(payload.payload);
  });
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
  /** A change to a cluster: where, what becomes of what, and what to know first. */
  | { kind: "change"; place: string; summary: string; notes: string[]; production?: boolean; diffs?: { title: string; diff: FileDiffStats }[] }
  | { kind: "failed"; reason: string }
  | { kind: "nothing" };

/** One round trip for the whole card: it shows every call the round asked for. */
export async function previewCalls(calls: PendingToolCall[]): Promise<ToolPreview[]> {
  if (!inTauri()) return calls.map(() => ({ kind: "nothing" }) as ToolPreview);
  return invoke<ToolPreview[]>("chat_preview", { calls });
}

// ---------------------------------------------------------------- review

/**
 * `/review`: a turn in Review mode over the working tree against HEAD. The
 * backend adds the change to `messages` as the turn's message, so the
 * outcome's history carries it; otherwise it is `startChat`.
 */
export async function reviewStart(
  turnId: string,
  chatId: string,
  messages: LlmMessage[],
  todos: Task[] = [],
  plan: string | null = null,
): Promise<Outcome> {
  requireBackend();
  return invoke<Outcome>("review_start", { turnId, chatId, messages, todos, plan });
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
export async function rewindPreview(changes: FileChange[], chatId?: string): Promise<FileRewind[]> {
  requireBackend();
  return invoke<FileRewind[]>("rewind_preview", { changes, chatId });
}

/**
 * The message standing for the part of a chat being rewound away from —
 * `messages`, from the rewound one on — told as what was tried. `null` when
 * the summary came back empty. One request to the model.
 */
export async function branchSummary(messages: LlmMessage[], chatId: string): Promise<LlmMessage | null> {
  requireBackend();
  return invoke<LlmMessage | null>("chat_branch_summary", { messages, chatId });
}

/**
 * Puts them back, each file checked again right before it is written: in the
 * agent's open folder, or — `chatId` given — in that Chat-mode chat's own.
 */
export async function rewindApply(changes: FileChange[], chatId?: string): Promise<FileRewind[]> {
  requireBackend();
  return invoke<FileRewind[]>("rewind_apply", { changes, chatId });
}
