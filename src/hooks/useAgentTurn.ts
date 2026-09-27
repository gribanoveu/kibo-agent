import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { writtenChecklist, writtenPlan } from "../lib/plan";
import { branchAt, branchPoints } from "../lib/branch";
import { changesFrom } from "../lib/rewind";
import {
  cancelChat,
  compactHistory,
  contextUsage,
  loadChat,
  logNextPrompt,
  nextPromptSent,
  onTurnEvent,
  resumeChat,
  saveChat,
  startChat,
  steer as steerCommand,
  alwaysAllow,
  reviewStart,
  rewindApply,
  rewindPreview,
  type ContextUsage,
  type FileRewind,
  type LlmMessage,
  type Task,
  type ToolCallDecision,
} from "../lib/chat";
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
  reviewEnded,
  reviewStarted,
  type TurnState,
} from "../lib/chatTurnReducer";

// One conversation, driven from the window.
//
// The transcript lives here, not in the backend: `chat_start` takes the whole
// history and `chat_resume` takes the whole checkpoint, so this hook is the
// side that remembers. What it keeps is deliberately two things — the blocks a
// reader sees, and the messages the model sees. They are not the same list: a
// tool call is one block and two messages, and a collapsed detail is neither.
//
// Both halves are written to disk when a turn ends, which is what makes the
// conversation outlive the window. Not before it ends: a transcript saved
// mid-turn has a tool call in it with no result.

/**
 * A message waiting for the running turn to end, to be sent as the next one.
 * `sent`, as in `send`: a `/` command's prompt, when `text` is the command.
 */
export type Queued = { id: number; text: string; sent?: string };

