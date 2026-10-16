import { useCallback, useEffect, useState } from "react";
import { currentWorkspace, forgetWorkspace, openWorkspace, recentWorkspaces, type RecentFolder } from "../lib/chat";
import { pickFolder } from "../lib/dialog";

/** The folder the agent acts on. Resolved by the backend, displayed here. */
export function useWorkspace() {
  const [path, setPath] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<RecentFolder[]>([]);
  // Whether the folder is the one the app came back to rather than one the
  // user just chose: only then is its last conversation reopened too.
  const [resumed, setResumed] = useState(false);

  const open = useCallback(async (next: string) => {
    try {
      setPath(await openWorkspace(next.trim()));
      setError(null);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      recentWorkspaces().then(setRecent).catch(() => {});
    }
  }, []);

  // A reloaded window asks the backend, which still has its folder; a new
  // launch reopens the folder opened last. One that is gone is not in the
  // list, and the window starts empty as it would have.
  useEffect(() => {
    void (async () => {
      const list = await recentWorkspaces().catch(() => []);
      setRecent(list);
      const current = await currentWorkspace().catch(() => null);
      if (current) {
        setPath(current);
        setResumed(true);
      } else if (list[0] && (await open(list[0].path))) {
        setResumed(true);
      }
    })();
  }, [open]);

  /** Asks for a folder and opens it. `false` also means "the user cancelled". */
  const pick = useCallback(async () => {
    const chosen = await pickFolder();
    if (!chosen) return false;
    return open(chosen);
  }, [open]);

  /** Reads the folder list again — after a folder in it is gone. */
  const refreshRecent = useCallback(() => {
    recentWorkspaces().then(setRecent).catch(() => {});
  }, []);

  /** Drops a folder from the list: left on disk, and left open if it is open. */
  const forget = useCallback(
    async (path: string) => {
      try {
        await forgetWorkspace(path);
        setError(null);
      } catch (e) {
        setError(String(e));
      } finally {
        refreshRecent();
      }
    },
    [refreshRecent],
  );

  return { path, error, recent, resumed, open, pick, refreshRecent, forget };
}
