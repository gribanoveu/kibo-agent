import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { Block } from "../lib/chatTurnReducer";

// When a conversation is written down, and with what. The interesting part is
// not the call itself but its timing: a turn that ended is history, a turn
// still running is not, and a chat that was merely opened is not news.

type Call = { command: string; args: Record<string, unknown> };
const calls: Call[] = [];
const results: Record<string, unknown> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    const answer = results[command];
    // A function answers from what was sent, as the backend does.
    const result = typeof answer === "function" ? answer(args) : answer;
    // Tauri rejects with the command's error string.
    return result instanceof Error ? Promise.reject(result.message) : Promise.resolve(result ?? null);
  },
}));

// Every listener, so a test can say what the backend said on the channel.
type Listener = (event: { payload: unknown }) => void;
const listeners = new Set<Listener>();
const emit = (payload: unknown) => [...listeners].forEach((listener) => listener({ payload }));

mock.module("@tauri-apps/api/event", () => ({
  listen: (_name: string, listener: Listener) => {
    listeners.add(listener);
    return Promise.resolve(() => listeners.delete(listener));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useAgentTurn } = await import("../hooks/useAgentTurn");

/** A turn that answers `text`: its history is what was sent, then the answer. */
const done = (text: string) => (args: Record<string, unknown>) => ({
  status: "done",
  value: { text, truncated: false, todos: [], history: [...(args.messages as unknown[]), { role: "assistant", content: text }] },
});

afterEach(() => {
  calls.length = 0;
  for (const key of Object.keys(results)) delete results[key];
});

const saved = () => calls.filter((call) => call.command === "chat_save");

describe("making room before a turn", () => {
  /// Room in the window is the turn's to make, round by round: nothing is
  /// folded on the way to it, and what the turn hands back — folded or not —
  /// is what the next message is sent with.
  test("a message goes to the turn as it is, and the turn's history is kept", async () => {
    results.chat_start = (args: Record<string, unknown>) => ({
      status: "done",
      value: { text: "ok", truncated: false, todos: [], history: [{ role: "user", content: "[summary] earlier" }, ...(args.messages as unknown[]).slice(-1)] },
    });
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("hello");
    });
    expect(calls.filter((call) => call.command === "chat_compact")).toEqual([]);
    expect(calls.find((call) => call.command === "chat_start")?.args.messages).toEqual([{ role: "user", content: "hello" }]);

    await act(async () => {
      await result.current.send("and now?");
    });
    expect(calls.filter((call) => call.command === "chat_start")[1]?.args.messages).toEqual([
      { role: "user", content: "[summary] earlier" },
      { role: "user", content: "hello" },
      { role: "user", content: "and now?" },
    ]);
  });

  /// A `/` command: the transcript and the chat's name have what was typed,
  /// the model has the prompt it stands for.
  test("a command's prompt goes to the model, and the command to the transcript", async () => {
    results.chat_start = done("on it");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("/init the IPC layer", "Study this repository. Focus: the IPC layer");
    });

    const started = calls.find((call) => call.command === "chat_start");
    expect(started?.args.messages).toEqual([{ role: "user", content: "Study this repository. Focus: the IPC layer" }]);
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect((saved()[0].args.blocks as Block[])[0]).toMatchObject({
      kind: "user",
      text: "/init the IPC layer",
      sent: "Study this repository. Focus: the IPC layer",
    });
  });

  /// Typed while a turn runs, a command steers it: the model gets the prompt,
  /// and the transcript — told back by the backend — the command.
  test("a command typed mid-turn steers with its prompt, shown as typed", async () => {
    results.chat_start = () => new Promise(() => {});
    results.chat_steer = "note-1";
    const { result } = renderHook(() => useAgentTurn());
    act(() => {
      void result.current.send("go");
    });
    await waitFor(() => expect(result.current.turn.status).toBe("running"));

    await act(async () => {
      await result.current.send("/review a.rs", "Review a.rs line by line");
      await result.current.send("and b.rs");
    });

    expect(calls.filter((call) => call.command === "chat_steer").map((call) => call.args)).toEqual([
      { text: "Review a.rs line by line", shown: "/review a.rs" },
      { text: "and b.rs", shown: null },
    ]);
  });

  /// The summary takes seconds; the backend says when it starts, and the card
  /// is on screen until the call returns.
  test("a pass under way is on screen until it ends", async () => {
    let finish: (value: unknown) => void = () => {};
    results.chat_compact = (args: Record<string, unknown>) => {
      emit({ turnId: args.turnId, seq: 1, round: 0, type: "historyCompacting" });
      return new Promise((resolve) => (finish = resolve));
    };
    const { result } = renderHook(() => useAgentTurn());

    let pass: Promise<boolean> = Promise.resolve(false);
    act(() => {
      pass = result.current.compact(true);
    });
    await waitFor(() =>
      expect(result.current.turn.blocks).toEqual([{ kind: "compaction", id: "compaction:0", status: "running" }]),
    );

    await act(async () => {
      finish({ history: [{ role: "user", content: "summary" }], folded: 7 });
      await pass;
    });
    expect(result.current.turn.blocks).toEqual([
      { kind: "compaction", id: "compaction:0", status: "done", folded: 7 },
    ]);
  });

  test("a pass that failed says it gave up", async () => {
    results.chat_compact = (args: Record<string, unknown>) => {
      emit({ turnId: args.turnId, seq: 1, round: 0, type: "historyCompacting" });
      return new Error("provider said no");
    };
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      expect(await result.current.compact(true)).toBe(false);
    });
    expect(result.current.turn.blocks).toEqual([{ kind: "compaction", id: "compaction:0", status: "failed" }]);
    expect(result.current.error).toContain("provider said no");
  });

  /// Another pass's start, or one arriving after its own end, is not this one.
  test("a start said under another id is not this pass", async () => {
    results.chat_compact = () => {
      emit({ turnId: "compact-elsewhere", seq: 1, round: 0, type: "historyCompacting" });
      return null;
    };
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.compact(true);
    });
    expect(result.current.turn.blocks).toEqual([]);
  });

  /// Asked for outright, with nothing worth folding: the answer is no, and
  /// the caller is the one that says so.
  test("asking for one that cannot help reports back", async () => {
    results.chat_compact = null;
    const { result } = renderHook(() => useAgentTurn());

    let folded: boolean | undefined;
    await act(async () => {
      folded = await result.current.compact(true);
    });

    expect(folded).toBe(false);
    expect(calls.filter((call) => call.command === "chat_compact")).toEqual([
      { command: "chat_compact", args: { messages: [], force: true, plan: null, turnId: expect.stringMatching(/^compact-/) } },
    ]);
  });
});

