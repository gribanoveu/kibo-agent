import { describe, expect, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import {
  EMPTY_HOOK,
  EMPTY_SERVER,
  readHook,
  readMcpServer,
  removeHook,
  removeMcpServer,
  writeHook,
  writeMcpServer,
} from "../lib/configEntries";
import { HookForm, McpServerForm } from "../components/ConfigEntryForm";

// One server or one hook, edited through a form and written back into the
// same file: its order and the keys the form has no field for survive.

const text = (value: unknown) => JSON.stringify(value, null, 2);
const written = (result: { text: string } | { error: string }) => {
  if ("error" in result) throw new Error(result.error);
  return JSON.parse(result.text);
};

const MCP = text({
  mcpServers: {
    github: { command: "npx", args: ["-y", "server-github"], env: { TOKEN: "t" }, weight: 5, disabled: true },
    fs: { command: "fs-server", type: "stdio" },
    docs: { type: "http", url: "https://mcp.example.com/mcp", headers: { Authorization: "Bearer t", "X-Team": "a" } },
  },
  other: 1,
});

describe("an MCP server", () => {
  test("reads into fields: args one per line, env as KEY=VALUE", () => {
    expect(readMcpServer(MCP, "github")).toEqual({
      name: "github",
      transport: "command",
      command: "npx",
      args: "-y\nserver-github",
      env: "TOKEN=t",
      url: "",
      headers: "",
      weight: "5",
      timeoutSecs: "",
      exposure: "direct",
      keepResults: false,
    });
    expect(readMcpServer(MCP, "nope")).toBeNull();
  });

  test("keeping its results is written only when set, and read back", () => {
    const kept = written(writeMcpServer(MCP, null, { ...EMPTY_SERVER, name: "kb", command: "kb", keepResults: true }));
    expect(kept.mcpServers.kb).toEqual({ command: "kb", keepResults: true });
    const file = text(kept);
    expect(readMcpServer(file, "kb")!.keepResults).toBe(true);
    const back = written(writeMcpServer(file, "kb", { ...readMcpServer(file, "kb")!, keepResults: false }));
    expect(back.mcpServers.kb).toEqual({ command: "kb" });
  });

  test("how its tools are reached is written only when not the default, and per-tool entries stay", () => {
    const deferred = written(writeMcpServer(MCP, null, { ...EMPTY_SERVER, name: "big", command: "big", exposure: "deferred" }));
    expect(deferred.mcpServers.big).toEqual({ command: "big", exposure: "deferred" });
    const withTools = text({ mcpServers: { big: { command: "big", exposure: "hidden", toolExposure: { find: "direct" } } } });
    expect(readMcpServer(withTools, "big")!.exposure).toBe("hidden");
    const back = written(writeMcpServer(withTools, "big", { ...readMcpServer(withTools, "big")!, exposure: "direct" }));
    expect(back.mcpServers.big).toEqual({ command: "big", toolExposure: { find: "direct" } });
    expect(readMcpServer(text({ mcpServers: { x: { command: "x", exposure: "codemode" } } }), "x")!.exposure).toBe("direct");
  });

  test("an edit stays in its place and keeps what the form has no field for", () => {
    const fields = { ...readMcpServer(MCP, "github")!, name: "gh", args: "-y\nserver-github\n--ro", weight: "" };
    const file = written(writeMcpServer(MCP, "github", fields));
    expect(Object.keys(file.mcpServers)).toEqual(["gh", "fs", "docs"]);
    expect(file.mcpServers.gh).toEqual({
      command: "npx",
      args: ["-y", "server-github", "--ro"],
      env: { TOKEN: "t" },
      disabled: true,
    });
    expect(file.other).toBe(1);
  });

  test("a new one goes at the end, and a whole command line is split into command and args", () => {
    const file = written(writeMcpServer(MCP, null, { ...EMPTY_SERVER, name: "web", command: "npx -y server-web", timeoutSecs: "30" }));
    expect(Object.keys(file.mcpServers)).toEqual(["github", "fs", "docs", "web"]);
    expect(file.mcpServers.web).toEqual({ command: "npx", args: ["-y", "server-web"], timeoutSecs: 30 });
    const quoted = written(writeMcpServer(MCP, null, { ...EMPTY_SERVER, name: "q", command: '"/My Apps/srv"' }));
    expect(quoted.mcpServers.q).toEqual({ command: '"/My Apps/srv"' });
  });

  test("one at a URL reads into its URL and Name: value header lines", () => {
    expect(readMcpServer(MCP, "docs")).toMatchObject({
      transport: "url",
      url: "https://mcp.example.com/mcp",
      headers: "Authorization: Bearer t\nX-Team: a",
      command: "",
    });
  });

  test("one at a URL is written with type http, and a header value may hold a colon", () => {
    const fields = { ...EMPTY_SERVER, name: "web", transport: "url" as const, url: " https://a.example/mcp ", headers: "X-Key: a:b\n\n" };
    expect(written(writeMcpServer(MCP, null, fields)).mcpServers.web).toEqual({
      type: "http",
      url: "https://a.example/mcp",
      headers: { "X-Key": "a:b" },
    });
    const bare = written(writeMcpServer(MCP, null, { ...fields, headers: "" })).mcpServers.web;
    expect(bare).toEqual({ type: "http", url: "https://a.example/mcp" });
  });

  /** A URL and a command together would not start, and a type left over would say the wrong thing. */
  test("switching how it runs drops what belonged to the other way", () => {
    const toUrl = { ...readMcpServer(MCP, "fs")!, transport: "url" as const, url: "https://b.example/mcp" };
    expect(written(writeMcpServer(MCP, "fs", toUrl)).mcpServers.fs).toEqual({ type: "http", url: "https://b.example/mcp" });
    const toCommand = { ...readMcpServer(MCP, "docs")!, transport: "command" as const, command: "docs-server", weight: "2" };
    expect(written(writeMcpServer(MCP, "docs", toCommand)).mcpServers.docs).toEqual({ command: "docs-server", weight: 2 });
    const staysStdio = { ...readMcpServer(MCP, "fs")!, args: "--ro" };
    expect(written(writeMcpServer(MCP, "fs", staysStdio)).mcpServers.fs).toEqual({ command: "fs-server", args: ["--ro"], type: "stdio" });
    const legacy = text({ mcpServers: { old: { type: "sse", command: "x" } } });
    expect(written(writeMcpServer(legacy, "old", readMcpServer(legacy, "old")!)).mcpServers.old).toEqual({ command: "x" });
  });

  test("says what is wrong rather than writing it", () => {
    const write = (fields: Partial<typeof EMPTY_SERVER>, previous: string | null = null) =>
      writeMcpServer(MCP, previous, { ...EMPTY_SERVER, name: "x", command: "c", ...fields });
    expect(write({ transport: "url", url: " " })).toEqual({ error: "The server needs a URL" });
    expect(write({ transport: "url", url: "https://a", headers: "Bearer t" })).toMatchObject({ error: expect.stringContaining("Name: value") });
    expect(write({ transport: "url", url: "https://a", headers: ": t" })).toMatchObject({ error: expect.stringContaining("Name: value") });
    expect(write({ name: " " })).toEqual({ error: "The server needs a name" });
    expect(write({ name: "fs" })).toEqual({ error: "There is already a server called fs" });
    expect("text" in write({ name: "fs" }, "fs")).toBe(true);
    expect(write({ command: "" })).toEqual({ error: "The server needs a command to start it" });
    expect(write({ env: "TOKEN" })).toMatchObject({ error: expect.stringContaining("KEY=VALUE") });
    expect(write({ env: "=secret" })).toMatchObject({ error: expect.stringContaining("KEY=VALUE") });
    expect(write({ weight: "0" })).toEqual({ error: "Weight must be a whole number of at least 1" });
    expect(write({ timeoutSecs: "1.5" })).toEqual({ error: "Timeout must be a whole number of at least 1" });
    expect(writeMcpServer("{", null, { ...EMPTY_SERVER, name: "x", command: "c" })).toMatchObject({ error: expect.stringContaining("not valid JSON") });
  });

  test("is removed by name, the rest untouched", () => {
    const file = JSON.parse(removeMcpServer(MCP, "github")!);
    expect(Object.keys(file.mcpServers)).toEqual(["fs", "docs"]);
    expect(file.other).toBe(1);
    expect(removeMcpServer(MCP, "nope")).toBeNull();
  });
});

// Rows in the tab's order: events by name, then groups and commands as written.
// Row 0 is PostToolUse's lint, 1 its fmt, 2 PreToolUse's guard, 3 Stop's say.
const HOOKS = text({
  hooks: {
    Stop: [{ hooks: [{ type: "command", command: "say done" }] }],
    PreToolUse: [{ matcher: "runCommand", hooks: [{ type: "command", command: "guard.sh", timeout: 10, note: "kept" }] }],
    PostToolUse: [{ matcher: "editFile", hooks: [{ type: "command", command: "lint" }, { type: "command", command: "fmt" }] }],
  },
});

describe("a hook", () => {
  test("is found by its row in the tab", () => {
    expect(readHook(HOOKS, 1)).toEqual({ event: "PostToolUse", matcher: "editFile", command: "fmt", timeout: "" });
    expect(readHook(HOOKS, 2)).toEqual({ event: "PreToolUse", matcher: "runCommand", command: "guard.sh", timeout: "10" });
    expect(readHook(HOOKS, 3)).toEqual({ event: "Stop", matcher: "", command: "say done", timeout: "" });
    expect(readHook(HOOKS, 4)).toBeNull();
  });

  test("an edit that keeps its event and tools stays where it was, with its other keys", () => {
    const file = written(writeHook(HOOKS, 2, { ...readHook(HOOKS, 2)!, command: "guard2.sh", timeout: "" }));
    expect(file.hooks.PreToolUse).toEqual([{ matcher: "runCommand", hooks: [{ type: "command", command: "guard2.sh", note: "kept" }] }]);
  });

  test("changing its tools moves it to the group with those tools, dropping a group left empty", () => {
    const moved = written(writeHook(HOOKS, 2, { ...readHook(HOOKS, 2)!, event: "PostToolUse", matcher: "editFile" }));
    expect(moved.hooks.PreToolUse).toBeUndefined();
    expect(moved.hooks.PostToolUse[0].hooks.map((h: { command: string }) => h.command)).toEqual(["lint", "fmt", "guard.sh"]);

    const apart = written(writeHook(HOOKS, 1, { ...readHook(HOOKS, 1)!, matcher: "writeFile" }));
    expect(apart.hooks.PostToolUse).toEqual([
      { matcher: "editFile", hooks: [{ type: "command", command: "lint" }] },
      { matcher: "writeFile", hooks: [{ type: "command", command: "fmt" }] },
    ]);
  });

  test("a new one joins the group with its tools, or starts one; Stop has no tools", () => {
    const joined = written(writeHook(HOOKS, null, { ...EMPTY_HOOK, event: "PostToolUse", matcher: " editFile ", command: "test" }));
    expect(joined.hooks.PostToolUse[0].hooks.at(-1)).toEqual({ type: "command", command: "test" });
    const stop = written(writeHook("{}", null, { ...EMPTY_HOOK, event: "Stop", matcher: "ignored", command: "say", timeout: "5" }));
    expect(stop.hooks.Stop).toEqual([{ hooks: [{ type: "command", command: "say", timeout: 5 }] }]);
  });

  test("says what is wrong rather than writing it", () => {
    expect(writeHook(HOOKS, null, { ...EMPTY_HOOK, command: " " })).toEqual({ error: "The hook needs a command to run" });
    expect(writeHook(HOOKS, null, { ...EMPTY_HOOK, command: "x", timeout: "-1" })).toMatchObject({ error: expect.stringContaining("Timeout") });
    expect(writeHook(HOOKS, 9, { ...EMPTY_HOOK, command: "x" })).toMatchObject({ error: expect.stringContaining("no longer in the file") });
  });

  test("is removed by its row, and an event left with nothing goes too", () => {
    const file = JSON.parse(removeHook(HOOKS, 2)!);
    expect(Object.keys(file.hooks).sort()).toEqual(["PostToolUse", "Stop"]);
    const one = JSON.parse(removeHook(HOOKS, 0)!);
    expect(one.hooks.PostToolUse[0].hooks).toEqual([{ type: "command", command: "fmt" }]);
    expect(removeHook(HOOKS, 9)).toBeNull();
  });

  test("a group without commands is kept and does not count as a row", () => {
    const sparse = text({ hooks: { PreToolUse: [{ matcher: "x" }, { hooks: [{ type: "command", command: "a" }] }] } });
    expect(readHook(sparse, 0)?.command).toBe("a");
    expect(JSON.parse(removeHook(sparse, 0)!).hooks.PreToolUse).toEqual([{ matcher: "x" }]);
  });
});

describe("the forms", () => {
  test("a server form saves the file with the server in it and closes", async () => {
    const saved: string[] = [];
    let closed = false;
    render(
      <McpServerForm
        name="fs"
        text={MCP}
        error={null}
        onSave={async (t) => (saved.push(t), true)}
        onClose={() => (closed = true)}
        onEditJson={() => {}}
      />,
    );
    expect((screen.getByLabelText(/^Command/) as HTMLInputElement).value).toBe("fs-server");
    fireEvent.change(screen.getByLabelText(/^Arguments/), { target: { value: "--root\n/tmp" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(JSON.parse(saved[0]).mcpServers.fs).toEqual({ command: "fs-server", args: ["--root", "/tmp"], type: "stdio" });
    expect(closed).toBe(true);
  });

  test("a server form switches to a URL and saves the server that way", async () => {
    const saved: string[] = [];
    render(
      <McpServerForm name="fs" text={MCP} error={null} onSave={async (t) => (saved.push(t), true)} onClose={() => {}} onEditJson={() => {}} />,
    );
    expect(screen.queryByLabelText(/^URL/)).toBeNull();
    fireEvent.click(screen.getByText("A command"));
    fireEvent.click(screen.getByRole("option", { name: /At a URL/ }));
    expect(screen.queryByLabelText(/^Command/)).toBeNull();
    fireEvent.change(screen.getByLabelText(/^URL/), { target: { value: "https://c.example/mcp" } });
    fireEvent.change(screen.getByLabelText(/^Headers/), { target: { value: "Authorization: Bearer x" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(JSON.parse(saved[0]).mcpServers.fs).toEqual({
      type: "http",
      url: "https://c.example/mcp",
      headers: { Authorization: "Bearer x" },
    });
  });

  test("an HTTP server opens in the form as one", () => {
    render(<McpServerForm name="docs" text={MCP} error={null} onSave={async () => true} onClose={() => {}} onEditJson={() => {}} />);
    expect((screen.getByLabelText(/^URL/) as HTMLInputElement).value).toBe("https://mcp.example.com/mcp");
    expect((screen.getByLabelText(/^Headers/) as HTMLTextAreaElement).value).toBe("Authorization: Bearer t\nX-Team: a");
  });

  test("a problem the form can see is shown, and nothing is saved", async () => {
    const saved: string[] = [];
    render(
      <McpServerForm name={null} text={MCP} error={null} onSave={async (t) => (saved.push(t), true)} onClose={() => {}} onEditJson={() => {}} />,
    );
    fireEvent.change(screen.getByLabelText(/^Name/), { target: { value: "fs" } });
    fireEvent.change(screen.getByLabelText(/^Command/), { target: { value: "x" } });
    await act(async () => {
      fireEvent.click(screen.getByText("Save"));
    });
    expect(screen.getByText("There is already a server called fs")).toBeTruthy();
    expect(saved).toEqual([]);
  });

  test("a hook form hides the tools field for Stop, and hands over to the JSON editor", () => {
    let json = 0;
    render(<HookForm index={3} text={HOOKS} error={null} onSave={async () => true} onClose={() => {}} onEditJson={() => json++} />);
    expect(screen.queryByLabelText(/^Tools/)).toBeNull();
    fireEvent.click(screen.getByText("Stop"));
    fireEvent.click(screen.getByRole("option", { name: /PreToolUse/ }));
    expect(screen.getByLabelText(/^Tools/)).toBeTruthy();
    fireEvent.click(screen.getByText("Edit as JSON"));
    expect(json).toBe(1);
  });
});
