import { describe, expect, test } from "bun:test";
import {
  acceptEvent,
  acceptOutcome,
  appendUserMessage,
  clearApproval,
  compactionEnded,
  compactionStarted,
  emptyTurn,
  endTurn,
  liveSpeed,
  restoredTurn,
  type Block,
  type TurnState,
} from "../lib/chatTurnReducer";
import type { Checkpoint, TurnEvent } from "../lib/chat";

// The reducer is where a stream that crossed a process boundary becomes a
// transcript. Everything that can go wrong with such a stream — a repeat, an
// early arrival, a lost delta — is a test here rather than a surprise in the
// window.

let seq = 0;
const ev = (event: Omit<TurnEvent, "turnId" | "seq" | "round"> & { round?: number; seq?: number }) =>
  ({
    turnId: "turn-1",
    seq: event.seq ?? ++seq,
    round: event.round ?? 1,
    ...event,
  }) as TurnEvent;

const run = (events: TurnEvent[], from: TurnState = emptyTurn()) =>
  events.reduce(acceptEvent, from);

const kinds = (state: TurnState) => state.blocks.map((b) => b.kind);
const text = (state: TurnState, kind: Block["kind"]) =>
  state.blocks.filter((b) => b.kind === kind).map((b) => (b as { text: string }).text);

describe("prose", () => {
  test("deltas build one paragraph per round", () => {
    seq = 0;
    const state = run([
      ev({ type: "roundStarted" }),
      ev({ type: "delta", payload: { delta: "Looking " } }),
      ev({ type: "delta", payload: { delta: "at the tax path." } }),
    ]);

    expect(text(state, "message")).toEqual(["Looking at the tax path."]);
  });

  /// Two rounds' answers used to be concatenated mid-sentence, permanently,
  /// because only a tool call could close a paragraph.
  test("a second round starts its own paragraph", () => {
    seq = 0;
    const state = run([
      ev({ type: "delta", payload: { delta: "first" }, round: 1 }),
      ev({ type: "roundStarted", round: 2 }),
      ev({ type: "delta", payload: { delta: "second" }, round: 2 }),
    ]);

    expect(text(state, "message")).toEqual(["first", "second"]);
  });

  /// The safety net: a delta lost on its way here is permanent once the
  /// transcript is saved, so the round's own authoritative text wins.
  test("the round's final text replaces what the deltas built", () => {
    seq = 0;
    const state = run([
      ev({ type: "delta", payload: { delta: "half a sen" } }),
      ev({ type: "roundCompleted", payload: { text: "half a sentence, whole again" } }),
    ]);

    expect(text(state, "message")).toEqual(["half a sentence, whole again"]);
  });

  test("a round that only called tools draws no empty paragraph", () => {
    seq = 0;
    const state = run([
      ev({ type: "roundStarted" }),
      ev({ type: "roundCompleted", payload: { text: "" } }),
    ]);

    expect(kinds(state)).toEqual([]);
  });

  test("thinking is kept apart from the answer", () => {
    seq = 0;
    const state = run([
      ev({ type: "reasoning", payload: { delta: "hmm" } }),
      ev({ type: "delta", payload: { delta: "answer" } }),
    ]);

    expect(text(state, "reasoning")).toEqual(["hmm"]);
    expect(text(state, "message")).toEqual(["answer"]);
  });
});

