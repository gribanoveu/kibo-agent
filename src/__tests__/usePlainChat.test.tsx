import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, renderHook, waitFor } from "@testing-library/react";
import { acceptEvent, appendUserMessage, restoredTurn } from "../lib/chatTurnReducer";

// Chat mode's turn is the agent's loop in a role: the window sends, answers the
// card, and saves the agent's blocks. And the chats saved before that — as
// bubbles, or only as what the model was sent — still open.

type Call = { command: string; args: Record<string, unknown> };
const calls: Call[] = [];
const results: Record<string, unknown> = {};

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: Record<string, unknown>) => {
    calls.push({ command, args });
    const answer = results[command];
    const result = typeof answer === "function" ? answer(args) : answer;
    return result instanceof Error ? Promise.reject(result.message) : Promise.resolve(result ?? null);
  },
}));

mock.module("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { usePlainChat, chatBlocks } = await import("../hooks/usePlainChat");

afterEach(() => {
  calls.length = 0;
  for (const key of Object.keys(results)) delete results[key];
  localStorage.clear();
});

const called = (command: string) => calls.filter((call) => call.command === command);

describe("a chat saved before the agent's blocks", () => {
  test("its bubbles become the blocks the reducer would have made", () => {
    const blocks = chatBlocks(
      [
        { role: "user", text: "hi" },
        { role: "assistant", text: "hello", reasoning: "greet back" },
      ],
      [],
    );
    expect(blocks).toEqual([
      { kind: "user", id: "user:0", text: "hi" },
      { kind: "reasoning", id: "turn:1:round:1:reasoning", round: 1, text: "greet back" },
      { kind: "message", id: "turn:1:round:1:message", round: 1, text: "hello" },
    ]);
  });

  test("with no bubbles, it is drawn from what the model was sent", () => {
    const blocks = chatBlocks(null, [
      { role: "user", content: "hi" },
      { role: "assistant", content: "hello" },
    ]);
    expect(blocks.map((b) => b.kind)).toEqual(["user", "message"]);
  });

  test("blocks are taken as they are", () => {
    const saved = [{ kind: "notice", id: "notice:0", text: "said" }];
    expect(chatBlocks(saved, [])).toBe(saved);
  });

  /// The point of reusing the reducer's ids: the next question's answer is a
  /// new block, not appended to the old answer halfway up the chat.
  test("a turn after it answers in a block of its own", () => {
    const old = restoredTurn(chatBlocks([{ role: "user", text: "hi" }, { role: "assistant", text: "hello" }], []));
    const asked = appendUserMessage(old, "and?");
    const answered = acceptEvent(asked, { turnId: "t", seq: 1, round: 1, type: "delta", payload: { delta: "more" } } as never);
    const texts = answered.blocks.filter((b) => b.kind === "message").map((b) => (b as { text: string }).text);
    expect(texts).toEqual(["hello", "more"]);
    expect(new Set(answered.blocks.map((b) => b.id)).size).toBe(answered.blocks.length);
  });
});

describe("a turn in a role", () => {
  const card = {
    status: "pendingApproval",
    value: {
      history: [{ role: "user", content: "delete it" }],
      round: 1,
      budgetUsed: 1,
      eventSeq: 3,
      calls: [{ id: "c1", name: "deleteFile", arguments: "{}", requiresConfirmation: true }],
      todos: [],
      reads: {},
    },
  };

  test("pauses on the card, and the answer resumes it with the checkpoint", async () => {
    results.plain_chat_send = card;
    results.plain_chat_resume = {
      status: "done",
      value: { text: "kept", history: [{ role: "user", content: "delete it" }, { role: "assistant", content: "kept" }], todos: [] },
    };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("delete it");
    });
    expect(result.current.turn.status).toBe("awaitingApproval");
    expect(result.current.busy).toBe(true);

    const decision = [{ id: "c1", approved: false, reason: "keep it" }];
    await act(async () => {
      await result.current.decide(decision, ["deleteFile"]);
    });

    expect(called("plain_chat_always_allow")[0]?.args).toEqual({ tool: "deleteFile" });
    const resumed = called("plain_chat_resume")[0]?.args;
    expect(resumed?.checkpoint).toEqual(card.value);
    expect(resumed?.decisions).toEqual(decision);
    expect(resumed?.role).toBe("assistant");
    // The chat it is a turn of: a role with a folder finds its folder by it.
    expect(resumed?.chatId).toBe(called("plain_chat_save")[0]?.args.id);
    expect(called("plain_chat_send")[0]?.args.chatId).toBe(called("plain_chat_save")[0]?.args.id);
    // Saved at rest with what the model read, and the blocks as drawn.
    await waitFor(() => expect(called("plain_chat_save").length).toBe(2));
    const last = called("plain_chat_save")[1].args;
    expect(last.messages).toEqual([{ role: "user", content: "delete it" }, { role: "assistant", content: "kept" }]);
    expect((last.blocks as { kind: string }[]).some((b) => b.kind === "approval")).toBe(false);
  });

  test("while it waits, the chat is not left: another, or a new one, is refused", async () => {
    results.plain_chat_send = card;
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("delete it");
    });
    let opened = true;
    await act(async () => {
      opened = await result.current.open("other");
    });
    expect(opened).toBe(false);
    expect(result.current.newChat()).toBe(false);
    expect(called("chat_load")).toEqual([]);
  });

  test("a turn that fails says so in the transcript", async () => {
    results.plain_chat_send = new Error("no provider");
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("hi");
    });
    const notice = result.current.turn.blocks.find((b) => b.kind === "notice") as { text: string } | undefined;
    expect(notice?.text).toBe("The turn failed: no provider");
    expect(result.current.busy).toBe(false);
  });
});

