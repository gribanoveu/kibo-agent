import { useCallback, useEffect, useState } from "react";
import { setToolLogEnabled, toolLogClear, toolLogEnabled, type ToolLogFilter, toolLogQuery, type ToolLogRow } from "../lib/toolLog";

export const PAGE = 100;

/**
 * The log window's state: a filter, the rows it matched so far, and the
 * switch. Read while `open` — the log grows during a turn, so each opening
 * and each filter change asks again rather than keeping a stale page.
 */
export function useToolLog(open: boolean) {
  const [filter, setFilter] = useState<ToolLogFilter>({});
  const [rows, setRows] = useState<ToolLogRow[]>([]);
  const [total, setTotal] = useState(0);
  const [enabled, setEnabled] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (offset: number) => {
    try {
      const page = await toolLogQuery({ ...filter, limit: PAGE, offset });
      setRows((known) => (offset === 0 ? page.rows : [...known, ...page.rows]));
      setTotal(page.total);
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, [filter]);

  useEffect(() => {
    if (open) load(0);
  }, [open, load]);

  useEffect(() => {
    if (open) toolLogEnabled().then(setEnabled, (e) => setError(String(e)));
  }, [open]);

  const toggle = useCallback(async (next: boolean) => {
    try {
      await setToolLogEnabled(next);
      setEnabled(next);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const clear = useCallback(async () => {
    try {
      await toolLogClear();
      await load(0);
    } catch (e) {
      setError(String(e));
    }
  }, [load]);

  return {
    filter,
    setFilter,
    rows,
    total,
    more: () => load(rows.length),
    enabled,
    toggle,
    clear,
    error,
  };
}
