import { afterAll, beforeEach, describe, expect, mock, test } from "bun:test";
import { render, screen, waitFor } from "@testing-library/react";
import type { DailyMetric } from "../lib/usage";

// The line under a new chat's box: what today has cost, what this calendar week
// has cost, and how long the run of days is. Nothing at all until something has
// been counted, and no error line on this screen.

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

/** How many days of this week have passed, today included: one on a Monday. */
const weekDays = () => ((new Date().getDay() + 6) % 7) + 1;
/** Each day of this week so far, Monday first. */
const thisWeek = (each: number) => run(weekDays(), each);

const parts = () => [...document.querySelectorAll(".usage-glance span")].map((one) => one.textContent);

describe("the line under a new chat's box", () => {
  test("today, this week, and the run of days up to today", async () => {
    rows = thisWeek(100);
    const streak = weekDays() > 1 ? [`${weekDays()}-day streak`] : [];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toEqual(["today 100", `${100 * weekDays()} this week`, ...streak]));
  });

  /// The week is the calendar's, Monday to Sunday: what the Sunday before it
  /// spent is not this week's, however few days ago that was.
  test("the week starts at its Monday, not seven days back", async () => {
    rows = [spent(weekDays(), 999), ...thisWeek(100)];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toContain(`${100 * weekDays()} this week`));
    expect(parts().join(" ")).not.toContain("999");
    expect(parts().join(" ")).not.toContain("1.1k");
  });

  /// The total is the week's; the run is not cut at it — ten days in a row are
  /// ten, however short the week beside them.
  test("a run longer than the week is still said whole", async () => {
    rows = run(10, 100);
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toContain("10-day streak"));
    expect(parts()).toContain(`${100 * weekDays()} this week`);
  });

  /// A day with nothing spent yet is not a zero to read, and it does not break
  /// the run that reached yesterday.
  test("nothing today drops that part, and yesterday's run still stands", async () => {
    rows = [spent(2, 300), spent(1, 300)];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toContain("2-day streak"));
    expect(parts().some((p) => p.startsWith("today"))).toBe(false);
    expect(parts().some((p) => p.endsWith("this week"))).toBe(true);
  });

  test("a single day is no streak worth saying", async () => {
    rows = [spent(1, 900)];
    render(<UsageGlance />);
    await waitFor(() => expect(parts()).toHaveLength(1));
    expect(parts()[0].endsWith("this week")).toBe(true);
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
