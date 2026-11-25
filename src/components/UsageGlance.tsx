import { useMemo } from "react";
import { useUsage } from "../hooks/useUsage";
import { tokensToday, usageStats } from "../lib/usage";
import { formatTokens } from "./ChatPanel";
import "./UsageGlance.css";

/**
 * The line under a new chat's box: what today has cost, what this week has
 * cost, and how long the run of days is — the numbers a working day is
 * measured by. Nothing at all until something has been counted, and no error
 * line here: this screen is about the next message, and the figures live in
 * Settings.
 */
export function UsageGlance() {
  const { rows } = useUsage();
  const today = useMemo(() => tokensToday(rows ?? [], new Date()), [rows]);
  // The calendar week — Monday to Sunday — not the last seven days: on a
  // Monday it starts again, as the week it is read in does.
  const week = useMemo(() => usageStats(rows ?? [], "week", new Date()), [rows]);
  // The run is counted over everything, not over the week: a streak longer than
  // the range the total is taken from still stands.
  const run = useMemo(() => usageStats(rows ?? [], "all", new Date()), [rows]);
  if (!rows?.length) return null;

  const parts = [
    today > 0 ? `today ${formatTokens(today)}` : null,
    `${formatTokens(week.total)} this week`,
    run.currentStreak > 1 ? `${run.currentStreak}-day streak` : null,
  ].filter((part): part is string => part !== null);
  return (
    <p className="usage-glance">
      {parts.map((part) => (
        <span key={part}>{part}</span>
      ))}
    </p>
  );
}
