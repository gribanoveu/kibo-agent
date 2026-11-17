import { useCallback, useEffect, useState } from "react";
import {
  addWorkspace,
  closeWorkspace,
  currentWorkspace,
  forgetWorkspace,
  listWorkspaces,
  type RecentFolder,
  recentWorkspaces,
} from "../lib/chat";
import { pickFolder } from "../lib/dialog";

/**
 * Where the sidebar's sections stay put: a folder keeps its place, and one
 * opened since goes at the end — the backend lists the last activated first,
 * and a list that reordered whenever a chat of another folder was opened
 * would move under the pointer.
 */
export function keepOrder(shown: string[], open: string[]): string[] {
  const kept = shown.filter((path) => open.includes(path));
  const added = open.filter((path) => !shown.includes(path)).reverse();
  return [...kept, ...added];
}

/**
 * The folders open in the window (F-8) and the active one — the folder of the
 * chat on screen, which the right panel and a new chat work in. Resolved by
 * the backend, displayed here.
 */
export function useWorkspace() {
  const [path, setPath] = useState<string | null>(null);
  const [folders, setFolders] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<RecentFolder[]>([]);
  // Whether the folder is the one the app came back to rather than one the
  // user just chose: only then is its last conversation reopened too.
  const [resumed, setResumed] = useState(false);

  /** The open set and the active mark as the backend has them now. */
  const reread = useCallback(async () => {
    const [open, active] = await Promise.all([listWorkspaces().catch(() => null), currentWorkspace().catch(() => null)]);
    setFolders((shown) => {
      const next = open ? keepOrder(shown, open.map((folder) => folder.path)) : shown;
      // The active folder always has its section, even when the list could not be read.
      return active && !next.includes(active) ? [...next, active] : next;
    });
    setPath(active);
  }, []);

  /**
   * Opens a folder beside the others, or makes an open one active. Nothing
   * of another folder stops: closing is `close`.
   */
  const open = useCallback(
    async (next: string) => {
      try {
        setPath(await addWorkspace(next.trim()));
        setError(null);
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      } finally {
        recentWorkspaces().then(setRecent).catch(() => {});
        await reread();
      }
    },
    [reread],
  );

  /** Closes a folder: its processes, shells, servers and index stop; the next open one becomes active. */
  const close = useCallback(
    async (folder: string) => {
      try {
        await closeWorkspace(folder);
        setError(null);
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      } finally {
        await reread();
      }
    },
    [reread],
  );

  // A reloaded window asks the backend, which still has its folders; a new
  // launch reopens the folder opened last. One that is gone is not in the
  // list, and the window starts empty as it would have.
  useEffect(() => {
    void (async () => {
      const list = await recentWorkspaces().catch(() => []);
      setRecent(list);
      const current = await currentWorkspace().catch(() => null);
      if (current) {
        await reread();
        setResumed(true);
      } else if (list[0] && (await open(list[0].path))) {
        setResumed(true);
      }
    })();
  }, [open, reread]);

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

  return { path, folders, error, recent, resumed, open, close, pick, refreshRecent, forget };
}
