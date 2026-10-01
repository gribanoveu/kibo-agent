import {
  type ChatUsage,
  type Checkpoint,
  type ContextUsage,
  type FileChange,
  type McpAnswer,
  type McpQuestion,
  type Outcome,
  type PendingToolCall,
  type ProcessInfo,
  type TurnEvent,
} from "./chat";

// The transcript, assembled from one ordered stream of events.
//
// Pure and free of React on purpose: what a turn looks like is decided here,
// where it can be tested by feeding it events, and the components only render
// what comes out. The block shapes follow the prototype
// (`docs/prototype/atlas-cli.html`): a user bubble, the agent's prose, a list
// of tool items with an expandable detail, and an approval card.

export type ToolStatus = "running" | "done" | "failed";

export type Block =
  | {
      kind: "user";
      id: string;
      /** What the user typed, and what the transcript shows. */
      text: string;
      /** What the model was sent instead, when that differs — a `/` command's
          prompt. The transcript shows `/init`, not the page it stands for. */
      sent?: string;
      /** How long the agent worked on this message, in ms — its own time,
          not the time spent waiting on an approval. Saved with the chat. */
      workedMs?: number;
    }
  | { kind: "message"; id: string; round: number; text: string }
  | { kind: "reasoning"; id: string; round: number; text: string }
  | { kind: "steer"; id: string; text: string }
  /** Something the app did to the conversation, said out loud. */
  | { kind: "notice"; id: string; text: string }
  /** A pass folding older history into a summary: under way, done, or given up on. */
  | { kind: "compaction"; id: string; status: "running" | "done" | "failed"; folded?: number }
  /** A background process ended — what the model was told at its round, drawn as a card. */
  | { kind: "processEnded"; id: string; process: ProcessInfo }
  | {
      kind: "tool";
      id: string;
      round: number;
      name: string;
      /** Raw JSON. Turning it into something readable is the renderer's job. */
      arguments: string;
      status: ToolStatus;
      result?: unknown;
      error?: string;
      /** Live output of a running command, appended as it arrives. */
      output: string;
      /** What the call did to files, kept for a rewind — absent when nothing. */
      changes?: FileChange[];
    }
  /** An MCP server asking the user something in the middle of call `call`. */
  | {
      kind: "question";
      id: string;
      round: number;
      call: string;
      server: string;
      question: McpQuestion;
      /** `open` until answered, or left when its call ended. */
      status: "open" | McpAnswer["action"];
    }
  | {
      kind: "approval";
      id: string;
      round: number;
      /** One card per call the round asked for, decided or not. */
      calls: PendingToolCall[];
    };

export type TurnStatus = "idle" | "running" | "awaitingApproval" | "done" | "cancelled";

export type TurnState = {
  blocks: Block[];
  status: TurnStatus;
  /** The last sequence number applied. Everything at or below it is a replay. */
  lastSeq: number;
  /** Events that arrived early, kept until the gap in front of them is filled. */
  buffered: TurnEvent[];
  usage: ChatUsage | null;
  /** Tokens the turn's requests have spent so far, in and out, every round's summed; a pause does not reset it. */
  spent: number;
  /** What the running turn's next request costs, said before each round; `null` until it says. */
  estimate: ContextUsage | null;
  retrying: { attempt: number; maxAttempts: number; delaySeconds: number } | null;
  /** Set when the turn pauses; sent back verbatim to continue it. */
  checkpoint: Checkpoint | null;
  /** When the agent last started working (ms since the epoch); `null` while it is not. */
  runningSince: number | null;
  /** When the round under way streamed its first token; `null` before it has. */
  streamSince: number | null;
  /** Characters the round under way has streamed: text, reasoning, call arguments. */
  streamChars: number;
  /** The provider's output speed: the last round's tokens per second, and the
      session's tokens, streaming time and characters it averages over. */
  speed: { last: number | null; tokens: number; ms: number; chars: number };
};

