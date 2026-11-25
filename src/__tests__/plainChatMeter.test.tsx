import { describe, expect, mock, test } from "bun:test";
import { render } from "@testing-library/react";
import { emptyTurn } from "../lib/chatTurnReducer";

// The context ring is what the next request costs. On a start screen there is
// no request to cost yet: the prompt and the schemas an empty chat already
// carries are read in Overview, and the ring waits for the first message.

mock.module("@tauri-apps/api/core", () => ({
  invoke: () => Promise.resolve(null),
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));
mock.module("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { PlainChat } = await import("../components/PlainChat");

type Chat = Parameters<typeof PlainChat>[0]["chat"];
type Kube = Parameters<typeof PlainChat>[0]["kube"];

const CONTEXT = { instructions: 1_000, tools: 3_000, conversation: 0, total: 4_000, limit: 200_000, compactsAt: null };

const screenWith = (started: boolean) => {
  const chat = {
    chatId: "c1",
    role: "assistant" as const,
    roles: [{ id: "assistant" as const, name: "Assistant", description: "Answers in text" }],
    turn: { ...emptyTurn(), status: "done" as const, blocks: started ? [{ kind: "user" as const, id: "u0", text: "hi" }] : [] },
    busy: false,
    draft: null,
    preview: null,
    branchable: null,
    error: null,
    kube: null,
    context: CONTEXT,
    spent: null,
    send: () => {},
    decide: () => {},
    stop: () => {},
    setRole: () => {},
    setPin: () => {},
  };
  render(
    <PlainChat
      chat={chat as unknown as Chat}
      focus={0}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      kube={{ configs: [], pick: () => {} } as unknown as Kube}
      onSetUpKube={() => {}}
      onCompact={() => {}}
    />,
  );
};

describe("the context ring in Chat mode", () => {
  test("is not on a start screen, though the estimate is already there", () => {
    screenWith(false);
    expect(document.querySelector(".plain-meter")).toBeNull();
  });

  test("is beside the box from the first message on", () => {
    screenWith(true);
    expect(document.querySelector(".plain-meter")).toBeTruthy();
  });
});
