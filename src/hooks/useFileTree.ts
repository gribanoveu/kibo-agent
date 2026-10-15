import { useCallback, useEffect, useRef, useState } from "react";
import { type FolderListing, onFolderChanged, workspaceList } from "../lib/chat";

/**
 * The open folder as a tree, one folder read at a time as it is unfolded —
 * a run of folders each holding only the next unfolds in one go.
 * Read while the Files tab is on screen: when it opens, and — the root and
 * every unfolded folder again — when the backend says the folder changed.
 */
export function useFileTree(visible: boolean, workspace: string | null) {
  const [listings, setListings] = useState<Record<string, FolderListing>>({});
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  // Read by the event handlers, which would otherwise see the set as it was
  // when they subscribed.
  const open = useRef(expanded);
  open.current = expanded;

  // Which folder a reply is for: one that arrives after another folder was
  // opened belongs to the old tree.
  const current = useRef(workspace);
  current.current = workspace;

  /**
   * Reads one folder. Unfolding one (`chain`) goes on down while a folder
   * holds a single folder and nothing else — `src/main/java/com/example` is
   * one click, not five — and stops where there is a file or a choice.
   */
  const load = useCallback((dir: string, chain = false) => {
    const asked = current.current;
    workspaceList(dir).then(
      (listing) => {
        if (current.current !== asked) return;
        setListings((known) => ({ ...known, [dir]: listing }));
        setError(null);
        const only = listing.entries.length === 1 && listing.more === 0 ? listing.entries[0] : null;
        if (chain && only?.isDir) {
          setExpanded((now) => new Set(now).add(only.path));
          load(only.path, true);
        }
      },
      (e) => current.current === asked && setError(String(e)),
    );
  }, []);

  // Another folder is another tree.
  useEffect(() => {
    setListings({});
    setExpanded(new Set());
  }, [workspace]);

  useEffect(() => {
    if (!visible || !workspace) return;
    const reload = () => ["", ...open.current].forEach((dir) => load(dir));
    reload();
    let unlisten: (() => void) | undefined;
    let live = true;
    onFolderChanged(workspace, () => reload()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [visible, workspace, load]);

  const toggle = useCallback(
    (dir: string) => {
      const opening = !open.current.has(dir);
      if (opening) load(dir, true);
      setExpanded((now) => {
        const next = new Set(now);
        if (opening) next.add(dir);
        else next.delete(dir);
        return next;
      });
    },
    [load],
  );

  return { listings, expanded, error, toggle };
}
