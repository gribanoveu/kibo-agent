import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { Composer } from "../components/Composer";
import { IS_MAC } from "../lib/shortcuts";

const calls: { command: string; args?: Record<string, unknown> }[] = [];
/** The channel the latest `dictation_start` got: its events are driven by hand. */
let dictationChannel: { onmessage: ((event: unknown) => void) | null } | null = null;
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "dictation_start") dictationChannel = args?.onEvent as { onmessage: ((event: unknown) => void) | null };
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

mock.module("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
const originalExecCommand = document.execCommand;
afterEach(() => {
  calls.length = 0;
  dictationChannel = null;
  localStorage.clear();
  document.execCommand = originalExecCommand;
});

const box = (dictate: boolean) => (
  <Composer
    draftKey="dictation-layout-test"
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
    dictate={dictate}
    layoutCorrection
  />
);
const input = () => screen.getByRole("textbox") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(input(), { target: { value: text } });
const applyKey = () => fireEvent.keyDown(input(), { code: "KeyF", key: "f", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });

describe("layout correction with the voice-input feature on", () => {
  test("the hint appears for text typed while dictation is not running", async () => {
    render(box(true));
    type("ghbdtn");
    expect(await screen.findByRole("button", { name: "Fix" }, undefined, { timeout: 1200 })).toBeTruthy();
  });

  test("the shortcut fixes text typed while dictation is not running", async () => {
    render(box(true));
    type("ghbdtn");
    applyKey();
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(input().value).toBe("привет");
  });
});

const say = (event: unknown) => dictationChannel?.onmessage?.(event);
const altDown = () => fireEvent.keyDown(window, { code: "AltRight", key: "Alt", altKey: true });
const altUp = () => fireEvent.keyUp(window, { code: "AltRight", key: "Alt" });
const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

describe("layout correction around a dictation", () => {
  test("it is quiet while dictating and works again once the dictation ends", async () => {
    render(box(true));
    // Hold Right Alt past HOLD_MS: the dictation begins.
    altDown();
    await sleep(250);
    expect(dictationChannel).toBeTruthy();
    say({ kind: "preparing" });
    await sleep(50);
    expect(input().readOnly).toBe(true);

    // A phrase goes into the box; no hint is offered mid-dictation.
    say({ kind: "phrase", text: "привет" });
    await sleep(50);
    expect(input().value).toContain("привет");
    fireEvent.blur(input());
    await sleep(600);
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();

    // The key comes up, the end is recognized, the dictation ends.
    altUp();
    say({ kind: "ended", reason: "stopped" });
    await sleep(50);
    expect(input().readOnly).toBe(false);

    // From here the box is the typist's again: the hint works.
    type("ghbdtn");
    expect(await screen.findByRole("button", { name: "Fix" }, undefined, { timeout: 1200 })).toBeTruthy();
  });

  test("a dictation that fails leaves the hint working", async () => {
    render(box(true));
    altDown();
    await sleep(250);
    say({ kind: "preparing" });
    await sleep(50);
    say({ kind: "failed", message: "no microphone" });
    await sleep(50);
    expect(input().readOnly).toBe(false);

    type("ghbdtn");
    expect(await screen.findByRole("button", { name: "Fix" }, undefined, { timeout: 1200 })).toBeTruthy();
  });
});