export const emptyTurn = (): TurnState => ({
  blocks: [],
  status: "idle",
  lastSeq: 0,
  buffered: [],
  usage: null,
  spent: 0,
  estimate: null,
  retrying: null,
  checkpoint: null,
  runningSince: null,
  streamSince: null,
  streamChars: 0,
  speed: { last: null, tokens: 0, ms: 0, chars: 0 },
});

/**
 * The round under way's speed while it streams, an estimate: the provider
 * counts tokens only at the end. Characters are turned into tokens at the
 * ratio this session's finished rounds had — which follows the model's
 * tokenizer and the language — or a guess before there is one. `null` for
 * the first second, when a few characters say nothing.
 */
export function liveSpeed(state: TurnState, now: number): number | null {
  if (state.streamSince === null || now - state.streamSince < 1000) return null;
  const { chars, tokens } = state.speed;
  const perToken = chars > 0 && tokens > 0 ? chars / tokens : 3.5;
  return (state.streamChars / perToken) * (1000 / (now - state.streamSince));
}

/**
 * Stops the clock: the time since `runningSince` is added to the message that
 * started the turn. Every way a stretch of work ends goes through here — a
 * pause for approval, the end, a failure.
 */
function stopClock(state: TurnState, now: number): TurnState {
  if (state.runningSince === null) return state;
  const spent = Math.max(0, now - state.runningSince);
  const at = state.blocks.map((b) => b.kind).lastIndexOf("user");
  const blocks = state.blocks.map((block, i) =>
    i === at && block.kind === "user" ? { ...block, workedMs: (block.workedMs ?? 0) + spent } : block,
  );
  return { ...state, blocks, runningSince: null };
}

/** The turn ended without an outcome — it failed on the way. */
export function endTurn(state: TurnState, now = Date.now()): TurnState {
  return { ...compactionEnded(stopClock(state, now), null), status: "done" };
}

/** A summary of the older history has been asked for: a card, under way. */
export function compactionStarted(state: TurnState): TurnState {
  return {
    ...state,
    blocks: [...state.blocks, { kind: "compaction", id: `compaction:${state.blocks.length}`, status: "running" }],
  };
}

/**
 * How the pass under way ended: `folded` messages summarized, or `null` —
 * it gave up (no summary came back, the request failed, the turn ended).
 *
 * With no card under way, a pass that folded something still gets one — its
 * start may never have been seen — and one that did not is nothing to show.
 */
export function compactionEnded(state: TurnState, end: { folded: number } | null): TurnState {
  const at = state.blocks.map((b) => b.kind === "compaction" && b.status === "running").lastIndexOf(true);
  if (at < 0) {
    if (!end) return state;
    return {
      ...state,
      blocks: [...state.blocks, { kind: "compaction", id: `compaction:${state.blocks.length}`, status: "done", folded: end.folded }],
    };
  }
  const blocks = state.blocks.map((block, i) =>
    i === at && block.kind === "compaction"
      ? { ...block, status: end ? ("done" as const) : ("failed" as const), folded: end?.folded }
      : block,
  );
  return { ...state, blocks };
}

/** `sent` is what the model gets when it is not `text` — see the user block. */
export function appendUserMessage(state: TurnState, text: string, now = Date.now(), sent?: string): TurnState {
  return {
    ...state,
    status: "running",
    runningSince: now,
    // A fresh turn numbers its events from one again — `seq` is a cursor
    // within a turn, not within the conversation. Carrying the previous
    // turn's cursor over would make every event of this one look like a
    // replay, and the answer would never appear.
    lastSeq: 0,
    buffered: [],
    // The last turn's figure; the window's own, read as this one starts, is newer.
    estimate: null,
    spent: 0,
    blocks: [
      ...state.blocks,
      { kind: "user", id: `user:${state.blocks.length}`, text, ...(sent !== undefined && sent !== text && { sent }) },
    ],
  };
}

/** Says what the app did, in the place where it happened. */
export function appendNotice(state: TurnState, text: string): TurnState {
  return {
    ...state,
    blocks: [...state.blocks, { kind: "notice", id: `notice:${state.blocks.length}`, text }],
  };
}

