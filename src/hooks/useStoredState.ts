import { useEffect, useState } from "react";

/**
 * `useState` that the next launch starts from — for how the window was laid
 * out and conveniences like a query history, nothing more. Browser storage is
 * this window's own and may be empty or refuse (a cleared profile, a full
 * disk): then the default stands, and nothing depends on the value surviving.
 *
 * `accept` screens what comes back: a value stored by an older build — a tab
 * since renamed, a width of the wrong type — is the default, not a broken
 * layout.
 */
export function useStoredState<T>(key: string, initial: T, accept: (value: unknown) => value is T) {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      const parsed: unknown = raw === null ? undefined : JSON.parse(raw);
      return accept(parsed) ? parsed : initial;
    } catch {
      return initial;
    }
  });

  useEffect(() => {
    try {
      localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Only next launch's layout is lost.
    }
  }, [key, value]);

  return [value, setValue] as const;
}

export const isBoolean = (value: unknown): value is boolean => typeof value === "boolean";
