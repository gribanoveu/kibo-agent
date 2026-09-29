import { useCallback, useEffect, useRef, useState } from "react";
import {
  deleteChat,
  loadChat,
  onTurnEvent,
  plainChatAlwaysAllow,
  plainChatCancel,
  plainChatList,
  plainChatResume,
  plainChatRoles,
  plainChatSave,
  plainChatSend,
  setChatArchived,
  type ChatRoleId,
  type ChatRoleView,
  type ChatSummary,
  type LlmMessage,
  type Outcome,
  type ToolCallDecision,
} from "../lib/chat";
import {
  acceptEvent,
  acceptOutcome,
  appendNotice,
  appendUserMessage,
  clearApproval,
  emptyTurn,
  endTurn,
  restoredTurn,
  type Block,
  type TurnState,
} from "../lib/chatTurnReducer";
import type { KubePin } from "../lib/kube";
import { useStoredState } from "./useStoredState";

const isId = (value: unknown): value is string | null => value === null || typeof value === "string";

/** A bubble as Chat mode saved it before its transcript was the agent's blocks. */
type Bubble = { role: "user" | "assistant"; text: string; reasoning?: string };

const isBubbles = (value: unknown): value is Bubble[] =>
  Array.isArray(value) &&
  value.every((m) => (m?.role === "user" || m?.role === "assistant") && typeof m?.text === "string");

const isBlocks = (value: unknown): value is Block[] =>
  Array.isArray(value) && value.every((b) => typeof b?.kind === "string" && typeof b?.id === "string");

/**
 * A saved chat's transcript as blocks. Chats from before the agent's blocks
 * were kept as bubbles, and the first ones only as what the model was sent;
 * both are turned into blocks with the ids the reducer itself would give them,
 * so a turn added after them never lands on one.
 */
export function chatBlocks(saved: unknown, messages: LlmMessage[]): Block[] {
  if (isBlocks(saved) && saved.length > 0) return saved;
  const bubbles: Bubble[] =
    isBubbles(saved) && saved.length > 0
    ? saved
    : messages.flatMap((m) =>
        (m.role === "user" || m.role === "assistant") && m.content ? [{ role: m.role, text: m.content }] : [],
      );
  const blocks: Block[] = [];
  let turns = 0;
  for (const bubble of bubbles) {
    if (bubble.role === "user") {
      blocks.push({ kind: "user", id: `user:${blocks.length}`, text: bubble.text });
      turns += 1;
      continue;
    }
    if (bubble.reasoning) blocks.push({ kind: "reasoning", id: `turn:${turns}:round:1:reasoning`, round: 1, text: bubble.reasoning });
    blocks.push({ kind: "message", id: `turn:${turns}:round:1:message`, round: 1, text: bubble.text });
  }
  return blocks;
}

/**
 * Chat mode: its conversations, stored by the backend, and the one open. A
 * turn is the agent's loop in the chat's role — its answer, its calls and its
 * approval cards arrive as the agent's do and are drawn the same way. As with
 * the agent, the window keeps both halves: the blocks a reader sees and the
 * messages the model is sent again.
 */
