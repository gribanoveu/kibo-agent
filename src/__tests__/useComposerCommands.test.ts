import { afterAll, describe, expect, test, mock } from "bun:test";
import { renderHook } from "@testing-library/react";
import { expandTemplate } from "../lib/slashCommands";
import initPrompt from "../prompts/init.md?raw";
import reviewPrompt from "../prompts/review.md?raw";

// Module mocks are global to the run: snapshot the values (the namespace is
// live and the mock patches its bindings in place) and give them back after.
const realMcp = { ...(await import("../lib/mcp")) };
const written = { text: "Write the issue.", fail: null as string | null };
const writeCalls: [string, string, string, string | null][] = [];
mock.module("../lib/mcp", () => ({
  ...realMcp,
  mcpPromptGet: async (server: string, name: string, args: string, folder: string | null) => {
    writeCalls.push([server, name, args, folder]);
    if (written.fail) throw new Error(written.fail);
    return written.text;
  },
}));
afterAll(() => mock.module("../lib/mcp", () => realMcp));

const { useComposerCommands } = await import("../hooks/useComposerCommands");

type Over = Partial<{
  status: string;
  started: boolean;
  folder: string | null;
  agentMode: boolean;
  files: { name: string; description: string; template: string; arguments?: string[]; argumentHint?: string }[];
  prompts: { server: string; name: string; title: string | null; description: string; arguments: { name: string; required: boolean; description: string }[] }[];
}>;

function setup(over: Over = {}) {
  const calls: string[] = [];
  const said: string[] = [];
  const sent: [string, string | undefined][] = [];
  const agent = {
    turn: { status: over.status ?? "idle", blocks: over.started === false ? [] : [{ kind: "user" }] },
    branch: () => calls.push("branch"),
  };
  const view = renderHook(() =>
    useComposerCommands({
      agent: agent as never,
      folder: over.folder === undefined ? "/repo" : over.folder,
      agentMode: over.agentMode ?? true,
      compact: () => calls.push("compact"),
      send: (text, command) => {
        sent.push([text, command]);
      },
      trust: () => calls.push("trust"),
      commandFiles: { files: over.files ?? [] } as never,
      serverPrompts: { prompts: over.prompts ?? [] } as never,
      notify: (msg) => said.push(msg),
    }),
  );
  const byName = (name: string) => view.result.current.find((c) => c.name === name);
  return { ...view, calls, said, sent, byName };
}

const prompt = (server: string, name: string) => ({ server, name, title: null, description: "", arguments: [] });

describe("the composer's commands", () => {
  test("the app's own are all there, each saying why it cannot run now", () => {
    const idle = setup({ started: true });
    expect(idle.result.current.map((c) => c.name)).toEqual(["compact", "fork", "review", "init", "trust"]);
    expect(idle.result.current.every((c) => !c.unavailable)).toBe(true);
    const busy = setup({ status: "running" });
    expect(busy.byName("compact")?.unavailable).toBe("Not while a turn is running");
    expect(busy.byName("review")?.unavailable).toBe("Not while a turn is running");
    const fresh = setup({ started: false });
    expect(fresh.byName("fork")?.unavailable).toBe("Nothing to fork yet");
    const noFolder = setup({ folder: null });
    expect(noFolder.byName("review")?.unavailable).toBe("Open a folder first");
    expect(noFolder.byName("trust")?.unavailable).toBe("Open a folder first");
    const analyst = setup({ agentMode: false });
    expect(analyst.byName("init")?.unavailable).toBe("Switch to Agent mode — it writes files");
  });

  test("compact, fork and trust run their own action", () => {
    const { byName, calls } = setup();
    byName("compact")!.run("");
    byName("fork")!.run("");
    byName("trust")!.run("");
    expect(calls).toEqual(["compact", "branch", "trust"]);
  });

  test("review and init send the command typed and their prompt expanded", () => {
    const { byName, sent } = setup();
    byName("review")!.run("main");
    byName("init")!.run("the IPC layer");
    expect(sent).toEqual([
      ["/review main", expandTemplate(reviewPrompt, "main")],
      ["/init the IPC layer", expandTemplate(initPrompt, "the IPC layer")],
    ]);
  });

  test("a command file is added and sends its prompt; a built-in's name stays the app's", () => {
    const { byName, sent, result } = setup({
      files: [
        { name: "deploy", description: "Ship it", template: "Ship $ARGUMENTS" },
        { name: "compact", description: "Not yours", template: "Hijack" },
      ],
    });
    expect(result.current.filter((c) => c.name === "compact")).toHaveLength(1);
    byName("deploy")!.run("v1");
    expect(sent).toEqual([["/deploy v1", "Ship v1"]]);
  });

  test("an MCP prompt is named for its server, writes through the folder, and a refusal is said", async () => {
    writeCalls.length = 0;
    const { byName, sent, said } = setup({ folder: "/repo", prompts: [prompt("GitHub", "issue")] });
    expect(byName("github:issue")?.hint).toContain("from GitHub");
    byName("github:issue")!.run("42");
    await Promise.resolve();
    expect(writeCalls).toEqual([["GitHub", "issue", "42", "/repo"]]);
    expect(sent).toEqual([["/github:issue 42", "Write the issue."]]);
    written.fail = "boom";
    const failing = setup({ prompts: [prompt("GitHub", "issue")] });
    failing.byName("github:issue")!.run("42");
    await Promise.resolve();
    await Promise.resolve();
    expect(failing.said).toEqual(["GitHub could not write /github:issue: Error: boom"]);
    written.fail = null;
    // A name a file already has stays the file's.
    const taken = setup({ files: [{ name: "github:issue", description: "Mine", template: "x" }], prompts: [prompt("GitHub", "issue")] });
    expect(taken.result.current.filter((c) => c.name === "github:issue")).toHaveLength(1);
  });
});