describe("tool calls", () => {
  const call = { id: "c1", name: "readFile", arguments: '{"path":"a.rs"}' };

  test("a call and its result are one block, paired by id", () => {
    seq = 0;
    const state = run([
      ev({ type: "toolCall", payload: call }),
      ev({ type: "toolResult", payload: { id: "c1", result: { content: "x" } } }),
    ]);

    expect(state.blocks).toHaveLength(1);
    const tool = state.blocks[0] as Extract<Block, { kind: "tool" }>;
    expect(tool.name).toBe("readFile");
    expect(tool.status).toBe("done");
  });

  test("arguments still arriving are appended, and the call itself replaces them", () => {
    seq = 0;
    const deltas = [
      ev({ type: "toolCallDelta", payload: { id: "c1", name: "writeFile", arguments: '{"pa' } }),
      ev({ type: "toolCallDelta", payload: { id: "c1", name: "", arguments: 'th":"a.rs"}' } }),
    ];
    const streaming = run(deltas);
    const tool = streaming.blocks[0] as Extract<Block, { kind: "tool" }>;
    expect(tool.arguments).toBe('{"path":"a.rs"}');
    expect(tool.name).toBe("writeFile");
    expect(streaming.blocks).toHaveLength(1);

    const called = run([...deltas, ev({ type: "toolCall", payload: { id: "c1", name: "writeFile", arguments: '{"path":"b.rs"}' } })]);
    expect((called.blocks[0] as Extract<Block, { kind: "tool" }>).arguments).toBe('{"path":"b.rs"}');
  });

  test("a failed call says so and carries the message the model got", () => {
    seq = 0;
    const state = run([
      ev({ type: "toolCall", payload: call }),
      ev({ type: "toolResult", payload: { id: "c1", error: "Error: not found: a.rs" } }),
    ]);

    const tool = state.blocks[0] as Extract<Block, { kind: "tool" }>;
    expect(tool.status).toBe("failed");
    expect(tool.error).toContain("not found");
  });

  test("two calls in one round stay separate", () => {
    seq = 0;
    const state = run([
      ev({ type: "toolCall", payload: call }),
      ev({ type: "toolCall", payload: { id: "c2", name: "grep", arguments: "{}" } }),
      ev({ type: "toolResult", payload: { id: "c2", result: {} } }),
    ]);

    expect(state.blocks).toHaveLength(2);
    expect((state.blocks[0] as { status: string }).status).toBe("running");
    expect((state.blocks[1] as { status: string }).status).toBe("done");
  });
});

describe("command output", () => {
  /// It carries no sequence number at all — it is written from the runner's
  /// reader threads, where the turn's cursor does not exist. Putting it through
  /// the ordering would hold every chunk back forever.
  test("arrives unordered and still lands on its call", () => {
    seq = 0;
    const state = run([
      ev({ type: "toolCall", payload: { id: "c1", name: "runCommand", arguments: "{}" } }),
      ev({ type: "commandOutput", payload: { id: "c1", stream: "stdout", chunk: "BUILD " }, seq: 0 }),
      ev({ type: "commandOutput", payload: { id: "c1", stream: "stdout", chunk: "OK\n" }, seq: 0 }),
    ]);

    const tool = state.blocks[0] as Extract<Block, { kind: "tool" }>;
    expect(tool.output).toBe("BUILD OK\n");
    expect(tool.status).toBe("running");
  });

  test("and does not disturb the ordering of everything else", () => {
    seq = 0;
    const state = run([
      ev({ type: "toolCall", payload: { id: "c1", name: "runCommand", arguments: "{}" }, seq: 1 }),
      ev({ type: "commandOutput", payload: { id: "c1", stream: "stdout", chunk: "x" }, seq: 0 }),
      ev({ type: "toolResult", payload: { id: "c1", result: {} }, seq: 2 }),
    ]);

    expect((state.blocks[0] as { status: string }).status).toBe("done");
    expect(state.lastSeq).toBe(2);
  });
});

