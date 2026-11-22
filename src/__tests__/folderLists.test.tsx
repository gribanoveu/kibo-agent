import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import type { ChatGroup, ChatSummary } from "../lib/savedChats";

// F-8.6: the sidebar lists every open folder's chats and chat folders, each
// read from its own folder, and a chat is known by the folder it is listed in.

const chat = (id: string): ChatSummary => ({ id, title: id, updatedAt: 1, archived: false });
let chats: Record<string, ChatSummary[]> = {};
let groups: Record<string, ChatGroup[]> = {};
let failing: string | null = null;
/** While set, a list read now answers only once it settles — with what it read now. */
let hold: Promise<void> | null = null;
let asked: { command: string; args: Record<string, unknown> }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown> = {}) => {
    asked.push({ command, args });
    const scope = (args.workspace as string | null) ?? (args.list as string | undefined) ?? "";
    if (scope === failing) return Promise.reject("unreadable");
    if (command === "chat_list") {
      const read = chats[scope] ?? [];
      return (hold ?? Promise.resolve()).then(() => read);
    }
    if (command === "chat_group_list") return Promise.resolve(groups[scope] ?? []);
    if (command === "chat_group_create") {
      const made = { id: `${scope}:${args.name}`, name: args.name as string };
      groups[scope] = [...(groups[scope] ?? []), made];
      return Promise.resolve(made);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});
afterEach(() => {
  chats = {};
  groups = {};
  failing = null;
  asked = [];
});

const { useChatHistory } = await import("../hooks/useChatHistory");
const { useChatGroups } = await import("../hooks/useChatGroups");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

describe("useChatHistory", () => {
  test("each open folder's chats, the active one's apart, and the folder of a chat", async () => {
    chats = { "/a": [chat("a1")], "/b": [chat("b1"), chat("b2")] };
    const { result } = renderHook(() => useChatHistory(["/a", "/b"], "/b"));
    await settle();
    expect(result.current.byFolder).toEqual({ "/a": [chat("a1")], "/b": [chat("b1"), chat("b2")] });
    expect(result.current.chats.map((c) => c.id)).toEqual(["b1", "b2"]);
    expect(result.current.folderOf("a1")).toBe("/a");
    expect(result.current.folderOf("b2")).toBe("/b");
    expect(result.current.folderOf("gone")).toBeNull();
  });

  test("the active folder is listed before the open set has caught up with it", async () => {
    chats = { "/new": [chat("n1")] };
    const { result } = renderHook(() => useChatHistory([], "/new"));
    await settle();
    expect(result.current.chats.map((c) => c.id)).toEqual(["n1"]);
  });

  test("a refresh re-reads every folder; one that fails keeps what it showed", async () => {
    chats = { "/a": [chat("a1")], "/b": [chat("b1")] };
    const { result } = renderHook(() => useChatHistory(["/a", "/b"], "/a"));
    await settle();
    chats = { "/a": [chat("a1"), chat("a2")], "/b": [] };
    failing = "/b";
    act(() => result.current.refresh());
    await settle();
    expect(result.current.byFolder["/a"].map((c) => c.id)).toEqual(["a1", "a2"]);
    expect(result.current.byFolder["/b"].map((c) => c.id)).toEqual(["b1"]);
  });

  // A refresh that started before a folder opened (a save, the worktree
  // move) lands after the new folder's own read: it neither blanks the new
  // section nor brings a closed one back.
  test("a refresh that started before the folders changed lands into the folders of now", async () => {
    chats = { "/a": [chat("a1")], "/gone": [chat("g1")] };
    const { result, rerender } = renderHook(({ open }) => useChatHistory(open, "/a"), { initialProps: { open: ["/a", "/gone"] } });
    await settle();
    let release = () => {};
    hold = new Promise<void>((resolve) => (release = resolve));
    act(() => result.current.refresh());
    hold = null;

    chats["/wt"] = [chat("moved")];
    rerender({ open: ["/a", "/wt"] });
    await settle();
    expect(result.current.folderOf("moved")).toBe("/wt");

    release();
    await settle();
    expect(result.current.folderOf("moved")).toBe("/wt");
    expect(result.current.folderOf("g1")).toBeNull();
    expect(Object.keys(result.current.byFolder).sort()).toEqual(["/a", "/wt"]);
  });

  test("a folder closed drops out of the lists", async () => {
    chats = { "/a": [chat("a1")], "/b": [chat("b1")] };
    const { result, rerender } = renderHook(({ open }) => useChatHistory(open, "/a"), { initialProps: { open: ["/a", "/b"] } });
    await settle();
    rerender({ open: ["/a"] });
    await settle();
    expect(Object.keys(result.current.byFolder)).toEqual(["/a"]);
  });
});

describe("useChatGroups", () => {
  /// Chat mode's folders and the ones every project shares are two sets: the
  /// hook reads the one asked for, and another one once the mode changes.
  test("reads the set asked for, and the other one when it changes", async () => {
    groups = { projects: [{ id: "g", name: "Ops", position: 1 }], chat: [{ id: "c", name: "Mine", position: 1 }] };
    const { result, rerender } = renderHook(({ list }) => useChatGroups(list), { initialProps: { list: "projects" as "projects" | "chat" } });
    await settle();
    expect(result.current.groups.map((g) => g.name)).toEqual(["Ops"]);
    rerender({ list: "chat" });
    expect(result.current.groups).toEqual([]);
    await settle();
    expect(result.current.groups.map((g) => g.name)).toEqual(["Mine"]);
  });

  test("a chat folder is made in the set on screen, and read again", async () => {
    const { result } = renderHook(() => useChatGroups("projects"));
    await settle();
    await act(async () => {
      await result.current.create("Later");
    });
    await settle();
    expect(asked).toContainEqual({ command: "chat_group_create", args: { list: "projects", name: "Later" } });
    expect(result.current.groups.map((g) => g.name)).toEqual(["Later"]);
  });
});
