import { isAsideTab, type AsideTab } from "../types";

/** What the window shows: a pane in the column right of the chat (hidden or not), and the terminal under the message box, or none. */
export type Docks = {
  top: AsideTab;
  topHidden: boolean;
  bottom: AsideTab | null;
  /** The IDE layout: a pane for the top takes the top's place, as an editor's side bar switches views. */
  swap?: boolean;
};

/**
 * Opens a pane. A pane of the column right of the chat takes it, closing what
 * was there and closed in turn by the next one. The terminal is not in that
 * column: it opens under the message box, beside whatever the column shows, and
 * closes on its own. One already on screen stays where it is, so it is never
 * drawn twice. In the IDE layout a pane takes the top's place.
 */
export function openPane(docks: Docks, pane: AsideTab, dock: "right" | "bottom"): Docks {
  if (docks.bottom === pane || (docks.top === pane && !docks.topHidden)) return docks;
  if (dock === "bottom") return { ...docks, bottom: pane };
  return { ...docks, top: pane, topHidden: false };
}

/** Entering the IDE layout: the tree on top, and under the message box only what goes there anyway. */
export const ideDocks = (docks: Docks): Docks => ({
  top: "files",
  topHidden: false,
  bottom: docks.bottom === "terminal" ? "terminal" : null,
});

/** A pane's button or shortcut: shown anywhere, it hides it; otherwise it opens it like the menu would. */
export function togglePane(docks: Docks, pane: AsideTab, dock: "right" | "bottom"): Docks {
  if (docks.bottom === pane) return { ...docks, bottom: null };
  if (docks.top === pane && !docks.topHidden) return { ...docks, topHidden: true };
  return openPane(docks, pane, dock);
}

/** The header's Terminal button: the terminal only ever opens under the message box. */
export const toggleTerminal = (docks: Docks) => togglePane(docks, "terminal", "bottom");

/** The panes as they were before the IDE layout, kept to go back to: stored, so only a whole one is trusted. */
export const isSavedDocks = (value: unknown): value is Docks | null => {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const v = value as Docks;
  return isAsideTab(v.top) && typeof v.topHidden === "boolean" && (v.bottom === null || isAsideTab(v.bottom));
};
