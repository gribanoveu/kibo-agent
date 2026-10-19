import { describe, expect, test } from "bun:test";
import { fireEvent, render, screen } from "@testing-library/react";
import { OverviewPanel, type OverviewProps } from "../components/OverviewPanel";
import type { McpServerItem } from "../lib/mcp";

const server = (name: string, enabled: boolean, state: McpServerItem["state"]): McpServerItem => ({
  name,
  command: name,
  enabled,
  error: null,
  warning: null,
  state,
});

const props = (over: Partial<OverviewProps> = {}): OverviewProps => ({
  branch: "main",
  changes: { files: 3, add: 593, del: 34 },
  context: null,
  usage: null,
  running: false,
  onCompact: () => {},
  servers: [],
  processes: [],
  agents: [],
  onOpenProcess: () => {},
  onOpenAgent: () => {},
  onOpenPanel: () => {},
  ...over,
});

describe("Overview", () => {
  test("lists the MCP servers that are on, with what they are doing", () => {
    const tools = [{ name: "a", description: "" }, { name: "b", description: "" }];
    render(
      <OverviewPanel
        {...props({
          servers: [server("github", true, { state: "running", tools }), server("off", false, { state: "notStarted" }), server("kube", true, { state: "starting" })],
        })}
      />,
    );
    expect(screen.getByText("github")).toBeTruthy();
    expect(screen.getByText("2 tools")).toBeTruthy();
    expect(screen.getByText("starting")).toBeTruthy();
    expect(screen.queryByText("off")).toBeNull();
  });

  test("the branch and the lines changed open Changes", () => {
    const opened: string[] = [];
    render(<OverviewPanel {...props({ onOpenPanel: (tab) => opened.push(tab) })} />);
    expect(screen.getByText("main")).toBeTruthy();
    expect(screen.getByText("−34")).toBeTruthy();
    fireEvent.click(screen.getByText("+593"));
    expect(opened).toEqual(["changes"]);
  });

  test("lists what runs in the background, opens it, and counts what ended", () => {
    const opened: unknown[] = [];
    const tokens = { prompt: 0, cached: 0, completion: 0 };
    render(
      <OverviewPanel
        {...props({
          processes: [
            { id: 1, command: "bun run dev", cwd: "/", state: { state: "running" } },
            { id: 2, command: "cargo test", cwd: "/", state: { state: "exited", code: 0 } },
          ],
          agents: [
            { id: 7, task: "Find the ranking code", state: { state: "running" }, steps: [], tokens, answer: null },
            { id: 8, task: "Old", state: { state: "done" }, steps: [], tokens, answer: "x" },
          ],
          onOpenProcess: (id) => opened.push(id),
          onOpenAgent: (agent) => opened.push(agent),
        })}
      />,
    );
    expect(screen.queryByText("cargo test")).toBeNull();
    fireEvent.click(screen.getByText("bun run dev"));
    fireEvent.click(screen.getByText("Find the ranking code"));
    expect(opened).toEqual([1, { id: 7, task: "Find the ranking code" }]);
    expect(screen.getByText("2 ended")).toBeTruthy();
  });

  test("says when there is nothing to count", () => {
    render(<OverviewPanel {...props({ changes: null })} />);
    expect(screen.getByText("Not a git repository.")).toBeTruthy();
    expect(screen.getByText("Nothing is running.")).toBeTruthy();
  });
});
