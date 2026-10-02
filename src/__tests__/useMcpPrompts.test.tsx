import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";

// The `/` menu's MCP prompts: read as they are, or read once the servers not
// running yet are started — and while that takes, said to be coming.

const calls: string[] = [];
let answer: (command: string) => Promise<unknown> = () => Promise.resolve([]);

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    calls.push(command);
    return answer(command);
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useMcpPrompts } = await import("../hooks/useMcpPrompts");

const greet = { server: "a", name: "greet", arguments: [] };

afterEach(() => {
  calls.length = 0;
  answer = () => Promise.resolve([]);
});

describe("starting the servers for the menu", () => {
  test("their prompts are what the start answers", async () => {
    answer = (command) => Promise.resolve(command === "mcp_prompts_start" ? [greet] : []);
    const { result } = renderHook(() => useMcpPrompts("/work", "idle"));
    await waitFor(() => expect(calls).toContain("mcp_prompts"));

    act(() => result.current.start());
    await waitFor(() => expect(result.current.prompts).toEqual([greet]));
    expect(result.current.starting).toBe(false);
  });

  /// Servers already running answer at once: the line would only flash.
  test("a quick answer is never said to be coming", async () => {
    // Quick, not instant: under the threshold, but long enough for a line shown at once to be seen.
    answer = () => new Promise((resolve) => setTimeout(() => resolve([]), 60));
    const { result } = renderHook(() => useMcpPrompts("/work", "idle"));
    const seen: boolean[] = [];
    act(() => result.current.start());
    for (let i = 0; i < 10; i++) {
      await new Promise((r) => setTimeout(r, 15));
      seen.push(result.current.starting);
    }
    expect(seen.every((starting) => !starting)).toBe(true);
  });

  /// A first `npx` run takes seconds: the menu says the prompts are coming
  /// until they are there, and opening it again meanwhile starts nothing more.
  test("a slow start is said to be coming until it answers, and is not asked twice", async () => {
    let finish: (prompts: unknown) => void = () => {};
    answer = (command) => (command === "mcp_prompts_start" ? new Promise((resolve) => (finish = resolve)) : Promise.resolve([]));
    const { result } = renderHook(() => useMcpPrompts("/work", "idle"));

    act(() => result.current.start());
    await waitFor(() => expect(result.current.starting).toBe(true));
    act(() => result.current.start());
    expect(calls.filter((c) => c === "mcp_prompts_start")).toHaveLength(1);

    await act(async () => finish([greet]));
    expect(result.current.starting).toBe(false);
    expect(result.current.prompts).toEqual([greet]);
  });

  test("a start that fails leaves no prompts and stops saying they are coming", async () => {
    answer = (command) => (command === "mcp_prompts_start" ? Promise.reject("no folder") : Promise.resolve([greet]));
    const { result } = renderHook(() => useMcpPrompts("/work", "idle"));
    await waitFor(() => expect(result.current.prompts).toEqual([greet]));

    act(() => result.current.start());
    await waitFor(() => expect(result.current.prompts).toEqual([]));
    expect(result.current.starting).toBe(false);
  });
});
