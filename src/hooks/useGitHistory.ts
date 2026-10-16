import { useCallback, useEffect, useState } from "react";
import { gitHistory, type GitHistory, onGitChanged } from "../lib/git";

/** Commits asked for at a time: the first screen, then each "Load more". */
const PAGE = 50;

/**
 * The open folder's branch and recent commits, read while the History tab is
 * on screen: when it opens, and when the backend says `.git` changed — a
 * commit, a checkout or a fetch, from here or from a terminal.
 */
export function useGitHistory(visible: boolean, workspace: string | null) {
  const [limit, setLimit] = useState(PAGE);
  const [history, setHistory] = useState<GitHistory | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    () =>
      gitHistory(limit).then(
        (next) => {
          setHistory(next);
          setError(null);
        },
        (e) => {
          setHistory(null);
          setError(String(e));
        },
      ),
    [limit],
  );

  useEffect(() => {
    if (!visible) return;
    void load();
    let stop: (() => void) | undefined;
    let live = true;
    onGitChanged((root) => root === workspace && void load()).then((unlisten) => (live ? (stop = unlisten) : unlisten()));
    return () => {
      live = false;
      stop?.();
    };
  }, [visible, workspace, load]);

  return { history, error, loadMore: () => setLimit((n) => n + PAGE) };
}
