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
const { emptyTurn } = await import("../lib/chatTurnReducer");

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

const agentBox = (stacked = false) => (
  <Composer
    stacked={stacked}
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
const chatState = (chatId: string, started = false) => ({
  chatId,
  role: "assistant" as const,
  roles: [{ id: "assistant" as const, name: "Assistant", description: "Answers in text" }],
  turn: { ...emptyTurn(), blocks: started ? [{ kind: "user" as const, id: "u0", text: "hi" }] : [], status: "done" as const },
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

const chatBox = (chatId: string, started = false) => (
  <PlainChat
    chat={chatState(chatId, started) as unknown as Chat}
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

/** A narrow column's taller box: the cap comes from the stylesheet, so the
    box stops where CSS says and scrolls past it, not at a constant. */
const capped = async () => {
  const input = box();
  fireEvent.change(input, { target: { value: "a message longer than the box" } });
  input.style.maxHeight = "80px";
  metrics(input, { clientWidth: 300, scrollHeight: 200 });
  widthChange();
  await waitFor(() => expect(input.style.height).toBe("80px"));
  expect(input.style.overflowY).toBe("auto");
};

/** Every style attribute the box gets from here on, in order — a value with
    "auto" in it is the collapse the measurement used to start from. */
const styleHistory = (input: HTMLTextAreaElement) => {
  const seen: string[] = [];
  const watch = new MutationObserver((records) => {
    for (const record of records) if (record.oldValue !== null) seen.push(record.oldValue);
  });
  watch.observe(input, { attributes: true, attributeFilter: ["style"], attributeOldValue: true });
  // The last write is still only in the attribute: the record of the one after
  // it would carry it, and there may be none.
  return () => {
    watch.disconnect();
    seen.push(input.getAttribute("style") ?? "");
    return seen.some((style) => style.includes("auto"));
  };
};

/**
 * The thread above the box is read at its end; a box collapsed to "auto" to be
 * measured grows that thread for the layout pass and the browser clamps the
 * thread's scroll there — the stale scroll read that follows is what made the
 * answer's tail pop out of view and back with every letter typed. Text that
 * only got longer cannot need less room, so it is measured in place; deleting
 * it may, and starts from the top as before.
 */
const measuredInPlace = async () => {
  const input = box();
  metrics(input, { clientWidth: 640, scrollHeight: 30 });
  fireEvent.change(input, { target: { value: "a" } });

  const typing = styleHistory(input);
  fireEvent.change(input, { target: { value: "ab" } });
  fireEvent.change(input, { target: { value: "abc" } });
  await Bun.sleep(0);
  expect(typing()).toBe(false);

  const deleting = styleHistory(input);
  fireEvent.change(input, { target: { value: "ab" } });
  await Bun.sleep(0);
  expect(deleting()).toBe(true);
};

/** More than one line: the text takes the whole box and the buttons a row
    under it; back beside them only when it fits one line there too — at the
    full width it may fit while beside them it wraps, and the switch must not
    flip with every measure. */
const stacksWhenTall = async () => {
  const input = box();
  const bar = input.closest(".composer-bar, .plain-composer-bar") as HTMLElement;
  metrics(bar, { clientWidth: 640 });
  // The buttons take 340px beside the text (a 334px button and a 6px gap);
  // their own widths are what the probe subtracts, in either layout.
  for (const child of Array.from(bar.children)) {
    if (child !== input && child !== input.parentElement) metrics(child as HTMLElement, { offsetWidth: 334 });
  }
  // Beside the buttons the text has 300px. The probe sets the laid width the
  // way each box honours it: width for the agent's block textarea, flex-basis
  // for Chat mode's flex item.
  let laid = 300;
  const probe = () => input.style.width || input.style.flex.match(/([\d.]+)px/)?.[1] || "";
  Object.defineProperty(input, "clientWidth", {
    configurable: true,
    get: () => (probe() ? parseFloat(probe()) : laid),
  });
  Object.defineProperty(input, "scrollHeight", {
    configurable: true,
    get: () => (input.clientWidth <= 300 ? 60 : 30),
  });
  fireEvent.change(input, { target: { value: "a line that wraps beside the buttons" } });
  await waitFor(() => expect(bar.className).toContain("tall"));
  // Stacked, the text has the full width and is one line there — but beside
  // the buttons it still wraps, so it stays stacked.
  laid = 640;
  widthChange();
  await waitFor(() => expect(input.style.height).toBe("30px"));
  expect(bar.className).toContain("tall");
  // A short message fits one line beside the buttons too: back to one row.
  Object.defineProperty(input, "scrollHeight", { configurable: true, get: () => 30 });
  fireEvent.change(input, { target: { value: "short" } });
  await waitFor(() => expect(bar.className).not.toContain("tall"));
};

describe("the agent's box follows the column's width", () => {
  test("a wrapped line is re-measured in and out of IDE layout", async () => {
    render(agentBox());
    await thereAndBack();
  });

  test("stops at the stylesheet's cap, not a number of its own", async () => {
    render(agentBox());
    await capped();
  });

  test("is measured in place while text is added", async () => {
    render(agentBox());
    await measuredInPlace();
  });

  test("more than one line takes the whole box, the buttons a row under it", async () => {
    render(agentBox());
    await stacksWhenTall();
  });

  test("a chat under way is two rows from the start, even with an empty box", () => {
    render(agentBox(true));
    const bar = box().closest(".composer-bar") as HTMLElement;
    expect(bar.className).toContain("tall");
  });
});

describe("Chat mode's box follows the column's width", () => {
  test("a wrapped line is re-measured in and out of IDE layout", async () => {
    render(chatBox("width-chat"));
    await thereAndBack();
  });

  test("stops at the stylesheet's cap, not a number of its own", async () => {
    render(chatBox("width-chat-cap"));
    await capped();
  });

  test("is measured in place while text is added", async () => {
    render(chatBox("width-chat-place"));
    await measuredInPlace();
  });

  test("more than one line takes the whole box, the buttons a row under it", async () => {
    render(chatBox("width-chat-tall"));
    await stacksWhenTall();
  });

  test("a chat under way is two rows from the start, even with an empty box", () => {
    render(chatBox("width-chat-started", true));
    const bar = box().closest(".plain-composer-bar") as HTMLElement;
    expect(bar.className).toContain("tall");
  });
});
