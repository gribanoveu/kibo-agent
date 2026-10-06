import { useCallback, useRef } from "react";
import { useStoredState } from "./useStoredState";

type PanelKey = "sidebar" | "aside" | "bottom" | "viewer" | "chat";
const KEYS: PanelKey[] = ["sidebar", "aside", "bottom", "viewer", "chat"];
const zeros = (): Record<PanelKey, number> => ({ sidebar: 0, aside: 0, bottom: 0, viewer: 0, chat: 0 });

// The minimums mirror the CSS (Sidebar.css, AsidePanel.css, FileViewer.css),
// and each is what the panel's widest row needs on one line.
export const PANEL_LIMITS: Record<PanelKey, { min: number; max: number; initial: number; rail?: number }> = {
  // `rail` is the collapsed width. The side panel has none: it is hidden from
  // the chat header, never by dragging.
  sidebar: { min: 180, max: 420, initial: 248, rail: 58 },
  // Changes' "Generate description" and "Commit" side by side: 246.
  aside: { min: 260, max: 560, initial: 300 },
  // A height: the lower pane of the right column. Like the side panel it stops
  // at its minimum — it closes from its own button, never by dragging.
  bottom: { min: 120, max: 640, initial: 240 },
  // The file viewer beside the chat; it closes from its own button. Its
  // heading's badge, counts, Diff/File/Preview and the arrows take 400; the
  // rest is the path's.
  viewer: { min: 440, max: 1400, initial: 560 },
  // The chat right of the file in the IDE layout. Elsewhere it takes what the
  // panels leave, and only its minimum counts.
  chat: { min: 400, max: 1000, initial: 480 },
};

/** The chat's minimum (`.main` in App.css): it never gives up width to the panels beside it. */
export const CHAT_MIN = PANEL_LIMITS.chat.min;
/** `.body`'s padding and each resize handle: the gaps between panels are this wide. */
const GAP = 10;

/**
 * How wide the window must be to hold these panels side by side at their
 * minimums: the sidebar (or its rail), the chat, and the file viewer and the
 * column right of it when they are open. `frame` is the window's own border,
 * both sides — none where the OS draws it. The IDE layout has no sidebar, nor
 * the gap after it: the chat header holds the list of chats.
 */
export function roomFor({ rail, viewer, dock, frame, ide = false }: { rail: boolean; viewer: boolean; dock: boolean; frame: number; ide?: boolean }) {
  const sidebar = ide ? 0 : (rail ? (PANEL_LIMITS.sidebar.rail ?? 0) : PANEL_LIMITS.sidebar.min) + GAP;
  return (
    frame +
    2 * GAP +
    sidebar +
    CHAT_MIN +
    (viewer ? GAP + PANEL_LIMITS.viewer.min : 0) +
    (dock ? GAP + PANEL_LIMITS.aside.min : 0)
  );
}

const INITIAL = Object.fromEntries(KEYS.map((key) => [key, PANEL_LIMITS[key].initial])) as Record<PanelKey, number>;

/** How far past the minimum the drag must continue before the panel snaps shut. */
const COLLAPSE_OVERSHOOT_RATIO = 0.4;
/** How far a collapsed panel must be pulled out before it opens again. */
const EXPAND_THRESHOLD = 40;

// Within the limits too: they may have changed since the width was stored. A
// panel added since is missing, and starts at its initial size.
const isWidths = (value: unknown): value is Partial<Record<PanelKey, number>> => {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<PanelKey, unknown>;
  return KEYS.every(
    (key) =>
      v[key] === undefined ||
      (typeof v[key] === "number" && v[key] >= PANEL_LIMITS[key].min && v[key] <= PANEL_LIMITS[key].max),
  );
};

export type PanelControl = {
  collapsed: boolean;
  collapse: () => void;
  expand: () => void;
};

const clamp = (key: PanelKey, width: number) =>
  Math.min(PANEL_LIMITS[key].max, Math.max(PANEL_LIMITS[key].min, width));