describe("the context estimate", () => {
  /// The meter has to have something to show before the first message: an
  /// empty conversation still costs the prompt and the tool schemas.
  test("is asked for on the first render, with an empty history", async () => {
    results.chat_context_usage = {
      instructions: 1_000,
      skills: 0,
      tools: 3_000,
      mcp: 0,
      conversation: 0,
      total: 4_000,
      limit: null,
      compactsAt: null,
    };
    const { result } = renderHook(() => useAgentTurn());

    await waitFor(() => expect(result.current.context?.total).toBe(4_000));
    expect(calls).toContainEqual({ command: "chat_context_usage", args: { messages: [], plan: null } });
  });

  /// It is an estimate over the history, so it has to be asked again once the
  /// history is shorter — otherwise the meter still reads full after a fold.
  test("is asked again after the conversation was folded", async () => {
    results.chat_compact = { history: [{ role: "user", content: "summary" }], folded: 9 };
    const { result } = renderHook(() => useAgentTurn());
    await waitFor(() =>
      expect(calls.some((call) => call.command === "chat_context_usage")).toBe(true),
    );
    calls.length = 0;

    await act(async () => {
      await result.current.compact(true);
    });

    expect(calls).toContainEqual({
      command: "chat_context_usage",
      args: { messages: [{ role: "user", content: "summary" }], plan: null },
    });
  });
});

