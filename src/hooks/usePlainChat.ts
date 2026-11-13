import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { branchAt, branchPoints } from "../lib/branch";
import {
  branchSummary,
  onTurnEvent,
  rewindApply,
  rewindPreview,
  type ContextUsage,
  type FileRewind,
  type LlmMessage,
  type Outcome,
  type PendingToolCall,
  type ToolCallDecision,
} from "../lib/chat";
import type { Attached } from "./useAttachments";
import {
  type ChatRoleId,
  type ChatRoleView,
  plainChatAlwaysAllow,
  plainChatCancel,
  plainChatCompact,
  plainChatContextUsage,
  plainChatList,
  plainChatPreview,
  plainChatResume,
  plainChatRoles,
  plainChatSave,
  plainChatSend,
} from "../lib/plainChat";
import { type ChatSummary, deleteChat, generateChatTitle, loadChat, setChatArchived, setChatGroup, setChatPinned } from "../lib/savedChats";
import {
  acceptEvent,
  acceptOutcome,
  appendNotice,
  appendRestored,
  appendUserMessage,
  clearApproval,
  compactionEnded,
  compactionStarted,
  emptyTurn,
  endTurn,
  restoredTurn,
  type Block,
  type TurnState,
} from "../lib/chatTurnReducer";
import type { KubePin } from "../lib/kube";
import { addPrototypeAsset, assetsNote, pointNote, restoreNote, restorePrototype, type Asset, type Pointed } from "../lib/prototype";
import { changesFrom, rewoundNotice } from "../lib/rewind";
import type { ChatActivity } from "../types";
import { useChatTokens } from "./useChatTokens";
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

type Session = {
  key: string;
  chatId: string | null;
  turn: TurnState;
  history: LlmMessage[];
  role: ChatRoleId;
  pinned: KubePin | null;
  error: string | null;
  context: ContextUsage | null;
  contextSeq: number;
  compacting: boolean;
  unsaved: boolean;
  stopRequested: boolean;
  turnId: string;
  subscribed: (() => void) | null;
  draft: { text: string; seq: number } | null;
  writes: Set<Promise<unknown>>;
  writeTail: Promise<unknown>;
  finished: boolean;
};

const fresh = (key: string = crypto.randomUUID(), role: ChatRoleId = "assistant", pinned: KubePin | null = null): Session => ({
  key,
  chatId: null,
  turn: emptyTurn(),
  history: [],
  role,
  pinned,
  error: null,
  context: null,
  contextSeq: 0,
  compacting: false,
  unsaved: false,
  stopRequested: false,
  turnId: "",
  subscribed: null,
  draft: null,
  writes: new Set(),
  writeTail: Promise.resolve(),
  finished: false,
});

const restored = (record: Awaited<ReturnType<typeof loadChat>>): Session => ({
  ...fresh(record.id, record.role ?? "assistant"),
  chatId: record.id,
  turn: restoredTurn(chatBlocks(record.blocks, record.messages)),
  history: record.messages,
  // "Allow changes" belongs to the chat as it is open now, not to its saved pin.
  pinned: record.kube ? { kubeconfig: record.kube.kubeconfig, context: record.kube.context, namespace: record.kube.namespace } : null,
});

const kubeFor = (session: Session, lastKubeconfig: string | null): KubePin | null =>
  session.pinned ?? (lastKubeconfig ? { kubeconfig: lastKubeconfig, context: null, namespace: null } : null);

// Turn ids share one event channel across hook instances and windows.
const turnCounts = new Map<string, number>();
const mint = (session: Session, prefix: string) => {
  const count = (turnCounts.get(session.key) ?? 0) + 1;
  turnCounts.set(session.key, count);
  return `${session.key}/${prefix}-${count}`;
};

/**
 * Chat mode's conversations. Every open chat owns its history, turn listener
 * and approval card, so leaving one does not stop it or mix its events into
 * the next chat opened. The active session is the one the panels draw.
 */
