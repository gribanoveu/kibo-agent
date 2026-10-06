import { useEffect, useRef, useState } from "react";
import { fileView, onGitChanged, onIndexEvent, type FileTarget, type FileView } from "../lib/chat";

/**
 * The two versions of the file the viewer shows, read when it opens and again
 * when the backend says the folder changed — an edit arrives as an index
 * sync, staging as a `.git` change — so it follows the agent's edits.
 */
export function useFileView(target: FileTarget | null, workspace: string | null) {
  const [view, setView] = useState<FileView | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The read of the file showing now, for a caller that knows it changed.
  const reload = useRef<{ path: string; side: string; load: () => Promise<FileView | null> } | null>(null);
  const path = target?.path;
  const side = target?.side;

  useEffect(() => {
    if (!path || !side) return;
    let live = true;
    const load = () =>
      fileView({ path, side }).then(
        (next) => {
          if (!live) return null;
          setView(next);
          setError(null);
          return next;
        },
        (e) => {
          if (!live) return null;
          setView(null);
          setError(String(e));
          return null;
        },
      );
    reload.current = { path, side, load };
    setView(null);
    void load();
    const stops: (() => void)[] = [];
    const keep = (stop: () => void) => (live ? stops.push(stop) : stop());
    onIndexEvent((event) => event.root === workspace && event.kind === "syncStarted" && void load()).then(keep);
    onGitChanged((root) => root === workspace && void load()).then(keep);
    return () => {
      live = false;
      stops.forEach((stop) => stop());
    };
  }, [path, side, workspace]);

  /**
   * Reads `file` again, if it is the one showing: what was read, or `null`
   * when it could not be, or another file is showing by now.
   */
  const again = async (file: FileTarget) => {
    const shown = reload.current;
    return shown && shown.path === file.path && shown.side === file.side ? shown.load() : null;
  };
  return { view, error, reload: again };
}