describe("saving", () => {
  test("a finished turn is written down, with both lists", async () => {
    results.chat_start = done("here you go");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("fix the parser");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));

    const args = saved()[0].args;
    expect(args.messages).toEqual([
      { role: "user", content: "fix the parser" },
      { role: "assistant", content: "here you go" },
    ]);
    // The transcript, not a reconstruction of it from the messages — with
    // how long the agent worked on it, so a reopened chat still says so.
    expect(args.blocks).toEqual([
      { kind: "user", id: "user:0", text: "fix the parser", workedMs: expect.any(Number) },
    ]);
    expect(result.current.chatId).toBe(args.id as string);
  });

  test("the second turn of a conversation goes to the same chat", async () => {
    results.chat_start = done("one");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("first");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    const first = saved()[0].args.id;

    await act(async () => {
      await result.current.send("second");
    });
    await waitFor(() => expect(saved()).toHaveLength(2));

    expect(saved()[1].args.id).toBe(first as string);
  });

  /// Reading is not a change. Saving here would also reorder the sidebar,
  /// which lists by when a chat was last written to.
  test("opening a chat does not write it straight back", async () => {
    results.chat_load = {
      schemaVersion: 1,
      id: "kept",
      workspace: "/repo",
      title: "earlier",
      createdAt: 1,
      updatedAt: 2,
      messages: [{ role: "user", content: "earlier" }],
      blocks: [{ kind: "user", id: "user:0", text: "earlier" }],
      todos: [],
    };
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.open("kept");
    });

    expect(saved()).toEqual([]);
    expect(result.current.chatId).toBe("kept");
    expect(result.current.turn.blocks).toHaveLength(1);
  });

  /// What was said carries on from where the transcript left off — otherwise
  /// the model answers the next question having forgotten the last one.
  test("a reopened chat continues its own history", async () => {
    results.chat_load = {
      schemaVersion: 1,
      id: "kept",
      workspace: "/repo",
      title: "earlier",
      createdAt: 1,
      updatedAt: 2,
      messages: [
        { role: "user", content: "earlier" },
        { role: "assistant", content: "answered" },
      ],
      blocks: [],
      todos: [],
    };
    results.chat_start = done("still here");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.open("kept");
    });
    await act(async () => {
      await result.current.send("and now?");
    });

    const started = calls.find((call) => call.command === "chat_start");
    expect(started?.args.messages).toEqual([
      { role: "user", content: "earlier" },
      { role: "assistant", content: "answered" },
      { role: "user", content: "and now?" },
    ]);
  });

  /// A turn that paused is not over: its last tool call has been asked about
  /// and not yet answered, and a transcript saved here has a hole in it.
  test("a turn waiting for approval is not written down yet", async () => {
    results.chat_start = {
      status: "pendingApproval",
      value: {
        history: [],
        round: 1,
        budgetUsed: 1,
        eventSeq: 3,
        calls: [{ id: "w1", name: "writeFile", arguments: "{}", requiresConfirmation: true }],
        todos: [],
        reads: {},
      },
    };
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("write it");
    });

    expect(result.current.turn.status).toBe("awaitingApproval");
    expect(saved()).toEqual([]);
  });

  test("starting a new chat leaves the old one where it is", async () => {
    results.chat_start = done("one");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("first");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    const first = saved()[0].args.id;

    act(() => result.current.reset());
    await act(async () => {
      await result.current.send("second");
    });
    await waitFor(() => expect(saved()).toHaveLength(2));

    expect(saved()[1].args.id).not.toBe(first as string);
    expect(saved()[1].args.messages).toEqual([
      { role: "user", content: "second" },
      { role: "assistant", content: "one" },
    ]);
  });
});

