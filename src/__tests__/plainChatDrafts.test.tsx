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
    if (command === "kube_contexts") {
      return Promise.resolve({ contexts: [{ name: "orbstack", cluster: "orbstack", namespace: "shop" }], current: "orbstack" });
    }
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
  test("a picture can be chosen right before the text, when the box takes them", () => {
    const { rerender } = render(chatScreen("a"));
    expect(screen.queryByRole("button", { name: "Attach a picture" })).toBeNull();

    rerender(chatScreen("a", undefined, true));
    const attach = screen.getByRole("button", { name: "Attach a picture" });
    expect(attach.nextElementSibling?.tagName).toBe("TEXTAREA");

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

describe("a Kubernetes chat's box", () => {
  type Pin = { kubeconfig: string; context: string | null; namespace: string | null; writes?: boolean };
  const pinned = (writes: boolean | undefined, pins: Pin[] = [], production = false) => (
    <PlainChat
      chat={
        {
          ...state("k"),
          role: "kubernetes",
          roles: [{ id: "kubernetes", name: "Kubernetes", description: "Works with a cluster" }],
          kube: { kubeconfig: "local", context: null, namespace: null, writes },
          setPin: (pin: Pin) => pins.push(pin),
        } as unknown as Chat
      }
      focus={0}
      models={{ choices: [], current: null }}
      onModel={() => {}}
      onEffort={() => {}}
      onLoadModels={() => {}}
      kube={{ configs: [{ name: "local", path: "/k", production }], pick: () => {} } as unknown as Kube}
      onSetUpKube={() => {}}
      onCompact={() => {}}
    />
  );
  const settings = () => fireEvent.click(screen.getByTitle("Mode, model and thinking"));
  const cluster = () => fireEvent.click(screen.getAllByTitle("Kubeconfig, context, namespace and access")[0]);
  const line = () =>
    [...document.querySelectorAll(".kube-panel .composer-settings-summary > span")].map((el) => el.textContent);
  const loaded = () => screen.findByText("shop");

  /// One chip for the whole cluster, and its panel apart from the model's.
  test("says the cluster on the line and sets it in a panel of its own", async () => {
    render(pinned(undefined));
    await loaded();
    expect(line()).toEqual(["local", "shop"]);
    expect(screen.queryByTitle("The namespace this chat works in")).toBeNull();

    cluster();
    expect(screen.getByTitle("The kubeconfig this chat works with").textContent).toContain("local");
    expect(screen.getByTitle("The context of the kubeconfig — which cluster, as which user").textContent).toContain("orbstack");
    expect(screen.getByTitle("The namespace this chat works in").textContent).toContain("shop");
    const toggle = screen.getByRole("switch", { name: "Allow changes" });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(toggle.closest("label")?.title).toContain("it may change shop");
  });

  test("the model's panel has none of it", async () => {
    render(pinned(undefined));
    await loaded();
    settings();
    expect(screen.getByTitle("Model")).toBeTruthy();
    expect(screen.queryByRole("switch", { name: "Allow changes" })).toBeNull();
    expect(screen.queryByTitle("The kubeconfig this chat works with")).toBeNull();
  });

  test("the switch goes to the chat's pin, the rest of the pin kept", async () => {
    const pins: Pin[] = [];
    render(pinned(false, pins));
    await loaded();
    cluster();
    fireEvent.click(screen.getByRole("switch", { name: "Allow changes" }));
    expect(pins).toEqual([{ kubeconfig: "local", context: null, namespace: null, writes: true }]);
  });

  /// Moved elsewhere, the chat is not the one that was allowed to change things.
  test("another namespace takes the permission away", async () => {
    const pins: Pin[] = [];
    render(pinned(true, pins));
    await loaded();
    cluster();
    fireEvent.click(screen.getByTitle("The namespace this chat works in"));
    const other = screen.getByRole("textbox", { name: "Another namespace" });
    fireEvent.change(other, { target: { value: "staging" } });
    fireEvent.keyDown(other, { code: "Enter", key: "Enter" });
    expect(pins).toEqual([{ kubeconfig: "local", context: null, namespace: "staging", writes: false }]);
  });

  /// The panel is closed most of the time, and a chat that may change a cluster
  /// is not something to find out by opening it.
  test("a chat that may change the cluster says so on the line, one that only reads does not", async () => {
    const { rerender } = render(pinned(true));
    await loaded();
    expect(line()).toEqual(["local", "shop", "changes"]);

    rerender(pinned(false));
    expect(line()).toEqual(["local", "shop"]);
  });

  /// As red on the line as it was on its chip: seen before a word is typed.
  test("a cluster marked production is red on the line", async () => {
    render(pinned(false, [], true));
    await loaded();
    expect(document.querySelector(".kube-panel .kube-panel-production")?.textContent).toBe("local");
  });

  /// Nothing to change until there is a cluster to change.
  test("has no switch before a context is known", () => {
    render(pinned(false));
    cluster();
    expect(screen.queryByRole("switch", { name: "Allow changes" })).toBeNull();
    expect(line()).toEqual(["local"]);
  });

  test("another role has no such chip", () => {
    render(chatScreen("a"));
    expect(screen.queryByTitle("Kubeconfig, context, namespace and access")).toBeNull();
  });
});
