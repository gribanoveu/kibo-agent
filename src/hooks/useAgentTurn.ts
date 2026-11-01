import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
import { dropOpen, writtenChecklist, writtenPlan } from "../lib/plan";
import { HAND_TO_AGENT, typedCommand } from "../lib/slashCommands";
import { branchAt, branchPoints } from "../lib/branch";
import { changesFrom, rewoundNotice } from "../lib/rewind";
import {
  alwaysAllow,
  branchSummary,
  cancelChat,
  cancelSteer,
  compactHistory,
  contextUsage,
  type ContextUsage,
  type FileRewind,
  type LlmMessage,
  onTurnEvent,
  resumeChat,
  reviewStart,
  rewindApply,
  rewindPreview,
  startChat,
  steer as steerCommand,
  type Task,
  type ToolCallDecision,
} from "../lib/chat";
import type { ImagePart } from "../lib/images";
import { answerMcpQuestion, type McpAnswer } from "../lib/mcp";
import { useChatTokens } from "./useChatTokens";
import { generateChatTitle, loadChat, logNextPrompt, nextPromptSent, saveChat, type ChatRecord } from "../lib/savedChats";
import {
  acceptEvent,
  acceptOutcome,
  appendNotice,
  appendUserMessage,
  clearApproval,
  compactionEnded,
  compactionStarted,
  endTurn,
  emptyTurn,
  restoredTurn,
  type TurnState,
} from "../lib/chatTurnReducer";
import type { ChatActivity } from "../types";

// Every open conversation has its own session. The active one is drawn; the
// others keep their subscriptions, histories and queues, and save themselves.
// The backend remains stateless between turns: each start gets its session's
// whole history, each resume its own checkpoint.

/** A message waiting for the running turn to end, to be sent as the next one. */
export type Queued = { id: number; text: string };

/** A note typed into the running turn, until a round of it reads the note. */
export type Steered = { id: string; text: string };

type Session = {
  key: string;
  chatId: string | null;
  turn: TurnState;
  error: string | null;
  context: ContextUsage | null;
  history: LlmMessage[];
  todos: Task[];
  plan: string | null;
  turnStart: number;
  unsaved: boolean;
  turnId: number;
  branchedFrom: string | null;
  draft: { text: string; seq: number } | null;
  compacting: boolean;
  queued: Queued[];
  queueSeq: number;
  sentNotes: Steered[];
  lastTurn: { id: string; user: string } | null;
  journaled: Promise<string | null> | null;
  subscribed: (() => void) | null;
  /** Serialized saves and title writes that must settle before this chat can be deleted. */
  writes: Set<Promise<unknown>>;
  writeTail: Promise<unknown>;
  /** Finished while another chat was on screen; cleared when this one opens. */
  finished: boolean;
};

const fresh = (key: string = crypto.randomUUID()): Session => ({
  key,
  chatId: null,
  turn: emptyTurn(),
  error: null,
  context: null,
  history: [],
  todos: [],
  plan: null,
  turnStart: 0,
  unsaved: false,
  turnId: 0,
  branchedFrom: null,
  draft: null,
  compacting: false,
  queued: [],
  queueSeq: 0,
  sentNotes: [],
  lastTurn: null,
  journaled: null,
  subscribed: null,
  writes: new Set(),
  writeTail: Promise.resolve(),
  finished: false,
});

const restored = (record: ChatRecord): Session => ({
  ...fresh(record.id),
  chatId: record.id,
  turn: restoredTurn(record.blocks),
  history: record.messages,
  todos: record.todos,
  plan: record.plan ?? null,
  branchedFrom: record.branchedFrom ?? null,
});

// Turn ids on the one global event channel, counted per chat key rather than
// per hook instance. Grows by an entry per chat touched in a window's run —
// small, and a window's life is bounded.
const turnCounts = new Map<string, number>();

