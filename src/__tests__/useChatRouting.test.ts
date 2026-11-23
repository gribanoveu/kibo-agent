import { afterAll, describe, expect, test, mock } from "bun:test";
import { act, renderHook } from "@testing-library/react";
import { COMMIT_REQUEST, PULL_REQUEST, REVIEW_REQUEST, SYNC_BASE } from "../lib/offer";
import { withLanguageReminder } from "../lib/settings";
import { STAY } from "../hooks/useSidebarActions";

// Module mocks are global to the run: keep every other export as it is, so a
// later file's import of these modules still finds what it needs. The spread
// snapshots the values — the namespace itself is live, and the mock patches
// its bindings in place.
const realSavedChats = { ...(await import("../lib/savedChats")) };
const realDialog = { ...(await import("../lib/dialog")) };
const moved = { path: "/repo-wt", fail: null as string | null };
const exported: [string, string][] = [];
const exportFail = { value: null as string | null };
const savePath = { value: "/tmp/chat.md" as string | null };
mock.module("../lib/savedChats", () => ({
  ...realSavedChats,
  moveChatToWorktree: async (_id: string) => {
    if (moved.fail) throw new Error(moved.fail);
    return moved.path;
  },
  exportChat: async (id: string, path: string) => {
    exported.push([id, path]);
    if (exportFail.value) throw new Error(exportFail.value);
  },
}));
mock.module("../lib/dialog", () => ({ ...realDialog, pickSavePath: async () => savePath.value }));

// File order is not alphabetical: give the real modules back, or a later
// file's own save/worktree tests would run against these doubles.
afterAll(() => {
  mock.module("../lib/savedChats", () => realSavedChats);
  mock.module("../lib/dialog", () => realDialog);
});

const { useChatRouting } = await import("../hooks/useChatRouting");

type Over = Partial<{
  agentMode: boolean;
  path: string | null;
  chatId: string | null;
  status: string;
  openAnswer: boolean;
  pickAnswer: string | null;
  resetAnswer: boolean;
  newChatAnswer: boolean;
  compactResult: true | string | null;
  modeFail: string | null;
  worktreeMain: string | null;
  closeAnswer: boolean;
  replyLanguage: "auto" | "english" | "russian";
  holdOpens: boolean;
  guardBlocks: boolean;
}>;

/** The stores as recording doubles; the guard lets everything through unless `guardBlocks`. */
function setup(over: Over = {}) {
  const calls: string[] = [];
  const said: string[] = [];
  const asked: string[] = [];
  const guarded: string[][] = [];
  const sent: [string, string | undefined, unknown[], boolean | undefined][] = [];
  const back: [string, unknown[]][] = [];
  let focused = 0;
  let left = 0;
  const opens: ((opened: boolean) => void)[] = [];
  const agent = {
    chatId: over.chatId === undefined ? "c1" : over.chatId,
    turn: { status: over.status ?? "idle", blocks: [] },
    open: async (id: string) => {
      calls.push(`agent.open:${id}`);
      return over.openAnswer ?? true;
    },
    reset: () => {
      calls.push("agent.reset");
      return over.resetAnswer ?? true;
    },
    send: (text: string, command?: string, images: unknown[] = [], implement?: boolean) => {
      sent.push([text, command, images, implement]);
    },
    compact: async (force: boolean) => {
      calls.push(`compact:${force}`);
      return over.compactResult === undefined ? true : over.compactResult;
    },
  };
  const plain = {
    open: async (id: string) => {
      calls.push(`plain.open:${id}`);
      return over.openAnswer ?? true;
    },
    newChat: () => {
      calls.push("plain.newChat");
      return over.newChatAnswer ?? true;
    },
  };
  const history = {
    chats: [{ id: "c1", title: "My chat", archived: false }],
    folderOf: (id: string) => (id === "c2" ? "/other" : "/repo"),
  };
  const workspace = {
    path: over.path === undefined ? "/repo" : over.path,
    open: (path: string) => {
      calls.push(`workspace.open:${path}`);
      // Held until the test lets it land, to pick another chat meanwhile.
      if (over.holdOpens) return new Promise<boolean>((resolve) => opens.push(resolve));
      return Promise.resolve(over.openAnswer ?? true);
    },
    pick: async () => {
      calls.push("workspace.pick");
      return over.pickAnswer ?? null;
    },
    close: (folder: string) => {
      calls.push(`workspace.close:${folder}`);
      return Promise.resolve(over.closeAnswer ?? true);
    },
  };
  const folderSwitch = {
    guard: (go: () => void, _onCancel?: () => void, closing: string[] = []) => {
      calls.push("guard");
      guarded.push(closing);
      // A guard that keeps its dialog open — a running turn there, say — runs nothing after it.
      if (over.guardBlocks) return Promise.resolve();
      return Promise.resolve(go());
    },
  };
  const conversation = {
    value: "agent" as const,
    pick: async (mode: string) => {
      calls.push(`pick:${mode}`);
      return over.modeFail ?? null;
    },
  };
  const view = renderHook(() =>
    useChatRouting({
      agentMode: over.agentMode ?? true,
      agent: agent as never,
      plain: plain as never,
      history: history as never,
      workspace: workspace as never,
      folderSwitch: folderSwitch as never,
      conversation,
      replyLanguage: over.replyLanguage ?? "auto",
      notify: (msg) => said.push(msg),
      focusComposer: () => focused++,
      leavePage: () => left++,
      giveBack: (text, images = []) => back.push([text, images]),
      worktreeMain: over.worktreeMain === undefined ? "/repo-main" : over.worktreeMain,
      openPlan: () => calls.push("openPlan"),
      askWorktreeRemoval: (path) => asked.push(path),
    }),
  );
  const landOpens = async (opened: boolean, count = opens.length) => {
    await act(async () => {
      for (const land of opens.splice(0, count)) land(opened);
    });
  };
  return { ...view, calls, said, sent, back, asked, guarded, focused: () => focused, left: () => left, landOpens };
}