describe("branching", () => {
  async function twoTurns() {
    results.chat_start = done("answer");
    const hook = renderHook(() => useAgentTurn());
    await act(async () => {
      await hook.result.current.send("first");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    await act(async () => {
      await hook.result.current.send("second");
    });
    await waitFor(() => expect(saved()).toHaveLength(2));
    return hook;
  }

  /// A new chat from the point before the message, which comes back to be
  /// changed. The original is not written to again.
  test("starts a new chat from before the message and hands it back", async () => {
    const { result } = await twoTurns();
    const original = result.current.chatId;
    expect([...(result.current.branchable ?? [])].sort()).toEqual(["user:0", "user:1"]);

    act(() => result.current.branch("user:1"));

    expect(result.current.chatId).toBeNull();
    expect(result.current.draft?.text).toBe("second");
    expect(result.current.turn.blocks.map((b) => b.kind)).toEqual(["user", "notice"]);
    expect(saved()).toHaveLength(2);

    results.chat_start = done("another answer");
    await act(async () => {
      await result.current.send("second, differently");
    });
    await waitFor(() => expect(saved()).toHaveLength(3));

    const branch = saved()[2].args;
    expect(branch.id).not.toBe(original as string);
    expect(branch.branchedFrom).toBe(original as string);
    expect(branch.messages).toEqual([
      { role: "user", content: "first" },
      { role: "assistant", content: "answer" },
      { role: "user", content: "second, differently" },
      { role: "assistant", content: "another answer" },
    ]);
  });

  /// `/fork`: the whole conversation in a new chat, nothing handed back, and
  /// saved at once — a row in the sidebar before anything is sent to it.
  test("a fork copies the whole conversation into a new chat, saved at once", async () => {
    const { result } = await twoTurns();
    const original = result.current.chatId;
    const opened = result.current.planWritten;

    act(() => result.current.branch());

    expect(result.current.draft).toBeNull();
    expect(result.current.turn.blocks.map((b) => b.kind)).toEqual(["user", "user", "notice"]);
    await waitFor(() => expect(saved()).toHaveLength(3));

    const fork = saved()[2].args;
    expect(fork.id).not.toBe(original as string);
    expect(fork.id).toBe(result.current.chatId as string);
    expect(fork.branchedFrom).toBe(original as string);
    expect(fork.messages).toEqual(saved()[1].args.messages);
    expect(result.current.planWritten).toBe(opened);

    results.chat_start = done("third answer");
    await act(async () => {
      await result.current.send("third");
    });
    await waitFor(() => expect(saved()).toHaveLength(4));
    expect(saved()[3].args.id).toBe(fork.id);
    expect(saved()[3].args.messages).toHaveLength(6);
  });

  /// The plan the original wrote in its last turn is copied, not announced
  /// again: the Plan tab opens for a plan just written, not for a fork.
  test("a fork of a chat whose last turn wrote a plan keeps it without reopening it", async () => {
    results.chat_load = branchRecord;
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("b");
    });

    act(() => result.current.branch());
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args.plan).toBe("# Plan");
    expect(result.current.planWritten).toBe(0);
  });

  test("a fork keeps the checklist", async () => {
    results.chat_start = (args: Record<string, unknown>) => ({
      status: "done",
      value: { text: "ok", truncated: false, todos: [{ id: "1", title: "later", status: "pending" }], history: args.messages },
    });
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("first");
    });

    act(() => result.current.branch());
    expect(result.current.checklist).toHaveLength(1);
  });

  test("an empty chat is not forked", () => {
    const { result } = renderHook(() => useAgentTurn());
    act(() => result.current.branch());
    expect(result.current.turn.blocks).toEqual([]);
  });

  /// The checklist kept is the latest one, and part of it may be work done
  /// after the branch point.
  test("the branch starts without the checklist", async () => {
    results.chat_start = (args: Record<string, unknown>) => ({
      status: "done",
      value: { text: "ok", truncated: false, todos: [{ id: "1", title: "later", status: "completed" }], history: args.messages },
    });
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("first");
    });
    expect(result.current.checklist).toHaveLength(1);

    act(() => result.current.branch("user:0"));
    expect(result.current.checklist).toEqual([]);
  });

  test("a chat opened again remembers it is a branch", async () => {
    results.chat_load = {
      schemaVersion: 1,
      id: "b",
      workspace: "/repo",
      title: "first",
      createdAt: 1,
      updatedAt: 2,
      messages: [],
      blocks: [],
      todos: [],
      branchedFrom: "a",
    };
    results.chat_start = done("ok");
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("b");
    });
    await act(async () => {
      await result.current.send("more");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args.branchedFrom).toBe("a");
  });

  const branchRecord = {
    schemaVersion: 1,
    id: "b",
    workspace: "/repo",
    title: "first",
    createdAt: 1,
    updatedAt: 2,
    messages: [
      { role: "user", content: "first" },
      { role: "assistant", content: "planned" },
      { role: "user", content: "second" },
    ],
    blocks: [
      { kind: "user", id: "user:0", text: "first" },
      { kind: "tool", id: "t", round: 1, name: "writePlan", arguments: JSON.stringify({ content: "# Plan" }), status: "done", output: "" },
      { kind: "user", id: "user:2", text: "second" },
    ],
    todos: [],
    plan: "# Plan",
    branchedFrom: "a",
  };

  /// The plan a branch starts with is the one written before its point, not
  /// one the original chat wrote later.
  test("the branch keeps the plan written before its point, and only that", async () => {
    results.chat_load = branchRecord;
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("b");
    });

    act(() => result.current.branch("user:2"));
    expect(result.current.plan).toBe("# Plan");

    await act(async () => {
      await result.current.open("b");
    });
    act(() => result.current.branch("user:0"));
    expect(result.current.plan).toBeNull();
  });

  /// A branch's own plan edit is a save of the branch, and must not forget
  /// what it is a branch of.
  test("editing the plan of a branch keeps it a branch", async () => {
    results.chat_load = branchRecord;
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("b");
    });

    act(() => result.current.editPlan("# Changed"));
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args.branchedFrom).toBe("a");
  });

  test("a new chat after a branch is not a branch", async () => {
    results.chat_load = branchRecord;
    results.chat_start = done("ok");
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("b");
    });

    act(() => result.current.reset());
    await act(async () => {
      await result.current.send("fresh");
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args.branchedFrom).toBeNull();
  });

  test("nothing can be branched while a turn waits for approval", async () => {
    results.chat_start = {
      status: "pendingApproval",
      value: { history: [], round: 1, budgetUsed: 1, eventSeq: 3, calls: [], todos: [], reads: {} },
    };
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("write it");
    });

    expect(result.current.branchable).toBeNull();
    const before = result.current.turn.blocks;
    act(() => result.current.branch("user:0"));
    expect(result.current.turn.blocks).toBe(before);
  });
});

describe("a turn that fails to start", () => {
  test("says why in the transcript, not only in `error`", async () => {
    results.chat_start = new Error("provider said 401");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("hello");
    });

    expect(result.current.turn.status).toBe("done");
    const notice = result.current.turn.blocks.find((b) => b.kind === "notice");
    expect(notice && "text" in notice && notice.text).toBe("The turn failed: provider said 401");
  });
});

