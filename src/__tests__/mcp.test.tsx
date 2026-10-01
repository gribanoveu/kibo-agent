import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { McpView } from "../lib/chat";

// The MCP file is the user's: the tab re-reads it, a switch is settled by
// what was saved, and a config that does not parse leaves the file alone and
// the editor open with the reason.

let disk: McpView;
let calls: string[] = [];

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: Record<string, unknown>) => {
    calls.push(command);
    if (command === "mcp_config_get") return Promise.resolve(structuredClone(disk));
    if (command === "mcp_config_save") {
      const text = args!.text as string;
      try {
        JSON.parse(text);
      } catch {
        return Promise.reject("the MCP configuration is not valid: EOF");
      }
      disk = { ...disk, text, servers: [{ name: "pasted", command: "npx x", enabled: true, error: null, warning: null, state: { state: "notStarted" } }] };
      return Promise.resolve(structuredClone(disk));
    }
    if (command === "mcp_server_connect") {
      disk.servers = disk.servers.map((s) =>
        s.name === args!.name
          ? { ...s, state: { state: "running", tools: [{ name: "echo", description: "Says it back" }] } }
          : s,
      );
      return Promise.resolve(structuredClone(disk));
    }
    if (command === "mcp_tool_set_shown") {
      disk.servers = disk.servers.map((s) =>
        s.name === args!.server && s.state.state === "running"
          ? { ...s, state: { ...s.state, tools: s.state.tools.map((t) => (t.name === args!.tool ? { ...t, exposure: args!.shown ? "deferred" : "hidden" } : t)) } }
          : s,
      );
      return Promise.resolve(structuredClone(disk));
    }
    if (command === "mcp_server_set_enabled") {
      disk.servers = disk.servers.map((s) => (s.name === args!.name ? { ...s, enabled: args!.enabled as boolean } : s));
      return Promise.resolve(structuredClone(disk));
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useMcp } = await import("../hooks/useMcp");
const { McpList } = await import("../components/panes");
const { ConfigFileEditor } = await import("../components/ConfigFileEditor");
const McpConfig = (props: { view: McpView; error: string | null; onSave: (text: string) => Promise<boolean>; onClose: () => void }) => (
  <ConfigFileEditor label="MCP configuration" text={props.view.text} note="" error={props.error} onSave={props.onSave} onClose={props.onClose} />
);
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

beforeEach(() => {
  disk = {
    path: "/home/.kibo/mcp.json",
    text: '{\n  "mcpServers": {}\n}\n',
    servers: [
      { name: "github", command: "npx -y server-github", enabled: true, error: null, warning: null, state: { state: "running", tools: [
        { name: "create_issue", description: "Opens an issue" },
        { name: "search_code", description: "Searches code" },
        { name: "get_file", description: "Reads a file" },
      ] } },
      { name: "idle", command: "npx -y server-idle", enabled: true, error: null, warning: null, state: { state: "notStarted" } },
      { name: "broken", command: "", enabled: true, error: "no command to start it with", warning: null, state: { state: "notStarted" } },
    ],
  };
  calls = [];
});

describe("useMcp", () => {
  test("reads the file when shown, and not while hidden", async () => {
    const { result, rerender } = renderHook(({ visible }) => useMcp(visible), { initialProps: { visible: false } });
    await settle();
    expect(calls).toEqual([]);
    rerender({ visible: true });
    await settle();
    expect(result.current.view?.servers.map((s) => s.name)).toEqual(["github", "idle", "broken"]);
  });

  test("a switch is what the backend saved", async () => {
    const { result } = renderHook(() => useMcp(true));
    await settle();
    await act(() => result.current.setEnabled("github", false));
    expect(result.current.view?.servers[0].enabled).toBe(false);
    expect(calls).toContain("mcp_server_set_enabled");
  });

  test("a tool's switch shows what the backend decided, not a guess", async () => {
    const { result } = renderHook(() => useMcp(true));
    await settle();
    await act(() => result.current.setToolShown("github", "search_code", true));
    const github = result.current.view!.servers[0].state;
    expect(github.state === "running" && github.tools[1].exposure).toBe("deferred");
    expect(calls).toContain("mcp_tool_set_shown");
  });

  test("opening a server's row starts it and its tools arrive", async () => {
    const { result } = renderHook(() => useMcp(true));
    await settle();
    await act(() => result.current.connect("idle"));
    expect(calls).toContain("mcp_server_connect");
    expect(result.current.view?.servers[1].state).toEqual({
      state: "running",
      tools: [{ name: "echo", description: "Says it back" }],
    });
  });

  test("a row that is already running keeps its tools while it answers", async () => {
    const { result } = renderHook(() => useMcp(true));
    await settle();
    const before = structuredClone(disk.servers[0].state);
    let answered: Promise<void> = Promise.resolve();
    act(() => {
      answered = result.current.connect("github");
    });
    expect(result.current.view?.servers[0].state).toEqual(before, "not blanked to \"starting\"");
    await act(() => answered);
  });

  test("a turn starting or ending re-reads what the servers are doing", async () => {
    const { rerender } = renderHook(({ status }) => useMcp(true, status), { initialProps: { status: "idle" } });
    await settle();
    rerender({ status: "running" });
    await settle();
    rerender({ status: "idle" });
    await settle();
    expect(calls.filter((c) => c === "mcp_config_get")).toHaveLength(3);
  });

  test("a refused save says why and reports it was not stored", async () => {
    const { result } = renderHook(() => useMcp(true));
    await settle();
    let stored = true;
    await act(async () => {
      stored = await result.current.save("{");
    });
    expect(stored).toBe(false);
    expect(result.current.error).toContain("not valid");
  });
});

describe("the MCP tab", () => {
  let actions: string[] = [];
  beforeEach(() => {
    actions = [];
  });
  const panel = (
    view: McpView,
    onAdd = () => {},
    onToggle = (_: string, __: boolean) => {},
    onOpen = (_: string) => {},
  ) =>
    render(
      <McpList
        view={view}
        error={null}
        onToggle={onToggle}
        onToggleTool={(server, tool, shown) => actions.push(`${shown ? "show" : "hide"} ${server}.${tool}`)}
        onOpen={onOpen}
        onAdd={onAdd}
        onEditServer={(name) => actions.push(`edit ${name}`)}
        onRemoveServer={(name) => actions.push(`remove ${name}`)}
        onEditFile={() => actions.push("file")}
      />,
    );

  test("lists the servers, and one that cannot start says why and has no switch", () => {
    const toggled: [string, boolean][] = [];
    panel(disk, () => {}, (name, on) => toggled.push([name, on]));

    expect(screen.getByText("npx -y server-github")).toBeTruthy();
    expect(screen.getByText("won't start")).toBeTruthy();
    expect(screen.getByText("no command to start it with")).toBeTruthy();
    const switches = screen.getAllByRole("button", { pressed: true });
    expect(switches).toHaveLength(2, "the two runnable servers; the broken one has no switch");
    fireEvent.click(switches[0]);
    expect(toggled).toEqual([["github", false]]);
  });

  test("a switched-on server says what its process is doing", () => {
    const server = disk.servers[0];
    panel({
      ...disk,
      servers: [
        server,
        { ...server, name: "one", state: { state: "running", tools: [{ name: "echo", description: "Says it back" }] } },
        { ...server, name: "idle", state: { state: "notStarted" } },
        { ...server, name: "boot", state: { state: "starting" } },
        { ...server, name: "gone", state: { state: "exited", error: "exited with code 1" } },
        { ...server, name: "bad", state: { state: "failed", error: "npx: not found" } },
        { ...server, name: "off", enabled: false, state: { state: "failed", error: "stale" } },
      ],
    });
    expect(screen.getByText("3 tools")).toBeTruthy();
    expect(screen.getByText("1 tool")).toBeTruthy();
    expect(screen.getByText("Open this row to start it and see its tools")).toBeTruthy();
    expect(screen.getByText("starting")).toBeTruthy();
    expect(screen.getByText("exited")).toBeTruthy();
    expect(screen.getByText("Restarts with the next call")).toBeTruthy();
    expect(screen.getByText("failed")).toBeTruthy();
    expect(screen.getByText("Switch off and on to try again")).toBeTruthy();
    expect(screen.queryByText("stale")).toBeNull();
  });

  test("an entry's warning shows in the open row, and the process's own trouble wins over it", () => {
    const server = disk.servers[1];
    const warning = "box.lan is reached over plain http";
    panel({
      ...disk,
      servers: [
        { ...server, name: "lan", command: "http://box.lan/mcp", warning },
        { ...server, name: "bad", command: "http://bad.lan/mcp", warning, state: { state: "failed", error: "refused" } },
      ],
    });
    fireEvent.click(screen.getByText("http://box.lan/mcp"));
    expect(screen.getByText(warning)).toBeTruthy();
    fireEvent.click(screen.getByText("http://bad.lan/mcp"));
    expect(screen.getByText("refused")).toBeTruthy();
    expect(screen.getAllByText(warning)).toHaveLength(1);
  });

  test("a running server's row shows what it says about its tools and what it tells the model", () => {
    const server = disk.servers[1];
    const warning = "box.lan is reached over plain http";
    panel({
      ...disk,
      servers: [
        {
          ...server,
          name: "tracker",
          command: "http://box.lan/mcp",
          warning,
          state: {
            state: "running",
            instructions: "Find before you purge.",
            tools: [
              { name: "find", description: "Finds issues", title: "Find issues", readOnly: true, destructive: false },
              { name: "purge", description: "Deletes them", title: null, readOnly: false, destructive: true },
              { name: "edit", description: "Edits one", readOnly: false, destructive: false },
            ],
          },
        },
        { ...server, name: "quiet", command: "http://quiet.lan/mcp", warning, state: { state: "running", tools: [], instructions: null } },
      ],
    });
    fireEvent.click(screen.getByText("http://box.lan/mcp"));
    expect(screen.getByText("find · read-only")).toBeTruthy();
    expect(screen.getByText("Find issues — Finds issues")).toBeTruthy();
    expect(screen.getByText("purge · destructive")).toBeTruthy();
    expect(screen.getByText("Deletes them")).toBeTruthy();
    expect(screen.getByText("edit")).toBeTruthy();
    const note = screen.getByText(/It tells the model: Find before you purge\./);
    expect(note.textContent).toContain(warning, "the entry's warning is not lost to the instructions");

    fireEvent.click(screen.getByText("http://quiet.lan/mcp"));
    expect(screen.getAllByText(/It tells the model/)).toHaveLength(1, "a server that said nothing has nothing quoted");
    expect(screen.getAllByText(new RegExp(warning))).toHaveLength(2);
  });

  test("each tool has its own switch, a hidden one drawn off and counted, a deferred one marked", () => {
    const github = disk.servers[0];
    if (github.state.state !== "running") throw new Error("fixture");
    const tools = github.state.tools;
    panel({
      ...disk,
      servers: [{ ...github, state: { ...github.state, tools: [{ ...tools[0], exposure: "hidden" }, { ...tools[1], exposure: "deferred" }, tools[2]] } }],
    });
    expect(screen.getByText("3 tools · 1 hidden")).toBeTruthy();
    fireEvent.click(screen.getByText("npx -y server-github"));
    expect(screen.getByText("search_code · via search")).toBeTruthy();
    const hidden = screen.getByLabelText("create_issue: hidden from the model");
    expect(hidden.getAttribute("aria-pressed")).toBe("false");
    expect(hidden.closest(".tool-row")!.className).toContain("off");
    fireEvent.click(hidden);
    fireEvent.click(screen.getByLabelText("search_code: offered to the model"));
    expect(actions).toEqual(["show github.create_issue", "hide github.search_code"], "by the server's own name, not the marked one");
  });

  test("opening a server that has not started asks for it, and only on the way open", () => {
    const opened: string[] = [];
    panel(disk, () => {}, () => {}, (name) => opened.push(name));
    const row = screen.getByText("npx -y server-idle");
    fireEvent.click(row);
    expect(opened).toEqual(["idle"]);
    fireEvent.click(row);
    expect(opened).toEqual(["idle"], "closing it asks for nothing");
  });

  test("expanding a running server lists the tools it offers", () => {
    panel(disk);
    expect(screen.queryByText("create_issue")).toBeNull();
    fireEvent.click(screen.getByText("npx -y server-github"));
    expect(screen.getByText("create_issue").className).toBe("tname");
    expect(screen.getByText("Searches code").className).toBe("tdesc");
  });

  test("the button adds a server, and the heading opens the whole file", () => {
    let adds = 0;
    panel({ ...disk, servers: [] }, () => adds++);
    fireEvent.click(screen.getByText("Add MCP server"));
    expect(adds).toBe(1);
    fireEvent.click(screen.getByText("Edit JSON"));
    expect(actions).toEqual(["file"]);
  });

  test("an open row edits its server, and removes it only once confirmed", () => {
    panel(disk);
    fireEvent.click(screen.getByText("npx -y server-github"));
    fireEvent.click(screen.getByText("Edit"));
    expect(actions).toEqual(["edit github"]);

    fireEvent.click(screen.getByText("Remove"));
    expect(actions).toEqual(["edit github"], "the first click only asks");
    expect(screen.getByText("Remove github from the file?")).toBeTruthy();
    fireEvent.click(screen.getByText("Keep"));
    expect(screen.queryByText("Remove github from the file?")).toBeNull();

    fireEvent.click(screen.getByText("Remove"));
    fireEvent.click(screen.getAllByText("Remove").at(-1)!);
    expect(actions).toEqual(["edit github", "remove github"]);
  });
});

describe("the MCP editor", () => {
  test("a pasted config is saved and the editor closes", async () => {
    const saved: string[] = [];
    let closed = false;
    render(
      <McpConfig
        view={disk}
        error={null}
        onSave={async (text) => (saved.push(text), true)}
        onClose={() => (closed = true)}
      />,
    );
    const box = screen.getByLabelText("MCP configuration") as HTMLTextAreaElement;
    expect(box.value).toBe(disk.text);

    fireEvent.change(box, { target: { value: '{"mcpServers":{"x":{"command":"y"}}}' } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(saved).toEqual(['{"mcpServers":{"x":{"command":"y"}}}']);
    expect(closed).toBe(true);
  });

  test("a refused save keeps the editor open with what was typed", async () => {
    let closed = false;
    const { rerender } = render(
      <McpConfig view={disk} error={null} onSave={async () => false} onClose={() => (closed = true)} />,
    );
    const box = screen.getByLabelText("MCP configuration") as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "{" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    rerender(<McpConfig view={disk} error="not valid: EOF" onSave={async () => false} onClose={() => (closed = true)} />);

    expect(closed).toBe(false);
    expect(box.value).toBe("{");
    expect(screen.getByText("not valid: EOF")).toBeTruthy();
  });
});
