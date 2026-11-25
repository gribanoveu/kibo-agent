import { afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";

// CM-3: the chip shows the open chat's own model. The pin travels from the
// record to the hook to `modelChoices`; a pick goes back through
// `chat_model_set`, and the global choice moves with it (docs/64-chat-model.md).

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

const listeners: ((event: { payload: Record<string, unknown> }) => void)[] = [];
mock.module("@tauri-apps/api/event", () => ({
  listen: (_name: string, cb: (event: { payload: Record<string, unknown> }) => void) => {
    listeners.push(cb);
    return Promise.resolve(() => {});
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { modelChoices, useLlmSettings } = await import("../hooks/useLlmSettings");
const { usePlainChat } = await import("../hooks/usePlainChat");
const { useAgentTurn } = await import("../hooks/useAgentTurn");

afterEach(() => {
  calls.length = 0;
  listeners.length = 0;
  for (const key of Object.keys(results)) delete results[key];
  localStorage.clear();
});

const called = (command: string) => calls.filter((call) => call.command === command);

const provider = (id: string, model: string | null, extra: Record<string, unknown> = {}) => ({
  id,
  baseUrl: `https://${id}.example/v1`,
  model,
  hasApiKey: true,
  ...extra,
});

const llm = (providers: ReturnType<typeof provider>[], activeProviderId: string | null) => ({
  providers,
  activeProviderId,
  debugLogging: false,
  replyLanguage: "auto" as const,
  turnLimits: { rounds: 100, budget: 500, parallel: 3 },
  keyStore: "file" as const,
});

describe("the chip's current choice", () => {
  test("is the open chat's pin, not the active provider", () => {
    const settings = llm([provider("a", "ma"), provider("b", "mb")], "b");
    const { current } = modelChoices(settings, {}, { provider: "a", model: "ma" });
    expect(current?.providerId).toBe("a");
    expect(current?.model).toBe("ma");
  });

  test("a chat without a pin shows the active provider", () => {
    const settings = llm([provider("a", "ma"), provider("b", "mb")], "b");
    const { current } = modelChoices(settings, {}, null);
    expect(current?.providerId).toBe("b");
    expect(current?.model).toBe("mb");
  });

  test("a pin whose provider is gone shows the active one, not 'no model'", () => {
    const settings = llm([provider("b", "mb")], "b");
    const { current } = modelChoices(settings, {}, { provider: "gone", model: "m" });
    expect(current?.providerId).toBe("b");
    expect(current?.model).toBe("mb");
  });

  test("a pinned model the provider no longer lists is still shown", () => {
    const settings = llm([provider("a", "ma")], "a");
    const { choices, current } = modelChoices(settings, { a: ["x", "y"] }, { provider: "a", model: "old" });
    expect(current?.model).toBe("old");
    expect(current?.label).toBe("a/old");
    expect(choices.some((c) => c.providerId === "a" && c.model === "old")).toBe(true);
  });

  test("auto in a pinned chat stays auto and names the model the provider would send", () => {
    const settings = llm([provider("a", "ma"), provider("b", "mb")], "b");
    // The provider's pin, not the first of the list: auto follows the pin.
    const { current } = modelChoices(settings, { a: ["listed", "ma"] }, { provider: "a", model: null });
    expect(current?.providerId).toBe("a");
    expect(current?.model).toBeNull();
    expect(current?.label).toBe("a/ma (auto)");

    // A pick in another chat moved the provider's pin: the auto chat follows.
    const moved = llm([provider("a", "new"), provider("b", "mb")], "b");
    expect(modelChoices(moved, { a: ["listed"] }, { provider: "a", model: null }).current?.label).toBe("a/new (auto)");
  });

  test("the thinking level is the pinned provider's own", () => {
    const settings = llm([provider("a", "ma", { reasoningEffort: "high" }), provider("b", "mb", { reasoningEffort: "low" })], "b");
    expect(modelChoices(settings, {}, { provider: "a", model: "ma" }).effort).toBe("high");
    expect(modelChoices(settings, {}, null).effort).toBe("low");
  });
});

describe("a pick in the chat's chip", () => {
  test("an opened chat exposes its pin, and a pick is written to it", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], role: "assistant", model: { provider: "a", model: "ma" } };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.open("c1");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: "ma" });

    act(() => result.current.pickChatModel("b", "mb"));
    await waitFor(() => expect(called("chat_model_set").length).toBe(1));
    expect(called("chat_model_set")[0].args).toEqual({ id: "c1", provider: "b", model: "mb" });
    expect(result.current.chatModel).toEqual({ provider: "b", model: "mb" });
  });

  test("an unsaved chat is stamped by its first save, not by a pick", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const choice = { providerId: "a", model: "ma", label: "a/ma" };
    const { result } = renderHook(() => usePlainChat(null, choice));
    act(() => result.current.pickChatModel("b", "mb"));
    expect(called("chat_model_set")).toEqual([]);
    expect(result.current.chatModel).toBeNull();

    await act(async () => {
      await result.current.send("hi");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: "ma" });
  });

  test("the agent's chat carries its pin the same way", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], todos: [], model: { provider: "a", model: null } };
    const { result } = renderHook(() => useAgentTurn());
    await act(async () => {
      await result.current.open("c1");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: null });

    act(() => result.current.pickChatModel("b", "mb"));
    await waitFor(() => expect(called("chat_model_set").length).toBe(1));
    expect(called("chat_model_set")[0].args).toEqual({ id: "c1", provider: "b", model: "mb" });
    expect(result.current.chatModel).toEqual({ provider: "b", model: "mb" });
  });

  test("the agent's first save stamps the pair the chip showed", async () => {
    results.chat_start = { status: "done", value: { text: "ok", todos: [], history: [{ role: "user", content: "hi" }, { role: "assistant", content: "ok" }] } };
    const choice = { providerId: "a", model: null, label: "a/ma (auto)" };
    const { result } = renderHook(() => useAgentTurn({ choice }));
    await act(async () => {
      await result.current.send("hi");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: null });
  });

  test("a saved chat keeps its pin through a later send, whatever the global choice", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], role: "assistant", model: { provider: "a", model: "ma" } };
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const choice = { providerId: "b", model: "mb", label: "b/mb" };
    const { result } = renderHook(() => usePlainChat(null, choice));
    await act(async () => {
      await result.current.open("c1");
    });
    await act(async () => {
      await result.current.send("again");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: "ma" });
  });

  test("a saved agent chat keeps its pin through a later turn", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], todos: [], model: { provider: "a", model: "ma" } };
    results.chat_start = { status: "done", value: { text: "ok", todos: [], history: [{ role: "user", content: "again" }, { role: "assistant", content: "ok" }] } };
    const choice = { providerId: "b", model: "mb", label: "b/mb" };
    const { result } = renderHook(() => useAgentTurn({ choice }));
    await act(async () => {
      await result.current.open("c1");
    });
    await act(async () => {
      await result.current.send("again");
    });
    expect(result.current.chatModel).toEqual({ provider: "a", model: "ma" });
  });

  test("the cache-miss hint compares the chat's model, not the global choice", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], todos: [], model: { provider: "a", model: "ma" } };
    results.chat_start = new Promise(() => {});
    const { result, rerender } = renderHook(({ choice }) => useAgentTurn({ choice }), {
      initialProps: { choice: { providerId: "a", model: "ma", label: "a/ma" } },
    });
    await act(async () => {
      await result.current.open("c1");
    });
    act(() => {
      void result.current.send("hi");
    });
    await waitFor(() => expect(called("chat_start").length).toBe(1));
    const turnId = called("chat_start")[0].args.turnId as string;
    const emit = (payload: Record<string, unknown>) => {
      act(() => {
        for (const cb of listeners) cb({ payload: { ...payload, turnId } });
      });
    };
    emit({ type: "contextUsage", seq: 1, round: 1, payload: { promptTokens: 30_000, completionTokens: 10, cachedTokens: 15_000 } });
    // The global choice moves in a neighbouring chat; this chat's pin stands.
    rerender({ choice: { providerId: "b", model: "mb", label: "b/mb" } });
    emit({ type: "contextUsage", seq: 2, round: 2, payload: { promptTokens: 30_000, completionTokens: 10, cachedTokens: 0 } });

    const notice = result.current.turn.blocks.find((b) => b.kind === "notice");
    expect(notice?.text).toContain("Cache miss");
    expect(notice?.text).not.toContain("after the model switch");
  });

  test("the Chat mode cache-miss hint compares the chat's model too", async () => {
    results.chat_load = { id: "c1", messages: [], blocks: [], role: "assistant", model: { provider: "a", model: "ma" } };
    results.plain_chat_send = new Promise(() => {});
    const { result, rerender } = renderHook(({ choice }) => usePlainChat(null, choice), {
      initialProps: { choice: { providerId: "a", model: "ma", label: "a/ma" } },
    });
    await act(async () => {
      await result.current.open("c1");
    });
    act(() => {
      void result.current.send("hi");
    });
    await waitFor(() => expect(called("plain_chat_send").length).toBe(1));
    const turnId = called("plain_chat_send")[0].args.turnId as string;
    const emit = (payload: Record<string, unknown>) => {
      act(() => {
        for (const cb of listeners) cb({ payload: { ...payload, turnId } });
      });
    };
    emit({ type: "contextUsage", seq: 1, round: 1, payload: { promptTokens: 30_000, completionTokens: 10, cachedTokens: 15_000 } });
    rerender({ choice: { providerId: "b", model: "mb", label: "b/mb" } });
    emit({ type: "contextUsage", seq: 2, round: 2, payload: { promptTokens: 30_000, completionTokens: 10, cachedTokens: 0 } });

    const notice = result.current.turn.blocks.find((b) => b.kind === "notice");
    expect(notice?.text).toContain("Cache miss");
    expect(notice?.text).not.toContain("after the model switch");
  });

  test("an unsaved agent chat is stamped by its first save, not by a pick", () => {
    const choice = { providerId: "a", model: "ma", label: "a/ma" };
    const { result } = renderHook(() => useAgentTurn({ choice }));
    act(() => result.current.pickChatModel("b", "mb"));
    expect(called("chat_model_set")).toEqual([]);
    expect(result.current.chatModel).toBeNull();
  });

  test("the stamp is there while the first turn still runs", async () => {
    results.chat_start = new Promise(() => {});
    const choice = { providerId: "a", model: "ma", label: "a/ma" };
    const { result } = renderHook(() => useAgentTurn({ choice }));
    act(() => {
      void result.current.send("hi");
    });
    await waitFor(() => expect(result.current.chatModel).toEqual({ provider: "a", model: "ma" }));
    expect(result.current.turn.status).toBe("running");
  });
});

describe("the settings behind the chip", () => {
  test("thinking is written to the chat's provider, not the active one", async () => {
    results.llm_settings_get = llm([provider("a", "ma"), provider("b", "mb")], "b");
    const { result } = renderHook(() => useLlmSettings());
    await waitFor(() => expect(result.current.settings).not.toBeNull());

    await act(async () => {
      await result.current.pickEffort("high", "a");
    });

    const save = called("llm_provider_save")[0];
    expect((save.args.provider as { id: string }).id).toBe("a");
    expect((save.args.provider as { reasoningEffort: string }).reasoningEffort).toBe("high");
  });

  test("a pick moves the global choice too — the next chat starts where this one ended", async () => {
    results.llm_settings_get = llm([provider("a", "ma"), provider("b", "mb")], "b");
    const { result } = renderHook(() => useLlmSettings());
    await waitFor(() => expect(result.current.settings).not.toBeNull());

    await act(async () => {
      await result.current.pickModel("a", "new");
    });

    expect(called("llm_active_provider_set")[0].args).toEqual({ id: "a" });
    const save = called("llm_provider_save")[0];
    expect((save.args.provider as { id: string }).id).toBe("a");
    expect((save.args.provider as { model: string }).model).toBe("new");
  });
});
