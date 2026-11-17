import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import type { IndexEvent, IndexSnapshot } from "../lib/chat";

// The first sync starts inside `workspace_add`, before the window knows the
// folder's path; the hook must not lose what it said meanwhile.

let emit: (event: IndexEvent) => void = () => {};
let snapshot: IndexSnapshot | null = null;
/** The folders the snapshot was asked for. */
let asked: unknown[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: { workspace?: string }) => {
    if (command !== "workspace_index_status") return Promise.resolve(null);
    asked.push(args?.workspace);
    return Promise.resolve(snapshot);
  },
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/api/event", () => ({
  listen: (_: string, handler: (message: { payload: IndexEvent }) => void) => {
    emit = (event) => handler({ payload: event });
    return Promise.resolve(() => {});
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useIndexStatus } = await import("../hooks/useIndexStatus");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  snapshot = null;
  asked = [];
});

describe("useIndexStatus", () => {
  test("events heard before the folder was known still count", async () => {
    const { result, rerender } = renderHook(({ path }) => useIndexStatus(path), {
      initialProps: { path: null as string | null },
    });
    await settle();
    act(() => emit({ root: "/repo", kind: "syncFinished", embedded: 5, embeddingError: null }));
    expect(result.current).toBeNull();

    rerender({ path: "/repo" });
    await settle();

    expect(result.current?.phase).toBe("ready");
    expect(result.current?.embedded).toBe(5);
  });

  test("a reloaded window starts from the snapshot, which never overrides an event", async () => {
    snapshot = { root: "/repo", syncing: false, embedded: 7, skipped: 1, embeddingError: null };
    const { result } = renderHook(() => useIndexStatus("/repo"));
    await settle();
    expect(result.current?.embedded).toBe(7);
    // Every open folder keeps an index (F-8.2): the snapshot is this folder's.
    expect(asked).toEqual(["/repo"]);

    act(() => emit({ root: "/repo", kind: "embeddingProgress", done: 1, total: 4 }));
    expect(result.current?.phase).toBe("embedding");
    expect(result.current?.skipped).toBe(1);
  });

  test("a stale snapshot loses to what the events already said", async () => {
    const { result, rerender } = renderHook(({ path }) => useIndexStatus(path), {
      initialProps: { path: null as string | null },
    });
    await settle();
    act(() => emit({ root: "/repo", kind: "embeddingProgress", done: 2, total: 4 }));
    snapshot = { root: "/repo", syncing: false, embedded: 0, skipped: 0, embeddingError: null };

    rerender({ path: "/repo" });
    await settle();

    expect(result.current?.phase).toBe("embedding");
  });

  test("another folder's events are not this folder's", async () => {
    const { result } = renderHook(() => useIndexStatus("/repo"));
    await settle();
    act(() => emit({ root: "/elsewhere", kind: "failed", error: "gone" }));
    expect(result.current).toBeNull();
  });
});
