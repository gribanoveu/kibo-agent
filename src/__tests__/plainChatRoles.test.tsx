import { describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";

// Who the model is in Chat mode: small chips under the card, each its own
// press. One role per chat — what was said was said to it — so once the chat
// has started only its own chip is left, and it is not a press.

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
const { emptyTurn } = await import("../lib/chatTurnReducer");

type Chat = Parameters<typeof PlainChat>[0]["chat"];
type Kube = Parameters<typeof PlainChat>[0]["kube"];

const ROLES = [
  { id: "assistant" as const, name: "Assistant", description: "Answers in text" },
  { id: "kubernetes" as const, name: "Kubernetes", description: "Reads the cluster" },
  { id: "prototype" as const, name: "Prototype", description: "Builds a page" },
];

const screenWith = (started: boolean, picked: string[] = []) => {
  const chat = {
    chatId: "c1",
    role: "assistant" as const,
    roles: ROLES,
    turn: { ...emptyTurn(), status: "done" as const, blocks: started ? [{ kind: "user" as const, id: "u0", text: "hi" }] : [] },
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
    setRole: (id: string) => picked.push(id),
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
  return picked;
};

const chips = () => [...document.querySelectorAll(".plain-role")];

describe("the role chips under Chat mode's card", () => {
  test("every role is its own press before the first message, and the current one is marked", () => {
    const picked = screenWith(false);
    expect(chips().map((c) => c.textContent)).toEqual(["Assistant", "Kubernetes", "Prototype"]);
    expect(chips().map((c) => c.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
    expect(screen.getByRole("radiogroup", { name: "Role" })).toBeTruthy();
    // Each says what the role is for, as the menu's hints did.
    expect(chips()[1].getAttribute("title")).toBe("Reads the cluster");

    fireEvent.click(chips()[2]);
    expect(picked).toEqual(["prototype"]);
  });

  test("a chat under way keeps its own role: one chip, marked, and not a press", () => {
    const picked = screenWith(true);
    expect(chips().map((c) => c.textContent)).toEqual(["Assistant"]);
    expect(chips()[0].getAttribute("aria-disabled")).toBe("true");
    expect(chips()[0].getAttribute("title")).toContain("start a new chat");
    fireEvent.click(chips()[0]);
    expect(picked).toEqual([]);
  });

  /// Under the card, not on the strip inside it: the strip is what the message
  /// goes out with, the chips are who it goes to.
  test("the row sits under the card, outside the strip", () => {
    screenWith(false);
    const wrap = document.querySelector(".plain-composer-wrap")!;
    const row = wrap.querySelector(".plain-roles")!;
    expect(row.parentElement).toBe(wrap);
    expect(wrap.querySelector(".plain-tab")!.contains(row)).toBe(false);
    expect([...wrap.children].indexOf(row)).toBeGreaterThan([...wrap.children].indexOf(wrap.querySelector(".plain-tab")!));
  });

  /// A radio group is one tab stop and answers the arrows: the choice moves,
  /// and the focus with it — wrapping at either end.
  test("the arrows move the choice round the group, which is one tab stop", () => {
    const picked = screenWith(false);
    expect(chips().map((c) => c.getAttribute("tabindex"))).toEqual(["0", "-1", "-1"]);

    fireEvent.keyDown(chips()[0], { code: "ArrowRight", key: "ArrowRight" });
    expect(picked).toEqual(["kubernetes"]);
    fireEvent.keyDown(chips()[0], { code: "ArrowLeft", key: "ArrowLeft" });
    expect(picked).toEqual(["kubernetes", "prototype"]);
    // Up and down belong to the / menu, not to this row: they are left alone.
    fireEvent.keyDown(chips()[0], { code: "ArrowDown", key: "ArrowDown" });
    fireEvent.keyDown(chips()[0], { code: "KeyA", key: "a" });
    expect(picked).toHaveLength(2);
  });

  test("a chat under way answers no arrow: its role is fixed", () => {
    const picked = screenWith(true);
    fireEvent.keyDown(chips()[0], { code: "ArrowRight", key: "ArrowRight" });
    expect(picked).toEqual([]);
  });
});
