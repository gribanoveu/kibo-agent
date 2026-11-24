import { describe, expect, test } from "bun:test";
import { act, render, renderHook, screen } from "@testing-library/react";
import { useConfigDialogs } from "../hooks/useConfigDialogs";
import { McpConfigDialog } from "../components/McpConfigDialog";
import { HooksConfigDialog } from "../components/HooksConfigDialog";
import type { McpView } from "../lib/mcp";
import type { HooksView } from "../lib/agentHooks";

const mcpView: McpView = { path: "/home/u/.kibo/mcp.json", text: '{"mcpServers":{}}', servers: [] };
const hooksView: HooksView = { path: "/home/u/.kibo/hooks.json", text: "", hooks: [] };
const save = async () => true;
const noop = () => {};

describe("the config dialogs' state", () => {
  test("each file opens as a form for one entry or the whole file, and closes on its own", () => {
    const { result } = renderHook(() => useConfigDialogs());
    expect(result.current.mcpDialog).toBeNull();
    expect(result.current.hooksDialog).toBeNull();
    expect(result.current.mcpEditing).toBe(false);
    expect(result.current.hooksEditing).toBe(false);
    act(() => result.current.addMcp());
    expect(result.current.mcpDialog).toEqual({ entry: null });
    expect(result.current.mcpEditing).toBe(true);
    act(() => result.current.editMcp("srv"));
    expect(result.current.mcpDialog).toEqual({ entry: "srv" });
    act(() => result.current.editMcpFile());
    expect(result.current.mcpDialog).toBe("json");
    act(() => result.current.addHook());
    expect(result.current.hooksDialog).toEqual({ entry: null });
    expect(result.current.hooksEditing).toBe(true);
    act(() => result.current.editHook(2));
    expect(result.current.hooksDialog).toEqual({ entry: 2 });
    act(() => result.current.editHooksFile());
    expect(result.current.hooksDialog).toBe("json");
    act(() => result.current.closeMcp());
    expect(result.current.mcpDialog).toBeNull();
    expect(result.current.mcpEditing).toBe(false);
    // The other file's dialog is not touched by this one's close.
    expect(result.current.hooksDialog).toBe("json");
    act(() => result.current.closeHooks());
    expect(result.current.hooksDialog).toBeNull();
    expect(result.current.hooksEditing).toBe(false);
  });
});

describe("the MCP dialog", () => {
  const props = { view: mcpView, error: null, onSave: save, onClose: noop, onEditJson: noop };
  test("closed draws nothing; a server opens the form; the whole file opens the editor", () => {
    const { container, rerender } = render(<McpConfigDialog dialog={null} {...props} />);
    expect(container.firstChild).toBeNull();
    rerender(<McpConfigDialog dialog={{ entry: null }} {...props} />);
    expect(screen.getByText("Add an MCP server")).toBeTruthy();
    expect(screen.queryByLabelText("MCP configuration")).toBeNull();
    rerender(<McpConfigDialog dialog={{ entry: "srv" }} {...props} />);
    expect(screen.getByText("srv")).toBeTruthy();
    rerender(<McpConfigDialog dialog="json" {...props} />);
    expect(screen.getByText("MCP servers")).toBeTruthy();
    expect(screen.getByLabelText("MCP configuration")).toBeTruthy();
  });
});

describe("the hooks dialog", () => {
  const props = { view: hooksView, error: null, onSave: save, onClose: noop, onEditJson: noop };
  test("closed draws nothing; a new hook and an existing one name themselves; the file opens the editor", () => {
    const { container, rerender } = render(<HooksConfigDialog dialog={null} {...props} />);
    expect(container.firstChild).toBeNull();
    rerender(<HooksConfigDialog dialog={{ entry: null }} {...props} />);
    expect(screen.getByText("Add a hook")).toBeTruthy();
    expect(screen.queryByLabelText("Hooks configuration")).toBeNull();
    rerender(<HooksConfigDialog dialog={{ entry: 0 }} {...props} />);
    expect(screen.getByText("Edit hook")).toBeTruthy();
    rerender(<HooksConfigDialog dialog="json" {...props} />);
    expect(screen.getByText("Hooks")).toBeTruthy();
    expect(screen.getByLabelText("Hooks configuration")).toBeTruthy();
  });
});