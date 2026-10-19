import { useEffect, useRef, useState } from "react";
import { indexStatus, onIndexEvent } from "../lib/chat";
import { applyIndexEvent, fromSnapshot, type IndexState } from "../lib/indexStatus";

/**
 * The index of the folder at `path`, kept current from its events.
 *
 * Listens from mount, for every folder: the first sync starts inside
 * `workspace_open`, before the window learns the path, and on a small folder
 * it is over by then. The snapshot covers a window that was not listening at
 * all — a reload — and never overrides what an event has already said.
 *
 * The events carry the state rather than only signal a change — progress is
 * a stream, like a reply's deltas (AGENTS.md, "Keeping the frontend
 * current"). What makes that safe is the order: the snapshot is read only
 * once the listener is attached. Read before, a sync ending in between would
 * be said by neither — the snapshot still syncing, its `syncFinished` heard
 * by nobody — and the badge would stay on "Indexing…" until the next sync.
 */
export function useIndexStatus(path: string | null): IndexState | null {
  const [byRoot, setByRoot] = useState<Record<string, IndexState>>({});
  const attached = useRef<{ promise: Promise<void>; resolve: () => void } | null>(null);
  if (!attached.current) {
    let resolve = () => {};
    const promise = new Promise<void>((done) => (resolve = done));
    attached.current = { promise, resolve };
  }

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let live = true;
    onIndexEvent((event) =>
      setByRoot((known) => ({ ...known, [event.root]: applyIndexEvent(known[event.root], event) })),
    ).then((stop) => {
      if (!live) return stop();
      unlisten = stop;
      attached.current?.resolve();
    });
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!path) return;
    let live = true;
    attached.current?.promise
      .then(indexStatus)
      .then((snapshot) => {
        if (!live || !snapshot) return;
        setByRoot((known) =>
          known[snapshot.root] ? known : { ...known, [snapshot.root]: fromSnapshot(snapshot) },
        );
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [path]);

  return path ? (byRoot[path] ?? null) : null;
}
