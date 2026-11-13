import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

// Both message boxes fit their text by measuring it (scrollHeight), and the
// measurement only followed the text: IDE layout moves the column's width, a
// line wraps or unwraps, and the paid-for height stayed until the next
// keystroke. The width itself re-measures now — these tests drive the
// composer's ResizeObserver by hand, happy-dom doing no layout.

const calls: { command: string; args?: unknown }[] = [];
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: unknown) => {
    calls.push({ command, args });
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));
// The balance chip beside the model listens for features being switched.
mock.module("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { Composer } = await import("../components/Composer");
const { PlainChat } = await import("../components/PlainChat");

// What observes what, to fire a width change by hand.
let instances: { callback: ResizeObserverCallback; el: Element }[] = [];
class FakeObserver {
  constructor(private readonly callback: ResizeObserverCallback) {}
  observe(el: Element) {
    instances.push({ callback: this.callback, el });
  }
  disconnect() {}
}
const RealObserver = globalThis.ResizeObserver;
beforeEach(() => {
  instances = [];
  globalThis.ResizeObserver = FakeObserver as unknown as typeof ResizeObserver;
});
afterEach(() => {
  globalThis.ResizeObserver = RealObserver;
  calls.length = 0;
  localStorage.clear();
});

/** The boxes' own observers: the other components' are not part of this. */
const widthChange = () =>
  instances
    .filter((instance) => instance.el.tagName === "TEXTAREA")
    .forEach((instance) => instance.callback([], {} as ResizeObserver));
const metrics = (el: HTMLElement, values: Record<string, number>) => {
  for (const [key, value] of Object.entries(values)) Object.defineProperty(el, key, { configurable: true, value });
};

const agentBox = () => (
  <Composer
    draftKey="width-agent"
    onSend={() => {}}
    onStop={() => {}}
    running={false}
    conversation="agent"
    onConversation={() => {}}
    unattended={false}
    onUnattended={() => {}}
    models={{ choices: [], current: null }}
    onModel={() => {}}
    onEffort={() => {}}
    onLoadModels={() => {}}
    context={null}
    usage={null}
    onCompact={() => {}}
  />
);

/** Only what the chat on screen reads: a conversation that has not started. */
const chatState = (chatId: string) => ({
  chatId,
  role: "assistant" as const,
  roles: [{ id: "assistant" as const, name: "Assistant", description: "Answers in text" }],
  turn: { blocks: [], status: "done" as const, usage: null },
  busy: false,
  draft: null,
  preview: null,
  branchable: null,
  error: null,
  kube: null,
  context: null,
  spent: null,
  send: () => {},
  decide: () => {},
  stop: () => {},
  setRole: () => {},
  setPin: () => {},
});
type Chat = Parameters<typeof PlainChat>[0]["chat"];
type Kube = Parameters<typeof PlainChat>[0]["kube"];

const chatBox = (chatId: string) => (
  <PlainChat
    chat={chatState(chatId) as unknown as Chat}
    focus={0}
    models={{ choices: [], current: null }}
    onModel={() => {}}
    onEffort={() => {}}
    onLoadModels={() => {}}
    kube={{ configs: [], pick: () => {} } as unknown as Kube}
    onSetUpKube={() => {}}
    onCompact={() => {}}
  />
);

const box = () => screen.getByRole("textbox") as HTMLTextAreaElement;

/** Narrow the column, then widen it back — the height follows the width, no keystroke needed. */
const thereAndBack = async () => {
  const input = box();
  fireEvent.change(input, { target: { value: "a line that fills the box" } });
  // IDE layout: the column narrows, the line takes two lines.
  metrics(input, { clientWidth: 300, scrollHeight: 60 });
  widthChange();
  await waitFor(() => expect(input.style.height).toBe("60px"));
  // Out of IDE layout: the column is wide, the line is one again — nothing typed.
  metrics(input, { clientWidth: 640, scrollHeight: 30 });
  widthChange();
  await waitFor(() => expect(input.style.height).toBe("30px"));
};

describe("the agent's box follows the column's width", () => {
  test("a wrapped line is re-measured in and out of IDE layout", async () => {
    render(agentBox());
    await thereAndBack();
  });
});

describe("Chat mode's box follows the column's width", () => {
  test("a wrapped line is re-measured in and out of IDE layout", async () => {
    render(chatBox("width-chat"));
    await thereAndBack();
  });
});