describe("picking a chat", () => {
  test("a chat of this folder opens directly; a refusal says to stop the turn", async () => {
    const { result, calls, said, left } = setup();
    await act(async () => result.current.selectChat("c1"));
    expect(calls).toEqual(["agent.open:c1"]);
    expect(left()).toBe(1);
    expect(said).toEqual([]);
    const refused = setup({ openAnswer: false });
    await act(async () => refused.result.current.selectChat("c1"));
    expect(refused.said).toEqual([STAY]);
    const chat = setup({ agentMode: false });
    await act(async () => chat.result.current.selectChat("c1"));
    expect(chat.calls).toEqual(["plain.open:c1"]);
  });

  test("a chat of another folder waits for the folder, and a failed open drops only its own chat", async () => {
    const { result, calls, landOpens } = setup({ holdOpens: true });
    act(() => result.current.selectChat("c2"));
    expect(calls).toEqual(["guard", "workspace.open:/other"]);
    expect(result.current.pendingChat).toEqual({ id: "c2", path: "/other" });
    // Another chat of a third folder is picked while the first open is on its way.
    act(() => result.current.selectChat("c3", "/third"));
    expect(result.current.pendingChat).toEqual({ id: "c3", path: "/third" });
    // Only the first open lands, and fails: the chat picked since stays.
    await landOpens(false, 1);
    expect(result.current.pendingChat).toEqual({ id: "c3", path: "/third" });
    const alone = setup({ holdOpens: true });
    act(() => alone.result.current.selectChat("c2"));
    await alone.landOpens(false);
    expect(alone.result.current.pendingChat).toBeNull();
    // A chat of this folder picked meanwhile drops the one still on its way.
    const since = setup({ holdOpens: true });
    act(() => since.result.current.selectChat("c2"));
    await act(async () => since.result.current.selectChat("c1"));
    expect(since.result.current.pendingChat).toBeNull();
  });

  test("a new chat resets the mode's own thread, or says to stop the turn", () => {
    const agent = setup();
    act(() => agent.result.current.newChat());
    expect(agent.calls).toEqual(["agent.reset"]);
    expect(agent.focused()).toBe(1);
    const busy = setup({ resetAnswer: false });
    act(() => busy.result.current.newChat());
    expect(busy.said).toEqual([STAY]);
    expect(busy.focused()).toBe(0);
    const chat = setup({ agentMode: false, newChatAnswer: false });
    act(() => chat.result.current.newChat());
    expect(chat.said).toEqual([STAY]);
  });

  test("the section's + opens another folder through the guard; its own folder starts a chat", async () => {
    const { result, calls } = setup();
    await act(async () => result.current.newChatIn("/other"));
    expect(calls).toEqual(["guard", "workspace.open:/other"]);
    const own = setup();
    act(() => own.result.current.newChatIn("/repo"));
    expect(own.calls).toEqual(["agent.reset"]);
  });
});

