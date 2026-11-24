import { describe, expect, test } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { STAY, useSidebarActions } from "../hooks/useSidebarActions";

/** The stores as recording doubles: every call in order, every refusal said. */
function setup(agentMode: boolean, opts: { discard?: boolean; removed?: boolean; fail?: string } = {}) {
  const calls: string[] = [];
  const said: string[] = [];
  const drafts: [string, string][] = [];
  const boom = () => {
    if (opts.fail) throw new Error(opts.fail);
  };
  const agent = {
    discard: async (id: string) => {
      calls.push(`discard:${id}`);
      boom();
      return opts.discard ?? true;
    },
  };
  const plain = {
    archive: async (id: string, archived: boolean) => {
      calls.push(`plain.archive:${id}:${archived}`);
    },
    pin: async (id: string, pinned: boolean) => {
      calls.push(`plain.pin:${id}:${pinned}`);
    },
    move: async (id: string, group: string | null) => {
      calls.push(`plain.move:${id}:${group}`);
    },
    remove: async (id: string) => {
      calls.push(`plain.remove:${id}`);
      boom();
      return opts.removed ?? true;
    },
    refresh: () => {
      calls.push("plain.refresh");
      return Promise.resolve();
    },
  };
  const history = {
    archive: async (id: string, archived: boolean) => {
      calls.push(`history.archive:${id}:${archived}`);
      boom();
    },
    pin: async (id: string, pinned: boolean) => {
      calls.push(`history.pin:${id}:${pinned}`);
      boom();
    },
    move: async (id: string, group: string | null) => {
      calls.push(`history.move:${id}:${group}`);
      boom();
    },
    remove: async (id: string) => {
      calls.push(`history.remove:${id}`);
      boom();
    },
    refresh: () => {
      calls.push("history.refresh");
    },
  };
  const groups = {
    create: async (name: string) => {
      calls.push(`create:${name}`);
      boom();
      return { id: "g1", name };
    },
    rename: async (id: string, name: string) => {
      calls.push(`rename:${id}:${name}`);
      boom();
    },
    remove: async (id: string) => {
      calls.push(`group.remove:${id}`);
      boom();
    },
  };
  const view = renderHook(() =>
    useSidebarActions({
      agentMode,
      agent,
      plain,
      history,
      groups,
      notify: (msg) => said.push(msg),
      dropDraft: (mode, id) => drafts.push([mode, id]),
    }),
  );
  return { ...view, calls, said, drafts };
}

describe("the sidebar's actions", () => {
  test("Agent's row actions go to the history store; a refusal is said, not thrown", async () => {
    const { result, calls, said } = setup(true);
    await act(async () => {
      result.current.onArchiveChat("c1", true);
      result.current.onPinChat("c1", true);
      result.current.onMoveChat("c1", "g1");
    });
    expect(calls).toEqual(["history.archive:c1:true", "history.pin:c1:true", "history.move:c1:g1"]);
    expect(said).toEqual([]);
    const failing = setup(true, { fail: "boom" });
    await act(async () => failing.result.current.onArchiveChat("c1", true));
    expect(failing.said).toEqual(["Error: boom"]);
  });

  test("Chat's row actions go to its own conversations", async () => {
    const { result, calls } = setup(false);
    await act(async () => {
      result.current.onArchiveChat("c1", true);
      result.current.onPinChat("c1", false);
      result.current.onMoveChat("c1", null);
    });
    expect(calls).toEqual(["plain.archive:c1:true", "plain.pin:c1:false", "plain.move:c1:null"]);
  });

  test("deleting an Agent chat discards its turn first, then removes the row and its draft", async () => {
    const { result, calls, drafts, said } = setup(true);
    await act(async () => result.current.onDeleteChat("c1"));
    expect(calls).toEqual(["discard:c1", "history.remove:c1"]);
    expect(drafts).toEqual([["agent", "c1"]]);
    expect(said).toEqual([]);
  });

  test("a running turn refuses the delete; the row and its draft stay", async () => {
    const { result, calls, drafts, said } = setup(true, { discard: false });
    await act(async () => result.current.onDeleteChat("c1"));
    expect(calls).toEqual(["discard:c1"]);
    expect(drafts).toEqual([]);
    expect(said).toEqual(["Stop the running turn before deleting this chat"]);
  });

  test("a failed write behind the discard is said, not swallowed", async () => {
    const { result, calls, drafts, said } = setup(true, { fail: "boom" });
    await act(async () => result.current.onDeleteChat("c1"));
    expect(calls).toEqual(["discard:c1"]);
    expect(drafts).toEqual([]);
    expect(said).toEqual(["Error: boom"]);
  });

  test("deleting a Chat chat takes its draft; a refusal says to stop the turn, an error is said", async () => {
    const gone = setup(false);
    await act(async () => gone.result.current.onDeleteChat("c1"));
    expect(gone.calls).toEqual(["plain.remove:c1"]);
    expect(gone.drafts).toEqual([["chat", "c1"]]);
    const staying = setup(false, { removed: false });
    await act(async () => staying.result.current.onDeleteChat("c1"));
    expect(staying.drafts).toEqual([]);
    expect(staying.said).toEqual([STAY]);
    const failing = setup(false, { fail: "boom" });
    await act(async () => failing.result.current.onDeleteChat("c1"));
    expect(failing.said).toEqual(["Error: boom"]);
  });

  test("a group is created and renamed with the refusal coming back, not thrown", async () => {
    const { result } = setup(true);
    expect(await result.current.onCreateGroup("Work")).toEqual({ id: "g1" });
    expect(await result.current.onRenameGroup("g1", "Play")).toEqual({});
    const failing = setup(true, { fail: "boom" });
    expect(await failing.result.current.onCreateGroup("Work")).toEqual({ error: "Error: boom" });
    expect(await failing.result.current.onRenameGroup("g1", "Play")).toEqual({ error: "Error: boom" });
  });

  test("deleting a group re-reads the mode's own list; a refusal is said", async () => {
    const agent = setup(true);
    await act(async () => agent.result.current.onDeleteGroup("g1"));
    expect(agent.calls).toEqual(["group.remove:g1", "history.refresh"]);
    const chat = setup(false);
    await act(async () => chat.result.current.onDeleteGroup("g1"));
    expect(chat.calls).toEqual(["group.remove:g1", "plain.refresh"]);
    const failing = setup(true, { fail: "boom" });
    await act(async () => failing.result.current.onDeleteGroup("g1"));
    expect(failing.said).toEqual(["Error: boom"]);
  });
});