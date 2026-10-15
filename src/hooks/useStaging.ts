import { useCallback, useEffect, useState } from "react";
import { gitChanges, gitCommit, gitCommitMessage, gitStage, gitUnstage, onFolderChanged, type WorkingChanges } from "../lib/git";

const none: WorkingChanges = { staged: [], unstaged: [] };

/**
 * The open folder's staged and unstaged files, read while the Changes tab is
 * on screen: when it opens, after each of its own actions, and when the
 * backend says the folder changed — an edit to the tree starts an index sync,
 * and a change to `.git` has its own event.
 */
export function useStaging(visible: boolean, workspace: string | null) {
  const [changes, setChanges] = useState<WorkingChanges>(none);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    () =>
      gitChanges().then(
        (next) => {
          setChanges(next);
          setError(null);
        },
        (e) => {
          setChanges(none);
          setError(String(e));
        },
      ),
    [],
  );

  useEffect(() => {
    if (!visible) return;
    void load();
    let unlisten: (() => void) | undefined;
    let live = true;
    onFolderChanged(workspace, () => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [visible, workspace, load]);

  // Every action reads the status back rather than guessing it: staging a
  // file with edits on both sides, say, leaves it in one list, not two.
  const act = useCallback(
    async <T,>(op: () => Promise<T>): Promise<T> => {
      try {
        return await op();
      } finally {
        await load();
      }
    },
    [load],
  );

  return {
    ...changes,
    error,
    stage: (paths: string[]) => act(() => gitStage(paths)),
    unstage: (paths: string[]) => act(() => gitUnstage(paths)),
    commit: (message: string) => act(() => gitCommit(message)),
    // Changes nothing, so nothing to read back.
    describe: (draft: string) => gitCommitMessage(draft),
  };
}