describe("sending", () => {
  test("a message with no folder asks for one; a closed picker gives it back", async () => {
    const { result, back, sent } = setup({ path: null });
    await act(async () => result.current.send("fix it", undefined, [{ kind: "image" }]));
    expect(back).toEqual([["fix it", [{ kind: "image" }]]]);
    expect(sent).toEqual([]);
    const picked = setup({ path: null, pickAnswer: "/repo" });
    await act(async () => picked.result.current.send("fix it"));
    expect(picked.sent).toEqual([["fix it", undefined, [], undefined]]);
  });

  test("a command's prompt ends with the reply language; auto adds nothing", async () => {
    const { result, sent } = setup({ replyLanguage: "russian" });
    await act(async () => result.current.send("/init", "Write AGENTS.md."));
    expect(sent).toEqual([["/init", withLanguageReminder("Write AGENTS.md.", "russian"), [], undefined]]);
    const auto = setup();
    await act(async () => auto.result.current.send("/init", "Write AGENTS.md."));
    expect(auto.sent[0][1]).toBe("Write AGENTS.md.");
  });

  test("folding is refused while a turn runs; a failure says its reason, nothing to fold says so", async () => {
    const busy = setup({ status: "running" });
    await act(async () => busy.result.current.compactNow());
    expect(busy.said).toEqual(["Not while a turn is running"]);
    expect(busy.calls).toEqual([]);
    const failed = setup({ compactResult: "Error: provider said no" });
    await act(async () => failed.result.current.compactNow());
    expect(failed.said).toEqual(["Error: provider said no"]);
    const nothing = setup({ compactResult: null });
    await act(async () => nothing.result.current.compactNow());
    expect(nothing.said).toEqual(["Nothing worth folding away yet"]);
    const ok = setup();
    await act(async () => ok.result.current.compactNow());
    expect(ok.calls).toEqual(["compact:true"]);
    expect(ok.said).toEqual([]);
  });
});

