import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import { acceptEvent, appendUserMessage, restoredTurn } from "../lib/chatTurnReducer";

// Chat mode's turn is the agent's loop in a role: the window sends, answers the
// card, and saves the agent's blocks. And the chats saved before that — as
// bubbles, or only as what the model was sent — still open.

type Call = { command: string; args: Record<string, unknown> };
const calls: Call[] = [];
const results: Record<string, unknown> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    const answer = results[command];
    const result = typeof answer === "function" ? answer(args) : answer;
    return result instanceof Error ? Promise.reject(result.message) : Promise.resolve(result ?? null);
  },
}));

mock.module("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { usePlainChat, chatBlocks } = await import("../hooks/usePlainChat");

afterEach(() => {
  calls.length = 0;
  for (const key of Object.keys(results)) delete results[key];
  localStorage.clear();
});

const called = (command: string) => calls.filter((call) => call.command === command);

describe("a chat saved before the agent's blocks", () => {
  test("its bubbles become the blocks the reducer would have made", () => {
    const blocks = chatBlocks(
      [
        { role: "user", text: "hi" },
        { role: "assistant", text: "hello", reasoning: "greet back" },
      ],
      [],
    );
    expect(blocks).toEqual([
      { kind: "user", id: "user:0", text: "hi" },
      { kind: "reasoning", id: "turn:1:round:1:reasoning", round: 1, text: "greet back" },
      { kind: "message", id: "turn:1:round:1:message", round: 1, text: "hello" },
    ]);
  });

  test("with no bubbles, it is drawn from what the model was sent", () => {
    const blocks = chatBlocks(null, [
      { role: "user", content: "hi" },
      { role: "assistant", content: "hello" },
    ]);
    expect(blocks.map((b) => b.kind)).toEqual(["user", "message"]);
  });

  test("blocks are taken as they are", () => {
    const saved = [{ kind: "notice", id: "notice:0", text: "said" }];
    expect(chatBlocks(saved, [])).toBe(saved);
  });

  /// The point of reusing the reducer's ids: the next question's answer is a
  /// new block, not appended to the old answer halfway up the chat.
  test("a turn after it answers in a block of its own", () => {
    const old = restoredTurn(chatBlocks([{ role: "user", text: "hi" }, { role: "assistant", text: "hello" }], []));
    const asked = appendUserMessage(old, "and?");
    const answered = acceptEvent(asked, { turnId: "t", seq: 1, round: 1, type: "delta", payload: { delta: "more" } } as never);
    const texts = answered.blocks.filter((b) => b.kind === "message").map((b) => (b as { text: string }).text);
    expect(texts).toEqual(["hello", "more"]);
    expect(new Set(answered.blocks.map((b) => b.id)).size).toBe(answered.blocks.length);
  });
});

describe("a turn in a role", () => {
  const card = {
    status: "pendingApproval",
    value: {
      history: [{ role: "user", content: "delete it" }],
      round: 1,
      budgetUsed: 1,
      eventSeq: 3,
      calls: [{ id: "c1", name: "deleteFile", arguments: "{}", requiresConfirmation: true }],
      todos: [],
      reads: {},
    },
  };

  test("pauses on the card, and the answer resumes it with the checkpoint", async () => {
    results.plain_chat_send = card;
    results.plain_chat_resume = {
      status: "done",
      value: { text: "kept", history: [{ role: "user", content: "delete it" }, { role: "assistant", content: "kept" }], todos: [] },
    };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("delete it");
    });
    expect(result.current.turn.status).toBe("awaitingApproval");
    expect(result.current.busy).toBe(true);

    const decision = [{ id: "c1", approved: false, reason: "keep it" }];
    await act(async () => {
      await result.current.decide(decision, ["deleteFile"]);
    });

    expect(called("plain_chat_always_allow")[0]?.args).toEqual({ tool: "deleteFile" });
    const resumed = called("plain_chat_resume")[0]?.args;
    expect(resumed?.checkpoint).toEqual(card.value);
    expect(resumed?.decisions).toEqual(decision);
    expect(resumed?.role).toBe("assistant");
    // Saved at rest with what the model read, and the blocks as drawn.
    await waitFor(() => expect(called("plain_chat_save").length).toBe(2));
    const last = called("plain_chat_save")[1].args;
    expect(last.messages).toEqual([{ role: "user", content: "delete it" }, { role: "assistant", content: "kept" }]);
    expect((last.blocks as { kind: string }[]).some((b) => b.kind === "approval")).toBe(false);
  });

  test("while it waits, the chat is not left: another, or a new one, is refused", async () => {
    results.plain_chat_send = card;
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("delete it");
    });
    let opened = true;
    await act(async () => {
      opened = await result.current.open("other");
    });
    expect(opened).toBe(false);
    expect(result.current.newChat()).toBe(false);
    expect(called("chat_load")).toEqual([]);
  });

  test("a turn that fails says so in the transcript", async () => {
    results.plain_chat_send = new Error("no provider");
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("hi");
    });
    const notice = result.current.turn.blocks.find((b) => b.kind === "notice") as { text: string } | undefined;
    expect(notice?.text).toBe("The turn failed: no provider");
    expect(result.current.busy).toBe(false);
  });
});

describe("the cluster a chat works with", () => {
  test("a new chat starts at the kubeconfig picked last, and is pinned to it from its first message", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const { result } = renderHook(() => usePlainChat("prod"));
    expect(result.current.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
    await act(async () => {
      await result.current.send("pods?");
    });
    const sent = called("plain_chat_send")[0]?.args;
    expect(sent?.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
    expect(called("plain_chat_save")[0]?.args.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
  });

  test("a pin changed in a saved chat is kept with it at once", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const { result } = renderHook(() => usePlainChat("prod"));
    await act(async () => {
      await result.current.send("pods?");
    });
    await waitFor(() => expect(result.current.busy).toBe(false));
    const saves = called("plain_chat_save").length;
    act(() => result.current.setPin({ kubeconfig: "prod", context: "eks", namespace: "orders" }));
    const last = called("plain_chat_save").slice(saves);
    expect(last.map((call) => call.args.kube)).toEqual([{ kubeconfig: "prod", context: "eks", namespace: "orders" }]);
    expect(result.current.kube?.namespace).toBe("orders");
  });

  test("an opened chat works where it was pinned, whatever was picked last", async () => {
    const pin = { kubeconfig: "stg", context: "kind", namespace: "orders" };
    results.chat_load = { id: "c", messages: [], blocks: [], role: "kubernetes", kube: pin };
    const { result } = renderHook(() => usePlainChat("prod"));
    await act(async () => {
      await result.current.open("c");
    });
    expect(result.current.kube).toEqual(pin);
    expect(result.current.role).toBe("kubernetes");
  });
});