describe("what the next message is sent with", () => {
  /// The model answers a follow-up from what it read, not from what it
  /// happened to repeat in its answer.
  test("the turn's calls and results, not only its answer", async () => {
    const read = [
      { role: "assistant", content: null, toolCalls: [{ id: "r1", name: "readFile", arguments: '{"path":"a.rs"}' }] },
      { role: "tool", content: "All 1 lines:\nfn one() {}", toolCallId: "r1" },
    ];
    results.chat_start = (args: Record<string, unknown>) => ({
      status: "done",
      value: { text: "read it", truncated: false, todos: [], history: [...(args.messages as unknown[]), ...read, { role: "assistant", content: "read it" }] },
    });
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("read a.rs");
    });
    await act(async () => {
      await result.current.send("and now?");
    });

    const second = calls.filter((call) => call.command === "chat_start")[1];
    expect(second?.args.messages).toEqual([
      { role: "user", content: "read a.rs" },
      ...read,
      { role: "assistant", content: "read it" },
      { role: "user", content: "and now?" },
    ]);
  });
});

describe("the queue", () => {
  /** A turn held open until `end` is called with how it ended. */
  function heldTurn() {
    let end: (outcome: unknown) => void = () => {};
    let first = true;
    results.chat_start = (args: Record<string, unknown>) => {
      if (!first) return done("next")(args);
      first = false;
      return new Promise((resolve) => {
        end = (status) => resolve({ ...done("first")(args), status });
      });
    };
    return { end: (status: string) => end(status) };
  }

  const started = () => calls.filter((call) => call.command === "chat_start");

  test("a message queued mid-turn is sent as the next turn once this one is done, not steered", async () => {
    const turn = heldTurn();
    const { result } = renderHook(() => useAgentTurn());
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));

    act(() => result.current.queue("then update the docs"));
    expect(result.current.queued.map((q) => q.text)).toEqual(["then update the docs"]);
    expect(calls.some((call) => call.command === "chat_steer")).toBe(false);

    await act(async () => turn.end("done"));
    await waitFor(() => expect(started()).toHaveLength(2));
    expect((started()[1].args.messages as { content: string }[]).at(-1)).toEqual({ role: "user", content: "then update the docs" });
    await waitFor(() => expect(result.current.queued).toEqual([]));
  });

  test("a stopped turn gives the queue back instead of starting it", async () => {
    const turn = heldTurn();
    const given: string[] = [];
    const { result } = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    act(() => result.current.queue("one"));
    act(() => result.current.queue("two"));

    await act(async () => turn.end("cancelled"));
    await waitFor(() => expect(result.current.queued).toEqual([]));
    expect(given).toEqual(["one\n\ntwo"]);
    expect(started()).toHaveLength(1);
  });

  test("a failed turn gives the queue back too", async () => {
    let fail: (reason: string) => void = () => {};
    results.chat_start = () => new Promise((_, reject) => (fail = reject));
    const given: string[] = [];
    const { result } = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    act(() => result.current.queue("one"));

    await act(async () => fail("provider said 500"));
    await waitFor(() => expect(given).toEqual(["one"]));
    expect(started()).toHaveLength(1);
  });

  test("a row taken out goes back, and the rest stays", async () => {
    heldTurn();
    const given: string[] = [];
    const { result } = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    act(() => result.current.queue("one"));
    act(() => result.current.queue("two"));

    act(() => result.current.unqueue(result.current.queued[0].id));
    expect(given).toEqual(["one"]);
    expect(result.current.queued.map((q) => q.text)).toEqual(["two"]);
  });

  /** Starts a held turn and sends `note` into it; the backend calls it `note-1`. */
  async function steeredTurn(given: string[]) {
    const turn = heldTurn();
    results.chat_steer = "note-1";
    const hook = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void hook.result.current.send("fix it"));
    await waitFor(() => expect(hook.result.current.turn.status).toBe("running"));
    await act(async () => {
      await hook.result.current.send("use the helper");
    });
    const turnId = started()[0].args.turnId;
    const read = () =>
      act(() => emit({ turnId, seq: 1, round: 1, type: "steeringApplied", payload: { id: "note-1", text: "use the helper" } }));
    return { ...hook, turn, read };
  }

  test("a note sent into the turn is listed until a round reads it", async () => {
    const { result, read } = await steeredTurn([]);
    expect(result.current.steered).toEqual([{ id: "note-1", text: "use the helper" }]);

    read();
    expect(result.current.steered).toEqual([]);
    expect(result.current.turn.blocks.some((block) => block.kind === "steer")).toBe(true);
  });

  test("a note taken back before it is read goes back to the box", async () => {
    const given: string[] = [];
    const { result } = await steeredTurn(given);
    results.chat_cancel_steer = true;

    await act(() => result.current.withdraw("note-1"));
    expect(calls.find((call) => call.command === "chat_cancel_steer")?.args).toEqual({ id: "note-1" });
    expect(given).toEqual(["use the helper"]);
    expect(result.current.steered).toEqual([]);
  });

  /// A round took it while the user reached for the button: the model has it,
  /// and the transcript will show it, so the box does not get it back too.
  test("a note a round already took is not given back", async () => {
    const given: string[] = [];
    const { result } = await steeredTurn(given);
    results.chat_cancel_steer = false;

    await act(() => result.current.withdraw("note-1"));
    expect(given).toEqual([]);
    expect(result.current.steered).toEqual([]);
  });

  /// One hand-back, not two: the composer keeps only the last text it is given.
  test("a stopped turn gives back the unread note together with the queue", async () => {
    const given: string[] = [];
    const { result, turn } = await steeredTurn(given);
    act(() => result.current.queue("then the docs"));

    await act(async () => turn.end("cancelled"));
    await waitFor(() => expect(given).toEqual(["use the helper\n\nthen the docs"]));
    expect(result.current.steered).toEqual([]);
  });

  test("a stopped turn gives back an unread note with nothing queued", async () => {
    const given: string[] = [];
    const { result, turn } = await steeredTurn(given);

    await act(async () => turn.end("cancelled"));
    await waitFor(() => expect(given).toEqual(["use the helper"]));
    expect(result.current.steered).toEqual([]);
  });

  test("a finished turn gives back a note it never read, and still sends the queue", async () => {
    const given: string[] = [];
    const { result, turn } = await steeredTurn(given);
    act(() => result.current.queue("then the docs"));

    await act(async () => turn.end("done"));
    await waitFor(() => expect(started()).toHaveLength(2));
    expect(given).toEqual(["use the helper"]);
    expect((started()[1].args.messages as { content: string }[]).at(-1)?.content).toBe("then the docs");
  });

  test("a note the turn read is not given back when it ends", async () => {
    const given: string[] = [];
    const { result, turn, read } = await steeredTurn(given);
    read();

    await act(async () => turn.end("done"));
    await waitFor(() => expect(result.current.turn.status).toBe("done"));
    expect(given).toEqual([]);
  });

  /// The turn, its answer and its queue belong to the chat on screen: a new
  /// chat would take them over, so it waits for the turn to stop.
  test("a new chat is refused while the turn runs, and the queue stays", async () => {
    heldTurn();
    const given: string[] = [];
    const { result } = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    act(() => result.current.queue("one"));

    let left: boolean | undefined;
    act(() => {
      left = result.current.reset();
    });
    expect(left).toBe(false);
    expect(result.current.turn.status).toBe("running");
    expect(given).toEqual([]);
    expect(result.current.queued.map((q) => q.text)).toEqual(["one"]);
  });
});

