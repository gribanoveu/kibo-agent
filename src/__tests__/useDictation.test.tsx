import { afterAll, afterEach, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { useRef, useState } from "react";
import type { DictationEvent } from "../lib/dictation";

const calls: { command: string; args: any }[] = [];
let refuseMenu = false;
let chosen: string | null = null;
const tones: string[] = [];
let access = "granted";
let answer = "granted";
let prompt: Promise<string> | null = null;
let refuseStart = false;
let startGate: Promise<void> | null = null;
let channel: { onmessage: (event: DictationEvent) => void } | null = null;

class FakeChannel {
  onmessage: (event: DictationEvent) => void = () => {};
}

mock.module("@tauri-apps/api/core", () => ({
  Channel: FakeChannel,
  invoke: (command: string, args: any) => {
    calls.push({ command, args });
    if (command === "meeting_recording_access") return Promise.resolve({ microphone: access, call: "granted" });
    if (command === "meeting_recording_access_request") return prompt ?? Promise.resolve(answer);
    if (command === "meeting_microphones") return refuseMenu ? Promise.reject("no devices") : Promise.resolve([{ id: "m1", name: "Гарнитура" }]);
    if (command === "meeting_microphone_get") return Promise.resolve(chosen);
    if (command === "dictation_start") {
      if (refuseStart) return Promise.reject("a dictation is already running");
      channel = args.onEvent;
      if (startGate) return startGate;
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

// The box listens for files dropped on the window; nothing is dropped here.
mock.module("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}));

// The balance chip beside the model listens for features being switched.
mock.module("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

mock.module("../lib/sounds", () => ({
  playTone: (tone: string) => {
    tones.push(tone);
    return Promise.reject(new Error("no output device"));
  },
  playSound: () => Promise.resolve(),
  preloadSounds: () => {},
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};

const { useDictation } = await import("../hooks/useDictation");
const { DictateButton } = await import("../components/DictateButton");
const { HOLD_MS } = await import("../lib/dictation");
const { Composer } = await import("../components/Composer");
const { PlainChat } = await import("../components/PlainChat");

const cursorMoves: (string | number)[] = [];
let changes = 0;
const said = (command: string) => calls.filter((c) => c.command === command).length;
const tell = (event: DictationEvent) => act(() => channel!.onmessage(event));
const settled = () => act(() => new Promise<void>((done) => setTimeout(done, 0)));
const pass = (ms: number) => act(() => new Promise<void>((done) => setTimeout(done, ms)));

beforeEach(() => {
  calls.length = tones.length = cursorMoves.length = changes = 0;
  access = answer = "granted";
  prompt = null;
  refuseStart = false;
  startGate = null;
  refuseMenu = false;
  chosen = null;
  channel = null;
  document.body.innerHTML = "";
});

/** The hook over a box of its own; `key` is whose. */
function box(start = "", on = true, cursor = start.length) {
  return renderHook(
    ({ draftKey, on }) => {
      const [text, setText] = useState(start);
      const area = useRef({
        selectionStart: cursor,
        focus: () => cursorMoves.push("focus"),
        setSelectionRange: (at: number) => cursorMoves.push(at),
      } as unknown as HTMLTextAreaElement);
      return { text, setText, dictation: useDictation({ on, draftKey, text, setText, area, onChange: () => changes++ }) };
    },
    { initialProps: { draftKey: "a", on } },
  );
}

async function heldKeyBeforePreparing() {
  const hook = box();
  await settled();
  fireEvent.keyDown(window, { code: "AltRight", altKey: true });
  await pass(HOLD_MS * 1.2);
  return hook;
}

/** Started by the key, and still held. */
async function heldKey() {
  const hook = await heldKeyBeforePreparing();
  tell({ kind: "preparing" });
  return hook;
}

async function started(start = "") {
  const hook = box(start);
  await settled();
  act(() => hook.result.current.dictation.press());
  await settled();
  tell({ kind: "preparing" });
  return hook;
}

describe("a dictation", () => {
  test("plays its sound on the first buffer, and puts phrases in the box in turn", async () => {
    const { result } = await started("Да");
    expect(tones).toEqual([]);
    tell({ kind: "listening" });
    expect(tones).toEqual(["listening"]);
    expect(result.current.dictation.active).toBe(true);
    await act(async () => {
      // Two before the next render: the second goes after the first.
      channel!.onmessage({ kind: "phrase", text: "Раз." });
      channel!.onmessage({ kind: "phrase", text: "Два." });
    });
    expect(result.current.text).toBe("Да Раз. Два.");
  });

  test("Stop sounds at once, leaves the text and ends when told", async () => {
    const { result } = await started("Да");
    tell({ kind: "listening" });
    act(() => result.current.dictation.press());
    await settled();
    expect(tones).toEqual(["listening", "stopped"]);
    expect(said("dictation_stop")).toBe(1);
    expect(result.current.dictation.phase).toBe("finishing");
    expect(result.current.dictation.button.press).toBeNull();
    tell({ kind: "phrase", text: "Конец." });
    tell({ kind: "ended", reason: "stopped" });
    expect(result.current.text).toBe("Да Конец.");
    expect(result.current.dictation.active).toBe(false);
  });

  test("the first buffer after Stop plays nothing", async () => {
    const { result } = await started();
    tell({ kind: "listening" });
    act(() => result.current.dictation.press());
    tones.length = 0;
    tell({ kind: "listening" });
    expect(tones).toEqual([]);
  });

  test("the limit sounds as a Stop does", async () => {
    await started();
    tell({ kind: "ended", reason: "limit" });
    expect(tones).toEqual(["stopped"]);
  });

  test("a cancel goes back to the box as it was, whatever came", async () => {
    const { result } = await started("Было");
    tell({ kind: "listening" });
    tell({ kind: "phrase", text: "Лишнее." });
    expect(result.current.text).toBe("Было Лишнее.");
    await act(async () => {
      fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    });
    await settled();
    expect(said("dictation_cancel")).toBe(1);
    tell({ kind: "ended", reason: "cancelled" });
    expect(result.current.text).toBe("Было");
  });

  test("Escape with no dictation is someone else's", async () => {
    box();
    await settled();
    let seen = false;
    const listener = () => (seen = true);
    window.addEventListener("keydown", listener);
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    window.removeEventListener("keydown", listener);
    expect(seen).toBe(true);
    expect(said("dictation_cancel")).toBe(0);
  });

  test("a refused start says why and frees the button", async () => {
    refuseStart = true;
    const { result } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    expect(result.current.dictation.error).toContain("already running");
    act(() => result.current.dictation.clearError());
    expect(result.current.dictation.error).toBeNull();
    refuseStart = false;
    act(() => result.current.dictation.press());
    await settled();
    expect(said("dictation_start")).toBe(2);
  });

  test("a failure says why and the text stays", async () => {
    const { result } = await started();
    tell({ kind: "phrase", text: "Успело." });
    tell({ kind: "failed", message: "процесс упал" });
    expect(result.current.dictation.error).toBe("процесс упал");
    expect(result.current.text).toBe("Успело.");
    act(() => result.current.dictation.press());
    await settled();
    expect(said("dictation_start")).toBe(2);
  });

  test("another chat opened throws it away and nothing more reaches the box", async () => {
    const { result, rerender } = await started("Было");
    tell({ kind: "listening" });
    const late = channel!;
    rerender({ draftKey: "b" });
    await settled();
    expect(said("dictation_cancel")).toBe(1);
    expect(result.current.dictation.active).toBe(false);
    act(() => late.onmessage({ kind: "phrase", text: "Чужое." }));
    expect(result.current.text).toBe("Было");
  });

  test("something put in the box from outside is not undone, and the next phrase goes after it", async () => {
    const { result } = await started("Было");
    tell({ kind: "phrase", text: "Раз." });
    act(() => result.current.setText((t) => `${t}\n\nИз очереди`));
    tell({ kind: "phrase", text: "Два." });
    expect(result.current.text).toBe("Было Раз.\n\nИз очереди Два.");
    act(() => result.current.setText("Другое"));
    tell({ kind: "ended", reason: "cancelled" });
    expect(result.current.text).toBe("Другое");
  });

  test("leaves the cursor after what it said, in the box", async () => {
    await started("Да");
    tell({ kind: "phrase", text: "Нет." });
    expect(changes).toBe(1);
    tell({ kind: "ended", reason: "stopped" });
    await pass(40);
    expect(cursorMoves).toEqual(["focus", 7]);
  });

  test("while getting ready, the button cancels", async () => {
    const { result } = await started();
    act(() => result.current.dictation.press());
    await settled();
    expect(said("dictation_cancel")).toBe(1);
  });

  test("Stop ahead of its `preparing` stays a Stop", async () => {
    const { result } = await heldKeyBeforePreparing();
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    tell({ kind: "preparing" });
    tell({ kind: "listening" });
    expect(result.current.dictation.phase).toBe("finishing");
  });

  test("a cancel then the key let go is no Stop", async () => {
    await heldKey();
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    expect(said("dictation_stop")).toBe(0);
    expect(tones).toEqual([]);
  });

  test("Escape leaves an open menu to close itself", async () => {
    await started("Было");
    tell({ kind: "listening" });
    document.body.innerHTML = '<div role="listbox"></div>';
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    await settled();
    expect(said("dictation_cancel")).toBe(0);
  });

  test("a start refused after the box moved on says nothing here", async () => {
    let refused = (_: unknown) => {};
    startGate = new Promise((_, no) => (refused = no));
    const { result, rerender } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    rerender({ draftKey: "b", on: true });
    await act(async () => refused("gone"));
    await settled();
    expect(result.current.dictation.error).toBeNull();
  });

  test("another chat opened waits for the start before it cancels", async () => {
    let through = () => {};
    startGate = new Promise((done) => (through = done));
    const { result, rerender } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    rerender({ draftKey: "b", on: true });
    await settled();
    expect(said("dictation_cancel")).toBe(0);
    through();
    await settled();
    expect(said("dictation_cancel")).toBe(1);
  });

  test("the box hidden behind a page throws it away too", async () => {
    const { result, rerender } = await started("Было");
    tell({ kind: "listening" });
    tell({ kind: "phrase", text: "Сказанное." });
    rerender({ draftKey: "a", on: false });
    await settled();
    expect(said("dictation_cancel")).toBe(1);
    expect(result.current.dictation.active).toBe(false);
    expect(result.current.text).toBe("Было Сказанное.");
  });

  test("pressed twice before it is up is one dictation", async () => {
    const { result } = box();
    await settled();
    act(() => {
      result.current.dictation.press();
      result.current.dictation.press();
    });
    await settled();
    expect(said("dictation_start")).toBe(1);
  });

  test("goes where the cursor was, not at the end", async () => {
    const { result } = box("начало конец", true, 6);
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    tell({ kind: "phrase", text: "Середина." });
    expect(result.current.text).toBe("начало Середина. конец");
  });

  test("a second Stop is one Stop", async () => {
    const { result } = await heldKey();
    tell({ kind: "listening" });
    act(() => result.current.dictation.press());
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    expect(said("dictation_stop")).toBe(1);
    expect(tones.filter((t) => t === "stopped").length).toBe(1);
  });

  test("held again while the last one finishes starts once that ends", async () => {
    await heldKey();
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    expect(said("dictation_start")).toBe(1);
    tell({ kind: "ended", reason: "stopped" });
    await settled();
    expect(said("dictation_start")).toBe(2);
  });

  test("held again after a cancel starts from the box as it went back", async () => {
    const { result } = await heldKey();
    tell({ kind: "phrase", text: "Лишнее." });
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    fireEvent.keyUp(window, { code: "AltRight" });
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    tell({ kind: "ended", reason: "cancelled" });
    await settled();
    tell({ kind: "preparing" });
    tell({ kind: "phrase", text: "Новая." });
    expect(result.current.text).toBe("Новая.");
    tell({ kind: "ended", reason: "cancelled" });
    expect(result.current.text).toBe("");
  });

  test("held again and let go in time starts nothing", async () => {
    await heldKey();
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    fireEvent.keyUp(window, { code: "AltRight" });
    tell({ kind: "ended", reason: "stopped" });
    await settled();
    expect(said("dictation_start")).toBe(1);
  });

  test("Stop and Cancel wait until the service knows of it", async () => {
    for (const [end, command] of [[() => fireEvent.keyUp(window, { code: "AltRight" }), "dictation_stop"], [() => fireEvent.keyDown(window, { code: "Escape", key: "Escape" }), "dictation_cancel"]] as const) {
      calls.length = 0;
      let through = () => {};
      startGate = new Promise((done) => (through = done));
      const hook = box();
      await settled();
      fireEvent.keyDown(window, { code: "AltRight", altKey: true });
      await pass(HOLD_MS * 1.2);
      end();
      await settled();
      expect(said(command)).toBe(0);
      through();
      await settled();
      expect(said(command)).toBe(1);
      fireEvent.keyUp(window, { code: "AltRight" });
      hook.unmount();
      await settled();
    }
  });

  test("Escape is the dictation's while one runs", async () => {
    await started();
    let seen = false;
    const listener = () => (seen = true);
    window.addEventListener("keydown", listener, true);
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    window.removeEventListener("keydown", listener, true);
    expect(seen).toBe(false);
  });
});

describe("the system's leave", () => {
  test("asked first when it never was, and started when given", async () => {
    access = "notAsked";
    const { result } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    expect(said("meeting_recording_access_request")).toBe(1);
    expect(said("dictation_start")).toBe(1);
  });

  test("not given, nothing starts, and the button works again", async () => {
    access = "notAsked";
    answer = "denied";
    const { result } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    expect(said("dictation_start")).toBe(0);
    expect(result.current.dictation.button.look).toBe("refused");
    act(() => result.current.dictation.press());
    await settled();
    expect(said("meeting_recording_access_settings")).toBe(1);
    expect(said("dictation_start")).toBe(0);
  });

  test("Escape while the prompt is up starts nothing", async () => {
    access = "notAsked";
    let given = (_: string) => {};
    prompt = new Promise((done) => (given = done));
    const { result } = box();
    await settled();
    act(() => result.current.dictation.press());
    await settled();
    fireEvent.keyDown(window, { code: "Escape", key: "Escape" });
    given("granted");
    await settled();
    expect(said("dictation_start")).toBe(0);
  });

  test("refused, the key says so and does not start", async () => {
    access = "denied";
    const { result } = box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    expect(said("dictation_start")).toBe(0);
    expect(result.current.dictation.error).toContain("microphone");
  });

  test("the key let go while the prompt is up starts nothing", async () => {
    access = "notAsked";
    let given = (_: string) => {};
    prompt = new Promise((done) => (given = done));
    box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    expect(said("meeting_recording_access_request")).toBe(1);
    fireEvent.keyUp(window, { code: "AltRight" });
    given("granted");
    await settled();
    expect(said("dictation_start")).toBe(0);
  });

  test("with the feature off nothing is asked and no key is heard", async () => {
    box("", false);
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    fireEvent.keyUp(window, { code: "AltRight" });
    expect(calls).toEqual([]);
  });
});

describe("the key", () => {
  test("held starts, let go stops", async () => {
    box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS / 2);
    expect(said("dictation_start")).toBe(0);
    await pass(HOLD_MS);
    expect(said("dictation_start")).toBe(1);
    tell({ kind: "preparing" });
    fireEvent.keyUp(window, { code: "AltRight" });
    await settled();
    expect(said("dictation_stop")).toBe(1);
  });

  test("the window losing focus is letting go", async () => {
    box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    await pass(HOLD_MS * 1.2);
    tell({ kind: "preparing" });
    fireEvent.blur(window);
    await settled();
    expect(said("dictation_stop")).toBe(1);
  });

  test("another key on the way is a shortcut", async () => {
    box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    fireEvent.keyDown(window, { code: "Enter", altKey: true });
    await pass(HOLD_MS * 1.2);
    expect(said("dictation_start")).toBe(0);
  });

  test("another key let go is not this one", async () => {
    await heldKey();
    fireEvent.keyUp(window, { code: "ShiftLeft" });
    await settled();
    expect(said("dictation_stop")).toBe(0);
  });

  test("the box gone while it is held starts nothing", async () => {
    const { unmount } = box();
    await settled();
    fireEvent.keyDown(window, { code: "AltRight", altKey: true });
    unmount();
    await pass(HOLD_MS * 1.2);
    expect(said("dictation_start")).toBe(0);
  });
});

describe("the menu", () => {
  function Button() {
    const [text, setText] = useState("");
    const area = useRef<HTMLTextAreaElement>(null);
    return <DictateButton dictation={useDictation({ on: true, draftKey: "a", text, setText, area, onChange: () => {} })} />;
  }

  test("shows the one chosen, and one not plugged in says so", async () => {
    chosen = "gone";
    render(<Button />);
    await settled();
    fireEvent.click(screen.getByTitle("Dictate from"));
    expect(await screen.findByText("Chosen microphone, not plugged in")).toBeTruthy();
  });

  test("its failure leaves a dictation running", async () => {
    refuseMenu = true;
    const { result } = await started();
    tell({ kind: "listening" });
    act(() => result.current.dictation.readMicrophones());
    await settled();
    expect(result.current.dictation.error).toContain("no devices");
    expect(result.current.dictation.phase).toBe("listening");
  });

  test("lists the microphones when opened, and a pick is the setting Meetings reads", async () => {
    render(<Button />);
    await settled();
    expect(said("meeting_microphones")).toBe(0);
    fireEvent.click(screen.getByTitle("Dictate from"));
    await settled();
    fireEvent.click(await screen.findByText("Гарнитура"));
    await waitFor(() => expect(calls.at(-1)).toEqual({ command: "meeting_microphone_set", args: { id: "m1" } }));
    fireEvent.click(screen.getByTitle("Dictate from"));
    fireEvent.click(await screen.findByText("System default"));
    await waitFor(() => expect(calls.at(-1)).toEqual({ command: "meeting_microphone_set", args: { id: null } }));
  });
});

describe("the box of a plain chat", () => {
  type Chat = Parameters<typeof PlainChat>[0]["chat"];
  type Kube = Parameters<typeof PlainChat>[0]["kube"];
  const sent: string[] = [];

  async function plain(dictate = true) {
    sent.length = 0;
    localStorage.clear();
    const chat = {
      chatId: "c1",
      role: "assistant",
      roles: [{ id: "assistant", name: "Assistant", description: "Answers in text" }],
      turn: { blocks: [], status: "done", usage: null },
      busy: false,
      draft: null,
      preview: null,
      branchable: null,
      error: null,
      kube: null,
      context: null,
      spent: null,
      send: (text: string) => sent.push(text),
      decide: () => {},
      stop: () => {},
      setRole: () => {},
      setPin: () => {},
    } as unknown as Chat;
    render(
      <PlainChat
        chat={chat}
        focus={0}
        dictate={dictate}
        models={{ choices: [], current: null }}
        onModel={() => {}}
        onEffort={() => {}}
        onLoadModels={() => {}}
        kube={{ configs: [], pick: () => {} } as unknown as Kube}
        onSetUpKube={() => {}}
        onCompact={() => {}}
        onRewind={() => {}}
      />,
    );
    await settled();
    return screen.getByPlaceholderText("Message the model…") as HTMLTextAreaElement;
  }
  const dictating = async () => {
    fireEvent.click(screen.getByRole("button", { name: /^Dictate —/ }));
    await settled();
    tell({ kind: "preparing" });
    tell({ kind: "listening" });
  };
  const sendButton = () => screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;

  test("has no button and asks nothing while the feature is off", async () => {
    await plain(false);
    expect(screen.queryByRole("button", { name: /^Dictate —/ })).toBeNull();
    expect(said("meeting_recording_access")).toBe(0);
  });

  test("shows the wave and the phrase being said, and then the phrase is text", async () => {
    const area = await plain();
    await dictating();
    tell({ kind: "partial", text: "Привет, ми" });
    expect(document.querySelector(".voice-wave")).not.toBeNull();
    expect(document.querySelector(".dictation-partial")?.textContent).toBe("Привет, ми");
    tell({ kind: "phrase", text: "Привет, мир." });
    expect(area.value).toBe("Привет, мир.");
    tell({ kind: "ended", reason: "stopped" });
    expect(document.querySelector(".voice-wave")).toBeNull();
  });

  test("holds the text for the dictation, and gives it back to be sent by hand", async () => {
    const area = await plain();
    await dictating();
    tell({ kind: "phrase", text: "Привет." });
    expect(area.readOnly).toBe(true);
    expect(sendButton().disabled).toBe(true);
    fireEvent.keyDown(area, { code: "Enter", key: "Enter" });
    await settled();
    expect(sent).toEqual([]);
    tell({ kind: "ended", reason: "stopped" });
    expect(area.readOnly).toBe(false);
    fireEvent.keyDown(area, { code: "Enter", key: "Enter" });
    await settled();
    expect(sent).toEqual(["Привет."]);
  });

  test("says what went wrong above the field", async () => {
    await plain();
    await dictating();
    tell({ kind: "failed", message: "The microphone is gone" });
    expect(screen.getByText("The microphone is gone")).not.toBeNull();
  });
});

describe("the message box", () => {
  function box(dictate: boolean, onSend: (text: string) => void = () => {}, onQueue?: (text: string) => void) {
    localStorage.clear();
    return render(
      <Composer
        draftKey="dictation-box"
        dictate={dictate}
        onSend={onSend}
        onQueue={onQueue}
        onStop={() => {}}
        running={!!onQueue}
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
        commands={[{ name: "help", hint: "Help", run: () => {} }]}
      />,
    );
  }
  const area = () => screen.getByRole("textbox") as HTMLTextAreaElement;

  async function dictating(onSend?: (text: string) => void, onQueue?: (text: string) => void) {
    box(true, onSend, onQueue);
    await settled();
    fireEvent.click(document.querySelector(".dictate-mic")!);
    await settled();
    tell({ kind: "preparing" });
    tell({ kind: "listening" });
  }

  test("has no button and asks nothing while the feature is off", async () => {
    box(false);
    await settled();
    expect(document.querySelector(".dictate-mic")).toBeNull();
    expect(said("meeting_recording_access")).toBe(0);
  });

  test("draws the refusal on the button", async () => {
    access = "denied";
    box(true);
    await settled();
    expect(document.querySelector(".dictate-mic.refused")).toBeTruthy();
  });

  test("shows the wave and the phrase being said, and then the phrase is text", async () => {
    await dictating();
    tell({ kind: "levels", peaks: [0.25, 0.25, 0.25, 0.25] });
    tell({ kind: "partial", text: "Привет, ми" });
    expect(document.querySelector(".voice-wave")).toBeTruthy();
    expect(document.querySelector(".dictation-partial")?.textContent).toBe("Привет, ми");
    expect(document.querySelector(".dictate-mic.stop")).toBeTruthy();
    tell({ kind: "phrase", text: "Привет, мир." });
    expect(document.querySelector(".dictation-partial")).toBeNull();
    expect(area().value).toBe("Привет, мир.");
    tell({ kind: "ended", reason: "stopped" });
    expect(document.querySelector(".voice-wave")).toBeNull();
  });

  test("holds the text for the dictation, and gives it back to be sent by hand", async () => {
    const sent: string[] = [];
    await dictating((text) => sent.push(text));
    tell({ kind: "phrase", text: "Привет." });
    expect(area().readOnly).toBe(true);
    fireEvent.keyDown(area(), { code: "Enter", key: "Enter" });
    await settled();
    expect(sent).toEqual([]);
    tell({ kind: "ended", reason: "stopped" });
    expect(area().readOnly).toBe(false);
    expect(sent).toEqual([]);
    fireEvent.keyDown(area(), { code: "Enter", key: "Enter" });
    await settled();
    expect(sent).toEqual(["Привет."]);
  });

  test("holds nothing back for after the turn while it dictates", async () => {
    const queued: string[] = [];
    await dictating(undefined, (text) => queued.push(text));
    tell({ kind: "phrase", text: "Привет." });
    fireEvent.keyDown(area(), { code: "Enter", key: "Enter", altKey: true });
    await settled();
    expect(queued).toEqual([]);
  });

  test("offers no / commands while it dictates", async () => {
    await dictating();
    fireEvent.change(area(), { target: { value: "/he" } });
    expect(document.querySelector(".slash-menu")).toBeNull();
  });

  test("the wave follows what it heard", async () => {
    await dictating();
    tell({ kind: "levels", peaks: [0.25, 0.25, 0.25, 0.25] });
    const bars = [...document.querySelectorAll<HTMLElement>(".voice-wave span")].map((b) => b.style.height);
    expect(bars.length).toBe(48);
    expect(bars.at(-1)).toBe("50%");
    expect(bars[0]).toBe("6%");
  });

  test("says what went wrong above the field", async () => {
    await dictating();
    tell({ kind: "failed", message: "Микрофон не открылся" });
    expect(screen.getByText("Микрофон не открылся")).toBeTruthy();
  });
});

afterEach(() => {
  fireEvent.keyUp(window, { code: "AltRight" });
});

// The window is shared by every test file: the next one is not in Tauri.
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});
