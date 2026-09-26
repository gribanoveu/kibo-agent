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
  /// Compacting inside the turn would work once and be paid for again on the
  /// next message: the window owns the history, so the shorter one has to be
  /// kept here.
  test("a shorter history is adopted and sent", async () => {
    // What the backend returns: the summary, then the tail it was given —
    // the message just typed among it.
    results.chat_compact = {
      history: [
        { role: "user", content: "[summary] earlier" },
        { role: "user", content: "and now?" },
      ],
      folded: 20,
    };
    results.chat_start = done("carry on");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("and now?");
    });

    const started = calls.find((call) => call.command === "chat_start");
    expect(started?.args.messages).toEqual([
      { role: "user", content: "[summary] earlier" },
      { role: "user", content: "and now?" },
    ]);
    expect(result.current.turn.blocks).toContainEqual({
      kind: "compaction",
      id: "compaction:1",
      status: "done",
      folded: 20,
    });
  });

  test("and a history that needs nothing is sent as it is", async () => {
    results.chat_compact = null;
    results.chat_start = done("carry on");
    const { result } = renderHook(() => useAgentTurn());

    await act(async () => {
      await result.current.send("hello");
    });

    const started = calls.find((call) => call.command === "chat_start");
    expect(started?.args.messages).toEqual([{ role: "user", content: "hello" }]);
    expect(result.current.turn.blocks.some((b) => b.kind === "compaction")).toBe(false);
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

  test("a new chat takes the queue back rather than sending it there", async () => {
    heldTurn();
    const given: string[] = [];
    const { result } = renderHook(() => useAgentTurn({ onGiveBack: (text) => given.push(text) }));
    act(() => void result.current.send("fix it"));
    await waitFor(() => expect(result.current.turn.status).toBe("running"));
    act(() => result.current.queue("one"));

    act(() => result.current.reset());
    expect(given).toEqual(["one"]);
    expect(result.current.queued).toEqual([]);
  });
});
