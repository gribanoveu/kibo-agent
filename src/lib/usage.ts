import { invoke } from "@tauri-apps/api/core";
import { inTauri } from "./chat";

/** A counter the backend keeps per local day — `domain::metrics::Metric`. */
export type Metric =
  | "promptTokens"
  | "cachedTokens"
  | "completionTokens"
  | "modelTokens"
  | "hourTokens"
  | "sessions"
  | "prompts"
  | "toolCalls";

export const METRICS: Metric[] = [
  "promptTokens",
  "cachedTokens",
  "completionTokens",
  "modelTokens",
  "hourTokens",
  "sessions",
  "prompts",
  "toolCalls",
];

export interface DailyMetric {
  /** `YYYY-MM-DD`, the user's local date. */
  day: string;
  metric: Metric;
  /** What the counter is kept per: the model, the hour (`00`–`23`), the folder, the tool; empty otherwise. */
  key: string;
  value: number;
}

/** Every day `metrics` counted anything, oldest first. */
export async function dailyMetrics(metrics: Metric[]): Promise<DailyMetric[]> {
  if (!inTauri()) throw new Error("This needs the desktop app — there is no backend in a plain browser.");
  return invoke<DailyMetric[]>("daily_metrics", { metrics });
}

/** `14` is the new chat's strip; Settings offers the other three. */
export type UsageRange = "all" | "30" | "14" | "7";

/** How far back the heatmap reaches: half a year, enough columns to fill the card at a small cell. */
export const HEATMAP_WEEKS = 26;

export interface UsageDay {
  day: string;
  /** Sent and received; `cached` is part of it. */
  tokens: number;
  cached: number;
  completion: number;
  /** 0 for nothing spent, then 1–4 by quartile of the heatmap's spending days, as GitHub shades. */
  level: 0 | 1 | 2 | 3 | 4;
}

export interface UsageHeatmap {
  /** Columns of seven, Monday first, the last one this week's; `null` for the days after today. */
  weeks: (UsageDay | null)[][];
  /** The month a column starts, where it differs from the column before; `null` otherwise. */
  months: (string | null)[];
}

export interface UsageStats {
  total: number;
  cached: number;
  completion: number;
  activeDays: number;
  /** The days the range covers, today included. */
  days: number;
  /** Days in a row with something spent, up to today — or to yesterday, while today has nothing yet. */
  currentStreak: number;
  longestStreak: number;
  /** The busiest day of the range; `null` while nothing is spent. */
  mostActive: string | null;
  /** The model that spent the most tokens; `null` while none has. */
  favoriteModel: string | null;
  /** Models by tokens spent, most first. */
  topModels: Ranked[];
  sessions: number;
  prompts: number;
  toolCalls: number;
  /** Tokens per local hour of the day, midnight first. */
  hours: number[];
  /** Folders by chats started in them, busiest first; `""` is Chat mode. */
  topProjects: Ranked[];
  /** Tools by calls, most called first. */
  topTools: Ranked[];
}

export interface Ranked {
  key: string;
  value: number;
}

const TOP = 3;

