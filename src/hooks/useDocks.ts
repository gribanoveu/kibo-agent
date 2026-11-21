import { useEffect } from "react";
import { isAsideTab, type AsideTab } from "../types";
import { ideDocks, isSavedDocks, openPane, togglePane, type Docks } from "../lib/docks";
import { nativeFrame, windowsFrame } from "../lib/window";
import { useMediaQuery } from "./useMediaQuery";
import { roomFor } from "./usePanelSizes";
import { useShortcuts } from "./useShortcuts";
import { isBoolean, useStoredState } from "./useStoredState";

/** A pane as the layout needs it: where its key and the menu open it. */
type PaneRef = { id: AsideTab; dock: "right" | "bottom" };

/** A stored pane id, if it still opens in `dock` — a pane may have moved docks since it was stored. */
const isPaneIn =
  (panes: readonly PaneRef[], dock: PaneRef["dock"]) =>
  (value: unknown): value is AsideTab =>
    isAsideTab(value) && panes.find((p) => p.id === value)?.dock === dock;
// Any pane may sit under the top one: Changes goes there when the top is taken.
const isBottomTab = (value: unknown): value is AsideTab | null => value === null || isAsideTab(value);

/**
 * How the window is laid out: the sidebar, the column of panes right of the
 * chat, the terminal under the message box, and the IDE layout's swap. What
 * fits a narrow window is derived, not stored — wider again, everything comes
 * back as it was left. Where each pane goes is `lib/docks.ts`'s rule; this
 * holds the answer and the room the window has for it.
 */
export function useDocks({
  agentMode,
  ideLayout,
  setIdeLayout,
  page,
  panes,
  viewerOpen,
  closeViewer,
  swapViewer,
  terminalPaste,
}: {
  agentMode: boolean;
  /** The IDE layout's flag, held by App: the pages' guard reads it too. */
  ideLayout: boolean;
  setIdeLayout: (next: boolean) => void;
  /** The open sidebar page, if any: one with a dock keeps the column beside it. */
  page: { dock: boolean } | null;
  panes: readonly PaneRef[];
  viewerOpen: boolean;
  closeViewer: () => void;
  /** Each layout keeps its own files open. */
  swapViewer: () => void;
  terminalPaste: { command: string } | null;
}) {
  const ide = agentMode && ideLayout;
  // Laid out as it was left.
  const [collapsed, setCollapsed] = useStoredState("atlas-sidebar-collapsed", false, isBoolean);
  // Hidden until the chat header's button asks for it.
  const [asideHidden, setAsideHidden] = useStoredState("atlas-aside-hidden", true, isBoolean);
  const [tab, setTab] = useStoredState<AsideTab>("atlas-aside-tab", "changes", isPaneIn(panes, "right"));
  // The terminal under the message box: closed when null.
  const [bottomTab, setBottomTab] = useStoredState<AsideTab | null>("atlas-bottom-tab", null, isBottomTab);
  // A window too narrow for every open panel at its minimum: the sidebar
  // falls back to its rail first, then the column right of the viewer hides.
  // Neither is stored — wider again, both come back as they were left.
  const frame = nativeFrame || windowsFrame ? 0 : 2;
  const dockFits = useMediaQuery(`(min-width: ${roomFor({ rail: true, viewer: viewerOpen, dock: true, frame, ide })}px)`);
  // What the column shows: what was opened, while it fits — beside an Agent
  // chat, or beside a page that keeps it.
  const docked = page ? page.dock : agentMode;
  const topHidden = asideHidden || !dockFits || !docked;
  // The terminal is not in the column: it opens under the message box, as wide
  // as the chat, so it needs no width of the window's own.
  const bottomShown = agentMode ? bottomTab : null;
  // The plan is read in a sheet over the chat (PlanSheet) rather than in the
  // column: it asks the window for none of the width the column needs, so it
  // opens wherever it fits, a narrow window included.
  const planSheet = agentMode && !page && !asideHidden && tab === "plan";
  // The column, and its handle: away while the plan's sheet has the corner.
  const dockShown = !topHidden && !planSheet;
  const sidebarFits = useMediaQuery(`(min-width: ${roomFor({ rail: false, viewer: viewerOpen, dock: dockShown, frame, ide })}px)`);
  const rail = collapsed || !sidebarFits;
  // On screen in either dock: what decides whether a pane's data is read.
  const shown = (pane: AsideTab) => (tab === pane && !topHidden) || bottomShown === pane;

  // The plan's sheet asks for no room, so a narrow window is no reason for its
  // key to think it is away: what the key toggles is the user's wish.
  const docks: Docks = { top: tab, topHidden: planSheet ? false : topHidden, bottom: bottomShown, swap: ide };
  const putDocks = (next: Docks) => {
    setTab(next.top);
    setAsideHidden(next.topHidden);
    setBottomTab(next.bottom);
  };
  const setDocks = (next: Docks) => {
    // A pane asked for where the viewer leaves the column no room: the viewer
    // gives way, or the pane would open out of sight. The plan opens in a sheet
    // over the chat, so it asks for none of that room.
    if (!dockFits && !next.topHidden && next.top !== "plan") closeViewer();
    putDocks(next);
  };
  // The tree is what the layout is for: it opens with it, in the dock now on the
  // left. Leaving puts back the panes the user had open — as asked for, not as
  // the window's width last let them show.
  const [beforeIde, setBeforeIde] = useStoredState<Docks | null>("atlas-docks-before-ide", null, isSavedDocks);
  // Not through `setDocks` either way: there a pane with no room closes the
  // files, and these are the layout's own, put away by the swap below.
  const toggleIde = () => {
    if (!ideLayout) {
      setBeforeIde({ top: tab, topHidden: asideHidden, bottom: bottomTab });
      putDocks(ideDocks(docks));
    } else if (beforeIde) {
      putDocks(beforeIde);
    }
    // Each layout keeps its own files open.
    swapViewer();
    setIdeLayout(!ideLayout);
  };
  const openTab = (next: AsideTab) =>
    setDocks(openPane(docks, next, panes.find((p) => p.id === next)?.dock ?? "right"));
  // A command sent to a shell shows the shell.
  useEffect(() => {
    if (terminalPaste) openTab("terminal");
    // Once per ask: not again when the layout changes.
  }, [terminalPaste]);

  // Held on its rail by the viewer beside the chat: opening it closes the viewer.
  const toggleSidebar = () => (rail && !collapsed ? closeViewer() : setCollapsed((v) => !v));

  // A pane's key shows it, or hides it where it already is (`lib/shortcuts.ts`).
  useShortcuts(
    Object.fromEntries(
      panes.map(({ id, dock }) => [
        id,
        () => {
          // Focus leaves the composer, so the shell that opens takes the typing.
          if (id === "terminal") (document.activeElement as HTMLElement | null)?.blur();
          setDocks(togglePane(docks, id, dock));
        },
      ]),
    ),
  );

  return {
    rail,
    tab,
    topHidden,
    bottomShown,
    planSheet,
    dockShown,
    docks,
    setDocks,
    openTab,
    toggleIde,
    toggleSidebar,
    shown,
    setCollapsed,
    hideAside: () => setAsideHidden(true),
    closeBottom: () => setBottomTab(null),
  };
}