describe("the next-prompt journal", () => {
  const journal = () => calls.filter((call) => call.command === "next_prompt_log" || call.command === "next_prompt_sent");

  /// The input is paired with what the user really sent next, as they typed
  /// it — the command, not the prompt it stands for.
  test("a finished turn is journaled, and the next message answers it", async () => {
    results.chat_start = done("Готово. Закоммитить?");
    results.next_prompt_log = "row-1";
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("/init", "Study this repository");
    });
    await waitFor(() => expect(journal()).toHaveLength(1));
    const chatId = result.current.chatId;
    const turnIds = () => calls.filter((call) => call.command === "chat_start").map((call) => call.args.turnId);
    expect(journal()[0].args).toEqual({ chatId, turnId: turnIds()[0], user: "/init" });

    await act(async () => {
      await result.current.send("да");
    });
    await waitFor(() => expect(journal()).toHaveLength(3));
    expect(journal()[1]).toEqual({ command: "next_prompt_sent", args: { id: "row-1", text: "да" } });
    expect(journal()[2].args).toEqual({ chatId, turnId: turnIds()[1], user: "да" });
  });

  /// `/review` is what the user sent next, as much as a message is.
  test("a review answers the turn journaled before it", async () => {
    results.chat_start = done("ok");
    results.next_prompt_log = "row-1";
    results.review_start = done("reviewed");
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("first");
    });
    await waitFor(() => expect(journal()).toHaveLength(1));
    await act(async () => {
      await result.current.review();
    });
    await waitFor(() => expect(journal()).toHaveLength(3));
    expect(journal()[1]).toEqual({ command: "next_prompt_sent", args: { id: "row-1", text: "/review" } });
    // A review is a turn, journaled like one.
    expect(journal()[2]).toMatchObject({ command: "next_prompt_log", args: { user: "/review" } });
  });

  /// What is typed into another chat is not an answer to this one.
  test("a new chat leaves the last turn unanswered", async () => {
    results.chat_start = done("ok");
    results.next_prompt_log = "row-1";
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("first");
    });
    await waitFor(() => expect(journal()).toHaveLength(1));

    act(() => result.current.reset());
    await act(async () => {
      await result.current.send("something else");
    });
    await waitFor(() => expect(journal()).toHaveLength(2));
    expect(calls.some((call) => call.command === "next_prompt_sent")).toBe(false);
  });

  /// Nothing to journal comes back as `null`, and then nothing is answered.
  test("a turn the backend did not journal is not answered", async () => {
    results.chat_start = done("ok");
    results.next_prompt_log = null;
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.send("first");
    });
    await waitFor(() => expect(journal()).toHaveLength(1));
    await act(async () => {
      await result.current.send("second");
    });
    await waitFor(() => expect(journal()).toHaveLength(2));
    expect(calls.some((call) => call.command === "next_prompt_sent")).toBe(false);
  });
});

