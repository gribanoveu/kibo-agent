import { useEffect, useRef } from "react";
import { matches, type ShortcutId } from "../lib/shortcuts";

/**
 * Runs a handler when its shortcut is pressed anywhere in the window. Caught
 * in the capture phase, before whatever has focus sees it — the terminal
 * would otherwise send Ctrl+` to the shell as a NUL.
 *
 * Handlers are read at the moment of the press, so they may close over
 * this render's state without the listener being set up again. One that
 * returns `false` declines: the key goes on to whatever has focus.
 */
export function useShortcuts(handlers: Partial<Record<ShortcutId, () => void | false>>) {
  const current = useRef(handlers);
  current.current = handlers;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      for (const [id, run] of Object.entries(current.current)) {
        if (!run || !matches(e, id as ShortcutId)) continue;
        // Held down, a toggle would flicker.
        if (!e.repeat && run() === false) return;
        e.preventDefault();
        e.stopPropagation();
        return;
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
}
