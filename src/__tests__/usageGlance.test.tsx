import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor } from "@testing-library/react";
import type { DailyMetric } from "../lib/usage";

// The line under a new chat's box: what today has cost, what the week cost, and
// how long the run of days is. Nothing at all until something has been counted,
// and no error line on this screen.

let rows: DailyMetric[] | Error;

mock.module("@tauri-apps/api/core", () => ({
  invoke: (command: string) => {
    if (command !== "daily_metrics") return Promise.resolve(null);
    return rows instanceof Error ? Promise.reject(rows) : Promise.resolve(rows);
  },
}));

const { UsageGlance } = await import("../components/UsageGlance");

beforeEach(() => {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
});
afterAll(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

/** A local day `back` days ago, as the counters are kept. */
const day = (back: number) => {
  const d = new Date();
  d.setDate(d.getDate() - back);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};
const spent = (back: number, value: number): DailyMetric => ({ day: day(back), metric: "promptTokens", key: "", value });

/** The counters come oldest first, as the backend keeps them: the run of days
    is counted from the first of them. */
const run = (days: number, each: number) => Array.from({ length: days }, (_, i) => spent(days - 1 - i, each));

const parts = () => [...document.querySelectorAll(".usage-glance span")].map((one) => one.textContent);

describe("the line under a new chat's box", () => {
  test("today, the week, and the run of days up to today", async () => {
    rows = run(3, 400);
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toEqual(["today 400", "1.2k in 7 days", "3-day streak"]));
  });

  /// The total is the week's; the run is not cut at it — ten days in a row are
  /// ten, however short the range beside them.
  test("a run longer than the week is still said whole", async () => {
    rows = run(10, 100);
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toEqual(["today 100", "700 in 7 days", "10-day streak"]));
  });

  /// A day with nothing spent yet is not a zero to read, and it does not break
  /// the run that reached yesterday.
  test("nothing today drops that part, and yesterday's run still stands", async () => {
    rows = [spent(2, 300), spent(1, 300)];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toEqual(["600 in 7 days", "2-day streak"]));
  });

  test("a single day is no streak worth saying", async () => {
    rows = [spent(1, 900)];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toEqual(["900 in 7 days"]));
  });

  test("nothing before the first count, and nothing when it cannot be read", async () => {
    for (const read of [[], new Error("locked")]) {
      rows = read;
      const { unmount } = render(<UsageGlance />);
      await waitFor(() => expect(document.querySelector(".usage-glance")).toBeNull());
      await new Promise((r) => setTimeout(r, 0));
      expect(document.querySelector(".usage-glance")).toBeNull();
      expect(screen.queryByText("locked", { exact: false })).toBeNull();
      unmount();
    }
  });
});