const parse = (day: string) => {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
};
const format = (date: Date) =>
  `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
const addDays = (date: Date, n: number) => new Date(date.getFullYear(), date.getMonth(), date.getDate() + n);
const midnight = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate());

/** Every day from `start` to `end`, each with what it spent. */
function spentDays(rows: DailyMetric[], start: Date, end: Date): UsageDay[] {
  const byDay = new Map<string, { prompt: number; cached: number; completion: number }>();
  for (const { day, metric, value } of rows) {
    if (metric !== "promptTokens" && metric !== "cachedTokens" && metric !== "completionTokens") continue;
    const known = byDay.get(day) ?? { prompt: 0, cached: 0, completion: 0 };
    if (metric === "promptTokens") known.prompt += value;
    else if (metric === "cachedTokens") known.cached += value;
    else known.completion += value;
    byDay.set(day, known);
  }
  const days: UsageDay[] = [];
  for (let date = start; date <= end; date = addDays(date, 1)) {
    const day = format(date);
    const spent = byDay.get(day);
    days.push({
      day,
      tokens: spent ? spent.prompt + spent.completion : 0,
      cached: spent?.cached ?? 0,
      completion: spent?.completion ?? 0,
      level: 0,
    });
  }
  return days;
}

/** Shades `days` in place: 1–4 by quartile of those that spent anything. */
function shade(days: UsageDay[]): UsageDay[] {
  const spending = days.map((d) => d.tokens).filter((t) => t > 0).sort((a, b) => a - b);
  const quartile = (p: number) => spending[Math.floor(p * (spending.length - 1))];
  const cuts = [quartile(0.25), quartile(0.5), quartile(0.75)];
  for (const d of days) {
    if (d.tokens > 0) d.level = (1 + cuts.filter((cut) => d.tokens > cut).length) as UsageDay["level"];
  }
  return days;
}

/** The last `count` days in a row, today last — the new chat's strip. */
export function usageStrip(rows: DailyMetric[], today: Date, count: number): UsageDay[] {
  const end = midnight(today);
  return shade(spentDays(rows, addDays(end, -(count - 1)), end));
}

/** The last [`HEATMAP_WEEKS`] weeks, this one on the right. */
export function usageHeatmap(rows: DailyMetric[], today: Date): UsageHeatmap {
  const end = midnight(today);
  const monday = addDays(end, -((end.getDay() + 6) % 7));
  const days = shade(spentDays(rows, addDays(monday, -7 * (HEATMAP_WEEKS - 1)), end));

  const cells: (UsageDay | null)[] = [...days];
  while (cells.length % 7) cells.push(null);
  const weeks: (UsageDay | null)[][] = [];
  for (let i = 0; i < cells.length; i += 7) weeks.push(cells.slice(i, i + 7));

  let shown = "";
  const months = weeks.map((week) => {
    const month = parse(week[0]!.day).toLocaleString("en", { month: "short" });
    if (month === shown) return null;
    shown = month;
    return month;
  });
  return { weeks, months };
}

/** What `range` up to `today` spent. All time starts at the first day counted. */
export function usageStats(rows: DailyMetric[], range: UsageRange, today: Date): UsageStats {
  const end = midnight(today);
  const start =
    range === "all" ? (rows.length ? parse(rows[0].day) : end) : addDays(end, -(Number(range) - 1));
  const days = spentDays(rows, start, end);

  let run = 0;
  let longestStreak = 0;
  for (const d of days) {
    run = d.tokens > 0 ? run + 1 : 0;
    longestStreak = Math.max(longestStreak, run);
  }
  // Today not over yet: a streak that reached yesterday still stands.
  const upTo = days.length && days[days.length - 1].tokens === 0 ? days.length - 1 : days.length;
  let currentStreak = 0;
  while (currentStreak < upTo && days[upTo - 1 - currentStreak].tokens > 0) currentStreak++;

  const busiest = days.reduce<UsageDay | null>((best, d) => (d.tokens > (best?.tokens ?? 0) ? d : best), null);

  // The keyed counters, summed over the range.
  const from = format(start);
  const to = format(end);
  const sums = new Map<Metric, Map<string, number>>();
  for (const { day, metric, key, value } of rows) {
    if (day < from || day > to) continue;
    const byKey = sums.get(metric) ?? new Map<string, number>();
    byKey.set(key, (byKey.get(key) ?? 0) + value);
    sums.set(metric, byKey);
  }
  const ranked = (metric: Metric): Ranked[] =>
    [...(sums.get(metric) ?? new Map<string, number>())]
      .map(([key, value]) => ({ key, value }))
      .sort((a, b) => b.value - a.value || a.key.localeCompare(b.key));
  const sum = (metric: Metric) => ranked(metric).reduce((total, r) => total + r.value, 0);
  const hourly = sums.get("hourTokens");

  return {
    total: days.reduce((sum, d) => sum + d.tokens, 0),
    cached: days.reduce((sum, d) => sum + d.cached, 0),
    completion: days.reduce((sum, d) => sum + d.completion, 0),
    activeDays: days.filter((d) => d.tokens > 0).length,
    days: days.length,
    currentStreak,
    longestStreak,
    mostActive: busiest?.day ?? null,
    favoriteModel: ranked("modelTokens")[0]?.key ?? null,
    topModels: ranked("modelTokens").slice(0, TOP),
    sessions: sum("sessions"),
    prompts: sum("prompts"),
    toolCalls: sum("toolCalls"),
    hours: Array.from({ length: 24 }, (_, h) => hourly?.get(String(h).padStart(2, "0")) ?? 0),
    topProjects: ranked("sessions").slice(0, TOP),
    topTools: ranked("toolCalls").slice(0, TOP),
  };
}

/** `2026-10-03` as `Oct 3`. */
export const shortDay = (day: string) => parse(day).toLocaleString("en", { month: "short", day: "numeric" });