describe("rewinding", () => {
  const stored = (hash: string) => ({ kind: "stored", hash });
  const aChange = { path: "a.rs", before: stored("a0"), after: stored("a1") };
  const bChange = { path: "b.rs", before: { kind: "absent" }, after: stored("b1") };
  const record = {
    schemaVersion: 1,
    id: "kept",
    workspace: "/repo",
    title: "fix",
    createdAt: 1,
    updatedAt: 2,
    messages: [
      { role: "user", content: "first" },
      { role: "assistant", content: "done a" },
      { role: "user", content: "second" },
      { role: "assistant", content: "done b" },
    ],
    blocks: [
      { kind: "user", id: "user:0", text: "first" },
      { kind: "tool", id: "t1", round: 1, name: "writeFile", arguments: "{}", status: "done", output: "", changes: [aChange] },
      { kind: "message", id: "m1", round: 2, text: "done a" },
      { kind: "user", id: "user:1", text: "second" },
      { kind: "tool", id: "t2", round: 1, name: "writeFile", arguments: "{}", status: "done", output: "", changes: [bChange] },
      { kind: "tool", id: "t3", round: 2, name: "runCommand", arguments: "{}", status: "done", output: "" },
      { kind: "message", id: "m2", round: 3, text: "done b" },
    ],
    todos: [],
  };

  async function opened() {
    results.chat_load = record;
    const hook = renderHook(() => useAgentTurn());
    await act(async () => {
      await hook.result.current.open("kept");
    });
    return hook;
  }

  test("a preview asks about the changes from that message on, and counts the commands", async () => {
    results.rewind_preview = [{ path: "b.rs", action: "created", expected: stored("b1"), target: { kind: "absent" }, skip: null }];
    const { result } = await opened();
    let preview: Awaited<ReturnType<typeof result.current.previewRewind>> = null;
    await act(async () => {
      preview = await result.current.previewRewind("user:1");
    });
    expect(calls.find((call) => call.command === "rewind_preview")?.args).toEqual({ changes: [bChange] });
    expect(preview).toMatchObject({ unrecorded: 1, files: [{ path: "b.rs" }] });
  });

  /// In place: the same chat, cut before the message, which comes back to
  /// the box; the files first.
  test("puts the files back, cuts this chat before the message and hands it back", async () => {
    results.rewind_apply = [
      { path: "a.rs", action: "modified", expected: stored("a1"), target: stored("a0"), skip: null },
      { path: "b.rs", action: "created", expected: stored("b1"), target: { kind: "absent" }, skip: "changedSince" },
    ];
    const { result } = await opened();
    await act(async () => {
      await result.current.rewind("user:0");
    });

    expect(calls.find((call) => call.command === "rewind_apply")?.args).toEqual({ changes: [aChange, bChange] });
    expect(result.current.chatId).toBe("kept");
    expect(result.current.draft?.text).toBe("first");
    expect(result.current.turn.blocks).toMatchObject([
      { kind: "notice", text: "Rewound to before this message — 1 file put back, 1 file left as it is" },
    ]);
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args).toMatchObject({ id: "kept", messages: [] });
  });

  test("with a summary, the part cut off is told to the model and kept after the conversation", async () => {
    const summary = { role: "user", content: "[Summary of a conversation branch]\n\nb.rs was tried and failed" };
    results.chat_branch_summary = summary;
    results.rewind_apply = [];
    const { result } = await opened();
    await act(async () => {
      await result.current.rewind("user:1", true);
    });

    expect(calls.find((call) => call.command === "chat_branch_summary")?.args).toEqual({ messages: record.messages.slice(2) });
    expect(result.current.draft?.text).toBe("second");
    expect(result.current.turn.blocks.at(-1)).toMatchObject({
      kind: "notice",
      text: "Rewound to before this message — no files to put back; the agent keeps a summary of what was tried",
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args).toMatchObject({ messages: [...record.messages.slice(0, 2), summary] });
  });

  test("a summary that failed stops the rewind before the files", async () => {
    results.chat_branch_summary = new Error("the provider is down");
    const { result } = await opened();
    await act(async () => {
      await result.current.rewind("user:1", true);
    });
    expect(result.current.error).toBe("the provider is down");
    expect(calls.some((call) => call.command === "rewind_apply")).toBe(false);
    expect(result.current.turn.blocks).toHaveLength(record.blocks.length);
  });

  test("an empty summary still rewinds, and says nothing of one", async () => {
    results.chat_branch_summary = null;
    results.rewind_apply = [];
    const { result } = await opened();
    await act(async () => {
      await result.current.rewind("user:1", true);
    });
    expect(result.current.turn.blocks.at(-1)).toMatchObject({ text: "Rewound to before this message — no files to put back" });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args).toMatchObject({ messages: record.messages.slice(0, 2) });
  });

  test("a rewind that could not reach the files leaves the chat as it was", async () => {
    results.rewind_apply = new Error("no folder is open");
    const { result } = await opened();
    await act(async () => {
      await result.current.rewind("user:1");
    });
    expect(result.current.error).toBe("no folder is open");
    expect(result.current.turn.blocks).toHaveLength(record.blocks.length);
    expect(saved()).toHaveLength(0);
  });

  test("the transcript says how the files went", async () => {
    const { rewoundNotice } = await import("../hooks/useAgentTurn");
    const file = (skip: string | null) => ({ path: "a", action: "modified", expected: stored("1"), target: stored("0"), skip }) as never;
    expect(rewoundNotice([])).toBe("Rewound to before this message — no files to put back");
    expect(rewoundNotice([file(null), file(null)])).toBe("Rewound to before this message — 2 files put back");
    expect(rewoundNotice([file(null), file("expired"), file("changedSince")])).toBe(
      "Rewound to before this message — 1 file put back, 2 files left as they are",
    );
  });
});