export function usePlainChat(lastKubeconfig: string | null = null, model?: string, shown = false) {
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [openId, setOpenId] = useStoredState<string | null>("plain-chat-open", null, isId);
  const [roles, setRoles] = useState<ChatRoleView[]>([]);
  const sessions = useRef(new Map<string, Session>());
  const active = useRef<Session>(null as unknown as Session);
  if (sessions.current.size === 0) {
    active.current = fresh();
    sessions.current.set(active.current.key, active.current);
  }
  const [revision, redraw] = useReducer((n: number) => n + 1, 0);
  const [spentTick, spentChanged] = useReducer((n: number) => n + 1, 0);
  const modelRef = useRef(model);
  modelRef.current = model;

  const changed = useCallback((session: Session, force = false) => {
    if (force || session === active.current) redraw();
  }, []);
  const setTurn = useCallback(
    (session: Session, update: (state: TurnState) => TurnState) => {
      const before = session.turn.status;
      session.turn = update(session.turn);
      if (session.turn.status === "running" || session.turn.status === "awaitingApproval") session.finished = false;
      const ended = before === "running" || before === "awaitingApproval";
      if (ended && (session.turn.status === "done" || session.turn.status === "cancelled") && session !== active.current) {
        session.finished = true;
      }
      changed(session, before !== session.turn.status);
    },
    [changed],
  );
  const trackWrite = useCallback((session: Session, write: (snapshot: Session) => Promise<unknown>) => {
    // An async restore/rewind may finish after its chat was deleted. A write
    // from that continuation must not recreate the record.
    if (sessions.current.get(session.key) !== session) return Promise.resolve();
    const snapshot = { ...session };
    const queued = session.writes.size === 0 ? write(snapshot) : session.writeTail.then(() => write(snapshot));
    session.writeTail = queued.then(
      () => undefined,
      () => undefined,
    );
    session.writes.add(queued);
    void queued.then(
      () => session.writes.delete(queued),
      () => session.writes.delete(queued),
    );
    return queued;
  }, []);

  const stopListening = useCallback((session: Session) => {
    session.subscribed?.();
    session.subscribed = null;
  }, []);
  const listen = useCallback(
    async (session: Session, id: string) => {
      stopListening(session);
      session.subscribed = await onTurnEvent(id, (event) => {
        const now = Date.now();
        setTurn(session, (state) => acceptEvent(state, event, now, modelRef.current));
      });
    },
    [setTurn, stopListening],
  );
  useEffect(
    () => () => {
      for (const session of sessions.current.values()) stopListening(session);
    },
    [stopListening],
  );

  const refresh = useCallback(() => {
    return plainChatList().then(setChats, (e) => {
      active.current.error = String(e);
      changed(active.current);
    });
  }, [changed]);

  const finish = useCallback(
    (session: Session, outcome: Outcome) => {
      session.stopRequested = false;
      setTurn(session, (state) => acceptOutcome(state, outcome));
      if (outcome.status === "pendingApproval") return;
      session.history = outcome.value.history;
      stopListening(session);
    },
    [setTurn, stopListening],
  );
  const failed = useCallback(
    (session: Session, e: unknown) => {
      session.stopRequested = false;
      session.error = String(e);
      stopListening(session);
      setTurn(session, (state) => appendNotice(endTurn(state), `The turn failed: ${e}`));
    },
    [setTurn, stopListening],
  );

  const current = active.current;
  const busy = current.turn.status === "running" || current.turn.status === "awaitingApproval" || current.compacting;
  const kube = kubeFor(current, lastKubeconfig);
  const spent = useChatTokens(current.chatId, shown, current.turn.usage, current.turn.status, spentTick);

  const refreshContext = useCallback(
    (session: Session) => {
      const seq = ++session.contextSeq;
      plainChatContextUsage(session.role, kubeFor(session, lastKubeconfig), session.history)
        .then((context) => {
          if (seq !== session.contextSeq) return;
          session.context = context;
          changed(session);
        })
        .catch(() => {
          if (seq !== session.contextSeq) return;
          session.context = null;
          changed(session);
        });
    },
    [changed, lastKubeconfig],
  );
  useEffect(
    () => refreshContext(current),
    [current, current.chatId, current.role, current.pinned, current.turn.status, lastKubeconfig, refreshContext],
  );

  // The final save runs for background chats too, from the render holding the
  // outcome's last blocks. Writes per chat are serialized for safe deletion.
  useEffect(() => {
    for (const session of sessions.current.values()) {
      if (!session.unsaved || session.chatId === null) continue;
      if (session.turn.status !== "done" && session.turn.status !== "cancelled") continue;
      session.unsaved = false;
      trackWrite(session, (snapshot) =>
        plainChatSave(snapshot.key, snapshot.role, kubeFor(snapshot, lastKubeconfig), snapshot.history, snapshot.turn.blocks)
          .then(() => refresh())
          .catch((e) => {
            session.error = String(e);
            changed(session);
          }),
      );
      changed(session, true);
    }
  }, [revision, changed, trackWrite, refresh, lastKubeconfig]);

  const openRequest = useRef(0);
  const openLoads = useRef(new Map<string, number>());
  const activate = useCallback(
    (session: Session) => {
      openRequest.current += 1;
      session.finished = false;
      active.current = session;
      setOpenId(session.chatId);
      redraw();
    },
    [setOpenId],
  );

  const open = useCallback(
    async (id: string) => {
      const request = ++openRequest.current;
      const existing = sessions.current.get(id);
      if (existing) {
        activate(existing);
        return true;
      }
      openLoads.current.set(id, request);
      try {
        const record = await loadChat(id);
        if (request !== openRequest.current || openLoads.current.get(id) !== request) return true;
        openLoads.current.delete(id);
        const session = sessions.current.get(id) ?? restored(record);
        sessions.current.set(id, session);
        activate(session);
      } catch (e) {
        if (request !== openRequest.current || openLoads.current.get(id) !== request) return true;
        openLoads.current.delete(id);
        active.current.error = String(e);
        changed(active.current);
      }
      return true;
    },
    [activate, changed],
  );

  useEffect(() => {
    plainChatRoles().then(setRoles, (e) => {
      active.current.error = String(e);
      redraw();
    });
    void refresh();
    if (openId) void open(openId);
  }, []);

  const newChat = useCallback(() => {
    const previous = active.current;
    const pinned = previous.pinned
      ? { kubeconfig: previous.pinned.kubeconfig, context: previous.pinned.context, namespace: previous.pinned.namespace }
      : null;
    const session = fresh(crypto.randomUUID(), previous.role, pinned);
    sessions.current.set(session.key, session);
    activate(session);
    return true;
  }, [activate]);

  const setRole = (next: ChatRoleId) => {
    const session = active.current;
    if (session.turn.blocks.length > 0) return;
    session.role = next;
    changed(session);
  };

  const setPin = (next: KubePin) => {
    const session = active.current;
    session.pinned = next;
    changed(session);
    if (session.chatId === null || busy || session.turn.blocks.length === 0) return;
    trackWrite(session, (snapshot) =>
      plainChatSave(snapshot.key, snapshot.role, next, snapshot.history, snapshot.turn.blocks).catch((e) => {
        session.error = String(e);
        changed(session);
      }),
    );
  };

  const send = async (text: string, attached: Attached[] = [], pointed: Pointed | null = null, sees = true) => {
    const session = active.current;
    const trimmed = text.trim();
    const images = attached.map((one) => one.image);
    if (session.turn.status === "running" || session.turn.status === "awaitingApproval" || session.compacting || (!trimmed && images.length === 0)) return;
    const id = session.key;
    const isNew = session.chatId === null;
    const role = session.role;
    const pinned = kubeFor(session, lastKubeconfig);
    const before = session.turn;
    const kept: Asset[] = [];
    if (role === "prototype") {
      try {
        for (const one of attached) kept.push(await addPrototypeAsset(id, one.name, one.image));
      } catch (e) {
        session.error = `The pictures could not be put in the chat's folder: ${e}`;
        changed(session);
        return;
      }
    }
    if (sessions.current.get(id) !== session) return;
    const notes = [pointed && pointNote(pointed), restoreNote(before.blocks), assetsNote(kept)].filter((note): note is string => !!note);
    const sent = notes.length > 0 ? [trimmed, ...notes].filter(Boolean).join("\n\n") : undefined;
    const asked = appendUserMessage(before, trimmed, Date.now(), sent, images, pointed?.selector);
    const seen = role === "prototype" && !sees ? [] : images;
    session.history = [...session.history, { role: "user", content: sent ?? trimmed, ...(seen.length > 0 && { images: seen }) }];
    session.unsaved = true;
    session.chatId = id;
    session.pinned = pinned;
    session.error = null;
    session.stopRequested = false;
    setOpenId(id);
    setTurn(session, () => asked);
    session.turnId = mint(session, "turn");
    try {
      await listen(session, session.turnId);
      await trackWrite(session, (snapshot) => plainChatSave(id, role, pinned, snapshot.history, asked.blocks));
      if (sessions.current.get(id) !== session) {
        stopListening(session);
        return;
      }
      void refresh();
      if (isNew)
        generateChatTitle(id).then(() => {
          void refresh();
          spentChanged();
        }, () => {});
      const request = plainChatSend(session.turnId, id, role, pinned, session.history);
      // Stop was available during the initial save; that save may have delayed
      // the backend start which clears an earlier cancel flag.
      if (session.stopRequested) void plainChatCancel(id).catch(() => {});
      finish(session, await request);
    } catch (e) {
      failed(session, e);
    }
  };

  const decide = async (decisions: ToolCallDecision[], always: string[] = []) => {
    const session = active.current;
    const before = session.turn;
    const checkpoint = before.checkpoint;
    if (!checkpoint || session.chatId === null) return;
    setTurn(session, clearApproval);
    try {
      for (const tool of always) await plainChatAlwaysAllow(tool);
      if (sessions.current.get(session.key) !== session) return;
      finish(
        session,
        await plainChatResume(session.turnId, session.chatId, session.role, kubeFor(session, lastKubeconfig), checkpoint, decisions),
      );
    } catch (e) {
      // Admission can refuse a resume while another chat holds the last slot.
      // Keep the checkpoint and its card so the user can answer it again.
      session.error = String(e);
      setTurn(session, () => before);
    }
  };

  const stop = () => {
    const session = active.current;
    session.stopRequested = true;
    void plainChatCancel(session.key).catch(() => {});
  };

  const makeRoom = async (force: boolean) => {
    const session = active.current;
    if (session.turn.status === "running" || session.turn.status === "awaitingApproval" || session.compacting) return false;
    session.compacting = true;
    changed(session);
    const id = mint(session, "compact");
    let ended = false;
    const off = await onTurnEvent(id, (event) => {
      if (event.type === "historyCompacting" && !ended) setTurn(session, compactionStarted);
    });
    const end = (result: { folded: number } | null) => {
      ended = true;
      off();
      setTurn(session, (state) => compactionEnded(state, result));
    };
    try {
      const shorter = await plainChatCompact(session.role, kubeFor(session, lastKubeconfig), session.history, force, id, session.chatId);
      end(shorter && { folded: shorter.folded });
      if (!shorter) return false;
      session.history = shorter.history;
      session.unsaved = true;
      refreshContext(session);
      return true;
    } catch (e) {
      end(null);
      session.error = String(e);
      changed(session);
      return false;
    } finally {
      session.compacting = false;
      // A background compaction has a save to schedule even when no status changed.
      changed(session, true);
      spentChanged();
    }
  };

  const branchable = useMemo(
    () =>
      current.turn.status === "done" || current.turn.status === "cancelled"
        ? new Set(branchPoints(current.turn.blocks, current.history).keys())
        : null,
    [current.turn.status, current.turn.blocks],
  );

  const previewRewind = async (bubbleId: string) => {
    const session = active.current;
    if (!branchable?.has(bubbleId) || session.chatId === null) return null;
    const { changes, unrecorded } = changesFrom(session.turn.blocks, bubbleId);
    return { files: changes.length ? await rewindPreview(changes, session.chatId) : [], unrecorded };
  };

  const rewind = async (bubbleId: string, summarize = false): Promise<FileRewind[] | null> => {
    const session = active.current;
    if ((session.turn.status !== "done" && session.turn.status !== "cancelled") || session.chatId === null) return null;
    const cut = branchAt(session.turn.blocks, session.history, bubbleId);
    if (!cut) return null;
    let summary: LlmMessage | null = null;
    if (summarize) {
      const before = session.history;
      try {
        summary = await branchSummary(before.slice(cut.history.length), session.chatId);
        spentChanged();
      } catch (e) {
        session.error = String(e);
        changed(session);
        return null;
      }
      if (session.history !== before) return null;
    }
    const { changes } = changesFrom(session.turn.blocks, bubbleId);
    let files: FileRewind[] = [];
    try {
      if (changes.length) files = await rewindApply(changes, session.chatId);
    } catch (e) {
      session.error = String(e);
      changed(session);
      return null;
    }
    stopListening(session);
    session.history = summary ? [...cut.history, summary] : cut.history;
    const next = appendNotice(restoredTurn(cut.blocks), rewoundNotice(files, summary !== null));
    session.unsaved = true;
    session.error = null;
    setTurn(session, () => next);
    session.draft = { text: cut.text ?? "", seq: (session.draft?.seq ?? 0) + 1 };
    await trackWrite(session, (snapshot) =>
      plainChatSave(snapshot.key, snapshot.role, kubeFor(snapshot, lastKubeconfig), snapshot.history, next.blocks).catch((e) => {
        session.error = String(e);
        changed(session);
      }),
    );
    void refresh();
    return files;
  };

  const restore = async (version: number) => {
    const session = active.current;
    if (session.turn.status === "running" || session.turn.status === "awaitingApproval" || session.compacting || session.chatId === null) return false;
    try {
      const restoredVersion = await restorePrototype(session.chatId, version);
      const next = appendRestored(session.turn, restoredVersion);
      setTurn(session, () => next);
      await trackWrite(session, (snapshot) => plainChatSave(snapshot.key, snapshot.role, kubeFor(snapshot, lastKubeconfig), snapshot.history, next.blocks));
      return true;
    } catch (e) {
      session.error = String(e);
      changed(session);
      return false;
    }
  };

  const remove = async (id: string) => {
    openLoads.current.delete(id);
    const session = sessions.current.get(id);
    const isBusy = () => session?.turn.status === "running" || session?.turn.status === "awaitingApproval";
    if (isBusy()) return false;
    if (session) {
      while (session.writes.size > 0) await Promise.all([...session.writes]);
      if (isBusy() || session.unsaved) return false;
      stopListening(session);
      sessions.current.delete(id);
      if (session === active.current) newChat();
    }
    try {
      await deleteChat(id);
    } finally {
      void refresh();
    }
    return true;
  };

  const archive = async (id: string, archived: boolean) => {
    try {
      await setChatArchived(id, archived);
    } catch (e) {
      active.current.error = String(e);
      changed(active.current);
    }
    void refresh();
  };

  const pin = async (id: string, pinned: boolean) => {
    try {
      await setChatPinned(id, pinned);
    } catch (e) {
      active.current.error = String(e);
      changed(active.current);
    }
    void refresh();
  };

  const move = async (id: string, groupId: string | null) => {
    try {
      await setChatGroup(id, groupId);
    } catch (e) {
      active.current.error = String(e);
      changed(active.current);
    }
    void refresh();
  };

  const preview = useCallback(
    (calls: PendingToolCall[]) => plainChatPreview(kubeFor(active.current, lastKubeconfig), calls),
    [lastKubeconfig],
  );

  const chatActivity: Record<string, ChatActivity> = {};
  for (const session of sessions.current.values()) {
    if (!session.chatId) continue;
    if (session.turn.status === "running" || session.turn.status === "awaitingApproval") {
      chatActivity[session.chatId] = session.turn.status;
    } else if (session.finished) chatActivity[session.chatId] = "finished";
  }

  return {
    preview,
    chats,
    chatId: current.chatId,
    chatActivity,
    turn: current.turn,
    busy,
    roles,
    role: current.role,
    setRole,
    kube,
    setPin,
    error: current.error,
    context: busy ? (current.turn.estimate ?? current.context) : current.context,
    spent,
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
    draft: current.draft,
    remove,
    archive,
    pin,
    move,
    refresh,
  };
}

export type PlainChatState = ReturnType<typeof usePlainChat>;