export function useAgentTurn({
  onSaved,
  onGiveBack,
}: {
  onSaved?: () => void;
  /** Queued text that will not be sent after all, for the composer to take back. */
  onGiveBack?: (text: string) => void;
} = {}) {
  const [turn, setTurn] = useState<TurnState>(emptyTurn);
  const [chatId, setChatId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // What the next request would cost. Not derived from the last turn's usage:
  // that is prompt *and* completion of a request already paid for, while the
  // question the meter answers is what the next one will weigh.
  const [context, setContext] = useState<ContextUsage | null>(null);
  const history = useRef<LlmMessage[]>([]);
  const todos = useRef<Task[]>([]);
  // The checklist and the plan, as state as well as refs: the Plan tab shows
  // them, and the callbacks below need the current value without re-binding.
  const [checklist, setChecklist] = useState<Task[]>([]);
  const [plan, setPlanState] = useState<string | null>(null);
  const planRef = useRef<string | null>(null);
  // Counts the turns that finished having written a plan — the window opens
  // the Plan tab on each. A count, not a flag: two plans in a row are two
  // openings, and opening a saved chat is none.
  const [planWritten, setPlanWritten] = useState(0);
  // Where the running turn's blocks begin — what `writtenPlan` looks at.
  const turnStart = useRef(0);
  const keepPlan = useCallback((next: string | null) => {
    planRef.current = next;
    setPlanState(next);
  }, []);
  const keepTodos = useCallback((next: Task[]) => {
    todos.current = next;
    setChecklist(next);
  }, []);
  // Set when something happened that is worth writing down. Without it,
  // *opening* a chat would save it straight back and push it to the top of
  // the sidebar for having been read.
  const unsaved = useRef(false);
  // A turn's own id, so its events can be told from another turn's on the one
  // global channel.
  const turnId = useRef(0);
  // The chat this one was branched from, written with every save of it.
  const branchedFrom = useRef<string | null>(null);
  // Text handed to the composer — a branch gives back the message it starts
  // at. A counter rather than the text alone, so the same text twice still
  // lands.
  const [draft, setDraft] = useState<{ text: string; seq: number } | null>(null);
  const subscribed = useRef<(() => void) | null>(null);
  const compacting = useRef(false);
  const [queued, setQueued] = useState<Queued[]>([]);
  const queueSeq = useRef(0);
  // The next-prompt journal: the turn started by `send` and what started it,
  // then the row it was journaled as — a promise, since a queued message can
  // be sent before the row is written.
  const lastTurn = useRef<{ id: string; user: string } | null>(null);
  const journaled = useRef<Promise<string | null> | null>(null);
  const giveBackRef = useRef(onGiveBack);
  giveBackRef.current = onGiveBack;

  useEffect(() => () => subscribed.current?.(), []);

  const listen = useCallback(async (id: string) => {
    subscribed.current?.();
    const off = await onTurnEvent(id, (event) => setTurn((state) => acceptEvent(state, event)));
    subscribed.current = off;
  }, []);

  const finish = useCallback((outcome: Awaited<ReturnType<typeof startChat>>) => {
    setTurn((state) => acceptOutcome(state, outcome));
    if (outcome.status !== "pendingApproval") {
      keepTodos(outcome.value.todos);
      // The turn's own history, calls and results included: the next
      // message is sent with what the model read, not only what it answered.
      history.current = outcome.value.history;
      subscribed.current?.();
      subscribed.current = null;
    }
  }, [keepTodos]);

  // The checklist while the turn works: the turn hands its list back only when
  // it ends, so until then the Plan tab follows the `todo` calls themselves.
  useEffect(() => {
    if (turn.status !== "running" && turn.status !== "awaitingApproval") return;
    const live = writtenChecklist(turn.blocks.slice(turnStart.current));
    if (live) setChecklist(live);
  }, [turn.status, turn.blocks]);

  // Saved once the turn has come to rest, from the render that has the last
  // block in it — which is why this is an effect and not the tail of `finish`,
  // where the final events have been dispatched but not yet applied.
  useEffect(() => {
    if (!unsaved.current) return;
    if (turn.status !== "done" && turn.status !== "cancelled") return;
    unsaved.current = false;

    const written = writtenPlan(turn.blocks.slice(turnStart.current));
    if (written !== null) keepPlan(written);
    // Stopped halfway, the plan is not ready to be read.
    if (written !== null && turn.status === "done") setPlanWritten((n) => n + 1);
    const id = chatId ?? crypto.randomUUID();
    setChatId(id);
    if (lastTurn.current) {
      // A journal that cannot be written is not the user's problem.
      journaled.current = logNextPrompt(id, lastTurn.current.id, lastTurn.current.user).catch(() => null);
      lastTurn.current = null;
    }
    saveChat(id, history.current, turn.blocks, todos.current, written ?? planRef.current, branchedFrom.current)
      .then(() => onSaved?.())
      .catch((e) => setError(String(e)));
  }, [turn.status, turn.blocks, chatId, onSaved, keepPlan]);

  const refreshContext = useCallback(() => {
    contextUsage(history.current, planRef.current)
      .then(setContext)
      // A meter that cannot be drawn is not worth an error banner over the
      // conversation it is measuring.
      .catch(() => setContext(null));
  }, []);

  // Every point the history can have changed: a turn starting, a turn coming
  // to rest, a chat being opened or cleared — and the first render, where an
  // empty conversation already costs the prompt and the schemas.
  useEffect(refreshContext, [refreshContext, turn.status, chatId]);

  /**
   * Folds the older part of the conversation into a summary, when the backend
   * says it is worth it — `force` is the user asking outright.
   *
   * Here rather than inside the turn because the window owns the history: a
   * turn can shorten its own copy (and does, when a request is refused), but
   * only this side can keep the shorter one for next time. Nothing about when
   * or how much is decided here.
   */
  const makeRoom = useCallback(async (force: boolean) => {
    // One pass at a time: a second would summarize the history the first is
    // about to replace.
    if (compacting.current) return false;
    compacting.current = true;
    // Its own id on the turn channel: the backend says there when a summary
    // is being made, which only it knows. Its end is what the call returns —
    // so a start arriving after that is stale, not a second pass.
    const id = `compact-${++turnId.current}`;
    let ended = false;
    const off = await onTurnEvent(id, (event) => {
      if (event.type === "historyCompacting" && !ended) setTurn(compactionStarted);
    });
    const end = (result: { folded: number } | null) => {
      ended = true;
      off();
      setTurn((state) => compactionEnded(state, result));
    };
    try {
      const shorter = await compactHistory(history.current, force, planRef.current, id);
      end(shorter && { folded: shorter.folded });
      if (!shorter) return false;
      history.current = shorter.history;
      unsaved.current = true;
      refreshContext();
      return true;
    } catch (e) {
      // Not fatal on the way to a turn: the request may well still fit, and if
      // it does not, the turn's own pass reports what the provider said. The
      // card says the pass gave up; only an explicit request is worth an error.
      end(null);
      if (force) setError(String(e));
      return false;
    } finally {
      compacting.current = false;
    }
  }, [refreshContext]);

  /** Holds `text` until the running turn ends, to be sent as the next message. */
  const queue = useCallback((text: string, sent?: string) => {
    const trimmed = text.trim();
    if (trimmed) setQueued((list) => [...list, { id: ++queueSeq.current, text: trimmed, sent }]);
  }, []);

  /**
   * Sends what the user typed. While a turn is running the same text steers it.
   * `sent`, when given, is what the model gets instead — a `/` command's prompt,
   * with `text` (`/init`) what the transcript shows.
   */
  const send = useCallback(
    async (text: string, sent?: string) => {
      const trimmed = text.trim();
      const content = sent?.trim() || trimmed;
      if (!trimmed) return;

      if (turn.status === "running") {
        // A review's workers take no notes: what is typed during one waits
        // for it, as the next message.
        if (turn.blocks.some((block) => block.kind === "review" && block.report === null)) queue(trimmed, sent);
        else await steerCommand(content, content === trimmed ? undefined : trimmed);
        return;
      }

      setError(null);
      const answered = journaled.current;
      journaled.current = null;
      void answered?.then((row) => (row ? nextPromptSent(row, trimmed) : undefined)).catch(() => {});
      unsaved.current = true;
      turnStart.current = turn.blocks.length;
      setTurn((state) => appendUserMessage(state, trimmed, Date.now(), content));
      history.current = [...history.current, { role: "user", content }];
      // Before the turn, so the room is made once and kept — a turn that
      // compacts its own copy pays for the summary again on the next message.
      await makeRoom(false);

      const id = `turn-${++turnId.current}`;
      lastTurn.current = { id, user: trimmed };
      try {
        await listen(id);
        finish(await startChat(id, history.current, todos.current, planRef.current));
      } catch (e) {
        setError(String(e));
        // In the transcript, not only in `error`: a turn that fails before its
        // first event otherwise ends with nothing on screen to say why.
        setTurn((state) => appendNotice(endTurn(state), `The turn failed: ${e}`));
      }
    },
    [turn.status, turn.blocks, listen, finish, makeRoom, queue],
  );

  /**
   * `/review`: the working tree's changes reviewed by the backend's workers,
   * as a card in the chat. The report joins the history as the answer to
   * `/review`, so the next message can say "fix 2".
   */
  const review = useCallback(async () => {
    if (turn.status === "running" || turn.status === "awaitingApproval") return;
    setError(null);
    // `/review` is the user's next message after the turn journaled last.
    const answered = journaled.current;
    journaled.current = null;
    void answered?.then((row) => (row ? nextPromptSent(row, "/review") : undefined)).catch(() => {});
    unsaved.current = true;
    turnStart.current = turn.blocks.length;
    setTurn((state) => reviewStarted(appendUserMessage(state, "/review", Date.now())));
    const id = `turn-${++turnId.current}`;
    try {
      await listen(id);
      const done = await reviewStart(id);
      history.current = [...history.current, { role: "user", content: "/review" }, { role: "assistant", content: done.forModel }];
      setTurn((state) => reviewEnded(state, done.report));
    } catch (e) {
      setError(String(e));
      // In the history too: the bubble has to have its message, or every
      // bubble before it stops being one a branch can start at.
      history.current = [...history.current, { role: "user", content: "/review" }, { role: "assistant", content: `[The review failed: ${e}]` }];
      setTurn((state) =>
        appendNotice(
          endTurn({ ...state, blocks: state.blocks.filter((block) => block.kind !== "review" || block.report !== null) }),
          `The review failed: ${e}`,
        ),
      );
    } finally {
      subscribed.current?.();
      subscribed.current = null;
    }
  }, [turn.status, turn.blocks.length, listen]);

  /** Takes a message out of the queue and hands it back to the composer. */
  const unqueue = useCallback(
    (id: number) => {
      const item = queued.find((q) => q.id === id);
      if (!item) return;
      setQueued(queued.filter((q) => q !== item));
      giveBackRef.current?.(item.text);
    },
    [queued],
  );

  const giveBackQueue = useCallback(() => {
    if (queued.length === 0) return;
    setQueued([]);
    giveBackRef.current?.(queued.map((q) => q.text).join("\n\n"));
  }, [queued]);

  // A turn that finished sends the next queued message. One that was stopped
  // or failed gives the queue back instead: what was meant to follow it may
  // no longer be what the user wants, and starting it unasked is worse than
  // an extra Enter. After the save effect above, which it would otherwise
  // find already marked unsaved.
  useEffect(() => {
    if (queued.length === 0 || (turn.status !== "done" && turn.status !== "cancelled")) return;
    if (turn.status === "cancelled" || error !== null) return giveBackQueue();
    const [next, ...rest] = queued;
    setQueued(rest);
    void send(next.text, next.sent);
  }, [turn.status, error, queued, send, giveBackQueue]);

  /** Answers the approval card. `always` widens the policy before continuing. */
  const decide = useCallback(
    async (decisions: ToolCallDecision[], always: string[] = []) => {
      const checkpoint = turn.checkpoint;
      if (!checkpoint) return;

      setTurn((state) => clearApproval(state));
      const id = `turn-${turnId.current}`;
      try {
        for (const tool of always) await alwaysAllow(tool);
        finish(await resumeChat(id, checkpoint, decisions, planRef.current));
      } catch (e) {
        setError(String(e));
        // In the transcript, not only in `error`: a turn that fails before its
        // first event otherwise ends with nothing on screen to say why.
        setTurn((state) => appendNotice(endTurn(state), `The turn failed: ${e}`));
      }
    },
    [turn.checkpoint, finish],
  );

  const cancel = useCallback(async () => {
    await cancelChat();
  }, []);

  // A turn under way belongs to the chat on screen: its answer, its approval
  // card and its first save all land here. Leaving would hand them to the
  // next chat and drop this one, so leaving waits for it to stop.
  const busy = turn.status === "running" || turn.status === "awaitingApproval";

  /**
   * Reopens a saved conversation, transcript and model history both. `false`
   * when it cannot, because a turn is under way here.
   */
  const open = useCallback(async (id: string) => {
    if (busy) return false;
    journaled.current = null;
    try {
      const record = await loadChat(id);
      subscribed.current?.();
      subscribed.current = null;
      history.current = record.messages;
      keepTodos(record.todos);
      keepPlan(record.plan ?? null);
      branchedFrom.current = record.branchedFrom ?? null;
      unsaved.current = false;
      setChatId(record.id);
      setError(null);
      setTurn(restoredTurn(record.blocks));
    } catch (e) {
      setError(String(e));
    }
    return true;
  }, [busy, keepTodos, keepPlan]);

  /**
   * Starts over. What was said is already on disk; this only stops pointing
   * at it. `false` while a turn is under way, which has not been.
   */
  const reset = useCallback(() => {
    if (busy) return false;
    journaled.current = null;
    subscribed.current?.();
    subscribed.current = null;
    history.current = [];
    keepTodos([]);
    keepPlan(null);
    branchedFrom.current = null;
    unsaved.current = false;
    setChatId(null);
    setError(null);
    setTurn(emptyTurn());
    return true;
  }, [busy, keepTodos, keepPlan]);

  // Bubbles a branch can start at; `null` while a turn is under way, when
  // none can. Recomputed with the transcript: the history only changes when
  // the blocks do.
  const branchable = useMemo(
    () =>
      turn.status === "done" || turn.status === "cancelled"
        ? new Set(branchPoints(turn.blocks, history.current).keys())
        : null,
    [turn.status, turn.blocks],
  );

  /**
   * Starts a new chat from the conversation as it was just before `bubbleId`,
   * and hands that message back to the composer to be changed and sent. The
   * chat it came from is left as it is. Without `bubbleId` it is a fork: the
   * whole conversation, checklist and plan included, and nothing handed back.
   *
   * A branch is saved only once it is sent: one abandoned is not a row in the
   * sidebar. A fork is saved at once — asked for by name, it is the chat the
   * user means to keep. The branch's checklist starts empty — the one kept is
   * the latest, and part of it may be work done after this point — and the
   * plan is the last one written before it.
   */
  const branch = useCallback(
    (bubbleId?: string) => {
      if (turn.status !== "done" && turn.status !== "cancelled") return;
      const whole = bubbleId === undefined;
      const cut = whole
        ? { blocks: turn.blocks, history: history.current, text: null }
        : branchAt(turn.blocks, history.current, bubbleId);
      if (!cut || (whole && cut.blocks.length === 0)) return;
      subscribed.current?.();
      subscribed.current = null;
      journaled.current = null;
      branchedFrom.current = chatId;
      history.current = cut.history;
      keepTodos(whole ? todos.current : []);
      keepPlan(whole ? planRef.current : writtenPlan(cut.blocks));
      // A fork is written by the save effect, under a new id since `chatId`
      // is cleared. Its turn begins past the copied blocks, so the plan the
      // original wrote is not announced again as just written.
      unsaved.current = whole;
      turnStart.current = cut.blocks.length;
      setChatId(null);
      setError(null);
      setTurn(
        appendNotice(
          restoredTurn(cut.blocks),
          whole
            ? "Forked — a copy of the conversation; the original chat is left as it is"
            : "Branched from here — files the agent changed later in the original chat are left as they are now",
        ),
      );
      const text = cut.text;
      if (text !== null) setDraft((last) => ({ text, seq: (last?.seq ?? 0) + 1 }));
      refreshContext();
    },
    [turn.status, turn.blocks, chatId, keepTodos, keepPlan, refreshContext],
  );

  /**
   * What rewinding to before `bubbleId` would do to files, and how many calls
   * ran whose changes were not recorded. `null` where a rewind cannot start —
   * the same places a branch cannot.
   */
  const previewRewind = useCallback(
    async (bubbleId: string) => {
      if (!branchable?.has(bubbleId)) return null;
      const { changes, unrecorded } = changesFrom(turn.blocks, bubbleId);
      return { files: changes.length ? await rewindPreview(changes) : [], unrecorded };
    },
    [branchable, turn.blocks],
  );

  /**
   * Takes this chat back to just before `bubbleId`: the files the agent
   * changed from there on are put back where nobody changed them since, the
   * messages from there on are gone, and that message is handed back to the
   * composer. Files first — a rewind that could not reach them leaves the
   * conversation as it was. Returns how each file went, or `null` when
   * nothing was done.
   */
  const rewind = useCallback(
    async (bubbleId: string) => {
      if (turn.status !== "done" && turn.status !== "cancelled") return null;
      const cut = branchAt(turn.blocks, history.current, bubbleId);
      if (!cut) return null;
      const { changes } = changesFrom(turn.blocks, bubbleId);
      let files: FileRewind[] = [];
      try {
        if (changes.length) files = await rewindApply(changes);
      } catch (e) {
        setError(String(e));
        return null;
      }
      subscribed.current?.();
      subscribed.current = null;
      history.current = cut.history;
      keepTodos([]);
      keepPlan(writtenPlan(cut.blocks));
      unsaved.current = true;
      turnStart.current = cut.blocks.length;
      setError(null);
      setTurn(appendNotice(restoredTurn(cut.blocks), rewoundNotice(files)));
      const text = cut.text;
      setDraft((last) => ({ text, seq: (last?.seq ?? 0) + 1 }));
      refreshContext();
      return files;
    },
    [turn.status, turn.blocks, keepTodos, keepPlan, refreshContext],
  );

  /**
   * The user's own edit to the plan. Saved at once when the chat exists —
   * the next turn is sent this version, and so is the file. A plan typed
   * into a chat that has not started yet is saved with its first turn.
   */
  const editPlan = useCallback(
    (next: string) => {
      const value = next.trim() ? next : null;
      keepPlan(value);
      if (!chatId || turn.status === "running") return;
      saveChat(chatId, history.current, turn.blocks, todos.current, value, branchedFrom.current).catch((e) => setError(String(e)));
    },
    [chatId, turn.status, turn.blocks, keepPlan],
  );

  return {
    turn,
    chatId,
    error,
    context,
    send,
    review,
    queued,
    queue,
    unqueue,
    decide,
    cancel,
    open,
    reset,
    compact: makeRoom,
    plan,
    planWritten,
    editPlan,
    checklist,
    branch,
    branchable,
    previewRewind,
    rewind,
    draft,
  };
}

/** What the transcript says where a rewind cut it. */
export function rewoundNotice(files: FileRewind[]): string {
  const back = files.filter((file) => file.skip === null).length;
  const left = files.length - back;
  const plural = (n: number) => `${n} ${n === 1 ? "file" : "files"}`;
  if (files.length === 0) return "Rewound to before this message — no files to put back";
  if (left === 0) return `Rewound to before this message — ${plural(back)} put back`;
  return `Rewound to before this message — ${plural(back)} put back, ${plural(left)} left as ${left === 1 ? "it is" : "they are"}`;
}
