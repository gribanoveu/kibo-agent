import { describe, expect, test } from "bun:test";
import { HEATMAP_WEEKS, usageHeatmap, usageStats, usageStrip, type DailyMetric } from "../lib/usage";

// The usage pane's arithmetic: the weeks the heatmap draws and how
// dark each day is, and the figures for the range picked under it.

const row = (day: string, metric: DailyMetric["metric"], value: number, key = ""): DailyMetric => ({ day, metric, key, value });
const spent = (day: string, tokens: number) => row(day, "promptTokens", tokens);
// A Friday.
const today = new Date(2026, 9, 9);

describe("usageHeatmap", () => {
  test("half a year of weeks, Monday first, this one last and cut at today", () => {
    const { weeks } = usageHeatmap([], today);
    expect(weeks).toHaveLength(HEATMAP_WEEKS);
    expect(weeks[0][0]!.day).toBe("2026-04-13");
    expect(weeks[HEATMAP_WEEKS - 1].map((d) => d?.day ?? null)).toEqual([
      "2026-10-05", "2026-10-06", "2026-10-07", "2026-10-08", "2026-10-09", null, null,
    ]);
  });

  test("a month is named over the first column it starts", () => {
    const { months } = usageHeatmap([], today);
    expect(months.filter((m) => m !== null)).toEqual(["Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct"]);
  });

  test("busier days are darker, by quartile of the days that spent anything", () => {
    const { weeks } = usageHeatmap([1, 2, 3, 4].map((n) => spent(`2026-10-0${n + 4}`, n * 100)), today);
    expect(weeks[HEATMAP_WEEKS - 1].slice(0, 5).map((d) => d!.level)).toEqual([1, 2, 3, 4, 0]);
  });
});

describe("usageStrip", () => {
  test("the last fourteen days in a row, today last, shaded as the heatmap is", () => {
    const strip = usageStrip([spent("2026-09-26", 100), spent("2026-10-09", 400), spent("2026-09-25", 999)], today, 14);
    expect(strip.map((d) => d.day)[0]).toBe("2026-09-26");
    expect(strip.map((d) => d.day)[13]).toBe("2026-10-09");
    expect(strip).toHaveLength(14);
    expect([strip[0].level, strip[1].level, strip[13].level]).toEqual([1, 0, 4]);
  });
});

describe("usageStats", () => {
  test("a day's tokens are prompt plus output, cached counted inside them", () => {
    const stats = usageStats(
      [row("2026-10-09", "cachedTokens", 800), row("2026-10-09", "completionTokens", 50), row("2026-10-09", "promptTokens", 1000)],
      "30",
      today,
    );
    expect([stats.total, stats.cached, stats.completion]).toEqual([1050, 800, 50]);
    expect([stats.activeDays, stats.days]).toEqual([1, 30]);
  });

  test("days outside the range are left out, and all time starts at the first day counted", () => {
    const rows = [spent("2026-09-01", 5), spent("2026-10-09", 7)];
    expect(usageStats(rows, "7", today).total).toBe(7);
    const all = usageStats(rows, "all", today);
    expect([all.total, all.days]).toEqual([12, 39]);
  });

  test("streaks: the longest run, and the current one still standing while today is empty", () => {
    const rows = ["2026-09-30", "2026-10-01", "2026-10-02", "2026-10-06", "2026-10-07", "2026-10-08"].map((d) => spent(d, 1));
    const stats = usageStats(rows, "30", today);
    expect([stats.currentStreak, stats.longestStreak]).toEqual([3, 3]);
    expect(usageStats([...rows, spent("2026-10-09", 1)], "30", today).currentStreak).toBe(4);
    expect(usageStats([spent("2026-10-07", 1)], "30", today).currentStreak).toBe(0);
  });

  test("the most active day is the busiest of the range", () => {
    expect(usageStats([spent("2026-10-03", 5), spent("2026-10-05", 9), spent("2026-10-08", 2)], "7", today).mostActive).toBe("2026-10-05");
    expect(usageStats([], "7", today).mostActive).toBeNull();
  });

  test("the keyed counters: favourite model, sessions by folder, tools by calls, tokens by hour — within the range", () => {
    const rows = [
      row("2026-09-01", "modelTokens", 999, "old"),
      row("2026-10-08", "modelTokens", 300, "qwen"),
      row("2026-10-09", "modelTokens", 200, "gpt"),
      row("2026-10-09", "modelTokens", 200, "qwen"),
      row("2026-10-08", "sessions", 2, "/work/kibo"),
      row("2026-10-09", "sessions", 1, ""),
      row("2026-10-09", "sessions", 1, "/work/kibo"),
      row("2026-10-09", "prompts", 7),
      row("2026-10-09", "toolCalls", 5, "readFile"),
      row("2026-10-09", "toolCalls", 9, "grep"),
      row("2026-10-09", "hourTokens", 40, "14"),
      row("2026-10-09", "hourTokens", 2, "00"),
    ];
    const stats = usageStats(rows, "7", today);
    expect(stats.favoriteModel).toBe("qwen");
    expect([stats.sessions, stats.prompts, stats.toolCalls]).toEqual([4, 7, 14]);
    expect(stats.topProjects).toEqual([{ key: "/work/kibo", value: 3 }, { key: "", value: 1 }]);
    expect(stats.topTools.map((t) => t.key)).toEqual(["grep", "readFile"]);
    expect([stats.hours.length, stats.hours[0], stats.hours[14], stats.hours[15]]).toEqual([24, 2, 40, 0]);
    // Keyed counters are not tokens of the day.
    expect(stats.total).toBe(0);
    expect(usageStats(rows, "all", today).favoriteModel).toBe("old");
  });
});
