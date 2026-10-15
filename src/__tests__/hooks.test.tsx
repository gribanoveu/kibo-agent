import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, render, renderHook, screen } from "@testing-library/react";
import type { HooksView } from "../lib/agentHooks";

// The hooks file is the user's: the tab re-reads it and lists each command
// with what it runs on, and a hook that will not run says why rather than
// sitting there looking configured.

let disk: HooksView;
let calls: string[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push(command);
    if (command === "hooks_config_get") return Promise.resolve(structuredClone(disk));
    if (command === "hooks_config_save") {
      const text = args!.text as string;
      try {
        JSON.parse(text);
      } catch {
        return Promise.reject("the hooks configuration is not valid: EOF");
      }
      disk = { ...disk, text };
      return Promise.resolve(structuredClone(disk));
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useHooks } = await import("../hooks/useHooks");
const { HooksList } = await import("../components/panes");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  disk = {
    path: "/home/.kibo/hooks.json",
    text: '{\n  "hooks": {}\n}\n',
    hooks: [
      { event: "PreToolUse", matcher: "runCommand", command: "guard.sh", timeoutSecs: 60, problem: null },
      { event: "PostToolUse", matcher: "", command: "prettier --write", timeoutSecs: 30, problem: null },
      { event: "Stop", matcher: "", command: "say done", timeoutSecs: 60, problem: null },
      { event: "PreToolUse", matcher: "Bash", command: "from-claude", timeoutSecs: 60, problem: "names none of this app's tools" },
    ],
  };
  calls = [];
});

describe("useHooks", () => {
  test("reads the file when shown, and not while hidden", async () => {
    const { result, rerender } = renderHook(({ visible }) => useHooks(visible), { initialProps: { visible: false } });
    await settle();
    expect(calls).toEqual([]);
    rerender({ visible: true });
    await settle();
    expect(result.current.view?.hooks).toHaveLength(4);
  });

  test("a refused save says why and reports it was not stored", async () => {
    const { result } = renderHook(() => useHooks(true));
    await settle();
    let stored = true;
    await act(async () => {
      stored = await result.current.save("{");
    });
    expect(stored).toBe(false);
    expect(result.current.error).toContain("not valid");
    await act(async () => {
      stored = await result.current.save('{"hooks":{}}');
    });
    expect(stored).toBe(true);
    expect(result.current.error).toBeNull();
  });
});

describe("the Hooks tab", () => {
  test("lists what each hook runs on, and one that will not run says why", () => {
    const actions: string[] = [];
    render(
      <HooksList
        view={disk}
        error={null}
        onAdd={() => actions.push("add")}
        onEditHook={(i) => actions.push(`edit ${i}`)}
        onRemoveHook={(i) => actions.push(`remove ${i}`)}
        onEditFile={() => actions.push("file")}
      />,
    );
    expect(screen.getByText("PreToolUse · runCommand")).toBeTruthy();
    expect(screen.getByText("PostToolUse · every tool")).toBeTruthy();
    expect(screen.getByText("Stop")).toBeTruthy();
    expect(screen.getByText("Stopped after 30 s")).toBeTruthy();
    expect(screen.getByText("won't run")).toBeTruthy();
    expect(screen.getByText("names none of this app's tools")).toBeTruthy();
    expect(screen.getByText("3/4")).toBeTruthy();
    act(() => screen.getByText("Add a hook").click());
    act(() => screen.getByText("Edit JSON").click());
    // The row is the hook's place in the file: the form reads it back by that.
    act(() => screen.getByText("prettier --write").click());
    act(() => screen.getByText("Edit").click());
    expect(actions).toEqual(["add", "file", "edit 1"]);
  });
});