describe("the offers after an answer", () => {
  test("review, commit, pull request and base sync send their own request", async () => {
    const { result, sent } = setup();
    await act(async () => result.current.pickOffer({ kind: "review", reason: "why" }));
    await act(async () => result.current.pickOffer({ kind: "commit", reason: "why" }));
    await act(async () => result.current.pickOffer({ kind: "pullRequest", reason: "why" }));
    await act(async () => result.current.pickOffer({ kind: "syncBase", reason: "why" }));
    expect(sent.map(([text]) => text)).toEqual([REVIEW_REQUEST, COMMIT_REQUEST, PULL_REQUEST, SYNC_BASE]);
  });

  test("a message offer sends exactly its text", async () => {
    const { result, sent } = setup();
    await act(async () => result.current.pickOffer({ kind: "message", reason: "why", text: "Start OF-1 and stop for review" }));
    expect(sent.map(([text]) => text)).toEqual(["Start OF-1 and stop for review"]);
  });

  test("a message offer without text sends nothing", async () => {
    const { result, sent } = setup();
    await act(async () => result.current.pickOffer({ kind: "message", reason: "why" }));
    expect(sent).toEqual([]);
  });

  test("the plan card opens the Plan tab and sends nothing; the tab's own button picks the writing mode first", async () => {
    const { result, calls, sent } = setup();
    await act(async () => result.current.pickOffer({ kind: "plan", reason: "why" }));
    expect(calls).toEqual(["openPlan"]);
    expect(sent).toEqual([]);
    await act(async () => result.current.implement());
    expect(calls).toEqual(["openPlan", "pick:agent"]);
    expect(sent).toEqual([["Implement the plan above. Work through the checklist in order.", undefined, [], true]]);
    const refused = setup({ modeFail: "not now" });
    await act(async () => refused.result.current.implement());
    expect(refused.sent).toEqual([]);
    expect(refused.said).toEqual(["not now"]);
  });

  test("worktree moves the chat and opens its folder beside this one; a failure drops the pending chat", async () => {
    const { result } = setup();
    await act(async () => result.current.pickOffer({ kind: "worktree", reason: "why" }));
    expect(result.current.pendingChat).toEqual({ id: "c1", path: "/repo-wt" });
    const refused = setup({ openAnswer: false });
    await act(async () => refused.result.current.pickOffer({ kind: "worktree", reason: "why" }));
    expect(refused.result.current.pendingChat).toBeNull();
    moved.fail = "boom";
    const failing = setup();
    await act(async () => failing.result.current.pickOffer({ kind: "worktree", reason: "why" }));
    expect(failing.result.current.pendingChat).toBeNull();
    expect(failing.said).toEqual(["Error: boom"]);
    moved.fail = null;
    const noChat = setup({ chatId: null });
    await act(async () => noChat.result.current.pickOffer({ kind: "worktree", reason: "why" }));
    expect(noChat.calls).toEqual([]);
  });

  test("cleanup asks the folder guard first, then opens the main checkout, closes the worktree, and asks the removal dialog", async () => {
    const { result, calls, asked, guarded } = setup();
    await act(async () => result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    // What closes is what the guard reads processes and turns for — an open
    // folder is never removable, so the main checkout comes first.
    expect(guarded).toEqual([["/repo"]]);
    expect(calls).toEqual(["guard", "workspace.open:/repo-main", "workspace.close:/repo"]);
    expect(asked).toEqual(["/repo"]);
    // A guard held open by a running turn there runs nothing after it.
    const blocked = setup({ guardBlocks: true });
    await act(async () => blocked.result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    expect(blocked.calls).toEqual(["guard"]);
    expect(blocked.asked).toEqual([]);
    // A failed open or close stops before the dialog: an open folder is never removable.
    const refused = setup({ openAnswer: false });
    await act(async () => refused.result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    expect(refused.calls).toEqual(["guard", "workspace.open:/repo-main"]);
    expect(refused.asked).toEqual([]);
    const closeFail = setup({ closeAnswer: false });
    await act(async () => closeFail.result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    expect(closeFail.calls).toEqual(["guard", "workspace.open:/repo-main", "workspace.close:/repo"]);
    expect(closeFail.asked).toEqual([]);
    const noMain = setup({ worktreeMain: null });
    await act(async () => noMain.result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    expect(noMain.calls).toEqual([]);
    expect(noMain.asked).toEqual([]);
    // A press that can do nothing says why, instead of nothing happening.
    expect(noMain.said).toEqual(["The main checkout of this worktree is unknown — open it in the folder list and remove the worktree there"]);
    const noFolder = setup({ path: null });
    await act(async () => noFolder.result.current.pickOffer({ kind: "cleanup", reason: "why" }));
    expect(noFolder.calls).toEqual([]);
    expect(noFolder.asked).toEqual([]);
    expect(noFolder.said).toEqual(["The main checkout of this worktree is unknown — open it in the folder list and remove the worktree there"]);
  });
});

describe("exporting the conversation", () => {
  test("nothing saved says so; a closed picker writes nothing; a write is named, a failure said", async () => {
    const none = setup({ chatId: "ghost" });
    await act(async () => none.result.current.exportOpenChat());
    expect(none.said).toEqual(["Nothing saved to export yet"]);
    savePath.value = null;
    const cancelled = setup();
    await act(async () => cancelled.result.current.exportOpenChat());
    expect(exported).toEqual([]);
    savePath.value = "/tmp/chat.md";
    const ok = setup();
    await act(async () => ok.result.current.exportOpenChat());
    expect(exported).toEqual([["c1", "/tmp/chat.md"]]);
    expect(ok.said).toEqual(["Exported to chat.md"]);
    exportFail.value = "disk full";
    const failing = setup();
    await act(async () => failing.result.current.exportOpenChat());
    expect(failing.said).toEqual(["Error: disk full"]);
    exportFail.value = null;
  });
});