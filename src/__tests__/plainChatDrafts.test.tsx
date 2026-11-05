import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";

// Chat mode's box belongs to its chat, as the agent's does: what is typed in
// one chat is not in the next, and comes back with the chat it was typed in.
// The two modes save their chats in stores of their own, so the key says which
// mode's chat it is.

const calls: { command: string; args: unknown }[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args: unknown) => {
    calls.push({ command, args });
    // A pasted picture, sanitized: what the backend answers with.
    if (command === "image_prepare") return Promise.resolve({ mediaType: "image/png", data: "PNG", width: 4, height: 3 });
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

// The balance chip beside the model listens for features being switched.
mock.module("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { PlainChat } = await import("../components/PlainChat");
const { chatDraftKey, readChatDraft } = await import("../hooks/useChatDraft");
const { useState } = await import("react");

afterEach(() => {
  calls.length = 0;
  localStorage.clear();
});

/** Only what the chat on screen reads: a conversation that has not started. */
const state = (chatId: string | null, send: (text: string) => void = () => {}, role: "assistant" | "prototype" = "assistant") => ({
  chatId,
  role,
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
  send,
  decide: () => {},
  stop: () => {},
  setRole: () => {},
  setPin: () => {},
});

type Chat = Parameters<typeof PlainChat>[0]["chat"];
type Kube = Parameters<typeof PlainChat>[0]["kube"];

/** The chat as `PlainChat` is drawn for it — `state` stands in for `usePlainChat`. */
const chatScreen = (
  chatId: string | null,
  send?: (text: string) => void,
  imagesEnabled = false,
  role: "assistant" | "prototype" = "assistant",
) => (
  <PlainChat
    chat={state(chatId, send, role) as unknown as Chat}
    focus={0}
    imagesEnabled={imagesEnabled}
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
const type = (text: string) => fireEvent.change(box(), { target: { value: text } });

describe("Chat mode's box", () => {
  /// One rule for every box (`boxTakesPictures`): a model that sees pictures,
  /// and a Prototype chat, whose pictures are the page's assets whatever the
  /// model can see.
  test("a picture can be chosen beside send, when the box takes them", () => {
    const { rerender } = render(chatScreen("a"));
    expect(screen.queryByRole("button", { name: "Attach a picture" })).toBeNull();

    rerender(chatScreen("a", undefined, true));
    const attach = screen.getByRole("button", { name: "Attach a picture" });
    expect(attach.nextElementSibling?.getAttribute("title")).toBe("Send");

    // Text only: an assistant chat takes none; a Prototype chat keeps its assets.
    rerender(chatScreen("a", undefined, false));
    expect(screen.queryByRole("button", { name: "Attach a picture" })).toBeNull();

    rerender(chatScreen("a", undefined, false, "prototype"));
    expect(screen.getByRole("button", { name: "Attach a picture" })).toBeTruthy();
  });

  /// A picture is a message of its own here too: with nothing typed, the send
  /// button is the way to send it — not only Enter.
  test("a picture alone can be sent with the button", async () => {
    render(chatScreen("a", undefined, true));
    fireEvent.paste(box(), { clipboardData: { files: [new File([new Uint8Array([1])], "shot.png", { type: "image/png" })] } });

    await waitFor(() => expect(screen.getByAltText("Image 1, 4×3")).toBeTruthy());
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false);
  });

  test("each chat finds its own text, and only its own", () => {
    const { rerender } = render(chatScreen("a"));
    type("про первый");
    // Chat mode's chats have their own keys: the agent's chat with the same id
    // is another chat, and its own draft.
    expect(localStorage.getItem(chatDraftKey("chat", "a"))).toBe("про первый");

    rerender(chatScreen("b"));
    expect(box().value).toBe("");

    type("про второй");
    rerender(chatScreen("a"));
    expect(box().value).toBe("про первый");
    rerender(chatScreen("b"));
    expect(box().value).toBe("про второй");
  });

  /// A chat not saved yet is one slot: what was typed in it is in the next new
  /// one, and not in a chat opened from the list.
  test("a chat opened from the list does not take the new chat's text", () => {
    const { rerender } = render(chatScreen(null));
    type("черновик нового");

    rerender(chatScreen("a"));
    expect(box().value).toBe("");
    rerender(chatScreen(null));
    expect(box().value).toBe("черновик нового");
  });

  /// The first message of a new chat gives the chat its id in the same commit;
  /// the draft that has to go is the one the text was typed under, not the one
  /// the box shows a moment later.
  test("a message sent leaves no draft for the next new chat", () => {
    const sent: string[] = [];
    function First() {
      const [chatId, setChatId] = useState<string | null>(null);
      return chatScreen(chatId, (text) => {
        sent.push(text);
        setChatId(crypto.randomUUID());
      });
    }
    render(<First />);
    type("сделай");
    expect(readChatDraft(chatDraftKey("chat", null))).toBe("сделай");

    fireEvent.keyDown(box(), { code: "Enter", key: "Enter" });
    expect(sent).toEqual(["сделай"]);
    expect(box().value).toBe("");
    expect(readChatDraft(chatDraftKey("chat", null))).toBe("");
  });
});
