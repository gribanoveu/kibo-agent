import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { inTauri } from "./chat";

// The window is undecorated and transparent (tauri.conf.json), so the app's own
// titlebar drives it. Components call these wrappers, never the API directly.
// Outside Tauri (plain `bun run dev`) there is no window to drive — no-op instead
// of throwing, so the UI stays previewable in a browser.

// On macOS the window keeps its native frame with the title bar hidden
// (tauri.macos.conf.json): the OS draws the traffic lights, the corners and the
// edges a resize grabs — an undecorated window there leaves a pixel-thin edge.
export const nativeFrame = inTauri() && navigator.userAgent.includes("Mac");

// On Windows the window is undecorated but opaque (tauri.windows.conf.json): a
// transparent WebView2 paints its clear pixels black, which showed as a dark
// frame round the rounded corners. The system's shadow rounds the corners and
// draws the edge there, and the caption buttons are Windows' own shape, on the right.
export const windowsFrame = inTauri() && navigator.userAgent.includes("Windows");

export const minimizeWindow = () => inTauri() && getCurrentWindow().minimize();
export const toggleMaximizeWindow = () => inTauri() && getCurrentWindow().toggleMaximize();
export const closeWindow = () => inTauri() && getCurrentWindow().close();
export const startWindowDrag = () => inTauri() && getCurrentWindow().startDragging();

/** In a Markdown viewer (`viewer.html`), the file it was opened on: its name and its text. */
export const viewerFile = () => invoke<{ name: string; text: string }>("viewer_file");

/** The viewers' text size as last set — `null` before any was — and setting it. */
export const viewerTextScale = () => invoke<number | null>("viewer_text_scale_get");
export const setViewerTextScale = (scale: number) => invoke<void>("viewer_text_scale_set", { scale });

/**
 * Whether the window is maximized: `onChange` gets it once now and again after
 * every resize, which is when it can change. Returns the unsubscribe.
 */
export function onMaximizedChange(onChange: (maximized: boolean) => void): () => void {
  if (!inTauri()) return () => {};
  const window = getCurrentWindow();
  const read = () => void window.isMaximized().then(onChange, () => {});
  read();
  const unlisten = window.onResized(read);
  return () => void unlisten.then((stop) => stop());
}

/**
 * Before the window closes — its button, ⌘W, ⌘Q: `keep` resolves to whether
 * it stays open. One that fails lets it close. Returns the unsubscribe.
 */
export function onWindowClose(keep: () => Promise<boolean>): () => void {
  if (!inTauri()) return () => {};
  const unlisten = getCurrentWindow().onCloseRequested(async (event) => {
    if (await keep().catch(() => false)) event.preventDefault();
  });
  return () => void unlisten.then((stop) => stop());
}

/**
 * Whether a right click gets the webview's own menu. Not on the window at
 * large: there it is Reload and Inspect Element, a browser's, not the app's.
 * In a field or the editor it is the platform's Cut, Copy and Paste, and over
 * selected text its Copy — kept. In a dev build ⇧ brings it back anywhere,
 * for the inspector.
 */
export function nativeMenu(e: MouseEvent, dev = import.meta.env.DEV): boolean {
  if (dev && e.shiftKey) return true;
  const target = e.target instanceof Element ? e.target : null;
  if (target?.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return true;
  return !!window.getSelection()?.toString();
}

/** Files held over the window or let go on it — the drop's paths, as the OS gave them. */
export type FileDrop = { type: "over" } | { type: "drop"; paths: string[] } | { type: "leave" };

/**
 * Files dragged onto the window from Finder or a browser. Tauri's own drag and
 * drop rather than the page's: the webview's HTML5 drop does not reach the page
 * on every platform, and this one does. Returns the unsubscribe.
 */
export function onFileDrop(handler: (drop: FileDrop) => void): () => void {
  if (!inTauri()) return () => {};
  const unlisten = getCurrentWindow().onDragDropEvent(({ payload }) => {
    if (payload.type === "enter" || payload.type === "over") handler({ type: "over" });
    else if (payload.type === "drop") handler({ type: "drop", paths: payload.paths });
    else handler({ type: "leave" });
  });
  return () => void unlisten.then((stop) => stop());
}