/** What a hook did, in the words of what it changed. */
function hookNotice({ event, message, blocked }: { event: string; message: string; blocked: boolean }): string {
  if (!blocked) return `${event} hook: ${message}`;
  switch (event) {
    case "PreToolUse":
      return `A hook refused the call: ${message}`;
    case "PostToolUse":
      return `A hook, after the call: ${message}`;
    case "Stop":
      return `A Stop hook sent the agent back: ${message}`;
    default:
      return `${event} hook: ${message}`;
  }
}

/** A conversation reopened from disk: its transcript, and nothing in flight. */
export function restoredTurn(blocks: Block[]): TurnState {
  return { ...emptyTurn(), blocks, status: "done" };
}

/**
 * Applies one event, in order.
 *
 * Three things can go wrong with a stream that crosses a process boundary, and
 * all three are handled here rather than by whoever renders it: an event can
 * arrive twice (dropped), early (buffered until its predecessor lands), or —
 * for command output alone — with no sequence number at all.
 *
 * That last one is not an oversight: command output is written from the
 * runner's reader threads, where the turn's cursor does not exist. It belongs
 * to the call named in its payload and is ordered against nothing. An MCP
 * server's question is sent from inside its call too, and is keyed by its id.
 */
export function acceptEvent(state: TurnState, event: TurnEvent, now = Date.now()): TurnState {
  if (event.type === "commandOutput" || event.type === "mcpQuestion" || event.type === "mcpQuestionClosed") {
    return applyEvent(state, event, now);
  }

  if (event.seq <= state.lastSeq) return state;
  if (event.seq > state.lastSeq + 1) {
    return { ...state, buffered: [...state.buffered, event] };
  }

  let next = applyEvent({ ...state, lastSeq: event.seq }, event, now);

  // The gap is filled; anything that was waiting on it may now apply, in order.
  let progressed = true;
  while (progressed) {
    progressed = false;
    const ready = next.buffered.find((e) => e.seq === next.lastSeq + 1);
    if (ready) {
      next = applyEvent(
        { ...next, lastSeq: ready.seq, buffered: next.buffered.filter((e) => e !== ready) },
        ready,
        now,
      );
      progressed = true;
    }
  }
  return next;
}

/** What the turn's own outcome adds: the pause, or the end. */
export function acceptOutcome(state: TurnState, outcome: Outcome, now = Date.now()): TurnState {
  state = stopClock(state, now);
  if (outcome.status === "pendingApproval") {
    const checkpoint = outcome.value;
    return {
      ...state,
      status: "awaitingApproval",
      checkpoint,
      blocks: [
        ...state.blocks,
        {
          kind: "approval",
          id: `approval:${checkpoint.round}`,
          round: checkpoint.round,
          calls: checkpoint.calls,
        },
      ],
    };
  }
  const ended: TurnState = {
    ...compactionEnded(state, null),
    status: outcome.status === "cancelled" ? "cancelled" : "done",
    checkpoint: null,
    retrying: null,
  };
  const rounds = outcome.value.limitReached;
  return rounds
    ? appendNotice(
        ended,
        `The turn stopped at its limit after ${rounds} rounds. Its work is kept — say "continue" to go on from here, or raise the limits in Settings → Agent.`,
      )
    : ended;
}

/** Removes the pause once it has been answered, so the card does not linger. */
export function clearApproval(state: TurnState, now = Date.now()): TurnState {
  return {
    ...state,
    status: "running",
    runningSince: now,
    checkpoint: null,
    blocks: state.blocks.filter((block) => block.kind !== "approval"),
  };
}

/**
 * Times the round's streaming, from its first token to its usage: time spent
 * on the prompt before the first token is the provider reading, not sending.
 */
