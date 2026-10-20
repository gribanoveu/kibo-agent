import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { branchAt, branchPoints } from "../lib/branch";
import { branchSummary, rewindApply, rewindPreview, type ContextUsage, type FileRewind, type LlmMessage, type Outcome, type PendingToolCall, type ToolCallDecision } from "../lib/chat";
import type { Attached } from "./useAttachments";
import { type ChatRoleId, type ChatRoleView, plainChatAlwaysAllow, plainChatCancel, plainChatCompact, plainChatContextUsage, plainChatList, plainChatPreview, plainChatResume, plainChatRoles, plainChatSave, plainChatSend } from "../lib/plainChat";
import { type ChatSummary, deleteChat, generateChatTitle, loadChat, setChatArchived } from "../lib/savedChats";
import {
  acceptOutcome,
  appendNotice,
  appendRestored,
  appendUserMessage,
  clearApproval,
  emptyTurn,
  endTurn,
  restoredTurn,
  type Block,
  type TurnState,
} from "../lib/chatTurnReducer";
import type { KubePin } from "../lib/kube";
import { addPrototypeAsset, assetsNote, pointNote, restoreNote, restorePrototype, type Asset, type Pointed } from "../lib/prototype";
import { changesFrom, rewoundNotice } from "../lib/rewind";
import { useStoredState } from "./useStoredState";
import { useTurnChannel } from "./useTurnChannel";

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
 *
 * `model` is the one turns run on — `acceptEvent` says when a cache miss came with a switch.
 */
