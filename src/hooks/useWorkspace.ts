import { useCallback, useEffect, useRef, useState } from "react";
import {
  addWorkspace,
  closeWorkspace,
  currentWorkspace,
  forgetWorkspace,
  listWorkspaces,
  type RecentFolder,
  recentWorkspaces,
  restoreWorkspaces,
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

/** The manual order of the folder sections (FO-1) — a view state, kept in
 * window storage like a section's fold, not in the backend's open set: that
 * list also remembers which folder to reopen at launch (F-8.8), and a chat
 * switch moves its head, so reordering it would scramble the manual order on
 * the next switch. Here the two facts stay apart. */
const ORDER_KEY = "kibo-folder-order";
const isOrder = (value: unknown): value is string[] => Array.isArray(value) && value.every((path) => typeof path === "string");
function storedOrder(): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(ORDER_KEY) ?? "null");
    return isOrder(parsed) ? parsed : [];
  } catch {
    return [];
  }
}
function saveOrder(order: string[]) {
  try {
    localStorage.setItem(ORDER_KEY, JSON.stringify(order));
  } catch {
    // Only the next launch's order is lost.
  }
}

/**
 * The folders open in the window (F-8) and the active one — the folder of the
 * chat on screen, which the right panel and a new chat work in. Resolved by
 * the backend, displayed here.
 */
export function useWorkspace() {
  const [path, setPath] = useState<string | null>(null);
  const [folders, setFolders] = useState<string[]>([]);
  // An open folder that is a linked worktree → its main checkout.
  const [worktrees, setWorktrees] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<RecentFolder[]>([]);
  // Whether the folder is the one the app came back to rather than one the
  // user just chose: only then is its last conversation reopened too.
  const [resumed, setResumed] = useState(false);
  // The stored order mirrors the screen, but only once the screen has been
  // seeded from the backend: the empty list a window starts with is not an
  // order to remember.
  const seeded = useRef(false);
  useEffect(() => {
    if (seeded.current) saveOrder(folders);
  }, [folders]);

  /** The open set and the active mark as the backend has them now. */
  const reread = useCallback(async () => {
    const [open, active] = await Promise.all([listWorkspaces().catch(() => null), currentWorkspace().catch(() => null)]);
    setFolders((shown) => {
      const next = open ? keepOrder(shown.length ? shown : storedOrder(), open.map((folder) => folder.path)) : shown;
      // The active folder always has its section, even when the list could not be read.
      return active && !next.includes(active) ? [...next, active] : next;
    });
    if (open) seeded.current = true;
    if (open) setWorktrees(Object.fromEntries(open.flatMap((one) => (one.worktreeOf ? [[one.path, one.worktreeOf]] : []))));
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

  // A launch reopens every folder open last time, the folder of the chat
  // viewed last active (F-8.8); a reloaded window keeps the backend's. The
  // backend decides which: with every folder closed by hand the window starts
  // empty.
  useEffect(() => {
    void (async () => {
      const restored = await restoreWorkspaces().catch(() => null);
      recentWorkspaces().then(setRecent).catch(() => {});
      if (restored) {
        await reread();
        setResumed(true);
      }
    })();
  }, [reread]);

  /** Moves a folder one place up (-1) or down (+1) among the sections (FO-1);
   * at an edge, nothing. The order is the view's, kept in window storage. */
  const move = useCallback((folder: string, dir: -1 | 1) => {
    setFolders((shown) => {
      const i = shown.indexOf(folder);
      const j = i + dir;
      if (i < 0 || j < 0 || j >= shown.length) return shown;
      const next = [...shown];
      [next[i], next[j]] = [next[j], next[i]];
      return next;
    });
  }, []);

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

  return { path, folders, worktrees, error, recent, resumed, open, close, move, pick, refreshRecent, forget };
}