function timeStream(state: TurnState, event: TurnEvent, now: number): TurnState {
  switch (event.type) {
    case "roundStarted":
      return { ...state, streamSince: null, streamChars: 0 };
    case "delta":
    case "reasoning":
      return streamed(state, event.payload.delta.length, now);
    case "toolCallDelta": {
      // A call's arguments arrive whole each time: what is new is the growth.
      const { id, arguments: args } = event.payload;
      const before = state.blocks.find((b) => b.kind === "tool" && b.id === id);
      return streamed(state, Math.max(0, args.length - (before?.kind === "tool" ? before.arguments.length : 0)), now);
    }
    case "contextUsage": {
      const ms = state.streamSince === null ? 0 : now - state.streamSince;
      const tokens = event.payload.completionTokens;
      const ended = { ...state, streamSince: null, streamChars: 0 };
      // An answer that arrived in one piece has no streaming time to divide by.
      if (ms < 250 || tokens <= 0) return ended;
      const { speed } = state;
      return {
        ...ended,
        speed: {
          last: (tokens * 1000) / ms,
          tokens: speed.tokens + tokens,
          ms: speed.ms + ms,
          chars: speed.chars + state.streamChars,
        },
      };
    }
    default:
      return state;
  }
}

function streamed(state: TurnState, chars: number, now: number): TurnState {
  return { ...state, streamSince: state.streamSince ?? now, streamChars: state.streamChars + chars };
}

function applyEvent(state: TurnState, event: TurnEvent, now: number): TurnState {
  state = timeStream(state, event, now);
  switch (event.type) {
    case "roundStarted":
      // Nothing to add — but the next prose must not join the previous round's
      // paragraph, so a fresh block is opened by `appendText` keying on the
      // round. Stated as an event rather than inferred, because a round that
      // ended in prose and was followed by another had its two answers
      // concatenated mid-sentence, permanently.
      return { ...state, retrying: null };

    case "delta":
      return appendText(state, "message", event.round, event.payload.delta);

    case "reasoning":
      return appendText(state, "reasoning", event.round, event.payload.delta);

    case "roundCompleted":
      // The authoritative text, replacing whatever the deltas built: a delta
      // lost on the way here is permanent once the transcript is saved.
      return setText(state, "message", event.round, event.payload.text);

    case "historyCompacting":
      return compactionStarted(state);

    case "historyCompacted":
      // The transcript keeps every message; it is the model's copy that got
      // shorter. Saying so is the whole point — history that disappears on
      // its own looks like the agent forgetting for no reason.
      return compactionEnded(state, { folded: event.payload.folded });

    case "hookFeedback":
      return appendNotice(state, hookNotice(event.payload));

    case "wrapUpReminded":
      // A review that went on long was asked to finish with what it has.
      return appendNotice(
        state,
        `The review had spent ${event.payload.rounds} rounds and ~${Math.round(event.payload.tokens / 1000)}k tokens — the agent was asked to wrap up`,
      );

    case "loopReminded":
      // The model was told it is going round in circles; the reader sees why
      // the next round changes course, or why it should have.
      return appendNotice(
        state,
        event.payload.failing
          ? `${event.payload.tool} kept failing the same way — the agent was told to find the cause or change route`
          : `${event.payload.tool} kept returning the same result — the agent was told to stop repeating it`,
      );

    case "processesEnded":
      // What the model was just told, said to the reader too.
      return {
        ...state,
        blocks: [
          ...state.blocks,
          ...event.payload.processes.map((process, at) => ({
            kind: "processEnded" as const,
            id: `ended:${state.blocks.length + at}`,
            process,
          })),
        ],
      };

    case "steeringApplied":
      return {
        ...state,
        blocks: [
          ...state.blocks,
          { kind: "steer", id: `steer:${event.payload.id}`, text: event.payload.text },
        ],
      };

    case "toolCallDelta":
    case "toolCall":
      return upsertTool(state, event.round, event.payload.id, (block) => ({
        ...block,
        name: event.payload.name || block.name,
        arguments: event.payload.arguments,
      }));

    case "toolResult":
      return upsertTool(state, event.round, event.payload.id, (block) => ({
        ...block,
        status: event.payload.error ? "failed" : "done",
        result: event.payload.result ?? undefined,
        error: event.payload.error ?? undefined,
        ...(event.payload.changes?.length ? { changes: event.payload.changes } : {}),
      }));

    case "commandOutput":
      return upsertTool(state, event.round, event.payload.id, (block) => ({
        ...block,
        output: block.output + event.payload.chunk,
      }));

    case "mcpQuestion": {
      const { id, call, server, question } = event.payload;
      if (state.blocks.some((block) => block.kind === "question" && block.id === id)) return state;
      return { ...state, blocks: [...state.blocks, { kind: "question", id, round: event.round, call, server, question, status: "open" }] };
    }

    case "mcpQuestionClosed":
      return {
        ...state,
        blocks: state.blocks.map((block) =>
          block.kind === "question" && block.id === event.payload.id ? { ...block, status: event.payload.action } : block,
        ),
      };

    case "contextUsage":
      return {
        ...state,
        usage: event.payload,
        spent: state.spent + event.payload.promptTokens + event.payload.completionTokens,
      };

    case "contextEstimate":
      return { ...state, estimate: event.payload };

    case "retrying":
      return { ...state, retrying: event.payload };

    default:
      return state;
  }
}