describe("a stream that crossed a process boundary", () => {
  test("a repeated event changes nothing", () => {
    seq = 0;
    const first = ev({ type: "delta", payload: { delta: "hi" } });
    const state = run([first, first, first]);

    expect(text(state, "message")).toEqual(["hi"]);
  });

  /// A listener that reconnects mid-turn can see events out of order. Applying
  /// them as they land would interleave the transcript.
  test("an early event waits for the gap in front of it", () => {
    seq = 0;
    let state = run([ev({ type: "delta", payload: { delta: "one " }, seq: 1 })]);

    state = acceptEvent(state, ev({ type: "delta", payload: { delta: "three" }, seq: 3 }));
    expect(text(state, "message")).toEqual(["one "]);
    expect(state.buffered).toHaveLength(1);

    state = acceptEvent(state, ev({ type: "delta", payload: { delta: "two " }, seq: 2 }));
    expect(text(state, "message")).toEqual(["one two three"]);
    expect(state.buffered).toHaveLength(0);
  });

  test("a whole run of early events drains in order once the gap closes", () => {
    seq = 0;
    let state = emptyTurn();
    for (const n of [4, 2, 3]) {
      state = acceptEvent(state, ev({ type: "delta", payload: { delta: `${n}` }, seq: n }));
    }
    state = acceptEvent(state, ev({ type: "delta", payload: { delta: "1" }, seq: 1 }));

    expect(text(state, "message")).toEqual(["1234"]);
    expect(state.lastSeq).toBe(4);
  });
});

describe("pausing", () => {
  const checkpoint: Checkpoint = {
    history: [],
    round: 2,
    budgetUsed: 4,
    eventSeq: 9,
    calls: [
      { id: "w1", name: "writeFile", arguments: '{"path":"a.rs"}', requiresConfirmation: true },
    ],
    todos: [],
    reads: {},
  };

  test("a pause adds the card and keeps the checkpoint to send back", () => {
    const state = acceptOutcome(emptyTurn(), { status: "pendingApproval", value: checkpoint });

    expect(state.status).toBe("awaitingApproval");
    expect(kinds(state)).toEqual(["approval"]);
    expect(state.checkpoint).toEqual(checkpoint);
  });

  test("a turn stopped at its limits ends with a notice saying its work is kept; a finished one adds none", () => {
    const result = { text: "", truncated: false, todos: [], history: [] };
    const stopped = acceptOutcome(emptyTurn(), { status: "done", value: { ...result, limitReached: 27 } });
    expect(stopped.status).toBe("done");
    expect(kinds(stopped)).toEqual(["notice"]);
    expect((stopped.blocks[0] as { text: string }).text).toContain("after 27 rounds");

    expect(kinds(acceptOutcome(emptyTurn(), { status: "done", value: result }))).toEqual([]);
  });

  test("what the turn's rounds spent adds up, and the next turn starts from nothing", () => {
    const usage = (promptTokens: number, completionTokens: number, cachedTokens: number) => ({
      promptTokens,
      completionTokens,
      totalTokens: promptTokens + completionTokens,
      cachedTokens,
    });
    let state = appendUserMessage(emptyTurn(), "go", 1_000);
    state = acceptEvent(state, { turnId: "t", seq: 1, round: 1, type: "contextUsage", payload: usage(1_000, 50, 0) });
    state = acceptEvent(state, { turnId: "t", seq: 2, round: 2, type: "contextUsage", payload: usage(1_200, 30, 900) });
    expect(state.spent).toBe(2_280);
    expect(appendUserMessage(state, "more", 2_000).spent).toBe(0);
  });

  test("a user message keeps its pictures, and one without has no field for them", () => {
    const picture = { mediaType: "image/jpeg" as const, data: "J", width: 3, height: 4 };
    const [withPictures] = appendUserMessage(emptyTurn(), "look", 1, undefined, [picture]).blocks;
    expect(withPictures).toMatchObject({ kind: "user", text: "look", images: [picture] });
    const [without] = appendUserMessage(emptyTurn(), "look", 1).blocks;
    expect("images" in without).toBe(false);
  });

  test("the estimate a round sends is kept until the next turn starts", () => {
    const estimate = { instructions: 1, skills: 0, tools: 2, mcp: 0, conversation: 7, total: 10, limit: 100, compactsAt: 80 };
    let state = appendUserMessage(emptyTurn(), "go", 1_000);
    state = acceptEvent(state, { turnId: "t", seq: 1, round: 1, type: "contextEstimate", payload: estimate });
    expect(state.estimate).toEqual(estimate);
    expect(appendUserMessage(state, "more", 2_000).estimate).toBeNull();
  });

  test("answering it clears the card", () => {
    let state = acceptOutcome(emptyTurn(), { status: "pendingApproval", value: checkpoint });
    state = clearApproval(state);

    expect(kinds(state)).toEqual([]);
    expect(state.checkpoint).toBeNull();
    expect(state.status).toBe("running");
  });

  /// Like Claude Code's "Worked for": the agent's own time. Waiting on the
  /// user's answer is not work, and a failure still ends the clock.
  test("the time worked is kept on the message, without the wait for an answer", () => {
    const result = { text: "", truncated: false, todos: [] };
    const worked = (state: TurnState) => (state.blocks[0] as { workedMs?: number }).workedMs;

    let state = appendUserMessage(emptyTurn(), "fix it", 1_000);
    expect(state.runningSince).toBe(1_000);
    state = acceptOutcome(state, { status: "pendingApproval", value: checkpoint }, 4_000);
    expect(state.runningSince).toBeNull();
    state = clearApproval(state, 60_000);
    state = acceptOutcome(state, { status: "done", value: result }, 62_000);
    expect(worked(state)).toBe(5_000);

    const failed = endTurn(appendUserMessage(emptyTurn(), "again", 0), 7_000);
    expect([failed.status, worked(failed)]).toEqual(["done", 7_000]);
  });

  test("an ending turn is done, a stopped one is cancelled", () => {
    const result = { text: "", truncated: false, todos: [] };
    expect(acceptOutcome(emptyTurn(), { status: "done", value: result }).status).toBe("done");
    expect(acceptOutcome(emptyTurn(), { status: "cancelled", value: result }).status).toBe(
      "cancelled",
    );
  });
});

