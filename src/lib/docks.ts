import type { AsideTab } from "../types";

/** What the column right of the chat shows: a pane on top (hidden or not) and one under it, or none. */
export type Docks = {
  top: AsideTab;
  topHidden: boolean;
  bottom: AsideTab | null;
  /** The docs layout: a pane for the top takes the top's place, as an editor's side bar switches views. */
  swap?: boolean;
};

/**
 * Opens a pane without closing the one already open: on top when the top is
 * free, otherwise under it — the top stays what the user opened first, and a
 * third pane takes the bottom's place. A bottom pane (Terminal) always goes
 * under. One already on screen stays where it is, so it is never drawn twice.
 */
export function openPane(docks: Docks, pane: AsideTab, dock: "right" | "bottom"): Docks {
  if (docks.bottom === pane || (docks.top === pane && !docks.topHidden)) return docks;
  if (dock === "bottom" || (!docks.topHidden && !docks.swap)) return { ...docks, bottom: pane };
  return { ...docks, top: pane, topHidden: false };
}

/** Entering the docs layout: the tree on top, and under the file only what goes there anyway. */
export const docsDocks = (docks: Docks): Docks => ({
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

export const toggleChanges = (docks: Docks) => togglePane(docks, "changes", "right");

export const changesShown = (docks: Docks) =>
  docks.bottom === "changes" || (docks.top === "changes" && !docks.topHidden);

/** The header's Terminal button: Terminal only ever sits in the bottom dock. */
export const toggleTerminal = (docks: Docks) => togglePane(docks, "terminal", "bottom");
