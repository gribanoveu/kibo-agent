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