describe("reviewing", () => {
  /// The backend adds the change to the history as the message the
  /// bubble stands for; the outcome brings it back like any turn's.
  const reviewed = (answer: string) => (args: Record<string, unknown>) => ({
    status: "done",
    value: {
      text: answer,
      truncated: false,
      todos: [],
      history: [...(args.messages as unknown[]), { role: "user", content: "Review the uncommitted changes…" }, { role: "assistant", content: answer }],
    },
  });

  test("a review is a turn: it is sent the history, and keeps the one it ends with", async () => {
    results.review_start = reviewed("Nothing wrong.");
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.review();
    });
    const call = calls.find((c) => c.command === "review_start");
    expect(call?.args).toEqual({ turnId: expect.stringMatching(/^turn-/), messages: [], todos: [], plan: null });
    expect(result.current.turn.status).toBe("done");
    expect(result.current.turn.blocks[0]).toMatchObject({ kind: "user", text: "/review" });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect((saved()[0].args.messages as { content: string }[]).map((m) => m.content)).toEqual([
      "Review the uncommitted changes…",
      "Nothing wrong.",
    ]);
  });

  /// It is a turn like any other: what is typed steers it, and leaving
  /// waits for it, as it would for a message's turn.
  test("while a review runs, a message steers it and the chat cannot be left", async () => {
    let finish: (value: unknown) => void = () => {};
    results.review_start = new Promise((resolve) => (finish = resolve));
    results.chat_steer = "note-1";
    const { result } = renderHook(() => useAgentTurn());
    let running: Promise<void> = Promise.resolve();
    act(() => {
      running = result.current.review();
    });
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    await act(async () => {
      await result.current.send("and check the tests");
    });
    expect(calls.find((c) => c.command === "chat_steer")?.args).toMatchObject({ text: "and check the tests" });
    let left: boolean | undefined;
    act(() => {
      left = result.current.reset();
    });
    expect(left).toBe(false);
    expect(await result.current.open("other")).toBe(false);
    await act(async () => {
      finish(reviewed("done")({ messages: [] }));
      await running;
    });
    expect(result.current.turn.status).toBe("done");
  });

  /// A review runs for minutes; the chat is in the sidebar from the start.
  test("a new chat is saved as the review starts, with its message only, and again under the same id at the end", async () => {
    let finish: (value: unknown) => void = () => {};
    results.review_start = new Promise((resolve) => (finish = resolve));
    const { result } = renderHook(() => useAgentTurn());
    let running: Promise<void> = Promise.resolve();
    act(() => {
      running = result.current.review();
    });
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect(saved()[0].args.blocks).toEqual([expect.objectContaining({ kind: "user", text: "/review" })]);
    expect(result.current.chatId).toBe(saved()[0].args.id as string);
    await act(async () => {
      finish(reviewed("done")({ messages: [] }));
      await running;
    });
    await waitFor(() => expect(saved()).toHaveLength(2));
    expect(saved()[1].args.id).toBe(saved()[0].args.id);
  });

  test("a review that fails says why, and its bubble keeps a message in the history", async () => {
    results.review_start = new Error("nothing to review: the working tree matches HEAD");
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.review();
    });
    expect(result.current.turn.blocks.map((b) => b.kind)).toEqual(["user", "notice"]);
    expect(result.current.error).toBe("nothing to review: the working tree matches HEAD");
    await waitFor(() => expect(saved()).toHaveLength(1));
    expect((saved()[0].args.messages as { content: string }[]).map((m) => m.content)).toEqual([
      "/review",
      "[The review failed: nothing to review: the working tree matches HEAD]",
    ]);
  });
});
