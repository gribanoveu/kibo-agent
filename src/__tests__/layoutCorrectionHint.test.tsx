import { afterEach, describe, expect, mock, test } from "bun:test";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Composer } from "../components/Composer";
import { IS_MAC } from "../lib/shortcuts";
import { layoutCorrectionMetrics } from "../lib/layoutCorrection";

const calls: { command: string; args?: Record<string, unknown> }[] = [];
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
mock.module("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }) }));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
const originalExecCommand = document.execCommand;
afterEach(() => {
  calls.length = 0;
  localStorage.clear();
  document.execCommand = originalExecCommand;
});

const box = (on = true, switchLayoutAfterFix = false) => (
  <Composer
    draftKey="layout-test"
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
    layoutCorrection={on}
    switchLayoutAfterFix={switchLayoutAfterFix}
  />
);
const input = () => screen.getByRole("textbox") as HTMLTextAreaElement;
const type = (text: string) => fireEvent.change(input(), { target: { value: text } });
const applyKey = () => fireEvent.keyDown(input(), { code: "KeyF", key: "f", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });

describe("the agent box's layout hint", () => {
  test("blur offers a correction without changing the text; accepting keeps the selection and counts both events", async () => {
    render(box());
    type("ghbdtn");
    expect(input().value).toBe("ghbdtn");
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();

    fireEvent.blur(input());
    expect(await screen.findByRole("button", { name: "Fix" })).toBeTruthy();
    expect(screen.getByText("«привет»")).toBeTruthy();
    expect(input().value).toBe("ghbdtn");
    input().setSelectionRange(2, 4);
    fireEvent.click(screen.getByRole("button", { name: "Fix" }));

    await waitFor(() => expect(input().value).toBe("привет"));
    await waitFor(() => expect([input().selectionStart, input().selectionEnd]).toEqual([2, 4]));
    expect(layoutCorrectionMetrics()).toEqual({ shown: 1, accepted: 1 });
    expect(calls.filter((call) => call.command === "keyboard_layout_set")).toEqual([]);
  });

  test("the shortcut accepts immediately, without waiting for the debounce", async () => {
    render(box());
    type("руддщ");
    applyKey();
    await waitFor(() => expect(input().value).toBe("hello"));
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
    expect(layoutCorrectionMetrics()).toEqual({ shown: 0, accepted: 1 });
  });

  test("optional Fix selects the corrected prose's dominant system layout", async () => {
    render(box(true, true));
    type("ghbdtn");
    applyKey();
    await waitFor(() => expect(input().value).toBe("привет"));
    await waitFor(() => expect(calls).toContainEqual({ command: "keyboard_layout_set", args: { layout: "ru" } }));

    type("руддщ");
    applyKey();
    await waitFor(() => expect(input().value).toBe("hello"));
    await waitFor(() => expect(calls).toContainEqual({ command: "keyboard_layout_set", args: { layout: "en" } }));
  });

  test("the hint also appears after 500 ms, and not while the feature is off", async () => {
    const view = render(box());
    type("ghbdtn");
    expect(await screen.findByRole("button", { name: "Fix" }, undefined, { timeout: 1200 })).toBeTruthy();

    view.rerender(box(false));
    fireEvent.blur(input());
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
    type("руддщ");
    applyKey();
    expect(input().value).toBe("руддщ");
  });

  test("an untouched hint stays up, and later text is still detected", async () => {
    const view = render(box());
    type("ghbdtn");
    expect(await screen.findByRole("button", { name: "Fix" })).toBeTruthy();

    await new Promise((resolve) => setTimeout(resolve, 1100));
    view.rerender(box());
    expect(screen.getByRole("button", { name: "Fix" })).toBeTruthy();

    type("руддщ");
    expect(screen.queryByText("«привет»")).toBeNull();
    expect(await screen.findByText("«hello»")).toBeTruthy();
  });

  test("new text hides the old hint at once and is not counted as its impression", async () => {
    render(box());
    type("ghbdtn");
    fireEvent.blur(input());
    expect(await screen.findByRole("button", { name: "Fix" })).toBeTruthy();

    type("hello");
    expect(screen.queryByRole("button", { name: "Fix" })).toBeNull();
    expect(layoutCorrectionMetrics()).toEqual({ shown: 1, accepted: 0 });
  });

  test("accepting goes through native insertion, the browser's undo-stack path", async () => {
    const commands: string[] = [];
    document.execCommand = mock((command: string, _ui: boolean, value?: string) => {
      commands.push(command);
      if (command !== "insertText") return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
      setter.call(input(), value);
      input().dispatchEvent(new Event("input", { bubbles: true }));
      return true;
    });
    render(box());
    type("ghbdtn");
    fireEvent.blur(input());
    fireEvent.click(await screen.findByRole("button", { name: "Fix" }));

    await waitFor(() => expect(input().value).toBe("привет"));
    expect(commands).toEqual(["insertText"]);
  });

  test("fallback correction has a one-step Undo with the original selection, whether insertion refuses or throws", async () => {
    for (const throws of [false, true]) {
      document.execCommand = mock(() => {
        if (throws) throw new Error("no native insertion");
        return false;
      });
      const view = render(box());
      type("ghbdtn");
      input().setSelectionRange(2, 4);
      applyKey();
      await waitFor(() => expect(input().value).toBe("привет"));

      fireEvent.keyDown(input(), { code: "KeyZ", key: "z", ...(IS_MAC ? { metaKey: true } : { ctrlKey: true }) });
      await waitFor(() => expect(input().value).toBe("ghbdtn"));
      await waitFor(() => expect([input().selectionStart, input().selectionEnd]).toEqual([2, 4]));
      view.unmount();
    }
  });
});