export function useAgentTurn({
  model,
  onSaved,
  onGiveBack,
  spentShown = false,
}: {
  /** The model turns run on — `acceptEvent` says when a cache miss came with a switch. */
  model?: string;
  onSaved?: () => void;
  /** Queued text that will not be sent after all, for the composer to take back. */
  onGiveBack?: (text: string) => void;
  /** The session's total is read only while something shows it — the Overview. */
  spentShown?: boolean;
} = {}) {
  const sessions = useRef(new Map<string, Session>());
  const active = useRef<Session>(null as unknown as Session);
  if (sessions.current.size === 0) {
    active.current = fresh();
    sessions.current.set(active.current.key, active.current);
  }
  const [revision, redraw] = useReducer((n: number) => n + 1, 0);
  const [spentTick, spentChanged] = useReducer((n: number) => n + 1, 0);
  const savedRef = useRef(onSaved);
  savedRef.current = onSaved;
  const giveBackRef = useRef(onGiveBack);
  giveBackRef.current = onGiveBack;
  const modelRef = useRef(model);
  modelRef.current = model;

  // Background deltas need no render of the chat on screen. A status change
  // does: it is what the sidebar's per-chat indicator is derived from.
  const changed = useCallback((session: Session, force = false) => {
    if (force || session === active.current) redraw();
  }, []);
  const trackWrite = useCallback((session: Session, write: (snapshot: Session) => Promise<unknown>) => {
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

  useEffect(
    () => () => {
      for (const session of sessions.current.values()) session.subscribed?.();
    },
    [],
  );

  const listen = useCallback(
    async (session: Session, id: string) => {
      session.subscribed?.();
      session.subscribed = await onTurnEvent(id, (event) => {
        // Stamped on arrival: the updater may run renders later, and the speed is timed from these.
        const now = Date.now();
        setTurn(session, (state) => acceptEvent(state, event, now, modelRef.current));
      });
    },
    [setTurn],
  );
  const stop = useCallback((session: Session) => {
    session.subscribed?.();
    session.subscribed = null;
  }, []);

  const finish = useCallback(
    (session: Session, outcome: Awaited<ReturnType<typeof startChat>>) => {
      setTurn(session, (state) => acceptOutcome(state, outcome));
      if (outcome.status !== "pendingApproval") {
        session.todos = outcome.value.todos;
        // The turn's own history, calls and results included: the next
        // message is sent with what the model read, not only what it answered.
        session.history = outcome.value.history;
        stop(session);
      }
    },
    [setTurn, stop],
  );

  // A new chat is in the sidebar from its first message, not from the end of
  // its first turn. Only the message is written: a live call may have no result.
  // Scanning every session is what lets a chat left in the background save too.
  useEffect(() => {
    for (const session of sessions.current.values()) {
      if (session.chatId !== null || !session.unsaved || session.turn.status !== "running") continue;
      session.chatId = session.key;
      changed(session, true);
      trackWrite(
        session,
        (snapshot) => saveChat(
          snapshot.key,
          snapshot.history,
          snapshot.turn.blocks.slice(0, snapshot.turnStart + 1),
          snapshot.todos,
          snapshot.plan,
          snapshot.branchedFrom,
        )
          .then(() => {
            savedRef.current?.();
            return generateChatTitle(snapshot.key).then(() => {
              savedRef.current?.();
              spentChanged();
            }, () => {});
          })
          .catch((e) => {
            session.error = String(e);
            changed(session);
          }),
      );
    }
  }, [revision, changed, trackWrite]);

  // Save only from the render that has the last block. The same effect serves
  // the active chat and one that came to rest in the background.
  useEffect(() => {
    for (const session of sessions.current.values()) {
      if (!session.unsaved || (session.turn.status !== "done" && session.turn.status !== "cancelled")) continue;
      session.unsaved = false;
      const written = writtenPlan(session.turn.blocks.slice(session.turnStart));
      if (written !== null) session.plan = written;
      session.chatId = session.key;
      if (session.lastTurn) {
        session.journaled = logNextPrompt(session.key, session.lastTurn.id, session.lastTurn.user).catch(() => null);
        session.lastTurn = null;
      }
      trackWrite(
        session,
        (snapshot) => saveChat(snapshot.key, snapshot.history, snapshot.turn.blocks, snapshot.todos, snapshot.plan, snapshot.branchedFrom)
          .then(() => savedRef.current?.())
          .catch((e) => {
            session.error = String(e);
            changed(session);
          }),
      );
      changed(session, true);
    }
  }, [revision, changed, trackWrite]);

  const refreshContext = useCallback(
    (session: Session) => {
      contextUsage(session.history).then(
        (context) => {
          session.context = context;
          changed(session);
        },
        () => {
          // A meter that cannot be drawn is not worth an error banner.
          session.context = null;
          changed(session);
        },
      );
    },
    [changed],
  );

  const current = active.current;
  useEffect(
    () => refreshContext(active.current),
    // The history can change at a turn start/end, or when another chat opens.
    [current, current.chatId, current.turn.status, refreshContext],
  );
  const spent = useChatTokens(current.chatId ?? current.key, spentShown, current.turn.usage, current.turn.status, spentTick);

  const mint = useCallback((session: Session, prefix: string) => {
    const n = (turnCounts.get(session.key) ?? 0) + 1;
    turnCounts.set(session.key, n);
    session.turnId = n;
    return `${session.key}/${prefix}-${n}`;
  }, []);

  /** Folds the older part of a conversation into a summary. */
  const compactSession = useCallback(
    async (session: Session, force: boolean) => {
      if (session.turn.status === "running" || session.turn.status === "awaitingApproval") return false;
      if (session.compacting) return false;
      session.compacting = true;
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
        const shorter = await compactHistory(session.history, force, id, session.key);
        end(shorter && { folded: shorter.folded });
        if (!shorter) return false;
        session.history = shorter.history;
        session.unsaved = true;
        refreshContext(session);
        return true;
      } catch (e) {
        end(null);
        if (force) {
          session.error = String(e);
          changed(session);
        }
        return false;
      } finally {
        session.compacting = false;
        spentChanged();
      }
    },
    [mint, setTurn, refreshContext, changed],
  );

  const sendSession = useCallback(
    async (session: Session, text: string, sent?: string, images: ImagePart[] = [], keepChecklist = false) => {
      const trimmed = text.trim();
      const content = sent?.trim() || trimmed;
      if (!trimmed && images.length === 0) return;

      if (session.turn.status === "running") {
        const id = await steerCommand(content, content === trimmed ? undefined : trimmed, session.key);
        session.sentNotes = [...session.sentNotes, { id, text: trimmed }];
        changed(session);
        return;
      }

      session.error = null;
      const answered = session.journaled;
      session.journaled = null;
      void answered?.then((row) => (row ? nextPromptSent(row, trimmed) : undefined)).catch(() => {});
      session.unsaved = true;
      session.turnStart = session.turn.blocks.length;
      setTurn(session, (state) => appendUserMessage(state, trimmed, Date.now(), content, images));
      session.history = [...session.history, { role: "user", content, ...(images.length > 0 && { images }) }];
      if (!keepChecklist) session.todos = dropOpen(session.todos);

      const id = mint(session, "turn");
      session.lastTurn = { id, user: trimmed };
      try {
        await listen(session, id);
        finish(session, await startChat(id, session.key, session.history, session.todos, session.plan));
      } catch (e) {
        session.error = String(e);
        stop(session);
        setTurn(session, (state) => appendNotice(endTurn(state), `The turn failed: ${e}`));
      }
    },
    [setTurn, mint, listen, finish, changed, stop],
  );

  const reviewSession = useCallback(
    async (session: Session, handed = false) => {
      const typed = handed ? typedCommand("review", HAND_TO_AGENT) : "/review";
      if (session.turn.status === "running" || session.turn.status === "awaitingApproval") return;
      session.error = null;
      const answered = session.journaled;
      session.journaled = null;
      void answered?.then((row) => (row ? nextPromptSent(row, typed) : undefined)).catch(() => {});
      session.unsaved = true;
      session.turnStart = session.turn.blocks.length;
      setTurn(session, (state) => appendUserMessage(state, typed, Date.now(), "/review"));
      session.todos = dropOpen(session.todos);

      const id = mint(session, "turn");
      session.lastTurn = { id, user: typed };
      try {
        await listen(session, id);
        finish(session, await reviewStart(id, session.key, session.history, session.todos, session.plan, handed));
      } catch (e) {
        session.error = String(e);
        stop(session);
        session.history = [
          ...session.history,
          { role: "user", content: "/review" },
          { role: "assistant", content: `[The review failed: ${e}]` },
        ];
        setTurn(session, (state) => appendNotice(endTurn(state), `The review failed: ${e}`));
      }
    },
    [setTurn, mint, listen, finish, stop],
  );

  // A finished turn sends its queue. Text that has to come back waits while
  // its chat is in the background, rather than filling another chat's composer.
  useEffect(() => {
    for (const session of sessions.current.values()) {
      if (session.turn.status !== "done" && session.turn.status !== "cancelled") continue;
      if (session !== active.current) continue;
      if (session.queued.length === 0 && session.sentNotes.length === 0) continue;
      const read = new Set(session.turn.blocks.flatMap((block) => (block.kind === "steer" ? [block.id] : [])));
      const back = session.sentNotes.filter((note) => !read.has(`steer:${note.id}`)).map((note) => note.text);
      session.sentNotes = [];
      const stopped = session.turn.status === "cancelled" || session.error !== null;
      if (stopped) back.push(...session.queued.map((q) => q.text));
      if (back.length > 0) giveBackRef.current?.(back.join("\n\n"));
      if (stopped || session.queued.length === 0) {
        session.queued = [];
        changed(session);
        continue;
      }
      const [next, ...rest] = session.queued;
      session.queued = rest;
      changed(session);
      void sendSession(session, next.text);
    }
  }, [revision, changed, sendSession]);

  const openRequest = useRef(0);
  const openLoads = useRef(new Map<string, number>());
  const activate = useCallback((session: Session) => {
    openRequest.current += 1;
    session.finished = false;
    active.current = session;
    redraw();
  }, []);

  const open = useCallback(
    async (id: string) => {
      const request = ++openRequest.current;
      const existing = sessions.current.get(id);
      if (existing) {
        activate(existing);
        return true;
      }
      openLoads.current.set(id, request);
      active.current.journaled = null;
      try {
        const record = await loadChat(id);
        if (request !== openRequest.current || openLoads.current.get(id) !== request) return true;
        openLoads.current.delete(id);
        // Two clicks may have loaded the same chat together; keep only one.
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

  const reset = useCallback(() => {
    active.current.journaled = null;
    const session = fresh();
    sessions.current.set(session.key, session);
    activate(session);
    return true;
  }, [activate]);

  /** Forgets a deleted chat once its writes settle; a running one must be stopped first. */
  const discard = useCallback(
    async (id: string) => {
      openLoads.current.delete(id);
      const session = sessions.current.get(id);
      if (!session) return true;
      const busy = () => session.turn.status === "running" || session.turn.status === "awaitingApproval";
      if (busy()) return false;
      while (session.writes.size > 0) await Promise.all([...session.writes]);
      if (busy() || session.unsaved) return false;
      stop(session);
      sessions.current.delete(id);
      if (session === active.current) reset();
      else redraw();
      return true;
    },
    [reset, stop],
  );

  const queue = useCallback(
    (text: string) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      const session = active.current;
      session.queued = [...session.queued, { id: ++session.queueSeq, text: trimmed }];
      changed(session);
    },
    [changed],
  );

  const unqueue = useCallback(
    (id: number) => {
      const session = active.current;
      const item = session.queued.find((q) => q.id === id);
      if (!item) return;
      session.queued = session.queued.filter((q) => q !== item);
      giveBackRef.current?.(item.text);
      changed(session);
    },
    [changed],
  );

  const withdraw = useCallback(
    async (id: string) => {
      const session = active.current;
      const note = session.sentNotes.find((n) => n.id === id);
      if (!note) return;
      const previous = session.draft?.seq ?? 0;
      session.sentNotes = session.sentNotes.filter((n) => n.id !== id);
      changed(session);
      try {
        if (await cancelSteer(id, session.key)) {
          if (session === active.current) giveBackRef.current?.(note.text);
          else {
            const text = session.draft ? `${session.draft.text}\n\n${note.text}` : note.text;
            session.draft = { text, seq: (session.draft?.seq ?? previous) + 1 };
          }
          changed(session);
        }
      } catch (e) {
        session.error = String(e);
        changed(session);
      }
    },
    [changed],
  );

  const decide = useCallback(
    async (decisions: ToolCallDecision[], always: string[] = []) => {
      const session = active.current;
      const checkpoint = session.turn.checkpoint;
      if (!checkpoint) return;
      setTurn(session, clearApproval);
      const id = `${session.key}/turn-${session.turnId}`;
      try {
        for (const tool of always) await alwaysAllow(tool);
        const plan = writtenPlan(session.turn.blocks.slice(session.turnStart)) ?? session.plan;
        finish(session, await resumeChat(id, session.key, checkpoint, decisions, plan));
      } catch (e) {
        session.error = String(e);
        stop(session);
        setTurn(session, (state) => appendNotice(endTurn(state), `The turn failed: ${e}`));
      }
    },
    [setTurn, finish, stop],
  );

  const cancel = useCallback(() => cancelChat(active.current.key), []);
  const cancelAll = useCallback(async () => {
    const running = [...sessions.current.values()].filter(
      (session) => session.turn.status === "running" || session.turn.status === "awaitingApproval",
    );
    await Promise.all(running.map((session) => cancelChat(session.key)));
  }, []);

  const answerQuestion = useCallback(
    async (id: string, answer: McpAnswer) => {
      const session = active.current;
      try {
        await answerMcpQuestion(id, answer);
      } catch (e) {
        setTurn(session, (state) => appendNotice(state, `The answer did not reach the server: ${e}`));
      }
    },
    [setTurn],
  );

  const branchable = useMemo(
    () =>
      current.turn.status === "done" || current.turn.status === "cancelled"
        ? new Set(branchPoints(current.turn.blocks, current.history).keys())
        : null,
    [current.turn.status, current.turn.blocks],
  );

  const branch = useCallback(
    (bubbleId?: string) => {
      const from = active.current;
      if (from.turn.status !== "done" && from.turn.status !== "cancelled") return;
      const whole = bubbleId === undefined;
      const cut = whole
        ? { blocks: from.turn.blocks, history: from.history, text: null }
        : branchAt(from.turn.blocks, from.history, bubbleId);
      if (!cut || (whole && cut.blocks.length === 0)) return;
      from.journaled = null;
      const session = fresh();
      session.branchedFrom = from.chatId;
      session.history = cut.history;
      session.todos = whole ? from.todos : [];
      session.plan = whole ? from.plan : writtenPlan(cut.blocks);
      session.unsaved = whole;
      session.turnStart = cut.blocks.length;
      session.turn = appendNotice(
        restoredTurn(cut.blocks),
        whole
          ? "Forked — a copy of the conversation; the original chat is left as it is"
          : "Branched from here — files the agent changed later in the original chat are left as they are now",
      );
      session.draft = cut.text === null ? null : { text: cut.text, seq: (from.draft?.seq ?? 0) + 1 };
      sessions.current.set(session.key, session);
      activate(session);
      refreshContext(session);
    },
    [activate, refreshContext],
  );

  const previewRewind = useCallback(
    async (bubbleId: string) => {
      const session = active.current;
      const points = session.turn.status === "done" || session.turn.status === "cancelled"
        ? new Set(branchPoints(session.turn.blocks, session.history).keys())
        : null;
      if (!points?.has(bubbleId)) return null;
      const { changes, unrecorded } = changesFrom(session.turn.blocks, bubbleId);
      return { files: changes.length ? await rewindPreview(changes) : [], unrecorded };
    },
    [],
  );

  const rewind = useCallback(
    async (bubbleId: string, summarize = false) => {
      const session = active.current;
      if (session.turn.status !== "done" && session.turn.status !== "cancelled") return null;
      const cut = branchAt(session.turn.blocks, session.history, bubbleId);
      if (!cut) return null;
      let summary: LlmMessage | null = null;
      if (summarize) {
        const before = session.history;
        try {
          summary = await branchSummary(before.slice(cut.history.length), session.key);
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
        if (changes.length) files = await rewindApply(changes);
      } catch (e) {
        session.error = String(e);
        changed(session);
        return null;
      }
      stop(session);
      session.history = summary ? [...cut.history, summary] : cut.history;
      session.todos = [];
      session.plan = writtenPlan(cut.blocks);
      session.unsaved = true;
      session.turnStart = cut.blocks.length;
      session.error = null;
      setTurn(session, () => appendNotice(restoredTurn(cut.blocks), rewoundNotice(files, summary !== null)));
      session.draft = cut.text === null ? null : { text: cut.text, seq: (session.draft?.seq ?? 0) + 1 };
      refreshContext(session);
      return files;
    },
    [stop, setTurn, refreshContext, changed],
  );

  const editPlan = useCallback(
    (next: string) => {
      const session = active.current;
      const value = next.trim() ? next : null;
      session.plan = value;
      changed(session);
      if (!session.chatId || session.turn.status === "running") return;
      trackWrite(
        session,
        (snapshot) => saveChat(snapshot.key, snapshot.history, snapshot.turn.blocks, snapshot.todos, value, snapshot.branchedFrom).catch((e) => {
          session.error = String(e);
          changed(session);
        }),
      );
    },
    [changed, trackWrite],
  );

  const saveBranch = useCallback(() => {
    const session = active.current;
    if (!session.chatId || session.turn.status === "running") return;
    trackWrite(
      session,
      (snapshot) => saveChat(snapshot.key, snapshot.history, snapshot.turn.blocks, snapshot.todos, snapshot.plan, snapshot.branchedFrom)
        .then(() => savedRef.current?.())
        .catch((e) => {
          session.error = String(e);
          changed(session);
        }),
    );
  }, [changed, trackWrite]);

  const busy = current.turn.status === "running" || current.turn.status === "awaitingApproval";
  const liveBlocks = busy ? current.turn.blocks.slice(current.turnStart) : [];
  const liveChecklist = writtenChecklist(liveBlocks);
  const livePlan = writtenPlan(liveBlocks);
  const steered = current.sentNotes.filter((note) =>
    !current.turn.blocks.some((block) => block.kind === "steer" && block.id === `steer:${note.id}`),
  );
  const chatActivity: Record<string, ChatActivity> = {};
  for (const session of sessions.current.values()) {
    if (!session.chatId) continue;
    if (session.turn.status === "running" || session.turn.status === "awaitingApproval") {
      chatActivity[session.chatId] = session.turn.status;
    } else if (session.finished) chatActivity[session.chatId] = "finished";
  }

  return {
    turn: current.turn,
    chatId: current.chatId,
    chatActivity,
    spent,
    error: current.error,
    context: busy ? (current.turn.estimate ?? current.context) : current.context,
    send: (text: string, sent?: string, images: ImagePart[] = [], keepChecklist = false) =>
      sendSession(active.current, text, sent, images, keepChecklist),
    review: (handed = false) => reviewSession(active.current, handed),
    queued: current.queued,
    queue,
    unqueue,
    steered,
    withdraw,
    decide,
    cancel,
    cancelAll,
    answerQuestion,
    open,
    reset,
    discard,
    compact: (force: boolean) => compactSession(active.current, force),
    plan: livePlan ?? current.plan,
    editPlan,
    saveBranch,
    checklist: liveChecklist ?? current.todos,
    branch,
    branchable,
    previewRewind,
    rewind,
    draft: current.draft,
  };
}
