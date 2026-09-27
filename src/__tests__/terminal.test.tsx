import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { TerminalInfo } from "../lib/terminal";

// The Terminal tab: the user's own shells, a tab each, and the agent's
// background processes as the last tab. xterm.js itself is not drawn here —
// happy-dom has no layout to measure — so the screen hook is replaced by a
// record of which terminal it was asked to draw.

let listed: TerminalInfo[];
let calls: { command: string; args?: Record<string, unknown> }[] = [];
let nextId = 10;

class FakeChannel {
  static made: FakeChannel[] = [];
  id = 100 + FakeChannel.made.length;
  onmessage: (data: ArrayBuffer) => void = () => {};
  constructor() {
    FakeChannel.made.push(this);
  }
}

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push({ command, args });
    if (command === "terminal_list") return Promise.resolve(structuredClone(listed));
    if (command === "processes_list") return Promise.resolve([]);
    if (command === "agents_list") return Promise.resolve([]);
    if (command === "terminal_open") {
      const opened: TerminalInfo = { id: nextId++, shell: "zsh", state: { state: "running" } };
      listed.push(opened);
      return Promise.resolve(structuredClone(opened));
    }
    if (command === "terminal_close") {
      listed = listed.filter((t) => t.id !== args!.id);
      return Promise.resolve(null);
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
  Channel: FakeChannel,
}));

/** Who is listening for `terminals:changed`, to say it to them. */
const listeners = new Set<(message: { payload: unknown }) => void>();
const changed = (id: number) => listeners.forEach((handler) => handler({ payload: { id } }));
mock.module("@tauri-apps/api/event", () => ({
  listen: (channel: string, handler: (message: { payload: unknown }) => void) => {
    if (channel === "terminals:changed") listeners.add(handler);
    return Promise.resolve(() => listeners.delete(handler));
  },
}));