describe("room in the window", () => {
  const done = { status: "done", value: { text: "ok", history: [], todos: [] } };

  /// Room is the turn's to make, round by round: nothing is folded on the way.
  test("a message goes to the turn without a pass before it", async () => {
    results.plain_chat_send = done;
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("hi");
    });
    expect(called("plain_chat_compact")).toEqual([]);
    expect(called("plain_chat_send")[0]?.args.messages).toEqual([{ role: "user", content: "hi" }]);
  });

  test("pictures go with the message, and alone are one", async () => {
    results.plain_chat_send = done;
    const picture = { mediaType: "image/png" as const, data: "PNG", width: 2, height: 2 };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("", [{ image: picture, name: "p.png" }]);
    });
    expect(called("plain_chat_send")[0]?.args.messages).toEqual([{ role: "user", content: "", images: [picture] }]);
    expect(result.current.turn.blocks[0]).toMatchObject({ kind: "user", text: "", images: [picture] });
    expect(called("prototype_asset_add"), "only a Prototype chat keeps them").toEqual([]);
  });

  /// A Prototype chat puts its pictures into the folder first, under their
  /// files' names, and tells the model where; a model that cannot see them
  /// is told only that.
  test("in a Prototype chat pictures go into assets/ and the message says where", async () => {
    results.plain_chat_send = done;
    results.prototype_asset_add = (args: { name: string }) => ({ name: args.name.replace("Our Logo.png", "our-logo.png"), bytes: 3, width: 2, height: 2 });
    const picture = { mediaType: "image/png" as const, data: "PNG", width: 2, height: 2 };
    for (const sees of [true, false]) {
      calls.length = 0;
      const { result } = renderHook(() => usePlainChat());
      act(() => void result.current.setRole("prototype"));
      await act(async () => {
        await result.current.send("put it in the header", [{ image: picture, name: "Our Logo.png" }], null, sees);
      });
      const added = called("prototype_asset_add");
      expect(added.map((c) => c.args)).toEqual([{ chatId: result.current.chatId, name: "Our Logo.png", image: picture }]);
      const [message] = called("plain_chat_send")[0]?.args.messages as { content: string; images?: unknown }[];
      expect(message.content).toBe("put it in the header\n\n[Attached picture, now in your folder: `assets/our-logo.png` (2×2).]");
      expect(message.images).toEqual(sees ? [picture] : undefined);
      expect(result.current.turn.blocks[0]).toMatchObject({ kind: "user", text: "put it in the header", images: [picture] });
    }
  });

  test("a picture the folder refuses stops the message, saying why", async () => {
    results.prototype_asset_add = new Error("that is not a PNG or JPEG picture");
    const picture = { mediaType: "image/png" as const, data: "PNG", width: 2, height: 2 };
    const { result } = renderHook(() => usePlainChat());
    act(() => void result.current.setRole("prototype"));
    await act(async () => {
      await result.current.send("hi", [{ image: picture, name: "x.png" }]);
    });
    expect(called("plain_chat_send")).toEqual([]);
    expect(result.current.error).toContain("not a PNG or JPEG");
    expect(result.current.turn.blocks).toEqual([]);
  });

  test("Compact now asks outright, and says when nothing was folded", async () => {
    results.plain_chat_send = done;
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("hi");
    });
    let folded = true;
    await act(async () => {
      folded = await result.current.compact();
    });
    expect(folded).toBe(false);
    expect(called("plain_chat_compact").map((c) => c.args.force)).toEqual([true]);
    // Its summary is the chat's spending, as the turn's is.
    expect(called("plain_chat_compact")[0].args.chatId).toBe(called("plain_chat_send")[0].args.chatId);
  });

  /// On screen, the chat's total is read; and again once Compact now is back,
  /// which no turn event reports.
  test("the session's total is read while the chat is shown", async () => {
    results.plain_chat_send = done;
    const id = () => called("plain_chat_send")[0]?.args.chatId;
    results.daily_metrics = () => [{ day: "2026-10-26", metric: "chatTokens", key: id(), value: 70 }];
    const { result } = renderHook(() => usePlainChat(null, undefined, true));
    await act(async () => {
      await result.current.send("hi");
    });
    await waitFor(() => expect(result.current.spent).toBe(70));
    const before = called("daily_metrics").length;
    await act(async () => {
      await result.current.compact();
    });
    await waitFor(() => expect(called("daily_metrics").length).toBeGreaterThan(before));
  });

  test("hidden, it is not read", async () => {
    results.plain_chat_send = done;
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("hi");
    });
    expect(called("daily_metrics")).toEqual([]);
    expect(result.current.spent).toBeNull();
  });
});

