import { describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";

// The header's branch: asked for when the folder opens, when a turn ends, and
// whenever HEAD moves — including in the middle of a turn, when the agent
// switched it with a command.

let branch: string | null = "main";
let asked = 0;
let main: string | null = null;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    if (command === "workspace_worktree_of") return Promise.resolve(main);
    if (command === "workspace_branch") {
      asked++;
      return Promise.resolve(branch);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

const listeners = new Set<(message: { payload: unknown }) => void>();
mock.module("@tauri-apps/api/event", () => ({
  listen: (_channel: string, handler: (message: { payload: unknown }) => void) => {
    listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));
const gitChanged = (root: string) => listeners.forEach((handler) => handler({ payload: { root } }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { useGitBranch, useWorktreeOf } = await import("../hooks/useGitBranch");

describe("the checked-out branch", () => {
  test("is read for the open folder and again after a turn", async () => {
    asked = 0;
    branch = "main";
    const { result, rerender } = renderHook(({ status }) => useGitBranch("/work/a", status), {
      initialProps: { status: "idle" },
    });
    await waitFor(() => expect(result.current).toBe("main"));

    rerender({ status: "running" });
    await waitFor(() => expect(asked).toBe(2));

    branch = "feature/x";
    rerender({ status: "done" });
    await waitFor(() => expect(result.current).toBe("feature/x"));
  });

  // The agent ran `git switch` in the middle of a turn.
  test("follows HEAD while a turn is running", async () => {
    asked = 0;
    branch = "main";
    const { result } = renderHook(() => useGitBranch("/work/a", "running"));
    await waitFor(() => expect(result.current).toBe("main"));

    branch = "feature/y";
    act(() => gitChanged("/work/a"));
    await waitFor(() => expect(result.current).toBe("feature/y"));
  });

  // A switch from the branch menu, or one made in a terminal, moves HEAD.
  test("is read again when its own repository says HEAD moved", async () => {
    asked = 0;
    branch = "main";
    const { result } = renderHook(() => useGitBranch("/work/a", "idle"));
    await waitFor(() => expect(result.current).toBe("main"));

    branch = "other";
    act(() => gitChanged("/work/b"));
    expect(asked).toBe(1);
    act(() => gitChanged("/work/a"));
    await waitFor(() => expect(result.current).toBe("other"));
  });

  test("is nothing with no folder open", () => {
    asked = 0;
    const { result } = renderHook(() => useGitBranch(null, "idle"));
    expect(result.current).toBeNull();
    expect(asked).toBe(0);
  });
});

describe("the repository a worktree belongs to", () => {
  test("is read for each folder, and cleared at once on a switch", async () => {
    main = "/work/kibo";
    const { result, rerender } = renderHook(({ folder }) => useWorktreeOf(folder), {
      initialProps: { folder: "/wt/kibo/main" as string | null },
    });
    await waitFor(() => expect(result.current).toBe("/work/kibo"));

    main = null;
    rerender({ folder: "/work/kibo" });
    expect(result.current).toBeNull();
    rerender({ folder: null });
    expect(result.current).toBeNull();
  });
});
