import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { ProcessView } from "../lib/chat";
import type { TerminalInfo } from "../lib/terminal";

// Opening another folder stops the background processes, closes the user's
// terminals and would file a running turn's chat in the wrong folder. The
// user is told before any of it.

let processes: ProcessView[] = [];
let terminals: TerminalInfo[] = [];
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) =>
    Promise.resolve(command === "processes_list" ? processes : command === "terminal_list" ? terminals : null),
  transformCallback: (callback: unknown) => callback,
}));
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});
afterEach(() => {
  processes = [];
  terminals = [];
});

const { useFolderSwitch } = await import("../hooks/useFolderSwitch");
const { FolderSwitchDialog } = await import("../components/FolderSwitchDialog");

const proc = (id: number, command: string, state: ProcessView["state"]): ProcessView => ({
  id,
  command,
  cwd: "/work/kibo",
  state,
  tail: "",
});

describe("switching folders", () => {
  test("with nothing running, it just switches", async () => {
    processes = [proc(1, "bun test", { state: "exited", code: 0 })];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(false));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(1);
    expect(result.current.blocked).toBeNull();
  });

  test("with processes running, it asks first and names only the running ones", async () => {
    processes = [proc(1, "bun run dev", { state: "running" }), proc(2, "bun test", { state: "exited", code: 0 })];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(false));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(0);
    const blocked = result.current.blocked;
    expect(blocked?.kind === "processes" && blocked.processes.map((p) => p.command)).toEqual(["bun run dev"]);
  });

  test("an open shell asks too; one whose shell has ended does not", async () => {
    terminals = [{ id: 1, shell: "zsh", state: { state: "exited", code: 0 } }];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(false));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(1);

    terminals.push({ id: 2, shell: "zsh", state: { state: "running" } });
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(1);
    const blocked = result.current.blocked;
    expect(blocked?.kind === "processes" && blocked.terminals.map((t) => t.id)).toEqual([2]);
  });

  /// A message on its way to a new worktree goes back to the box when the switch does not happen.
  test("says it was cancelled when the dialog closes without the switch, and not after the switch", async () => {
    processes = [proc(1, "bun run dev", { state: "running" })];
    let went = 0;
    let cancelled = 0;
    const { result } = renderHook(() => useFolderSwitch(false));
    await act(() => result.current.guard(() => went++, () => cancelled++));
    act(() => result.current.close());
    expect([went, cancelled]).toEqual([0, 1]);

    await act(() => result.current.guard(() => went++, () => cancelled++));
    act(() => {
      const blocked = result.current.blocked;
      if (blocked?.kind === "processes") blocked.go();
      result.current.close();
    });
    expect([went, cancelled]).toEqual([1, 1]);

    const busy = renderHook(() => useFolderSwitch("running"));
    await act(() => busy.result.current.guard(() => went++, () => cancelled++));
    expect([went, cancelled]).toEqual([1, 2]);
  });

  /// The turn's chat is saved into whatever folder is open when it ends.
  test("while the agent works it is refused, whatever runs", async () => {
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch("running"));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(0);
    expect(result.current.blocked).toEqual({ kind: "agent", waiting: false });
  });

  /// A turn paused on an approval card is a turn too: leaving would drop the card.
  test("a turn waiting on a card is refused as well, and said to be waiting", async () => {
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch("waiting"));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(0);
    expect(result.current.blocked).toEqual({ kind: "agent", waiting: true });
  });
});

describe("the dialog", () => {
  test("switches only when the user agrees to stop the processes", () => {
    let went = 0;
    let closed = 0;
    const blocked = {
      kind: "processes" as const,
      processes: [proc(1, "bun run dev", { state: "running" })],
      terminals: [{ id: 3, shell: "zsh", state: { state: "running" as const } }],
      go: () => went++,
    };
    const { unmount } = render(
      <FolderSwitchDialog blocked={blocked} folder="/work/kibo" onStopAgent={() => {}} onClose={() => closed++} />,
    );
    expect(screen.getByText("bun run dev")).toBeTruthy();
    expect(screen.getByText("terminal: zsh")).toBeTruthy();
    fireEvent.click(screen.getByText("Cancel"));
    expect(went).toBe(0);
    fireEvent.click(screen.getByText("Stop them and switch"));
    expect(went).toBe(1);
    expect(closed).toBe(2);
    unmount();
  });

  test("offers to stop the agent, not to switch", () => {
    let stopped = 0;
    render(
      <FolderSwitchDialog blocked={{ kind: "agent", waiting: false }} folder="/work/kibo" onStopAgent={() => stopped++} onClose={() => {}} />,
    );
    expect(screen.getByRole("dialog").textContent).toContain("kibo");
    fireEvent.click(screen.getByText("Stop the agent"));
    expect(stopped).toBe(1);
  });

  /// Stop does not end a paused turn — an answer does — so it is not offered.
  test("a turn waiting on a card is told to be answered, with no Stop", () => {
    render(<FolderSwitchDialog blocked={{ kind: "agent", waiting: true }} folder="/work/kibo" onStopAgent={() => {}} onClose={() => {}} />);
    expect(screen.getByRole("dialog").textContent).toContain("Answer the card");
    expect(screen.queryByText("Stop the agent")).toBeNull();
  });

  test("nothing blocked, nothing shown", () => {
    render(<FolderSwitchDialog blocked={null} folder="/work/kibo" onStopAgent={() => {}} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
