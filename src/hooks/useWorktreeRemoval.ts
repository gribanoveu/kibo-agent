import { useCallback, useState } from "react";
import { gitWorktreeCheck, gitWorktreeRemove, type WorktreeCheck, type WorktreeRemoved } from "../lib/git";

type Deps = {
  notify: (message: string) => void;
  /** The folder list, read again once the worktree is gone from it. */
  refreshRecent: () => void;
};

/** What the toast says once it is done. */
export function describeRemoval({ branchKept, chatsRemoved }: WorktreeRemoved) {
  const chats = chatsRemoved === 0 ? "" : chatsRemoved === 1 ? " with its chat" : ` with its ${chatsRemoved} chats`;
  const branch = branchKept ? ` — branch ${branchKept} kept` : "";
  return `Worktree removed${chats}${branch}`;
}

/**
 * Removing a worktree from the folder list: its state is read and shown
 * first, then the folder is deleted with its chats. Never the open folder —
 * the list offers that one's button disabled, and the backend refuses it.
 *
 * Nothing uncommitted is ever deleted: the backend refuses, and the dialog
 * says so before anyone clicks.
 */
export function useWorktreeRemoval({ notify, refreshRecent }: Deps) {
  const [asked, setAsked] = useState<{ path: string; check: WorktreeCheck } | null>(null);

  const ask = useCallback(
    async (path: string) => {
      try {
        setAsked({ path, check: await gitWorktreeCheck(path) });
      } catch (e) {
        notify(String(e));
      }
    },
    [notify],
  );

  const close = useCallback(() => setAsked(null), []);

  const confirm = useCallback(async () => {
    if (!asked) return;
    setAsked(null);
    try {
      notify(describeRemoval(await gitWorktreeRemove(asked.path)));
    } catch (e) {
      notify(String(e));
    } finally {
      refreshRecent();
    }
  }, [asked, notify, refreshRecent]);

  return { asked, ask, close, confirm };
}
