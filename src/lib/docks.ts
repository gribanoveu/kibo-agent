import { isAsideTab, type AsideTab } from "../types";

/** What the column right of the chat shows: a pane on top (hidden or not) and one under it, or none. */
export type Docks = {
  top: AsideTab;
  topHidden: boolean;
  bottom: AsideTab | null;
  /** The IDE layout: a pane for the top takes the top's place, as an editor's side bar switches views. */
  swap?: boolean;
};

/** One panel in two parts: Overview on top, Terminal under it. Every other pane is a panel of its own. */
const paired = (pane: AsideTab | null) => pane === "overview" || pane === "terminal";

/**
 * Opens a pane. Overview and Terminal open beside each other, Overview on top
 * and Terminal under it; any other pane takes the whole column, closing what
 * was there, and is closed in turn by the next one — Terminal included. One
 * already on screen stays where it is, so it is never drawn twice. In the IDE
 * layout a pane takes the top's place and Terminal goes under the file.
 */
export function openPane(docks: Docks, pane: AsideTab, dock: "right" | "bottom"): Docks {
  if (docks.bottom === pane || (docks.top === pane && !docks.topHidden)) return docks;
  if (dock === "bottom") return { ...docks, bottom: pane, topHidden: docks.topHidden || (!docks.swap && !paired(docks.top)) };
  if (docks.swap || paired(pane)) return { ...docks, top: pane, topHidden: false, bottom: docks.swap || paired(docks.bottom) ? docks.bottom : null };
  return { top: pane, topHidden: false, bottom: null };
}

/** Entering the IDE layout: the tree on top, and under the file only what goes there anyway. */
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

/** The header's Terminal button: Terminal only ever sits in the bottom dock. */
export const toggleTerminal = (docks: Docks) => togglePane(docks, "terminal", "bottom");

/** The panes as they were before the IDE layout, kept to go back to: stored, so only a whole one is trusted. */
export const isSavedDocks = (value: unknown): value is Docks | null => {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const v = value as Docks;
  return isAsideTab(v.top) && typeof v.topHidden === "boolean" && (v.bottom === null || isAsideTab(v.bottom));
};
