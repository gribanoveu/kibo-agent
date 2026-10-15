import { describe, expect, test } from "bun:test";
import { render, screen } from "@testing-library/react";
import { DataPolicy } from "../components/DataPolicy";
import type { HookItem } from "../lib/agentHooks";
import type { McpServerItem } from "../lib/mcp";
import type { ProviderView } from "../lib/settings";

// "Where your data goes" says what is configured now, not what could be.

const provider: ProviderView = { id: "work", baseUrl: "https://llm.corp.example/v1", hasApiKey: true };
const server = (name: string, enabled: boolean, command = "npx"): McpServerItem => ({
  name,
  command,
  enabled,
  error: null,
  warning: null,
  state: { state: "notStarted" },
});
const hook: HookItem = { event: "PreToolUse", matcher: "", command: "./guard.sh", timeoutSecs: 60, problem: null };

const row = (name: string) => screen.getByText(name).closest(".data-policy-row")?.textContent ?? "";

describe("where your data goes", () => {
  test("names the provider's host and only the servers that are on", () => {
    render(
      <DataPolicy
        provider={provider}
        debugLogging={false}
        mcpServers={[server("github", true), server("sentry", false), server("fs", true)]}
        hooks={[hook, hook]}
      />,
    );

    expect(row("Model provider")).toContain("llm.corp.example (work)");
    expect(row("MCP servers")).toContain("github, fs");
    expect(row("MCP servers")).not.toContain("sentry");
    expect(row("Hooks")).toContain("2 commands");
    expect(row("Request log")).toContain("off");
  });

  test("a server at a URL is named with its host: that is where its calls go", () => {
    render(
      <DataPolicy
        provider={provider}
        debugLogging={false}
        mcpServers={[server("github", true), server("docs", true, "HTTPS://mcp.example.com/mcp")]}
        hooks={[]}
      />,
    );
    expect(row("MCP servers")).toContain("github, docs (mcp.example.com)");
    expect(row("MCP servers")).toContain("over the network");
  });

  test("says when nothing is set up, and when the request log writes to disk", () => {
    render(<DataPolicy provider={null} debugLogging mcpServers={[server("github", false)]} hooks={[]} />);

    expect(row("Model provider")).toContain("none set up");
    expect(row("MCP servers")).toContain("none enabled");
    expect(row("Hooks")).toContain("none");
    expect(row("Request log")).toContain("written whole");
  });

  test("a long list of servers is cut, and one not yet loaded is not called empty", () => {
    const many = ["a", "b", "c", "d", "e"].map((name) => server(name, true));
    const { rerender } = render(<DataPolicy provider={provider} debugLogging={false} mcpServers={many} hooks={null} />);
    expect(row("MCP servers")).toContain("a, b, c and 2 more");
    expect(row("Hooks")).not.toContain("none");

    rerender(<DataPolicy provider={{ ...provider, baseUrl: "not a url" }} debugLogging={false} mcpServers={null} hooks={[hook]} />);
    expect(row("Model provider")).toContain("not a url");
    expect(row("MCP servers")).not.toContain("none");
    expect(row("Hooks")).toContain("1 command");
    expect(row("Hooks")).not.toContain("1 commands");
  });
});
