import { useEffect, useState } from "react";
import { type ChangeTotals, gitTotals, onFolderChanged } from "../lib/chat";

/**
 * Lines added and removed in the open folder since the last commit, for the
 * chat header. Read when the folder opens and whenever the backend says it
 * changed — the same two signals the Changes tab follows. `null` outside a
 * repository, or when it cannot be read: the header then shows nothing.
 */
export function useChangeTotals(workspace: string | null): ChangeTotals | null {
  const [totals, setTotals] = useState<ChangeTotals | null>(null);

  useEffect(() => {
    setTotals(null);
    if (!workspace) return;
    let live = true;
    const load = () =>
      gitTotals().then(
        (next) => live && setTotals(next),
        () => live && setTotals(null),
      );
    void load();
    let unlisten: (() => void) | undefined;
    onFolderChanged(workspace, () => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [workspace]);

  return totals;
}
