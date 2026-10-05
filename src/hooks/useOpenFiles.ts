import { useEffect, useMemo, useReducer } from "react";
import type { FileTarget } from "../lib/chat";

/**
 * The viewer's tabs, in the order they were opened, and the one showing.
 * `preview` is the tab a single click opened, which the next single click
 * reuses — clicking down a list leaves one tab, not one per file — until it
 * is pinned by a double click.
 */
export type OpenFiles = { files: FileTarget[]; active: FileTarget | null; preview: FileTarget | null };

type Action =
  | { kind: "open"; target: FileTarget; pin: boolean }
  | { kind: "pin"; target: FileTarget }
  | { kind: "close"; target: FileTarget }
  | { kind: "closeAll" };

/** The same file on the same side: one tab, however often it is opened. */
export const sameFile = (a: FileTarget, b: FileTarget) => a.path === b.path && a.side === b.side;

/**
 * The file `step` places from `current` in `list`, going round at the ends.
 * From a file not in the list, forward is the first and back the last.
 */
export function stepThrough(list: FileTarget[], current: FileTarget, step: 1 | -1): FileTarget | null {
  if (list.length === 0) return null;
  const at = list.findIndex((f) => sameFile(f, current));
  if (at < 0) return step > 0 ? list[0] : list[list.length - 1];
  return list[(at + step + list.length) % list.length];
}

const none: OpenFiles = { files: [], active: null, preview: null };
const unpin = (preview: FileTarget | null, target: FileTarget) => (preview && sameFile(preview, target) ? null : preview);

/**
 * Opening a file already open shows its tab, and pins it if asked. A new file
 * opened to pin gets a tab of its own; opened with a single click it takes
 * the preview tab's place. Closing the tab showing moves to the one after it,
 * or before it when it was the last.
 */
export function openFilesReducer(state: OpenFiles, action: Action): OpenFiles {
  switch (action.kind) {
    case "open": {
      const open = state.files.find((f) => sameFile(f, action.target));
      if (open) return { ...state, active: open, preview: action.pin ? unpin(state.preview, open) : state.preview };
      const target = action.target;
      if (action.pin) return { ...state, files: [...state.files, target], active: target };
      const files = state.preview
        ? state.files.map((f) => (sameFile(f, state.preview!) ? target : f))
        : [...state.files, target];
      return { files, active: target, preview: target };
    }
    case "pin":
      return { ...state, preview: unpin(state.preview, action.target) };
    case "close": {
      const at = state.files.findIndex((f) => sameFile(f, action.target));
      if (at < 0) return state;
      const files = state.files.filter((_, i) => i !== at);
      const showing = state.active && sameFile(state.active, action.target);
      return {
        files,
        active: showing ? (files[at] ?? files[at - 1] ?? null) : state.active,
        preview: unpin(state.preview, action.target),
      };
    }
    case "closeAll":
      return none;
  }
}

/**
 * The tabs on screen and the other layout's, put away: the IDE layout and the
 * chat's each keep their own, and switching swaps them. A folder opened
 * elsewhere empties both — "Close all" only the ones on screen.
 */
type Viewer = { shown: OpenFiles; other: OpenFiles };

export function viewerReducer(state: Viewer, action: Action | { kind: "swap" } | { kind: "reset" }): Viewer {
  switch (action.kind) {
    case "swap":
      return { shown: state.other, other: state.shown };
    case "reset":
      return { shown: none, other: none };
    default: {
      const shown = openFilesReducer(state.shown, action);
      return shown === state.shown ? state : { ...state, shown };
    }
  }
}

/** The files open in the viewer beside the chat; a folder opened elsewhere closes them all. */
export function useOpenFiles(workspace: string | null) {
  const [state, dispatch] = useReducer(viewerReducer, { shown: none, other: none });
  useEffect(() => dispatch({ kind: "reset" }), [workspace]);
  // Stable, so a callback built on them does not re-render every answer.
  const actions = useMemo(
    () => ({
      /** A single click previews; `pin`, from a double click, keeps the tab. */
      open: (target: FileTarget, pin = false) => dispatch({ kind: "open", target, pin }),
      pin: (target: FileTarget) => dispatch({ kind: "pin", target }),
      close: (target: FileTarget) => dispatch({ kind: "close", target }),
      closeAll: () => dispatch({ kind: "closeAll" }),
      /** Shows the other layout's tabs, putting these away until the next swap. */
      swap: () => dispatch({ kind: "swap" }),
    }),
    [],
  );
  return { ...state.shown, ...actions };
}

/**
 * The path in the open folder a link in an answer names — `src/a.ts`,
 * `./src/a.ts`, the absolute path, `file://` — or `null` for one outside it.
 * A line reference (`:42`, `:42:7`, `#L42`) is dropped: the viewer opens the file.
 */
export function fileLinkPath(link: string, workspace: string): string | null {
  let path = link.replace(/^file:\/\//, "").replace(/#.*$/, "").replace(/(:\d+)+$/, "");
  try {
    path = decodeURIComponent(path);
  } catch {
    return null;
  }
  const root = workspace.replace(/\/+$/, "") + "/";
  if (path.startsWith(root)) path = path.slice(root.length);
  const parts = path.split("/").filter((part) => part !== "" && part !== ".");
  if (path.startsWith("/") || parts.length === 0 || parts.includes("..")) return null;
  return parts.join("/");
}
