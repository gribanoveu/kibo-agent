import { describe, expect, test } from "bun:test";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { jsonError, mergeHooks, mergeMcp } from "../lib/configSnippets";
import { ConfigFileEditor } from "../components/ConfigFileEditor";
import { IS_MAC } from "../lib/shortcuts";

const MOD = IS_MAC ? { metaKey: true } : { ctrlKey: true };

// The MCP and hooks files are JSON edited in one box: a snippet pasted from a
// README joins the file, and broken JSON says where it broke before Save.

const EMPTY_MCP = '{\n  "mcpServers": {}\n}\n';
const GITHUB = { command: "npx", args: ["-y", "server-github"] };
const servers = (text: string) => JSON.parse(text).mcpServers;

describe("jsonError", () => {
  test("valid JSON has none", () => {
    expect(jsonError('{"a": [1, true, null, "x"]}')).toBeNull();
  });

  test("says the line and column where it broke", () => {
    expect(jsonError('{\n  "a": 1\n  "b": 2\n}')).toEqual({ message: 'Expected "," or "}"', line: 3, column: 3 });
    expect(jsonError('{\n  "a": 1,\n}')).toMatchObject({ line: 3, column: 1, message: expect.stringContaining("comma") });
    expect(jsonError('{"a": "open\n}')).toMatchObject({ line: 1, message: expect.stringContaining("not closed") });
    expect(jsonError('{"a": 1')).toMatchObject({ line: 1, column: 8 });
    expect(jsonError("{a: 1}")).toMatchObject({ column: 2, message: "Expected a key in double quotes" });
  });
});

describe("a pasted MCP snippet", () => {
  test("joins the file in every spelling READMEs print", () => {
    const whole = mergeMcp(EMPTY_MCP, JSON.stringify({ mcpServers: { github: GITHUB } }));
    const bare = mergeMcp(EMPTY_MCP, JSON.stringify({ github: GITHUB }));
    const cut = mergeMcp(EMPTY_MCP, `"github": ${JSON.stringify(GITHUB)},`);
    for (const merged of [whole, bare, cut]) {
      expect(servers(merged!.text)).toEqual({ github: GITHUB });
      expect(merged!.message).toBe("Added github");
    }
  });

  test("keeps the servers already there and says which one it replaced", () => {
    const file = JSON.stringify({ mcpServers: { github: { command: "old" }, fs: { command: "fs" } }, other: 1 });
    const merged = mergeMcp(file, JSON.stringify({ github: GITHUB, web: { url: "https://x" } }))!;
    expect(servers(merged.text)).toEqual({ github: GITHUB, fs: { command: "fs" }, web: { url: "https://x" } });
    expect(JSON.parse(merged.text).other).toBe(1);
    expect(merged.message).toBe("Added github, web — replaced the github already here");
  });

  test("is not a snippet when it is not one, or when the file is broken", () => {
    expect(mergeMcp(EMPTY_MCP, "npx -y server-github")).toBeNull();
    expect(mergeMcp(EMPTY_MCP, '{"name": "github"}')).toBeNull();
    expect(mergeMcp(EMPTY_MCP, '{"mcpServers": {}}')).toBeNull();
    expect(mergeMcp('{"mcpServers": {', JSON.stringify({ github: GITHUB }))).toBeNull();
    expect(servers(mergeMcp("", JSON.stringify({ github: GITHUB }))!.text)).toEqual({ github: GITHUB });
  });
});

describe("a pasted hooks snippet", () => {
  const group = (command: string) => ({ matcher: "editFile", hooks: [{ type: "command", command }] });

  test("adds its groups beside the ones under the same event, and not twice", () => {
    const file = JSON.stringify({ hooks: { PostToolUse: [group("lint")] } });
    const merged = mergeHooks(file, JSON.stringify({ hooks: { PostToolUse: [group("lint"), group("fmt")] } }))!;
    expect(JSON.parse(merged.text).hooks.PostToolUse).toEqual([group("lint"), group("fmt")]);
    expect(merged.message).toBe("Added hooks for PostToolUse");

    const bare = mergeHooks(file, `"Stop": [${JSON.stringify({ hooks: [{ type: "command", command: "say" }] })}]`)!;
    expect(Object.keys(JSON.parse(bare.text).hooks)).toEqual(["PostToolUse", "Stop"]);
  });

  test("is not a snippet when the events do not hold groups", () => {
    expect(mergeHooks("{}", '{"PostToolUse": "lint"}')).toBeNull();
    expect(mergeHooks("{}", '{"hooks": {}}')).toBeNull();
  });
});

describe("the editor", () => {
  const editor = (props: { text?: string; onSave?: (text: string) => Promise<boolean> } = {}) =>
    render(
      <ConfigFileEditor
        label="MCP configuration"
        text={props.text ?? EMPTY_MCP}
        error={null}
        note=""
        merge={mergeMcp}
        onSave={props.onSave ?? (async () => true)}
        onClose={() => {}}
      />,
    );
  const box = () => screen.getByLabelText("MCP configuration") as HTMLTextAreaElement;
  const paste = (text: string) => fireEvent.paste(box(), { clipboardData: { getData: () => text } });

  test("a pasted snippet joins the file and says what it added", () => {
    editor();
    box().setSelectionRange(3, 3);
    paste(JSON.stringify({ github: GITHUB }));
    expect(servers(box().value)).toEqual({ github: GITHUB });
    expect(screen.getByText("Added github")).toBeTruthy();
  });

  test("pasting over everything replaces it instead", () => {
    editor();
    box().setSelectionRange(0, box().value.length);
    const event = paste(JSON.stringify({ github: GITHUB }));
    expect(event).toBe(true);
    expect(box().value).toBe(EMPTY_MCP);
  });

  test("broken JSON says where, and cannot be saved until fixed", async () => {
    const saved: string[] = [];
    editor({ onSave: async (text) => (saved.push(text), true) });
    fireEvent.change(box(), { target: { value: '{\n  "mcpServers": {\n}' } });
    expect(screen.getByText(/^Line 3, column 2:/)).toBeTruthy();
    const save = screen.getByText("Save") as HTMLButtonElement;
    expect(save.disabled).toBe(true);
    fireEvent.keyDown(box(), { key: "s", code: "KeyS", ...MOD });

    fireEvent.change(box(), { target: { value: '{\n  "mcpServers": {}\n}' } });
    expect(save.disabled).toBe(false);
    await act(async () => {
      fireEvent.keyDown(box(), { key: "s", code: "KeyS", ...MOD });
    });
    expect(saved).toEqual(['{\n  "mcpServers": {}\n}']);
  });

  test("Tab indents instead of leaving the box", () => {
    editor({ text: "{}" });
    box().setSelectionRange(1, 1);
    fireEvent.keyDown(box(), { code: "Tab", key: "Tab" });
    expect(box().value).toBe("{  }");
  });
});
