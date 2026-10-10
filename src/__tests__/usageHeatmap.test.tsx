import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor } from "@testing-library/react";
import type { DailyMetric } from "../lib/usage";

// The heatmap on a new agent chat: there once anything has been counted, and
// nothing at all before that — no empty grid, no error line.

let rows: DailyMetric[] | Error;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    if (command !== "daily_metrics") return Promise.resolve(null);
    return rows instanceof Error ? Promise.reject(rows) : Promise.resolve(rows);
  },
}));

const { ChatEmptyState } = await import("../components/ChatEmptyState");

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

const today = () => {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};

describe("a new agent chat", () => {
  test("shows a strip of fourteen days and one line for them once anything has been counted", async () => {
    rows = [
      { day: today(), metric: "promptTokens", key: "", value: 1200 },
      { day: today(), metric: "sessions", key: "/repo", value: 1 },
      { day: today(), metric: "modelTokens", key: "qwen", value: 1200 },
    ];
    render(<ChatEmptyState workspace="/repo" onOpenRepo={() => {}} />);
    expect(await screen.findByRole("grid", { name: "Tokens per day" })).toBeTruthy();
    expect(screen.getAllByRole("gridcell")).toHaveLength(14);
    for (const said of ["Last 14 days", "1.2k tokens", "1 session", "qwen"]) expect(screen.getByText(said)).toBeTruthy();
    // Minimal: no range to pick, no figures, no details.
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.queryByText("Peak hours")).toBeNull();
  });

  test("shows nothing before the first count, or when it cannot read", async () => {
    for (const read of [[], new Error("locked")]) {
      rows = read;
      const { unmount } = render(<ChatEmptyState workspace="/repo" onOpenRepo={() => {}} />);
      await waitFor(() => expect(screen.getByText("Start the conversation")).toBeTruthy());
      await new Promise((r) => setTimeout(r, 0));
      expect(screen.queryByRole("grid")).toBeNull();
      expect(screen.queryByText("locked", { exact: false })).toBeNull();
      unmount();
    }
  });

  test("is not on the page with no folder open", async () => {
    rows = [{ day: today(), metric: "promptTokens", key: "", value: 1200 }];
    render(<ChatEmptyState workspace={null} onOpenRepo={() => {}} />);
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByRole("grid")).toBeNull();
  });
});