describe("a chat's role", () => {
  test("is picked before the first message and fixed after it", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const { result } = renderHook(() => usePlainChat());
    act(() => {
      result.current.setRole("kubernetes");
    });
    expect(result.current.role).toBe("kubernetes");
    await act(async () => {
      await result.current.send("hi");
    });
    act(() => {
      result.current.setRole("assistant");
    });
    expect(result.current.role).toBe("kubernetes");
  });
});

describe("the cluster a chat works with", () => {
  test("a new chat starts at the kubeconfig picked last, and is pinned to it from its first message", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const { result } = renderHook(() => usePlainChat("prod"));
    expect(result.current.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
    await act(async () => {
      await result.current.send("pods?");
    });
    const sent = called("plain_chat_send")[0]?.args;
    expect(sent?.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
    expect(called("plain_chat_save")[0]?.args.kube).toEqual({ kubeconfig: "prod", context: null, namespace: null });
  });

  test("a pin changed in a saved chat is kept with it at once", async () => {
    results.plain_chat_send = { status: "done", value: { text: "ok", history: [], todos: [] } };
    const { result } = renderHook(() => usePlainChat("prod"));
    await act(async () => {
      await result.current.send("pods?");
    });
    await waitFor(() => expect(result.current.busy).toBe(false));
    const saves = called("plain_chat_save").length;
    act(() => result.current.setPin({ kubeconfig: "prod", context: "eks", namespace: "orders" }));
    const last = called("plain_chat_save").slice(saves);
    expect(last.map((call) => call.args.kube)).toEqual([{ kubeconfig: "prod", context: "eks", namespace: "orders" }]);
    expect(result.current.kube?.namespace).toBe("orders");
  });

  test("an opened chat works where it was pinned, whatever was picked last", async () => {
    const pin = { kubeconfig: "stg", context: "kind", namespace: "orders" };
    results.chat_load = { id: "c", messages: [], blocks: [], role: "kubernetes", kube: pin };
    const { result } = renderHook(() => usePlainChat("prod"));
    await act(async () => {
      await result.current.open("c");
    });
    expect(result.current.kube).toEqual(pin);
    expect(result.current.role).toBe("kubernetes");
  });

  /// "Changes" is a brake released for the chat as it is open: a chat saved
  /// with it on opens reading only, and so does a new chat started from it.
  test("a chat saved with Changes on opens read-only", async () => {
    const pin = { kubeconfig: "stg", context: "kind", namespace: "orders" };
    results.chat_load = { id: "c", messages: [], blocks: [], role: "kubernetes", kube: { ...pin, writes: true } };
    const { result } = renderHook(() => usePlainChat("prod"));
    await act(async () => {
      await result.current.open("c");
    });
    expect(result.current.kube).toEqual(pin);
    act(() => result.current.setPin({ ...pin, writes: true }));
    expect(result.current.kube?.writes).toBe(true);
    act(() => void result.current.newChat());
    expect(result.current.kube).toEqual(pin);
  });

  test("a card's preview is asked of the chat's own cluster", async () => {
    results.plain_chat_preview = [{ kind: "nothing" }];
    const { result } = renderHook(() => usePlainChat("prod"));
    const calls = [{ id: "c1", name: "kubeScale", arguments: "{}", requiresConfirmation: true }];
    await act(async () => {
      await result.current.preview(calls);
    });
    expect(called("plain_chat_preview")[0]?.args).toEqual({ kube: { kubeconfig: "prod", context: null, namespace: null }, calls });
  });
});

describe("a Prototype chat's folder put back", () => {
  const done = { status: "done", value: { text: "ok", history: [], todos: [] } };
  const lastSent = () => (called("plain_chat_send").at(-1)?.args.messages as { content: string }[]).at(-1)?.content;

  test("a restore is said in the transcript, saved, and told to the model once, with the next message", async () => {
    results.plain_chat_send = done;
    results.prototype_restore = { from: 1, version: 3, before: 2 };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("a page");
    });
    await waitFor(() => expect(result.current.busy).toBe(false));

    let restored = false;
    await act(async () => {
      restored = await result.current.restore(1);
    });
    expect(restored).toBe(true);
    expect(called("prototype_restore")[0]?.args).toMatchObject({ version: 1, chatId: result.current.chatId });
    expect(result.current.turn.blocks.at(-1)).toMatchObject({ kind: "restored", from: 1, version: 3, before: 2 });
    expect((called("plain_chat_save").at(-1)?.args.blocks as { kind: string }[]).at(-1)?.kind).toBe("restored");

    await act(async () => {
      await result.current.send("make the header blue");
    });
    expect(lastSent()).toStartWith("make the header blue\n\n[The user restored the chat's folder to version 1 (now kept as version 3)");
    const bubbles = result.current.turn.blocks.filter((b) => b.kind === "user") as { text: string }[];
    expect(bubbles.at(-1)?.text).toBe("make the header blue");

    await waitFor(() => expect(result.current.busy).toBe(false));
    await act(async () => {
      await result.current.send("and the footer");
    });
    expect(lastSent()).toBe("and the footer");
  });

  test("an element pointed at goes to the model with the message, and shows on the bubble by its selector", async () => {
    results.plain_chat_send = done;
    results.prototype_restore = { from: 1, version: 3, before: null };
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("a page");
    });
    await waitFor(() => expect(result.current.busy).toBe(false));
    await act(async () => {
      await result.current.restore(1);
    });
    const pointed = { page: "index.html", selector: "main > h1", html: "<h1>Coffee</h1>", text: "Coffee" };
    await act(async () => {
      await result.current.send("make it bigger", [], pointed);
    });
    // What was typed, the element, then the restore — each said once.
    const sent = lastSent();
    expect(sent).toStartWith("make it bigger\n\n[The user pointed at an element on the page index.html: `main > h1`.");
    expect(sent).toContain("<h1>Coffee</h1>");
    expect(sent.indexOf("pointed at")).toBeLessThan(sent.indexOf("restored the chat's folder"));
    expect(result.current.turn.blocks.at(-1)).toMatchObject({ kind: "user", text: "make it bigger", element: "main > h1" });

    await waitFor(() => expect(result.current.busy).toBe(false));
    await act(async () => {
      await result.current.send("and the footer");
    });
    expect(lastSent()).toBe("and the footer");
    expect(result.current.turn.blocks.at(-1)).not.toHaveProperty("element");
  });

  // The turn may be writing those very files: the folder is not put back under it.
  test("nothing is restored while a turn is under way", async () => {
    results.plain_chat_send = () => new Promise(() => {});
    results.prototype_restore = { from: 1, version: 3, before: null };
    const { result } = renderHook(() => usePlainChat());
    act(() => {
      void result.current.send("a page");
    });
    await waitFor(() => expect(result.current.busy).toBe(true));
    let restored = true;
    await act(async () => {
      restored = await result.current.restore(1);
    });
    expect(restored).toBe(false);
    expect(called("prototype_restore")).toEqual([]);
  });

  test("a restore that failed says why and adds nothing", async () => {
    results.plain_chat_send = done;
    results.prototype_restore = new Error("there is no version 9 of this chat's folder");
    const { result } = renderHook(() => usePlainChat());
    await act(async () => {
      await result.current.send("a page");
    });
    await waitFor(() => expect(result.current.busy).toBe(false));
    const before = result.current.turn.blocks.length;
    let restored = true;
    await act(async () => {
      restored = await result.current.restore(9);
    });
    expect(restored).toBe(false);
    expect(result.current.turn.blocks.length).toBe(before);
    expect(result.current.error).toContain("no version 9");
  });
});

