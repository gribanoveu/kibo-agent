import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";

// The wrappers are the only place command names and payload shapes are
// written down, so what is worth checking here is that they are written
// correctly — and that one turn's listener never sees another turn's events.

type Call = { command: string; args: unknown };
const calls: Call[] = [];
let invokeResult: unknown = null;

type Listener = (event: { payload: unknown }) => void;
let listeners: { name: string; handler: Listener }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    return Promise.resolve(invokeResult);
  },
}));

mock.module("@tauri-apps/api/event", () => ({
  listen: (name: string, handler: Listener) => {
    listeners.push({ name, handler });
    return Promise.resolve(() => {
      listeners = listeners.filter((l) => l.handler !== handler);
    });
  },
}));

// The wrappers refuse to run outside the app; these tests are the app's side.
//
// `window` is shared with every other test file in the run, so this has to be
// put back: with it left behind, components in other files believe they are in
// the app and call an `invoke` that is mocked here — which is how this suite
// passed file by file and failed as a whole.
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const chat = await import("../lib/chat");
const savedChats = await import("../lib/savedChats");

afterEach(() => {
  calls.length = 0;
  listeners = [];
  invokeResult = null;
});

const emit = (payload: unknown) => {
  for (const listener of listeners) listener.handler({ payload });
};

describe("command wrappers", () => {
  test("a turn is started with the whole conversation", async () => {
    invokeResult = { status: "done", value: { text: "hi", truncated: false, todos: [] } };
    const messages = [{ role: "user" as const, content: "go" }];

    const outcome = await chat.startChat("turn-1", messages, [], "# Plan");

    expect(calls).toEqual([
      { command: "chat_start", args: { turnId: "turn-1", messages, todos: [], plan: "# Plan" } },
    ]);
    expect(outcome.status).toBe("done");
  });

  test("a checkpoint goes back untouched", async () => {
    const checkpoint = {
      history: [],
      round: 3,
      budgetUsed: 7,
      eventSeq: 42,
      calls: [],
      todos: [],
      reads: { seen: {} },
    };
    invokeResult = { status: "done", value: { text: "", truncated: false, todos: [] } };

    await chat.resumeChat("turn-1", checkpoint, [{ id: "w1", approved: true }], "# Plan");

    // Every field, including the ones this side never reads: dropping one is a
    // reset round ceiling or a write refused for never having read the file.
    expect(calls[0]?.args).toEqual({
      turnId: "turn-1",
      checkpoint,
      decisions: [{ id: "w1", approved: true }],
      plan: "# Plan",
    });
  });

  test("each command is named once, here", async () => {
    invokeResult = true;
    await chat.cancelChat();
    await chat.steer("use the helper");
    await chat.cancelSteer("note-1");
    await chat.alwaysAllow("writeFile");
    await chat.setUnattended(true);
    await chat.openWorkspace("/tmp/project");

    expect(calls.map((c) => c.command)).toEqual([
      "chat_cancel",
      "chat_steer",
      "chat_cancel_steer",
      "approval_always_allow",
      "approval_set_unattended",
      "workspace_open",
    ]);
    expect(calls[1]?.args).toEqual({ text: "use the helper", shown: null });
    expect(calls[5]?.args).toEqual({ path: "/tmp/project" });
  });
});

describe("turn events", () => {
  test("a listener hears its own turn", async () => {
    const heard: unknown[] = [];
    await chat.onTurnEvent("turn-1", (event) => heard.push(event));

    emit({ turnId: "turn-1", seq: 1, round: 1, type: "delta", payload: { delta: "hi" } });

    expect(heard).toHaveLength(1);
  });

  /// The channel is global. Without the filter two turns interleave into one
  /// message, character by character.
  test("and nothing from another turn", async () => {
    const heard: unknown[] = [];
    await chat.onTurnEvent("turn-1", (event) => heard.push(event));

    emit({ turnId: "turn-2", seq: 1, round: 1, type: "delta", payload: { delta: "not mine" } });

    expect(heard).toEqual([]);
  });

  test("unsubscribing stops the events", async () => {
    const heard: unknown[] = [];
    const off = await chat.onTurnEvent("turn-1", (event) => heard.push(event));

    off();
    emit({ turnId: "turn-1", seq: 1, round: 1, type: "delta", payload: { delta: "hi" } });

    expect(heard).toEqual([]);
  });
});

describe("compaction", () => {
  test("the whole conversation goes over, and the shorter one comes back", async () => {
    const messages = [{ role: "user" as const, content: "hi" }];
    invokeResult = { history: messages, folded: 3 };

    const shorter = await chat.compactHistory(messages, false, "compact-1");

    expect(calls).toEqual([
      { command: "chat_compact", args: { messages, force: false, turnId: "compact-1" } },
    ]);
    expect(shorter?.folded).toBe(3);
  });

  test("and asking for one outright says so", async () => {
    invokeResult = null;
    await chat.compactHistory([], true, "compact-2");
    expect(calls[0].args).toEqual({ messages: [], force: true, turnId: "compact-2" });
  });
});

describe("saved chats", () => {
  test("a chat is saved under its id, with both lists and the plan", async () => {
    invokeResult = { id: "c1", title: "fix the parser", updatedAt: 7 };
    const messages = [{ role: "user" as const, content: "fix the parser" }];
    const blocks = [{ kind: "user" as const, id: "user:0", text: "fix the parser" }];

    const summary = await savedChats.saveChat("c1", messages, blocks, [], null);

    expect(calls).toEqual([
      { command: "chat_save", args: { id: "c1", messages, blocks, todos: [], plan: null, branchedFrom: null } },
    ]);
    expect(summary.title).toBe("fix the parser");
  });

  test("a chat is loaded by id", async () => {
    invokeResult = { id: "c1", messages: [], blocks: [], todos: [] };
    await savedChats.loadChat("c1");
    expect(calls).toEqual([{ command: "chat_load", args: { id: "c1" } }]);
  });

  test("the list asks for nothing: the backend knows which folder is open", async () => {
    invokeResult = [];
    await savedChats.listChats();
    expect(calls).toEqual([{ command: "chat_list", args: undefined }]);
  });

  /// The sidebar is drawn in a plain browser too (`bun run dev`), where there
  /// is no backend to ask. An empty list is the honest answer; a thrown error
  /// would be an empty sidebar with a broken page behind it.
  test("and outside the app it is simply empty", async () => {
    const internals = (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    try {
      expect(await savedChats.listChats()).toEqual([]);
      expect(calls).toEqual([]);
    } finally {
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = internals;
    }
  });
});
