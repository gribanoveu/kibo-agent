import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { AgentInfo } from "../lib/agents";

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
const { AgentSheet } = await import("../components/AgentSheet");
const { tokensSpent } = await import("../lib/agents");
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
    const { result, rerender } = renderHook(({ visible }) => useAgents(visible, "c"), { initialProps: { visible: false } });
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
    const { result } = renderHook(() => useAgents(true, "c"));
    await settle();
    await act(() => result.current.stop(2));
    expect(calls.at(-1)).toEqual({ command: "agent_stop", args: { id: 2, chatId: "c" } });
    expect(result.current.agents[0].state).toEqual({ state: "stopped" });
  });

  test("lists the open chat's, reads again for another chat, and nothing for a chat not started", async () => {
    const { result, rerender } = renderHook(({ chat }) => useAgents(true, chat), { initialProps: { chat: "a" as string | null } });
    await settle();
    expect(calls).toEqual([{ command: "agents_list", args: { chatId: "a" } }]);
    expect(result.current.agents).toHaveLength(2);

    listed = [listed[1]];
    rerender({ chat: "b" });
    // Till b's list arrives, a's is not shown under it.
    expect(result.current.agents).toEqual([]);
    await settle();
    expect(calls.at(-1)).toEqual({ command: "agents_list", args: { chatId: "b" } });
    expect(result.current.agents.map((a) => a.id)).toEqual([1]);

    rerender({ chat: null });
    await settle();
    expect(calls).toHaveLength(2);
    expect(result.current.agents).toEqual([]);
  });
});

describe("the agent list", () => {
  test("a line per run: what it was asked, the tool it is calling now or how it ended, Stop only while it runs", () => {
    const stopped: number[] = [];
    render(<AgentList agents={listed} error={null} onStop={(id) => stopped.push(id)} onOpen={() => {}} />);
    const rows = [...document.querySelectorAll(".agent-open")];
    expect(rows.map((row) => row.textContent)).toEqual(["Where is the token refreshed?readFile", "Which tests cover the parser?done"]);
    // The detail is the sheet's: no brief, steps or cost here.
    expect(screen.queryByText("31k tokens · 90% cached")).toBeNull();
    expect(screen.queryByText("src/auth.rs")).toBeNull();

    fireEvent.click(screen.getByLabelText("Stop"));
    expect(stopped).toEqual([2]);
    expect(screen.getAllByLabelText("Stop")).toHaveLength(1);
  });

  test("a click opens the run, by its number and its task; the row, Stop included, is the sheet's opener", () => {
    const opened: unknown[] = [];
    render(<AgentList agents={listed} error={null} onStop={() => {}} onOpen={(agent) => opened.push(agent)} />);
    fireEvent.click(screen.getByText("Which tests cover the parser?"));
    expect(opened).toEqual([{ id: 1, task: "Which tests cover the parser?" }]);
    expect(screen.getByLabelText("Stop").closest("[data-sheet-opener]")).toBeTruthy();
    expect(screen.queryByText("tests/parser.rs:12")).toBeNull();
  });

  test("with no runs it says what would start one", () => {
    render(<AgentList agents={[]} error={null} onStop={() => {}} onOpen={() => {}} />);
    expect(screen.getByText(/No helper agents yet/)).toBeTruthy();
  });

  test("nothing reported is no cost shown, not zero", () => {
    expect(tokensSpent({ prompt: 0, cached: 0, completion: 0 })).toBe("");
  });
});

describe("the agent sheet", () => {
  const sheet = (run: { id: number; task: string }, onClose = () => {}) => (
    <>
      <button type="button" data-sheet-opener>
        another run
      </button>
      <p>the chat</p>
      <AgentSheet chatId="c" run={run} onClose={onClose} />
    </>
  );

  test("the whole brief, how it stands, what it cost, the tools it called — the last one under way — and Stop while it runs", async () => {
    const brief = "Repository: /work/kibo\nGoal: Where is the token refreshed?\nLook under src/auth.";
    listed[0] = { ...listed[0], task: brief };
    render(sheet({ id: 2, task: brief }));
    await settle();
    expect(screen.getByRole("complementary", { name: "Where is the token refreshed?" })).toBeTruthy();
    expect(document.querySelector(".agent-task")?.textContent).toBe(brief);
    expect(screen.getByText("#2")).toBeTruthy();
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.getByText("2 steps")).toBeTruthy();
    expect(screen.getByText("31k tokens · 90% cached")).toBeTruthy();
    const steps = [...document.querySelectorAll(".agent-steps li")];
    expect(steps.map((li) => li.textContent)).toEqual(["greprefresh", "readFilesrc/auth.rs"]);
    expect(steps.map((li) => li.className)).toEqual(["", "now"]);

    fireEvent.click(screen.getByText("Stop"));
    await settle();
    expect(calls.at(-1)).toEqual({ command: "agent_stop", args: { id: 2, chatId: "c" } });
    expect(screen.getByText("stopped")).toBeTruthy();
    expect(screen.queryByText("Stop")).toBeNull();
    expect(document.querySelector(".agent-steps li.now")).toBeNull();
  });

  test("a running helper moves on in it: each change is read again, nothing polls", async () => {
    render(sheet({ id: 2, task: "Where is the token refreshed?" }));
    await settle();
    const asked = () => calls.filter((c) => c.command === "agents_list").length;
    expect(asked()).toBe(1);
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(asked()).toBe(1);

    listed[0] = { ...listed[0], state: { state: "done" }, steps: [...listed[0].steps, "readFile src/token.rs"], answer: "src/token.rs:40" };
    act(() => changed(2));
    await settle();
    expect(asked()).toBe(2);
    expect(screen.getByText("3 steps")).toBeTruthy();
    expect(screen.getByText("src/token.rs:40")).toBeTruthy();
    expect(screen.queryByText("Stop")).toBeNull();
  });

  test("a failed run says why", async () => {
    listed[1] = { ...listed[1], state: { state: "failed", reason: "the helper used up its 60 rounds" }, answer: null };
    render(sheet({ id: 1, task: "Which tests cover the parser?" }));
    await settle();
    expect(screen.getByText("failed")).toBeTruthy();
    expect(screen.getByText("the helper used up its 60 rounds")).toBeTruthy();
  });

  test("a number from before a restart is not today's run with that number", async () => {
    render(sheet({ id: 1, task: "something asked yesterday" }));
    // Not said before the list is read: it may still be on its way.
    expect(screen.queryByText("This run is no longer listed.")).toBeNull();
    await settle();
    expect(screen.getByText("something asked yesterday")).toBeTruthy();
    expect(screen.getByText("This run is no longer listed.")).toBeTruthy();
    expect(screen.queryByText("tests/parser.rs:12")).toBeNull();
  });

  test("Escape or a click outside closes it; a click in it or on another run's opener does not", async () => {
    let closed = 0;
    render(sheet({ id: 1, task: "Which tests cover the parser?" }, () => closed++));
    await settle();
    fireEvent.pointerDown(screen.getByText("tests/parser.rs:12"));
    fireEvent.pointerDown(screen.getByText("another run"));
    expect(closed).toBe(0);
    fireEvent.pointerDown(screen.getByText("the chat"));
    expect(closed).toBe(1);
    fireEvent.keyDown(document, { key: "Escape", code: "Escape" });
    expect(closed).toBe(2);
    fireEvent.click(screen.getByLabelText("Close panel"));
    expect(closed).toBe(3);
  });
});
