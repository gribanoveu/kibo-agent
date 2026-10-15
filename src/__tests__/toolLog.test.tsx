import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import type { ToolLogFilter, ToolLogRow } from "../lib/toolLog";

// The log window asks again on every opening and every filter change — the
// log grows while a turn runs — and pages by offset.

let stored: ToolLogRow[] = [];
let asked: ToolLogFilter[] = [];
let enabled = true;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string, args?: { filter?: ToolLogFilter; enabled?: boolean }) => {
    if (command === "tool_log_query") {
      const filter = args!.filter!;
      asked.push(filter);
      const matching = stored.filter((r) => !filter.tool || r.tool === filter.tool);
      const offset = filter.offset ?? 0;
      return Promise.resolve({ rows: matching.slice(offset, offset + (filter.limit ?? 100)), total: matching.length });
    }
    if (command === "tool_log_clear") {
      const n = stored.length;
      stored = [];
      return Promise.resolve(n);
    }
    if (command === "tool_log_enabled_get") return Promise.resolve(enabled);
    if (command === "tool_log_enabled_set") {
      enabled = args!.enabled!;
      return Promise.resolve();
    }
    return Promise.resolve(null);
  },
  transformCallback: (callback: unknown) => callback,
}));

(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const { useToolLog, PAGE } = await import("../hooks/useToolLog");
const { ToolLog } = await import("../components/ToolLog");
const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 0)));

function row(id: number, extra: Partial<ToolLogRow> = {}): ToolLogRow {
  return {
    id,
    tsMs: Date.now(),
    repoRoot: "/repo",
    round: 1,
    providerId: "local",
    model: "qwen",
    tool: "readFile",
    args: { tool: "readFile", args: { path: `src/f${id}.rs` } },
    status: "ok",
    error: null,
    result: { result: "file", content: "<redacted>" },
    durationMs: 3,
    ...extra,
  };
}

beforeEach(() => {
  stored = [];
  asked = [];
  enabled = true;
});

describe("useToolLog", () => {
  test("nothing is read while closed; opening reads the first page", async () => {
    stored = [row(1)];
    const { result, rerender } = renderHook(({ open }) => useToolLog(open), { initialProps: { open: false } });
    await settle();
    expect(asked).toEqual([]);

    rerender({ open: true });
    await settle();
    expect(result.current.rows.map((r) => r.id)).toEqual([1]);
    expect(asked[0]).toEqual({ limit: PAGE, offset: 0 });
  });

  test("a filter starts over; more appends the next page", async () => {
    stored = Array.from({ length: PAGE + 5 }, (_, i) => row(i, { tool: i % 2 ? "grep" : "readFile" }));
    const { result } = renderHook(() => useToolLog(true));
    await settle();
    expect(result.current.rows).toHaveLength(PAGE);

    await act(() => result.current.more());
    expect(result.current.rows).toHaveLength(PAGE + 5);
    expect(asked.at(-1)?.offset).toBe(PAGE);

    act(() => result.current.setFilter({ tool: "grep" }));
    await settle();
    expect(asked.at(-1)).toEqual({ tool: "grep", limit: PAGE, offset: 0 });
    expect(result.current.total).toBe(Math.floor((PAGE + 5) / 2));
    expect(result.current.rows.every((r) => r.tool === "grep")).toBe(true);
  });

  test("clearing empties it; the switch is read and saved", async () => {
    stored = [row(1)];
    enabled = false;
    const { result } = renderHook(() => useToolLog(true));
    await settle();
    expect(result.current.enabled).toBe(false);

    await act(() => result.current.toggle(true));
    expect(enabled).toBe(true);
    await act(() => result.current.clear());
    expect(result.current.rows).toEqual([]);
    expect(result.current.total).toBe(0);
  });
});

describe("the log window", () => {
  const view = (rows: ToolLogRow[], props: Partial<Parameters<typeof ToolLog>[0]> = {}) =>
    render(
      <ToolLog
        rows={rows}
        total={rows.length}
        filter={{}}
        onFilter={() => {}}
        onMore={() => {}}
        onClear={() => {}}
        enabled
        onToggle={() => {}}
        error={null}
        {...props}
      />,
    );

  test("a row says what the call was about, and opens to its details", () => {
    view([
      row(1),
      row(2, { tool: "runCommand", args: { tool: "runCommand", args: { command: "cargo test" } }, status: "error", error: "exit 101" }),
      row(3, { tool: "writeFile", args: null, status: "error" }),
    ]);

    expect(screen.getByText("src/f1.rs")).toBeTruthy();
    expect(screen.getByText("cargo test")).toBeTruthy();
    expect(screen.getByText("(arguments did not parse)")).toBeTruthy();

    fireEvent.click(screen.getByText("cargo test"));
    expect(screen.getByText("exit 101")).toBeTruthy();
    expect(screen.getByText(/local · qwen · round 1 · \/repo/)).toBeTruthy();
  });

  test("clearing takes a second press", () => {
    let cleared = 0;
    view([row(1)], { onClear: () => cleared++ });

    fireEvent.click(screen.getByText("Clear log"));
    expect(cleared).toBe(0);
    fireEvent.click(screen.getByText("Delete all 1?"));
    expect(cleared).toBe(1);
  });

  test("load more appears only while there is more", () => {
    const { rerender } = view([row(1)]);
    expect(screen.queryByText("Load more")).toBeNull();
    rerender(
      <ToolLog rows={[row(1)]} total={5} filter={{}} onFilter={() => {}} onMore={() => {}} onClear={() => {}} enabled onToggle={() => {}} error={null} />,
    );
    expect(screen.getByText("1 of 5")).toBeTruthy();
    expect(screen.getByText("Load more")).toBeTruthy();
  });

  test("search goes into the filter; an empty box clears it", () => {
    const filters: ToolLogFilter[] = [];
    view([row(1)], { filter: { search: "x" }, onFilter: (f) => filters.push(f) });

    fireEvent.change(screen.getByLabelText("Search the log"), { target: { value: "src/" } });
    fireEvent.change(screen.getByLabelText("Search the log"), { target: { value: "" } });
    expect(filters).toEqual([{ search: "src/" }, { search: undefined }]);
  });

  test("an empty log says whether it is recording", () => {
    const { rerender } = view([]);
    expect(screen.getByText("No tool calls match.")).toBeTruthy();
    rerender(
      <ToolLog rows={[]} total={0} filter={{}} onFilter={() => {}} onMore={() => {}} onClear={() => {}} enabled={false} onToggle={() => {}} error={null} />,
    );
    expect(screen.getByText(/The log is off/)).toBeTruthy();
  });
});
