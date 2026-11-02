import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";

// The folder picker has two answers, and one of them is "the user changed
// their mind". Telling them apart is the whole of this hook's new part.

const calls: { command: string; args: unknown }[] = [];
let opened: string | null = null;
let recent: string[] = [];
const folders = (paths: string[]) => paths.map((path) => ({ path, worktreeOf: null }));
let current: string | null = null;
let failOpen = false;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    if (command === "workspace_recent") return Promise.resolve(folders(recent));
    if (command === "workspace_current") return Promise.resolve(current);
    if (command === "workspace_open" && failOpen) return Promise.reject("cannot open: gone");
    return Promise.resolve(command === "workspace_open" ? (args as { path: string }).path : null);
  },
  // The dialog plugin imports this from the same module; a mock that omits it
  // fails at import time rather than at the call.
  transformCallback: (callback: unknown) => callback,
}));

// Module mocks are global to the run: every export the app imports from here
// has to be in it, or another file's import of `lib/dialog` fails.
mock.module("@tauri-apps/plugin-dialog", () => ({
  open: () => Promise.resolve(opened),
  save: () => Promise.resolve(null),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useWorkspace } = await import("../hooks/useWorkspace");

afterEach(() => {
  calls.length = 0;
  opened = null;
  recent = [];
  current = null;
  failOpen = false;
});

const settle = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
});

describe("coming back", () => {
  test("a new launch reopens the folder opened last, and says it came back", async () => {
    recent = ["/work/b", "/work/a"];
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(calls).toContainEqual({ command: "workspace_open", args: { path: "/work/b", keepRunning: false } });
    expect(result.current.path).toBe("/work/b");
    expect(result.current.recent).toEqual(folders(["/work/b", "/work/a"]));
    expect(result.current.resumed).toBe(true);
  });

  test("a reloaded window keeps the backend's folder and opens nothing", async () => {
    recent = ["/work/b"];
    current = "/work/a";
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(calls.filter((call) => call.command === "workspace_open")).toEqual([]);
    expect(result.current.path).toBe("/work/a");
    expect(result.current.resumed).toBe(true);
  });

  test("nothing opened before: nothing open, nothing resumed", async () => {
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(calls.filter((call) => call.command === "workspace_open")).toEqual([]);
    expect(result.current.path).toBeNull();
    expect(result.current.resumed).toBe(false);
  });

  test("a folder that will not open is not resumed", async () => {
    recent = ["/work/b"];
    failOpen = true;
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(result.current.path).toBeNull();
    expect(result.current.resumed).toBe(false);
  });

  test("a folder chosen by hand is not a resumed one", async () => {
    opened = "/work/new";
    const { result } = renderHook(() => useWorkspace());
    await settle();
    await act(async () => {
      await result.current.pick();
    });
    expect(result.current.path).toBe("/work/new");
    expect(result.current.resumed).toBe(false);
  });

  test("the list follows a folder opened by hand", async () => {
    opened = "/work/new";
    const { result } = renderHook(() => useWorkspace());
    await settle();
    recent = ["/work/new"];
    await act(async () => {
      await result.current.pick();
    });
    expect(result.current.recent).toEqual(folders(["/work/new"]));
  });
});

describe("picking a folder", () => {
  test("a chosen folder is opened", async () => {
    opened = "/Users/me/project";
    const { result } = renderHook(() => useWorkspace());

    await act(async () => {
      expect(await result.current.pick()).toBe(true);
    });

    expect(calls).toContainEqual({
      command: "workspace_open",
      args: { path: "/Users/me/project", keepRunning: false },
    });
    expect(result.current.path).toBe("/Users/me/project");
  });

  /// Cancelling is not a path. Passing it on opened the workspace at
  /// whatever `null` spells once it has been through a string.
  test("cancelling opens nothing", async () => {
    opened = null;
    const { result } = renderHook(() => useWorkspace());

    await act(async () => {
      expect(await result.current.pick()).toBe(false);
    });

    expect(calls.filter((call) => call.command === "workspace_open")).toEqual([]);
    expect(result.current.path).toBeNull();
    // And it is not a failure either: a toast saying something went wrong is
    // a strange answer to having pressed Cancel.
    expect(result.current.error).toBeNull();
  });
});