export function usePlainChat(lastKubeconfig: string | null = null) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  // Which chat is open, so the next launch opens it again. Null until the first message is saved.
  const [chatId, setChatId] = useStoredState<string | null>("plain-chat-open", null, isId);
  const [turn, setTurn] = useState<TurnState>(emptyTurn);
  const history = useRef<LlmMessage[]>([]);
  const [roles, setRoles] = useState<ChatRoleView[]>([]);
  const [role, setRole] = useState<ChatRoleId>("assistant");
  // The cluster this chat is pinned to; until it is pinned, the kubeconfig
  // picked last, at its current context and that context's namespace.
  const [pinned, setPinned] = useState<KubePin | null>(null);
  const kube: KubePin | null =
    pinned ?? (lastKubeconfig ? { kubeconfig: lastKubeconfig, context: null, namespace: null } : null);
  const [error, setError] = useState<string | null>(null);
  // The running turn's id on the one event channel, and its listener.
  const turnId = useRef("");
  const subscribed = useRef<(() => void) | null>(null);
  // Set when something happened worth writing down, so opening a chat does
  // not save it straight back and move it up the sidebar.
  const unsaved = useRef(false);

  // A turn under way belongs to the chat on screen: its answer, its card and
  // its save land here. Leaving waits for it to stop, as the agent's does.
  const busy = turn.status === "running" || turn.status === "awaitingApproval";

  const refresh = useCallback(() => plainChatList().then(setChats, (e) => setError(String(e))), []);

  useEffect(() => () => subscribed.current?.(), []);

  const show = (id: string | null, blocks: Block[], messages: LlmMessage[], shownRole: ChatRoleId, pin: KubePin | null) => {
    subscribed.current?.();
    subscribed.current = null;
    history.current = messages;
    unsaved.current = false;
    setChatId(id);
    setTurn(restoredTurn(blocks));
    setRole(shownRole);
    setPinned(pin);
    setError(null);
  };

  /** Opens a saved chat; `false` while a turn is under way here. */
  const open = async (id: string) => {
    if (busy) return false;
    try {
      const record = await loadChat(id);
      show(id, chatBlocks(record.blocks, record.messages), record.messages, record.role ?? "assistant", record.kube ?? null);
    } catch (e) {
      // A chat deleted since it was left open is simply not reopened.
      show(null, [], [], "assistant", null);
      if (id !== chatId) setError(String(e));
    }
    return true;
  };

  useEffect(() => {
    plainChatRoles().then(setRoles, () => {});
    void refresh();
    if (chatId) void open(chatId);
  }, []);

  /** Starts over, keeping the role and the cluster; `false` while a turn is under way. */
  const newChat = () => {
    if (busy) return false;
    show(null, [], [], role, pinned);
    return true;
  };

  /** Pins the chat to a cluster — kept with a chat already saved, at once. */
  const setPin = (next: KubePin) => {
    setPinned(next);
    if (chatId === null || busy || turn.blocks.length === 0) return;
    plainChatSave(chatId, role, next, history.current, turn.blocks).catch((e) => setError(String(e)));
  };

  // Saved once the turn has come to rest, from the render that has its last
  // block in it — the final events are applied after the outcome arrives.
  useEffect(() => {
    if (!unsaved.current || chatId === null) return;
    if (turn.status !== "done" && turn.status !== "cancelled") return;
    unsaved.current = false;
    plainChatSave(chatId, role, kube, history.current, turn.blocks)
      .then(() => refresh())
      .catch((e) => setError(String(e)));
  }, [turn.status, turn.blocks, chatId, role, kube, refresh]);

  const finish = (outcome: Outcome) => {
    setTurn((state) => acceptOutcome(state, outcome));
    if (outcome.status === "pendingApproval") return;
    // The turn's own history, calls and results included: the next message
    // is sent with what the model read, not only what it answered.
    history.current = outcome.value.history;
    subscribed.current?.();
    subscribed.current = null;
  };

  // In the transcript, where the turn was: one that fails before its first
  // event otherwise ends with nothing on screen to say why.
  const failed = (e: unknown) => setTurn((state) => appendNotice(endTurn(state), `The turn failed: ${e}`));

  const send = async (text: string) => {
    const trimmed = text.trim();
    if (busy || !trimmed) return;
    const id = chatId ?? crypto.randomUUID();
    const before = turn;
    const asked = appendUserMessage(before, trimmed);
    history.current = [...history.current, { role: "user", content: trimmed }];
    unsaved.current = true;
    setChatId(id);
    setError(null);
    setTurn(asked);
    // Pinned from its first message: the default it started with is its own now.
    setPinned(kube);
    turnId.current = crypto.randomUUID();
    try {
      subscribed.current?.();
      subscribed.current = await onTurnEvent(turnId.current, (event) => setTurn((state) => acceptEvent(state, event)));
      // Saved before the turn: one that fails leaves what the user said to
      // retry, not retype — and the chat is in the sidebar from its first message.
      await plainChatSave(id, role, kube, history.current, asked.blocks);
      void refresh();
      finish(await plainChatSend(turnId.current, role, kube, history.current));
    } catch (e) {
      failed(e);
    }
  };

  /** Answers the approval card. `always` stops those tools asking in Chat mode first. */
  const decide = async (decisions: ToolCallDecision[], always: string[] = []) => {
    const checkpoint = turn.checkpoint;
    if (!checkpoint) return;
    setTurn((state) => clearApproval(state));
    try {
      for (const tool of always) await plainChatAlwaysAllow(tool);
      finish(await plainChatResume(turnId.current, role, kube, checkpoint, decisions));
    } catch (e) {
      failed(e);
    }
  };

  const stop = () => void plainChatCancel().catch(() => {});

  /** `false` for the open chat while a turn is under way: its next save would write it back. */
  const remove = async (id: string) => {
    if (id === chatId && !newChat()) return false;
    try {
      await deleteChat(id);
    } catch (e) {
      setError(String(e));
    }
    void refresh();
    return true;
  };

  const archive = async (id: string, archived: boolean) => {
    try {
      await setChatArchived(id, archived);
    } catch (e) {
      setError(String(e));
    }
    void refresh();
  };

  return { chats, chatId, turn, busy, roles, role, setRole, kube, setPin, error, send, decide, stop, open, newChat, remove, archive };
}

export type PlainChatState = ReturnType<typeof usePlainChat>;
