import { useEffect, useState } from "react";

/**
 * A list the backend owns, read while `on`: when it starts, and again each
 * time `listen`'s event says it changed — the event is a signal to read
 * again, never the state itself (AGENTS.md, "Keeping the frontend current").
 * A read that lands after it stopped listening is dropped. `setValue` and
 * `setError` are for the hook's own actions, which answer with the new list.
 */
export function useLiveRead<T>(
  read: () => Promise<T>,
  listen: (changed: () => void) => Promise<() => void>,
  initial: T,
  on = true,
) {
  const [value, setValue] = useState<T>(initial);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!on) return;
    let live = true;
    const load = () =>
      read().then(
        (next) => {
          if (!live) return;
          setValue(next);
          setError(null);
        },
        (e) => live && setError(String(e)),
      );
    void load();
    let unlisten: (() => void) | undefined;
    listen(() => void load()).then((off) => (live ? (unlisten = off) : off()));
    return () => {
      live = false;
      unlisten?.();
    };
  }, [on]);
  return { value, setValue, error, setError };
}