describe("the rest of the turn's state", () => {
  test("usage is the whole context, not a per-round statistic", () => {
    seq = 0;
    const state = run([
      ev({
        type: "contextUsage",
        payload: { promptTokens: 16000, completionTokens: 200, totalTokens: 16200 },
      }),
    ]);

    expect(state.usage?.totalTokens).toBe(16200);
  });

  /// A wait with no visible reason is indistinguishable from a hang.
  test("a retry is visible while it waits and gone once the round restarts", () => {
    seq = 0;
    let state = run([
      ev({ type: "retrying", payload: { attempt: 1, maxAttempts: 5, delaySeconds: 20 } }),
    ]);
    expect(state.retrying?.delaySeconds).toBe(20);

    state = acceptEvent(state, ev({ type: "roundStarted" }));
    expect(state.retrying).toBeNull();
  });

  test("a mid-turn note of the user's own shows in the transcript", () => {
    seq = 0;
    const state = run([
      ev({ type: "steeringApplied", payload: { id: "n1", text: "use the helper" } }),
    ]);

    expect(kinds(state)).toEqual(["steer"]);
  });

  test("the user's own message opens the turn", () => {
    const state = appendUserMessage(emptyTurn(), "fix the NPE");
    expect(kinds(state)).toEqual(["user"]);
    expect(state.status).toBe("running");
  });
});

describe("more than one turn", () => {
  /// Each turn numbers its own events from one. Keeping the previous turn's
  /// cursor made every event of the next one look like a repeat, and the
  /// second answer to a conversation never appeared at all.
  test("a new question starts the sequence over", () => {
    const first = run([
      ev({ type: "roundStarted", seq: 1 }),
      ev({ type: "delta", seq: 2, payload: { delta: "first answer" } }),
    ]);
    expect(first.lastSeq).toBe(2);

    const second = run(
      [
        ev({ type: "roundStarted", seq: 1 }),
        ev({ type: "delta", seq: 2, payload: { delta: "second answer" } }),
      ],
      appendUserMessage(first, "and again"),
    );

    expect(second.blocks.filter((b) => b.kind === "message")).toHaveLength(2);
    expect(second.blocks.at(-1)).toMatchObject({ kind: "message", text: "second answer" });
  });

  /// Not the same as a fresh question: a resumed turn carries on from the
  /// checkpoint's own count, so its cursor must survive the pause.
  test("answering an approval keeps the cursor", () => {
    const paused = { ...emptyTurn(), lastSeq: 9, status: "awaitingApproval" as const };
    expect(clearApproval(paused).lastSeq).toBe(9);
  });
});

