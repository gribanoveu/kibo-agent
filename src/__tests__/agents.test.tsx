import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { AgentInfo } from "../lib/chat";

// The Agents tab: every explore run, asked for while the tab is open, newest
// first, with its steps, its cost and its answer, and a Stop for a running one.

let listed: AgentInfo[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "agents_list") return Promise.resolve(structuredClone(listed));
    if (command === "agent_stop") {
      listed = listed.map((a) => (a.id === args!.id ? { ...a, state: { state: "stopped" as const } } : a));
      return Promise.resolve(structuredClone(listed));
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Set<(message: { payload: unknown }) => void>();
const changed = (id: number) => listeners.forEach((handler) => handler({ payload: { id } }));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (channel === "agents:changed") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useAgents } = await import("../hooks/useAgents");
const { AgentList } = await import("../components/AgentList");
const { tokensSpent } = await import("../lib/chat");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  listed = [
    {
      id: 2,
      task: "Where is the token refreshed?",
      state: { state: "running" },
      steps: ["grep refresh", "readFile src/auth.rs"],
      tokens: { prompt: 30000, cached: 27000, completion: 800 },
      answer: null,
    },
    {
      id: 1,
      task: "Which tests cover the parser?",
      state: { state: "done" },
      steps: ["grep parse"],
      tokens: { prompt: 900, cached: 0, completion: 60 },
      answer: "tests/parser.rs:12",
    },
  ];
  calls = [];
});

describe("useAgents", () => {
  test("asks when the tab opens and when a run changes, and not once it closes", async () => {
    const { result, rerender } = renderHook(({ visible }) => useAgents(visible), { initialProps: { visible: false } });
    const asked = () => calls.filter((c) => c.command === "agents_list").length;
    await settle();
    expect(calls).toEqual([]);
    rerender({ visible: true });
    await settle();
    expect(result.current.agents.map((a) => a.id)).toEqual([2, 1]);

    listed = [{ ...listed[0], steps: [...listed[0].steps, "readFile src/token.rs"] }, listed[1]];
    act(() => changed(2));
    await settle();
    expect(asked()).toBe(2);
    expect(result.current.agents[0].steps).toHaveLength(3);

    rerender({ visible: false });
    await settle();
    act(() => changed(2));
    await settle();
    expect(asked()).toBe(2);
  });

  test("a stop is settled by what the backend says", async () => {
    const { result } = renderHook(() => useAgents(true));
    await settle();
    await act(() => result.current.stop(2));
    expect(calls.at(-1)).toEqual({ command: "agent_stop", args: { id: 2 } });
    expect(result.current.agents[0].state).toEqual({ state: "stopped" });
  });
});

describe("the agent list", () => {
  test("shows each run, its cost, the newest open with its steps, and Stop only for a running one", () => {
    const stopped: number[] = [];
    render(<AgentList agents={listed} error={null} onStop={(id) => stopped.push(id)} />);
    expect(screen.getByText("Where is the token refreshed?")).toBeTruthy();
    expect(screen.getByText("31k tokens · 90% cached")).toBeTruthy();
    expect(screen.getByText("960 tokens")).toBeTruthy();
    expect(screen.getByText(/readFile src\/auth\.rs/)).toBeTruthy();
    expect(screen.queryByText("tests/parser.rs:12")).toBeNull();

    const stops = screen.getAllByText("Stop");
    expect(stops).toHaveLength(1);
    fireEvent.click(stops[0]);
    expect(stopped).toEqual([2]);

    fireEvent.click(screen.getByText("Which tests cover the parser?"));
    expect(screen.getByText("tests/parser.rs:12")).toBeTruthy();
    expect(screen.queryByText(/readFile src\/auth\.rs/)).toBeNull();
  });

  test("a run asked for from the chat opens once it is listed, and only if it is still that run", () => {
    // One ask, one object: the list is what changes, read after the tab opened for it.
    const focus = { id: 1, task: "Which tests cover the parser?" };
    const { rerender } = render(<AgentList agents={[]} error={null} onStop={() => {}} focus={focus} />);
    rerender(<AgentList agents={listed} error={null} onStop={() => {}} focus={focus} />);
    expect(screen.getByText("tests/parser.rs:12")).toBeTruthy();
  });

  test("a number from before a restart is not today's run with that number", () => {
    render(<AgentList agents={listed} error={null} onStop={() => {}} focus={{ id: 1, task: "something asked yesterday" }} />);
    expect(screen.queryByText("tests/parser.rs:12")).toBeNull();
    expect(screen.getByText(/readFile src\/auth\.rs/)).toBeTruthy();
  });

  test("a failed run says why", () => {
    render(
      <AgentList
        agents={[{ ...listed[1], state: { state: "failed", reason: "the helper used up its 60 rounds" }, answer: null }]}
        error={null}
        onStop={() => {}}
      />,
    );
    expect(screen.getByText("failed")).toBeTruthy();
    expect(screen.getByText("the helper used up its 60 rounds")).toBeTruthy();
  });

  test("with no runs it says what would start one", () => {
    render(<AgentList agents={[]} error={null} onStop={() => {}} />);
    expect(screen.getByText(/No helper agents yet/)).toBeTruthy();
  });

  test("nothing reported is no cost shown, not zero", () => {
    expect(tokensSpent({ prompt: 0, cached: 0, completion: 0 })).toBe("");
  });
});
