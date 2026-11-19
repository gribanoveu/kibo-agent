import { afterAll, afterEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { ProcessView } from "../lib/processes";
import type { TerminalInfo } from "../lib/terminal";

// Closing a folder stops its background processes and closes its terminals,
// and waits for the turns working in it; the user is told before any of it.
// Opening a folder closes none (F-8.6), and a turn keeps its own folder while
// the window moves (F-8.7), so it asks about nothing that runs.

let processes: ProcessView[] = [];
let terminals: TerminalInfo[] = [];
let open = ["/work/kibo"];
let asked: { command: string; args: Record<string, unknown> }[] = [];
mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    if (command === "processes_list" || command === "terminal_list") asked.push({ command, args: args ?? {} });
    if (command === "workspaces_list") return Promise.resolve(open.map((path, i) => ({ path, active: i === 0 })));
    // Each folder answers with its own: the lists are the one folder's, read per folder.
    const folder = args?.workspace;
    if (command === "processes_list") return Promise.resolve(processes.filter((p) => p.cwd === folder));
    if (command === "terminal_list") return Promise.resolve(folder === "/work/kibo" ? terminals : []);
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});
afterEach(() => {
  processes = [];
  terminals = [];
  open = ["/work/kibo"];
  asked = [];
});

const { turnsIn, useFolderSwitch } = await import("../hooks/useFolderSwitch");
const { FolderSwitchDialog } = await import("../components/FolderSwitchDialog");

const proc = (id: number, command: string, state: ProcessView["state"]): ProcessView => ({
  id,
  command,
  cwd: "/work/kibo",
  state,
  tail: "",
});

const KIBO = ["/work/kibo"];
const none = () => [];
/** A turn working in `folder`, as App's `turnsIn` reports it. */
const turnIn =
  (folder: string, id = "c1", waiting = false) =>
  (asked: string) =>
    asked === folder ? [{ id, waiting }] : [];

describe("switching folders", () => {
  test("opening a folder asks about nothing, whatever runs in the others", async () => {
    open = ["/work/kibo", "/work/other"];
    processes = [proc(1, "bun run dev", { state: "running" })];
    terminals = [{ id: 1, shell: "zsh", state: { state: "running" } }];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(1);
    expect(result.current.blocked).toBeNull();
    expect(asked).toEqual([]);
  });

  test("closing a folder with nothing running just closes it", async () => {
    processes = [proc(1, "bun test", { state: "exited", code: 0 })];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => went++, undefined, KIBO));
    expect(went).toBe(1);
    expect(result.current.blocked).toBeNull();
  });

  test("with processes running, it asks first and names only the running ones", async () => {
    processes = [proc(1, "bun run dev", { state: "running" }), proc(2, "bun test", { state: "exited", code: 0 })];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => went++, undefined, KIBO));
    expect(went).toBe(0);
    const blocked = result.current.blocked;
    expect(blocked?.kind === "processes" && blocked.processes.map((p) => p.command)).toEqual(["bun run dev"]);
    expect(blocked?.kind === "processes" && blocked.folders).toEqual(KIBO);
    // Every chat's of the folder closed, not the open chat's (F-8.3).
    expect(asked).toContainEqual({ command: "processes_list", args: { chatId: null, workspace: "/work/kibo" } });
    expect(asked).toContainEqual({ command: "terminal_list", args: { workspace: "/work/kibo" } });
  });

  test("names what runs in the folder that closes, and not in the others", async () => {
    open = ["/work/kibo-wt", "/work/kibo", "/work/other"];
    processes = [
      { ...proc(1, "bun run dev", { state: "running" }), cwd: "/work/kibo" },
      { ...proc(2, "cargo watch", { state: "running" }), cwd: "/work/other" },
    ];
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => {}, undefined, KIBO));
    const blocked = result.current.blocked;
    expect(blocked?.kind === "processes" && blocked.processes.map((p) => p.command)).toEqual(["bun run dev"]);
  });

  test("an open shell asks too; one whose shell has ended does not", async () => {
    terminals = [{ id: 1, shell: "zsh", state: { state: "exited", code: 0 } }];
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => went++, undefined, KIBO));
    expect(went).toBe(1);

    terminals.push({ id: 2, shell: "zsh", state: { state: "running" } });
    await act(() => result.current.guard(() => went++, undefined, KIBO));
    expect(went).toBe(1);
    const blocked = result.current.blocked;
    expect(blocked?.kind === "processes" && blocked.terminals.map((t) => t.id)).toEqual([2]);
  });

  /// A message on its way to a new worktree goes back to the box when the switch does not happen.
  test("says it was cancelled when the dialog closes without the switch, and not after the switch", async () => {
    processes = [proc(1, "bun run dev", { state: "running" })];
    let went = 0;
    let cancelled = 0;
    const { result } = renderHook(() => useFolderSwitch(none));
    await act(() => result.current.guard(() => went++, () => cancelled++, KIBO));
    act(() => result.current.close());
    expect([went, cancelled]).toEqual([0, 1]);

    await act(() => result.current.guard(() => went++, () => cancelled++, KIBO));
    act(() => {
      const blocked = result.current.blocked;
      if (blocked?.kind === "processes") blocked.go();
      result.current.close();
    });
    expect([went, cancelled]).toEqual([1, 1]);

    const busy = renderHook(() => useFolderSwitch(turnIn("/work/kibo")));
    await act(() => busy.result.current.guard(() => went++, () => cancelled++, KIBO));
    expect([went, cancelled]).toEqual([1, 2]);
  });

  /// F-8.7: the turn keeps its folder, so the window may move while it works.
  test("a working turn does not hold the window: opening another folder goes ahead", async () => {
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(turnIn("/work/kibo")));
    await act(() => result.current.guard(() => went++));
    expect(went).toBe(1);
    expect(result.current.blocked).toBeNull();
  });

  /// A folder cannot close under a turn working in it; another one closes.
  test("a turn blocks closing its own folder, not another one, and names its chats", async () => {
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(turnIn("/work/kibo")));
    await act(() => result.current.guard(() => went++, undefined, ["/work/other"]));
    expect(went).toBe(1);
    expect(result.current.blocked).toBeNull();

    asked = [];
    await act(() => result.current.guard(() => went++, undefined, ["/work/other", "/work/kibo"]));
    expect(went).toBe(1);
    expect(result.current.blocked).toEqual({ kind: "agent", folder: "/work/kibo", chats: ["c1"], waiting: false });
    expect(asked).toEqual([]);
  });

  /// A turn paused on an approval card is a turn too: closing would drop the card.
  test("a turn waiting on a card blocks closing as well, and is said to be waiting", async () => {
    let went = 0;
    const { result } = renderHook(() => useFolderSwitch(turnIn("/work/kibo", "c2", true)));
    await act(() => result.current.guard(() => went++, undefined, KIBO));
    expect(went).toBe(0);
    expect(result.current.blocked).toEqual({ kind: "agent", folder: "/work/kibo", chats: ["c2"], waiting: true });
  });
});