describe("reopening a saved chat", () => {
  test("the transcript comes back at rest, with nothing in flight", () => {
    const blocks: Block[] = [
      { kind: "user", id: "user:0", text: "earlier" },
      { kind: "message", id: "m1", round: 1, text: "answered" },
    ];
    const state = restoredTurn(blocks);

    expect(state.blocks).toEqual(blocks);
    expect(state.status).toBe("done");
    expect(state.checkpoint).toBeNull();
    expect(state.lastSeq).toBe(0);
    expect(state.buffered).toEqual([]);
  });
});

describe("compaction", () => {
  /// The summary takes seconds: the card is there while it is made, and the
  /// same card says how it ended rather than a second one appearing.
  test("a pass is a card, under way and then done", () => {
    const started = run([ev({ type: "historyCompacting", seq: 1 })]);
    expect(started.blocks).toEqual([{ kind: "compaction", id: "compaction:0", status: "running" }]);

    const done = run([ev({ type: "historyCompacted", seq: 2, payload: { folded: 12 } })], started);
    expect(done.blocks).toEqual([{ kind: "compaction", id: "compaction:0", status: "done", folded: 12 }]);
  });

  /// The model quietly forgetting what it was told, with nothing in the
  /// window to explain it, is the outcome this exists to prevent.
  test("a fold whose start was not seen is still said", () => {
    const state = run([ev({ type: "historyCompacted", seq: 1, payload: { folded: 3 } })]);
    expect(state.blocks).toEqual([{ kind: "compaction", id: "compaction:0", status: "done", folded: 3 }]);
  });

  /// A pass that gave up leaves no card spinning, however the turn ended.
  test("a pass under way when the turn ends gave up", () => {
    const running = appendUserMessage(emptyTurn(), "go");
    const compacting = run([ev({ type: "historyCompacting", seq: 1 })], running);
    const failed = { kind: "compaction", id: "compaction:1", status: "failed" };

    expect(endTurn(compacting).blocks[1]).toEqual(failed);
    expect(
      acceptOutcome(compacting, { status: "done", value: { text: "", truncated: false, todos: [], history: [] } })
        .blocks[1],
    ).toEqual(failed);
  });

  test("ending with nothing folded and no card shows nothing", () => {
    expect(compactionEnded(emptyTurn(), null).blocks).toEqual([]);
    expect(compactionEnded(compactionStarted(emptyTurn()), null).blocks).toEqual([
      { kind: "compaction", id: "compaction:0", status: "failed" },
    ]);
  });
});

describe("hooks", () => {
  const said = (payload: { event: string; message: string; blocked: boolean }) =>
    (run([ev({ type: "hookFeedback", seq: 1, payload })]).blocks[0] as { kind: string; text: string });

  /// What a hook did is said in terms of what it changed: a refused call,
  /// a note on a finished one, a turn sent back — or only that it failed.
  test("each refusal says what it changed, and a failure is only a failure", () => {
    expect(said({ event: "PreToolUse", message: "no prod", blocked: true })).toMatchObject({
      kind: "notice",
      text: "A hook refused the call: no prod",
    });
    expect(said({ event: "PostToolUse", message: "lint: 2", blocked: true }).text).toBe("A hook, after the call: lint: 2");
    expect(said({ event: "Stop", message: "run tests", blocked: true }).text).toBe("A Stop hook sent the agent back: run tests");
    expect(said({ event: "PreToolUse", message: "jq: not found", blocked: false }).text).toBe(
      "PreToolUse hook: jq: not found",
    );
  });
});

