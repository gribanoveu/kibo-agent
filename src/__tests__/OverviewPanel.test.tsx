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
  spent: null,
  subagents: null,
  servers: [],
  processes: [],
  agents: [],
  onOpenProcess: () => {},
  onOpenProcesses: () => {},
  onOpenAgent: () => {},
  onOpenPanel: () => {},
  ...over,
});

describe("Overview", () => {
  /// Switching to a chat whose estimate is still being read: the card keeps
  /// its rows, with placeholders for the numbers, so nothing below it moves
  /// when they arrive.
  test("holds the context card's place while its estimate is read", () => {
    const { rerender } = render(<OverviewPanel {...props({ contextPending: true, spent: 10 })} />);
    const card = () => document.querySelector(".overview-ctx") as HTMLElement;
    expect(card().getAttribute("aria-busy")).toBe("true");
    expect(screen.getByText("Context window")).toBeTruthy();
    // Every number not known yet waits in its row; the one already read shows.
    const unread = [...card().querySelectorAll(".ctx-rows > div")]
      .filter((row) => !row.querySelector(".ctx-skeleton"))
      .map((row) => row.querySelector("dt")?.textContent);
    expect(unread).toEqual(["Session tokens"]);
    const rows = card().querySelectorAll(".ctx-rows > div").length;

    const context = { instructions: 1, skills: 2, tools: 3, mcp: 4, conversation: 5, total: 15, limit: 100, compactsAt: null };
    rerender(<OverviewPanel {...props({ context, spent: 10 })} />);
    expect(card().getAttribute("aria-busy")).toBe("false");
    expect(card().querySelectorAll(".ctx-skeleton")).toHaveLength(0);
    expect(card().querySelectorAll(".ctx-rows > div")).toHaveLength(rows);
  });

  test("draws no context card when there is no estimate to wait for", () => {
    render(<OverviewPanel {...props()} />);
    expect(screen.queryByText("Context window")).toBeNull();
  });

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

  /// Another chat of the folder switched it: Changes shows what is checked
  /// out, and this chat's own branch under it, one click away.
  test("a chat the folder was moved off says which branch is its own, and switches back", () => {
    let switched = 0;
    const { rerender } = render(<OverviewPanel {...props({ away: { branch: "feature", onSwitch: () => switched++ } })} />);
    expect(screen.getByText("main")).toBeTruthy();
    fireEvent.click(screen.getByTitle(/This chat works on feature; the folder is on main now/));
    expect(switched).toBe(1);

    rerender(<OverviewPanel {...props({ away: { branch: "feature", onSwitch: () => switched++, locked: "An agent is working in this folder" } })} />);
    const back = screen.getByTitle("An agent is working in this folder");
    expect(back.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(back);
    expect(switched).toBe(1);

    rerender(<OverviewPanel {...props()} />);
    expect(screen.queryByText(/Back to/)).toBeNull();
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
            { id: 7, task: "Find the ranking code", state: { state: "running" }, steps: [], tokens, answer: null, progress: { streamSince: null, streamChars: 0, timed: { tokens: 0, ms: 0, chars: 0 } } },
            { id: 8, task: "Old", state: { state: "done" }, steps: [], tokens, answer: "x", progress: { streamSince: null, streamChars: 0, timed: { tokens: 0, ms: 0, chars: 0 } } },
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
    expect(screen.getByText("Find the ranking code").closest("[data-sheet-opener]")).toBeTruthy();
    expect(screen.getByText("2 ended")).toBeTruthy();
  });

  // Terminal alone would open on whichever of its tabs was picked last.
  test("the Background heading asks for the processes, not just the Terminal", () => {
    const opened: string[] = [];
    render(<OverviewPanel {...props({ onOpenProcesses: () => opened.push("processes"), onOpenPanel: (tab) => opened.push(tab) })} />);
    fireEvent.click(screen.getByText("Background"));
    expect(opened).toEqual(["processes"]);
  });

  test("the subagents' share of the session total stands under it", () => {
    render(
      <OverviewPanel
        {...props({
          context: { instructions: 1000, skills: 0, tools: 2000, mcp: 0, conversation: 7000, total: 10000, limit: 100000, compactsAt: 80000 },
          spent: 38000,
          subagents: 12400,
        })}
      />,
    );
    expect(screen.getByText("Session tokens").nextElementSibling?.textContent).toBe("38k");
    expect(screen.getByText("Subagents tokens").nextElementSibling?.textContent).toBe("12k");
  });

  test("says when there is nothing to count", () => {
    render(<OverviewPanel {...props({ changes: null })} />);
    expect(screen.getByText("Not a git repository.")).toBeTruthy();
    expect(screen.getByText("Nothing is running.")).toBeTruthy();
  });

  test("shows the cache hit for the context, without the cost note or Compact", () => {
    const context = { instructions: 1000, skills: 0, tools: 2000, mcp: 0, conversation: 7000, total: 10000, limit: 100000, compactsAt: 80000 };
    const usage = { promptTokens: 10000, cachedTokens: 8000, completionTokens: 100, totalTokens: 10100 };
    render(<OverviewPanel {...props({ context, usage })} />);
    expect(screen.getByText("Cache hit").nextElementSibling?.textContent).toBe("80%");
    expect(screen.queryByText(/actually cost/)).toBeNull();
    expect(screen.queryByText(/on its own at/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Compact now" })).toBeNull();
  });
});