export function usePlainChat(lastKubeconfig: string | null = null, model?: string) {
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
  const [context, setContext] = useState<ContextUsage | null>(null);
  const [compacting, setCompacting] = useState(false);
  // The running turn's id on the one event channel, and its listener.
  const turnId = useRef("");
  const channel = useTurnChannel(setTurn, model);
  // Set when something happened worth writing down, so opening a chat does
  // not save it straight back and move it up the sidebar.
  const unsaved = useRef(false);

  // A turn under way belongs to the chat on screen: its answer, its card and
  // its save land here. Leaving waits for it to stop, as the agent's does.
  // A summary being made counts too: a message sent under it would be folded away.
  const busy = turn.status === "running" || turn.status === "awaitingApproval" || compacting;

  const refresh = useCallback(() => plainChatList().then(setChats, (e) => setError(String(e))), []);


  // As the agent's meter: every point the history, the role or the cluster can
  // have changed. Asking reaches the pinned cluster, so an answer that comes
  // back after a newer question is dropped.
  const asked = useRef(0);
  const refreshContext = useCallback(() => {
    const seq = ++asked.current;
    plainChatContextUsage(role, kube, history.current)
      .then((usage) => seq === asked.current && setContext(usage))
      .catch(() => seq === asked.current && setContext(null));
  }, [role, kube?.kubeconfig, kube?.context, kube?.namespace]);
  useEffect(refreshContext, [refreshContext, turn.status, chatId]);

  const show = (id: string | null, blocks: Block[], messages: LlmMessage[], shownRole: ChatRoleId, pin: KubePin | null) => {
    channel.stop();
    history.current = messages;
    unsaved.current = false;
    setChatId(id);
    setTurn(restoredTurn(blocks));
    setRole(shownRole);
    // "Changes" is for the chat as it is open now: a chat reopened, or a new
    // one started from it, reads only until the switch is turned again.
    setPinned(pin && { kubeconfig: pin.kubeconfig, context: pin.context, namespace: pin.namespace });
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
    plainChatRoles().then(setRoles, (e) => setError(String(e)));
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
    channel.stop();
  };

  // In the transcript, where the turn was: one that fails before its first
  // event otherwise ends with nothing on screen to say why.
  const failed = (e: unknown) => setTurn((state) => appendNotice(endTurn(state), `The turn failed: ${e}`));

  /**
   * Sends a message. `sees` is whether the model is set to see pictures: in a
   * Prototype chat the pictures go into its folder's `assets/` first, named
   * by their files, and the model is told where — sent to it as pictures
   * only when it can see them.
   */
  const send = async (text: string, attached: Attached[] = [], pointed: Pointed | null = null, sees = true) => {
    const trimmed = text.trim();
    const images = attached.map((one) => one.image);
    if (busy || (!trimmed && images.length === 0)) return;
    const id = chatId ?? crypto.randomUUID();
    const before = turn;
    // Before the chat has its id: the Preview panel reads the folder when it does.
    const kept: Asset[] = [];
    if (role === "prototype") {
      try {
        for (const one of attached) kept.push(await addPrototypeAsset(id, one.name, one.image));
      } catch (e) {
        setError(`The pictures could not be put in the chat's folder: ${e}`);
        return;
      }
    }
    // The element pointed at, a restore since the last message and the
    // pictures kept: the model is told with this one, and the transcript
    // shows what was typed.
    const notes = [pointed && pointNote(pointed), restoreNote(before.blocks), assetsNote(kept)].filter((note): note is string => !!note);
    const sent = notes.length > 0 ? [trimmed, ...notes].filter(Boolean).join("\n\n") : undefined;
    const asked = appendUserMessage(before, trimmed, Date.now(), sent, images, pointed?.selector);
    const seen = role === "prototype" && !sees ? [] : images;
    history.current = [...history.current, { role: "user", content: sent ?? trimmed, ...(seen.length > 0 && { images: seen }) }];
    unsaved.current = true;
    setChatId(id);
    setError(null);
    setTurn(asked);
    // Pinned from its first message: the default it started with is its own now.
    setPinned(kube);
    turnId.current = crypto.randomUUID();
    try {
      await channel.listen(turnId.current);
      // Saved before the turn: one that fails leaves what the user said to
      // retry, not retype — and the chat is in the sidebar from its first message.
      await plainChatSave(id, role, kube, history.current, asked.blocks);
      void refresh();
      // Beside the turn; a chat the model could not name keeps its first message as its name.
      if (chatId === null) generateChatTitle(id).then(refresh, () => {});
      finish(await plainChatSend(turnId.current, id, role, kube, history.current));
    } catch (e) {
      failed(e);
    }
  };

  /** Answers the approval card. `always` stops those tools asking in Chat mode first. */
  const decide = async (decisions: ToolCallDecision[], always: string[] = []) => {
    const checkpoint = turn.checkpoint;
    if (!checkpoint || chatId === null) return;
    setTurn((state) => clearApproval(state));
    try {
      for (const tool of always) await plainChatAlwaysAllow(tool);
      finish(await plainChatResume(turnId.current, chatId, role, kube, checkpoint, decisions));
    } catch (e) {
      failed(e);
    }
  };

  const stop = () => void plainChatCancel().catch(() => {});

  /**
   * Folds the older part of the conversation when the backend says it is
   * worth it — before each message, as the agent's window does — or at once
   * (`force`, "Compact now"). `false` when nothing was folded.
   */
  const makeRoom = async (force: boolean) => {
    setCompacting(true);
    // On the turn channel, as the agent's pass: the backend says when a summary is being made.
    const id = `compact-${crypto.randomUUID()}`;
    const ended = await channel.watchCompaction(id);
    const end = (result: { folded: number } | null) => {
      ended(result);
      setCompacting(false);
    };
    try {
      const shorter = await plainChatCompact(role, kube, history.current, force, id);
      end(shorter && { folded: shorter.folded });
      if (!shorter) return false;
      history.current = shorter.history;
      unsaved.current = true;
      refreshContext();
      return true;
    } catch (e) {
      end(null);
      setError(String(e));
      return false;
    }
  };

  // Bubbles a rewind can start at; `null` while a turn is under way. A chat
  // has no branches of its own — this is the rewind's, as the agent's is.
  const branchable = useMemo(
    () => (turn.status === "done" || turn.status === "cancelled" ? new Set(branchPoints(turn.blocks, history.current).keys()) : null),
    [turn.status, turn.blocks],
  );
  // What the message box is handed — a rewound message to change and send again.
  const [draft, setDraft] = useState<{ text: string; seq: number } | null>(null);

  /** What rewinding to before `bubbleId` would do to files, as the agent's `previewRewind`. */
  const previewRewind = async (bubbleId: string) => {
    if (!branchable?.has(bubbleId) || chatId === null) return null;
    const { changes, unrecorded } = changesFrom(turn.blocks, bubbleId);
    return { files: changes.length ? await rewindPreview(changes, chatId) : [], unrecorded };
  };

  /**
   * Takes the chat back to just before `bubbleId`, as the agent's `rewind`
   * does (docs/31-prototype-role.md, P-7): the folder's files put back where
   * nobody changed them since, the messages from there on gone, and that one
   * handed back to the message box. Files first — a rewind that could not
   * reach them leaves the conversation as it was. The versions on disk stay:
   * they are history, and only leave the panel's list with the messages that
   * made them. `null` when nothing was done.
   */
  const rewind = async (bubbleId: string, summarize = false): Promise<FileRewind[] | null> => {
    if ((turn.status !== "done" && turn.status !== "cancelled") || chatId === null) return null;
    const cut = branchAt(turn.blocks, history.current, bubbleId);
    if (!cut) return null;
    let summary: LlmMessage | null = null;
    if (summarize) {
      const before = history.current;
      try {
        summary = await branchSummary(before.slice(cut.history.length));
      } catch (e) {
        setError(String(e));
        return null;
      }
      // The chat moved on while the summary was written: the cut is stale.
      if (history.current !== before) return null;
    }
    const { changes } = changesFrom(turn.blocks, bubbleId);
    let files: FileRewind[] = [];
    try {
      // Always the chat's own folder, never the agent's open one: a chat
      // whose role has none finds nothing there to put back.
      if (changes.length) files = await rewindApply(changes, chatId);
    } catch (e) {
      setError(String(e));
      return null;
    }
    channel.stop();
    history.current = summary ? [...cut.history, summary] : cut.history;
    const next = appendNotice(restoredTurn(cut.blocks), rewoundNotice(files, summary !== null));
    setTurn(next);
    setError(null);
    setDraft((last) => ({ text: cut.text, seq: (last?.seq ?? 0) + 1 }));
    if (chatId !== null) {
      await plainChatSave(chatId, role, kube, history.current, next.blocks).catch((e) => setError(String(e)));
      void refresh();
    }
    return files;
  };

  /**
   * Puts a Prototype chat's folder back to `version` (docs/31-prototype-role.md,
   * P-6), says so in the transcript — where the model's next message learns it
   * too — and saves. `false` while a turn is under way, or when it failed.
   */
  const restore = async (version: number) => {
    if (busy || chatId === null) return false;
    try {
      const restored = await restorePrototype(chatId, version);
      const next = appendRestored(turn, restored);
      setTurn(next);
      await plainChatSave(chatId, role, kube, history.current, next.blocks);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

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

  // What a card's calls would do in this chat's cluster. One function for
  // the chat's life, reading the pin as it is when the card asks.
  const pin = useRef(kube);
  pin.current = kube;
  const preview = useCallback((calls: PendingToolCall[]) => plainChatPreview(pin.current, calls), []);

  return {
    preview,
    chats,
    chatId,
    turn,
    busy,
    roles,
    role,
    setRole: (next: ChatRoleId) => turn.blocks.length === 0 && setRole(next),
    kube,
    setPin,
    error,
    // While the turn works, its own figure: the window's history is the one
    // the turn started from.
    context: busy ? (turn.estimate ?? context) : context,
    send,
    decide,
    stop,
    compact: () => (busy ? Promise.resolve(false) : makeRoom(true)),
    open,
    newChat,
    restore,
    branchable,
    previewRewind,
    rewind,
    draft,
    remove,
    archive,
  };
}

export type PlainChatState = ReturnType<typeof usePlainChat>;