describe("file changes", () => {
  /// Kept on the block, so the chat saves them and a rewind can find them.
  test("what a call did to files stays on its block; nothing is kept when nothing changed", () => {
    const change = { path: "a.rs", before: { kind: "absent" as const }, after: { kind: "stored" as const, hash: "h" } };
    const state = run([
      ev({ type: "toolCall", seq: 1, payload: { id: "w", name: "writeFile", arguments: "{}" } }),
      ev({ type: "toolResult", seq: 2, payload: { id: "w", result: {}, changes: [change] } }),
      ev({ type: "toolCall", seq: 3, payload: { id: "r", name: "readFile", arguments: "{}" } }),
      ev({ type: "toolResult", seq: 4, payload: { id: "r", result: {}, changes: [] } }),
    ]);
    const [written, read] = state.blocks as Extract<Block, { kind: "tool" }>[];
    expect(written.changes).toEqual([change]);
    expect("changes" in read).toBe(false);
  });
});

describe("the loop guard", () => {
  test("a reminder to the model is a notice naming the tool and what it kept doing", () => {
    const state = run([
      ev({ type: "loopReminded", seq: 1, payload: { tool: "editFile", failing: true } }),
      ev({ type: "loopReminded", seq: 2, payload: { tool: "readFile", failing: false } }),
    ]);
    expect(kinds(state)).toEqual(["notice", "notice"]);
    expect(text(state, "notice")).toEqual([
      "editFile kept failing the same way — the agent was told to find the cause or change route",
      "readFile kept returning the same result — the agent was told to stop repeating it",
    ]);
  });
});

describe("background processes", () => {
  /// The model was told at the start of its round; the reader sees the same.
  test("each ended process is a block of its own, carrying the process", () => {
    const state = run([
      ev({
        type: "processesEnded",
        seq: 1,
        payload: {
          processes: [
            { id: 2, command: "npm run dev", cwd: ".", state: { state: "exited", code: 1 } },
            { id: 3, command: "tail -f log", cwd: ".", state: { state: "stopped" } },
          ],
        },
      }),
    ]);
    expect(state.blocks).toEqual([
      {
        kind: "processEnded",
        id: "ended:0",
        process: { id: 2, command: "npm run dev", cwd: ".", state: { state: "exited", code: 1 } },
      },
      {
        kind: "processEnded",
        id: "ended:1",
        process: { id: 3, command: "tail -f log", cwd: ".", state: { state: "stopped" } },
      },
    ]);
  });
});

describe("a user message", () => {
  test("keeps what the model was sent only when it is not what is shown", () => {
    const command = appendUserMessage(emptyTurn(), "/init", 0, "Study this repository");
    expect(command.blocks[0]).toEqual({ kind: "user", id: "user:0", text: "/init", sent: "Study this repository" });

    const typed = appendUserMessage(emptyTurn(), "hello", 0, "hello");
    expect(typed.blocks[0]).toEqual({ kind: "user", id: "user:0", text: "hello" });
  });
});

