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
// The backend's registry: the last activated first, as `workspaces_list` answers.
let openSet: string[] = [];
// `workspaces.json`: the folders open last time, the last activated first;
// `null` when it was never written.
let saved: string[] | null = null;
let worktreeOf: Record<string, string> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    const path = (args as { path?: string } | undefined)?.path ?? "";
    if (command === "workspace_recent") return Promise.resolve(folders(recent));
    if (command === "workspace_current") return Promise.resolve(current);
    if (command === "workspaces_list")
      return Promise.resolve(openSet.map((one) => ({ path: one, active: one === current, worktreeOf: worktreeOf[one] ?? null })));
    if (command === "workspace_add") {
      if (failOpen) return Promise.reject("cannot open: gone");
      openSet = [path, ...openSet.filter((one) => one !== path)];
      current = path;
      return Promise.resolve(path);
    }
    if (command === "workspaces_restore") {
      if (current) return Promise.resolve(current);
      // As the backend: with no open set saved, the folder opened last.
      const reopen = saved ?? (recent[0] && !failOpen ? [recent[0]] : []);
      for (const one of [...reopen].reverse()) {
        openSet = [one, ...openSet.filter((other) => other !== one)];
        current = one;
      }
      return Promise.resolve(current);
    }
    if (command === "workspace_close") {
      openSet = openSet.filter((one) => one !== path);
      if (current === path) current = openSet[0] ?? null;
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
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
  openSet = [];
  saved = null;
  worktreeOf = {};
});

const settle = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
});

describe("coming back", () => {
  /// F-8.8: every folder open last time, the one activated last active.
  test("a launch reopens every folder open last time, the last activated active", async () => {
    saved = ["/work/b", "/work/a"];
    recent = ["/work/c"];
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(result.current.path).toBe("/work/b");
    expect(result.current.folders).toEqual(["/work/a", "/work/b"]);
    expect(result.current.resumed).toBe(true);
    expect(calls.filter((call) => call.command === "workspace_add")).toEqual([]);
  });

  test("with none open last time, a launch reopens the folder opened last, and says it came back", async () => {
    recent = ["/work/b", "/work/a"];
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(result.current.path).toBe("/work/b");
    expect(result.current.recent).toEqual(folders(["/work/b", "/work/a"]));
    expect(result.current.resumed).toBe(true);
  });

  test("a reloaded window keeps the backend's folder and opens nothing", async () => {
    recent = ["/work/b"];
    current = "/work/a";
    openSet = ["/work/a", "/work/c"];
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(calls.filter((call) => call.command === "workspace_add")).toEqual([]);
    expect(result.current.path).toBe("/work/a");
    expect(result.current.folders).toEqual(["/work/c", "/work/a"]);
    expect(result.current.resumed).toBe(true);
  });

  /// Closing every folder is deliberate: the recent one does not come back either.
  test("every folder closed last time: the window starts empty", async () => {
    saved = [];
    recent = ["/work/b"];
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(result.current.path).toBeNull();
    expect(result.current.folders).toEqual([]);
    expect(result.current.resumed).toBe(false);
    expect(result.current.recent).toEqual(folders(["/work/b"]));
  });

  test("nothing opened before: nothing open, nothing resumed", async () => {
    const { result } = renderHook(() => useWorkspace());
    await settle();
    expect(calls.filter((call) => call.command === "workspace_add")).toEqual([]);
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

    expect(calls).toContainEqual({ command: "workspace_add", args: { path: "/Users/me/project" } });
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

    expect(calls.filter((call) => call.command === "workspace_add")).toEqual([]);
    expect(result.current.path).toBeNull();
    // And it is not a failure either: a toast saying something went wrong is
    // a strange answer to having pressed Cancel.
    expect(result.current.error).toBeNull();
  });
});

// F-8.6: a folder opens beside the others, and closes on its own.
describe("open folders", () => {
  test("a worktree among them is known by its main checkout", async () => {
    worktreeOf = { "/wt/api/fix": "/work/api" };
    const { result } = renderHook(() => useWorkspace());
    await settle();
    for (const path of ["/work/api", "/wt/api/fix"]) {
      await act(async () => {
        await result.current.open(path);
      });
    }
    expect(result.current.worktrees).toEqual({ "/wt/api/fix": "/work/api" });
  });

  test("opening a folder adds it and makes it active; the others stay open, in their places", async () => {
    const { result } = renderHook(() => useWorkspace());
    await settle();
    for (const path of ["/work/a", "/work/b", "/work/c"]) {
      await act(async () => {
        await result.current.open(path);
      });
    }
    expect(result.current.folders).toEqual(["/work/a", "/work/b", "/work/c"]);

    // Back to a folder already open: active again, and its section does not move.
    await act(async () => {
      await result.current.open("/work/a");
    });
    expect(result.current.path).toBe("/work/a");
    expect(result.current.folders).toEqual(["/work/a", "/work/b", "/work/c"]);
  });

  test("closing a folder leaves the rest, and the backend's next one is active", async () => {
    const { result } = renderHook(() => useWorkspace());
    await settle();
    for (const path of ["/work/a", "/work/b"]) {
      await act(async () => {
        await result.current.open(path);
      });
    }
    // Not the active one: it stays active.
    await act(async () => {
      expect(await result.current.close("/work/a")).toBe(true);
    });
    expect(calls).toContainEqual({ command: "workspace_close", args: { path: "/work/a" } });
    expect(result.current.folders).toEqual(["/work/b"]);
    expect(result.current.path).toBe("/work/b");

    // The active one: the backend's next is active — here none is left.
    await act(async () => {
      await result.current.close("/work/b");
    });
    expect(result.current.folders).toEqual([]);
    expect(result.current.path).toBeNull();
  });

  test("the active folder has its section even when the open set cannot be read", async () => {
    current = "/work/a";
    const { result } = renderHook(() => useWorkspace());
    await settle();
    // `workspaces_list` answers nothing here: the mock's set is empty, as a failed read leaves it.
    expect(result.current.path).toBe("/work/a");
    expect(result.current.folders).toEqual(["/work/a"]);
  });
});

describe("keepOrder", () => {
  test("keeps the places shown, drops the closed, and puts the new at the end", async () => {
    const { keepOrder } = await import("../hooks/useWorkspace");
    expect(keepOrder(["/a", "/b", "/c"], ["/b", "/d", "/a"])).toEqual(["/a", "/b", "/d"]);
    expect(keepOrder([], ["/c", "/b", "/a"])).toEqual(["/a", "/b", "/c"]);
  });
});