const drawn: number[] = [];
/** The drawn screen's selection callback, to select as a user would. */
let select: (text: string) => void = () => {};
let cleared = 0;
mock.module("../hooks/useTerminalScreen", () => ({
  useTerminalScreen: (id: number, _container: unknown, onSelection: (text: string) => void) => {
    if (drawn.at(-1) !== id) drawn.push(id);
    select = onSelection;
    return { clearSelection: () => cleared++ };
  },
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { terminalAttach, terminalQuote, terminalTitle } = await import("../lib/terminal");
const { useTerminals } = await import("../hooks/useTerminals");
const { TerminalPanel } = await import("../components/TerminalPanel");
const { pasteAtPrompt } = await import("../lib/pasteAtPrompt");
const { Terminal } = await import("@xterm/xterm");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));
const asked = (command: string) => calls.filter((c) => c.command === command);

beforeEach(() => {
  listed = [];
  calls = [];
  drawn.length = 0;
  FakeChannel.made = [];
  nextId = 10;
  cleared = 0;
});

describe("terminalAttach", () => {
  test("bytes arrive down the channel until it is let go, and only that channel is let go", async () => {
    const got: number[][] = [];
    const detach = await terminalAttach(3, (bytes) => got.push([...bytes]));
    const channel = FakeChannel.made[0];
    expect(asked("terminal_attach")[0].args).toEqual({ id: 3, onOutput: channel });

    channel.onmessage(new Uint8Array([27, 91, 208]).buffer);
    expect(got).toEqual([[27, 91, 208]]);

    detach();
    expect(asked("terminal_detach")[0].args).toEqual({ id: 3, channel: channel.id });
    channel.onmessage(new Uint8Array([1]).buffer);
    expect(got).toHaveLength(1);
  });

  test("a tab names the shell, and how it ended", () => {
    expect(terminalTitle({ id: 1, shell: "zsh", state: { state: "running" } })).toBe("zsh");
    expect(terminalTitle({ id: 1, shell: "zsh", state: { state: "exited", code: 3 } })).toBe("zsh — exited 3");
    expect(terminalTitle({ id: 1, shell: "zsh", state: { state: "exited", code: null } })).toBe("zsh — ended");
  });

  test("a selection is quoted with its terminal, in a fence nothing inside can close", () => {
    const zsh = { id: 2, shell: "zsh", state: { state: "running" as const } };
    expect(terminalQuote(zsh, "$ npm test\n1 failed\n\n")).toBe("From my terminal (zsh #2):\n```\n$ npm test\n1 failed\n```");
    expect(terminalQuote(zsh, "see ```js``` and ````")).toBe("From my terminal (zsh #2):\n`````\nsee ```js``` and ````\n`````");
  });
});

describe("useTerminals", () => {
  test("reads when the tab opens and on each change, not before and not after", async () => {
    listed = [{ id: 1, shell: "zsh", state: { state: "running" } }];
    const { result, rerender } = renderHook(({ visible }) => useTerminals(visible), { initialProps: { visible: false } });
    await settle();
    expect(calls).toEqual([]);

    rerender({ visible: true });
    await settle();
    expect(result.current.terminals.map((t) => t.id)).toEqual([1]);
    expect(result.current.loaded).toBe(true);

    listed[0].state = { state: "exited", code: 0 };
    await act(async () => changed(1));
    await settle();
    expect(result.current.terminals[0].state).toEqual({ state: "exited", code: 0 });

    rerender({ visible: false });
    await act(async () => changed(1));
    expect(asked("terminal_list")).toHaveLength(2);
  });

  test("opens one in the list and closes it out of it", async () => {
    const { result } = renderHook(() => useTerminals(true));
    await settle();
    await act(async () => void (await result.current.open()));
    expect(result.current.terminals.map((t) => t.id)).toEqual([10]);
    await act(() => result.current.close(10));
    expect(asked("terminal_close")[0].args).toEqual({ id: 10 });
    expect(result.current.terminals).toEqual([]);
  });
});

describe("TerminalPanel", () => {
  const panel = (props: Partial<Parameters<typeof TerminalPanel>[0]> = {}) => (
    <TerminalPanel active workspace="/work/kibo" processFocus={null} terminalPaste={null} onTerminalPasted={() => {}} onAddToChat={() => {}} {...props} />
  );

  test("opening the tab with no shell gives one, and draws it", async () => {
    render(panel());
    await settle();
    await settle();
    expect(asked("terminal_open")).toHaveLength(1);
    expect(screen.getByRole("tab", { name: "zsh" }).getAttribute("aria-selected")).toBe("true");
    expect(drawn).toEqual([10]);
  });

  test("not again once the last one is closed: the processes show instead", async () => {
    render(panel());
    await settle();
    await settle();
    fireEvent.click(screen.getByLabelText("Close zsh"));
    await settle();
    expect(asked("terminal_open")).toHaveLength(1);
    expect(screen.getByRole("tab", { name: "Processes" }).getAttribute("aria-selected")).toBe("true");
  });

  test("the helper agents have a tab of their own beside the processes", async () => {
    render(panel());
    await settle();
    await settle();
    fireEvent.click(screen.getByRole("tab", { name: "Agents" }));
    expect(screen.getByRole("tab", { name: "Agents" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText(/No helper agents yet/)).toBeTruthy();
    fireEvent.click(screen.getByRole("tab", { name: "Processes" }));
    expect(screen.getByText(/No background processes/)).toBeTruthy();
  });

  test("a helper run asked for from the chat shows the Agents tab", async () => {
    const { rerender } = render(panel());
    await settle();
    await settle();
    rerender(panel({ agentFocus: { id: 1, task: "t" } }));
    expect(screen.getByRole("tab", { name: "Agents" }).getAttribute("aria-selected")).toBe("true");
  });

  test("with no folder open there is nothing to open one in", async () => {
    render(panel({ workspace: null }));
    await settle();
    expect(asked("terminal_open")).toEqual([]);
    expect((screen.getByLabelText("New terminal") as HTMLButtonElement).disabled).toBe(true);
  });

  test("shells already open are not added to; the newest is drawn, another on a click", async () => {
    listed = [
      { id: 1, shell: "zsh", state: { state: "exited", code: 3 } },
      { id: 2, shell: "bash", state: { state: "running" } },
    ];
    render(panel());
    await settle();
    await settle();
    expect(asked("terminal_open")).toEqual([]);
    expect(drawn).toEqual([2]);
    fireEvent.click(screen.getByRole("tab", { name: "zsh — exited 3" }));
    expect(drawn).toEqual([2, 1]);
    fireEvent.click(screen.getByLabelText("New terminal"));
    await settle();
    expect(drawn).toEqual([2, 1, 10]);
  });

  test("a selection offers to go into the message, quoted with its terminal", async () => {
    listed = [{ id: 1, shell: "zsh", state: { state: "running" } }];
    const added: string[] = [];
    render(panel({ onAddToChat: (text) => added.push(text) }));
    await settle();
    expect(screen.queryByText("Add to chat")).toBeNull();
    act(() => select("  "));
    expect(screen.queryByText("Add to chat")).toBeNull();
    act(() => select("1 failed"));
    fireEvent.click(screen.getByText("Add to chat"));
    expect(added).toEqual([terminalQuote(listed[0], "1 failed")]);
    expect(cleared).toBe(1);
    expect(screen.queryByText("Add to chat")).toBeNull();
  });

  test("a process asked for from the chat shows the processes", async () => {
    listed = [{ id: 1, shell: "zsh", state: { state: "running" } }];
    const { rerender } = render(panel());
    await settle();
    rerender(panel({ processFocus: { id: 4 } }));
    expect(screen.getByRole("tab", { name: "Processes" }).getAttribute("aria-selected")).toBe("true");
  });

  /** What a screen of terminal `id` pastes at its prompt — the screen hook is
   * mocked here, so a real xterm.js stands in for it. */
  const pastedAt = async (id: number) => {
    const term = new Terminal();
    term.open(document.body.appendChild(document.createElement("div")));
    let sent = "";
    term.onData((data) => (sent += data));
    const stop = pasteAtPrompt(term, id);
    await new Promise<void>((resolve) => term.write("\x1b[?2004h% ", resolve));
    stop();
    term.dispose();
    return sent;
  };
  const bracketed = (text: string) => `\x1b[200~${text}\x1b[201~`;

  test("a command from the chat is pasted, not run, in the running shell on screen, once", async () => {
    listed = [
      { id: 1, shell: "zsh", state: { state: "running" } },
      { id: 2, shell: "bash", state: { state: "exited", code: 0 } },
    ];
    const ask = { command: "bun test\nbun run tsc --noEmit" };
    let taken = 0;
    const { rerender } = render(panel());
    await settle();
    rerender(panel({ terminalPaste: ask, onTerminalPasted: () => taken++ }));
    await settle();
    expect(taken).toBe(1);
    expect(drawn.at(-1)).toBe(1);
    expect(asked("terminal_open")).toEqual([]);
    expect(asked("terminal_write")).toEqual([]);
    // Drawn again before App has let go of it: not queued a second time.
    rerender(panel({ terminalPaste: ask, onTerminalPasted: () => taken++ }));
    await settle();
    expect(taken).toBe(1);
    expect(await pastedAt(1)).toBe(bracketed("bun test\rbun run tsc --noEmit"));
    expect(await pastedAt(1)).toBe("");
  });

  test("of two running shells, the one picked on screen takes the command", async () => {
    listed = [
      { id: 1, shell: "zsh", state: { state: "running" } },
      { id: 2, shell: "bash", state: { state: "running" } },
    ];
    const { rerender } = render(panel());
    await settle();
    fireEvent.click(screen.getByRole("tab", { name: "zsh" }));
    rerender(panel({ terminalPaste: { command: "ls" } }));
    await settle();
    expect(await pastedAt(2)).toBe("");
    expect(await pastedAt(1)).toBe(bracketed("ls"));
  });

  test("with no shell a command gets a new one, only one, and is pasted there", async () => {
    render(panel({ terminalPaste: { command: "ls" } }));
    await settle();
    await settle();
    expect(asked("terminal_open")).toHaveLength(1);
    expect(drawn).toEqual([10]);
    expect(await pastedAt(10)).toBe(bracketed("ls"));
  });

  test("with only ended shells a command gets a new one", async () => {
    listed = [{ id: 1, shell: "zsh", state: { state: "exited", code: 0 } }];
    render(panel({ terminalPaste: { command: "ls" } }));
    await settle();
    await settle();
    expect(asked("terminal_open")).toHaveLength(1);
    expect(await pastedAt(1)).toBe("");
    expect(await pastedAt(10)).toBe(bracketed("ls"));
  });

});