describe("rewinding a chat", () => {
  // A saved chat in which the model wrote a page: its call recorded the change.
  const change = { path: "index.html", before: { kind: "absent" }, after: { kind: "stored", hash: "h1" } };

  const opened = async (role: "prototype" | "assistant", shown = false) => {
    results.chat_load = {
      id: "saved",
      role,
      kube: null,
      messages: [
        { role: "user", content: "a page" },
        { role: "assistant", content: "made it" },
      ],
      blocks: [
        { kind: "user", id: "user:0", text: "a page" },
        { kind: "tool", id: "c1", round: 1, name: "writeFile", arguments: "{}", status: "done", output: "", changes: [change] },
        { kind: "message", id: "m1", round: 1, text: "made it" },
      ],
    };
    const { result } = renderHook(() => usePlainChat(null, undefined, shown));
    await act(async () => {
      await result.current.open("saved");
    });
    await waitFor(() => expect(result.current.chatId).toBe("saved"));
    return result;
  };

  /// The summary is the chat's spending, and no turn event says so: the
  /// total is read again once it is back.
  test("a rewind with a summary counts under the chat and reads the total again", async () => {
    const result = await opened("assistant", true);
    const bubble = result.current.turn.blocks.find((b) => b.kind === "user")!;
    results.chat_branch_summary = { role: "user", content: "[Summary] tried a page" };
    results.rewind_apply = [];
    await waitFor(() => expect(called("daily_metrics").length).toBeGreaterThan(0));
    const before = called("daily_metrics").length;
    await act(async () => {
      await result.current.rewind(bubble.id, true);
    });
    expect(called("chat_branch_summary")[0]?.args.chatId).toBe("saved");
    await waitFor(() => expect(called("daily_metrics").length).toBeGreaterThan(before));
  });

  test("a Prototype chat puts its own folder back, cuts the chat before the message and hands it back", async () => {
    const result = await opened("prototype");
    const bubble = result.current.turn.blocks.find((b) => b.kind === "user")!;
    expect(result.current.branchable?.has(bubble.id)).toBe(true);

    results.rewind_preview = [{ path: "index.html", action: "created", expected: change.after, skip: null }];
    const preview = await result.current.previewRewind(bubble.id);
    expect(preview?.files).toHaveLength(1);
    expect(called("rewind_preview")[0]?.args).toEqual({ changes: [change], chatId: result.current.chatId });

    results.rewind_apply = [{ path: "index.html", action: "created", expected: change.after, skip: null }];
    await act(async () => {
      await result.current.rewind(bubble.id);
    });
    expect(called("rewind_apply")[0]?.args).toEqual({ changes: [change], chatId: result.current.chatId });
    expect(result.current.turn.blocks.map((b) => b.kind)).toEqual(["notice"]);
    expect((result.current.turn.blocks[0] as { text: string }).text).toBe("Rewound to before this message — 1 file put back");
    expect(result.current.draft?.text).toBe("a page");
    expect(called("plain_chat_save").at(-1)?.args.messages).toEqual([]);
  });

  /// Without a chat named, the backend rewinds the agent's open folder — the
  /// user's project. A chat in Chat mode names itself whatever its role, so
  /// recorded changes can only land in its own folder, where a role without
  /// one finds nothing to put back.
  test("a chat in another role still names its own folder, never the agent's", async () => {
    const result = await opened("assistant");
    const bubble = result.current.turn.blocks.find((b) => b.kind === "user")!;
    results.rewind_preview = [];
    await result.current.previewRewind(bubble.id);
    expect(called("rewind_preview")[0]?.args.chatId).toBe("saved");
    results.rewind_apply = [];
    await act(async () => {
      await result.current.rewind(bubble.id);
    });
    expect(called("rewind_apply")[0]?.args.chatId).toBe("saved");
  });

  test("nothing is rewound while a turn is under way, and files that could not be reached leave the chat as it was", async () => {
    const result = await opened("prototype");
    const bubble = result.current.turn.blocks.find((b) => b.kind === "user")!;
    results.rewind_apply = new Error("the folder is gone");
    await act(async () => {
      expect(await result.current.rewind(bubble.id)).toBeNull();
    });
    expect(result.current.turn.blocks.some((b) => b.kind === "user")).toBe(true);
    expect(result.current.error).toContain("the folder is gone");

    // A turn under way: nothing reaches the folder, though the files could be put back now.
    results.rewind_apply = [];
    const applied = called("rewind_apply").length;
    results.plain_chat_send = () => new Promise(() => {});
    act(() => {
      void result.current.send("more");
    });
    await waitFor(() => expect(result.current.busy).toBe(true));
    expect(result.current.branchable).toBeNull();
    await act(async () => {
      expect(await result.current.rewind(bubble.id)).toBeNull();
    });
    expect(called("rewind_apply").length).toBe(applied);
  });
});

describe("deleting a chat", () => {
  test("a delete that failed rejects with why, and the list is read again either way", async () => {
    const { result } = renderHook(() => usePlainChat());
    results.chat_delete = new Error("database is locked");
    let failed: unknown = null;
    await act(async () => {
      await result.current.remove("gone").catch((e) => (failed = e));
    });
    expect(failed).toBe("database is locked");
    expect(called("plain_chat_list").length).toBeGreaterThan(0);

    delete results.chat_delete;
    let removed: boolean | undefined;
    await act(async () => {
      removed = await result.current.remove("gone");
    });
    expect(removed).toBe(true);
  });
});
