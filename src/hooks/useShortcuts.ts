import { useEffect, useRef } from "react";
import { holdShortcuts, matches, shortcutsHeld, type ShortcutId } from "../lib/shortcuts";

/**
 * Runs a handler when its shortcut is pressed anywhere in the window. Caught
 * in the capture phase, before whatever has focus sees it — the terminal
 * would otherwise send Ctrl+` to the shell as a NUL.
 *
 * Handlers are read at the moment of the press, so they may close over
 * this render's state without the listener being set up again.
 */
export function useShortcuts(handlers: Partial<Record<ShortcutId, () => void>>) {
  const current = useRef(handlers);
  current.current = handlers;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // A layer over the window — the first run — keeps the keys to itself.
      if (shortcutsHeld()) return;
      for (const [id, run] of Object.entries(current.current)) {
        if (!run || !matches(e, id as ShortcutId)) continue;
        e.preventDefault();
        e.stopPropagation();
        // Held down, a toggle would flicker.
        if (!e.repeat) run();
        return;
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);
}

/**
 * Holds the window's keys while `on` — for a layer that owns the window, so a
 * key it hides cannot act, nor park a dialog to appear after it is gone.
 */
export function useHeldShortcuts(on = true) {
  useEffect(() => (on ? holdShortcuts() : undefined), [on]);
}
