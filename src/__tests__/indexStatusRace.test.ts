import { afterAll, describe, expect, mock, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";

// The title bar's index badge: kept from the index's events, with a snapshot
// for a window that was not listening. The snapshot has to be read after the
// listener is attached, or a sync finishing in between is said by neither.

/** What the backend's index says now, and whether a listener is attached yet. */
let syncing = true;
let attach: (() => void) | null = null;
const handlers: ((message: { payload: unknown }) => void)[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) =>
    Promise.resolve(
      command === "workspace_index_status"
        ? { root: "/repo", syncing, embedded: syncing ? 0 : 12, skipped: 0, embeddingError: null }
        : null,
    ),
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/api/event", () => ({
  // Attached when the test says so: subscribing takes a round trip to the backend.
  listen: (_channel: string, handler: (message: { payload: unknown }) => void) =>
    new Promise<() => void>((resolve) => {
      attach = () => {
        handlers.push(handler);
        resolve(() => {});
      };
    }),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useIndexStatus } = await import("../hooks/useIndexStatus");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

describe("the index badge", () => {
  test("a sync that ends before the listener is attached is not left showing as running", async () => {
    const { result } = renderHook(() => useIndexStatus("/repo"));
    await settle();
    // The sync ends while the subscription is still on its way: its
    // `syncFinished` reaches nobody.
    syncing = false;
    await act(async () => attach?.());
    await settle();
    expect(result.current?.phase).toBe("ready");
    expect(result.current?.embedded).toBe(12);

    // From here events speak, and win over what the snapshot said.
    await act(async () => handlers.forEach((handle) => handle({ payload: { root: "/repo", kind: "syncStarted" } })));
    expect(result.current?.phase).toBe("syncing");
  });

  test("an event heard before the snapshot answers is not overridden by it", async () => {
    syncing = false;
    handlers.length = 0;
    const { result } = renderHook(() => useIndexStatus("/repo"));
    await settle();
    // Attached, and a new sync starts before the snapshot's answer lands.
    await act(async () => {
      attach?.();
      handlers.forEach((handle) => handle({ payload: { root: "/repo", kind: "syncStarted" } }));
    });
    await settle();
    expect(result.current?.phase).toBe("syncing");
  });
});