describe("the turns of a folder", () => {
  test("a working or waiting chat counts in the folder it is listed under; a finished one nowhere", () => {
    const folders: Record<string, string> = { a: "/work/kibo", b: "/work/kibo", c: "/work/other", d: "/work/kibo" };
    const of = turnsIn({ a: "running", b: "awaitingApproval", c: "running", d: "finished" }, (id) => folders[id] ?? null);
    expect(of("/work/kibo")).toEqual([
      { id: "a", waiting: false },
      { id: "b", waiting: true },
    ]);
    expect(of("/work/other")).toEqual([{ id: "c", waiting: false }]);
    expect(of("/work/none")).toEqual([]);
  });

  /// Its folder is not known here: Stop for another folder must not reach it.
  test("a chat not listed yet counts in no folder", () => {
    const of = turnsIn({ fresh: "running" }, () => null);
    expect(of("/work/kibo")).toEqual([]);
  });
});

describe("the dialog", () => {
  test("closes only when the user agrees to stop the processes", () => {
    let went = 0;
    let closed = 0;
    const blocked = {
      kind: "processes" as const,
      folders: ["/work/other"],
      processes: [proc(1, "bun run dev", { state: "running" })],
      terminals: [{ id: 3, shell: "zsh", state: { state: "running" as const } }],
      go: () => went++,
    };
    const { unmount } = render(
      <FolderSwitchDialog blocked={blocked} onStopAgent={() => {}} onClose={() => closed++} />,
    );
    expect(screen.getByText("bun run dev")).toBeTruthy();
    expect(screen.getByText("terminal: zsh")).toBeTruthy();
    // The folder closing, not the active one.
    expect(screen.getByText("other")).toBeTruthy();
    fireEvent.click(screen.getByText("Cancel"));
    expect(went).toBe(0);
    fireEvent.click(screen.getByText("Stop them and close"));
    expect(went).toBe(1);
    expect(closed).toBe(2);
    unmount();
  });

  test("offers to stop the turns of the folder that closes", () => {
    const stopped: string[][] = [];
    render(
      <FolderSwitchDialog
        blocked={{ kind: "agent", folder: "/work/kibo", chats: ["c1", "c3"], waiting: false }}
        onStopAgent={(chats) => stopped.push(chats)}
        onClose={() => {}}
      />,
    );
    expect(screen.getByRole("dialog").textContent).toContain("kibo");
    fireEvent.click(screen.getByText("Stop the agent"));
    expect(stopped).toEqual([["c1", "c3"]]);
  });

  /// Stop does not end a paused turn — an answer does — so it is not offered.
  test("a turn waiting on a card is told to be answered, with no Stop", () => {
    render(<FolderSwitchDialog blocked={{ kind: "agent", folder: "/work/kibo", chats: ["c1"], waiting: true }} onStopAgent={() => {}} onClose={() => {}} />);
    expect(screen.getByRole("dialog").textContent).toContain("Answer the card");
    expect(screen.queryByText("Stop the agent")).toBeNull();
  });

  test("nothing blocked, nothing shown", () => {
    render(<FolderSwitchDialog blocked={null} onStopAgent={() => {}} onClose={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