/**
 * Panel widths driven by the edge handles. Dragging below the minimum holds the
 * panel at that minimum and banks the extra movement; once it adds up the panel
 * collapses. Pulling a collapsed panel outward reopens it the same way — the
 * drag-to-collapse gesture from docflow's usePanelLayout, plus the way back.
 * A panel without a control only stops at its minimum.
 */
export function usePanelSizes(controls: Partial<Record<PanelKey, PanelControl>>) {
  const [stored, setWidths] = useStoredState<Partial<Record<PanelKey, number>>>("atlas-panel-widths", INITIAL, isWidths);
  const widths: Record<PanelKey, number> = { ...INITIAL, ...stored };
  const widthsRef = useRef(widths);
  const overshoot = useRef(zeros());
  // Distance the pointer still has to travel before it meets the panel edge
  // again. A panel reopens at its minimum width, which is wider than the rail
  // the pointer just left — without this it would keep widening from under the
  // cursor instead of waiting for it.
  const catchUp = useRef(zeros());
  const controlsRef = useRef(controls);
  controlsRef.current = controls;

  const apply = useCallback((key: PanelKey, width: number) => {
    const next = { ...widthsRef.current, [key]: clamp(key, width) };
    widthsRef.current = next;
    setWidths(next);
  }, []);

  const resize = useCallback(
    (key: PanelKey, delta: number) => {
      const { min } = PANEL_LIMITS[key];
      const control = controlsRef.current[key];

      if (control?.collapsed) {
        if (delta <= 0) return;
        overshoot.current[key] += delta;
        if (overshoot.current[key] >= EXPAND_THRESHOLD) {
          const travelled = overshoot.current[key];
          overshoot.current[key] = 0;
          catchUp.current[key] = Math.max(0, min - (PANEL_LIMITS[key].rail ?? 0) - travelled);
          apply(key, min);
          control.expand();
        }
        return;
      }

      if (catchUp.current[key] > 0) {
        // Hold the width until the pointer has caught up with the edge.
        if (delta <= 0) {
          catchUp.current[key] -= delta;
          return;
        }
        const paid = Math.min(delta, catchUp.current[key]);
        catchUp.current[key] -= paid;
        delta -= paid;
        if (delta === 0) return;
      }

      const current = widthsRef.current[key];

      if (delta > 0) {
        overshoot.current[key] = 0;
        apply(key, current + delta);
        return;
      }

      const next = current + delta;
      if (next > min) {
        overshoot.current[key] = 0;
        apply(key, next);
        return;
      }

      apply(key, min);
      if (!control) return;
      overshoot.current[key] += current <= min ? -delta : min - next;
      if (overshoot.current[key] >= min * COLLAPSE_OVERSHOOT_RATIO) {
        overshoot.current[key] = 0;
        catchUp.current[key] = 0;
        control.collapse();
      }
    },
    [apply],
  );

  return {
    widths,
    resizeSidebarBy: useCallback((delta: number) => resize("sidebar", delta), [resize]),
    resizeAsideBy: useCallback((delta: number) => resize("aside", delta), [resize]),
    resizeBottomBy: useCallback((delta: number) => resize("bottom", delta), [resize]),
    resizeViewerBy: useCallback((delta: number) => resize("viewer", delta), [resize]),
    resizeChatBy: useCallback((delta: number) => resize("chat", delta), [resize]),
    /**
     * The drag is over. `size` is how large the panel came out: when the
     * window had no room for what was dragged, the panel stopped short, and
     * its width is brought down to that — or the next drag would first have
     * to take back width that never showed.
     */
    endResize: useCallback(
      (key?: PanelKey, size?: number) => {
        overshoot.current = zeros();
        catchUp.current = zeros();
        if (!key || size === undefined || controlsRef.current[key]?.collapsed) return;
        if (size < widthsRef.current[key] - 0.5) apply(key, Math.round(size));
      },
      [apply],
    ),
  };
}