describe("an MCP server's question", () => {
  const question = { mode: "form" as const, message: "Which repository?", fields: [] };
  const asked = (id: string) =>
    ev({ type: "mcpQuestion", seq: 0, payload: { id, call: "c1", server: "gh", question } });

  test("arrives outside the sequence as a card of its own, once, and closes with the answer", () => {
    let state = acceptEvent(emptyTurn(), ev({ type: "roundStarted", seq: 1 }));
    state = acceptEvent(state, asked("q1"));
    state = acceptEvent(state, asked("q1"));
    const cards = state.blocks.filter((b): b is Extract<Block, { kind: "question" }> => b.kind === "question");
    expect(cards).toHaveLength(1, "a repeat is not a second question");
    expect(cards[0]).toMatchObject({ id: "q1", call: "c1", server: "gh", status: "open", question });
    expect(state.lastSeq).toBe(1, "the sequence is not moved by it");

    state = acceptEvent(state, ev({ type: "mcpQuestionClosed", seq: 0, payload: { id: "q1", action: "decline" } }));
    expect(state.blocks.find((b) => b.kind === "question")).toMatchObject({ status: "decline" });
  });

  test("is not held back behind a gap in the sequence", () => {
    const early = acceptEvent(emptyTurn(), ev({ type: "delta", seq: 5, payload: { delta: "x" } }));
    const state = acceptEvent(early, asked("q2"));
    expect(state.blocks.some((b) => b.kind === "question")).toBe(true);
  });
});

describe("speed", () => {
  test("tokens per second run from a round's first token to its usage, and average over the session", () => {
    seq = 0;
    const usage = (completionTokens: number) => ({ promptTokens: 1000, completionTokens, totalTokens: 1000 + completionTokens });
    const at: [TurnEvent, number][] = [
      [ev({ type: "roundStarted" }), 0],
      // The wait for the first token is the provider reading the prompt, not counted.
      [ev({ type: "reasoning", payload: { delta: "hm" } }), 5_000],
      [ev({ type: "delta", payload: { delta: "hi" } }), 6_000],
      [ev({ type: "contextUsage", payload: usage(100) }), 7_000],
      [ev({ type: "roundStarted", round: 2 }), 7_100],
      [ev({ type: "toolCallDelta", round: 2, payload: { id: "c", name: "read", arguments: "" } }), 8_000],
      [ev({ type: "contextUsage", round: 2, payload: usage(20) }), 9_000],
      // An answer that came in one piece is not timed.
      [ev({ type: "roundStarted", round: 3 }), 9_100],
      [ev({ type: "delta", round: 3, payload: { delta: "all" } }), 10_000],
      [ev({ type: "contextUsage", round: 3, payload: usage(500) }), 10_010],
    ];
    const state = at.reduce((s, [event, now]) => acceptEvent(s, event, now), emptyTurn());
    // "hm" + "hi" in round one, nothing counted from the one-piece round three.
    expect(state.speed).toEqual({ last: 20, tokens: 120, ms: 3_000, chars: 4 });
  });

  test("while a round streams, its speed is estimated from characters at the session's ratio", () => {
    seq = 0;
    const usage = { promptTokens: 1000, completionTokens: 100, totalTokens: 1100 };
    const at: [TurnEvent, number][] = [
      [ev({ type: "roundStarted" }), 0],
      [ev({ type: "delta", payload: { delta: "x".repeat(400) } }), 1_000],
      [ev({ type: "contextUsage", payload: usage }), 2_000],
      [ev({ type: "roundStarted", round: 2 }), 2_100],
      // Arguments come as what is new: 30 characters, then 30 more.
      [ev({ type: "toolCallDelta", round: 2, payload: { id: "c", name: "read", arguments: "a".repeat(30) } }), 3_000],
      [ev({ type: "toolCallDelta", round: 2, payload: { id: "c", name: "read", arguments: "a".repeat(30) } }), 3_500],
      [ev({ type: "delta", round: 2, payload: { delta: "y".repeat(60) } }), 4_000],
    ];
    const state = at.reduce((s, [event, now]) => acceptEvent(s, event, now), emptyTurn());
    // Round one: 400 characters were 100 tokens, so 120 characters are 30 tokens, over 2 s.
    expect(liveSpeed(state, 5_000)).toBe(15);
    // Too early to say anything.
    expect(liveSpeed(state, 3_900)).toBeNull();
  });
});

