import { useEffect, useState } from "react";
import { dailyMetrics, METRICS, type DailyMetric } from "../lib/usage";

/**
 * Settings → Usage: every counter of every day, read once when the
 * pane opens. Nothing announces a new count — the pane is a look back, and
 * opening it again reads again.
 */
export function useUsage() {
  const [rows, setRows] = useState<DailyMetric[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    dailyMetrics(METRICS).then(setRows, (e) => setError(String(e)));
  }, []);

  return { rows, error };
}