function appendText(
  state: TurnState,
  kind: "message" | "reasoning",
  round: number,
  delta: string,
): TurnState {
  const id = textId(state, kind, round);
  const existing = blockWithId(state, id);
  if (!existing) {
    return { ...state, blocks: [...state.blocks, { kind, id, round, text: delta }] };
  }
  return replace(state, existing, { ...existing, text: existing.text + delta });
}

function setText(
  state: TurnState,
  kind: "message" | "reasoning",
  round: number,
  text: string,
): TurnState {
  const id = textId(state, kind, round);
  const existing = blockWithId(state, id);
  // A round that only called tools says nothing, and an empty block would draw
  // an empty paragraph in the transcript.
  if (!existing) {
    if (!text) return state;
    return { ...state, blocks: [...state.blocks, { kind, id, round, text }] };
  }
  return replace(state, existing, { ...existing, text });
}

/**
 * Where this round's prose goes — a round of *this* turn.
 *
 * Rounds are numbered from one inside every turn, so "the message block of
 * round 1" names one block per question asked. Without the turn in the key,
 * the second question's first answer was appended to the first question's
 * paragraph, halfway up the transcript. The turn is counted rather than
 * stored: a conversation has had exactly as many turns as it has questions in
 * it, including the ones restored from disk.
 */
function textId(state: TurnState, kind: "message" | "reasoning", round: number) {
  const turn = state.blocks.filter((block) => block.kind === "user").length;
  return `turn:${turn}:round:${round}:${kind}`;
}

function blockWithId(state: TurnState, id: string) {
  return state.blocks.find(
    (block): block is Extract<Block, { kind: "message" | "reasoning" }> =>
      (block.kind === "message" || block.kind === "reasoning") && block.id === id,
  );
}

/**
 * Tool blocks are addressed by the model's own call id, so a result applies to
 * the call it answers however many other calls and rounds happen in between.
 */
function upsertTool(
  state: TurnState,
  round: number,
  id: string,
  update: (block: Extract<Block, { kind: "tool" }>) => Extract<Block, { kind: "tool" }>,
): TurnState {
  const existing = state.blocks.find(
    (block): block is Extract<Block, { kind: "tool" }> => block.kind === "tool" && block.id === id,
  );
  if (!existing) {
    const fresh: Extract<Block, { kind: "tool" }> = {
      kind: "tool",
      id,
      round,
      name: "",
      arguments: "",
      status: "running",
      output: "",
    };
    return { ...state, blocks: [...state.blocks, update(fresh)] };
  }
  return replace(state, existing, update(existing));
}

function replace(state: TurnState, before: Block, after: Block): TurnState {
  return {
    ...state,
    blocks: state.blocks.map((block) => (block === before ? after : block)),
  };
}
