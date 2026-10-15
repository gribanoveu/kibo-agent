import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import type { ChangeTotals } from "../lib/git";

// The folder tab's count of lines added and removed since the last commit:
// read when the folder opens and on the backend's change events for it.

let totals: ChangeTotals | null;
let reads = 0;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    if (command === "git_totals") {
      reads++;
      return Promise.resolve(totals);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Map<string, Set<(message: { payload: unknown }) => void>>();
const emit = (channel: string, payload: unknown) => listeners.get(channel)?.forEach((handler) => handler({ payload }));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (!listeners.has(channel)) listeners.set(channel, new Set());
    listeners.get(channel)!.add(handler);
    return Promise.resolve(() => listeners.get(channel)!.delete(handler));
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useChangeTotals } = await import("../hooks/useChangeTotals");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  totals = { files: 2, add: 12, del: 3 };
  reads = 0;
});

describe("useChangeTotals", () => {
  test("reads when the folder opens and on its own change events only", async () => {
    const { result } = renderHook(() => useChangeTotals("/repo"));
    await settle();
    expect(result.current).toEqual({ files: 2, add: 12, del: 3 });

    totals = { files: 3, add: 20, del: 3 };
    act(() => emit("workspace-index:event", { root: "/repo", kind: "syncStarted" }));
    await settle();
    expect(result.current?.add).toBe(20);

    totals = { files: 3, add: 25, del: 3 };
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(result.current?.add).toBe(25);

    const before = reads;
    act(() => {
      emit("workspace-git:changed", { root: "/elsewhere" });
      emit("workspace-index:event", { root: "/elsewhere", kind: "syncStarted" });
      emit("workspace-index:event", { root: "/repo", kind: "keywordsReady" });
    });
    await settle();
    expect(reads).toBe(before);
  });

  test("no folder, nothing to count; a folder left stops being listened to", async () => {
    const { result, rerender } = renderHook(({ path }) => useChangeTotals(path), {
      initialProps: { path: null as string | null },
    });
    await settle();
    expect(result.current).toBeNull();
    expect(reads).toBe(0);

    rerender({ path: "/repo" });
    await settle();
    rerender({ path: null });
    await settle();
    expect(result.current).toBeNull();
    const before = reads;
    act(() => emit("workspace-git:changed", { root: "/repo" }));
    await settle();
    expect(reads).toBe(before);
  });
});