describe("cache misses", () => {
  const MIN = 60_000;
  // One request as a turn of its own: sent at `sentAt`, answered at `endedAt`.
  const request = (
    state: TurnState,
    { prompt, cached, sentAt, endedAt = sentAt + 1_000, model = "a/one" }: { prompt: number; cached: number; sentAt: number; endedAt?: number; model?: string },
  ) => {
    state = appendUserMessage(state, "go", sentAt);
    state = acceptEvent(state, { turnId: "t", seq: 1, round: 1, type: "roundStarted" }, sentAt, model);
    const payload = { promptTokens: prompt, completionTokens: 10, totalTokens: prompt + 10, cachedTokens: cached };
    return acceptEvent(state, { turnId: "t", seq: 2, round: 1, type: "contextUsage", payload }, endedAt, model);
  };
  const notices = (state: TurnState) => text(state, "notice");
  const warm = (prompt = 90_000) => request(emptyTurn(), { prompt, cached: 1_000, sentAt: 0 });

  test("a prompt read back from the cache says nothing", () => {
    expect(notices(request(warm(), { prompt: 95_000, cached: 89_000, sentAt: MIN }))).toEqual([]);
  });

  test("a pause past the cache's life is named as the cause", () => {
    expect(notices(request(warm(), { prompt: 95_000, cached: 6_000, sentAt: 7 * MIN }))).toEqual([
      "Cache miss after 7 min idle — ~84k tokens of the prompt were sent again at full price",
    ]);
  });

  test("the pause is counted to when the request was sent, not to its answer", () => {
    expect(notices(request(warm(), { prompt: 95_000, cached: 6_000, sentAt: 2 * MIN, endedAt: 9 * MIN }))).toEqual([
      "Cache miss — ~84k tokens of the prompt were sent again at full price",
    ]);
  });

  test("a model switch is named, and takes precedence over a pause", () => {
    expect(notices(request(warm(), { prompt: 95_000, cached: 0, sentAt: 9 * MIN, model: "b/two" }))).toEqual([
      "Cache miss after the model switch — ~90k tokens of the prompt were sent again at full price",
    ]);
  });

  test("only what both prompts shared is counted", () => {
    // The new prompt is shorter: 40k of it was in the last one, 10k came from the cache.
    expect(notices(request(warm(), { prompt: 40_000, cached: 10_000, sentAt: MIN }))).toEqual([
      "Cache miss — ~30k tokens of the prompt were sent again at full price",
    ]);
  });

  test("a small miss is not worth a line", () => {
    expect(notices(request(warm(), { prompt: 95_000, cached: 70_001, sentAt: MIN }))).toEqual([]);
  });

  test("a provider that never reported its cache is not judged by a zero", () => {
    const silent = request(emptyTurn(), { prompt: 90_000, cached: 0, sentAt: 0 });
    expect(notices(request(silent, { prompt: 95_000, cached: 0, sentAt: 9 * MIN }))).toEqual([]);
  });

  test("once a provider has reported its cache, every zero after is a full miss", () => {
    const missed = request(warm(), { prompt: 95_000, cached: 0, sentAt: MIN });
    expect(notices(request(missed, { prompt: 99_000, cached: 0, sentAt: 2 * MIN }))).toHaveLength(2);
  });

  test("the first report of a cache already tells a miss", () => {
    const silent = request(emptyTurn(), { prompt: 90_000, cached: 0, sentAt: 0 });
    expect(notices(request(silent, { prompt: 95_000, cached: 30_000, sentAt: MIN }))).toHaveLength(1);
  });

  test("a folded history starts over: its first request is not a miss", () => {
    const folded = compactionEnded(warm(), { folded: 12 });
    expect(notices(request(folded, { prompt: 95_000, cached: 0, sentAt: MIN }))).toEqual([]);
  });

  test("a pass that gave up keeps the history, and the comparison", () => {
    const failed = compactionEnded(warm(), null);
    expect(notices(request(failed, { prompt: 95_000, cached: 0, sentAt: MIN }))).toHaveLength(1);
  });
});
